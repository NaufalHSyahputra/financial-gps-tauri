use crate::sql::{OptionalExtension, Row, Value, params, params_from_iter};
use chrono::{Days, NaiveDate};
use std::collections::{BTreeSet, HashMap};

use super::{accounts, fx, *};

dto! {
    pub struct SplitDraft { pub category_id: String, pub minor: String }
    pub struct InstallmentRef { pub series_id: String, pub number: u16 }
    pub struct OriginalAmount { pub amount: Money, pub rate_decimal: String, pub rate_date: String, pub rate_source: String }
    pub struct TxDraft {
        pub account_id: String, pub bank_date: String, pub amount: Money, pub direction: Direction,
        pub status: TxStatus, pub description: String,
        #[serde(default)] pub notes: String,
        pub payee_id: Option<String>, pub category_id: Option<String>,
        #[serde(default)] pub splits: Vec<SplitDraft>,
        pub effective_bill_date: Option<String>, pub reporting_date_override: Option<String>,
        #[serde(default)] pub ignored: bool,
        #[serde(default)] pub exclude_from_pnl: bool,
        #[serde(default)] pub flagged: bool,
        pub flag_due_date: Option<String>, pub alias_id: Option<String>, pub statement_id: Option<String>,
        pub installment: Option<InstallmentRef>, pub original: Option<OriginalAmount>,
    }
    #[derive(PartialEq, Eq)]
    pub enum EntryKind { Ordinary, Opening, Adjustment }
    pub struct TxRow {
        pub id: String, pub version: String, pub account_id: String, pub amount: Money, pub direction: Direction,
        pub bank_date: String, pub effective_date: String, pub reporting_date: String,
        pub status: TxStatus, pub entry_kind: EntryKind, pub description: String, pub payee_id: Option<String>,
        pub category_id: Option<String>, pub category: Option<TransactionCategory>, pub statement_id: Option<String>, pub ignored: bool, pub exclude_from_pnl: bool,
        pub flagged: bool, pub flag_due_date: Option<String>, pub transfer_id: Option<String>, pub running_balance: Option<Money>, pub pnl_amount:Option<Money>,
    }
    pub struct TransactionCategory { pub name: String, pub kind: super::catalog::CategoryKind, pub icon: Option<String> }
    pub struct ImportSource { pub namespace: Option<String>, pub external_id: Option<String>, pub bank_date: String, pub amount: Money, pub direction: String, pub description: String }
    pub struct TxDetail {
        pub row: TxRow, pub draft: TxDraft, pub applied_mode: AccountingMode, pub sources: Vec<ImportSource>,
        pub tags: Vec<String>, pub recurring_rule_id: Option<String>, pub shared_expense_id: Option<String>, pub attachment_ids: Vec<String>,
    }
    pub enum SortField { Date, Amount, Description, Payee, Category, Account, Type, Status, FlagDueDate }
    pub struct Sort { pub field: SortField, #[serde(default)] pub ascending: bool }
    pub enum FlagState { Flagged, Overdue, DueWithinWeek }
    pub struct AmountBounds { pub currency: String, pub minimum: Option<String>, pub maximum: Option<String> }
    #[derive(Default)]
    pub struct LedgerFilter {
        #[serde(default)] pub account_ids: Vec<String>, #[serde(default)] pub alias_ids: Vec<String>,
        #[serde(default)] pub category_ids: Vec<String>, #[serde(default)] pub payee_ids: Vec<String>,
        #[serde(default)] pub group_ids: Vec<String>,
        pub statement_id: Option<String>, #[serde(default)] pub unbilled: bool,
        pub text: Option<String>, #[serde(default)] pub uncategorized: bool, #[serde(default)] pub tags: Vec<String>,
        pub native_amount: Option<AmountBounds>, pub display_amount: Option<AmountBounds>,
        pub entry_kind: Option<EntryKind>, pub status: Option<TxStatus>, pub direction: Option<Direction>,
        pub include_opening: Option<bool>, pub transfer: Option<bool>, pub pnl_only: Option<bool>, pub ignored: Option<bool>,
        pub flag_state: Option<FlagState>, pub basis: Option<DateBasis>, pub range: Option<DateRange>, pub sort: Option<Sort>,
    }
}

impl Direction {
    fn read(s: &str) -> crate::sql::Result<Self> {
        match s {
            "debit" => Ok(Self::Debit),
            "credit" => Ok(Self::Credit),
            _ => Err(crate::sql::Error::InvalidQuery),
        }
    }
}
impl TxStatus {
    fn read(s: &str) -> crate::sql::Result<Self> {
        match s {
            "posted" => Ok(Self::Posted),
            "pending" => Ok(Self::Pending),
            "projected" => Ok(Self::Projected),
            _ => Err(crate::sql::Error::InvalidQuery),
        }
    }
}
impl EntryKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ordinary => "ordinary",
            Self::Opening => "opening",
            Self::Adjustment => "adjustment",
        }
    }
    fn read(s: &str) -> crate::sql::Result<Self> {
        match s {
            "ordinary" => Ok(Self::Ordinary),
            "opening" => Ok(Self::Opening),
            "adjustment" => Ok(Self::Adjustment),
            _ => Err(crate::sql::Error::InvalidQuery),
        }
    }
}

pub const LINK_ID: &str = "coalesce((SELECT l.id FROM transfer_links l WHERE l.debit_id=t.id),(SELECT l.id FROM transfer_links l WHERE l.credit_id=t.id))";
pub const NO_TRANSFER: &str =
    "NOT EXISTS(SELECT 1 FROM transfer_links l WHERE l.debit_id=t.id OR l.credit_id=t.id)";
pub const VISIBLE_CATEGORY: &str = "NOT EXISTS(SELECT 1 FROM categories c WHERE c.id=t.category_id AND (c.ignored=1 OR c.kind='transfer'))";
pub const VISIBLE_SPLIT: &str =
    "(NOT EXISTS(SELECT 1 FROM transaction_splits s WHERE s.transaction_id=t.id) OR
    EXISTS(SELECT 1 FROM transaction_splits s JOIN categories c ON c.id=s.category_id
           WHERE s.transaction_id=t.id AND c.ignored=0 AND c.kind<>'transfer'))";
