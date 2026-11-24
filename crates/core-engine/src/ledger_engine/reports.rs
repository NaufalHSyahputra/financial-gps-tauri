use crate::planning::{assets, recurring};
use crate::sql::{Value, params, params_from_iter};
use chrono::{Datelike, Days, NaiveDate};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use super::{
    accounts::{self, AccountDetail, Credit},
    fx::{self, ConversionStatus, Resolver},
    transactions::{self as tx, LedgerFilter, Query, TxRow},
    *,
};

dto! {
    pub enum ReportKind { NetWorth, IncomeVsExpense, CashFlow, CategoryTrends }
    pub enum Granularity { Day, Week, Month }
    pub struct Totals {
        pub native: Vec<Money>, pub reliable: Money, pub missing_estimate: Option<Money>,
        pub prior_rate_count: String, pub missing_rate_count: String,
    }
    pub struct ReportPoint {
        pub date: String, pub net_worth: Option<Totals>, pub income: Option<Totals>, pub expense: Option<Totals>,
        pub booked: Option<Totals>, pub forecast: Option<Totals>, pub projected_movement: Option<Totals>,
        pub categories: Vec<CategoryPoint>,
    }
    pub struct CategoryPoint { pub category_id: Option<String>, pub direction: Direction, pub amount: Totals }
    pub struct ReportView { pub kind: ReportKind, pub currency: String, pub points: Vec<ReportPoint>, pub revision: String,
        pub balance_history_floor: Option<String>, pub separate_postings: Vec<SeparatePosting> }
    pub struct SeparatePosting { pub id: String, pub date: String, pub label: String, pub amount: Money }
    pub enum DashboardWidget { Accounts, Credit, NetWorth, IncomeVsExpense, CashFlow }
    pub struct DashboardView {
        pub as_of: String, pub accounts: Vec<AccountDetail>, pub credit: Vec<Credit>, pub net_worth: Option<ReportView>,
        pub income_vs_expense: Option<ReportView>, pub cash_flow: Option<ReportView>, pub revision: String,
    }
    pub struct CalendarEvent { pub transaction: TxRow, pub projected: bool, pub movement_date: String, pub forecast_date: String,
        pub occurrence: Option<recurring::Occurrence> }
    pub struct CalendarDay { pub date: String, pub in_range: bool, pub events: Vec<CalendarEvent>, pub booked_movements: Vec<Money>, pub forecast_movements: Vec<Money> }
    pub struct CalendarView { pub basis: DateBasis, pub days: Vec<CalendarDay>, pub revision: String }
    #[derive(PartialEq, Eq)]
    pub enum SearchKind { Account, Category, Payee, Transaction, Recurring, Asset, Goal, Group }
    pub struct SearchHit { pub id: String, pub kind: SearchKind, pub label: String }
}

#[derive(Default)]
struct Aggregate {
    native: BTreeMap<String, i128>,
    reliable: i128,
    estimated: i128,
    prior: u64,
    missing: u64,
}
fn add_amount(total: &mut i128, amount: i128) -> Result<()> {
    *total = total
        .checked_add(amount)
        .ok_or_else(|| integrity("aggregate_overflow"))?;
    Ok(())
}
impl Aggregate {
    fn add(
        &mut self,
        fx: &mut Resolver<'_, '_>,
        currency: &str,
        amount: i128,
        target: &str,
        date: NaiveDate,
    ) -> Result<()> {
        add_amount(self.native.entry(currency.to_owned()).or_default(), amount)?;
        let conversion = fx.convert(&money(currency, amount), target, date)?;
        if let Some(reliable) = conversion.displayed {
            add_amount(&mut self.reliable, fx::minor_i128(&reliable.minor)?)?;
        }
        if let Some(estimate) = conversion.estimate {
            add_amount(&mut self.estimated, fx::minor_i128(&estimate.minor)?)?;
        }
        self.prior += u64::from(conversion.status == ConversionStatus::Prior);
        self.missing += u64::from(conversion.status == ConversionStatus::Missing);
        Ok(())
    }
    fn finish(self, target: &str) -> Totals {
        Totals {
            native: self
                .native
                .into_iter()
                .map(|(code, minor)| money(&code, minor))
                .collect(),
            reliable: money(target, self.reliable),
            missing_estimate: (self.missing > 0).then(|| money(target, self.estimated)),
            prior_rate_count: self.prior.to_string(),
            missing_rate_count: self.missing.to_string(),
        }
    }
}
#[derive(Default)]
struct FinancialPoint {
    value: Aggregate,
    income: Aggregate,
    expense: Aggregate,
    booked: Aggregate,
    forecast: Aggregate,
    projected: Aggregate,
    categories: BTreeMap<(Option<String>, bool), Aggregate>,
}
struct Period {
    from: NaiveDate,
    to: NaiveDate,
}

