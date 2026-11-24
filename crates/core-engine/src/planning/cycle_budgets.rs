use std::collections::BTreeMap;

use crate::{
    domain::{Result, parse_date},
    ledger_engine::{Context, Deleted, Money, conflict, dto, invalid, minor, new_id},
    sql::{OptionalExtension, params},
};

use super::calendar::{self, FinancialCycle};

dto! {
    pub struct CycleBudgetQuery { pub start: Option<String> }
    pub struct CycleBudgetDraft { pub id: Option<String>, pub name: String, pub limit_minor: String, pub category_ids: Vec<String> }
    pub enum CycleBudgetChange { Save(CycleBudgetDraft), Delete { id: String }, CopyPrevious }
    pub struct CycleBudgetEdit { pub start: String, pub change: CycleBudgetChange }
    pub enum LimitStatus { Under, Near, At, Over, ZeroUnused, ZeroExceeded }
    pub enum BudgetPace { OnTrack, AheadOfPlan }
    pub struct CycleBudgetRow {
        pub id: String, pub name: String, pub category_ids: Vec<String>, pub limit: Money, pub spent: Money,
        pub remaining: Money, pub run_rate: Option<Money>, pub status: LimitStatus, pub pace: Option<BudgetPace>,
    }
    pub struct CycleBudgetView { pub cycle: FinancialCycle, pub rows: Vec<CycleBudgetRow>, pub excluded_foreign: String }
}

pub(super) fn idr(minor: i128) -> Money {
    Money {
        currency: "IDR".into(),
        minor: minor.to_string(),
    }
}

const ELIGIBLE: &str = "t.direction='debit' AND t.status='posted' AND t.ignored=0
  AND t.exclude_from_pnl=0 AND t.entry_kind='ordinary'
  AND NOT EXISTS(SELECT 1 FROM transfer_links l WHERE l.debit_id=t.id)
  AND NOT EXISTS(SELECT 1 FROM loan_payment_allocations p WHERE p.transaction_id=t.id)";

pub fn get(ctx: &Context<'_>, query: &CycleBudgetQuery) -> Result<Option<CycleBudgetView>> {
    let Some(calendar) = calendar::load(ctx)? else {
        return Ok(None);
    };
    let date = query
        .start
        .as_deref()
        .map(parse_date)
        .transpose()?
        .unwrap_or(calendar.today);
    let cycle = calendar.cycle(date)?;
    let as_of = calendar.today.min(cycle.end);
    let position = calendar::describe(cycle, as_of.max(cycle.start))?;
    let elapsed = if as_of < cycle.start {
        0
    } else {
        i128::from(position.elapsed)
    };
    let days = i128::from(position.days);

    let start = cycle.start.to_string();
    let end = as_of.to_string();
    let today = calendar.today.to_string();
    let mut spend = BTreeMap::<String, i128>::new();
    // Linked refunds offset the purchase's cycle. Integer remainder is assigned in split order.
    let mut stmt = ctx.tx.prepare(&format!(
        "SELECT t.id,t.category_id,t.amount_minor,s.category_id,s.amount_minor,
            coalesce((SELECT sum(r.amount_minor) FROM posting_refunds p JOIN transactions r ON r.id=p.refund_id
              WHERE p.original_id=t.id AND r.ignored=0 AND r.status='posted' AND r.bank_date<=?3),0)
         FROM transactions t LEFT JOIN transaction_splits s ON s.transaction_id=t.id
         WHERE t.bank_date BETWEEN ?1 AND ?2 AND t.currency_code='IDR' AND {ELIGIBLE}
         ORDER BY t.id,s.position"
    ))?;
    let mut rows = stmt.query(params![start, end, today])?;
    let mut previous = String::new();
    let mut running = 0i128;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        if previous != id {
            previous = id;
            running = 0;
        }
        let amount = i128::from(row.get::<_, i64>(2)?);
        let part = i128::from(row.get::<_, Option<i64>>(4)?.unwrap_or(amount as i64));
        let refund = i128::from(row.get::<_, i64>(5)?).min(amount);
        let net = part - ((running + part) * refund / amount - running * refund / amount);
        running += part;
        if let Some(category) = row.get::<_, Option<String>>(3)?.or(row.get(1)?) {
            *spend.entry(category).or_default() += net;
        }
    }
    let excluded: i64 = ctx.tx.query_row(
        &format!("SELECT count(*) FROM transactions t WHERE t.bank_date BETWEEN ?1 AND ?2 AND t.currency_code<>'IDR' AND {ELIGIBLE}"),
        params![start,end], |r| r.get(0)
    )?;
    let mut select = ctx.tx.prepare(
        "SELECT id,name,limit_minor FROM cycle_budgets WHERE cycle_start=?1 ORDER BY name,id",
    )?;
    let mut data = select.query([&start])?;
    let mut output = Vec::new();
    while let Some(row) = data.next()? {
        let id: String = row.get(0)?;
        let name = row.get(1)?;
        let limit = i128::from(row.get::<_, i64>(2)?);
        let mut select = ctx.tx.prepare(
            "SELECT b.category_id,c.ignored FROM cycle_budget_categories b JOIN categories c ON c.id=b.category_id
             WHERE b.budget_id=?1 ORDER BY b.category_id"
        )?;
        let categories = select
            .query_map([&id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?))
            })?
            .collect::<crate::sql::Result<Vec<_>>>()?;
        let actual = categories
            .iter()
            .filter(|(_, ignored)| !ignored)
            .map(|(category, _)| spend.get(category).copied().unwrap_or_default())
            .sum::<i128>();
        let status = match (limit, actual) {
            (0, 0) => LimitStatus::ZeroUnused,
            (0, _) => LimitStatus::ZeroExceeded,
            _ if actual > limit => LimitStatus::Over,
            _ if actual == limit => LimitStatus::At,
            _ if actual * 100 >= limit * 80 => LimitStatus::Near,
            _ => LimitStatus::Under,
        };
        output.push(CycleBudgetRow {
            id,
            name,
            category_ids: categories.into_iter().map(|(id, _)| id).collect(),
            limit: idr(limit),
            spent: idr(actual),
            remaining: idr(limit - actual),
            status,
            run_rate: (elapsed >= 3).then(|| idr(actual * days / elapsed)),
            pace: (limit > 0 && elapsed > 0).then(|| {
                if actual * days * 100 > limit * (elapsed * 100 + days * 5) {
                    BudgetPace::AheadOfPlan
                } else {
                    BudgetPace::OnTrack
                }
            }),
        });
    }
    Ok(Some(CycleBudgetView {
        cycle: position,
        rows: output,
        excluded_foreign: excluded.to_string(),
    }))
}