pub const ROW_COLUMNS: &str = "t.id,CAST(t.version AS TEXT),t.account_id,t.currency_code,CAST(t.amount_minor AS TEXT),t.direction,
    t.bank_date,t.effective_date,t.reporting_date,t.status,t.entry_kind,t.description,t.payee_id,t.category_id,t.statement_id,
    t.ignored,t.exclude_from_pnl,t.flagged,t.flag_due_date";
pub const SOURCE: &str = " FROM transactions t JOIN accounts a ON a.id=t.account_id
    LEFT JOIN payees p ON p.id=t.payee_id LEFT JOIN categories cat ON cat.id=t.category_id WHERE 1=1";

pub fn read_row(r: &Row<'_>) -> crate::sql::Result<TxRow> {
    Ok(TxRow {
        id: r.get(0)?,
        version: r.get(1)?,
        account_id: r.get(2)?,
        amount: Money {
            currency: r.get(3)?,
            minor: r.get(4)?,
        },
        direction: Direction::read(r.get_ref(5)?.as_str()?)?,
        bank_date: r.get(6)?,
        effective_date: r.get(7)?,
        reporting_date: r.get(8)?,
        status: TxStatus::read(r.get_ref(9)?.as_str()?)?,
        entry_kind: EntryKind::read(r.get_ref(10)?.as_str()?)?,
        description: r.get(11)?,
        payee_id: r.get(12)?,
        category_id: r.get(13)?,
        category: None,
        statement_id: r.get(14)?,
        ignored: r.get(15)?,
        exclude_from_pnl: r.get(16)?,
        flagged: r.get(17)?,
        flag_due_date: r.get(18)?,
        transfer_id: r.get(19)?,
        running_balance: None,
        pnl_amount: None,
    })
}

fn read_category(row: &Row<'_>, start: usize) -> crate::sql::Result<Option<TransactionCategory>> {
    let Some(name) = row.get(start)? else {
        return Ok(None);
    };
    Ok(Some(TransactionCategory {
        name,
        kind: super::catalog::CategoryKind::read(row.get_ref(start + 1)?.as_str()?)?,
        icon: row.get(start + 2)?,
    }))
}

pub fn get(ctx: &Context<'_>, id: &str) -> Result<TxDetail> {
    let (row, notes, bill, override_date, alias, explicit_statement, installment, original, applied_mode) = ctx.tx.query_row(
        &format!("SELECT {ROW_COLUMNS},{LINK_ID},t.notes,t.effective_bill_date,t.reporting_date_override,t.alias_id,
         CASE WHEN EXISTS(SELECT 1 FROM transaction_overrides o WHERE o.transaction_id=t.id AND o.field_key='statement_id') THEN t.statement_id END,
         t.installment_series_id,t.installment_number,t.original_currency_code,CAST(t.original_amount_minor AS TEXT),
         t.account_fx_rate_decimal,t.account_fx_rate_date,t.account_fx_source,t.applied_mode,cat.name,cat.kind,cat.icon
         FROM transactions t LEFT JOIN categories cat ON cat.id=t.category_id WHERE t.id=?1"),
        [id], |r| {
            let installment = if let Some(series_id) = r.get::<_,Option<String>>(25)? {
                Some(InstallmentRef {series_id,number:r.get(26)?})
            } else {None};
            let original = if let Some(currency)=r.get::<_,Option<String>>(27)? {
                Some(OriginalAmount {amount:Money{currency,minor:r.get(28)?},rate_decimal:r.get(29)?,rate_date:r.get(30)?,rate_source:r.get(31)?})
            } else {None};
            let mut row = read_row(r)?;
            row.category = read_category(r, 33)?;
            Ok((row,r.get(20)?,r.get(21)?,r.get(22)?,r.get(23)?,r.get(24)?,installment,original,AccountingMode::read(r.get_ref(32)?.as_str()?)?))
        },
    ).optional()?.ok_or_else(|| missing("transactionId"))?;
    let splits = ctx.tx.prepare_cached("SELECT category_id,CAST(amount_minor AS TEXT) FROM transaction_splits WHERE transaction_id=?1 ORDER BY position")?
        .query_map([id],|r|Ok(SplitDraft{category_id:r.get(0)?,minor:r.get(1)?}))?.collect::<crate::sql::Result<_>>()?;
    let sources=ctx.tx.prepare_cached("SELECT provider_namespace,external_id,raw_bank_date,raw_currency,CAST(raw_amount_minor AS TEXT),raw_direction,raw_description
        FROM transaction_import_keys WHERE transaction_id=?1 ORDER BY id")?.query_map([id],|r|Ok(ImportSource {
            namespace:r.get(0)?,external_id:r.get(1)?,bank_date:r.get(2)?,amount:Money{currency:r.get(3)?,minor:r.get(4)?},
            direction:r.get(5)?,description:r.get(6)?,
        }))?.collect::<crate::sql::Result<_>>()?;
    let tags = accounts::strings(
        &ctx.tx,
        "SELECT token FROM transaction_tags WHERE transaction_id=?1 ORDER BY token",
        id,
    )?;
    let attachment_ids = accounts::strings(
        &ctx.tx,
        "SELECT id FROM attachments WHERE transaction_id=?1 AND state='ready' ORDER BY id",
        id,
    )?;
    let recurring_rule_id = ctx
        .tx
        .query_row(
            "SELECT rule_id FROM recurring_occurrences WHERE transaction_id=?1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    let shared_expense_id = ctx
        .tx
        .query_row(
            "SELECT id FROM shared_expenses WHERE transaction_id=?1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    let draft = TxDraft {
        account_id: row.account_id.clone(),
        bank_date: row.bank_date.clone(),
        amount: row.amount.clone(),
        direction: row.direction.clone(),
        status: row.status.clone(),
        description: row.description.clone(),
        notes,
        payee_id: row.payee_id.clone(),
        category_id: row.category_id.clone(),
        splits,
        effective_bill_date: bill,
        reporting_date_override: override_date,
        ignored: row.ignored,
        exclude_from_pnl: row.exclude_from_pnl,
        flagged: row.flagged,
        flag_due_date: row.flag_due_date.clone(),
        alias_id: alias,
        statement_id: explicit_statement,
        installment,
        original,
    };
    Ok(TxDetail {
        row,
        draft,
        applied_mode,
        sources,
        tags,
        recurring_rule_id,
        shared_expense_id,
        attachment_ids,
    })
}