fn range(ctx: &Context<'_>, range: Option<&DateRange>, limit: i64) -> Result<Period> {
    if let Some(range) = range {
        date_range(range)?;
    }
    let to = date_option(range.and_then(|r| r.to.as_deref()))?.unwrap_or(ctx.today);
    let from = date_option(range.and_then(|r| r.from.as_deref()))?
        .unwrap_or_else(|| to.checked_sub_days(Days::new(29)).unwrap_or(to));
    if to < from || (to - from).num_days() >= limit {
        return Err(invalid("range", "report_range_out_of_bounds"));
    }
    Ok(Period { from, to })
}
fn bucket(date: NaiveDate, granularity: &Granularity) -> Result<NaiveDate> {
    match granularity {
        Granularity::Day => Ok(date),
        Granularity::Week => date
            .checked_sub_days(Days::new(u64::from(date.weekday().num_days_from_sunday())))
            .filter(|date| date.year() >= 1)
            .ok_or_else(|| invalid("range", "date_out_of_range")),
        Granularity::Month => date
            .with_day(1)
            .ok_or_else(|| invalid("date", "invalid_date")),
    }
}
fn starts(period: &Period, granularity: &Granularity) -> Result<Vec<NaiveDate>> {
    let mut result = BTreeSet::new();
    let mut day = period.from;
    loop {
        result.insert(bucket(day, granularity)?);
        if day == period.to {
            break;
        }
        day = day
            .succ_opt()
            .ok_or_else(|| invalid("range", "date_out_of_range"))?;
    }
    Ok(result.into_iter().collect())
}
fn add_range(query: &mut Query, expression: &str, period: &Period) {
    let from = query.bind(period.from.to_string());
    let to = query.bind(period.to.to_string());
    query.and(&format!("{expression} BETWEEN {from} AND {to}"));
}

pub fn report(
    ctx: &Context<'_>,
    kind: &ReportKind,
    filter: &LedgerFilter,
    granularity: &Granularity,
    currency: Option<&str>,
) -> Result<ReportView> {
    report_at(ctx, kind, filter, granularity, currency, ctx.today)
}
fn report_at(
    ctx: &Context<'_>,
    kind: &ReportKind,
    filter: &LedgerFilter,
    granularity: &Granularity,
    currency: Option<&str>,
    cutoff: NaiveDate,
) -> Result<ReportView> {
    let default = accounts::settings(ctx)?.display_currency;
    let target = currency.unwrap_or(&default);
    scale(&ctx.tx, target)?;
    let period = range(ctx, filter.range.as_ref(), 3661)?;
    let starts = starts(&period, granularity)?;
    let mut values: BTreeMap<NaiveDate, FinancialPoint> = starts
        .iter()
        .map(|date| (*date, FinancialPoint::default()))
        .collect();
    let mut resolver = Resolver::new(ctx);
    match kind {
        ReportKind::NetWorth => net_worth(
            ctx,
            filter,
            &period,
            &starts,
            &mut values,
            &mut resolver,
            target,
        )?,
        ReportKind::CashFlow => cash_flow(
            ctx,
            filter,
            &period,
            granularity,
            &mut values,
            &mut resolver,
            target,
            cutoff,
        )?,
        ReportKind::IncomeVsExpense | ReportKind::CategoryTrends => pnl(
            ctx,
            filter,
            &period,
            granularity,
            &mut values,
            &mut resolver,
            target,
            cutoff,
        )?,
    }
    let points = values
        .into_iter()
        .map(|(date, value)| {
            let FinancialPoint {
                value,
                income,
                expense,
                booked,
                forecast,
                projected,
                categories,
            } = value;
            let is_pnl = matches!(
                kind,
                ReportKind::IncomeVsExpense | ReportKind::CategoryTrends
            );
            let is_flow = matches!(kind, ReportKind::CashFlow);
            let categories = if matches!(kind, ReportKind::CategoryTrends) {
                categories
                    .into_iter()
                    .map(|((category_id, debit), amount)| CategoryPoint {
                        category_id,
                        direction: if debit {
                            Direction::Debit
                        } else {
                            Direction::Credit
                        },
                        amount: amount.finish(target),
                    })
                    .collect()
            } else {
                Vec::new()
            };
            ReportPoint {
                date: date.to_string(),
                net_worth: matches!(kind, ReportKind::NetWorth).then(|| value.finish(target)),
                income: is_pnl.then(|| income.finish(target)),
                expense: is_pnl.then(|| expense.finish(target)),
                booked: is_flow.then(|| booked.finish(target)),
                forecast: is_flow.then(|| forecast.finish(target)),
                projected_movement: is_flow.then(|| projected.finish(target)),
                categories,
            }
        })
        .collect();
    let balance_history_floor = if matches!(kind, ReportKind::NetWorth) {
        let mut query =
            Query::new("SELECT max(o.effective_on) FROM account_opening_snapshots o".into());
        query.sql.push_str(" WHERE 1");
        query.choices("o.account_id IN ({values})", &filter.account_ids);
        let start = query.bind(period.from.to_string());
        query.and(&format!("o.effective_on>{start}"));
        ctx.tx
            .prepare(&query.sql)?
            .query_row(params_from_iter(query.args), |r| r.get(0))?
    } else {
        None
    };
    let separate_postings = if matches!(kind, ReportKind::IncomeVsExpense | ReportKind::CashFlow) {
        separate_postings(ctx, filter, &period, cutoff)?
    } else {
        Vec::new()
    };
    Ok(ReportView {
        balance_history_floor,
        separate_postings,
        kind: kind.clone(),
        currency: target.to_owned(),
        points,
        revision: revision(&ctx.tx)?,
    })
}

