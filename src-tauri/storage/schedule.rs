use crate::{
    crypto::{BackupSecretStore, Credential},
    domain::{AppError, Result},
    ledger_engine::{accounts, *},
};
use chrono::{DateTime, Datelike, NaiveDate, NaiveTime, TimeZone, Utc};
use core_engine::sql::{OptionalExtension, params};
use uuid::Uuid;

dto! {
    pub enum BackupFrequency {Daily,Weekly}
    pub struct Schedule {pub frequency:BackupFrequency,pub local_time:String,pub weekday:Option<u8>,pub retain_days:Option<u16>}
    pub struct BackupSchedule {pub schedule:Option<Schedule>,pub remembered:bool,pub last_attempt:Option<String>,
        pub last_success:Option<String>,pub error:Option<String>, pub destination_name:Option<String>}
    pub struct ScheduleChange {pub schedule:Option<Schedule>,pub remember_secret:Option<bool>}
}
const KEY: &str = "backup.schedule";
const EMPTY: BackupSchedule = BackupSchedule {
    schedule: None,
    remembered: false,
    last_attempt: None,
    last_success: None,
    error: None,
    destination_name: None,
};
pub fn get(ctx: &Context<'_>) -> Result<BackupSchedule> {
    let value = ctx
        .tx
        .query_row(
            "SELECT value_json FROM ledger_settings WHERE key=?1",
            [KEY],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    value
        .map(|json| serde_json::from_str(&json).map_err(codec_error))
        .transpose()
        .map(|value| value.unwrap_or(EMPTY))
}
fn save(ctx: &Context<'_>, value: &BackupSchedule) -> Result<()> {
    ctx.tx.execute("INSERT INTO ledger_settings VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json",
        params![KEY,serde_json::to_string(value).map_err(codec_error)?])?;
    Ok(())
}
fn time(schedule: &Schedule) -> Result<NaiveTime> {
    if schedule.local_time.len() != 5
        || matches!(schedule.frequency, BackupFrequency::Weekly) != schedule.weekday.is_some()
        || schedule.weekday.is_some_and(|day| !(1..=7).contains(&day))
        || schedule
            .retain_days
            .is_some_and(|days| !(1..=3660).contains(&days))
    {
        return Err(invalid("schedule", "invalid_backup_schedule"));
    }
    NaiveTime::parse_from_str(&schedule.local_time, "%H:%M")
        .map_err(|_| invalid("localTime", "invalid_backup_time"))
}
fn instance(ctx: &Context<'_>) -> Result<Uuid> {
    Uuid::parse_str(&ctx.scope.ledger_instance_id)
        .map_err(|_| invalid("scope", "invalid_ledger_instance"))
}
pub(super) fn set(
    ctx: &Context<'_>,
    change: &ScheduleChange,
    credential: Option<&Credential>,
    store: &dyn BackupSecretStore,
    destination: Option<&super::ApprovedPath>,
) -> Result<BackupSchedule> {
    let mut status = get(ctx)?;
    let destination = destination
        .map(super::ApprovedPath::backup_directory)
        .transpose()?;
    if destination.is_some() && change.schedule.is_none() {
        return Err(invalid("destinationGrant", "schedule_required"));
    }
    let previous = destination_root(ctx)?;
    let destination_json = destination
        .map(serde_json::to_string)
        .transpose()
        .map_err(codec_error)?;

    if let Some(schedule) = change.schedule.as_ref() {
        time(schedule)?;
    }
    match change.remember_secret {
        Some(true) => {
            let credential =
                credential.ok_or_else(|| invalid("backupPassphrase", "credential_required"))?;
            store.remember(instance(ctx)?, credential)?;
            status.remembered = true;
        }
        Some(false) => {
            if credential.is_some() {
                return Err(invalid(
                    "rememberSecret",
                    "backup_credential_option_conflict",
                ));
            }
            store.revoke(instance(ctx)?)?;
            status.remembered = false;
        }
        None if credential.is_some() => {
            return Err(invalid(
                "rememberSecret",
                "backup_credential_option_conflict",
            ));
        }
        None => {}
    }
    let changed = digest(&status.schedule)? != digest(&change.schedule)?
        || previous.as_deref() != destination;
    if let Some(json) = destination_json {
        ctx.tx.execute("INSERT INTO ledger_settings VALUES ('backup.destination',?1) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json",[json])?;
    } else {
        ctx.tx.execute(
            "DELETE FROM ledger_settings WHERE key='backup.destination'",
            [],
        )?;
    }
    status.destination_name = destination
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned());
    status.schedule = change.schedule.clone();
    if changed {
        status.last_attempt = None;
        status.last_success = None;
    }
    status.error = None;
    save(ctx, &status)?;
    Ok(status)
}
fn occurrence<T: TimeZone>(
    schedule: &Schedule,
    zone: T,
    now: DateTime<Utc>,
) -> Result<DateTime<Utc>> {
    let time = time(schedule)?;
    let local = now.with_timezone(&zone);
    let date = local.date_naive();
    let prior_days = schedule
        .weekday
        .map(|weekday| (7 + date.weekday().number_from_monday() - u32::from(weekday)) % 7)
        .unwrap_or(0);
    let invalid_date = || invalid("schedule", "invalid_backup_due_date");
    let date = date
        .checked_sub_days(chrono::Days::new(u64::from(prior_days)))
        .ok_or_else(invalid_date)?;
    let resolve = |date: NaiveDate| -> Result<DateTime<Utc>> {
        let start = date.and_time(time);
        // Move into the first valid wall minute on spring-forward days.
        for minute in 0..=180 {
            let local = start
                .checked_add_signed(chrono::Duration::minutes(minute))
                .ok_or_else(invalid_date)?;
            if let Some(value) = zone.from_local_datetime(&local).earliest() {
                return Ok(value.with_timezone(&Utc));
            }
        }
        Err(invalid_date())
    };
    let due = resolve(date)?;
    if due <= now {
        return Ok(due);
    }
    let prior = if matches!(schedule.frequency, BackupFrequency::Daily) {
        1
    } else {
        7
    };
    resolve(
        date.checked_sub_days(chrono::Days::new(prior))
            .ok_or_else(invalid_date)?,
    )
}
pub(super) fn destination_root(ctx: &Context<'_>) -> Result<Option<std::path::PathBuf>> {
    let value = ctx
        .tx
        .query_row(
            "SELECT value_json FROM ledger_settings WHERE key='backup.destination'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    value
        .map(|value| serde_json::from_str(&value).map_err(codec_error))
        .transpose()
}

pub(crate) struct Due {
    pub stamp: String,
    pub config_hash: String,
    pub retain_days: Option<u16>,
    pub destination: Option<std::path::PathBuf>,
}
pub(crate) fn due(ctx: &Context<'_>) -> Result<Option<Due>> {
    let status = get(ctx)?;
    let Some(schedule) = status.schedule.as_ref() else {
        return Ok(None);
    };
    let now = DateTime::<Utc>::from_timestamp_millis(ctx.now_ms)
        .ok_or_else(|| integrity("invalid_clock"))?;
    let due = match accounts::settings(ctx)?.timezone_override {
        Some(zone) => occurrence(
            schedule,
            zone.parse::<chrono_tz::Tz>()
                .map_err(|_| integrity("invalid_stored_timezone"))?,
            now,
        )?,
        None => occurrence(schedule, chrono::Local, now)?,
    };
    let parse = |text: &str| {
        DateTime::parse_from_rfc3339(text)
            .ok()
            .map(|date| date.with_timezone(&Utc))
    };
    if status
        .last_success
        .as_deref()
        .and_then(parse)
        .is_some_and(|last| last >= due)
        || status
            .last_attempt
            .as_deref()
            .and_then(parse)
            .is_some_and(|last| now - last < chrono::Duration::minutes(15))
    {
        return Ok(None);
    }
    Ok(Some(Due {
        stamp: now.to_rfc3339(),
        config_hash: config_hash(ctx, &status)?,
        retain_days: schedule.retain_days,
        destination: destination_root(ctx)?,
    }))
}
fn config_hash(ctx: &Context<'_>, value: &BackupSchedule) -> Result<String> {
    digest(&(&value.schedule, value.remembered, destination_root(ctx)?))
}
pub(super) fn password(ctx: &Context<'_>, store: &dyn BackupSecretStore) -> Result<Credential> {
    if !get(ctx)?.remembered {
        return Err(invalid(
            "backupPassphrase",
            "scheduled_backup_needs_remembered_secret",
        ));
    }
    store.retrieve(instance(ctx)?)
}
pub(crate) fn approve(ctx: &Context<'_>, run: &Due) -> Result<()> {
    if config_hash(ctx, &get(ctx)?)? != run.config_hash {
        return Err(stale_revision());
    }
    Ok(())
}
pub(crate) fn finish(
    ctx: &Context<'_>,
    run: &Due,
    result: &Result<super::backups::BackupSummary>,
) -> Result<()> {
    let mut status = get(ctx)?;
    if config_hash(ctx, &status)? != run.config_hash {
        return Ok(());
    }
    status.last_attempt = Some(run.stamp.clone());
    status.error = result
        .as_ref()
        .err()
        .map(|error: &AppError| error.message_key.to_string());
    if result.is_ok() {
        status.last_success = Some(run.stamp.clone());
    }
    save(ctx, &status)
}
impl ReceiptValue for BackupSchedule {
    type Reference = ();
    fn reference(&self, _: &Context<'_>) {}
    fn reload(ctx: &Context<'_>, (): ()) -> Result<Self> {
        get(ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn daily_catches_up_at_first_valid_dst_minute_and_weekly_uses_previous_due() {
        let mut schedule = Schedule {
            frequency: BackupFrequency::Daily,
            local_time: "02:30".into(),
            weekday: None,
            retain_days: None,
        };
        let zone = chrono_tz::America::New_York;
        let now = "2026-03-08T08:00:00Z".parse().unwrap();
        assert_eq!(
            occurrence(&schedule, zone, now).unwrap(),
            "2026-03-08T07:00:00Z".parse::<DateTime<Utc>>().unwrap()
        );
        schedule.frequency = BackupFrequency::Weekly;
        schedule.weekday = Some(7);
        let now = "2026-03-08T06:00:00Z".parse().unwrap();
        assert_eq!(
            occurrence(&schedule, zone, now).unwrap(),
            "2026-03-01T07:30:00Z".parse::<DateTime<Utc>>().unwrap()
        );
    }
}
