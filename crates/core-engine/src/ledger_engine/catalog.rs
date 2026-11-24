use crate::sql::{OptionalExtension, params, params_from_iter};
use std::collections::{BTreeSet, HashSet};

use super::{
    accounts,
    transactions::{self as tx, Query},
    *,
};

dto! {
    pub struct CategoryGroupDraft { pub name: String, pub sort_order: i32 }
    pub struct CategoryGroup { pub id: String, pub version: String, pub draft: CategoryGroupDraft }
    #[derive(PartialEq, Eq)]
    pub enum CategoryKind { Income, Expense, Transfer }
    pub struct CategoryDraft {
        pub name: String, pub group_id: Option<String>, pub kind: CategoryKind,
        #[serde(default, skip_serializing_if = "Option::is_none")] pub icon: Option<String>,
        #[serde(default)] pub hidden: bool, #[serde(default)] pub ignored: bool, #[serde(default)] pub archived: bool,
    }
    pub struct Category { pub id: String, pub version: String, pub draft: CategoryDraft }
    pub struct PayeeDraft { pub name: String, #[serde(default)] pub archived: bool }
    pub struct Payee { pub id: String, pub version: String, pub draft: PayeeDraft }
    pub struct ReferenceChange { pub removed_ids: Vec<String>, pub replacement_id: Option<String>, pub changed_transaction_ids: Vec<String> }
}
impl CategoryKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Income => "income",
            Self::Expense => "expense",
            Self::Transfer => "transfer",
        }
    }
    pub(super) fn read(s: &str) -> crate::sql::Result<Self> {
        match s {
            "income" => Ok(Self::Income),
            "expense" => Ok(Self::Expense),
            "transfer" => Ok(Self::Transfer),
            _ => Err(crate::sql::Error::InvalidQuery),
        }
    }
}

