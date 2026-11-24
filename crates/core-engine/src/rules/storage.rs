use std::collections::{BTreeMap, BTreeSet};

use crate::sql::{OptionalExtension, params};

use super::{
    Action, Child, CollisionPolicy, CompiledRule, Field, Operand, PortableReference, ReferenceKind,
    Rule, RuleDraft, RulePack, Snapshot, input_scale,
};
use crate::{
    domain::{AppError, ErrorCode, Result, normalize},
    ledger_engine::{
        Context, Deleted, PageReq, ReceiptValue, VersionedId, check_version, codec_error, conflict,
        dto, invalid, missing, new_id, revision,
        staging::Stage,
        transactions::{self as tx, EntryKind, LedgerFilter, TxDraft},
    },
};

const RULE_SELECT: &str =
    "SELECT id,name,priority,enabled,apply_existing,overwrite_existing_category,schema_version,
    conditions_json,actions_json,CAST(version AS TEXT) FROM categorization_rules";
const RULE_PACK: &str = "securo-categorization-rules";

dto! {
    #[serde(tag="kind",rename_all_fields="camelCase")]
    pub enum RuleSelection { Draft{draft:RuleDraft}, Saved{ids:Vec<String>} }
    pub struct RuleSample {pub id:String,pub changed_fields:Vec<String>}
    pub struct RulePreview {pub preview_token:String,pub scanned:String,pub matched:String,pub would_change:String,pub samples:Vec<RuleSample>,pub revision:String}
    pub struct RuleApplyResult {pub changed:String,pub matched:String}
    #[derive(PartialEq, Eq)]
    pub enum OverridePolicy { Preserve, AllowExplicit }
    pub struct RulePackResult {pub ids:Vec<String>,pub count:String}
}

pub struct RulePlan {
    selection: RuleSelection,
    day: chrono::NaiveDate,
    override_policy: OverridePolicy,
    ids: Vec<String>,
}

fn decode(row: &crate::sql::Row<'_>) -> crate::sql::Result<Rule> {
    fn from_json<T: serde::de::DeserializeOwned>(value: String) -> crate::sql::Result<T> {
        serde_json::from_str(&value).map_err(|_| crate::sql::Error::InvalidDocument)
    }
    Ok(Rule {
        id: row.get(0)?,
        version: row.get(9)?,
        draft: RuleDraft {
            name: row.get(1)?,
            priority: row.get(2)?,
            enabled: row.get(3)?,
            apply_existing: row.get(4)?,
            overwrite_existing_category: row.get(5)?,
            schema_version: row.get(6)?,
            conditions: from_json(row.get(7)?)?,
            actions: from_json(row.get(8)?)?,
        },
    })
}

