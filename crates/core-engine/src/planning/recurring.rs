use crate::sql::{OptionalExtension, params};
use chrono::{Datelike, Days, NaiveDate, Weekday};

use crate::{
    domain::{Result, parse_date},
    ledger_engine::{
        Context, DateRange, Direction, Money, Page, PageReq, ReadPage, ReceiptValue, TxStatus,
        VersionedId, accounts, check_version, conflict, dto, invalid, missing, money, new_id,
        positive,
        transactions::{self as tx, SplitDraft, TxDraft},
    },
};

dto! {
    pub enum Frequency {Weekly,Biweekly,Monthly,Quarterly,Semiannual,Yearly}
    pub enum WeekendPolicy {None,PreviousFriday,NextMonday}
    pub struct RecurringDraft {
        pub account_id:String,pub amount:Money,pub direction:Direction,pub description:String,
        #[serde(default)] pub notes:String,pub payee_id:Option<String>,pub category_id:Option<String>,
        #[serde(default)] pub splits:Vec<SplitDraft>,pub frequency:Frequency,pub anchor_day:Option<u8>,
        pub starts_on:String,pub weekend_policy:WeekendPolicy,pub ends_on:Option<String>,pub count_limit:Option<u32>,
        #[serde(default)] pub skip_first:bool,#[serde(default)] pub auto_generate:bool,pub active:bool,
    }
    pub struct RecurringDetail {pub id:String,pub version:String,pub draft:RecurringDraft,pub generation_floor:Option<String>}
    pub enum OccurrenceState {Virtual,Generated,Matched,Skipped}
    pub struct Occurrence {pub rule_id:String,pub nominal_date:String,pub scheduled_date:String,pub state:OccurrenceState,pub transaction_id:Option<String>}
    pub enum OccurrenceAction {Generate,Skip,Match,Unmatch,Post}
}
macro_rules! text_values {
    ($kind:ident; $($variant:ident => $text:literal),+) => {
        impl $kind {
            fn as_str(&self)->&'static str {match self {$(Self::$variant=>$text),+}}
            fn read(s:&str)->crate::sql::Result<Self> {match s {$($text=>Ok(Self::$variant)),+,_=>Err(crate::sql::Error::InvalidQuery)}}
        }
    }
}
text_values!(Frequency; Weekly=>"weekly",Biweekly=>"biweekly",Monthly=>"monthly",Quarterly=>"quarterly",Semiannual=>"semiannual",Yearly=>"yearly");
text_values!(WeekendPolicy; None=>"none",PreviousFriday=>"previous_friday",NextMonday=>"next_monday");
text_values!(OccurrenceState; Virtual=>"virtual",Generated=>"generated",Matched=>"matched",Skipped=>"skipped");

