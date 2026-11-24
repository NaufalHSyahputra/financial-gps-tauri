use crate::sql::{OptionalExtension, params};
use chrono::{Datelike, Months, NaiveDate};
use std::collections::HashMap;

use super::*;

dto! {
    #[derive(PartialEq, Eq)]
    pub enum AccountKind { Checking, Savings, CreditCard, Loan, Cash }
    pub struct AccountDraft {
        pub name: String, pub institution: Option<String>, pub kind: AccountKind, pub currency_code: String,
        pub payment_method: Option<String>, pub close_day: Option<u8>, pub due_day: Option<u8>,
        pub card_limit_minor: Option<String>, pub credit_line_id: Option<String>, pub accounting_override: Option<AccountingMode>,
    }
    pub struct AccountChoice { pub id: String, pub name: String, pub kind: AccountKind }
    pub struct AccountDetail {
        pub id: String, pub version: String, pub account: AccountDraft, pub archived_on: Option<String>,
        pub as_of: String, pub balance: Money, pub credit: Option<Credit>,
    }
    pub struct Credit {
        pub line_id: Option<String>, pub debt: Money, pub limit: Option<Money>, pub available: Option<Money>,
        pub future_holds: Money, pub available_after_holds: Option<Money>,
    }
    pub struct CardLineDraft { pub name: String, pub currency_code: String, pub limit_minor: String }
    pub struct CardLineDetail { pub id: String, pub version: String, pub draft: CardLineDraft, pub credit: Credit }
    #[derive(PartialEq, Eq)]
    pub enum StatementSource { Computed, Manual, StatementFile }
    pub struct StatementDraft { pub account_id: String, pub close_on: String, pub due_on: String, pub source: StatementSource }
    pub struct StatementSummary { pub id: String, pub version: String, pub draft: StatementDraft }
    pub struct Settings { pub name: String, pub display_currency: String, pub timezone_override: Option<String>, pub accounting_mode: AccountingMode, pub revision: String }
    #[serde(tag = "change", content = "value")]
    pub enum Patch<T> { Keep, Set(T), Clear }
    pub struct SettingsPatch { pub name: Option<Patch<String>>, pub display_currency: Option<Patch<String>>, pub timezone_override: Option<Patch<String>>, pub accounting_mode: Option<Patch<AccountingMode>> }
}

impl AccountKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Cash => "cash",
            Self::Checking => "checking",
            Self::Savings => "savings",
            Self::CreditCard => "credit_card",
            Self::Loan => "loan",
        }
    }
    fn read(s: &str) -> crate::sql::Result<Self> {
        match s {
            "cash" => Ok(Self::Cash),
            "checking" => Ok(Self::Checking),
            "savings" => Ok(Self::Savings),
            "credit_card" => Ok(Self::CreditCard),
            "loan" => Ok(Self::Loan),
            _ => Err(crate::sql::Error::InvalidQuery),
        }
    }
}
impl AccountingMode {
    pub fn read(s: &str) -> crate::sql::Result<Self> {
        match s {
            "cash" => Ok(Self::Cash),
            "accrual" => Ok(Self::Accrual),
            _ => Err(crate::sql::Error::InvalidQuery),
        }
    }
}
impl StatementSource {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Computed => "computed",
            Self::Manual => "manual",
            Self::StatementFile => "statement_file",
        }
    }
    fn read(s: &str) -> crate::sql::Result<Self> {
        match s {
            "computed" => Ok(Self::Computed),
            "manual" => Ok(Self::Manual),
            "statement_file" => Ok(Self::StatementFile),
            _ => Err(crate::sql::Error::InvalidQuery),
        }
    }
}

pub fn month_shift(date: NaiveDate, months: i32, day: u32) -> Result<NaiveDate> {
    if !(1..=31).contains(&day) {
        return Err(invalid("day", "invalid_cycle_day"));
    }
    let first = date.with_day(1).expect("month has first day");
    let first = if months >= 0 {
        first.checked_add_months(Months::new(months.unsigned_abs()))
    } else {
        first.checked_sub_months(Months::new(months.unsigned_abs()))
    }
    .filter(|date| (1..=9999).contains(&date.year()))
    .ok_or_else(|| invalid("date", "date_out_of_range"))?;
    (1..=day)
        .rev()
        .find_map(|day| first.with_day(day))
        .ok_or_else(|| invalid("date", "date_out_of_range"))
}

