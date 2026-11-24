use crate::sql::{OptionalExtension, params};
use chrono::Datelike;
use parking_lot::Mutex;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;
use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};
#[cfg(target_arch = "wasm32")]
use web_time::Instant;

use super::{
    accounts::month_shift,
    transactions::{self as tx, EntryKind, InstallmentRef, LedgerFilter, TxDetail, TxDraft},
    *,
};

dto! {
    #[serde(tag = "kind", rename_all_fields = "camelCase")]
    pub enum IdsOrFilter { Ids { ids: Vec<String> }, Filter { filter: Box<LedgerFilter> } }
    #[serde(tag = "kind", rename_all_fields = "camelCase")]
    pub enum BulkAction {
        Categorize { category_id: Option<String> },
        AddTag { tag: String }, RemoveTag { tag: String },
        Flag { flagged: bool, due_date: Option<String> }, AssignGroup { group_id: String }, Delete,
    }
    pub struct PreviewItem { pub id: String, pub description: String, pub blocked: Option<String> }
    pub struct MutationPreview { pub preview_token:String, pub rows:Vec<PreviewItem>, pub revision:String, pub requires_confirmation:bool }
    pub struct BulkResult { pub changed_ids: Vec<String>, pub skipped_ids: Vec<String> }

    #[derive(PartialEq, Eq)]
    pub enum InstallmentScope { This, Future, All }
    pub struct InstallmentDraft {
        pub posting: TxDraft, pub purchase_date: String, pub occurrence_count: u16, pub anchor_id: Option<String>,
    }
    pub struct InstallmentOccurrence { pub existing_id:Option<String>, pub number:u16, pub bank_date:String, pub amount:Money }
    pub struct InstallmentPreview {
        pub preview_token:String, pub rows:Vec<InstallmentOccurrence>, pub revision:String,
    }
    pub struct InstallmentDetail { pub series_id:String, pub transactions:Vec<tx::TxRow> }
}

struct Preview {
    scope: Scope,
    window: String,
    revision: String,
    expires: Instant,
    plan: Plan,
}
#[derive(Clone)]
enum Plan {
    Bulk {
        selected: Vec<VersionedId>,
        skipped: Vec<String>,
        action: BulkAction,
    },
    Installment(InstallmentPlan),
}
#[derive(Clone)]
struct InstallmentPlan {
    series: String,
    create: bool,
    count: u16,
    purchase: String,
    postings: Vec<(Option<VersionedId>, TxDraft)>,
}
#[derive(Default)]
pub struct PreviewStore(Mutex<HashMap<String, Preview>>);

impl PreviewStore {
    fn put(&self, ctx: &Context<'_>, window: &str, plan: Plan) -> Result<(String, String)> {
        let now = Instant::now();
        let mut items = self.0.lock();
        items.retain(|_, entry| entry.expires > now);
        if items.len() >= 16 {
            return Err(AppError::busy());
        }
        let token = new_id();
        let revision = revision(&ctx.tx)?;
        items.insert(
            token.clone(),
            Preview {
                scope: ctx.scope.clone(),
                window: window.to_owned(),
                revision: revision.clone(),
                expires: now + Duration::from_secs(180),
                plan,
            },
        );
        Ok((token, revision))
    }
    fn get(&self, ctx: &Context<'_>, window: &str, token: &str) -> Result<Plan> {
        let items = self.0.lock();
        let entry = items
            .get(token)
            .filter(|entry| {
                entry.window == window
                    && entry.scope == *ctx.scope
                    && entry.expires > Instant::now()
            })
            .ok_or_else(|| invalid("previewToken", "invalid_or_expired_preview"))?;
        if entry.revision != revision(&ctx.tx)? {
            return Err(stale_revision());
        }
        Ok(entry.plan.clone())
    }
    pub fn assigns_group(&self, ctx: &Context<'_>, window: &str, token: &str) -> Result<bool> {
        Ok(matches!(
            self.get(ctx, window, token)?,
            Plan::Bulk {
                action: BulkAction::AssignGroup { .. },
                ..
            }
        ))
    }
    pub fn invalidate(&self, window: &str) {
        self.0.lock().retain(|_, entry| entry.window != window);
    }
}