fn insert(
    ctx: &Context<'_>,
    start: &str,
    id: &str,
    name: &str,
    amount: i64,
    categories: &[String],
) -> Result<()> {
    let duplicate: bool = ctx.tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM cycle_budgets WHERE cycle_start=?1 AND name=?2 AND id<>?3)",
        params![start, name, id],
        |r| r.get(0),
    )?;
    if duplicate {
        return Err(conflict("cycle_budget_name_already_used"));
    }
    ctx.tx.execute(
        "INSERT INTO cycle_budgets VALUES(?1,?2,?3,?4)
         ON CONFLICT(id) DO UPDATE SET name=excluded.name,limit_minor=excluded.limit_minor",
        params![id, start, name, amount],
    )?;
    ctx.tx.execute(
        "DELETE FROM cycle_budget_categories WHERE budget_id=?1",
        [id],
    )?;
    for category in categories {
        let valid: bool = ctx.tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM categories WHERE id=?1 AND kind='expense')
             AND NOT EXISTS(SELECT 1 FROM cycle_budget_categories WHERE cycle_start=?2 AND category_id=?1)",
            params![category,start], |r| r.get(0),
        )?;
        if !valid {
            return Err(invalid("categoryIds", "cycle_budget_category_unavailable"));
        }
        ctx.tx.execute(
            "INSERT INTO cycle_budget_categories VALUES(?1,?2,?3)",
            params![id, start, category],
        )?;
    }
    Ok(())
}

pub fn edit(ctx: &Context<'_>, request: &CycleBudgetEdit) -> Result<Deleted> {
    let calendar = calendar::configured(ctx)?;
    let date = parse_date(&request.start)?;
    if calendar.cycle(date)?.start != date || calendar.cycle(calendar.today)?.start != date {
        return Err(conflict("cycle_budget_active_cycle_required"));
    }
    let start = &request.start;
    let id = match &request.change {
        CycleBudgetChange::Save(draft) => {
            let name = draft.name.trim();
            let amount = minor(&draft.limit_minor)?;
            if name.is_empty() || name.chars().count() > 100 {
                return Err(invalid("name", "invalid_name"));
            }
            if !(0..=9_000_000_000_000).contains(&amount)
                || draft.category_ids.is_empty()
                || draft.category_ids.len() > 200
            {
                return Err(invalid("limitMinor", "invalid_cycle_budget"));
            }
            let id = draft.id.clone().unwrap_or_else(new_id);
            if draft.id.is_some() {
                let stored: Option<String> = ctx
                    .tx
                    .query_row(
                        "SELECT cycle_start FROM cycle_budgets WHERE id=?1",
                        [&id],
                        |r| r.get(0),
                    )
                    .optional()?;
                if stored.as_ref() != Some(start) {
                    return Err(conflict("cycle_budget_unavailable"));
                }
            }
            insert(ctx, start, &id, name, amount, &draft.category_ids)?;
            id
        }
        CycleBudgetChange::Delete { id } => {
            if ctx.tx.execute(
                "DELETE FROM cycle_budgets WHERE id=?1 AND cycle_start=?2",
                params![id, start],
            )? != 1
            {
                return Err(conflict("cycle_budget_unavailable"));
            }
            id.clone()
        }
        CycleBudgetChange::CopyPrevious => {
            let previous = calendar.cycle(
                date.pred_opt()
                    .ok_or_else(|| invalid("start", "invalid_cycle_date"))?,
            )?;
            let mut stmt = ctx.tx.prepare(
                "SELECT id,name,limit_minor FROM cycle_budgets WHERE cycle_start=?1 ORDER BY id",
            )?;
            let data = stmt
                .query_map([previous.start.to_string()], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get(2)?))
                })?
                .collect::<crate::sql::Result<Vec<_>>>()?;
            for (id, name, amount) in data {
                let mut stmt = ctx.tx.prepare("SELECT category_id FROM cycle_budget_categories WHERE budget_id=?1 ORDER BY category_id")?;
                let categories = stmt
                    .query_map([id], |r| r.get(0))?
                    .collect::<crate::sql::Result<Vec<String>>>()?;
                insert(ctx, start, &new_id(), &name, amount, &categories)?;
            }
            start.clone()
        }
    };
    Ok(Deleted { id })
}