#[derive(Debug, PartialEq, Eq)]
pub struct CardCycle {
    pub previous_close: NaiveDate,
    pub close: NaiveDate,
    pub due: NaiveDate,
}
pub fn cycle(bank: NaiveDate, close_day: u8, due_day: u8) -> Result<CardCycle> {
    let close_day = u32::from(close_day);
    let due_day = u32::from(due_day);
    let mut close = month_shift(bank, 0, close_day)?;
    if close <= bank {
        close = month_shift(bank, 1, close_day)?;
    }
    let previous_close = month_shift(close, -1, close_day)?;
    let mut due = month_shift(close, 0, due_day)?;
    if due <= close {
        due = month_shift(close, 1, due_day)?;
    }
    Ok(CardCycle {
        previous_close,
        close,
        due,
    })
}

pub(crate) fn raw(ctx: &Context<'_>, id: &str) -> Result<(AccountDraft, Option<String>)> {
    ctx.tx.query_row(
        "SELECT name,institution,kind,currency_code,payment_method,close_day,due_day,card_limit_minor,credit_line_id,accounting_override,archived_on FROM accounts WHERE id=?1",
        [id], |r| Ok((AccountDraft {
            name: r.get(0)?, institution: r.get(1)?, kind: AccountKind::read(r.get_ref(2)?.as_str()?)?,
            currency_code: r.get(3)?, payment_method: r.get(4)?, close_day: r.get(5)?,
            due_day: r.get(6)?, card_limit_minor: r.get::<_, Option<i64>>(7)?.map(|n|n.to_string()),
            credit_line_id: r.get(8)?, accounting_override: r.get::<_, Option<String>>(9)?.as_deref().map(AccountingMode::read).transpose()?,
        }, r.get(10)?)),
    ).optional()?.ok_or_else(|| missing("accountId"))
}

pub(crate) fn booking(
    ctx: &Context<'_>,
    id: &str,
    account: &AccountDraft,
    direction: &Direction,
    bank: NaiveDate,
    bill: Option<NaiveDate>,
    statement: Option<&str>,
) -> Result<(String, AccountingMode, Option<String>)> {
    let mode = if account.kind == AccountKind::CreditCard {
        match &account.accounting_override {
            Some(mode) => mode.clone(),
            None => settings(ctx)?.accounting_mode,
        }
    } else {
        AccountingMode::Cash
    };
    if statement.is_some() && account.kind != AccountKind::CreditCard {
        return Err(invalid("statementId", "card_required"));
    }
    let explicit = statement
        .map(|id_ref| {
            ctx.tx
                .query_row(
                    "SELECT due_on FROM card_statements WHERE account_id=?1 AND id=?2",
                    params![id, id_ref],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
                .ok_or_else(|| missing("statementId"))
        })
        .transpose()?;
    let mut computed = None;
    let mut derived_statement = statement.map(str::to_owned);
    if account.kind == AccountKind::CreditCard
        && matches!(direction, Direction::Debit)
        && explicit.is_none()
        && let Some((close_day, due_day)) = account.close_day.zip(account.due_day)
    {
        let mut closing = None;
        if let Some(bill) = bill {
            let series = closings(ctx, id, bill, close_day, due_day, -3, 1)?;
            let matching = series
                .windows(2)
                .enumerate()
                .filter(|(_, pair)| pair[0].due < bill && bill <= pair[1].due)
                .collect::<Vec<_>>();
            if let [(index, pair)] = matching.as_slice()
                && series
                    .get(index + 2)
                    .is_none_or(|next| next.due != pair[1].due)
            {
                closing = Some(pair[1].clone());
            }
        }
        if closing.is_none() {
            closing = closings(ctx, id, bank, close_day, due_day, 0, 1)?
                .into_iter()
                .find(|row| bank < row.close);
        }
        if let Some(row) = closing {
            derived_statement = row.id;
            computed = Some(row.due.to_string());
        }
    }
    let date = bill
        .map(|date| date.to_string())
        .or_else(|| {
            matches!(direction, Direction::Debit)
                .then_some(explicit)
                .flatten()
        })
        .or(computed)
        .unwrap_or_else(|| bank.to_string());
    Ok((date, mode, derived_statement))
}

#[derive(Clone)]
struct Closing {
    id: Option<String>,
    close: NaiveDate,
    due: NaiveDate,
}

fn closings(
    ctx: &Context<'_>,
    account_id: &str,
    around: NaiveDate,
    close_day: u8,
    due_day: u8,
    first: i32,
    last: i32,
) -> Result<Vec<Closing>> {
    let mut result = Vec::new();
    let mut statement=ctx.tx.prepare_cached(
        "SELECT id,close_on,due_on FROM card_statements WHERE account_id=?1 AND close_on BETWEEN ?2 AND ?3 ORDER BY close_on,id"
    )?;
    for offset in first..=last {
        let from = month_shift(around, offset, 1)?;
        let to = month_shift(around, offset, 31)?;
        let mut rows = statement.query(params![account_id, from.to_string(), to.to_string()])?;
        let before = result.len();
        while let Some(row) = rows.next()? {
            result.push(Closing {
                id: Some(row.get(0)?),
                close: parse_date(row.get_ref(1)?.as_str()?)?,
                due: parse_date(row.get_ref(2)?.as_str()?)?,
            });
        }
        if result.len() == before {
            let close = month_shift(around, offset, u32::from(close_day))?;
            let mut due = month_shift(close, 0, u32::from(due_day))?;
            if due <= close {
                due = month_shift(close, 1, u32::from(due_day))?;
            }
            result.push(Closing {
                id: None,
                close,
                due,
            });
        }
    }
    Ok(result)
}

const SIGNED: &str = "CASE t.direction WHEN 'credit' THEN t.amount_minor ELSE -t.amount_minor END";

pub const BALANCE_VISIBLE: &str = "t.ignored=0 AND t.status IN ('posted','pending') AND
    (t.status='posted' OR a.kind='credit_card') AND NOT EXISTS
    (SELECT 1 FROM categories b WHERE b.id=t.category_id AND b.ignored=1)";

pub(crate) const SNAPSHOT_VISIBLE: &str = "NOT EXISTS(
    SELECT 1 FROM account_opening_snapshots o WHERE o.account_id=t.account_id AND t.bank_date<o.effective_on
)";