// These postings reconcile cash and balances but are excluded from ordinary income and expense.
fn separate_postings(
    ctx: &Context<'_>,
    filter: &LedgerFilter,
    period: &Period,
    cutoff: NaiveDate,
) -> Result<Vec<SeparatePosting>> {
    let any: bool = ctx.tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM transactions WHERE entry_kind='adjustment')
           OR EXISTS(SELECT 1 FROM posting_refunds WHERE original_id IS NULL)",
        [],
        |row| row.get(0),
    )?;
    if !any {
        return Ok(Vec::new());
    }
    let mut query = Query::new(format!(
        "SELECT t.id,t.bank_date,t.entry_kind,t.direction,t.currency_code,t.amount_minor{}",
        tx::SOURCE
    ));
    let mut scoped = filter.clone();
    scoped.range = None;
    tx::filter_query(ctx, &mut query, &scoped)?;
    query.and(
        "t.ignored=0 AND t.status='posted' AND (t.entry_kind='adjustment' OR EXISTS(
        SELECT 1 FROM posting_refunds p WHERE p.refund_id=t.id AND p.original_id IS NULL
    ))",
    );
    let end = query.bind(cutoff.to_string());
    query.and(&format!("t.bank_date<={end}"));
    add_range(&mut query, "t.bank_date", period);
    query
        .sql
        .push_str(" ORDER BY t.bank_date DESC,t.id DESC LIMIT 100");
    let mut stmt = ctx.tx.prepare(&query.sql)?;
    let mut rows = stmt.query(params_from_iter(query.args))?;
    let mut result = Vec::new();
    while let Some(row) = rows.next()? {
        let amount = row.get::<_, i64>(5)?;
        let label = if row.get_ref(2)?.as_str()? == "adjustment" {
            "Balance adjustment"
        } else {
            "Unlinked refund"
        };
        let signed = if row.get_ref(3)?.as_str()? == "debit" {
            -amount
        } else {
            amount
        };
        result.push(SeparatePosting {
            id: row.get(0)?,
            date: row.get(1)?,
            label: label.into(),
            amount: money(row.get_ref(4)?.as_str()?, signed),
        });
    }
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
fn pnl(
    ctx: &Context<'_>,
    filter: &LedgerFilter,
    period: &Period,
    granularity: &Granularity,
    values: &mut BTreeMap<NaiveDate, FinancialPoint>,
    resolver: &mut Resolver<'_, '_>,
    target: &str,
    cutoff: NaiveDate,
) -> Result<()> {
    let source=tx::SOURCE.replace(" WHERE 1=1"," LEFT JOIN transaction_splits s ON s.transaction_id=t.id LEFT JOIN categories sc ON sc.id=s.category_id WHERE 1=1");
    let mut query = Query::new(format!(
        "SELECT t.reporting_date,t.currency_code,t.direction,coalesce(s.category_id,t.category_id),
        coalesce(s.amount_minor,t.amount_minor),t.amount_minor,
        CASE WHEN t.direction='debit' THEN coalesce((SELECT sum(r.amount_minor) FROM posting_refunds p
            JOIN transactions r ON r.id=p.refund_id WHERE p.original_id=t.id AND r.ignored=0
            AND r.status='posted' AND r.bank_date<=?1),0) ELSE 0 END,
        CASE WHEN s.position IS NULL THEN 0 ELSE coalesce((SELECT sum(prior.amount_minor) FROM transaction_splits prior
            WHERE prior.transaction_id=t.id AND prior.position<s.position),0) END{source}"
    ));
    query.args.push(cutoff.to_string().into());
    let mut filter = filter.clone();
    filter.range = None;
    tx::filter_query(ctx, &mut query, &filter)?;
    tx::pnl_query(&mut query, cutoff);
    query.and("(s.transaction_id IS NULL OR (sc.ignored=0 AND sc.kind<>'transfer'))");
    query.choices(
        "coalesce(s.category_id,t.category_id) IN ({values})",
        &filter.category_ids,
    );
    add_range(&mut query, "t.reporting_date", period);
    let mut stmt = ctx.tx.prepare(&query.sql)?;
    let mut rows = stmt.query(params_from_iter(query.args))?;
    let display = filter
        .display_amount
        .as_ref()
        .map(|bounds| {
            scale(&ctx.tx, &bounds.currency)?;
            Ok::<_, AppError>((&bounds.currency, tx::bounds_data(bounds)?))
        })
        .transpose()?;
    while let Some(row) = rows.next()? {
        let date = parse_date(row.get_ref(0)?.as_str()?)?;
        let code = row.get_ref(1)?.as_str()?;
        let debit = row.get_ref(2)?.as_str()? == "debit";
        let category = row.get::<_, Option<String>>(3)?;
        let gross = i128::from(row.get::<_, i64>(4)?);
        let whole = i128::from(row.get::<_, i64>(5)?);
        let refund = i128::from(row.get::<_, i64>(6)?).min(whole);
        let preceding = i128::from(row.get::<_, i64>(7)?);
        let amount = gross - ((preceding + gross) * refund / whole - preceding * refund / whole);
        if let Some((code_target, (min, max))) = display {
            let result =
                resolver.convert(&money(code, row.get::<_, i64>(5)?), code_target, date)?;
            let Some(displayed) = result.displayed else {
                continue;
            };
            if !(i128::from(min)..=i128::from(max)).contains(&fx::minor_i128(&displayed.minor)?) {
                continue;
            }
        }
        let point = values
            .get_mut(&bucket(date, granularity)?)
            .expect("bounded report date");
        if debit {
            point.expense.add(resolver, code, amount, target, date)?;
        } else {
            point.income.add(resolver, code, amount, target, date)?;
        }
        point
            .categories
            .entry((category, debit))
            .or_default()
            .add(resolver, code, amount, target, date)?;
    }
    Ok(())
}

