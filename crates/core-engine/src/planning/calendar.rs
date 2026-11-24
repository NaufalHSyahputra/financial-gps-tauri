use chrono::{Datelike, Days, Months, NaiveDate};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    None,
    PreviousWorkingDay,
    NextWorkingDay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    anchor: u32,
    pub policy: Policy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Revision {
    pub effective_from: NaiveDate,
    pub settings: Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CycleKind {
    Regular,
    Short,
    Extended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cycle {
    pub start: NaiveDate,
    pub end: NaiveDate,
    pub kind: CycleKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub date: NaiveDate,
    pub elapsed: i64,
    pub remaining: i64,
    pub total: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum CalendarError {
    #[error("Choose an anchor day from 1 to 31.")]
    Anchor,
    #[error("Financial calendar history is unavailable for this date.")]
    History,
    #[error("The date is outside the cycle.")]
    OutsideCycle,
    #[error("The date is outside the supported calendar range.")]
    DateRange,
}

type Result<T> = std::result::Result<T, CalendarError>;

impl Settings {
    pub fn validate(self) -> Result<Self> {
        if (1..=31).contains(&self.anchor) {
            Ok(self)
        } else {
            Err(CalendarError::Anchor)
        }
    }

    fn boundary(self, month: NaiveDate) -> Result<NaiveDate> {
        self.validate()?;
        let mut date = (1..=self.anchor)
            .rev()
            .find_map(|day| month.with_day(day))
            .ok_or(CalendarError::DateRange)?;
        while date.weekday().number_from_monday() > 5 {
            date = match self.policy {
                Policy::None => break,
                Policy::PreviousWorkingDay => date.pred_opt(),
                Policy::NextWorkingDay => date.succ_opt(),
            }
            .ok_or(CalendarError::DateRange)?;
        }
        Ok(date)
    }

    fn window(self, date: NaiveDate) -> Result<[NaiveDate; 5]> {
        let month = date.with_day(1).ok_or(CalendarError::DateRange)?;
        let mut boundaries = [date; 5];
        for (index, boundary) in boundaries.iter_mut().enumerate() {
            let month = if index < 2 {
                month.checked_sub_months(Months::new((2 - index) as u32))
            } else {
                month.checked_add_months(Months::new((index - 2) as u32))
            }
            .ok_or(CalendarError::DateRange)?;
            *boundary = self.boundary(month)?;
        }
        Ok(boundaries)
    }

    pub fn regular(self, date: NaiveDate) -> Result<Cycle> {
        let bounds = self.window(date)?;
        let pair = bounds
            .windows(2)
            .find(|pair| pair[0] <= date && date < pair[1]);
        let pair = pair.ok_or(CalendarError::DateRange)?;
        Cycle::new(pair[0], pair[1], CycleKind::Regular)
    }
}

impl Cycle {
    fn new(start: NaiveDate, next: NaiveDate, kind: CycleKind) -> Result<Self> {
        let end = next
            .checked_sub_days(Days::new(1))
            .ok_or(CalendarError::DateRange)?;
        Ok(Self { start, end, kind })
    }

    pub fn next(self) -> Result<NaiveDate> {
        self.end.succ_opt().ok_or(CalendarError::DateRange)
    }

    pub fn position(self, date: NaiveDate) -> Result<Position> {
        if date < self.start || date > self.end {
            return Err(CalendarError::OutsideCycle);
        }
        Ok(Position {
            date,
            total: (self.end - self.start).num_days() + 1,
            elapsed: (date - self.start).num_days() + 1,
            remaining: (self.end - date).num_days() + 1,
        })
    }
}

pub fn active(revisions: &[Revision], date: NaiveDate) -> Result<(Revision, Cycle)> {
    let index = revisions.partition_point(|revision| revision.effective_from <= date);
    let revision = *revisions
        .get(index.checked_sub(1).ok_or(CalendarError::History)?)
        .ok_or(CalendarError::History)?;
    if index > 1 {
        let start = revision.effective_from;
        let bounds = revision.settings.window(start)?;
        let mut next = bounds.into_iter().filter(|boundary| *boundary >= start);
        let first = next.next().ok_or(CalendarError::DateRange)?;
        let transition = if (first - start).num_days() < 15 {
            Cycle::new(
                start,
                next.next().ok_or(CalendarError::DateRange)?,
                CycleKind::Extended,
            )?
        } else {
            Cycle::new(start, first, CycleKind::Short)?
        };
        if date <= transition.end {
            return Ok((revision, transition));
        }
    }
    Ok((revision, revision.settings.regular(date)?))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Scheduled(Revision),
    Replaced(Revision),
    Cancelled,
    NoChange,
}

pub fn change(revisions: &[Revision], date: NaiveDate, settings: Settings) -> Result<Change> {
    settings.validate()?;
    let (current, cycle) = active(revisions, date)?;
    let pending = revisions
        .last()
        .filter(|revision| revision.effective_from > date);
    if settings == current.settings {
        return Ok(if pending.is_some() {
            Change::Cancelled
        } else {
            Change::NoChange
        });
    }
    let revision = Revision {
        effective_from: cycle.next()?,
        settings,
    };
    Ok(match pending {
        Some(pending) if *pending == revision => Change::NoChange,
        Some(_) => Change::Replaced(revision),
        None => Change::Scheduled(revision),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(value: &str) -> NaiveDate {
        value.parse().unwrap()
    }

    #[test]
    fn boundaries_are_increasing_for_all_anchors_and_policies() {
        for year in [1999, 2000, 2023, 2024, 2099, 2100] {
            for policy in [
                Policy::None,
                Policy::PreviousWorkingDay,
                Policy::NextWorkingDay,
            ] {
                for anchor in 1..=31 {
                    let settings = Settings { anchor, policy };
                    let mut month = NaiveDate::from_ymd_opt(year, 1, 1).unwrap();
                    let mut previous = settings.boundary(month).unwrap();
                    for _ in 0..12 {
                        month = month.checked_add_months(Months::new(1)).unwrap();
                        let next = settings.boundary(month).unwrap();
                        assert!(next > previous);
                        for offset in 0..(next - previous).num_days() {
                            let today = previous + Days::new(offset as u64);
                            let cycle = settings.regular(today).unwrap();
                            assert_eq!((cycle.start, cycle.next().unwrap()), (previous, next));
                            let position = cycle.position(today).unwrap();
                            assert_eq!(position.elapsed + position.remaining, position.total + 1);
                        }
                        previous = next;
                    }
                }
            }
        }
    }

    #[test]
    fn clamp_before_weekday_adjustment() {
        let month = date("2026-02-01");
        let settings = |policy| Settings { anchor: 31, policy };
        assert_eq!(
            settings(Policy::None).boundary(month).unwrap(),
            date("2026-02-28")
        );
        assert_eq!(
            settings(Policy::PreviousWorkingDay)
                .boundary(month)
                .unwrap(),
            date("2026-02-27")
        );
        assert_eq!(
            settings(Policy::NextWorkingDay).boundary(month).unwrap(),
            date("2026-03-02")
        );
        for anchor in [0, 32] {
            assert!(
                Settings {
                    anchor,
                    policy: Policy::None
                }
                .validate()
                .is_err()
            );
        }
    }

    #[test]
    fn transition_protects_active_cycle_and_uses_adjusted_threshold() {
        let original = Revision {
            effective_from: date("2026-10-01"),
            settings: Settings {
                anchor: 1,
                policy: Policy::None,
            },
        };
        let today = date("2026-10-20");
        assert!(matches!(
            active(&[original], date("2026-09-30")),
            Err(CalendarError::History)
        ));
        for (anchor, policy, kind) in [
            (15, Policy::None, CycleKind::Extended),
            (16, Policy::None, CycleKind::Short),
            (17, Policy::None, CycleKind::Short),
            (16, Policy::PreviousWorkingDay, CycleKind::Short),
            (15, Policy::PreviousWorkingDay, CycleKind::Extended),
            (15, Policy::NextWorkingDay, CycleKind::Short),
        ] {
            let Change::Scheduled(next) =
                change(&[original], today, Settings { anchor, policy }).unwrap()
            else {
                panic!()
            };
            let revisions = [original, next];
            assert_eq!(active(&revisions, today).unwrap().1.end, date("2026-10-31"));
            let transition = active(&revisions, date("2026-11-01")).unwrap().1;
            assert_eq!(transition.kind, kind);
            assert_eq!(transition.position(transition.end).unwrap().remaining, 1);
            assert_eq!(
                change(&revisions, today, original.settings).unwrap(),
                Change::Cancelled
            );
            let Change::Scheduled(later) =
                change(&revisions, transition.start, original.settings).unwrap()
            else {
                panic!()
            };
            assert_eq!(later.effective_from, transition.next().unwrap());
        }
    }
}

use crate::{
    domain::{AppError, ErrorCode},
    ledger_engine::{Context, Deleted, dto, invalid},
    sql::{OptionalExtension, params},
};

dto! {
    pub struct CalendarInput { pub timezone: String, pub anchor: u8, pub weekday_policy: CalendarPolicy }
    pub enum CalendarPolicy { None, PreviousWorkingDay, NextWorkingDay }
    pub enum FinancialCycleKind { Regular, Short, Extended }
    pub struct FinancialCycle {
        pub start: String, pub end: String, pub kind: FinancialCycleKind,
        pub elapsed: u16, pub remaining: u16, pub days: u16,
    }
    pub struct PendingCalendar { pub effective_from: String, pub settings: CalendarInput }
    pub struct FinancialCalendarView {
        pub settings: CalendarInput, pub today: String, pub history_floor: String,
        pub current: FinancialCycle, pub pending: Option<PendingCalendar>,
    }
}

impl CalendarPolicy {
    fn key(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::PreviousWorkingDay => "previousWorkingDay",
            Self::NextWorkingDay => "nextWorkingDay",
        }
    }

    fn read(value: &str) -> crate::sql::Result<Self> {
        match value {
            "none" => Ok(Self::None),
            "previousWorkingDay" => Ok(Self::PreviousWorkingDay),
            "nextWorkingDay" => Ok(Self::NextWorkingDay),
            _ => Err(crate::sql::Error::InvalidQuery),
        }
    }
}

impl From<CalendarError> for AppError {
    fn from(error: CalendarError) -> Self {
        match error {
            CalendarError::Anchor => invalid("anchor", "invalid_payday_anchor"),
            CalendarError::History => Self::new(
                ErrorCode::Conflict,
                "financial_calendar_history_unavailable",
            ),
            CalendarError::OutsideCycle | CalendarError::DateRange => {
                invalid("date", "invalid_cycle_date")
            }
        }
    }
}

pub(crate) struct Calendar {
    timezone: String,
    pub(crate) today: NaiveDate,
    revisions: Vec<Revision>,
}

impl Calendar {
    pub(crate) fn cycle(&self, date: NaiveDate) -> crate::domain::Result<Cycle> {
        Ok(active(&self.revisions, date)?.1)
    }
}

pub(crate) fn load(ctx: &Context<'_>) -> crate::domain::Result<Option<Calendar>> {
    let timezone: Option<String> = ctx
        .tx
        .query_row(
            "SELECT timezone FROM financial_calendar WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let Some(timezone) = timezone else {
        return Ok(None);
    };
    let today = local_date(ctx, &timezone)?;
    let mut statement = ctx.tx.prepare(
        "SELECT effective_from,anchor,weekday_policy FROM financial_calendar_revisions ORDER BY effective_from",
    )?;
    let revisions = statement
        .query_map([], |row| {
            let day: String = row.get(0)?;
            let effective_from = day.parse().map_err(|_| crate::sql::Error::InvalidQuery)?;
            let policy = match CalendarPolicy::read(&row.get::<_, String>(2)?)? {
                CalendarPolicy::None => Policy::None,
                CalendarPolicy::PreviousWorkingDay => Policy::PreviousWorkingDay,
                CalendarPolicy::NextWorkingDay => Policy::NextWorkingDay,
            };
            Ok(Revision {
                effective_from,
                settings: Settings {
                    anchor: row.get(1)?,
                    policy,
                },
            })
        })?
        .collect::<crate::sql::Result<Vec<_>>>()?;
    if revisions.is_empty() {
        return Err(AppError::new(
            ErrorCode::Integrity,
            "financial_calendar_missing_history",
        ));
    }
    Ok(Some(Calendar {
        timezone,
        today,
        revisions,
    }))
}

pub(crate) fn configured(ctx: &Context<'_>) -> crate::domain::Result<Calendar> {
    load(ctx)?.ok_or_else(|| AppError::new(ErrorCode::Conflict, "financial_calendar_required"))
}

fn local_date(ctx: &Context<'_>, timezone: &str) -> crate::domain::Result<NaiveDate> {
    let zone = match timezone {
        "Asia/Jakarta" => chrono_tz::Asia::Jakarta,
        "Asia/Makassar" => chrono_tz::Asia::Makassar,
        "Asia/Jayapura" => chrono_tz::Asia::Jayapura,
        _ => return Err(invalid("timezone", "invalid_financial_timezone")),
    };
    let instant = chrono::DateTime::from_timestamp_millis(ctx.now_ms)
        .ok_or_else(|| invalid("date", "invalid_cycle_date"))?;
    Ok(instant.with_timezone(&zone).date_naive())
}

fn input(timezone: &str, settings: Settings) -> CalendarInput {
    CalendarInput {
        timezone: timezone.to_owned(),
        anchor: settings.anchor as u8,
        weekday_policy: match settings.policy {
            Policy::None => CalendarPolicy::None,
            Policy::PreviousWorkingDay => CalendarPolicy::PreviousWorkingDay,
            Policy::NextWorkingDay => CalendarPolicy::NextWorkingDay,
        },
    }
}

pub(super) fn describe(cycle: Cycle, date: NaiveDate) -> Result<FinancialCycle> {
    let position = cycle.position(date)?;
    Ok(FinancialCycle {
        start: cycle.start.to_string(),
        end: cycle.end.to_string(),
        kind: match cycle.kind {
            CycleKind::Regular => FinancialCycleKind::Regular,
            CycleKind::Short => FinancialCycleKind::Short,
            CycleKind::Extended => FinancialCycleKind::Extended,
        },
        elapsed: position.elapsed as u16,
        remaining: position.remaining as u16,
        days: position.total as u16,
    })
}

pub fn get(ctx: &Context<'_>) -> crate::domain::Result<Option<FinancialCalendarView>> {
    let Some(calendar) = load(ctx)? else {
        return Ok(None);
    };
    let today = calendar.today;
    let (revision, cycle) = active(&calendar.revisions, today)?;
    Ok(Some(FinancialCalendarView {
        settings: input(&calendar.timezone, revision.settings),
        today: today.to_string(),
        history_floor: calendar.revisions[0].effective_from.to_string(),
        current: describe(cycle, today)?,
        pending: calendar
            .revisions
            .last()
            .filter(|entry| entry.effective_from > today)
            .map(|entry| PendingCalendar {
                effective_from: entry.effective_from.to_string(),
                settings: input(&calendar.timezone, entry.settings),
            }),
    }))
}

pub fn save(ctx: &Context<'_>, draft: &CalendarInput) -> crate::domain::Result<Deleted> {
    let today = local_date(ctx, &draft.timezone)?;
    let settings = Settings {
        anchor: u32::from(draft.anchor),
        policy: match draft.weekday_policy {
            CalendarPolicy::None => Policy::None,
            CalendarPolicy::PreviousWorkingDay => Policy::PreviousWorkingDay,
            CalendarPolicy::NextWorkingDay => Policy::NextWorkingDay,
        },
    }
    .validate()?;

    let effective_from = if let Some(calendar) = load(ctx)? {
        // Evaluate changes at the new local date, but never reopen a revision already in effect.
        let date = today
            .max(calendar.today)
            .max(calendar.revisions[0].effective_from);
        match change(&calendar.revisions, date, settings)? {
            Change::Scheduled(rev) | Change::Replaced(rev) => Some(rev.effective_from),
            Change::Cancelled => {
                ctx.tx.execute(
                    "DELETE FROM financial_calendar_revisions WHERE effective_from>?1",
                    [date.to_string()],
                )?;
                None
            }
            Change::NoChange => None,
        }
    } else {
        Some(settings.regular(today)?.start)
    };
    ctx.tx.execute(
        "INSERT INTO financial_calendar VALUES (1,?1) ON CONFLICT(singleton) DO UPDATE SET timezone=excluded.timezone",
        [&draft.timezone],
    )?;
    if let Some(date) = effective_from {
        ctx.tx.execute(
            "INSERT INTO financial_calendar_revisions VALUES (?1,?2,?3)
             ON CONFLICT(effective_from) DO UPDATE SET anchor=excluded.anchor,weekday_policy=excluded.weekday_policy",
            params![date.to_string(), draft.anchor, draft.weekday_policy.key()],
        )?;
    }
    Ok(Deleted {
        id: "financialCalendar".into(),
    })
}
