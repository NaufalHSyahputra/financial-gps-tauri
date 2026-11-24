use std::collections::BTreeMap;

use chrono::{Days, NaiveDate};

use crate::{
    domain::{Result, parse_date},
    ledger_engine::{
        Context, Deleted, Money, accounts, conflict, dto, invalid, new_id, positive, transactions,
    },
    sql::{OptionalExtension, params},
};

use super::{calendar, cycle_budgets::idr, loans, recurring};

dto! {
    pub enum CashPlanKind { Obligation, StableIncome, UncertainIncome }
    pub enum CashPlanSource { Manual, Recurring, CardStatement, Loan }
    pub enum CashPlanState { Upcoming, Overdue, Received, Missed, Settled }
    pub struct CashPlanDraft {
        pub id: Option<String>, pub name: String, pub due_on: String, pub minor: String,
        pub kind: CashPlanKind, pub transaction_id: Option<String>,
    }
    pub struct CashPlanStage { pub starts_on: String, pub ends_on: Option<String>, pub minor: String }
    pub struct CashPlanStages { pub rule_id: String, pub stages: Vec<CashPlanStage> }
    pub enum CashPlanEdit { Save(CashPlanDraft), Delete { id: String }, SetStages(CashPlanStages) }
    pub struct CashPlanQuery { pub through: Option<String>, pub uncertain_ids: Vec<String> }
    pub struct CashPlanEntry {
        pub id: String, pub name: String, pub due_on: String, pub amount: Money, pub remaining: Money,
        pub kind: CashPlanKind, pub source: CashPlanSource, pub state: CashPlanState,
        pub transaction_id: Option<String>, pub rule_id: Option<String>,
    }
    pub struct CashPlanDay {
        pub date: String, pub obligations: Money, pub expected_income: Money, pub scenario_income: Money,
        pub cash_only_end: Money, pub expected_end: Money, pub scenario_end: Money,
        pub cash_only_gap: Money, pub expected_gap: Money, pub scenario_gap: Money,
        pub depends_on_expected_income: bool, pub entries: Vec<CashPlanEntry>,
    }
    pub struct CashPlanView {
        pub today: String, pub through: String, pub cash_start: Money, pub days: Vec<CashPlanDay>,
        pub entries: Vec<CashPlanEntry>, pub excluded_foreign_accounts: String, pub stages: Vec<CashPlanStages>,
    }
}

impl CashPlanKind {
    fn read(text: &str) -> crate::sql::Result<Self> {
        match text {
            "obligation" => Ok(Self::Obligation),
            "stableIncome" => Ok(Self::StableIncome),
            "uncertainIncome" => Ok(Self::UncertainIncome),
            _ => Err(crate::sql::Error::InvalidQuery),
        }
    }

    fn key(&self) -> &'static str {
        match self {
            Self::Obligation => "obligation",
            Self::StableIncome => "stableIncome",
            Self::UncertainIncome => "uncertainIncome",
        }
    }
}

fn linked(ctx: &Context<'_>, id: &str, kind: &CashPlanKind, today: NaiveDate) -> Result<i128> {
    let record = transactions::get(ctx, id)?;
    let draft = &record.draft;
    let direction = match kind {
        CashPlanKind::Obligation => "debit",
        _ => "credit",
    };
    let liquid: bool = ctx.tx.query_row(
        "SELECT kind IN ('checking','savings','cash') FROM accounts WHERE id=?1",
        [&draft.account_id],
        |r| r.get(0),
    )?;
    if !liquid
        || draft.amount.currency != "IDR"
        || draft.ignored
        || record.row.transfer_id.is_some()
        || record.row.entry_kind != transactions::EntryKind::Ordinary
        || draft.exclude_from_pnl
        || draft.status.as_str() != "posted"
        || draft.direction.as_str() != direction
    {
        return Err(conflict("cash_plan_posting_ineligible"));
    }
    let category_ignored = if let Some(id) = &draft.category_id {
        ctx.tx
            .query_row("SELECT ignored FROM categories WHERE id=?1", [id], |r| {
                r.get(0)
            })?
    } else {
        false
    };
    if category_ignored || parse_date(&draft.bank_date)? > today {
        return Ok(0);
    }
    Ok(i128::from(positive(&draft.amount.minor)?))
}