pub fn assignable_category(ctx: &Context<'_>, id: &str) -> Result<()> {
    let valid = ctx
        .tx
        .query_row(
            "SELECT hidden=0 AND archived=0 FROM categories WHERE id=?1",
            [id],
            |r| r.get::<_, bool>(0),
        )
        .optional()?
        .unwrap_or(false);
    if !valid {
        return Err(invalid("categoryId", "unassignable_category"));
    }
    Ok(())
}

pub fn save(ctx: &Context<'_>, target: Option<&VersionedId>, draft: &TxDraft) -> Result<TxDetail> {
    save_posting(ctx, target, draft, false)
}

pub(crate) fn reconcile_import(
    ctx: &Context<'_>,
    target: &VersionedId,
    draft: &TxDraft,
) -> Result<TxDetail> {
    save_posting(ctx, Some(target), draft, true)
}

fn save_posting(
    ctx: &Context<'_>,
    target: Option<&VersionedId>,
    draft: &TxDraft,
    import: bool,
) -> Result<TxDetail> {
    let id = target.map_or_else(new_id, |t| t.id.clone());
    let old = target
        .map(|target| {
            let detail = get(ctx, &target.id)?;
            check_version(&detail.row.version, target)?;
            if detail.row.entry_kind != EntryKind::Ordinary {
                return Err(conflict("synthetic_posting_protected"));
            }
            Ok(detail)
        })
        .transpose()?;
    let (account, archived) = accounts::raw(ctx, &draft.account_id)?;
    if archived.is_some()
        && old
            .as_ref()
            .is_none_or(|old| old.row.account_id != draft.account_id)
    {
        return Err(conflict("account_archived"));
    }
    if account.currency_code != draft.amount.currency {
        return Err(invalid("currency", "currency_mismatch"));
    }
    let amount = positive(&draft.amount.minor)?;
    let bank = parse_date(&draft.bank_date)?;
    let bill = date_option(draft.effective_bill_date.as_deref())?;
    date_option(draft.reporting_date_override.as_deref())?;
    date_option(draft.flag_due_date.as_deref())?;
    if draft.flag_due_date.is_some() && !draft.flagged {
        return Err(invalid("flagDueDate", "flag_required"));
    }
    if draft.notes.chars().count() > 1000 {
        return Err(invalid("notes", "notes_too_long"));
    }
    if draft.description.trim().is_empty() || draft.description.chars().count() > 1024 {
        return Err(invalid("description", "invalid_description"));
    }
    if !draft.splits.is_empty()
        && (draft.splits.len() < 2 || draft.splits.len() > 1000 || draft.category_id.is_some())
    {
        return Err(invalid("splits", "invalid_split_shape"));
    }
    let mut split_total = 0i128;
    for split in &draft.splits {
        if old.as_ref().is_none_or(|old| {
            !old.draft
                .splits
                .iter()
                .any(|line| line.category_id == split.category_id)
        }) {
            assignable_category(ctx, &split.category_id)?;
        }
        split_total += i128::from(positive(&split.minor)?);
    }
    if !draft.splits.is_empty() && split_total != i128::from(amount) {
        return Err(invalid("splits", "split_sum_mismatch"));
    }
    if let Some(category) = &draft.category_id
        && old.as_ref().and_then(|old| old.row.category_id.as_ref()) != Some(category)
    {
        assignable_category(ctx, category)?;
    }
    if let Some(payee) = &draft.payee_id
        && old.as_ref().and_then(|old| old.row.payee_id.as_ref()) != Some(payee)
        && !ctx
            .tx
            .query_row("SELECT archived=0 FROM payees WHERE id=?1", [payee], |r| {
                r.get::<_, bool>(0)
            })
            .optional()?
            .unwrap_or(false)
    {
        return Err(invalid("payeeId", "unassignable_payee"));
    }
    if let Some(alias) = &draft.alias_id {
        let exists = ctx.tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM account_aliases WHERE id=?1 AND account_id=?2)",
            params![alias, draft.account_id],
            |r| r.get::<_, bool>(0),
        )?;
        if !exists {
            return Err(invalid("aliasId", "account_alias_mismatch"));
        }
    }
    if let Some(installment) = &draft.installment {
        let (purchase, count) = ctx
            .tx
            .query_row(
                "SELECT purchase_date,occurrence_count FROM installment_series WHERE id=?1",
                [&installment.series_id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, u16>(1)?)),
            )
            .optional()?
            .ok_or_else(|| missing("seriesId"))?;
        if !(1..=count).contains(&installment.number) || parse_date(&purchase)? > bank {
            return Err(invalid("installment", "invalid_installment_metadata"));
        }
    }
    let mut original = (None, None, None, None, None);
    if let Some(source) = &draft.original {
        if source.amount.currency == draft.amount.currency {
            return Err(invalid("original", "original_currency_must_differ"));
        }
        let converted = fx::convert_at_rate(
            ctx,
            &source.amount,
            &draft.amount.currency,
            fx::rate(&source.rate_decimal)?,
        )?;
        parse_date(&source.rate_date)?;
        if converted.minor != draft.amount.minor
            || source.rate_source.trim().is_empty()
            || source.rate_source.len() > 256
        {
            return Err(invalid("original", "original_conversion_mismatch"));
        }
        original = (
            Some(source.amount.currency.as_str()),
            Some(positive(&source.amount.minor)?),
            Some(fx::rate(&source.rate_decimal)?.normalize().to_string()),
            Some(source.rate_date.as_str()),
            Some(source.rate_source.as_str()),
        );
    }
    if let Some(old) = &old {
        let differs = old.row.account_id != draft.account_id
            || old.row.amount.currency != draft.amount.currency
            || old.row.amount.minor != draft.amount.minor
            || old.row.direction.as_str() != draft.direction.as_str();
        super::corrections::check_dependencies(
            ctx,
            &id,
            differs
                || old.row.bank_date != draft.bank_date
                || old.row.ignored != draft.ignored
                || old.row.exclude_from_pnl != draft.exclude_from_pnl
                || old.row.status.as_str() != draft.status.as_str(),
        )?;
        if !import
            && !old.sources.is_empty()
            && (differs
                || old.row.bank_date != draft.bank_date
                || digest(&old.draft.original)? != digest(&draft.original)?
                || (matches!(old.row.status, TxStatus::Pending)
                    && matches!(draft.status, TxStatus::Posted)))
        {
            return Err(conflict("imported_posting_protected"));
        }
        if old.row.transfer_id.is_some()
            && (differs
                || old.row.ignored != draft.ignored
                || draft.category_id.is_some()
                || !draft.splits.is_empty())
        {
            return Err(conflict("transfer_leg_protected"));
        }
        if (old.shared_expense_id.is_some()
            || old.recurring_rule_id.is_some()
            || !accounts::strings(
                &ctx.tx,
                "SELECT transaction_id FROM loan_payment_allocations WHERE transaction_id=?1",
                &id,
            )?
            .is_empty()
            || !accounts::strings(
                &ctx.tx,
                "SELECT id FROM shared_settlements WHERE transaction_id=?1",
                &id,
            )?
            .is_empty()
            || !accounts::strings(
                &ctx.tx,
                "SELECT id FROM asset_transactions WHERE linked_ledger_tx_id=?1",
                &id,
            )?
            .is_empty())
            && differs
        {
            return Err(conflict("posting_has_dependencies"));
        }
    }
    let (effective, mode, statement) = accounts::booking(
        ctx,
        &draft.account_id,
        &account,
        &draft.direction,
        bank,
        bill,
        draft.statement_id.as_deref(),
    )?;
    let assembling = old
        .as_ref()
        .is_none_or(|before| !before.draft.splits.is_empty() || !draft.splits.is_empty());
    if old.is_some() && assembling {
        ctx.tx.execute(
            "UPDATE transactions SET assembly_state=0 WHERE id=?1",
            [&id],
        )?;
        ctx.tx.execute(
            "DELETE FROM transaction_splits WHERE transaction_id=?1",
            [&id],
        )?;
    }
    ctx.tx.execute("INSERT INTO transactions(id,account_id,currency_code,amount_minor,direction,bank_date,effective_date,effective_bill_date,
        reporting_date_override,applied_mode,status,description,notes,payee_id,category_id,alias_id,statement_id,ignored,exclude_from_pnl,
        flagged,flag_due_date,installment_series_id,installment_number,original_currency_code,original_amount_minor,account_fx_rate_decimal,
        account_fx_rate_date,account_fx_source,assembly_state,created_at_ms)
        VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,0,?29)
        ON CONFLICT(id) DO UPDATE SET account_id=excluded.account_id,currency_code=excluded.currency_code,amount_minor=excluded.amount_minor,
        direction=excluded.direction,bank_date=excluded.bank_date,effective_date=excluded.effective_date,effective_bill_date=excluded.effective_bill_date,
        reporting_date_override=excluded.reporting_date_override,applied_mode=excluded.applied_mode,status=excluded.status,description=excluded.description,
        notes=excluded.notes,payee_id=excluded.payee_id,category_id=excluded.category_id,alias_id=excluded.alias_id,statement_id=excluded.statement_id,
        ignored=excluded.ignored,exclude_from_pnl=excluded.exclude_from_pnl,flagged=excluded.flagged,flag_due_date=excluded.flag_due_date,
        installment_series_id=excluded.installment_series_id,installment_number=excluded.installment_number,original_currency_code=excluded.original_currency_code,
        original_amount_minor=excluded.original_amount_minor,account_fx_rate_decimal=excluded.account_fx_rate_decimal,
        account_fx_rate_date=excluded.account_fx_rate_date,account_fx_source=excluded.account_fx_source,version=transactions.version+1",
        params![id,draft.account_id,draft.amount.currency,amount,draft.direction.as_str(),draft.bank_date,effective,draft.effective_bill_date,
            draft.reporting_date_override,mode.as_str(),draft.status.as_str(),draft.description,draft.notes,draft.payee_id,draft.category_id,draft.alias_id,
            statement,draft.ignored,draft.exclude_from_pnl,draft.flagged,draft.flag_due_date,draft.installment.as_ref().map(|i|&i.series_id),
            draft.installment.as_ref().map(|i|i.number),original.0,original.1,original.2,original.3,original.4,ctx.now_ms])?;
    for (index, split) in draft.splits.iter().enumerate() {
        ctx.tx.execute(
            "INSERT INTO transaction_splits VALUES(?1,?2,?3,?4)",
            params![
                id,
                (index + 1) as u32,
                split.category_id,
                positive(&split.minor)?
            ],
        )?;
    }
    if assembling {
        ctx.tx.execute(
            "UPDATE transactions SET assembly_state=1 WHERE id=?1",
            [&id],
        )?;
    }
    if draft.statement_id.is_some() {
        mark_override(ctx, &id, "statement_id")?;
    } else {
        ctx.tx.execute("DELETE FROM transaction_overrides WHERE transaction_id=?1 AND field_key='statement_id'",[&id])?;
    }
    let is_new = old.is_none();
    if let Some(old) = old.filter(|old| !import && !old.sources.is_empty()) {
        for (key, changed) in [
            ("description", old.draft.description != draft.description),
            ("notes", old.draft.notes != draft.notes),
            (
                "category_id",
                old.draft.category_id != draft.category_id
                    || digest(&old.draft.splits)? != digest(&draft.splits)?,
            ),
            ("payee_id", old.draft.payee_id != draft.payee_id),
            (
                "reporting_date_override",
                old.draft.reporting_date_override != draft.reporting_date_override,
            ),
            (
                "effective_bill_date",
                old.draft.effective_bill_date != draft.effective_bill_date,
            ),
            ("ignored", old.draft.ignored != draft.ignored),
            (
                "exclude_from_pnl",
                old.draft.exclude_from_pnl != draft.exclude_from_pnl,
            ),
            ("flagged", old.draft.flagged != draft.flagged),
            (
                "flag_due_date",
                old.draft.flag_due_date != draft.flag_due_date,
            ),
            ("alias_id", old.draft.alias_id != draft.alias_id),
        ] {
            if changed {
                mark_override(ctx, &id, key)?;
            }
        }
    }
    project_posting(ctx, &id, is_new)?;
    get(ctx, &id)
}