pub(crate) fn opening_floor(ctx: &Context<'_>, account: &str) -> Result<Option<String>> {
    Ok(ctx
        .tx
        .query_row(
            "SELECT effective_on FROM account_opening_snapshots WHERE account_id=?1",
            [account],
            |row| row.get(0),
        )
        .optional()?)
}

pub fn balance(
    ctx: &Context<'_>,
    account_id: &str,
    as_of: NaiveDate,
    through_id: Option<&str>,
) -> Result<i128> {
    let mut from = format!(
        "FROM transactions t JOIN accounts a ON a.id=t.account_id
        WHERE t.account_id=?1 AND t.bank_date<=?2 AND {BALANCE_VISIBLE}"
    );
    let floor = opening_floor(ctx, account_id)?;
    if floor.is_some() {
        from.push_str(" AND ");
        from.push_str(SNAPSHOT_VISIBLE);
    }
    if through_id.is_some() {
        if floor.is_some() {
            from.push_str(
                " AND ((t.bank_date,t.id)<=(?2,?3) OR (t.bank_date=?2 AND t.entry_kind='opening'))",
            );
        } else {
            from.push_str(" AND (t.bank_date,t.id)<=(?2,?3)");
        }
    }
    let day = as_of.to_string();
    let params: Vec<&str> = if let Some(id) = through_id {
        vec![account_id, &day, id]
    } else {
        vec![account_id, &day]
    };
    let sum = ctx
        .tx
        .prepare_cached(&format!("SELECT coalesce(sum({SIGNED}),0) {from}"))?
        .query_row(crate::sql::params_from_iter(&params), |row| {
            row.get::<_, i64>(0)
        });
    match sum {
        Ok(value) => return Ok(i128::from(value)),
        Err(crate::sql::Error::IntegerOverflow) => {}
        Err(error) => return Err(error.into()),
    }
    let mut stmt = ctx.tx.prepare_cached(&format!("SELECT {SIGNED} {from}"))?;
    let mut rows = stmt.query(crate::sql::params_from_iter(params))?;
    let mut total = 0i128;
    while let Some(row) = rows.next()? {
        total += i128::from(row.get::<_, i64>(0)?);
    }
    Ok(total)
}