fn set_stages(ctx: &Context<'_>, input: &CashPlanStages) -> Result<()> {
    let rule = recurring::get(ctx, &input.rule_id)?;
    if !input.stages.is_empty()
        && (rule.draft.amount.currency != "IDR"
            || rule.draft.direction.as_str() != "debit"
            || !rule.draft.splits.is_empty()
            || input.stages.len() > 100)
    {
        return Err(invalid("ruleId", "cash_plan_stage_rule_ineligible"));
    }
    let mut stages = input
        .stages
        .iter()
        .map(|stage| {
            let start = parse_date(&stage.starts_on)?;
            let end = stage.ends_on.as_deref().map(parse_date).transpose()?;
            if end.is_some_and(|end| end < start) {
                return Err(invalid("endsOn", "cash_plan_stage_invalid_range"));
            }
            Ok((start, end, positive(&stage.minor)?))
        })
        .collect::<Result<Vec<_>>>()?;
    stages.sort_by_key(|stage| stage.0);
    if stages
        .windows(2)
        .any(|pair| pair[0].1.is_none_or(|end| end >= pair[1].0))
    {
        return Err(invalid("startsOn", "cash_plan_stage_overlaps"));
    }
    ctx.tx.execute(
        "DELETE FROM cash_plan_stages WHERE rule_id=?1",
        [&input.rule_id],
    )?;
    for (start, end, amount) in stages {
        ctx.tx.execute(
            "INSERT INTO cash_plan_stages VALUES (?1,?2,?3,?4)",
            params![
                input.rule_id,
                start.to_string(),
                end.map(|end| end.to_string()),
                amount
            ],
        )?;
    }
    Ok(())
}

pub(super) fn staged_amount(
    ctx: &Context<'_>,
    rule: &recurring::RecurringDetail,
    date: &str,
) -> Result<Option<String>> {
    if rule.draft.amount.currency != "IDR"
        || rule.draft.direction.as_str() != "debit"
        || !rule.draft.splits.is_empty()
    {
        return Ok(None);
    }
    Ok(ctx.tx.query_row(
        "SELECT CAST(amount_minor AS TEXT) FROM cash_plan_stages WHERE rule_id=?1 AND starts_on<=?2
         AND (ends_on IS NULL OR ends_on>=?2) ORDER BY starts_on DESC LIMIT 1",
        params![rule.id,date], |r| r.get(0),
    ).optional()?)
}

fn stages(ctx: &Context<'_>) -> Result<BTreeMap<String, Vec<CashPlanStage>>> {
    let mut statement = ctx.tx.prepare("SELECT rule_id,starts_on,ends_on,CAST(amount_minor AS TEXT) FROM cash_plan_stages ORDER BY rule_id,starts_on")?;
    let mut rows = statement.query([])?;
    let mut stages = BTreeMap::<String, Vec<CashPlanStage>>::new();
    while let Some(row) = rows.next()? {
        stages.entry(row.get(0)?).or_default().push(CashPlanStage {
            starts_on: row.get(1)?,
            ends_on: row.get(2)?,
            minor: row.get(3)?,
        });
    }
    Ok(stages)
}

