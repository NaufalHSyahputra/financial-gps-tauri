use crate::sql::{OptionalExtension, params};

use super::{
    Context, Deleted, Direction, Money, TxStatus, VersionedId, accounts, check_version, conflict,
    digest, dto, invalid, new_id, positive, revision, stale_revision,
    transactions::{self as tx, EntryKind, TxDetail, TxDraft},
};

dto! {
    #[serde(rename_all_fields = "camelCase")]
    pub enum CorrectionAction {
        Void { target: VersionedId },
        Correct { target: VersionedId, draft: Box<TxDraft> },
        Opening { account_id: String, amount: Money, date: String },
        Refund { target: VersionedId, original_id: Option<String> },
    }
    pub struct CorrectionRequest { pub action: CorrectionAction, pub reason: String }
    pub struct CorrectionPreview {
        pub revision: String, pub token: String,
        pub before: Option<TxDetail>, pub proposed: Option<TxDraft>,
    }
    pub struct CorrectionConfirm { pub request: CorrectionRequest, pub token: String }
    pub struct PostingAuditItem {
        pub id: String, pub action: String, pub reason: String, pub recorded_at_ms: String,
        pub before: Option<TxDetail>, pub after: Option<TxDetail>,
    }
}

type Result<T> = crate::domain::Result<T>;

fn opening(ctx: &Context<'_>, account: &str) -> Result<Option<TxDetail>> {
    let id: Option<String> = ctx
        .tx
        .query_row(
            "SELECT id FROM transactions WHERE account_id=?1 AND entry_kind='opening'",
            [account],
            |r| r.get(0),
        )
        .optional()?;
    id.map(|id| tx::get(ctx, &id)).transpose()
}

fn ordinary(ctx: &Context<'_>, target: &VersionedId) -> Result<TxDetail> {
    let before = tx::check_delete(ctx, &target.id, false)?;
    check_version(&before.row.version, target)?;
    if before.row.entry_kind != EntryKind::Ordinary
        || before.row.status.as_str() != "posted"
        || before.draft.installment.is_some()
        || before.draft.ignored
    {
        return Err(conflict("posting_correction_requires_ordinary_posted"));
    }
    Ok(before)
}

fn eligible_refund(ctx: &Context<'_>, credit: &TxDetail, original: Option<&str>) -> Result<()> {
    // An unlinked refund can later be linked. A linked refund cannot silently be reallocated.
    let current: Option<Option<String>> = ctx
        .tx
        .query_row(
            "SELECT original_id FROM posting_refunds WHERE refund_id=?1",
            [&credit.row.id],
            |r| r.get(0),
        )
        .optional()?;
    if credit.row.direction.as_str() != "credit"
        || credit.row.category_id.is_some()
        || !credit.draft.splits.is_empty()
        || current
            .as_ref()
            .is_some_and(|id| id.is_some() || original.is_none())
    {
        return Err(conflict("refund_requires_unclassified_credit"));
    }
    if let Some(id) = original {
        let purchase = tx::get(ctx, id)?;
        if purchase.row.direction.as_str() != "debit"
            || purchase.row.ignored
            || purchase.row.exclude_from_pnl
            || purchase.row.status.as_str() != "posted"
            || purchase.row.transfer_id.is_some()
            || purchase.row.entry_kind != EntryKind::Ordinary
            || purchase.row.amount.currency != credit.row.amount.currency
            || purchase.row.bank_date > credit.row.bank_date
        {
            return Err(conflict("refund_original_ineligible"));
        }
        let mut stmt = ctx.tx.prepare("SELECT r.amount_minor FROM posting_refunds p JOIN transactions r ON r.id=p.refund_id WHERE p.original_id=?1 AND r.ignored=0")?;
        let mut rows = stmt.query([id])?;
        let mut total = i128::from(positive(&credit.row.amount.minor)?);
        while let Some(row) = rows.next()? {
            total += i128::from(row.get::<_, i64>(0)?);
        }
        if total > i128::from(positive(&purchase.row.amount.minor)?) {
            return Err(conflict("refund_exceeds_purchase"));
        }
    }
    Ok(())
}