fn validated_tag(value: &str) -> Result<String> {
    let tag = format!("#{}", value.trim().trim_start_matches('#'));
    if tag.chars().count() > 81
        || tx::hashtag_spans(&tag)
            .first()
            .is_none_or(|(_, end, _)| *end != tag.len())
    {
        return Err(invalid("tag", "invalid_tag"));
    }
    Ok(tag)
}
fn notes_after(notes: &str, value: &str, add: bool) -> Result<String> {
    let tag = validated_tag(value)?;
    let normalized = normalize(&tag[1..]);
    let existing = tx::hashtag_spans(notes);
    let mut result = notes.to_owned();
    if add && !existing.iter().any(|(_, _, tag)| *tag == normalized) {
        if !notes.is_empty() && !notes.ends_with(char::is_whitespace) {
            result.push(' ');
        }
        result.push_str(&tag);
    } else if !add {
        for (start, end, _) in existing
            .into_iter()
            .rev()
            .filter(|(_, _, tag)| *tag == normalized)
        {
            result.replace_range(start..end, "");
        }
    }
    if result.chars().count() > 1000 {
        return Err(invalid("notes", "notes_too_long"));
    }
    Ok(result)
}
fn group_self(ctx: &Context<'_>, group: &str) -> Result<String> {
    ctx.tx
        .query_row(
            "SELECT m.id FROM shared_group_members m JOIN shared_groups g ON g.id=m.group_id
        WHERE m.group_id=?1 AND m.is_self=1 AND g.archived=0",
            [group],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| invalid("groupId", "active_group_with_self_required"))
}
fn eligible(ctx: &Context<'_>, detail: &TxDetail, action: &BulkAction) -> Result<()> {
    match action {
        BulkAction::Categorize { .. }
            if detail.row.entry_kind != EntryKind::Ordinary
                || !detail.draft.splits.is_empty()
                || detail.row.transfer_id.is_some() =>
        {
            Err(conflict("posting_cannot_be_bulk_categorized"))
        }
        BulkAction::AddTag { tag } | BulkAction::RemoveTag { tag } => {
            notes_after(
                &detail.draft.notes,
                tag,
                matches!(action, BulkAction::AddTag { .. }),
            )?;
            Ok(())
        }
        BulkAction::AssignGroup { .. }
            if detail.row.entry_kind != EntryKind::Ordinary
                || !matches!(detail.row.direction, Direction::Debit)
                || detail.row.transfer_id.is_some()
                || detail.shared_expense_id.is_some() =>
        {
            Err(conflict("posting_cannot_be_assigned_to_group"))
        }
        BulkAction::Delete => {
            tx::check_delete(ctx, &detail.row.id, false)?;
            Ok(())
        }
        _ => Ok(()),
    }
}
pub fn assign_group(ctx: &Context<'_>, item: &TxDetail, group_id: &str) -> Result<()> {
    eligible(
        ctx,
        item,
        &BulkAction::AssignGroup {
            group_id: group_id.to_owned(),
        },
    )?;
    let owner = group_self(ctx, group_id)?;
    let id = new_id();
    let amount = positive(&item.row.amount.minor)?;
    ctx.tx.execute(
        "INSERT INTO shared_expenses VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'exact')",
        params![
            id,
            group_id,
            owner,
            item.row.id,
            item.row.bank_date,
            item.row.description,
            item.row.amount.currency,
            amount
        ],
    )?;
    ctx.tx.execute(
        "INSERT INTO shared_allocations VALUES(?1,?2,?3,?4,NULL,1)",
        params![id, group_id, owner, amount],
    )?;
    ctx.tx.execute(
        "UPDATE transactions SET version=version+1 WHERE id=?1",
        [&item.row.id],
    )?;
    Ok(())
}
pub fn bulk_preview(
    ctx: &Context<'_>,
    store: &PreviewStore,
    window: &str,
    selection: &IdsOrFilter,
    action: &BulkAction,
) -> Result<MutationPreview> {
    match action {
        BulkAction::Categorize {
            category_id: Some(id),
        } => tx::assignable_category(ctx, id)?,
        BulkAction::AddTag { tag } | BulkAction::RemoveTag { tag } => {
            validated_tag(tag)?;
        }
        BulkAction::Flag { flagged, due_date } => {
            date_option(due_date.as_deref())?;
            if !*flagged && due_date.is_some() {
                return Err(invalid("dueDate", "flag_required"));
            }
        }
        BulkAction::AssignGroup { group_id } => {
            group_self(ctx, group_id)?;
        }
        _ => {}
    }
    let ids = match selection {
        IdsOrFilter::Ids { ids } => {
            if ids.is_empty()
                || ids.len() > 5000
                || ids.iter().collect::<HashSet<_>>().len() != ids.len()
            {
                return Err(invalid("selection", "invalid_bulk_selection"));
            }
            ids.clone()
        }
        IdsOrFilter::Filter { filter } => {
            let mut ids = Vec::new();
            let mut input = PageReq {
                cursor: None,
                limit: 200,
            };
            loop {
                let page = tx::list(ctx, filter, &input, false)?;
                ids.extend(page.rows.into_iter().map(|row| row.id));
                if ids.len() > 5000 {
                    return Err(invalid("selection", "bulk_selection_too_large"));
                }
                if page.next_cursor.is_none() {
                    break;
                }
                input.cursor = page.next_cursor;
            }
            ids
        }
    };
    let mut selected = Vec::new();
    let mut skipped = Vec::new();
    let mut rows = Vec::new();
    for id in ids {
        let detail = tx::get(ctx, &id)?;
        let blocked = match eligible(ctx, &detail, action) {
            Ok(()) => {
                selected.push(VersionedId {
                    id: id.clone(),
                    version: detail.row.version,
                });
                None
            }
            Err(error) if matches!(error.code, ErrorCode::Validation | ErrorCode::Conflict) => {
                skipped.push(id.clone());
                Some(error.message_key.to_owned())
            }
            Err(error) => return Err(error),
        };
        rows.push(PreviewItem {
            id,
            description: detail.row.description,
            blocked,
        });
    }
    let requires_confirmation = matches!(action, BulkAction::Delete);
    let (preview_token, revision) = store.put(
        ctx,
        window,
        Plan::Bulk {
            selected,
            skipped,
            action: action.clone(),
        },
    )?;
    Ok(MutationPreview {
        preview_token,
        rows,
        revision,
        requires_confirmation,
    })
}
pub fn bulk_apply(
    ctx: &Context<'_>,
    store: &PreviewStore,
    window: &str,
    token: &str,
    confirmed: bool,
) -> Result<BulkResult> {
    let Plan::Bulk {
        selected,
        skipped,
        action,
    } = store.get(ctx, window, token)?
    else {
        return Err(invalid("previewToken", "preview_purpose_mismatch"));
    };
    if matches!(action, BulkAction::Delete) && !confirmed {
        return Err(invalid("confirm", "confirmation_required"));
    }
    let mut changed_ids = Vec::new();
    for target in selected {
        let item = tx::get(ctx, &target.id)?;
        check_version(&item.row.version, &target)?;
        eligible(ctx, &item, &action)?;
        match &action {
            BulkAction::Categorize { category_id } => {
                if let Some(id) = category_id {
                    tx::assignable_category(ctx, id)?;
                }
                ctx.tx.execute(
                    "UPDATE transactions SET category_id=?2,version=version+1 WHERE id=?1",
                    params![target.id, category_id],
                )?;
                tx::mark_override(ctx, &target.id, "category_id")?;
            }
            BulkAction::AddTag { tag } | BulkAction::RemoveTag { tag } => {
                let notes = notes_after(
                    &item.draft.notes,
                    tag,
                    matches!(action, BulkAction::AddTag { .. }),
                )?;
                ctx.tx.execute(
                    "UPDATE transactions SET notes=?2,version=version+1 WHERE id=?1",
                    params![target.id, notes],
                )?;
                tx::mark_override(ctx, &target.id, "notes")?;
                tx::project(ctx, &target.id)?;
            }
            BulkAction::Flag { flagged, due_date } => {
                ctx.tx.execute("UPDATE transactions SET flagged=?2,flag_due_date=?3,version=version+1 WHERE id=?1",params![target.id,flagged,due_date])?;
                tx::mark_override(ctx, &target.id, "flagged")?;
                tx::mark_override(ctx, &target.id, "flag_due_date")?;
            }
            BulkAction::AssignGroup { group_id } => {
                assign_group(ctx, &item, group_id)?;
            }
            BulkAction::Delete => {
                tx::remove(ctx, &target.id, false)?;
            }
        }
        changed_ids.push(target.id);
    }
    Ok(BulkResult {
        changed_ids,
        skipped_ids: skipped,
    })
}