pub fn list(ctx: &Context<'_>) -> Result<Vec<Rule>> {
    Ok(ctx
        .tx
        .prepare(&format!("{RULE_SELECT} ORDER BY priority,id"))?
        .query_map([], decode)?
        .collect::<crate::sql::Result<_>>()?)
}
pub fn get(ctx: &Context<'_>, id: &str) -> Result<Rule> {
    ctx.tx
        .query_row(&format!("{RULE_SELECT} WHERE id=?1"), [id], decode)
        .optional()?
        .ok_or_else(|| missing("ruleId"))
}
pub fn save(ctx: &Context<'_>, target: Option<&VersionedId>, draft: &RuleDraft) -> Result<Rule> {
    CompiledRule::new(draft)?;
    let count: i64 = ctx
        .tx
        .query_row("SELECT count(*) FROM categorization_rules", [], |row| {
            row.get(0)
        })?;
    if target.is_none() && count >= 128 {
        return Err(invalid("rule", "rule_count_limit"));
    }
    let id = if let Some(target) = target {
        check_version(&get(ctx, &target.id)?.version, target)?;
        target.id.clone()
    } else {
        new_id()
    };
    ctx.tx.execute("INSERT INTO categorization_rules(id,name,priority,enabled,apply_existing,overwrite_existing_category,schema_version,
        conditions_json,actions_json) VALUES(?1,?2,?3,?4,?5,?6,1,?7,?8)
        ON CONFLICT(id) DO UPDATE SET name=excluded.name,priority=excluded.priority,enabled=excluded.enabled,
        apply_existing=excluded.apply_existing,overwrite_existing_category=excluded.overwrite_existing_category,
        conditions_json=excluded.conditions_json,actions_json=excluded.actions_json,version=categorization_rules.version+1",
        params![id,draft.name.trim(),draft.priority,draft.enabled,draft.apply_existing,draft.overwrite_existing_category,
            serde_json::to_string(&draft.conditions).map_err(codec_error)?,serde_json::to_string(&draft.actions).map_err(codec_error)?])?;
    get(ctx, &id)
}
pub fn remove(ctx: &Context<'_>, id: &str) -> Result<Deleted> {
    if ctx
        .tx
        .execute("DELETE FROM categorization_rules WHERE id=?1", [id])?
        == 0
    {
        return Err(missing("ruleId"));
    }
    Ok(Deleted { id: id.to_owned() })
}
pub fn reorder(ctx: &Context<'_>, ids: &[String]) -> Result<Vec<Rule>> {
    let all = list(ctx)?;
    let current = all
        .iter()
        .map(|rule| rule.id.as_str())
        .collect::<BTreeSet<_>>();
    let provided = ids.iter().map(String::as_str).collect::<BTreeSet<_>>();
    if current != provided || ids.len() != current.len() {
        return Err(invalid("orderedIds", "invalid_rule_order"));
    }
    for (index, id) in ids.iter().enumerate() {
        ctx.tx.execute(
            "UPDATE categorization_rules SET priority=?2 WHERE id=?1",
            params![id, index as i32],
        )?;
    }
    list(ctx)
}

impl ReferenceKind {
    fn columns(&self) -> (&'static str, &'static str) {
        match self {
            Self::Account => (
                "accounts",
                "currency_code || ':' || kind || ':' || coalesce(institution,'')",
            ),
            Self::Category => (
                "categories",
                "kind || ':' || coalesce((SELECT name FROM category_groups g WHERE g.id=categories.group_id),'')",
            ),
            Self::Payee => ("payees", "NULL"),
        }
    }
    fn same(&self, other: &Self) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
}
fn visit_references(
    draft: &mut RuleDraft,
    mut visit: impl FnMut(ReferenceKind, &mut String) -> Result<()>,
) -> Result<()> {
    let mut predicate = |p: &mut super::Predicate| -> Result<()> {
        if let Operand::Text(id) = &mut p.value {
            match p.field {
                Field::AccountId => visit(ReferenceKind::Account, id)?,
                Field::PayeeId => visit(ReferenceKind::Payee, id)?,
                _ => {}
            }
        }
        Ok(())
    };
    for condition in &mut draft.conditions.children {
        match condition {
            Child::Predicate(value) => predicate(value)?,
            Child::Group(group) => {
                for value in &mut group.children {
                    predicate(value)?;
                }
            }
        }
    }
    for action in &mut draft.actions {
        match action {
            Action::Category(id) => visit(ReferenceKind::Category, id)?,
            Action::Payee(id) => visit(ReferenceKind::Payee, id)?,
            _ => {}
        }
    }
    Ok(())
}
fn portable_reference(
    ctx: &Context<'_>,
    entity: ReferenceKind,
    id: &str,
) -> Result<PortableReference> {
    let (table, qualifier) = entity.columns();
    let (name, qualifier) = ctx
        .tx
        .query_row(
            &format!("SELECT name,{qualifier} FROM {table} WHERE id=?1"),
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| conflict("rule_pack_reference_missing"))?;
    Ok(PortableReference {
        entity,
        name,
        qualifier,
    })
}
fn resolve_reference(ctx: &Context<'_>, reference: &PortableReference, id: &str) -> Result<String> {
    let (table, qualifier) = reference.entity.columns();
    let mut stmt = ctx.tx.prepare(&format!(
        "SELECT id,name,{qualifier} FROM {table} ORDER BY id"
    ))?;
    let mut rows = stmt.query([])?;
    let mut matches = Vec::new();
    let reference_name = normalize(&reference.name);
    let reference_qualifier = reference.qualifier.as_deref().map(normalize);
    while let Some(row) = rows.next()? {
        let candidate: String = row.get(0)?;
        let name = row.get_ref(1)?.as_str()?;
        let qualifier: Option<String> = row.get(2)?;
        if normalize(name) == reference_name
            && qualifier.as_deref().map(normalize) == reference_qualifier
        {
            if candidate == id {
                return Ok(candidate);
            }
            matches.push(candidate);
            if matches.len() > 1 {
                return Err(conflict("rule_pack_reference_ambiguous"));
            }
        }
    }
    matches
        .pop()
        .ok_or_else(|| conflict("rule_pack_reference_missing"))
}