pub(crate) fn mark_override(ctx: &Context<'_>, id: &str, field: &str) -> Result<()> {
    ctx.tx.execute(
        "INSERT OR IGNORE INTO transaction_overrides VALUES (?1,?2)",
        params![id, field],
    )?;
    Ok(())
}

pub fn post(ctx: &Context<'_>, target: &VersionedId, booking: Option<&str>) -> Result<TxDetail> {
    let mut detail = get(ctx, &target.id)?;
    check_version(&detail.row.version, target)?;
    if matches!(detail.row.status, TxStatus::Posted) || !detail.sources.is_empty() {
        return Err(conflict("posting_cannot_be_promoted"));
    }
    detail.draft.status = TxStatus::Posted;
    detail.draft.bank_date = booking.map_or_else(|| ctx.today.to_string(), str::to_owned);
    save(ctx, Some(target), &detail.draft)
}

pub fn rebuild_dates(ctx: &Context<'_>, account_id: &str) -> Result<()> {
    let (account, _) = accounts::raw(ctx, account_id)?;
    let mut stmt=ctx.tx.prepare("SELECT t.id,t.direction,t.bank_date,t.effective_bill_date,t.effective_date,t.applied_mode,t.statement_id,
        EXISTS(SELECT 1 FROM transaction_overrides o WHERE o.transaction_id=t.id AND field_key='statement_id')
        FROM transactions t WHERE account_id=?1 AND entry_kind='ordinary'")?;
    let records = stmt
        .query_map([account_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                Direction::read(r.get_ref(1)?.as_str()?)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, bool>(7)?,
            ))
        })?
        .collect::<crate::sql::Result<Vec<_>>>()?;
    for (id, direction, bank, bill, old_effective, old_mode, old_statement, manual) in records {
        let (effective, mode, statement) = accounts::booking(
            ctx,
            account_id,
            &account,
            &direction,
            parse_date(&bank)?,
            date_option(bill.as_deref())?,
            if manual {
                old_statement.as_deref()
            } else {
                None
            },
        )?;
        if effective != old_effective || mode.as_str() != old_mode || statement != old_statement {
            ctx.tx.execute("UPDATE transactions SET effective_date=?2,applied_mode=?3,statement_id=?4,version=version+1 WHERE id=?1",
                params![id,effective,mode.as_str(),statement])?;
        }
    }
    Ok(())
}