pub(super) fn group_get(ctx: &Context<'_>, id: &str) -> Result<CategoryGroup> {
    let draft = ctx
        .tx
        .query_row(
            "SELECT name,sort_order FROM category_groups WHERE id=?1",
            [id],
            |r| {
                Ok(CategoryGroupDraft {
                    name: r.get(0)?,
                    sort_order: r.get(1)?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| missing("categoryGroupId"))?;
    Ok(CategoryGroup {
        id: id.to_owned(),
        version: revision(&ctx.tx)?,
        draft,
    })
}

pub(super) fn category_get(ctx: &Context<'_>, id: &str) -> Result<Category> {
    let draft = ctx
        .tx
        .query_row(
            "SELECT name,group_id,kind,hidden,ignored,archived,icon FROM categories WHERE id=?1",
            [id],
            |r| {
                Ok(CategoryDraft {
                    name: r.get(0)?,
                    group_id: r.get(1)?,
                    kind: CategoryKind::read(r.get_ref(2)?.as_str()?)?,
                    hidden: r.get(3)?,
                    ignored: r.get(4)?,
                    archived: r.get(5)?,
                    icon: r.get(6)?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| missing("categoryId"))?;
    Ok(Category {
        id: id.to_owned(),
        version: revision(&ctx.tx)?,
        draft,
    })
}

pub(super) fn payee_get(ctx: &Context<'_>, id: &str) -> Result<Payee> {
    let draft = ctx
        .tx
        .query_row("SELECT name,archived FROM payees WHERE id=?1", [id], |r| {
            Ok(PayeeDraft {
                name: r.get(0)?,
                archived: r.get(1)?,
            })
        })
        .optional()?
        .ok_or_else(|| missing("payeeId"))?;
    Ok(Payee {
        id: id.to_owned(),
        version: revision(&ctx.tx)?,
        draft,
    })
}

pub fn groups(
    ctx: &Context<'_>,
    query: Option<&str>,
    _: bool,
    page: &PageReq,
) -> Result<Page<CategoryGroup>> {
    let cursor = ReadPage::new(ctx, page, &("category_groups", query))?;
    let mut sql = Query::new("SELECT id,name,sort_order FROM category_groups WHERE 1=1".to_owned());
    search(&mut sql, "name", query)?;
    tx::sort_cursor(&mut sql, &cursor, "sort_order", "id", true, true)?;
    sql.limit(cursor.limit + 1);
    let version = revision(&ctx.tx)?;
    let rows = ctx
        .tx
        .prepare(&sql.sql)?
        .query_map(params_from_iter(sql.args), |r| {
            Ok(CategoryGroup {
                id: r.get(0)?,
                version: version.clone(),
                draft: CategoryGroupDraft {
                    name: r.get(1)?,
                    sort_order: r.get(2)?,
                },
            })
        })?
        .collect::<crate::sql::Result<_>>()?;
    cursor.finish(rows, |item| {
        (item.draft.sort_order.to_string(), item.id.clone())
    })
}
pub fn categories(
    ctx: &Context<'_>,
    query: Option<&str>,
    archived: bool,
    page: &PageReq,
) -> Result<Page<Category>> {
    let cursor = ReadPage::new(ctx, page, &("categories", query, archived))?;
    let mut sql = Query::new(
        "SELECT id,name,group_id,kind,hidden,ignored,archived,icon FROM categories WHERE 1=1"
            .to_owned(),
    );
    if !archived {
        sql.and("archived=0");
    }
    search(&mut sql, "name", query)?;
    tx::sort_cursor(&mut sql, &cursor, "name", "id", false, true)?;
    sql.limit(cursor.limit + 1);
    let version = revision(&ctx.tx)?;
    let rows = ctx
        .tx
        .prepare(&sql.sql)?
        .query_map(params_from_iter(sql.args), |r| {
            Ok(Category {
                id: r.get(0)?,
                version: version.clone(),
                draft: CategoryDraft {
                    name: r.get(1)?,
                    group_id: r.get(2)?,
                    kind: CategoryKind::read(r.get_ref(3)?.as_str()?)?,
                    hidden: r.get(4)?,
                    ignored: r.get(5)?,
                    archived: r.get(6)?,
                    icon: r.get(7)?,
                },
            })
        })?
        .collect::<crate::sql::Result<_>>()?;
    cursor.finish(rows, |item| (item.draft.name.clone(), item.id.clone()))
}
pub fn payees(
    ctx: &Context<'_>,
    query: Option<&str>,
    archived: bool,
    page: &PageReq,
) -> Result<Page<Payee>> {
    let cursor = ReadPage::new(ctx, page, &("payees", query, archived))?;
    let mut sql =
        Query::new("SELECT id,name,archived,normalized_name FROM payees WHERE 1=1".to_owned());
    if !archived {
        sql.and("archived=0");
    }
    search(&mut sql, "normalized_name", query.map(normalize).as_deref())?;
    tx::sort_cursor(&mut sql, &cursor, "normalized_name", "id", false, true)?;
    sql.limit(cursor.limit + 1);
    let version = revision(&ctx.tx)?;
    let rows = ctx
        .tx
        .prepare(&sql.sql)?
        .query_map(params_from_iter(sql.args), |r| {
            Ok((
                r.get::<_, String>(3)?,
                Payee {
                    id: r.get(0)?,
                    version: version.clone(),
                    draft: PayeeDraft {
                        name: r.get(1)?,
                        archived: r.get(2)?,
                    },
                },
            ))
        })?
        .collect::<crate::sql::Result<_>>()?;
    let page = cursor.finish(rows, |(key, item)| (key.clone(), item.id.clone()))?;
    Ok(Page {
        rows: page.rows.into_iter().map(|(_, row)| row).collect(),
        next_cursor: page.next_cursor,
        revision: page.revision,
    })
}
fn search(sql: &mut Query, column: &str, query: Option<&str>) -> Result<()> {
    if let Some(value) = query
        && !value.trim().is_empty()
    {
        if value.len() > 512 {
            return Err(invalid("query", "search_too_long"));
        }
        let term = sql.bind(normalize(value));
        sql.and(&format!("instr(financial_normalize({column}),{term})>0"));
    }
    Ok(())
}
fn exists(ctx: &Context<'_>, table: &str, id: &str) -> Result<()> {
    // The table argument comes exclusively from this module's fixed catalog table names.
    if !ctx.tx.query_row(
        &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE id=?1)"),
        [id],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(missing("id"));
    }
    Ok(())
}
fn id_for(ctx: &Context<'_>, table: &str, target: Option<&VersionedId>) -> Result<String> {
    if let Some(target) = target {
        check_revision(ctx, target)?;
        exists(ctx, table, &target.id)?;
        Ok(target.id.clone())
    } else {
        Ok(new_id())
    }
}
pub fn group_save(
    ctx: &Context<'_>,
    target: Option<&VersionedId>,
    draft: &CategoryGroupDraft,
) -> Result<CategoryGroup> {
    let id = id_for(ctx, "category_groups", target)?;
    let mut draft = draft.clone();
    draft.name = name(&draft.name)?.to_owned();
    ctx.tx.execute("INSERT INTO category_groups VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET name=excluded.name,sort_order=excluded.sort_order",
        params![id,draft.name,draft.sort_order])?;
    Ok(CategoryGroup {
        id,
        version: accounts::next_version(ctx)?,
        draft,
    })
}
pub fn category_save(
    ctx: &Context<'_>,
    target: Option<&VersionedId>,
    draft: &CategoryDraft,
) -> Result<Category> {
    let id = id_for(ctx, "categories", target)?;
    let mut draft = draft.clone();
    draft.name = name(&draft.name)?.to_owned();
    if let Some(group) = &draft.group_id {
        exists(ctx, "category_groups", group)?;
    }
    if let Some(value) = draft.icon.as_mut() {
        let id = value.trim();
        if id.len() > 64
            || !id.split('-').all(|segment| {
                !segment.is_empty() && segment.bytes().all(|byte| byte.is_ascii_alphanumeric())
            })
        {
            return Err(invalid("icon", "invalid_category_icon"));
        }
        *value = id.to_owned();
    }
    ctx.tx.execute("INSERT INTO categories(id,group_id,name,kind,hidden,ignored,archived,icon)
        VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(id) DO UPDATE SET
        group_id=excluded.group_id,name=excluded.name,kind=excluded.kind,hidden=excluded.hidden,ignored=excluded.ignored,archived=excluded.archived,icon=excluded.icon",
        params![id,draft.group_id,draft.name,draft.kind.as_str(),draft.hidden,draft.ignored,draft.archived,draft.icon])?;
    Ok(Category {
        id,
        version: accounts::next_version(ctx)?,
        draft,
    })
}
pub fn payee_save(
    ctx: &Context<'_>,
    target: Option<&VersionedId>,
    draft: &PayeeDraft,
) -> Result<Payee> {
    let id = id_for(ctx, "payees", target)?;
    let mut draft = draft.clone();
    draft.name = name(&draft.name)?.to_owned();
    let normalized = normalize(&draft.name);
    if normalized.is_empty() {
        return Err(invalid("name", "invalid_name"));
    }
    if ctx.tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM payees WHERE normalized_name=?1 AND id<>?2)",
        params![normalized, id],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(AppError::new(ErrorCode::Duplicate, "payee_name_exists"));
    }
    ctx.tx.execute(
        "INSERT INTO payees VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET
        name=excluded.name,normalized_name=excluded.normalized_name,archived=excluded.archived",
        params![id, draft.name, normalized, draft.archived],
    )?;
    if target.is_some() {
        for posting in accounts::strings(
            &ctx.tx,
            "SELECT id FROM transactions WHERE payee_id=?1",
            &id,
        )? {
            tx::project(ctx, &posting)?;
        }
    }
    Ok(Payee {
        id,
        version: accounts::next_version(ctx)?,
        draft,
    })
}
fn replacement(ctx: &Context<'_>, table: &str, id: &str, replacement: Option<&str>) -> Result<()> {
    exists(ctx, table, id)?;
    if let Some(other) = replacement {
        if other == id {
            return Err(invalid("replacementId", "distinct_replacement_required"));
        }
        exists(ctx, table, other)?;
    }
    Ok(())
}
fn require_confirm(confirmed: bool, count: usize) -> Result<()> {
    if !confirmed && count != 0 {
        return Err(invalid("confirm", "confirmation_required"));
    }
    Ok(())
}
pub fn group_delete(
    ctx: &Context<'_>,
    id: &str,
    replace: Option<&str>,
    confirm: bool,
) -> Result<ReferenceChange> {
    replacement(ctx, "category_groups", id, replace)?;
    let categories = accounts::strings(&ctx.tx, "SELECT id FROM categories WHERE group_id=?1", id)?;
    require_confirm(confirm, categories.len())?;
    ctx.tx.execute(
        "UPDATE categories SET group_id=?2 WHERE group_id=?1",
        params![id, replace],
    )?;
    ctx.tx
        .execute("DELETE FROM category_groups WHERE id=?1", [id])?;
    Ok(ReferenceChange {
        removed_ids: vec![id.to_owned()],
        replacement_id: replace.map(str::to_owned),
        changed_transaction_ids: Vec::new(),
    })
}
pub fn category_delete(
    ctx: &Context<'_>,
    id: &str,
    replace: Option<&str>,
    confirm: bool,
) -> Result<ReferenceChange> {
    replacement(ctx, "categories", id, replace)?;
    if let Some(replace) = replace {
        tx::assignable_category(ctx, replace)?;
    }
    let single = accounts::strings(
        &ctx.tx,
        "SELECT id FROM transactions WHERE category_id=?1",
        id,
    )?;
    let split = accounts::strings(
        &ctx.tx,
        "SELECT DISTINCT transaction_id FROM transaction_splits WHERE category_id=?1",
        id,
    )?;
    let planning: bool = ctx.tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM budgets WHERE category_id=?1)
        OR EXISTS(SELECT 1 FROM budget_defaults WHERE category_id=?1)
        OR EXISTS(SELECT 1 FROM recurring_rule_splits WHERE category_id=?1)
        OR EXISTS(SELECT 1 FROM recurring_rules WHERE category_id=?1)",
        [id],
        |r| r.get(0),
    )?;
    if planning {
        return Err(conflict("category_has_planning_dependencies"));
    }
    require_confirm(confirm, single.len() + split.len())?;
    if !split.is_empty() && replace.is_none() {
        return Err(conflict("split_category_replacement_required"));
    }
    for parent in &split {
        ctx.tx.execute(
            "UPDATE transactions SET assembly_state=0 WHERE id=?1",
            [parent],
        )?;
        ctx.tx.execute("UPDATE transaction_splits SET category_id=?3 WHERE transaction_id=?1 AND category_id=?2",params![parent,id,replace])?;
        ctx.tx.execute(
            "UPDATE transactions SET assembly_state=1,version=version+1 WHERE id=?1",
            [parent],
        )?;
        tx::mark_override(ctx, parent, "category_id")?;
    }
    ctx.tx.execute(
        "UPDATE transactions SET category_id=?2,version=version+1 WHERE category_id=?1",
        params![id, replace],
    )?;
    for parent in &single {
        tx::mark_override(ctx, parent, "category_id")?;
    }
    ctx.tx.execute("DELETE FROM categories WHERE id=?1", [id])?;
    Ok(ReferenceChange {
        removed_ids: vec![id.to_owned()],
        replacement_id: replace.map(str::to_owned),
        changed_transaction_ids: single
            .into_iter()
            .chain(split)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
    })
}
pub fn payee_delete(
    ctx: &Context<'_>,
    id: &str,
    replace: Option<&str>,
    confirm: bool,
) -> Result<ReferenceChange> {
    payee_rewrite(ctx, &[id.to_owned()], replace, confirm)
}
pub fn payee_merge(
    ctx: &Context<'_>,
    from: &[String],
    into: &str,
    confirm: bool,
) -> Result<ReferenceChange> {
    if !confirm {
        return Err(invalid("confirm", "confirmation_required"));
    }
    payee_rewrite(ctx, from, Some(into), confirm)
}
fn payee_rewrite(
    ctx: &Context<'_>,
    from: &[String],
    replace: Option<&str>,
    confirm: bool,
) -> Result<ReferenceChange> {
    if from.is_empty()
        || from.len() > 200
        || from.iter().collect::<HashSet<_>>().len() != from.len()
    {
        return Err(invalid("fromIds", "invalid_payee_selection"));
    }
    if let Some(id) = replace
        && !ctx
            .tx
            .query_row("SELECT archived=0 FROM payees WHERE id=?1", [id], |r| {
                r.get::<_, bool>(0)
            })
            .optional()?
            .unwrap_or(false)
    {
        return Err(invalid("replacementId", "unassignable_payee"));
    }
    let mut posted = BTreeSet::new();
    for id in from {
        replacement(ctx, "payees", id, replace)?;
        let refs = accounts::strings(&ctx.tx, "SELECT id FROM transactions WHERE payee_id=?1", id)?;
        let recurring: bool = ctx.tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM recurring_rules WHERE payee_id=?1)",
            [id],
            |r| r.get(0),
        )?;
        require_confirm(confirm, refs.len() + usize::from(recurring))?;
        ctx.tx.execute(
            "UPDATE transactions SET payee_id=?2,version=version+1 WHERE payee_id=?1",
            params![id, replace],
        )?;
        ctx.tx.execute(
            "UPDATE recurring_rules SET payee_id=?2,version=version+1 WHERE payee_id=?1",
            params![id, replace],
        )?;
        ctx.tx.execute("DELETE FROM payees WHERE id=?1", [id])?;
        posted.extend(refs);
    }
    for id in &posted {
        tx::mark_override(ctx, id, "payee_id")?;
        tx::project(ctx, id)?;
    }
    Ok(ReferenceChange {
        removed_ids: from.to_vec(),
        replacement_id: replace.map(str::to_owned),
        changed_transaction_ids: posted.into_iter().collect(),
    })
}
