use super::{allocations, recurring};
use crate::sql::{OptionalExtension, params, params_from_iter};
use crate::{
    domain::{Result, parse_date},
    ledger_engine::{
        currency::{self, ConversionStatus, Resolver},
        transactions as tx, *,
    },
};
use chrono::{Datelike, Months, NaiveDate};
use rust_decimal::{Decimal, RoundingStrategy, prelude::ToPrimitive};
use std::collections::{BTreeMap, BTreeSet};

dto! {
    pub struct Key {pub category_id:String,pub month:Option<String>}
    pub struct Entry {pub key:Key,pub amount:Money}
    pub struct Budget {pub category_id:String,pub name:String,pub icon:Option<String>,pub monthly_override:bool,pub limit:Money,
        pub actual:Money,pub projected:Money,pub remaining:Money,pub carryover_eligible:bool,
        pub prior_rates:String,pub missing_rates:String}
    pub struct BudgetView {pub month:String,pub rows:Vec<Budget>,pub revision:String}
    pub struct Suggestion {pub category_id:String,pub average:Money,pub prior_rates:String,pub missing_rates:String}
}
#[derive(Default)]
struct Spend {
    actual: i128,
    projected: i128,
    prior: u64,
    missing: u64,
}

fn month(text: &str) -> Result<NaiveDate> {
    let date = if text.len() == 7 {
        parse_date(&format!("{text}-01"))?
    } else {
        parse_date(text)?
    };
    if date.day() != 1 {
        return Err(invalid("month", "month_start_required"));
    }
    Ok(date)
}
fn shifted(month: NaiveDate, delta: u32, back: bool) -> Result<NaiveDate> {
    let value = if back {
        month.checked_sub_months(Months::new(delta))
    } else {
        month.checked_add_months(Months::new(delta))
    };
    value
        .filter(|date| (1..=9999).contains(&date.year()))
        .ok_or_else(|| invalid("month", "month_out_of_range"))
}
fn eligible_category(ctx: &Context<'_>, id: &str) -> Result<String> {
    ctx.tx.query_row("SELECT name FROM categories WHERE id=?1 AND kind='expense' AND ignored=0 AND archived=0",[id],|row|row.get(0))
        .optional()?.ok_or_else(||invalid("categoryId","invalid_budget_category"))
}