fn hold(ctx: &Context<'_>, id: &str, date: NaiveDate) -> Result<i128> {
    let mut stmt = ctx.tx.prepare_cached("SELECT t.amount_minor FROM transactions t WHERE t.account_id=?1 AND
        t.bank_date>?2 AND t.installment_series_id IS NOT NULL AND t.direction='debit' AND t.ignored=0 AND
        NOT EXISTS(SELECT 1 FROM categories c WHERE c.id=t.category_id AND c.ignored=1)")?;
    let mut rows = stmt.query(params![id, date.to_string()])?;
    let mut amount = 0i128;
    while let Some(row) = rows.next()? {
        amount += i128::from(row.get::<_, i64>(0)?);
    }
    Ok(amount)
}

pub fn credit(
    ctx: &Context<'_>,
    ids: &[String],
    date: NaiveDate,
    currency: &str,
    limit: Option<i64>,
    line_id: Option<String>,
) -> Result<Credit> {
    let mut net = 0;
    let mut future = 0;
    for id in ids {
        net += balance(ctx, id, date, None)?;
        future += hold(ctx, id, date)?;
    }
    Ok(credit_from_totals(net, future, currency, limit, line_id))
}

fn credit_from_totals(
    net: i128,
    future: i128,
    currency: &str,
    limit: Option<i64>,
    line_id: Option<String>,
) -> Credit {
    let debt = (-net).max(0);
    let available = limit.map(|value| i128::from(value) - debt);
    Credit {
        line_id,
        debt: money(currency, debt),
        limit: limit.map(|v| money(currency, v)),
        available: available.map(|v| money(currency, v)),
        future_holds: money(currency, future),
        available_after_holds: available.map(|v| money(currency, v - future)),
    }
}

pub(super) fn line_credit(ctx: &Context<'_>, id: &str, date: NaiveDate) -> Result<CardLineDetail> {
    let draft = ctx.tx.query_row("SELECT name,currency_code,CAST(limit_minor AS TEXT) FROM credit_card_lines WHERE id=?1", [id],
        |r| Ok(CardLineDraft { name:r.get(0)?,currency_code:r.get(1)?,limit_minor:r.get(2)? })).optional()?.ok_or_else(|| missing("creditLineId"))?;
    let ids = strings(
        &ctx.tx,
        "SELECT id FROM accounts WHERE credit_line_id=?1",
        id,
    )?;
    let credit = credit(
        ctx,
        &ids,
        date,
        &draft.currency_code,
        Some(minor(&draft.limit_minor)?),
        Some(id.to_owned()),
    )?;
    Ok(CardLineDetail {
        id: id.to_owned(),
        version: revision(&ctx.tx)?,
        draft,
        credit,
    })
}

pub fn get(ctx: &Context<'_>, id: &str, date: Option<&str>) -> Result<AccountDetail> {
    let date = date_option(date)?.unwrap_or(ctx.today);
    let (account, archived_on) = raw(ctx, id)?;
    let balance = money(&account.currency_code, balance(ctx, id, date, None)?);
    let credit = if let Some(line) = &account.credit_line_id {
        Some(line_credit(ctx, line, date)?.credit)
    } else if account.kind == AccountKind::CreditCard {
        Some(credit(
            ctx,
            &[id.to_owned()],
            date,
            &account.currency_code,
            account.card_limit_minor.as_deref().map(minor).transpose()?,
            None,
        )?)
    } else {
        None
    };
    Ok(AccountDetail {
        id: id.to_owned(),
        version: revision(&ctx.tx)?,
        account,
        archived_on,
        as_of: date.to_string(),
        balance,
        credit,
    })
}

pub fn choices(ctx: &Context<'_>, archived: bool) -> Result<Vec<AccountChoice>> {
    let mut stmt = ctx.tx.prepare_cached(
        "SELECT id,name,kind FROM accounts WHERE (?1 OR archived_on IS NULL) ORDER BY name,id",
    )?;
    Ok(stmt
        .query_map([archived], |row| {
            Ok(AccountChoice {
                id: row.get(0)?,
                name: row.get(1)?,
                kind: AccountKind::read(row.get_ref(2)?.as_str()?)?,
            })
        })?
        .collect::<crate::sql::Result<Vec<_>>>()?)
}

pub fn list(ctx: &Context<'_>, archived: bool, date: Option<&str>) -> Result<Vec<AccountDetail>> {
    list_selected(ctx, archived, date, &[])
}

pub(super) fn list_selected(
    ctx: &Context<'_>,
    archived: bool,
    date: Option<&str>,
    selected: &[String],
) -> Result<Vec<AccountDetail>> {
    let date = date_option(date)?.unwrap_or(ctx.today);
    let day = date.to_string();
    let ids = choices(ctx, archived)?
        .into_iter()
        .map(|item| item.id)
        .filter(|id| selected.is_empty() || selected.contains(id))
        .collect::<Vec<_>>();
    let mut totals = HashMap::new();
    let has_snapshots: bool = ctx.tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM account_opening_snapshots)",
        [],
        |r| r.get(0),
    )?;
    if selected.is_empty() && ids.len() > 1 {
        let snapshot = if has_snapshots { SNAPSHOT_VISIBLE } else { "1" };
        // Visiting table pages once avoids decrypting the same pages through many account-index walks.
        totals.extend(ids.iter().map(|id| (id.clone(), 0i128)));
        let mut statement = ctx.tx.prepare_cached(&format!(
            "SELECT t.account_id,{SIGNED} FROM transactions t NOT INDEXED
             CROSS JOIN accounts a ON a.id=t.account_id WHERE t.bank_date<=?1 AND {BALANCE_VISIBLE} AND {snapshot}"
        ))?;
        let mut rows = statement.query([&day])?;
        while let Some(row) = rows.next()? {
            if let Some(total) = totals.get_mut(row.get_ref(0)?.as_str()?) {
                *total += i128::from(row.get::<_, i64>(1)?);
            }
        }
    }
    let mut details = Vec::with_capacity(ids.len());
    let mut lines = HashMap::new();
    let version = revision(&ctx.tx)?;
    for id in ids {
        let (account, archived_on) = raw(ctx, &id)?;
        let net = match totals.get(&id) {
            Some(net) => *net,
            None => balance(ctx, &id, date, None)?,
        };
        let credit = if let Some(line) = &account.credit_line_id {
            let value = match lines.entry(line.clone()) {
                std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(line_credit(ctx, line, date)?.credit)
                }
            };
            Some(value.clone())
        } else if account.kind == AccountKind::CreditCard {
            Some(credit_from_totals(
                net,
                hold(ctx, &id, date)?,
                &account.currency_code,
                account.card_limit_minor.as_deref().map(minor).transpose()?,
                None,
            ))
        } else {
            None
        };
        details.push(AccountDetail {
            id,
            version: version.clone(),
            balance: money(&account.currency_code, net),
            account,
            archived_on,
            as_of: day.clone(),
            credit,
        });
    }
    Ok(details)
}