pub fn delete(ctx: &Context<'_>, target: &VersionedId, confirm: bool) -> Result<Deleted> {
    if !confirm {
        return Err(invalid("confirm", "confirmation_required"));
    }
    let before = get(ctx, &target.id)?;
    check_version(&before.row.version, target)?;
    remove(ctx, &target.id, false)
}
pub(crate) fn check_delete(ctx: &Context<'_>, id: &str, opening: bool) -> Result<TxDetail> {
    check_removal(ctx, id, opening, false)
}

pub(crate) fn check_import_restore(ctx: &Context<'_>, id: &str) -> Result<TxDetail> {
    check_removal(ctx, id, false, true)
}

fn check_removal(
    ctx: &Context<'_>,
    id: &str,
    opening: bool,
    retain_occurrence: bool,
) -> Result<TxDetail> {
    let detail = get(ctx, id)?;
    super::corrections::check_dependencies(ctx, id, true)?;
    if detail.row.transfer_id.is_some() || (!opening && detail.row.entry_kind == EntryKind::Opening)
    {
        return Err(conflict("protected_posting"));
    }
    let has_dependency:bool=ctx.tx.query_row("SELECT
       EXISTS(SELECT 1 FROM attachments WHERE transaction_id=?1) OR EXISTS(SELECT 1 FROM shared_expenses WHERE transaction_id=?1)
       OR EXISTS(SELECT 1 FROM shared_settlements WHERE transaction_id=?1) OR EXISTS(SELECT 1 FROM loan_payment_allocations WHERE transaction_id=?1)
       OR EXISTS(SELECT 1 FROM asset_transactions WHERE linked_ledger_tx_id=?1) OR (NOT ?2 AND EXISTS(SELECT 1 FROM recurring_occurrences WHERE transaction_id=?1))",
       params![id,retain_occurrence],|r|r.get(0))?;
    if has_dependency {
        return Err(conflict("posting_has_dependencies"));
    }
    Ok(detail)
}
pub fn remove(ctx: &Context<'_>, id: &str, opening: bool) -> Result<Deleted> {
    let audited: bool = ctx.tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM posting_audit WHERE transaction_id=?1)",
        [id],
        |r| r.get(0),
    )?;
    if audited {
        return Err(conflict("posting_has_audited_dependencies"));
    }
    let detail = check_delete(ctx, id, opening)?;
    if !detail.draft.splits.is_empty() {
        ctx.tx
            .execute("UPDATE transactions SET assembly_state=0 WHERE id=?1", [id])?;
        ctx.tx.execute(
            "DELETE FROM transaction_splits WHERE transaction_id=?1",
            [id],
        )?;
    }
    let rowid = ctx
        .tx
        .query_row("SELECT rowid FROM transactions WHERE id=?1", [id], |r| {
            r.get(0)
        })?;
    if let Some(fts) = fts_rowid(ctx, id, rowid)? {
        ctx.tx
            .execute("DELETE FROM transaction_search WHERE rowid=?1", [fts])?;
    }
    ctx.tx
        .execute("DELETE FROM transactions WHERE id=?1", [id])?;
    Ok(Deleted { id: id.to_owned() })
}