struct Calculator<'ctx, 'tx> {
    ctx: &'ctx Context<'tx>,
    rates: Resolver<'ctx, 'tx>,
    target: &'ctx BTreeMap<String, String>,
    values: BTreeMap<String, Spend>,
}
impl<'ctx, 'tx> Calculator<'ctx, 'tx> {
    fn new(ctx: &'ctx Context<'tx>, target: &'ctx BTreeMap<String, String>) -> Self {
        Self {
            ctx,
            rates: Resolver::new(ctx),
            target,
            values: BTreeMap::new(),
        }
    }
    fn add(
        &mut self,
        category: &str,
        currency: &str,
        amount: i64,
        date: NaiveDate,
        actual: bool,
    ) -> Result<()> {
        let Some(target) = self.target.get(category) else {
            return Ok(());
        };
        let converted = self.rates.convert(&money(currency, amount), target, date)?;
        let value = self.values.entry(category.to_owned()).or_default();
        value.prior += u64::from(converted.status == ConversionStatus::Prior);
        value.missing += u64::from(converted.status == ConversionStatus::Missing);
        if let Some(amount) = converted.displayed {
            let amount = currency::minor_i128(&amount.minor)?;
            value.projected += amount;
            if actual {
                value.actual += amount;
            }
        }
        Ok(())
    }
    fn transactions(&mut self, from: NaiveDate, to: NaiveDate, project: bool) -> Result<()> {
        let mut query=tx::Query::new(format!("SELECT t.id,t.reporting_date,t.bank_date,t.status,t.currency_code,t.category_id,t.amount_minor,
            (SELECT coalesce(a.share_minor,0) FROM shared_expenses e JOIN shared_group_members m ON m.group_id=e.group_id AND m.is_self=1
              LEFT JOIN shared_allocations a ON a.expense_id=e.id AND a.member_id=m.id WHERE e.transaction_id=t.id)
            {}",tx::SOURCE));
        let from = query.bind(from.to_string());
        let to = query.bind(to.to_string());
        query.and(&format!("t.reporting_date BETWEEN {from} AND {to} AND t.direction='debit' AND t.entry_kind='ordinary'"));
        if project {
            query.and(&format!("t.status IN('posted','projected') AND t.ignored=0 AND t.exclude_from_pnl=0 AND {} AND {} AND {}
                AND NOT EXISTS(SELECT 1 FROM shared_settlements ss WHERE ss.transaction_id=t.id)",tx::NO_TRANSFER,tx::VISIBLE_CATEGORY,tx::VISIBLE_SPLIT));
        } else {
            tx::pnl_query(&mut query, self.ctx.today);
        }
        let mut stmt = self.ctx.tx.prepare(&query.sql)?;
        let mut rows = stmt.query(params_from_iter(query.args))?;
        let mut split=self.ctx.tx.prepare_cached("SELECT s.category_id,s.amount_minor,c.ignored OR c.kind='transfer'
            FROM transaction_splits s JOIN categories c ON c.id=s.category_id WHERE s.transaction_id=?1 ORDER BY s.position")?;
        while let Some(row) = rows.next()? {
            let id = row.get_ref(0)?.as_str()?;
            let date = parse_date(row.get_ref(1)?.as_str()?)?;
            let bank = parse_date(row.get_ref(2)?.as_str()?)?;
            let actual = row.get_ref(3)?.as_str()? == "posted"
                && bank <= self.ctx.today
                && date <= self.ctx.today;
            if !project && !actual {
                continue;
            }
            let currency = row.get_ref(4)?.as_str()?;
            let category = row.get::<_, Option<String>>(5)?;
            let total = row.get::<_, i64>(6)?;
            let owned = row.get::<_, Option<i64>>(7)?.unwrap_or(total);
            if owned < 0 || owned > total {
                return Err(integrity("invalid_owned_share"));
            }
            if owned == 0 {
                continue;
            }
            if let Some(category) = category {
                self.add(&category, currency, owned, date, actual)?;
                continue;
            }
            let parts = split
                .query_map([id], |s| {
                    Ok((
                        s.get::<_, String>(0)?,
                        s.get::<_, i64>(1)?,
                        s.get::<_, bool>(2)?,
                    ))
                })?
                .collect::<crate::sql::Result<Vec<_>>>()?;
            if parts.is_empty() {
                continue;
            }
            let allocated = allocations::distribute(
                total,
                owned,
                &parts.iter().map(|(_, value, _)| *value).collect::<Vec<_>>(),
            );
            for ((category, _, ignored), value) in parts.into_iter().zip(allocated) {
                if !ignored && value != 0 {
                    self.add(&category, currency, value, date, actual)?;
                }
            }
        }
        Ok(())
    }
    fn recurring(&mut self, from: NaiveDate, to: NaiveDate) -> Result<()> {
        if to < self.ctx.today {
            return Ok(());
        }
        let mut rules = BTreeMap::new();
        let earliest = from.checked_sub_days(chrono::Days::new(62)).unwrap_or(from);
        for occurrence in recurring::for_range(self.ctx, None, earliest.max(self.ctx.today), to)? {
            if !matches!(occurrence.state, recurring::OccurrenceState::Virtual) {
                continue;
            }
            if !rules.contains_key(&occurrence.rule_id) {
                rules.insert(
                    occurrence.rule_id.clone(),
                    recurring::get(self.ctx, &occurrence.rule_id)?.draft,
                );
            }
            let draft = &rules[&occurrence.rule_id];
            if !matches!(draft.direction, Direction::Debit) {
                continue;
            }
            let bank = parse_date(&occurrence.scheduled_date)?;
            let (account, archived) = accounts::raw(self.ctx, &draft.account_id)?;
            if archived.is_some() {
                continue;
            }
            let (effective, mode, _) = accounts::booking(
                self.ctx,
                &draft.account_id,
                &account,
                &draft.direction,
                bank,
                None,
                None,
            )?;
            let date = if matches!(mode, AccountingMode::Accrual) {
                parse_date(&effective)?
            } else {
                bank
            };
            if date < from || date > to {
                continue;
            }
            if let Some(category) = draft.category_id.as_deref() {
                self.add(
                    category,
                    &draft.amount.currency,
                    positive(&draft.amount.minor)?,
                    date,
                    false,
                )?;
            }
            for split in &draft.splits {
                self.add(
                    &split.category_id,
                    &draft.amount.currency,
                    positive(&split.minor)?,
                    date,
                    false,
                )?;
            }
        }
        Ok(())
    }
}
pub fn get(ctx: &Context<'_>, key: &str) -> Result<BudgetView> {
    let from = month(key)?;
    let to = shifted(from, 1, false)?.pred_opt().expect("bounded month");
    let key = from.to_string();
    let limits=ctx.tx.prepare_cached("WITH limits AS(
        SELECT category_id,currency_code,amount_minor,1 AS monthly FROM budgets WHERE month_start=?1
        UNION ALL SELECT d.category_id,d.currency_code,d.amount_minor,0 FROM budget_defaults d
        WHERE NOT EXISTS(SELECT 1 FROM budgets b WHERE b.month_start=?1 AND b.category_id=d.category_id))
        SELECT c.id,c.name,l.currency_code,l.amount_minor,l.monthly,c.icon FROM limits l JOIN categories c ON c.id=l.category_id
        WHERE c.kind='expense' AND c.ignored=0 ORDER BY c.name,c.id")?.query_map([&key],|r|Ok((
            r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,i64>(3)?,r.get::<_,bool>(4)?,r.get::<_,Option<String>>(5)?,
        )))?.collect::<crate::sql::Result<Vec<_>>>()?;
    let currencies = limits
        .iter()
        .map(|(id, _, currency, _, _, _)| (id.clone(), currency.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut calc = Calculator::new(ctx, &currencies);
    if !currencies.is_empty() {
        calc.transactions(from, to, to >= ctx.today)?;
        calc.recurring(from, to)?;
    }
    let rows = limits
        .into_iter()
        .map(|(id, name, currency, limit, monthly_override, icon)| {
            let value = calc.values.remove(&id).unwrap_or_default();
            let remaining = i128::from(limit) - value.projected;
            Budget {
                category_id: id,
                name,
                icon,
                monthly_override,
                limit: money(&currency, limit),
                actual: money(&currency, value.actual),
                projected: money(&currency, value.projected),
                remaining: money(&currency, remaining),
                carryover_eligible: remaining > 0,
                prior_rates: value.prior.to_string(),
                missing_rates: value.missing.to_string(),
            }
        })
        .collect();
    Ok(BudgetView {
        month: key,
        rows,
        revision: revision(&ctx.tx)?,
    })
}
fn batch<'a>(keys: impl Iterator<Item = &'a Key>, today: NaiveDate) -> Result<String> {
    let mut checked = BTreeSet::new();
    let mut months = BTreeSet::new();
    for key in keys {
        let month = key.month.as_deref().map(month).transpose()?;
        if let Some(month) = month {
            months.insert(month);
        }
        if !checked.insert((&key.category_id, month)) {
            return Err(invalid("entries", "duplicate_budget_key"));
        }
    }
    if checked.is_empty() || checked.len() > 200 || months.len() > 1 {
        return Err(invalid("entries", "invalid_budget_batch"));
    }
    Ok(months
        .first()
        .copied()
        .unwrap_or(today.with_day(1).expect("valid calendar day"))
        .to_string())
}
fn response(ctx: &Context<'_>, month: &str) -> Result<BudgetView> {
    let mut result = get(ctx, month)?;
    result.revision = accounts::next_version(ctx)?;
    Ok(result)
}
pub fn save(ctx: &Context<'_>, entries: &[Entry], overwrite: bool) -> Result<BudgetView> {
    let shown = batch(entries.iter().map(|entry| &entry.key), ctx.today)?;
    for row in entries {
        eligible_category(ctx, &row.key.category_id)?;
        let amount = positive(&row.amount.minor)?;
        scale(&ctx.tx, &row.amount.currency)?;
        let affected = if let Some(date) = row.key.month.as_deref() {
            ctx.tx.execute("INSERT INTO budgets VALUES(?1,?2,?3,?4) ON CONFLICT(category_id,month_start) DO UPDATE
                SET currency_code=excluded.currency_code,amount_minor=excluded.amount_minor WHERE ?5",
                params![row.key.category_id,month(date)?.to_string(),row.amount.currency,amount,overwrite])?
        } else {
            ctx.tx.execute("INSERT INTO budget_defaults VALUES(?1,?2,?3) ON CONFLICT(category_id) DO UPDATE
                SET currency_code=excluded.currency_code,amount_minor=excluded.amount_minor WHERE ?4",
                params![row.key.category_id,row.amount.currency,amount,overwrite])?
        };
        if affected == 0 {
            return Err(conflict("budget_overwrite_required"));
        }
    }
    response(ctx, &shown)
}
pub fn delete(ctx: &Context<'_>, keys: &[Key]) -> Result<BudgetView> {
    let shown = batch(keys.iter(), ctx.today)?;
    for row in keys {
        let count = if let Some(date) = row.month.as_deref() {
            ctx.tx.execute(
                "DELETE FROM budgets WHERE category_id=?1 AND month_start=?2",
                params![row.category_id, month(date)?.to_string()],
            )?
        } else {
            ctx.tx.execute(
                "DELETE FROM budget_defaults WHERE category_id=?1",
                [&row.category_id],
            )?
        };
        if count == 0 {
            return Err(missing("budgetKey"));
        }
    }
    response(ctx, &shown)
}
pub fn copy(
    ctx: &Context<'_>,
    from: &str,
    to: &str,
    adjust: &str,
    overwrite: bool,
) -> Result<BudgetView> {
    let from = month(from)?;
    let to = month(to)?;
    let value = Decimal::from_str_exact(adjust)
        .map_err(|_| invalid("adjustPercent", "invalid_budget_adjustment"))?;
    if from == to
        || value < Decimal::from(-100)
        || value > Decimal::from(1000)
        || value.scale() > 12
    {
        return Err(invalid("adjustPercent", "invalid_budget_copy"));
    }
    let source = get(ctx, &from.to_string())?;
    let mut entries = Vec::new();
    for row in source.rows {
        let amount = Decimal::from(positive(&row.limit.minor)?)
            * (Decimal::ONE + value / Decimal::ONE_HUNDRED);
        let amount = amount
            .round_dp_with_strategy(0, RoundingStrategy::MidpointAwayFromZero)
            .to_i64()
            .filter(|value| *value > 0)
            .ok_or_else(|| invalid("adjustPercent", "budget_copy_nonpositive_limit"))?;
        let key = Key {
            category_id: row.category_id,
            month: Some(to.to_string()),
        };
        entries.push(Entry {
            key,
            amount: money(&row.limit.currency, amount),
        });
    }
    save(ctx, &entries, overwrite)
}
pub fn suggestions(
    ctx: &Context<'_>,
    months: u8,
    selected: Option<&[String]>,
) -> Result<Vec<Suggestion>> {
    if !(1..=24).contains(&months) || selected.is_some_and(|ids| ids.len() > 200) {
        return Err(invalid("months", "invalid_budget_suggestion_range"));
    }
    let end = ctx.today.with_day(1).expect("valid day");
    let from = shifted(end, u32::from(months), true)?;
    let to = end
        .pred_opt()
        .ok_or_else(|| invalid("months", "invalid_budget_suggestion_range"))?;
    let default = accounts::settings(ctx)?.display_currency;
    let keys=ctx.tx.prepare_cached("SELECT c.id,coalesce(b.currency_code,?1) FROM categories c
        LEFT JOIN budget_defaults b ON b.category_id=c.id WHERE c.kind='expense' AND c.ignored=0 AND c.archived=0")?
        .query_map([default],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?
        .collect::<crate::sql::Result<Vec<_>>>()?.into_iter().filter(|(id,_)|selected.is_none_or(|selected|selected.contains(id)))
        .collect::<BTreeMap<_,_>>();
    let mut calc = Calculator::new(ctx, &keys);
    if !keys.is_empty() {
        calc.transactions(from, to, false)?;
    }
    let months = i128::from(months);
    Ok(calc
        .values
        .into_iter()
        .map(|(category_id, value)| Suggestion {
            average: money(&keys[&category_id], (value.actual + months / 2) / months),
            category_id,
            prior_rates: value.prior.to_string(),
            missing_rates: value.missing.to_string(),
        })
        .collect())
}
impl ReceiptValue for BudgetView {
    type Reference = String;
    fn reference(&self, _: &Context<'_>) -> String {
        self.month.clone()
    }
    fn reload(ctx: &Context<'_>, month: String) -> Result<Self> {
        get(ctx, &month)
    }
}