pub fn edit(ctx: &Context<'_>, input: &CashPlanEdit) -> Result<Deleted> {
    let calendar = calendar::configured(ctx)?;
    let id = match input {
        CashPlanEdit::SetStages(input) => {
            set_stages(ctx, input)?;
            input.rule_id.clone()
        }
        CashPlanEdit::Delete { id } => {
            if ctx
                .tx
                .execute("DELETE FROM cash_plan_entries WHERE id=?1", [id])?
                != 1
            {
                return Err(conflict("cash_plan_entry_unavailable"));
            }
            id.clone()
        }
        CashPlanEdit::Save(draft) => {
            parse_date(&draft.due_on)?;
            let amount = positive(&draft.minor)?;
            let name = draft.name.trim();
            if name.is_empty() || name.chars().count() > 100 {
                return Err(invalid("name", "invalid_name"));
            }
            let id = draft.id.clone().unwrap_or_else(new_id);
            if draft.id.is_some()
                && !ctx.tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM cash_plan_entries WHERE id=?1)",
                    [&id],
                    |r| r.get::<_, bool>(0),
                )?
            {
                return Err(conflict("cash_plan_entry_unavailable"));
            }
            if let Some(transaction) = &draft.transaction_id {
                if linked(ctx, transaction, &draft.kind, calendar.today)? == 0 {
                    return Err(conflict("cash_plan_posting_ineligible"));
                }
                let taken: bool = ctx.tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM cash_plan_entries WHERE transaction_id=?1 AND id<>?2)
                     OR EXISTS(SELECT 1 FROM recurring_occurrences WHERE transaction_id=?1)
                     OR EXISTS(SELECT 1 FROM loan_payment_allocations WHERE transaction_id=?1)",
                    params![transaction,id], |r| r.get(0)
                )?;
                if taken {
                    return Err(conflict("cash_plan_posting_already_linked"));
                }
            }
            ctx.tx.execute(
                "INSERT INTO cash_plan_entries VALUES(?1,?2,?3,?4,?5,?6)
                 ON CONFLICT(id) DO UPDATE SET name=excluded.name,due_on=excluded.due_on,
                 amount_minor=excluded.amount_minor,kind=excluded.kind,transaction_id=excluded.transaction_id",
                params![id,name,draft.due_on,amount,draft.kind.key(),draft.transaction_id],
            )?;
            id
        }
    };
    Ok(Deleted { id })
}

fn manual(ctx: &Context<'_>, today: NaiveDate) -> Result<Vec<CashPlanEntry>> {
    let mut stmt = ctx.tx.prepare("SELECT id,name,due_on,amount_minor,kind,transaction_id FROM cash_plan_entries ORDER BY due_on,id")?;
    let mut cursor = stmt.query([])?;
    let mut entries = Vec::new();
    while let Some(row) = cursor.next()? {
        let due_on: String = row.get(2)?;
        let date = parse_date(&due_on)?;
        let amount = i128::from(row.get::<_, i64>(3)?);
        let kind = CashPlanKind::read(&row.get::<_, String>(4)?)?;
        let transaction_id: Option<String> = row.get(5)?;
        let applied = transaction_id
            .as_deref()
            .map(|id| linked(ctx, id, &kind, today))
            .transpose()?
            .unwrap_or(0);
        let (state, remaining) = match kind {
            CashPlanKind::Obligation if applied >= amount => (CashPlanState::Settled, 0),
            CashPlanKind::Obligation if date < today => (CashPlanState::Overdue, amount - applied),
            CashPlanKind::Obligation => (CashPlanState::Upcoming, amount - applied),
            _ if applied > 0 => (CashPlanState::Received, 0),
            _ if date < today => (CashPlanState::Missed, 0),
            _ => (CashPlanState::Upcoming, amount),
        };
        entries.push(CashPlanEntry {
            id: row.get(0)?,
            name: row.get(1)?,
            due_on,
            amount: idr(amount),
            remaining: idr(remaining),
            kind,
            source: CashPlanSource::Manual,
            state,
            transaction_id,
            rule_id: None,
        });
    }
    Ok(entries)
}