pub(crate) fn hashtag_spans(notes: &str) -> Vec<(usize, usize, String)> {
    fn token_char(ch: char) -> bool {
        ch.is_alphanumeric() || ch == '_' || ch == '-' || is_combining_mark(ch)
    }
    let chars = notes.char_indices().collect::<Vec<_>>();
    let mut spans = Vec::new();
    for (index, &(start, ch)) in chars.iter().enumerate() {
        if ch != '#' || (index > 0 && (token_char(chars[index - 1].1) || chars[index - 1].1 == '#'))
        {
            continue;
        }
        let mut end = start + 1;
        for &(offset, ch) in chars
            .iter()
            .skip(index + 1)
            .take_while(|(_, ch)| token_char(*ch))
        {
            end = offset + ch.len_utf8();
        }
        if end > start + 1 {
            let tag = normalize(&notes[start + 1..end]);
            if !tag.is_empty() {
                spans.push((start, end, tag));
            }
        }
    }
    spans
}

pub fn project(ctx: &Context<'_>, id: &str) -> Result<()> {
    project_posting(ctx, id, false)
}
fn fts_rowid(ctx: &Context<'_>, id: &str, rowid: i64) -> Result<Option<i64>> {
    let current = ctx
        .tx
        .query_row(
            "SELECT rowid FROM transaction_search WHERE rowid=?1 AND transaction_id=?2",
            params![rowid, id],
            |r| r.get(0),
        )
        .optional()?;
    if current.is_some() {
        return Ok(current);
    }
    Ok(ctx
        .tx
        .query_row(
            "SELECT rowid FROM transaction_search WHERE transaction_id=?1",
            [id],
            |r| r.get(0),
        )
        .optional()?)
}
pub(crate) fn project_posting(ctx: &Context<'_>, id: &str, is_new: bool) -> Result<()> {
    let (description,notes,payee,rowid)=ctx.tx.query_row("SELECT t.description,t.notes,p.name,t.rowid FROM transactions t LEFT JOIN payees p ON p.id=t.payee_id
        WHERE t.id=?1",[id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,i64>(3)?)))?;
    ctx.tx
        .execute("DELETE FROM transaction_tags WHERE transaction_id=?1", [id])?;
    for tag in hashtag_spans(&notes)
        .into_iter()
        .map(|(_, _, tag)| tag)
        .collect::<BTreeSet<_>>()
    {
        ctx.tx.execute(
            "INSERT INTO transaction_tags VALUES(?1,?2)",
            params![id, tag],
        )?;
    }
    // Preserve the FTS row identifier when it exists, always resolve it via the durable UUID.
    let existing = if is_new {
        None
    } else {
        fts_rowid(ctx, id, rowid)?
    };
    if let Some(key) = existing {
        ctx.tx.execute("UPDATE transaction_search SET description=?2,notes=?3,payee=?4 WHERE rowid=?1 AND transaction_id=?5",
            params![key,normalize(&description),normalize(&notes),payee.as_deref().map(normalize),id])?;
    } else {
        let available: bool = ctx.tx.query_row(
            "SELECT NOT EXISTS(SELECT 1 FROM transaction_search WHERE rowid=?1)",
            [rowid],
            |r| r.get(0),
        )?;
        ctx.tx.execute("INSERT INTO transaction_search(rowid,transaction_id,description,notes,payee) VALUES(?1,?2,?3,?4,?5)",
            params![available.then_some(rowid),id,normalize(&description),normalize(&notes),payee.as_deref().map(normalize)])?;
    }
    Ok(())
}

pub fn search_terms(value: &str) -> Result<Option<String>> {
    if value.len() > 512 {
        return Err(invalid("text", "search_too_long"));
    }
    let value = normalize(value);
    let terms = value
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .take(16)
        .map(|term| format!("\"{term}\"*"))
        .collect::<Vec<_>>();
    Ok((!terms.is_empty()).then(|| terms.join(" AND ")))
}

pub struct Query {
    pub sql: String,
    pub args: Vec<Value>,
}
impl Query {
    pub fn new(sql: String) -> Self {
        Self {
            sql,
            args: Vec::new(),
        }
    }
    pub fn bind(&mut self, value: impl Into<Value>) -> String {
        self.args.push(value.into());
        format!("?{}", self.args.len())
    }
    pub fn and(&mut self, predicate: &str) {
        self.sql.push_str(" AND ");
        self.sql.push_str(predicate);
    }
    pub fn equals(&mut self, expr: &str, value: impl Into<Value>) {
        let param = self.bind(value);
        self.and(&format!("{expr}={param}"));
    }
    pub fn choices(&mut self, pattern: &str, values: &[String]) {
        if !values.is_empty() {
            let params = values
                .iter()
                .map(|id| self.bind(id.clone()))
                .collect::<Vec<_>>()
                .join(",");
            self.and(&pattern.replace("{values}", &params));
        }
    }
    pub fn limit(&mut self, limit: usize) {
        self.sql.push_str(&format!(" LIMIT {limit}"));
    }
}

pub fn pnl_query(query: &mut Query, today: NaiveDate) {
    let day = query.bind(today.to_string());
    query.and(&format!("t.status='posted' AND t.bank_date<={day} AND t.reporting_date<={day} AND t.ignored=0 AND t.exclude_from_pnl=0 AND {NO_TRANSFER} AND {VISIBLE_CATEGORY} AND {VISIBLE_SPLIT} AND NOT EXISTS(SELECT 1 FROM shared_settlements ss WHERE ss.transaction_id=t.id)"));
}
pub fn filter_query(ctx: &Context<'_>, query: &mut Query, filter: &LedgerFilter) -> Result<()> {
    for values in [
        &filter.account_ids,
        &filter.alias_ids,
        &filter.category_ids,
        &filter.payee_ids,
        &filter.group_ids,
        &filter.tags,
    ] {
        if values.len() > 100 {
            return Err(invalid("filter", "too_many_filter_values"));
        }
    }
    query.choices("t.account_id IN ({values})", &filter.account_ids);
    query.choices("t.alias_id IN ({values})", &filter.alias_ids);
    query.choices("(t.category_id IN ({values}) OR EXISTS(SELECT 1 FROM transaction_splits s WHERE s.transaction_id=t.id AND s.category_id IN ({values})))",&filter.category_ids);
    query.choices("t.payee_id IN ({values})", &filter.payee_ids);
    query.choices("EXISTS(SELECT 1 FROM shared_expenses e WHERE e.transaction_id=t.id AND e.group_id IN ({values}))",&filter.group_ids);
    if let Some(id) = &filter.statement_id {
        query.equals("t.statement_id", id.clone());
    } else {
        if filter.unbilled {
            query.and("a.kind='credit_card' AND t.statement_id IS NULL");
        }
        if let Some(range) = &filter.range {
            let (from, to) = date_range(range)?;
            let column = filter
                .basis
                .as_ref()
                .unwrap_or(&DateBasis::Reporting)
                .column();
            for (op, value) in [(">=", from), ("<=", to)] {
                if let Some(value) = value {
                    let p = query.bind(value);
                    query.and(&format!("t.{column}{op}{p}"));
                }
            }
        }
    }
    if let Some(text) = &filter.text
        && !text.trim().is_empty()
    {
        if let Some(terms) = search_terms(text)? {
            let param = query.bind(terms);
            query.and(&format!("t.id IN(SELECT transaction_id FROM transaction_search WHERE transaction_search MATCH {param})"));
        } else {
            query.and("0");
        }
    }
    if filter.uncategorized {
        query.and("t.category_id IS NULL AND NOT EXISTS(SELECT 1 FROM transaction_splits s WHERE s.transaction_id=t.id)");
    }
    let tags = filter
        .tags
        .iter()
        .map(|tag| normalize(tag.trim_start_matches('#')))
        .collect::<Vec<_>>();
    query.choices("t.id IN (SELECT tag.transaction_id FROM transaction_tags tag INDEXED BY tag_lookup WHERE tag.token IN ({values}))",&tags);
    if let Some(bounds) = &filter.native_amount {
        scale(&ctx.tx, &bounds.currency)?;
        let (min, max) = bounds_data(bounds)?;
        query.equals("t.currency_code", bounds.currency.clone());
        let lo = query.bind(min);
        let hi = query.bind(max);
        query.and(&format!("t.amount_minor BETWEEN {lo} AND {hi}"));
    }
    if let Some(kind) = &filter.entry_kind {
        query.equals("t.entry_kind", kind.as_str().to_owned());
    } else if filter.include_opening == Some(false) {
        query.and("t.entry_kind<>'opening'");
    }
    if let Some(status) = &filter.status {
        query.equals("t.status", status.as_str().to_owned());
    }
    if let Some(direction) = &filter.direction {
        query.equals("t.direction", direction.as_str().to_owned());
    }
    if let Some(ignored) = filter.ignored {
        query.equals("t.ignored", i64::from(ignored));
    }
    if let Some(transfer) = filter.transfer {
        query.and(if transfer {
            "EXISTS(SELECT 1 FROM transfer_links l WHERE l.debit_id=t.id OR l.credit_id=t.id)"
        } else {
            NO_TRANSFER
        });
    }
    if filter.pnl_only == Some(true) {
        pnl_query(query, ctx.today);
        query.choices("(t.category_id IN ({values}) OR EXISTS(SELECT 1 FROM transaction_splits s JOIN categories c ON c.id=s.category_id
            WHERE s.transaction_id=t.id AND s.category_id IN ({values}) AND c.ignored=0 AND c.kind<>'transfer'))",&filter.category_ids);
    }
    if let Some(flag) = &filter.flag_state {
        query.and("t.flagged=1");
        match flag {
            FlagState::Flagged => {}
            FlagState::Overdue => {
                let p = query.bind(ctx.today.to_string());
                query.and(&format!("t.flag_due_date<{p}"));
            }
            FlagState::DueWithinWeek => {
                let from = query.bind(ctx.today.to_string());
                let to = query.bind(
                    ctx.today
                        .checked_add_days(Days::new(7))
                        .ok_or_else(|| integrity("date_out_of_range"))?
                        .to_string(),
                );
                query.and(&format!("t.flag_due_date BETWEEN {from} AND {to}"));
            }
        }
    }
    Ok(())
}