pub fn next_version(ctx: &Context<'_>) -> Result<String> {
    let v: i64 = ctx
        .tx
        .query_row("SELECT revision FROM ledger_meta", [], |r| r.get(0))?;
    Ok(v.checked_add(1)
        .ok_or_else(|| integrity("revision_exhausted"))?
        .to_string())
}
fn updated(ctx: &Context<'_>, id: &str) -> Result<AccountDetail> {
    let mut detail = get(ctx, id, None)?;
    detail.version = next_version(ctx)?;
    Ok(detail)
}

pub fn save(
    ctx: &Context<'_>,
    target: Option<&VersionedId>,
    draft: &AccountDraft,
) -> Result<AccountDetail> {
    let name = name(&draft.name)?;
    scale(&ctx.tx, &draft.currency_code)?;
    if draft.close_day.is_some() != draft.due_day.is_some()
        || [draft.close_day, draft.due_day]
            .into_iter()
            .flatten()
            .any(|day| !(1..=31).contains(&day))
        || (draft.card_limit_minor.is_some() && draft.credit_line_id.is_some())
        || (draft.kind != AccountKind::CreditCard
            && (draft.close_day.is_some()
                || draft.card_limit_minor.is_some()
                || draft.credit_line_id.is_some()
                || draft.accounting_override.is_some()))
        || (draft.kind != AccountKind::Checking && draft.payment_method.is_some())
    {
        return Err(invalid("account", "account_fields_incompatible"));
    }
    let limit = draft
        .card_limit_minor
        .as_deref()
        .map(nonnegative)
        .transpose()?;
    if let Some(line) = &draft.credit_line_id {
        let currency = ctx
            .tx
            .query_row(
                "SELECT currency_code FROM credit_card_lines WHERE id=?1",
                [line],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .ok_or_else(|| missing("creditLineId"))?;
        if currency != draft.currency_code {
            return Err(invalid("creditLineId", "currency_mismatch"));
        }
    }
    let id = if let Some(target) = target {
        check_revision(ctx, target)?;
        let (existing, _) = raw(ctx, &target.id)?;
        if existing.kind != draft.kind || existing.currency_code != draft.currency_code {
            let dependent:bool=ctx.tx.query_row("SELECT EXISTS(SELECT 1 FROM transactions WHERE account_id=?1)
                OR EXISTS(SELECT 1 FROM card_statements WHERE account_id=?1) OR EXISTS(SELECT 1 FROM loans WHERE account_id=?1)",
                [&target.id], |r|r.get(0))?;
            if dependent {
                return Err(conflict("account_has_history"));
            }
        }
        target.id.clone()
    } else {
        new_id()
    };
    ctx.tx.execute("INSERT INTO accounts(id,name,institution,kind,currency_code,payment_method,close_day,due_day,card_limit_minor,credit_line_id,accounting_override)
        VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
        ON CONFLICT(id) DO UPDATE SET name=excluded.name,institution=excluded.institution,kind=excluded.kind,
        currency_code=excluded.currency_code,payment_method=excluded.payment_method,close_day=excluded.close_day,due_day=excluded.due_day,
        card_limit_minor=excluded.card_limit_minor,credit_line_id=excluded.credit_line_id,accounting_override=excluded.accounting_override",
        params![id,name,draft.institution,draft.kind.as_str(),draft.currency_code,draft.payment_method,draft.close_day,draft.due_day,limit,
            draft.credit_line_id,draft.accounting_override.as_ref().map(AccountingMode::as_str)])?;
    if target.is_some() {
        super::transactions::rebuild_dates(ctx, &id)?;
    }
    updated(ctx, &id)
}

pub fn archive(
    ctx: &Context<'_>,
    target: &VersionedId,
    archived_on: Option<&str>,
) -> Result<AccountDetail> {
    check_revision(ctx, target)?;
    let date = date_option(archived_on)?.unwrap_or(ctx.today).to_string();
    if ctx.tx.execute(
        "UPDATE accounts SET archived_on=?2 WHERE id=?1",
        params![target.id, date],
    )? == 0
    {
        return Err(missing("accountId"));
    }
    updated(ctx, &target.id)
}

pub fn delete(ctx: &Context<'_>, target: &VersionedId, confirm: bool) -> Result<Deleted> {
    if !confirm {
        return Err(invalid("confirm", "confirmation_required"));
    }
    check_revision(ctx, target)?;
    raw(ctx, &target.id)?;
    let linked: bool = ctx.tx.query_row(
        "SELECT
        EXISTS(SELECT 1 FROM transactions WHERE account_id=?1)
        OR EXISTS(SELECT 1 FROM card_statements WHERE account_id=?1)
        OR EXISTS(SELECT 1 FROM account_aliases WHERE account_id=?1)
        OR EXISTS(SELECT 1 FROM import_batches WHERE account_id=?1)
        OR EXISTS(SELECT 1 FROM loans WHERE account_id=?1)
        OR EXISTS(SELECT 1 FROM goals WHERE account_id=?1)
        OR EXISTS(SELECT 1 FROM recurring_rules WHERE account_id=?1)
        OR EXISTS(SELECT 1 FROM account_opening_snapshots WHERE account_id=?1)",
        [&target.id],
        |r| r.get(0),
    )?;
    if linked {
        return Err(conflict("account_has_history_archive_instead"));
    }
    ctx.tx
        .execute("DELETE FROM accounts WHERE id=?1", [&target.id])?;
    Ok(Deleted {
        id: target.id.clone(),
    })
}

pub fn change_balance(
    ctx: &Context<'_>,
    id: &str,
    amount: &Money,
    date: &str,
    reason: Option<&str>,
    opening: bool,
) -> Result<AccountDetail> {
    if opening && opening_floor(ctx, id)?.is_some() {
        return Err(conflict("opening_snapshot_requires_review"));
    }
    write_balance(ctx, id, amount, date, reason, opening)
}

pub(crate) fn write_balance(
    ctx: &Context<'_>,
    id: &str,
    amount: &Money,
    date: &str,
    reason: Option<&str>,
    opening: bool,
) -> Result<AccountDetail> {
    let (account, archived) = raw(ctx, id)?;
    if archived.is_some() {
        return Err(conflict("account_archived"));
    }
    parse_date(date)?;
    if account.currency_code != amount.currency {
        return Err(invalid("currency", "currency_mismatch"));
    }
    let signed = minor(&amount.minor)?;
    let magnitude = signed.unsigned_abs();
    if magnitude > 9_000_000_000_000
        || (!opening && (signed == 0 || reason.is_none_or(|text| text.trim().is_empty())))
    {
        return Err(invalid("signedMoney", "invalid_balance_change"));
    }
    if reason.is_some_and(|text| text.chars().count() > if opening { 1000 } else { 500 }) {
        return Err(invalid("reason", "notes_too_long"));
    }
    let old = if opening {
        ctx.tx
            .query_row(
                "SELECT id FROM transactions WHERE account_id=?1 AND entry_kind='opening'",
                [id],
                |r| r.get::<_, String>(0),
            )
            .optional()?
    } else {
        None
    };
    if signed == 0 {
        if let Some(old) = old {
            super::transactions::remove(ctx, &old, true)?;
        }
    } else {
        let kind = if opening { "opening" } else { "adjustment" };
        let description = if opening {
            "Opening balance"
        } else {
            "Balance adjustment"
        };
        let direction = if signed > 0 { "credit" } else { "debit" };
        if let Some(old) = old {
            ctx.tx.execute("UPDATE transactions SET amount_minor=?2,direction=?3,bank_date=?4,effective_date=?4,version=version+1 WHERE id=?1",
                params![old,magnitude as i64,direction,date])?;
        } else {
            let new = new_id();
            ctx.tx.execute("INSERT INTO transactions(id,account_id,currency_code,amount_minor,direction,bank_date,effective_date,applied_mode,status,
                entry_kind,description,notes,exclude_from_pnl,assembly_state,created_at_ms)
                VALUES(?1,?2,?3,?4,?5,?6,?6,'cash','posted',?7,?8,?9,1,0,?10)",
                params![new,id,amount.currency,magnitude as i64,direction,date,kind,description,reason.unwrap_or_default(),ctx.now_ms])?;
            ctx.tx.execute(
                "UPDATE transactions SET assembly_state=1 WHERE id=?1",
                [&new],
            )?;
            super::transactions::project_posting(ctx, &new, true)?;
            if !opening {
                super::corrections::record_adjustment(ctx, id, &new, reason.unwrap_or_default())?;
            }
        }
    }
    updated(ctx, id)
}