fn recurring_expenses(
    ctx: &Context<'_>,
    today: NaiveDate,
    through: NaiveDate,
    stages: &BTreeMap<String, Vec<CashPlanStage>>,
) -> Result<Vec<CashPlanEntry>> {
    let mut rules = BTreeMap::new();
    let mut entries = Vec::new();
    // Include persisted overdue generated obligations and project new ones from today.
    let mut overdue = ctx.tx.prepare(
        "SELECT rule_id,nominal_date,scheduled_date,transaction_id FROM recurring_occurrences
         WHERE scheduled_date<?1 AND state='generated' ORDER BY scheduled_date,rule_id,nominal_date"
    )?;
    let mut occurrences = overdue
        .query_map([today.to_string()], |r| {
            Ok(recurring::Occurrence {
                rule_id: r.get(0)?,
                nominal_date: r.get(1)?,
                scheduled_date: r.get(2)?,
                transaction_id: r.get(3)?,
                state: recurring::OccurrenceState::Generated,
            })
        })?
        .collect::<crate::sql::Result<Vec<_>>>()?;
    occurrences.extend(recurring::for_range(ctx, None, today, through)?);
    for item in occurrences {
        if !matches!(
            item.state,
            recurring::OccurrenceState::Virtual | recurring::OccurrenceState::Generated
        ) {
            continue;
        }
        let rule = match rules.entry(item.rule_id.clone()) {
            std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(recurring::get(ctx, &item.rule_id)?)
            }
        };
        let mut draft = rule.draft.clone();
        if let Some(id) = &item.transaction_id {
            let actual = transactions::get(ctx, id)?.draft;
            if actual.ignored || actual.status.as_str() != "projected" {
                continue;
            }
            draft.amount = actual.amount;
            draft.direction = actual.direction;
            draft.description = actual.description;
        }
        if draft.amount.currency != "IDR" || draft.direction.as_str() != "debit" {
            continue;
        }
        let liquid: Option<bool> = ctx.tx.query_row(
            "SELECT kind IN ('checking','savings','cash') FROM accounts WHERE id=?1 AND archived_on IS NULL",
            [&draft.account_id], |r| r.get(0)
        ).optional()?;
        if liquid != Some(true) {
            continue;
        }
        if let Some(category) = &draft.category_id
            && ctx.tx.query_row(
                "SELECT ignored FROM categories WHERE id=?1",
                [category],
                |r| r.get::<_, bool>(0),
            )?
        {
            continue;
        }
        if rule.draft.splits.is_empty()
            && let Some(stage) = stages.get(&item.rule_id).and_then(|stages| {
                stages.iter().find(|stage| {
                    stage.starts_on <= item.scheduled_date
                        && stage
                            .ends_on
                            .as_ref()
                            .is_none_or(|end| end >= &item.scheduled_date)
                })
            })
        {
            draft.amount.minor.clone_from(&stage.minor);
        }
        let state = if parse_date(&item.scheduled_date)? < today {
            CashPlanState::Overdue
        } else {
            CashPlanState::Upcoming
        };
        entries.push(CashPlanEntry {
            id: format!("{}:{}", item.rule_id, item.nominal_date),
            name: draft.description,
            due_on: item.scheduled_date,
            amount: draft.amount.clone(),
            remaining: draft.amount,
            kind: CashPlanKind::Obligation,
            state,
            source: CashPlanSource::Recurring,
            transaction_id: item.transaction_id,
            rule_id: Some(item.rule_id),
        });
    }
    Ok(entries)
}