pub fn export_pack(ctx: &Context<'_>, ids: Option<&[String]>) -> Result<RulePack> {
    let rules = list(ctx)?;
    if let Some(ids) = ids {
        let unique = ids.iter().collect::<BTreeSet<_>>();
        if ids.len() != unique.len()
            || ids
                .iter()
                .any(|id| !rules.iter().any(|rule| &rule.id == id))
        {
            return Err(invalid("ruleIds", "invalid_rule_selection"));
        }
    }
    let mut rules = rules
        .into_iter()
        .filter(|rule| ids.is_none_or(|ids| ids.contains(&rule.id)))
        .map(|rule| rule.draft)
        .collect::<Vec<_>>();
    let mut references = BTreeMap::<String, PortableReference>::new();
    for rule in &mut rules {
        visit_references(rule, |entity, id| {
            if let Some(existing) = references.get(id) {
                if !existing.entity.same(&entity) {
                    return Err(conflict("rule_pack_reference_kind_conflict"));
                }
            } else {
                references.insert(id.clone(), portable_reference(ctx, entity, id)?);
            }
            Ok(())
        })?;
    }
    Ok(RulePack {
        format: RULE_PACK.to_owned(),
        version: 1,
        rules,
        references,
    })
}
pub fn import_pack(
    ctx: &Context<'_>,
    pack: RulePack,
    policy: &CollisionPolicy,
) -> Result<RulePackResult> {
    if pack.format != RULE_PACK
        || pack.version != 1
        || pack.rules.len() > 128
        || pack.references.len() > 512
    {
        return Err(invalid("pack", "invalid_rule_pack"));
    }
    let existing = list(ctx)?;
    let mut names = BTreeSet::new();
    let mut ids = Vec::new();
    let mut resolved = BTreeMap::<String, String>::new();
    for mut draft in pack.rules {
        CompiledRule::new(&draft)?;
        let key = normalize(&draft.name);
        if !names.insert(key.clone()) {
            return Err(invalid("pack", "duplicate_rule_pack_name"));
        }
        let matching = existing
            .iter()
            .filter(|rule| normalize(&rule.draft.name) == key)
            .collect::<Vec<_>>();
        if matching.len() > 1 {
            return Err(conflict("ambiguous_rule_pack_rule"));
        }
        let target = match (matching.first(), policy) {
            (Some(_), CollisionPolicy::Reject) => return Err(conflict("rule_pack_collision")),
            (Some(_), CollisionPolicy::Skip) => continue,
            (Some(rule), CollisionPolicy::Replace) => Some(VersionedId {
                id: rule.id.clone(),
                version: rule.version.clone(),
            }),
            (None, _) => None,
        };
        visit_references(&mut draft, |entity, id| {
            if let Some(reference) = pack.references.get(id) {
                if !reference.entity.same(&entity) {
                    return Err(conflict("rule_pack_reference_kind_conflict"));
                }
                let mapped = if let Some(mapped) = resolved.get(id) {
                    mapped.clone()
                } else {
                    let mapped = resolve_reference(ctx, reference, id)?;
                    resolved.insert(id.clone(), mapped.clone());
                    mapped
                };
                *id = mapped;
            } else {
                portable_reference(ctx, entity, id)?;
            }
            Ok(())
        })?;
        ids.push(save(ctx, target.as_ref(), &draft)?.id);
    }
    Ok(RulePackResult {
        count: ids.len().to_string(),
        ids,
    })
}