pub fn nonnegative(value: &str) -> Result<i64> {
    let value = minor(value)?;
    if value < 0 {
        return Err(invalid("minor", "nonnegative_amount_required"));
    }
    Ok(value)
}
pub(crate) fn strings(tx: &Transaction<'_>, sql: &str, param: &str) -> Result<Vec<String>> {
    Ok(tx
        .prepare(sql)?
        .query_map([param], |r| r.get(0))?
        .collect::<crate::sql::Result<_>>()?)
}

pub fn lines(ctx: &Context<'_>) -> Result<Vec<CardLineDetail>> {
    let mut stmt = ctx
        .tx
        .prepare("SELECT id FROM credit_card_lines ORDER BY name,id")?;
    let ids = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<crate::sql::Result<Vec<_>>>()?;
    ids.iter()
        .map(|id| line_credit(ctx, id, ctx.today))
        .collect()
}
pub fn line_save(
    ctx: &Context<'_>,
    target: Option<&VersionedId>,
    draft: &CardLineDraft,
) -> Result<CardLineDetail> {
    let name = name(&draft.name)?;
    scale(&ctx.tx, &draft.currency_code)?;
    let limit = nonnegative(&draft.limit_minor)?;
    let id = if let Some(target) = target {
        check_revision(ctx, target)?;
        let existing = line_credit(ctx, &target.id, ctx.today)?;
        if existing.draft.currency_code != draft.currency_code
            && !strings(
                &ctx.tx,
                "SELECT id FROM accounts WHERE credit_line_id=?1",
                &target.id,
            )?
            .is_empty()
        {
            return Err(conflict("credit_line_has_cards"));
        }
        target.id.clone()
    } else {
        new_id()
    };
    ctx.tx.execute("INSERT INTO credit_card_lines VALUES(?1,?2,?3,?4)
        ON CONFLICT(id) DO UPDATE SET name=excluded.name,currency_code=excluded.currency_code,limit_minor=excluded.limit_minor",
        params![id,name,draft.currency_code,limit])?;
    let mut detail = line_credit(ctx, &id, ctx.today)?;
    detail.version = next_version(ctx)?;
    Ok(detail)
}
pub fn line_delete(ctx: &Context<'_>, id: &str) -> Result<Deleted> {
    if !strings(
        &ctx.tx,
        "SELECT id FROM accounts WHERE credit_line_id=?1",
        id,
    )?
    .is_empty()
    {
        return Err(conflict("credit_line_has_cards"));
    }
    if ctx
        .tx
        .execute("DELETE FROM credit_card_lines WHERE id=?1", [id])?
        == 0
    {
        return Err(missing("creditLineId"));
    }
    Ok(Deleted { id: id.to_owned() })
}