fn liability_entries(
    ctx: &Context<'_>,
    today: NaiveDate,
    through: NaiveDate,
) -> Result<Vec<CashPlanEntry>> {
    let day = today.to_string();
    let mut entries = Vec::new();
    let mut statements = ctx.tx.prepare(
        "SELECT s.id,s.due_on,a.name FROM card_statements s JOIN accounts a ON a.id=s.account_id
         WHERE a.kind='credit_card' AND a.currency_code='IDR' AND s.due_on<=?1
         AND EXISTS(SELECT 1 FROM transactions t WHERE t.statement_id=s.id AND t.status='posted' AND t.bank_date<=?2)"
    )?;
    let mut statements = statements.query(params![through.to_string(), day])?;
    let mut statement_rows = ctx.tx.prepare(
        "SELECT t.direction,t.amount_minor
         FROM transactions t LEFT JOIN categories c ON c.id=t.category_id
         WHERE t.statement_id=?1 AND t.status='posted' AND t.ignored=0 AND t.bank_date<=?2
         AND coalesce(c.ignored,0)=0
         AND ((t.direction='debit' AND t.entry_kind='ordinary' AND
               NOT EXISTS(SELECT 1 FROM transfer_links l WHERE l.debit_id=t.id))
            OR (t.direction='credit' AND (EXISTS(SELECT 1 FROM transfer_links l WHERE l.credit_id=t.id)
               OR EXISTS(SELECT 1 FROM posting_refunds p WHERE p.refund_id=t.id))))"
    )?;
    while let Some(row) = statements.next()? {
        let id: String = row.get(0)?;
        let mut amount = 0;
        let mut rows = statement_rows.query(params![id, day])?;
        while let Some(t) = rows.next()? {
            let sign = if t.get::<_, String>(0)? == "debit" {
                1
            } else {
                -1
            };
            amount += sign * i128::from(t.get::<_, i64>(1)?);
        }
        let name: String = row.get(2)?;
        push_liability(
            &mut entries,
            format!("card:{id}"),
            format!("{name} statement"),
            row.get(1)?,
            amount,
            &day,
            CashPlanSource::CardStatement,
        );
    }
    let mut loan_rows = ctx.tx.prepare(
        "SELECT a.id,a.name FROM accounts a JOIN loans l ON l.account_id=a.id WHERE l.currency_code='IDR'"
    )?;
    let mut loan_rows = loan_rows.query([])?;
    let mut payments = ctx.tx.prepare(
        "SELECT l.principal_minor,l.interest_minor FROM loan_payment_allocations l JOIN transactions t ON t.id=l.transaction_id
         WHERE l.loan_account_id=?1 AND t.status='posted' AND t.ignored=0 AND t.bank_date<=?2"
    )?;
    while let Some(row) = loan_rows.next()? {
        let id: String = row.get(0)?;
        let name: String = row.get(1)?;
        let mut totals = (0i128, 0i128);
        let mut rows = payments.query(params![id, day])?;
        while let Some(p) = rows.next()? {
            totals.0 += i128::from(p.get::<_, i64>(0)?);
            totals.1 += i128::from(p.get::<_, i64>(1)?);
        }
        let through = through.to_string();
        for period in loans::calculate(&loans::terms(ctx, &id)?, today)?.rows {
            if period.due_date > through {
                break;
            }
            let parse = |m: &Money| {
                m.minor
                    .parse::<i128>()
                    .map_err(|_| invalid("amount", "cash_plan_amount_out_of_range"))
            };
            let principal = parse(&period.principal)?;
            let interest = parse(&period.interest)?;
            let paid = (principal.min(totals.0), interest.min(totals.1));
            totals.0 -= paid.0;
            totals.1 -= paid.1;
            push_liability(
                &mut entries,
                format!("loan:{id}:{}", period.number),
                format!("{name} payment {}", period.number),
                period.due_date,
                principal + interest - paid.0 - paid.1,
                &day,
                CashPlanSource::Loan,
            );
        }
    }
    Ok(entries)
}

fn push_liability(
    entries: &mut Vec<CashPlanEntry>,
    id: String,
    name: String,
    due_on: String,
    outstanding: i128,
    today: &str,
    source: CashPlanSource,
) {
    if outstanding > 0 {
        let state = if due_on.as_str() < today {
            CashPlanState::Overdue
        } else {
            CashPlanState::Upcoming
        };
        entries.push(CashPlanEntry {
            id,
            name,
            due_on,
            amount: idr(outstanding),
            remaining: idr(outstanding),
            kind: CashPlanKind::Obligation,
            source,
            state,
            transaction_id: None,
            rule_id: None,
        });
    }
}