struct Prepared {
    rule: Rule,
    compiled: CompiledRule,
}
fn compile_selection(
    ctx: &Context<'_>,
    input: &RuleSelection,
    commit: bool,
) -> Result<Vec<Prepared>> {
    let selected = match input {
        RuleSelection::Draft { draft } if !commit => vec![Rule {
            id: String::new(),
            version: "0".to_owned(),
            draft: draft.clone(),
        }],
        RuleSelection::Draft { .. } => {
            return Err(invalid("draftOrIds", "save_rule_before_applying"));
        }
        RuleSelection::Saved { ids } => {
            if ids.len() > 128 || ids.iter().collect::<BTreeSet<_>>().len() != ids.len() {
                return Err(invalid("ids", "invalid_rule_selection"));
            }
            let all = list(ctx)?;
            if ids.iter().any(|id| !all.iter().any(|rule| &rule.id == id)) {
                return Err(missing("ruleId"));
            }
            all.into_iter()
                .filter(|rule| {
                    (ids.is_empty() || ids.contains(&rule.id))
                        && rule.draft.enabled
                        && rule.draft.apply_existing
                })
                .collect()
        }
    };
    selected
        .into_iter()
        .map(|rule| {
            Ok(Prepared {
                compiled: CompiledRule::new(&rule.draft)?,
                rule,
            })
        })
        .collect()
}

struct Evaluation {
    draft: TxDraft,
    changed: BTreeSet<String>,
    matched: bool,
    marks: Vec<(String, i64)>,
}
fn evaluate(
    ctx: &Context<'_>,
    original: &TxDraft,
    detail: Option<&tx::TxDetail>,
    category: Option<&str>,
    rules: &[Prepared],
    respect_manual: bool,
    existing: bool,
) -> Result<Evaluation> {
    let payee = original
        .payee_id
        .as_deref()
        .map(|id| {
            ctx.tx
                .query_row("SELECT name FROM payees WHERE id=?1", [id], |row| {
                    row.get::<_, String>(0)
                })
                .optional()
        })
        .transpose()?
        .flatten();
    let snapshot = Snapshot {
        draft: original,
        payee: payee.as_deref(),
        provider_category: category,
        scale: input_scale(ctx, original)?,
    };
    let mut output = Evaluation {
        draft: original.clone(),
        changed: BTreeSet::new(),
        matched: false,
        marks: Vec::new(),
    };
    let id = detail.map(|row| row.row.id.as_str());
    let overrides = if let Some(id) = id.filter(|_| respect_manual) {
        ctx.tx
            .prepare_cached("SELECT field_key FROM transaction_overrides WHERE transaction_id=?1")?
            .query_map([id], |row| row.get::<_, String>(0))?
            .collect::<crate::sql::Result<BTreeSet<_>>>()?
    } else {
        BTreeSet::new()
    };
    for prepared in rules {
        if !prepared.compiled.matches(&snapshot)? {
            continue;
        }
        output.matched = true;
        let protected = |field: &str| overrides.contains(field);
        let linked = detail.is_some_and(|row| row.row.transfer_id.is_some());
        for action in &prepared.rule.draft.actions {
            match action {
                Action::Category(category)
                    if !linked
                        && original.splits.is_empty()
                        && !protected("category_id")
                        && (!existing
                            || original.category_id.is_none()
                            || prepared.rule.draft.overwrite_existing_category) =>
                {
                    if output.draft.category_id.as_ref() != Some(category)
                        && tx::assignable_category(ctx, category).is_ok()
                    {
                        output.draft.category_id = Some(category.clone());
                        output.changed.insert("categoryId".to_owned());
                    }
                }
                Action::Payee(id)
                    if !protected("payee_id") && output.draft.payee_id.as_ref() != Some(id) =>
                {
                    let exists: bool = ctx.tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM payees WHERE id=?1 AND archived=0)",
                        [id],
                        |row| row.get(0),
                    )?;
                    if exists {
                        output.draft.payee_id = Some(id.clone());
                        output.changed.insert("payeeId".to_owned());
                    }
                }
                Action::Description(text)
                    if !protected("description") && output.draft.description != *text =>
                {
                    output.draft.description = text.clone();
                    output.changed.insert("description".to_owned());
                }
                Action::AppendNotes(text) if !protected("notes") => {
                    let version = prepared
                        .rule
                        .version
                        .parse::<i64>()
                        .map_err(|_| invalid("rule", "invalid_rule_version"))?;
                    let applied = if let Some(id) = id {
                        ctx.tx.query_row("SELECT EXISTS(SELECT 1 FROM rule_applications WHERE rule_id=?1 AND rule_version=?2 AND transaction_id=?3)",
                            params![prepared.rule.id,version,id],|row|row.get::<_,bool>(0))?
                    } else {
                        false
                    };
                    if !applied
                        && output.draft.notes.chars().count()
                            + text.chars().count()
                            + usize::from(!output.draft.notes.is_empty())
                            <= 1000
                    {
                        if !output.draft.notes.is_empty() {
                            output.draft.notes.push('\n');
                        }
                        output.draft.notes.push_str(text);
                        output.changed.insert("notes".to_owned());
                        if !prepared.rule.id.is_empty()
                            && !output.marks.iter().any(|(id, _)| id == &prepared.rule.id)
                        {
                            output.marks.push((prepared.rule.id.clone(), version));
                        }
                    }
                }
                Action::Ignore(ignore)
                    if !linked && !protected("ignored") && output.draft.ignored != *ignore =>
                {
                    output.draft.ignored = *ignore;
                    output.changed.insert("ignored".to_owned());
                }
                _ => {}
            }
        }
    }
    if output.draft.category_id == original.category_id {
        output.changed.remove("categoryId");
    }
    if output.draft.payee_id == original.payee_id {
        output.changed.remove("payeeId");
    }
    if output.draft.description == original.description {
        output.changed.remove("description");
    }
    if output.draft.ignored == original.ignored {
        output.changed.remove("ignored");
    }
    Ok(output)
}