pub fn installment_preview(
    ctx: &Context<'_>,
    store: &PreviewStore,
    window: &str,
    series: Option<&str>,
    draft: &InstallmentDraft,
    scope: &InstallmentScope,
) -> Result<InstallmentPreview> {
    if draft.occurrence_count == 0
        || draft.occurrence_count > 600
        || draft.posting.installment.is_some()
    {
        return Err(invalid("occurrenceCount", "invalid_installment_count"));
    }
    let purchase = parse_date(&draft.purchase_date)?;
    let initial = parse_date(&draft.posting.bank_date)?;
    if purchase > initial {
        return Err(invalid("purchaseDate", "purchase_after_posting"));
    }
    let (series, create, postings) = if let Some(series) = series {
        let (stored_purchase,stored_count,identity)=ctx.tx.query_row("SELECT purchase_date,occurrence_count,fingerprint FROM installment_series WHERE id=?1",
            [series],|r|Ok((r.get::<_,String>(0)?,r.get::<_,u16>(1)?,r.get::<_,Option<String>>(2)?))).optional()?.ok_or_else(||missing("seriesId"))?;
        if identity.is_none()
            || stored_purchase != draft.purchase_date
            || stored_count != draft.occurrence_count
        {
            return Err(conflict("installment_identity_protected"));
        }
        let anchor_id = draft
            .anchor_id
            .as_deref()
            .ok_or_else(|| invalid("anchorId", "anchor_required"))?;
        let anchor = tx::get(ctx, anchor_id)?;
        let position = anchor
            .draft
            .installment
            .as_ref()
            .filter(|value| value.series_id == series)
            .ok_or_else(|| invalid("anchorId", "installment_series_mismatch"))?
            .number;
        let mut query=ctx.tx.prepare("SELECT id FROM transactions WHERE installment_series_id=?1 ORDER BY installment_number")?;
        let ids = query
            .query_map([series], |r| r.get::<_, String>(0))?
            .collect::<crate::sql::Result<Vec<_>>>()?;
        let mut postings = Vec::new();
        for id in ids {
            let before = tx::get(ctx, &id)?;
            let member = before
                .draft
                .installment
                .clone()
                .ok_or_else(|| integrity("invalid_installment_metadata"))?;
            if (*scope == InstallmentScope::This && member.number != position)
                || (*scope == InstallmentScope::Future && member.number < position)
            {
                continue;
            }
            let mut posting = draft.posting.clone();
            posting.installment = Some(member);
            if id != anchor_id {
                posting.bank_date = before.draft.bank_date;
                posting.reporting_date_override = before.draft.reporting_date_override;
                posting.effective_bill_date = before.draft.effective_bill_date;
                posting.statement_id = before.draft.statement_id;
                posting.status = before.draft.status;
                posting.original = before.draft.original;
            }
            postings.push((
                Some(VersionedId {
                    id,
                    version: before.row.version,
                }),
                posting,
            ));
        }
        (series.to_owned(), false, postings)
    } else {
        if *scope != InstallmentScope::All
            || draft.anchor_id.is_some()
            || draft.posting.original.is_some()
            || draft.posting.statement_id.is_some()
            || draft.posting.effective_bill_date.is_some()
            || draft.posting.reporting_date_override.is_some()
        {
            return Err(invalid(
                "draft",
                "new_installment_requires_unlocked_dates_and_native_amount",
            ));
        }
        let series = new_id();
        let count = i64::from(draft.occurrence_count);
        let total = positive(&draft.posting.amount.minor)?;
        if total < count {
            return Err(invalid("amount", "installment_rounds_to_zero"));
        }
        let mut splits = Vec::new();
        if !draft.posting.splits.is_empty() {
            if !(2..=1000).contains(&draft.posting.splits.len()) {
                return Err(invalid("splits", "invalid_split_shape"));
            }
            let sum = draft.posting.splits.iter().try_fold(0i64, |sum, split| {
                Ok::<_, AppError>(sum + positive(&split.minor)?)
            })?;
            if sum != total {
                return Err(invalid("splits", "split_sum_mismatch"));
            }
            splits = draft
                .posting
                .splits
                .iter()
                .map(|split| positive(&split.minor).map(|value| value as u64))
                .collect::<Result<_>>()?;
        }
        let mut postings = Vec::new();
        for number in 1..=draft.occurrence_count {
            let mut posting = draft.posting.clone();
            let bank = month_shift(initial, i32::from(number) - 1, initial.day())?;
            posting.bank_date = bank.to_string();
            if bank > ctx.today {
                posting.status = TxStatus::Projected;
            }
            let amount = total / count + i64::from(i64::from(number) <= total % count);
            posting.amount.minor = amount.to_string();
            if !splits.is_empty() {
                let allocations = tx::weighted_split(amount, &splits)?;
                for (split, amount) in posting.splits.iter_mut().zip(allocations) {
                    split.minor = amount.to_string();
                }
            }
            posting.installment = Some(InstallmentRef {
                series_id: series.clone(),
                number,
            });
            postings.push((None, posting));
        }
        (series, true, postings)
    };
    let rows = postings
        .iter()
        .map(|(target, post)| InstallmentOccurrence {
            existing_id: target.as_ref().map(|t| t.id.clone()),
            number: post
                .installment
                .as_ref()
                .expect("planned installment")
                .number,
            bank_date: post.bank_date.clone(),
            amount: post.amount.clone(),
        })
        .collect();
    let plan = InstallmentPlan {
        series,
        create,
        count: draft.occurrence_count,
        purchase: draft.purchase_date.clone(),
        postings,
    };
    let (preview_token, revision) = store.put(ctx, window, Plan::Installment(plan))?;
    Ok(InstallmentPreview {
        preview_token,
        rows,
        revision,
    })
}
pub fn installment_apply(
    ctx: &Context<'_>,
    store: &PreviewStore,
    window: &str,
    token: &str,
) -> Result<InstallmentDetail> {
    let Plan::Installment(plan) = store.get(ctx, window, token)? else {
        return Err(invalid("previewToken", "preview_purpose_mismatch"));
    };
    if plan.create {
        let identity = digest(&("manual_installment", &plan.series))?;
        ctx.tx.execute(
            "INSERT INTO installment_series VALUES(?1,?2,?3,?4)",
            params![plan.series, plan.purchase, plan.count, identity],
        )?;
    }
    let mut transactions = Vec::new();
    for (target, posting) in plan.postings {
        transactions.push(tx::save(ctx, target.as_ref(), &posting)?.row);
    }
    Ok(InstallmentDetail {
        series_id: plan.series,
        transactions,
    })
}