impl Frequency {
    fn interval(&self) -> (i64, bool) {
        match self {
            Self::Weekly => (7, false),
            Self::Biweekly => (14, false),
            Self::Monthly => (1, true),
            Self::Quarterly => (3, true),
            Self::Semiannual => (6, true),
            Self::Yearly => (12, true),
        }
    }
}
fn checked(date: Option<NaiveDate>) -> Result<NaiveDate> {
    date.filter(|date| (1..=9999).contains(&date.year()))
        .ok_or_else(|| invalid("date", "date_out_of_range"))
}
fn move_weekend(value: NaiveDate, policy: &WeekendPolicy) -> Result<NaiveDate> {
    match (value.weekday(), policy) {
        (Weekday::Sat, WeekendPolicy::PreviousFriday) => {
            checked(value.checked_sub_days(Days::new(1)))
        }
        (Weekday::Sun, WeekendPolicy::PreviousFriday) => {
            checked(value.checked_sub_days(Days::new(2)))
        }
        (Weekday::Sat, WeekendPolicy::NextMonday) => checked(value.checked_add_days(Days::new(2))),
        (Weekday::Sun, WeekendPolicy::NextMonday) => checked(value.checked_add_days(Days::new(1))),
        _ => Ok(value),
    }
}
fn month_gap(a: NaiveDate, b: NaiveDate) -> i64 {
    i64::from((b.year() - a.year()) * 12 + b.month() as i32 - a.month() as i32)
}
pub fn dates(
    draft: &RecurringDraft,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<Vec<(NaiveDate, NaiveDate)>> {
    let start = parse_date(&draft.starts_on)?;
    let ends = draft.ends_on.as_deref().map(parse_date).transpose()?;
    let (interval, months) = draft.frequency.interval();
    if from > to
        || ends.is_some_and(|end| end < start)
        || (ends.is_some() && draft.count_limit.is_some())
        || draft.count_limit == Some(0)
        || draft
            .anchor_day
            .is_some_and(|d| !months || !(1..=31).contains(&d))
    {
        return Err(invalid("schedule", "invalid_recurring_schedule"));
    }
    let anchor = draft.anchor_day.map_or(start.day(), u32::from);
    let first = if months && accounts::month_shift(start, 0, anchor)? < start {
        accounts::month_shift(start, 1, anchor)?
    } else {
        start
    };
    let lookback = from.checked_sub_days(Days::new(2)).unwrap_or(from);
    let offset = if months {
        month_gap(first, lookback)
    } else {
        (lookback - first).num_days()
    };
    let skipped = i64::from(draft.skip_first);
    let mut index = (offset.div_euclid(interval) - 1).max(skipped);
    let mut output = Vec::new();
    let last = to.checked_add_days(Days::new(2)).unwrap_or(to);
    loop {
        if draft
            .count_limit
            .is_some_and(|count| index - skipped >= i64::from(count))
        {
            break;
        }
        let nominal = if months {
            accounts::month_shift(first, (index * interval) as i32, anchor)
        } else {
            checked(first.checked_add_days(Days::new((index * interval) as u64)))
        };
        let nominal = match nominal {
            Ok(value) => value,
            Err(_) if !output.is_empty() || first > to => break,
            Err(error) => return Err(error),
        };
        if nominal > last || ends.is_some_and(|end| nominal > end) {
            break;
        }
        let shifted = move_weekend(nominal, &draft.weekend_policy)?;
        if (from..=to).contains(&shifted) {
            output.push((nominal, shifted));
            if output.len() > 10_000 {
                return Err(invalid("range", "recurring_range_too_large"));
            }
        }
        index += 1;
    }
    Ok(output)
}

pub fn get(ctx: &Context<'_>, id: &str) -> Result<RecurringDetail> {
    let record=ctx.tx.query_row("SELECT account_id,currency_code,CAST(amount_minor AS TEXT),direction,description,notes,payee_id,category_id,
        frequency,anchor_day,starts_on,weekend_policy,ends_on,count_limit,skip_first,auto_generate,active,
        generated_through_nominal,CAST(version AS TEXT) FROM recurring_rules WHERE id=?1",[id],|row|{
        let direction=match row.get_ref(3)?.as_str()? {"debit"=>Direction::Debit,"credit"=>Direction::Credit,_=>return Err(crate::sql::Error::InvalidQuery)};
        Ok(RecurringDetail{id:id.to_owned(),version:row.get(18)?,generation_floor:row.get(17)?,draft:RecurringDraft{
            account_id:row.get(0)?,amount:Money{currency:row.get(1)?,minor:row.get(2)?},direction,description:row.get(4)?,
            notes:row.get(5)?,payee_id:row.get(6)?,category_id:row.get(7)?,frequency:Frequency::read(row.get_ref(8)?.as_str()?)?,
            anchor_day:row.get(9)?,starts_on:row.get(10)?,weekend_policy:WeekendPolicy::read(row.get_ref(11)?.as_str()?)?,
            ends_on:row.get(12)?,count_limit:row.get(13)?,skip_first:row.get(14)?,auto_generate:row.get(15)?,active:row.get(16)?,splits:Vec::new(),
        }})
    }).optional()?.ok_or_else(||missing("ruleId"))?;
    let mut record = record;
    record.draft.splits=ctx.tx.prepare_cached("SELECT category_id,CAST(amount_minor AS TEXT) FROM recurring_rule_splits WHERE rule_id=?1 ORDER BY position")?
        .query_map([id],|row|Ok(SplitDraft{category_id:row.get(0)?,minor:row.get(1)?}))?.collect::<crate::sql::Result<_>>()?;
    Ok(record)
}