pub struct ImportRules(Vec<Prepared>);
impl ImportRules {
    pub fn load(ctx: &Context<'_>) -> Result<Self> {
        Ok(Self(
            list(ctx)?
                .into_iter()
                .filter(|rule| rule.draft.enabled)
                .map(|rule| {
                    Ok(Prepared {
                        compiled: CompiledRule::new(&rule.draft)?,
                        rule,
                    })
                })
                .collect::<Result<_>>()?,
        ))
    }
    pub fn categorize(
        &self,
        ctx: &Context<'_>,
        draft: &TxDraft,
        category: Option<&str>,
    ) -> Result<TxDraft> {
        Ok(evaluate(ctx, draft, None, category, &self.0, true, false)?.draft)
    }
}

fn source_category(ctx: &Context<'_>, id: &str) -> Result<Option<String>> {
    Ok(ctx.tx.query_row("SELECT provider_category FROM transaction_import_keys WHERE transaction_id=?1 ORDER BY id DESC LIMIT 1",
        [id],|row|row.get::<_,Option<String>>(0)).optional()?.flatten())
}

pub fn preview(
    ctx: &Context<'_>,
    store: &Stage<RulePlan>,
    selection: RuleSelection,
    filter: LedgerFilter,
    sample_limit: u16,
    override_policy: OverridePolicy,
    cancelled: &dyn Fn() -> bool,
) -> Result<RulePreview> {
    if !(1..=200).contains(&sample_limit) {
        return Err(invalid("sampleLimit", "invalid_rule_sample_limit"));
    }
    let compiled = compile_selection(ctx, &selection, false)?;
    let mut page = PageReq {
        cursor: None,
        limit: 200,
    };
    let mut scanned = 0usize;
    let mut count = 0usize;
    let mut changed = 0usize;
    let mut samples = Vec::new();
    let mut ids = Vec::new();
    loop {
        let results = tx::list(ctx, &filter, &page, false)?;
        for item in results.rows {
            if cancelled() {
                return Err(AppError::new(ErrorCode::Cancelled, "operation_cancelled"));
            }
            if item.entry_kind != EntryKind::Ordinary {
                continue;
            }
            scanned += 1;
            if scanned > 50_000 {
                return Err(invalid("filter", "rule_preview_scope_too_large"));
            }
            let detail = tx::get(ctx, &item.id)?;
            let result = evaluate(
                ctx,
                &detail.draft,
                Some(&detail),
                source_category(ctx, &item.id)?.as_deref(),
                &compiled,
                matches!(override_policy, OverridePolicy::Preserve),
                true,
            )?;
            if result.matched {
                count += 1;
                ids.push(item.id.clone());
                changed += usize::from(!result.changed.is_empty());
                if samples.len() < usize::from(sample_limit) {
                    samples.push(RuleSample {
                        id: item.id,
                        changed_fields: result.changed.into_iter().collect(),
                    });
                }
            }
        }
        page.cursor = results.next_cursor;
        if page.cursor.is_none() {
            break;
        }
    }
    let preview_token = store.put(
        ctx,
        RulePlan {
            selection,
            day: ctx.today,
            override_policy,
            ids,
        },
    )?;
    Ok(RulePreview {
        preview_token,
        scanned: scanned.to_string(),
        matched: count.to_string(),
        would_change: changed.to_string(),
        samples,
        revision: revision(&ctx.tx)?,
    })
}