fn net_worth(
    ctx: &Context<'_>,
    filter: &LedgerFilter,
    period: &Period,
    starts: &[NaiveDate],
    values: &mut BTreeMap<NaiveDate, FinancialPoint>,
    resolver: &mut Resolver<'_, '_>,
    target: &str,
) -> Result<()> {
    let mut query = Query::new(format!(
        "SELECT t.bank_date,t.currency_code,
        CASE t.direction WHEN 'debit' THEN -t.amount_minor ELSE t.amount_minor END{}",
        tx::SOURCE
    ));
    query.and(accounts::BALANCE_VISIBLE);
    if ctx.tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM account_opening_snapshots)",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        query.and(accounts::SNAPSHOT_VISIBLE);
    }
    if filter.account_ids.len() > 100 {
        return Err(invalid("accountIds", "too_many_filter_values"));
    }
    query.choices("t.account_id IN ({values})", &filter.account_ids);
    let last = query.bind(period.to.min(ctx.today).to_string());
    query.and(&format!("t.bank_date<={last}"));
    query.sql.push_str(" ORDER BY t.bank_date,t.id");
    let mut stmt = ctx.tx.prepare(&query.sql)?;
    let mut rows = stmt.query(params_from_iter(query.args))?;
    let read = |r: &crate::sql::Row<'_>| -> crate::sql::Result<(String, String, i64)> {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?))
    };
    let mut next = rows.next()?.as_ref().map(read).transpose()?;
    let asset_ids = if filter.account_ids.is_empty() {
        ctx.tx
            .prepare_cached("SELECT id,archived_on FROM assets ORDER BY id")?
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })?
            .collect::<crate::sql::Result<Vec<_>>>()?
    } else {
        Vec::new()
    };
    let mut balances = BTreeMap::<String, i128>::new();
    for (i, start) in starts.iter().enumerate() {
        let end = starts
            .get(i + 1)
            .and_then(|day| day.pred_opt())
            .unwrap_or(period.to);
        let cutoff = end.to_string();
        while next.as_ref().is_some_and(|(date, _, _)| date <= &cutoff) {
            let (_, currency, amount) = next.take().expect("available row");
            add_amount(balances.entry(currency).or_default(), i128::from(amount))?;
            next = rows.next()?.as_ref().map(read).transpose()?;
        }
        let point = values.get_mut(start).expect("known report bucket");
        for (id, archived) in &asset_ids {
            if archived.as_ref().is_some_and(|date| date <= &cutoff) {
                continue;
            }
            let asset = assets::get(ctx, id, Some(&cutoff))?;
            if asset.valued_on.is_some() {
                let amount = fx::minor_i128(&asset.value.minor)?;
                point
                    .value
                    .add(resolver, &asset.value.currency, amount, target, end)?;
            }
        }
        for (currency, amount) in &balances {
            point.value.add(resolver, currency, *amount, target, end)?;
        }
    }
    Ok(())
}

// Credits explicitly tied to the statement replace the matching part of its forecast.
struct StatementForecasts<'a, 'tx> {
    ctx: &'a Context<'tx>,
    cutoff: String,
    remaining: HashMap<String, HashMap<String, i128>>,
}
impl<'a, 'tx> StatementForecasts<'a, 'tx> {
    fn new(ctx: &'a Context<'tx>, cutoff: NaiveDate) -> Self {
        Self {
            ctx,
            cutoff: cutoff.to_string(),
            remaining: HashMap::new(),
        }
    }

    fn amount(&mut self, id: &str, statement: Option<&str>, original: i128) -> Result<i128> {
        let Some(statement) = statement else {
            return Ok(original);
        };
        if !self.remaining.contains_key(statement) {
            let remaining = self.remaining_for(statement)?;
            self.remaining.insert(statement.to_owned(), remaining);
        }
        Ok(self.remaining[statement]
            .get(id)
            .copied()
            .unwrap_or(original))
    }