pub fn bounds_data(bounds: &AmountBounds) -> Result<(i64, i64)> {
    let min = bounds
        .minimum
        .as_deref()
        .map(accounts::nonnegative)
        .transpose()?
        .unwrap_or(0);
    let max = bounds
        .maximum
        .as_deref()
        .map(accounts::nonnegative)
        .transpose()?
        .unwrap_or(i64::MAX);
    if min > max {
        return Err(invalid("amount", "invalid_amount_range"));
    }
    Ok((min, max))
}

pub fn sort_cursor(
    query: &mut Query,
    cursor: &ReadPage,
    sorter: &str,
    id_expr: &str,
    numeric: bool,
    ascending: bool,
) -> Result<()> {
    if let Some((value, id)) = &cursor.after {
        let value = if numeric {
            query.bind(minor(value)?)
        } else {
            query.bind(value.clone())
        };
        let id = query.bind(id.clone());
        let op = if ascending { ">" } else { "<" };
        query.and(&format!("({sorter},{id_expr}) {op} ({value},{id})"));
    }
    let dir = if ascending { "ASC" } else { "DESC" };
    query
        .sql
        .push_str(&format!(" ORDER BY {sorter} {dir},{id_expr} {dir}"));
    Ok(())
}

pub fn list(
    ctx: &Context<'_>,
    filter: &LedgerFilter,
    page: &PageReq,
    running: bool,
) -> Result<Page<TxRow>> {
    let paging = ReadPage::new(
        ctx,
        page,
        &("transactions", filter, running, ctx.today.to_string()),
    )?;
    let column = filter
        .basis
        .as_ref()
        .unwrap_or(&DateBasis::Reporting)
        .column();
    let (sorter, numeric) = match filter.sort.as_ref().map(|sort| &sort.field) {
        None | Some(SortField::Date) => (format!("t.{column}"), false),
        Some(field) => (
            match field {
                SortField::Amount => "t.amount_minor",
                SortField::Description => "t.description",
                SortField::Payee => "coalesce(p.normalized_name,'')",
                SortField::Category => "coalesce(cat.name,'')",
                SortField::Account => "a.name",
                SortField::Type => "t.direction",
                SortField::Status => "t.status",
                SortField::FlagDueDate => "coalesce(t.flag_due_date,'')",
                SortField::Date => unreachable!(),
            }
            .to_owned(),
            matches!(field, SortField::Amount),
        ),
    };
    let mut query = Query::new(format!(
        "SELECT {ROW_COLUMNS},{LINK_ID},CAST({sorter} AS TEXT),cat.name,cat.kind,cat.icon{SOURCE}"
    ));
    filter_query(ctx, &mut query, filter)?;
    sort_cursor(
        &mut query,
        &paging,
        &sorter,
        "t.id",
        numeric,
        filter.sort.as_ref().is_some_and(|s| s.ascending),
    )?;
    let display = filter
        .display_amount
        .as_ref()
        .map(|bounds| {
            scale(&ctx.tx, &bounds.currency)?;
            let (lo, hi) = bounds_data(bounds)?;
            Ok::<_, AppError>((&bounds.currency, lo, hi))
        })
        .transpose()?;
    if display.is_none() {
        query.limit(paging.limit + 1);
    }
    let mut stmt = ctx.tx.prepare(&query.sql)?;
    let mut rows = stmt.query(params_from_iter(query.args))?;
    let mut out = Vec::new();
    let mut conversions = fx::Resolver::new(ctx);
    while let Some(row) = rows.next()? {
        let mut item = read_row(&row)?;
        item.category = read_category(&row, 21)?;
        if let Some((currency, min, max)) = display {
            let result =
                conversions.convert(&item.amount, currency, parse_date(&item.reporting_date)?)?;
            let Some(value) = result.displayed else {
                continue;
            };
            if !(min..=max).contains(&minor(&value.minor)?) {
                continue;
            }
        }
        out.push((row.get::<_, String>(20)?, item));
        if out.len() > paging.limit {
            break;
        }
    }
    let mut result = paging.finish(out, |(key, row)| (key.clone(), row.id.clone()))?;
    if running {
        running_balance(ctx, &mut result.rows)?;
    }
    if filter.pnl_only == Some(true) {
        let mut sums=Query::new("SELECT sum(s.amount_minor) FROM transaction_splits s JOIN categories c ON c.id=s.category_id
            WHERE c.ignored=0 AND c.kind<>'transfer' AND s.transaction_id=?1".to_owned());
        sums.args.push(Value::Null);
        sums.choices("s.category_id IN ({values})", &filter.category_ids);
        let mut statement = ctx.tx.prepare(&sums.sql)?;
        for (_, row) in &mut result.rows {
            sums.args[0] = row.id.clone().into();
            let split_total = statement.query_row(params_from_iter(&sums.args), |row| {
                row.get::<_, Option<i64>>(0)
            })?;
            row.pnl_amount = Some(split_total.map_or_else(
                || row.amount.clone(),
                |minor| money(&row.amount.currency, minor),
            ));
        }
    }
    Ok(Page {
        rows: result.rows.into_iter().map(|(_, row)| row).collect(),
        next_cursor: result.next_cursor,
        revision: result.revision,
    })
}

fn running_balance(ctx: &Context<'_>, selected: &mut [(String, TxRow)]) -> Result<()> {
    let mut locations: HashMap<String, Vec<usize>> = HashMap::new();
    for (index, (_, row)) in selected.iter().enumerate() {
        locations
            .entry(row.account_id.clone())
            .or_default()
            .push(index);
    }
    for (account, mut indices) in locations {
        let key = |index: usize| {
            let row = &selected[index].1;
            (row.bank_date.clone(), row.id.clone())
        };
        indices.sort_unstable_by_key(|index| key(*index));
        let floor = accounts::opening_floor(ctx, &account)?;
        if let Some(floor) = &floor {
            indices.retain(|index| selected[*index].1.bank_date >= *floor);
            if indices.is_empty() {
                continue;
            }
        }
        let (first, first_id) = key(indices[0]);
        let (last, last_id) = key(*indices.last().expect("one selected row"));
        let mut total = accounts::balance(ctx, &account, parse_date(&first)?, Some(&first_id))?;
        let snapshot = if floor.is_some() {
            "AND NOT (t.bank_date=?2 AND t.entry_kind='opening')"
        } else {
            ""
        };
        let mut stmt=ctx.tx.prepare_cached(&format!("SELECT t.bank_date,t.id,CASE t.direction WHEN 'credit' THEN t.amount_minor ELSE -t.amount_minor END
            FROM transactions t JOIN accounts a ON a.id=t.account_id WHERE t.account_id=?1 AND
            (t.bank_date,t.id)>(?2,?3) AND (t.bank_date,t.id)<=(?4,?5) AND {} {snapshot} ORDER BY t.bank_date,t.id",accounts::BALANCE_VISIBLE))?;
        let mut rows = stmt.query(params![account, first, first_id, last, last_id])?;
        let mut pending = rows.next()?.as_ref().map(movement).transpose()?;
        for position in indices {
            let row = &mut selected[position].1;
            while pending
                .as_ref()
                .is_some_and(|(date, id, _)| (date, id) <= (&row.bank_date, &row.id))
            {
                total += i128::from(pending.take().expect("pending movement").2);
                pending = rows.next()?.as_ref().map(movement).transpose()?;
            }
            row.running_balance = Some(money(&row.amount.currency, total));
        }
    }
    Ok(())
}
fn movement(row: &Row<'_>) -> crate::sql::Result<(String, String, i64)> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
}

pub fn weighted_split(amount: i64, weights: &[u64]) -> Result<Vec<i64>> {
    if !(1..=9_000_000_000_000).contains(&amount)
        || !(2..=1000).contains(&weights.len())
        || weights.contains(&0)
    {
        return Err(invalid("weights", "invalid_split_weights"));
    }
    let sum = weights
        .iter()
        .map(|weight| u128::from(*weight))
        .sum::<u128>();
    let mut shares = Vec::with_capacity(weights.len());
    let mut remainder = Vec::with_capacity(weights.len());
    for (index, weight) in weights.iter().enumerate() {
        let numerator = u128::from(amount as u64) * u128::from(*weight);
        shares.push((numerator / sum) as i64);
        remainder.push((numerator % sum, index));
    }
    remainder.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    for (_, index) in remainder
        .into_iter()
        .take((amount - shares.iter().sum::<i64>()) as usize)
    {
        shares[index] += 1;
    }
    if shares.contains(&0) {
        return Err(invalid("weights", "split_share_rounds_to_zero"));
    }
    Ok(shares)
}