pub fn list(
    ctx: &Context<'_>,
    range: Option<&DateRange>,
    page: &PageReq,
) -> Result<Page<RecurringDetail>> {
    let cursor = ReadPage::new(ctx, page, &("recurring_list", range))?;
    let (from, to) = range
        .map(crate::ledger_engine::date_range)
        .transpose()?
        .unwrap_or_default();
    let after = cursor.after.as_ref().map(|(_, id)| id.as_str());
    let mut stmt=ctx.tx.prepare_cached("SELECT id FROM recurring_rules WHERE (?1 IS NULL OR id>?1)
        AND (?2 IS NULL OR ends_on IS NULL OR ends_on>=?2) AND (?3 IS NULL OR starts_on<=?3) ORDER BY id LIMIT ?4")?;
    let ids = stmt
        .query_map(params![after, from, to, (cursor.limit + 1) as i64], |row| {
            row.get::<_, String>(0)
        })?
        .collect::<crate::sql::Result<Vec<_>>>()?;
    cursor.finish(
        ids.iter().map(|id| get(ctx, id)).collect::<Result<_>>()?,
        |row| (row.id.clone(), row.id.clone()),
    )
}

pub fn save(
    ctx: &Context<'_>,
    target: Option<&VersionedId>,
    draft: &RecurringDraft,
) -> Result<RecurringDetail> {
    let begin = parse_date(&draft.starts_on)?;
    dates(draft, begin, begin)?;
    let (account, archived) = accounts::raw(ctx, &draft.account_id)?;
    if account.currency_code != draft.amount.currency || archived.is_some() {
        return Err(invalid("accountId", "recurring_account_unavailable"));
    }
    let amount = positive(&draft.amount.minor)?;
    crate::ledger_engine::name(&draft.description)?;
    if draft.notes.chars().count() > 1000 {
        return Err(invalid("notes", "notes_too_long"));
    }
    if let Some(category) = &draft.category_id {
        tx::assignable_category(ctx, category)?;
    }
    if let Some(payee) = &draft.payee_id {
        let valid: bool = ctx.tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM payees WHERE id=?1 AND archived=0)",
            [payee],
            |row| row.get(0),
        )?;
        if !valid {
            return Err(invalid("payeeId", "unassignable_payee"));
        }
    }
    let mut sum = 0i128;
    if !draft.splits.is_empty()
        && (!(2..=1000).contains(&draft.splits.len()) || draft.category_id.is_some())
    {
        return Err(invalid("splits", "invalid_split_shape"));
    }
    for split in &draft.splits {
        tx::assignable_category(ctx, &split.category_id)?;
        sum += i128::from(positive(&split.minor)?);
    }
    if !draft.splits.is_empty() && sum != i128::from(amount) {
        return Err(invalid("splits", "split_sum_mismatch"));
    }
    let (id, floor) = if let Some(target) = target {
        let current = get(ctx, &target.id)?;
        check_version(&current.version, target)?;
        let today = ctx.today.to_string();
        let floor = current
            .generation_floor
            .map_or_else(|| today.clone(), |floor| floor.max(today.clone()));
        (target.id.clone(), Some(floor))
    } else {
        (new_id(), None)
    };
    ctx.tx.execute("INSERT INTO recurring_rules(id,account_id,currency_code,payee_id,category_id,direction,amount_minor,description,notes,
        frequency,anchor_day,starts_on,weekend_policy,ends_on,count_limit,skip_first,auto_generate,active,generated_through_nominal)
        VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)
        ON CONFLICT(id) DO UPDATE SET account_id=excluded.account_id,currency_code=excluded.currency_code,payee_id=excluded.payee_id,
        category_id=excluded.category_id,direction=excluded.direction,amount_minor=excluded.amount_minor,description=excluded.description,
        notes=excluded.notes,frequency=excluded.frequency,anchor_day=excluded.anchor_day,starts_on=excluded.starts_on,weekend_policy=excluded.weekend_policy,
        ends_on=excluded.ends_on,count_limit=excluded.count_limit,skip_first=excluded.skip_first,auto_generate=excluded.auto_generate,
        active=excluded.active,generated_through_nominal=excluded.generated_through_nominal,version=recurring_rules.version+1",
        params![id,draft.account_id,draft.amount.currency,draft.payee_id,draft.category_id,draft.direction.as_str(),amount,draft.description,
            draft.notes,draft.frequency.as_str(),draft.anchor_day,draft.starts_on,draft.weekend_policy.as_str(),draft.ends_on,draft.count_limit,
            draft.skip_first,draft.auto_generate,draft.active,floor])?;
    ctx.tx
        .execute("DELETE FROM recurring_rule_splits WHERE rule_id=?1", [&id])?;
    for (index, split) in draft.splits.iter().enumerate() {
        ctx.tx.execute(
            "INSERT INTO recurring_rule_splits VALUES(?1,?2,?3,?4)",
            params![
                id,
                (index + 1) as i64,
                split.category_id,
                positive(&split.minor)?
            ],
        )?;
    }
    get(ctx, &id)
}