pub fn apply(
    ctx: &Context<'_>,
    store: &Stage<RulePlan>,
    token: &str,
    policy: &OverridePolicy,
    cancelled: &dyn Fn() -> bool,
) -> Result<RuleApplyResult> {
    let plan = store.get(ctx, token)?;
    if &plan.override_policy != policy {
        return Err(conflict("rule_preview_override_policy_changed"));
    }
    // A scope changing at midnight must be previewed again for date-dependent filters.
    if plan.day != ctx.today {
        return Err(crate::ledger_engine::stale_revision());
    }
    let compiled = compile_selection(ctx, &plan.selection, true)?;
    let mut matched = 0usize;
    let mut changed = 0usize;
    for id in &plan.ids {
        if cancelled() {
            return Err(AppError::new(ErrorCode::Cancelled, "operation_cancelled"));
        }
        let detail = tx::get(ctx, id)?;
        let result = evaluate(
            ctx,
            &detail.draft,
            Some(&detail),
            source_category(ctx, id)?.as_deref(),
            &compiled,
            matches!(policy, OverridePolicy::Preserve),
            true,
        )?;
        matched += usize::from(result.matched);
        if result.changed.is_empty() {
            continue;
        }
        changed += 1;
        ctx.tx.execute("UPDATE transactions SET category_id=?2,payee_id=?3,description=?4,notes=?5,ignored=?6,version=version+1 WHERE id=?1",
            params![id,result.draft.category_id,result.draft.payee_id,result.draft.description,result.draft.notes,result.draft.ignored])?;
        for (rule, version) in result.marks {
            ctx.tx.execute(
                "INSERT OR IGNORE INTO rule_applications VALUES(?1,?2,?3)",
                params![rule, version, id],
            )?;
        }
        tx::project(ctx, id)?;
    }
    Ok(RuleApplyResult {
        changed: changed.to_string(),
        matched: matched.to_string(),
    })
}

impl ReceiptValue for Rule {
    type Reference = String;
    fn reference(&self, _: &Context<'_>) -> String {
        self.id.clone()
    }
    fn reload(ctx: &Context<'_>, id: String) -> Result<Self> {
        get(ctx, &id)
    }
}
impl ReceiptValue for Vec<Rule> {
    type Reference = ();
    fn reference(&self, _: &Context<'_>) {}
    fn reload(ctx: &Context<'_>, _: ()) -> Result<Self> {
        list(ctx)
    }
}
macro_rules! result_receipt {
    ($($kind:ty),*)=>{$(
        impl ReceiptValue for $kind {
            type Reference=Self;
            const NEEDS_SAME_REVISION:bool=false;
            fn reference(&self,_:&Context<'_>)->Self {self.clone()}
            fn reload(_:&Context<'_>,value:Self)->Result<Self> {Ok(value)}
        }
    )*};
}
result_receipt!(RuleApplyResult, RulePackResult);