pub fn preview(ctx: &Context<'_>, request: &CorrectionRequest) -> Result<CorrectionPreview> {
    if request.reason.trim().is_empty() || request.reason.trim().chars().count() > 500 {
        return Err(invalid("reason", "correction_reason_required"));
    }
    let (before, proposed) = match &request.action {
        CorrectionAction::Void { target } => {
            let before = ordinary(ctx, target)?;
            let mut draft = before.draft.clone();
            draft.ignored = true;
            (Some(before), Some(draft))
        }
        CorrectionAction::Correct { target, draft } => {
            let before = ordinary(ctx, target)?;
            if !before.sources.is_empty()
                || draft.ignored
                || !matches!(draft.status, TxStatus::Posted)
                || draft.installment.is_some()
                || draft.account_id != before.row.account_id
            {
                return Err(conflict("posting_requires_specialized_correction"));
            }
            positive(&draft.amount.minor)?;
            crate::domain::parse_date(&draft.bank_date)?;
            (Some(before), Some((**draft).clone()))
        }
        CorrectionAction::Opening {
            account_id,
            amount,
            date,
        } => {
            let (account, archived) = accounts::raw(ctx, account_id)?;
            let signed = super::minor(&amount.minor)?;
            if crate::domain::parse_date(date)? > ctx.today {
                return Err(invalid("date", "opening_snapshot_future_date"));
            }
            if archived.is_some()
                || account.currency_code != amount.currency
                || signed.unsigned_abs() > 9_000_000_000_000
            {
                return Err(invalid("amount", "invalid_balance_change"));
            }
            if matches!(
                account.kind,
                accounts::AccountKind::CreditCard | accounts::AccountKind::Loan
            ) {
                return Err(conflict("historical_card_checkpoint_required"));
            }
            let ambiguous: bool = ctx.tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM transactions WHERE account_id=?1 AND bank_date=?2
                 AND entry_kind<>'opening' AND ignored=0 AND status='posted')",
                params![account_id, date],
                |r| r.get(0),
            )?;
            if ambiguous && accounts::opening_floor(ctx, account_id)?.as_deref() != Some(date) {
                return Err(conflict("opening_snapshot_same_day_ordering_required"));
            }
            (opening(ctx, account_id)?, None)
        }
        CorrectionAction::Refund {
            target,
            original_id,
        } => {
            let before = tx::get(ctx, &target.id)?;
            check_version(&before.row.version, target)?;
            if before.row.entry_kind != EntryKind::Ordinary
                || before.row.status.as_str() != "posted"
                || before.row.ignored
                || before.row.transfer_id.is_some()
                || before.draft.installment.is_some()
            {
                return Err(conflict("refund_requires_unclassified_credit"));
            }
            let linked: bool = ctx.tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM cash_plan_entries WHERE transaction_id=?1)
                 OR EXISTS(SELECT 1 FROM recurring_occurrences WHERE transaction_id=?1)
                 OR EXISTS(SELECT 1 FROM shared_settlements WHERE transaction_id=?1)
                 OR EXISTS(SELECT 1 FROM asset_transactions WHERE linked_ledger_tx_id=?1)",
                [&target.id],
                |r| r.get(0),
            )?;
            if linked {
                return Err(conflict("posting_has_dependencies"));
            }
            eligible_refund(ctx, &before, original_id.as_deref())?;
            let mut draft = before.draft.clone();
            draft.exclude_from_pnl = true;
            (Some(before), Some(draft))
        }
    };
    let revision = revision(&ctx.tx)?;
    let token = digest(&(ctx.scope, &revision, request))?;
    Ok(CorrectionPreview {
        revision,
        token,
        before,
        proposed,
    })
}

fn encode(value: &Option<TxDetail>) -> Result<String> {
    serde_json::to_string(value).map_err(super::codec_error)
}