pub fn archive(ctx: &Context<'_>, id: &str) -> Result<RecurringDetail> {
    if ctx.tx.execute(
        "UPDATE recurring_rules SET active=0,version=version+1 WHERE id=?1",
        [id],
    )? == 0
    {
        return Err(missing("ruleId"));
    }
    get(ctx, id)
}

fn existing(ctx: &Context<'_>, id: &str, nominal: &str) -> Result<Option<Occurrence>> {
    Ok(ctx.tx.query_row("SELECT scheduled_date,transaction_id,state FROM recurring_occurrences WHERE rule_id=?1 AND nominal_date=?2",
        params![id,nominal],|row|Ok(Occurrence{rule_id:id.to_owned(),nominal_date:nominal.to_owned(),scheduled_date:row.get(0)?,
            transaction_id:row.get(1)?,state:OccurrenceState::read(row.get_ref(2)?.as_str()?)?})).optional()?)
}
pub fn for_range(
    ctx: &Context<'_>,
    rule_id: Option<&str>,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<Vec<Occurrence>> {
    if from > to || (to - from).num_days() > 3660 {
        return Err(invalid("range", "recurring_range_too_large"));
    }
    let mut ids = ctx
        .tx
        .prepare_cached("SELECT id FROM recurring_rules WHERE (?1 IS NULL OR id=?1) ORDER BY id")?;
    let ids = ids
        .query_map([rule_id], |row| row.get::<_, String>(0))?
        .collect::<crate::sql::Result<Vec<_>>>()?;
    if rule_id.is_some() && ids.is_empty() {
        return Err(missing("ruleId"));
    }
    let mut values = Vec::new();
    for id in ids {
        let rule = get(ctx, &id)?;
        let mut stmt=ctx.tx.prepare_cached("SELECT nominal_date,scheduled_date,transaction_id,state FROM recurring_occurrences WHERE rule_id=?1")?;
        let known = stmt
            .query_map([&id], |row| {
                Ok(Occurrence {
                    rule_id: id.clone(),
                    nominal_date: row.get(0)?,
                    scheduled_date: row.get(1)?,
                    transaction_id: row.get(2)?,
                    state: OccurrenceState::read(row.get_ref(3)?.as_str()?)?,
                })
            })?
            .collect::<crate::sql::Result<Vec<_>>>()?;
        let occupied = known
            .iter()
            .map(|row| row.nominal_date.clone())
            .collect::<std::collections::HashSet<_>>();
        values.extend(known.into_iter().filter(|row| {
            row.scheduled_date >= from.to_string() && row.scheduled_date <= to.to_string()
        }));
        if rule.draft.active {
            for (nominal, scheduled) in dates(&rule.draft, from, to)? {
                let nominal = nominal.to_string();
                if occupied.contains(&nominal)
                    || rule
                        .generation_floor
                        .as_ref()
                        .is_some_and(|floor| &nominal <= floor)
                {
                    continue;
                }
                values.push(Occurrence {
                    rule_id: id.clone(),
                    nominal_date: nominal,
                    scheduled_date: scheduled.to_string(),
                    state: OccurrenceState::Virtual,
                    transaction_id: None,
                });
            }
        }
        if values.len() > 50_000 {
            return Err(invalid("range", "recurring_range_too_large"));
        }
    }
    values.sort_by(|a, b| {
        (&a.scheduled_date, &a.rule_id, &a.nominal_date).cmp(&(
            &b.scheduled_date,
            &b.rule_id,
            &b.nominal_date,
        ))
    });
    Ok(values)
}
pub fn occurrences(
    ctx: &Context<'_>,
    rule: Option<&str>,
    range: &DateRange,
    page: &PageReq,
) -> Result<Page<Occurrence>> {
    let from = range
        .from
        .as_deref()
        .map(parse_date)
        .transpose()?
        .unwrap_or(ctx.today);
    let to = range
        .to
        .as_deref()
        .map(parse_date)
        .transpose()?
        .unwrap_or(checked(from.checked_add_days(Days::new(30)))?);
    let cursor = ReadPage::new(
        ctx,
        page,
        &(
            "recurring_occurrences",
            rule,
            from.to_string(),
            to.to_string(),
        ),
    )?;
    let key = |row: &Occurrence| {
        (
            row.scheduled_date.clone(),
            format!("{}/{}", row.rule_id, row.nominal_date),
        )
    };
    let rows = for_range(ctx, rule, from, to)?
        .into_iter()
        .filter(|row| cursor.after.as_ref().is_none_or(|after| key(row) > *after))
        .take(cursor.limit + 1)
        .collect();
    cursor.finish(rows, key)
}

fn proposed(rule: &RecurringDetail, date: &str) -> TxDraft {
    let r = &rule.draft;
    TxDraft {
        account_id: r.account_id.clone(),
        bank_date: date.to_owned(),
        amount: money(&r.amount.currency, &r.amount.minor),
        direction: r.direction.clone(),
        status: TxStatus::Projected,
        description: r.description.clone(),
        notes: r.notes.clone(),
        payee_id: r.payee_id.clone(),
        category_id: r.category_id.clone(),
        splits: r.splits.clone(),
        effective_bill_date: None,
        reporting_date_override: None,
        ignored: false,
        exclude_from_pnl: false,
        flagged: false,
        flag_due_date: None,
        alias_id: None,
        statement_id: None,
        installment: None,
        original: None,
    }
}
pub fn occurrence_apply(
    ctx: &Context<'_>,
    id: &str,
    nominal: &str,
    action: &OccurrenceAction,
    transaction_id: Option<&str>,
) -> Result<Occurrence> {
    let nominal_date = parse_date(nominal)?;
    let rule = get(ctx, id)?;
    let previous = existing(ctx, id, nominal)?;
    let scheduled = if let Some(existing) = &previous {
        existing.scheduled_date.clone()
    } else {
        if !rule.draft.active
            || rule
                .generation_floor
                .as_ref()
                .is_some_and(|floor| nominal <= floor.as_str())
        {
            return Err(conflict("recurring_occurrence_unavailable"));
        }
        let shifted = move_weekend(nominal_date, &rule.draft.weekend_policy)?;
        dates(&rule.draft, shifted, shifted)?
            .into_iter()
            .find(|(date, _)| *date == nominal_date)
            .map(|(_, value)| value.to_string())
            .ok_or_else(|| invalid("nominalDate", "invalid_recurring_occurrence"))?
    };
    let mut draft = proposed(&rule, &scheduled);
    if let Some(amount) = super::cash_plan::staged_amount(ctx, &rule, &scheduled)? {
        draft.amount.minor = amount;
    }
    let existing_post = previous
        .as_ref()
        .and_then(|row| row.transaction_id.as_deref());
    let (state, posting) = match action {
        OccurrenceAction::Generate => {
            if let Some(previous) = previous {
                return Ok(previous);
            }
            let posting = tx::save(ctx, None, &draft)?;
            (OccurrenceState::Generated, Some(posting.row.id))
        }
        OccurrenceAction::Skip => {
            if let Some(id) = existing_post {
                if !matches!(
                    previous.as_ref().map(|row| &row.state),
                    Some(OccurrenceState::Generated)
                ) || !matches!(tx::get(ctx, id)?.row.status, TxStatus::Projected)
                {
                    return Err(conflict("occurrence_has_actual_posting"));
                }
                ctx.tx.execute(
                    "DELETE FROM recurring_occurrences WHERE rule_id=?1 AND nominal_date=?2",
                    params![rule.id, nominal],
                )?;
                tx::remove(ctx, id, false)?;
            }
            (OccurrenceState::Skipped, None)
        }
        OccurrenceAction::Match => {
            let id = transaction_id.ok_or_else(|| invalid("transactionId", "required_field"))?;
            let posting = tx::get(ctx, id)?;
            let linked: bool = ctx.tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM cash_plan_entries WHERE transaction_id=?1)",
                [id],
                |r| r.get(0),
            )?;
            if linked {
                return Err(conflict("cash_plan_posting_already_linked"));
            }
            if posting.row.account_id != rule.draft.account_id
                || posting.row.status.as_str() != "posted"
                || posting.row.direction.as_str() != rule.draft.direction.as_str()
                || posting.row.amount.minor != draft.amount.minor
                || posting.row.amount.currency != rule.draft.amount.currency
                || posting.row.entry_kind != tx::EntryKind::Ordinary
            {
                return Err(invalid("transactionId", "recurring_match_mismatch"));
            }
            if let Some(old) = existing_post.filter(|other| *other != id) {
                if !matches!(
                    previous.as_ref().map(|row| &row.state),
                    Some(OccurrenceState::Generated)
                ) || !matches!(tx::get(ctx, old)?.row.status, TxStatus::Projected)
                {
                    return Err(conflict("occurrence_already_matched"));
                }
                ctx.tx.execute(
                    "DELETE FROM recurring_occurrences WHERE rule_id=?1 AND nominal_date=?2",
                    params![rule.id, nominal],
                )?;
                tx::remove(ctx, old, false)?;
            }
            (OccurrenceState::Matched, Some(id.to_owned()))
        }
        OccurrenceAction::Unmatch => {
            if !matches!(
                previous.as_ref().map(|row| &row.state),
                Some(OccurrenceState::Matched)
            ) {
                return Err(conflict("occurrence_not_matched"));
            }
            // Keep the actual posting and suppress the occurrence until explicitly rematched.
            (OccurrenceState::Skipped, None)
        }
        OccurrenceAction::Post => {
            if scheduled > ctx.today.to_string() {
                return Err(invalid("scheduledDate", "occurrence_not_due"));
            }
            let current = if let Some(id) = existing_post {
                tx::get(ctx, id)?
            } else {
                if previous.is_some() {
                    return Err(conflict("occurrence_skipped"));
                }
                tx::save(ctx, None, &draft)?
            };
            if matches!(current.row.status, TxStatus::Projected) {
                if current.row.amount.minor != draft.amount.minor
                    && super::cash_plan::staged_amount(ctx, &rule, &scheduled)?.is_some()
                {
                    return Err(conflict("staged_occurrence_amount_changed"));
                }
                tx::post(
                    ctx,
                    &VersionedId {
                        id: current.row.id.clone(),
                        version: current.row.version,
                    },
                    Some(&scheduled),
                )?;
            }
            (OccurrenceState::Matched, Some(current.row.id))
        }
    };
    ctx.tx.execute("INSERT INTO recurring_occurrences VALUES(?1,?2,?3,?4,?5,?6)
        ON CONFLICT(rule_id,nominal_date) DO UPDATE SET scheduled_date=excluded.scheduled_date,
        transaction_id=excluded.transaction_id,state=excluded.state,rule_version=excluded.rule_version",
        params![rule.id,nominal,scheduled,posting,state.as_str(),crate::ledger_engine::minor(&rule.version)?])?;
    existing(ctx, &rule.id, nominal)?.ok_or_else(|| missing("nominalDate"))
}