    fn remaining_for(&self, statement: &str) -> Result<HashMap<String, i128>> {
        let mut paid = 0i128;
        let mut payments = self.ctx.tx.prepare_cached(
            "SELECT credit.amount_minor FROM transactions credit
            JOIN transfer_links link ON link.credit_id=credit.id WHERE credit.statement_id=?1
            AND credit.status='posted' AND credit.ignored=0 AND credit.bank_date<=?2",
        )?;
        let mut rows = payments.query(params![statement, self.cutoff])?;
        while let Some(row) = rows.next()? {
            add_amount(&mut paid, i128::from(row.get::<_, i64>(0)?))?;
        }
        let mut outstanding = HashMap::new();
        if paid == 0 {
            return Ok(outstanding);
        }
        let mut charges=self.ctx.tx.prepare_cached(&format!("SELECT t.id,t.amount_minor FROM transactions t
            JOIN accounts a ON a.id=t.account_id WHERE t.statement_id=?1 AND t.bank_date<=?2 AND t.direction='debit'
            AND {} AND {} ORDER BY t.bank_date,t.id",accounts::BALANCE_VISIBLE,tx::NO_TRANSFER))?;
        let mut rows = charges.query(params![statement, self.cutoff])?;
        while paid > 0
            && let Some(row) = rows.next()?
        {
            let amount = i128::from(row.get::<_, i64>(1)?);
            let applied = paid.min(amount);
            paid -= applied;
            outstanding.insert(row.get(0)?, -(amount - applied));
        }
        Ok(outstanding)
    }
}

#[allow(clippy::too_many_arguments)]
fn cash_flow(
    ctx: &Context<'_>,
    filter: &LedgerFilter,
    period: &Period,
    granularity: &Granularity,
    values: &mut BTreeMap<NaiveDate, FinancialPoint>,
    resolver: &mut Resolver<'_, '_>,
    target: &str,
    cutoff: NaiveDate,
) -> Result<()> {
    let mut query=Query::new(format!("SELECT t.bank_date,t.effective_date,t.currency_code,t.direction,t.amount_minor,t.status,a.kind,
        {link},t.id,t.statement_id {source}",link=tx::LINK_ID,source=tx::SOURCE));
    let mut filter = filter.clone();
    filter.range = None;
    tx::filter_query(ctx, &mut query, &filter)?;
    query.and("t.ignored=0 AND coalesce(cat.ignored,0)=0");
    let lo = query.bind(period.from.to_string());
    let hi = query.bind(period.to.to_string());
    query.and(&format!(
        "(t.bank_date BETWEEN {lo} AND {hi} OR t.effective_date BETWEEN {lo} AND {hi})"
    ));
    let display = filter
        .display_amount
        .as_ref()
        .map(|bounds| {
            scale(&ctx.tx, &bounds.currency)?;
            Ok::<_, AppError>((&bounds.currency, tx::bounds_data(bounds)?))
        })
        .transpose()?;
    let mut stmt = ctx.tx.prepare(&query.sql)?;
    let mut rows = stmt.query(params_from_iter(query.args))?;
    let mut forecast = StatementForecasts::new(ctx, cutoff);
    while let Some(row) = rows.next()? {
        let bank = parse_date(row.get_ref(0)?.as_str()?)?;
        let effective = parse_date(row.get_ref(1)?.as_str()?)?;
        let currency = row.get_ref(2)?.as_str()?;
        let debit = row.get_ref(3)?.as_str()? == "debit";
        let unsigned = i128::from(row.get::<_, i64>(4)?);
        let amount = if debit { -unsigned } else { unsigned };
        let status = row.get_ref(5)?.as_str()?;
        let card = row.get_ref(6)?.as_str()? == "credit_card";
        let linked = row.get::<_, Option<String>>(7)?.is_some();
        if let Some((code, (min, max))) = display {
            let converted = resolver.convert(&money(currency, unsigned), code, effective)?;
            if converted.displayed.is_none_or(|result| {
                fx::minor_i128(&result.minor)
                    .is_ok_and(|v| v < i128::from(min) || v > i128::from(max))
            }) {
                continue;
            }
        }
        let booked = bank <= cutoff && (status == "posted" || (card && status == "pending"));
        if booked && (period.from..=period.to).contains(&bank) {
            values
                .get_mut(&bucket(bank, granularity)?)
                .expect("bank in range")
                .booked
                .add(resolver, currency, amount, target, bank)?;
        }
        if effective > cutoff
            && (period.from..=period.to).contains(&effective)
            && (!linked || filter.account_ids.len() == 1)
        {
            let point = values
                .get_mut(&bucket(effective, granularity)?)
                .expect("effective in range");
            let forecast_amount = if card && debit && booked {
                forecast.amount(
                    row.get_ref(8)?.as_str()?,
                    row.get::<_, Option<String>>(9)?.as_deref(),
                    amount,
                )?
            } else {
                amount
            };
            if forecast_amount != 0 {
                point
                    .forecast
                    .add(resolver, currency, forecast_amount, target, effective)?;
            }
            if !booked {
                point
                    .projected
                    .add(resolver, currency, amount, target, effective)?;
            }
        }
    }
    Ok(())
}

pub fn dashboard(
    ctx: &Context<'_>,
    as_of: Option<&str>,
    ids: &[String],
    widgets: &[DashboardWidget],
) -> Result<DashboardView> {
    if ids.len() > 100 {
        return Err(invalid("accountIds", "too_many_filter_values"));
    }
    let date = date_option(as_of)?.unwrap_or(ctx.today);
    let history = DateRange {
        from: Some(date.with_day(1).expect("date has first day").to_string()),
        to: Some(date.to_string()),
    };
    let filter = LedgerFilter {
        account_ids: ids.to_vec(),
        range: Some(history),
        ..Default::default()
    };
    let widget =
        |matches: fn(&DashboardWidget) -> bool| widgets.is_empty() || widgets.iter().any(matches);
    let needs_accounts = widget(|w| {
        matches!(
            w,
            DashboardWidget::Accounts | DashboardWidget::Credit | DashboardWidget::NetWorth
        )
    });
    let accounts = if needs_accounts {
        accounts::list_selected(ctx, false, Some(&date.to_string()), ids)?
    } else {
        Vec::new()
    };
    let mut lines = HashSet::new();
    let credit = if widget(|w| matches!(w, DashboardWidget::Credit)) {
        accounts
            .iter()
            .filter_map(|a| a.credit.clone())
            .filter(|line| {
                line.line_id
                    .as_ref()
                    .is_none_or(|id| lines.insert(id.clone()))
            })
            .collect()
    } else {
        Vec::new()
    };
    let current = if widget(|widget| matches!(widget, DashboardWidget::NetWorth)) {
        let target = accounts::settings(ctx)?.display_currency;
        let mut resolver = Resolver::new(ctx);
        let mut totals = Aggregate::default();
        for account in &accounts {
            totals.add(
                &mut resolver,
                &account.balance.currency,
                fx::minor_i128(&account.balance.minor)?,
                &target,
                date,
            )?;
        }
        let as_of = date.to_string();
        let mut balance_history_floor = None;
        for account in &accounts {
            if let Some(floor) = accounts::opening_floor(ctx, &account.id)?
                && floor > as_of
                && balance_history_floor
                    .as_ref()
                    .is_none_or(|prior| prior < &floor)
            {
                balance_history_floor = Some(floor);
            }
        }
        Some(ReportView {
            balance_history_floor,
            separate_postings: Vec::new(),
            kind: ReportKind::NetWorth,
            currency: target.clone(),
            revision: revision(&ctx.tx)?,
            points: vec![ReportPoint {
                date: date.to_string(),
                net_worth: Some(totals.finish(&target)),
                income: None,
                expense: None,
                booked: None,
                forecast: None,
                projected_movement: None,
                categories: Vec::new(),
            }],
        })
    } else {
        None
    };
    let profit = widget(|w| matches!(w, DashboardWidget::IncomeVsExpense))
        .then(|| {
            report_at(
                ctx,
                &ReportKind::IncomeVsExpense,
                &filter,
                &Granularity::Month,
                None,
                date.min(ctx.today),
            )
        })
        .transpose()?;
    let flow_filter = LedgerFilter {
        account_ids: ids.to_vec(),
        range: Some(DateRange {
            from: Some(date.to_string()),
            to: Some(
                date.checked_add_days(Days::new(29))
                    .ok_or_else(|| invalid("asOf", "date_out_of_range"))?
                    .to_string(),
            ),
        }),
        ..Default::default()
    };
    let flow = widget(|w| matches!(w, DashboardWidget::CashFlow))
        .then(|| {
            report_at(
                ctx,
                &ReportKind::CashFlow,
                &flow_filter,
                &Granularity::Week,
                None,
                date.min(ctx.today),
            )
        })
        .transpose()?;
    Ok(DashboardView {
        as_of: date.to_string(),
        accounts: if widget(|w| matches!(w, DashboardWidget::Accounts)) {
            accounts
        } else {
            Vec::new()
        },
        credit,
        net_worth: current,
        income_vs_expense: profit,
        cash_flow: flow,
        revision: revision(&ctx.tx)?,
    })
}
pub fn calendar(
    ctx: &Context<'_>,
    requested: &DateRange,
    basis: &DateBasis,
    account_ids: &[String],
    projection: bool,
) -> Result<CalendarView> {
    if matches!(basis, DateBasis::Effective) {
        return Err(invalid("basis", "invalid_calendar_basis"));
    }
    let wanted = range(ctx, Some(requested), 367)?;
    let from = bucket(wanted.from, &Granularity::Week)?;
    let to = bucket(wanted.to, &Granularity::Week)?
        .checked_add_days(Days::new(6))
        .filter(|d| d.year() <= 9999)
        .ok_or_else(|| invalid("range", "date_out_of_range"))?;
    let period = Period { from, to };
    type CurrencyTotals = BTreeMap<String, i128>;
    type CalendarBucket = (CalendarDay, CurrencyTotals, CurrencyTotals);
    let mut days = BTreeMap::<NaiveDate, CalendarBucket>::new();
    for day in starts(&period, &Granularity::Day)? {
        days.insert(
            day,
            (
                CalendarDay {
                    date: day.to_string(),
                    in_range: (wanted.from..=wanted.to).contains(&day),
                    events: Vec::new(),
                    booked_movements: Vec::new(),
                    forecast_movements: Vec::new(),
                },
                BTreeMap::new(),
                BTreeMap::new(),
            ),
        );
    }
    let column = basis.column();
    let mut query = Query::new(format!(
        "SELECT {},{},a.kind,a.archived_on,coalesce(cat.ignored,0){}",
        tx::ROW_COLUMNS,
        tx::LINK_ID,
        tx::SOURCE
    ));
    tx::filter_query(
        ctx,
        &mut query,
        &LedgerFilter {
            account_ids: account_ids.to_vec(),
            ignored: Some(false),
            ..Default::default()
        },
    )?;
    let lo = query.bind(from.to_string());
    let hi = query.bind(to.to_string());
    query.and(&format!("(t.{column} BETWEEN {lo} AND {hi} OR t.bank_date BETWEEN {lo} AND {hi} OR t.effective_date BETWEEN {lo} AND {hi})"));
    query.sql.push_str(" ORDER BY t.bank_date,t.id LIMIT 20001");
    let mut stmt = ctx.tx.prepare(&query.sql)?;
    let mut rows = stmt.query(params_from_iter(query.args))?;
    let mut seen = 0;
    let mut forecast_values = StatementForecasts::new(ctx, ctx.today);
    while let Some(r) = rows.next()? {
        seen += 1;
        if seen > 20000 {
            return Err(invalid("range", "calendar_event_limit"));
        }
        let item = tx::read_row(&r)?;
        let bank = parse_date(&item.bank_date)?;
        let effective = parse_date(&item.effective_date)?;
        let date = if matches!(basis, DateBasis::Bank) {
            bank
        } else {
            parse_date(&item.reporting_date)?
        };
        let card = r.get_ref(20)?.as_str()? == "credit_card";
        let category_ignored = r.get::<_, bool>(22)?;
        let booked = bank <= ctx.today
            && !category_ignored
            && (matches!(item.status, TxStatus::Posted)
                || (card && matches!(item.status, TxStatus::Pending)));
        let projected = bank > ctx.today || matches!(item.status, TxStatus::Projected);
        let signed = i128::from(minor(&item.amount.minor)?)
            * if matches!(item.direction, Direction::Debit) {
                -1
            } else {
                1
            };
        if booked && let Some((_, totals, _)) = days.get_mut(&bank) {
            add_amount(
                totals.entry(item.amount.currency.clone()).or_default(),
                signed,
            )?;
        }
        if projection
            && effective > ctx.today
            && !category_ignored
            && let Some((_, _, forecast)) = days.get_mut(&effective)
        {
            let amount = if card
                && booked
                && matches!(item.direction, Direction::Debit)
                && item.transfer_id.is_none()
            {
                forecast_values.amount(&item.id, item.statement_id.as_deref(), signed)?
            } else {
                signed
            };
            if amount != 0 {
                add_amount(
                    forecast.entry(item.amount.currency.clone()).or_default(),
                    amount,
                )?;
            }
        }
        if !category_ignored
            && (!projected || projection)
            && let Some((day, _, _)) = days.get_mut(&date)
        {
            day.events.push(CalendarEvent {
                movement_date: item.bank_date.clone(),
                forecast_date: item.effective_date.clone(),
                transaction: item,
                projected,
                occurrence: None,
            });
        }
    }
    if projection && to >= ctx.today {
        let earliest = from
            .checked_sub_days(Days::new(62))
            .unwrap_or(from)
            .max(ctx.today);
        for occurrence in recurring::for_range(ctx, None, earliest, to)? {
            if !matches!(occurrence.state, recurring::OccurrenceState::Virtual) {
                continue;
            }
            let rule = recurring::get(ctx, &occurrence.rule_id)?;
            let draft = &rule.draft;
            if !account_ids.is_empty() && !account_ids.contains(&draft.account_id) {
                continue;
            }
            let (account, archived) = accounts::raw(ctx, &draft.account_id)?;
            if archived.is_some() {
                continue;
            }
            if let Some(category) = draft.category_id.as_deref() {
                let ignored: bool = ctx.tx.query_row(
                    "SELECT ignored FROM categories WHERE id=?1",
                    [category],
                    |row| row.get(0),
                )?;
                if ignored {
                    continue;
                }
            }
            let bank = parse_date(&occurrence.scheduled_date)?;
            let (effective, mode, statement) = accounts::booking(
                ctx,
                &draft.account_id,
                &account,
                &draft.direction,
                bank,
                None,
                None,
            )?;
            let reporting = if matches!(mode, AccountingMode::Accrual) {
                effective.clone()
            } else {
                occurrence.scheduled_date.clone()
            };
            let event_date = if matches!(basis, DateBasis::Bank) {
                bank
            } else {
                parse_date(&reporting)?
            };
            let effective_date = parse_date(&effective)?;
            if effective_date > ctx.today
                && let Some((_, _, forecast)) = days.get_mut(&effective_date)
            {
                let amount = i128::from(positive(&draft.amount.minor)?);
                let signed = if matches!(draft.direction, Direction::Debit) {
                    -amount
                } else {
                    amount
                };
                add_amount(
                    forecast.entry(draft.amount.currency.clone()).or_default(),
                    signed,
                )?;
            }
            if let Some((day, _, _)) = days.get_mut(&event_date) {
                if seen >= 20_000 {
                    return Err(invalid("range", "calendar_event_limit"));
                }
                seen += 1;
                let item = TxRow {
                    id: format!("{}/{}", rule.id, occurrence.nominal_date),
                    version: rule.version,
                    account_id: draft.account_id.clone(),
                    amount: draft.amount.clone(),
                    direction: draft.direction.clone(),
                    bank_date: occurrence.scheduled_date.clone(),
                    effective_date: effective.clone(),
                    reporting_date: reporting,
                    status: TxStatus::Projected,
                    entry_kind: tx::EntryKind::Ordinary,
                    description: draft.description.clone(),
                    payee_id: draft.payee_id.clone(),
                    category_id: draft.category_id.clone(),
                    category: None,
                    statement_id: statement,
                    ignored: false,
                    exclude_from_pnl: false,
                    flagged: false,
                    flag_due_date: None,
                    transfer_id: None,
                    running_balance: None,
                    pnl_amount: None,
                };
                day.events.push(CalendarEvent {
                    movement_date: item.bank_date.clone(),
                    forecast_date: effective,
                    transaction: item,
                    projected: true,
                    occurrence: Some(occurrence),
                });
            }
        }
        for (day, _, _) in days.values_mut() {
            day.events.sort_by(|a, b| {
                (&a.movement_date, &a.transaction.id).cmp(&(&b.movement_date, &b.transaction.id))
            });
        }
    }
    let days = days
        .into_values()
        .map(|(mut value, booked, forecast)| {
            value.booked_movements = booked
                .into_iter()
                .map(|(code, value)| money(&code, value))
                .collect();
            value.forecast_movements = forecast
                .into_iter()
                .map(|(code, value)| money(&code, value))
                .collect();
            value
        })
        .collect();
    Ok(CalendarView {
        basis: basis.clone(),
        days,
        revision: revision(&ctx.tx)?,
    })
}

pub fn search(
    ctx: &Context<'_>,
    search: &str,
    types: &[SearchKind],
    page: &PageReq,
) -> Result<Page<SearchHit>> {
    let cursor = ReadPage::new(ctx, page, &("global_search", search, types))?;
    let Some(terms) = tx::search_terms(search)? else {
        return cursor.finish(Vec::new(), |_: &SearchHit| unreachable!());
    };
    let plain = normalize(search);
    let mut queries = Vec::new();
    let mut args = Vec::<Value>::new();
    for (kind, code, table, label, condition) in [
        (
            SearchKind::Account,
            "account",
            "accounts",
            "name",
            "instr(financial_normalize(name),?1)>0",
        ),
        (
            SearchKind::Category,
            "category",
            "categories",
            "name",
            "instr(financial_normalize(name),?1)>0",
        ),
        (
            SearchKind::Payee,
            "payee",
            "payees",
            "name",
            "instr(normalized_name,?1)>0",
        ),
        (
            SearchKind::Transaction,
            "transaction",
            "transactions",
            "description",
            "id IN (SELECT transaction_id FROM transaction_search WHERE transaction_search MATCH ?2)",
        ),
        (
            SearchKind::Recurring,
            "recurring",
            "recurring_rules",
            "description",
            "instr(financial_normalize(description),?1)>0",
        ),
        (
            SearchKind::Asset,
            "asset",
            "assets",
            "name",
            "instr(financial_normalize(name),?1)>0",
        ),
        (
            SearchKind::Goal,
            "goal",
            "goals",
            "name",
            "instr(financial_normalize(name),?1)>0",
        ),
        (
            SearchKind::Group,
            "group",
            "shared_groups",
            "name",
            "instr(financial_normalize(name),?1)>0",
        ),
    ] {
        if !types.is_empty() && !types.contains(&kind) {
            continue;
        }
        queries.push(format!(
            "SELECT '{code}' AS kind,id,{label} AS label FROM {table} WHERE {condition}"
        ));
    }
    args.push(plain.into());
    if types.is_empty() || types.contains(&SearchKind::Transaction) {
        args.push(terms.into());
    }
    let mut query = Query {
        sql: format!(
            "SELECT kind,id,label,financial_normalize(label) AS sort_label,kind||':'||id AS sort_id FROM ({}) WHERE 1=1",
            queries.join(" UNION ALL ")
        ),
        args,
    };
    tx::sort_cursor(&mut query, &cursor, "sort_label", "sort_id", false, true)?;
    query.limit(cursor.limit + 1);
    let mut stmt = ctx.tx.prepare(&query.sql)?;
    let rows = stmt
        .query_map(params_from_iter(query.args), |r| {
            let kind = match r.get_ref(0)?.as_str()? {
                "account" => SearchKind::Account,
                "category" => SearchKind::Category,
                "payee" => SearchKind::Payee,
                "transaction" => SearchKind::Transaction,
                "recurring" => SearchKind::Recurring,
                "asset" => SearchKind::Asset,
                "goal" => SearchKind::Goal,
                "group" => SearchKind::Group,
                _ => return Err(crate::sql::Error::InvalidQuery),
            };
            Ok((
                SearchHit {
                    id: r.get(1)?,
                    kind,
                    label: r.get(2)?,
                },
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?
        .collect::<crate::sql::Result<_>>()?;
    let page = cursor.finish(rows, |(_, key, id)| (key.clone(), id.clone()))?;
    Ok(Page {
        rows: page.rows.into_iter().map(|(hit, _, _)| hit).collect(),
        next_cursor: page.next_cursor,
        revision: page.revision,
    })
}