pub fn apply(ctx: &Context<'_>, confirm: &CorrectionConfirm) -> Result<Deleted> {
    let revision = revision(&ctx.tx)?;
    if digest(&(ctx.scope, &revision, &confirm.request))? != confirm.token {
        return Err(stale_revision());
    }
    let view = preview(ctx, &confirm.request)?;
    let before = view.before;
    let mut id = before.as_ref().map(|detail| detail.row.id.clone());
    let audit_id = new_id();
    let (action, after) = match &confirm.request.action {
        CorrectionAction::Void { target } => {
            let after = tx::save(ctx, Some(target), &view.proposed.expect("void draft"))?;
            ("void", Some(after))
        }
        CorrectionAction::Correct { target, draft } => {
            ("correction", Some(tx::save(ctx, Some(target), draft)?))
        }
        CorrectionAction::Opening {
            account_id,
            amount,
            date,
        } => {
            ctx.tx.execute(
                "INSERT INTO account_opening_snapshots VALUES(?1,?2)
                ON CONFLICT(account_id) DO UPDATE SET effective_on=excluded.effective_on",
                params![account_id, date],
            )?;
            accounts::write_balance(
                ctx,
                account_id,
                amount,
                date,
                Some(&confirm.request.reason),
                true,
            )?;
            let after = opening(ctx, account_id)?;
            id = Some(account_id.clone());
            ("opening", after)
        }
        CorrectionAction::Refund {
            target,
            original_id,
        } => {
            let after = if before
                .as_ref()
                .is_some_and(|detail| detail.draft.exclude_from_pnl)
            {
                tx::get(ctx, &target.id)?
            } else {
                tx::save(ctx, Some(target), &view.proposed.expect("refund draft"))?
            };
            ctx.tx.execute(
                "INSERT INTO posting_refunds VALUES(?1,?2) ON CONFLICT(refund_id) DO UPDATE SET original_id=excluded.original_id",
                params![target.id,original_id],
            )?;
            ("refund", Some(after))
        }
    };
    let id = id.expect("correction subject");
    ctx.tx.execute(
        "INSERT INTO posting_audit VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            audit_id,
            id,
            action,
            confirm.request.reason.trim(),
            encode(&before)?,
            encode(&after)?,
            ctx.now_ms,
            serde_json::to_string(&confirm.request.action).map_err(super::codec_error)?
        ],
    )?;
    if action == "void" {
        ctx.tx.execute(
            "INSERT INTO posting_voids VALUES(?1,?2)",
            params![id, audit_id],
        )?;
    }
    Ok(Deleted { id })
}

pub(crate) fn record_adjustment(
    ctx: &Context<'_>,
    account: &str,
    id: &str,
    reason: &str,
) -> Result<()> {
    let detail = Some(tx::get(ctx, id)?);
    ctx.tx.execute(
        "INSERT INTO posting_audit VALUES(?1,?2,'adjustment',?3,'null',?4,?5,'null')",
        params![
            new_id(),
            account,
            reason.trim(),
            encode(&detail)?,
            ctx.now_ms
        ],
    )?;
    Ok(())
}

pub fn history(ctx: &Context<'_>, id: &str) -> Result<Vec<PostingAuditItem>> {
    let mut stmt = ctx.tx.prepare(
        "SELECT id,action,reason,recorded_at_ms,before_json,after_json FROM posting_audit WHERE transaction_id=?1 ORDER BY recorded_at_ms,id"
    )?;
    let mut rows = stmt.query([id])?;
    let mut result = Vec::new();
    while let Some(row) = rows.next()? {
        let decode = |column| {
            serde_json::from_str(&row.get::<_, String>(column)?).map_err(super::codec_error)
        };
        result.push(PostingAuditItem {
            id: row.get(0)?,
            action: row.get(1)?,
            reason: row.get(2)?,
            recorded_at_ms: row.get::<_, i64>(3)?.to_string(),
            before: decode(4)?,
            after: decode(5)?,
        });
    }
    Ok(result)
}

pub(crate) fn check_dependencies(ctx: &Context<'_>, id: &str, financial_edit: bool) -> Result<()> {
    let blocked: bool = ctx.tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM posting_voids WHERE transaction_id=?1)
          OR (?2 AND (EXISTS(SELECT 1 FROM posting_refunds WHERE refund_id=?1 OR original_id=?1)
          OR EXISTS(SELECT 1 FROM cash_plan_entries WHERE transaction_id=?1)
          OR EXISTS(SELECT 1 FROM transfer_fees WHERE transaction_id=?1)))",
        params![id, financial_edit],
        |r| r.get(0),
    )?;
    if blocked {
        return Err(conflict("posting_has_audited_dependencies"));
    }
    Ok(())
}

pub(crate) fn save_transfer_fee(
    ctx: &Context<'_>,
    debit: &TxDetail,
    fee: &TxDraft,
) -> Result<String> {
    if fee.account_id != debit.row.account_id
        || fee.amount.currency != debit.row.amount.currency
        || fee.bank_date != debit.row.bank_date
        || !matches!(fee.direction, Direction::Debit)
        || !matches!(fee.status, TxStatus::Posted)
        || fee.ignored
        || fee.exclude_from_pnl
        || fee.installment.is_some()
    {
        return Err(invalid("fee", "invalid_transfer_fee"));
    }
    let Some(category) = &fee.category_id else {
        return Err(invalid("categoryId", "fee_category_required"));
    };
    let expense: bool = ctx.tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM categories WHERE id=?1 AND kind='expense' AND ignored=0)",
        [category],
        |r| r.get(0),
    )?;
    if !expense {
        return Err(invalid("categoryId", "fee_category_required"));
    }
    Ok(tx::save(ctx, None, fee)?.row.id)
}