pub(super) fn statement_get(ctx: &Context<'_>, id: &str) -> Result<StatementSummary> {
    let draft = ctx
        .tx
        .query_row(
            "SELECT account_id,close_on,due_on,source FROM card_statements WHERE id=?1",
            [id],
            |r| {
                Ok(StatementDraft {
                    account_id: r.get(0)?,
                    close_on: r.get(1)?,
                    due_on: r.get(2)?,
                    source: StatementSource::read(r.get_ref(3)?.as_str()?)?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| missing("statementId"))?;
    Ok(StatementSummary {
        id: id.to_owned(),
        version: revision(&ctx.tx)?,
        draft,
    })
}

pub fn statements(
    ctx: &Context<'_>,
    account_id: &str,
    range: &DateRange,
) -> Result<Vec<StatementSummary>> {
    if raw(ctx, account_id)?.0.kind != AccountKind::CreditCard {
        return Err(invalid("accountId", "card_required"));
    }
    let (from, to) = date_range(range)?;
    let version = revision(&ctx.tx)?;
    let mut stmt = ctx.tx.prepare(
        "SELECT id,close_on,due_on,source FROM card_statements WHERE account_id=?1
        AND (?2 IS NULL OR close_on>=?2) AND (?3 IS NULL OR close_on<=?3) ORDER BY close_on,id",
    )?;
    Ok(stmt
        .query_map(params![account_id, from, to], |r| {
            Ok(StatementSummary {
                id: r.get(0)?,
                version: version.clone(),
                draft: StatementDraft {
                    account_id: account_id.to_owned(),
                    close_on: r.get(1)?,
                    due_on: r.get(2)?,
                    source: StatementSource::read(r.get_ref(3)?.as_str()?)?,
                },
            })
        })?
        .collect::<crate::sql::Result<_>>()?)
}
pub fn statement_save(
    ctx: &Context<'_>,
    target: Option<&VersionedId>,
    draft: &StatementDraft,
) -> Result<StatementSummary> {
    if raw(ctx, &draft.account_id)?.0.kind != AccountKind::CreditCard {
        return Err(invalid("accountId", "card_required"));
    }
    if parse_date(&draft.close_on)? >= parse_date(&draft.due_on)? {
        return Err(invalid("dueOn", "due_must_follow_close"));
    }
    let id = if let Some(target) = target {
        check_revision(ctx, target)?;
        let previous = ctx
            .tx
            .query_row(
                "SELECT account_id FROM card_statements WHERE id=?1",
                [&target.id],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .ok_or_else(|| missing("statementId"))?;
        if previous != draft.account_id {
            return Err(conflict("statement_account_immutable"));
        }
        target.id.clone()
    } else {
        new_id()
    };
    ctx.tx.execute("INSERT INTO card_statements VALUES(?1,?2,?3,?4,?5)
        ON CONFLICT(id) DO UPDATE SET close_on=excluded.close_on,due_on=excluded.due_on,source=excluded.source",
        params![id,draft.account_id,draft.close_on,draft.due_on,draft.source.as_str()])?;
    super::transactions::rebuild_dates(ctx, &draft.account_id)?;
    Ok(StatementSummary {
        id,
        version: next_version(ctx)?,
        draft: draft.clone(),
    })
}
pub fn statement_delete(ctx: &Context<'_>, id: &str) -> Result<Deleted> {
    let account = ctx
        .tx
        .query_row(
            "SELECT account_id FROM card_statements WHERE id=?1",
            [id],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .ok_or_else(|| missing("statementId"))?;
    ctx.tx.execute(
        "UPDATE transactions SET statement_id=NULL,version=version+1 WHERE statement_id=?1",
        [id],
    )?;
    ctx.tx
        .execute("DELETE FROM card_statements WHERE id=?1", [id])?;
    super::transactions::rebuild_dates(ctx, &account)?;
    Ok(Deleted { id: id.to_owned() })
}

pub fn settings(ctx: &Context<'_>) -> Result<Settings> {
    Ok(ctx.tx.query_row("SELECT name,display_currency,timezone_override,accounting_mode,CAST(revision AS TEXT) FROM ledger_meta",[],|r| Ok(Settings {
        name:r.get(0)?, display_currency:r.get(1)?, timezone_override:r.get(2)?, accounting_mode:AccountingMode::read(r.get_ref(3)?.as_str()?)?,revision:r.get(4)?
    }))?)
}
fn apply_required<T>(value: &mut T, patch: Option<Patch<T>>, field: &str) -> Result<()> {
    match patch {
        Some(Patch::Set(new)) => *value = new,
        Some(Patch::Clear) => return Err(invalid(field, "required_field")),
        _ => {}
    }
    Ok(())
}
pub fn settings_save(ctx: &Context<'_>, patch: SettingsPatch) -> Result<Settings> {
    let mut value = settings(ctx)?;
    let previous = value.accounting_mode.as_str();
    apply_required(&mut value.name, patch.name, "name")?;
    apply_required(
        &mut value.display_currency,
        patch.display_currency,
        "displayCurrency",
    )?;
    apply_required(
        &mut value.accounting_mode,
        patch.accounting_mode,
        "accountingMode",
    )?;
    match patch.timezone_override {
        Some(Patch::Set(zone)) => {
            zone.parse::<chrono_tz::Tz>()
                .map_err(|_| invalid("timezoneOverride", "invalid_iana_timezone"))?;
            value.timezone_override = Some(zone);
        }
        Some(Patch::Clear) => value.timezone_override = None,
        _ => {}
    }
    let ledger_name = name(&value.name)?;
    if ledger_name.len() > 256 {
        return Err(invalid("name", "invalid_ledger_name"));
    }
    value.name = ledger_name.to_owned();
    scale(&ctx.tx, &value.display_currency)?;
    ctx.tx.execute("UPDATE ledger_meta SET name=?1,display_currency=?2,timezone_override=?3,accounting_mode=?4",
        params![value.name,value.display_currency,value.timezone_override,value.accounting_mode.as_str()])?;
    if previous != value.accounting_mode.as_str() {
        let ids = strings(
            &ctx.tx,
            "SELECT id FROM accounts WHERE kind=?1 AND accounting_override IS NULL",
            "credit_card",
        )?;
        for id in ids {
            super::transactions::rebuild_dates(ctx, &id)?;
        }
    }
    value.revision = next_version(ctx)?;
    Ok(value)
}