impl ReceiptValue for RecurringDetail {
    type Reference = String;
    fn reference(&self, _: &Context<'_>) -> String {
        self.id.clone()
    }
    fn reload(ctx: &Context<'_>, id: String) -> Result<Self> {
        get(ctx, &id)
    }
}
impl ReceiptValue for Occurrence {
    type Reference = (String, String);
    fn reference(&self, _: &Context<'_>) -> Self::Reference {
        (self.rule_id.clone(), self.nominal_date.clone())
    }
    fn reload(ctx: &Context<'_>, (id, nominal): Self::Reference) -> Result<Self> {
        existing(ctx, &id, &nominal)?.ok_or_else(|| missing("nominalDate"))
    }
}

pub fn generate_due(ctx: &Context<'_>) -> Result<usize> {
    let ids = ctx
        .tx
        .prepare_cached(
            "SELECT r.id FROM recurring_rules r JOIN accounts a ON a.id=r.account_id
        WHERE r.active=1 AND r.auto_generate=1 AND a.archived_on IS NULL ORDER BY r.id",
        )?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<crate::sql::Result<Vec<_>>>()?;
    let mut generated = 0;
    for id in ids {
        let rule = get(ctx, &id)?;
        let start = parse_date(&rule.draft.starts_on)?;
        // An occurrence scheduled before the nominal start can be shifted back by a weekend.
        let from = start.checked_sub_days(Days::new(2)).unwrap_or(start);
        if from > ctx.today {
            continue;
        }
        for (nominal, _) in dates(&rule.draft, from, ctx.today)? {
            let nominal = nominal.to_string();
            if rule
                .generation_floor
                .as_ref()
                .is_some_and(|floor| &nominal <= floor)
                || existing(ctx, &id, &nominal)?.is_some()
            {
                continue;
            }
            occurrence_apply(ctx, &id, &nominal, &OccurrenceAction::Generate, None)?;
            generated += 1;
            if generated >= 200 {
                break;
            }
        }
        if generated >= 200 {
            break;
        }
    }
    if generated > 0 {
        ctx.tx.execute(
            "UPDATE ledger_meta SET revision=revision+1 WHERE singleton=1",
            [],
        )?;
    }
    Ok(generated)
}