pub fn get(ctx: &Context<'_>, query: &CashPlanQuery) -> Result<Option<CashPlanView>> {
    let Some(calendar) = calendar::load(ctx)? else {
        return Ok(None);
    };
    let today = calendar.today;
    let through = match &query.through {
        Some(date) => parse_date(date)?,
        None => calendar.cycle(calendar.cycle(today)?.next()?)?.end,
    };
    if !(0..=366).contains(&(through - today).num_days()) || query.uncertain_ids.len() > 500 {
        return Err(invalid("through", "cash_plan_range_too_large"));
    }
    let mut stmt = ctx.tx.prepare(
        "SELECT id,currency_code FROM accounts WHERE kind IN ('checking','savings','cash')",
    )?;
    let mut rows = stmt.query([])?;
    let mut cash = 0i128;
    let mut excluded = 0u64;
    while let Some(row) = rows.next()? {
        let currency: String = row.get(1)?;
        if currency != "IDR" {
            excluded += 1;
            continue;
        }
        cash += accounts::balance(ctx, &row.get::<_, String>(0)?, today, None)?.max(0);
    }
    let mut entries = manual(ctx, today)?;
    let stages = stages(ctx)?;
    entries.extend(recurring_expenses(ctx, today, through, &stages)?);
    entries.extend(liability_entries(ctx, today, through)?);
    entries.sort_by(|a, b| (&a.due_on, &a.id).cmp(&(&b.due_on, &b.id)));

    let mut buckets = BTreeMap::<NaiveDate, Vec<CashPlanEntry>>::new();
    for entry in &entries {
        if entry.remaining.minor != "0" {
            let date = parse_date(&entry.due_on)?.max(today);
            if date <= through {
                buckets.entry(date).or_default().push(entry.clone());
            }
        }
    }
    let (mut actual, mut stable, mut uncertain) = (cash, 0i128, 0i128);
    let mut days = Vec::new();
    for offset in 0..=(through - today).num_days() {
        let date = today
            .checked_add_days(Days::new(offset as u64))
            .ok_or_else(|| invalid("through", "invalid_cycle_date"))?;
        let entries = buckets.remove(&date).unwrap_or_default();
        let (mut due, mut expected, mut scenario) = (0i128, 0i128, 0i128);
        for row in &entries {
            let amount = row
                .remaining
                .minor
                .parse::<i128>()
                .map_err(|_| invalid("amount", "cash_plan_amount_out_of_range"))?;
            match row.kind {
                CashPlanKind::Obligation => due += amount,
                CashPlanKind::StableIncome => expected += amount,
                CashPlanKind::UncertainIncome if query.uncertain_ids.contains(&row.id) => {
                    scenario += amount
                }
                CashPlanKind::UncertainIncome => {}
            }
        }
        actual -= due;
        let gap = |cash: i128| if due == 0 { 0 } else { (-cash).max(0) };
        let cash_gap = gap(actual);
        let expected_gap = gap(actual + stable);
        let scenario_gap = gap(actual + stable + uncertain);
        stable += expected;
        uncertain += scenario;
        days.push(CashPlanDay {
            date: date.to_string(),
            obligations: idr(due),
            expected_income: idr(expected),
            scenario_income: idr(scenario),
            cash_only_end: idr(actual),
            expected_end: idr(actual + stable),
            scenario_end: idr(actual + stable + uncertain),
            cash_only_gap: idr(cash_gap),
            expected_gap: idr(expected_gap),
            scenario_gap: idr(scenario_gap),
            depends_on_expected_income: expected_gap < cash_gap,
            entries,
        });
    }
    Ok(Some(CashPlanView {
        today: today.to_string(),
        through: through.to_string(),
        cash_start: idr(cash),
        days,
        entries,
        excluded_foreign_accounts: excluded.to_string(),
        stages: stages
            .into_iter()
            .map(|(rule_id, stages)| CashPlanStages { rule_id, stages })
            .collect(),
    }))
}
