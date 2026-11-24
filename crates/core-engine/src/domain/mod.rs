mod error;
mod ledger;
mod money;

use chrono::{DateTime, Local, NaiveDate, Utc};
use chrono_tz::Tz;
pub use error::{AppError, ErrorCode, Result};
pub use ledger::{
    LedgerDescriptor, LedgerSession, LifecycleState, OpenResult, Protection, Scope, StoragePolicy,
    UnlockChallenge,
};
pub use money::{CurrencyCode, CurrencyScale, Money};
use unicode_casefold::UnicodeCaseFold;
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

pub trait Clock {
    fn now(&self) -> DateTime<Utc>;

    fn local_date(&self, instant: DateTime<Utc>) -> NaiveDate {
        instant.with_timezone(&Local).date_naive()
    }

    fn today(&self, timezone: Option<Tz>) -> NaiveDate {
        let now = self.now();
        timezone.map_or_else(
            || self.local_date(now),
            |zone| now.with_timezone(&zone).date_naive(),
        )
    }
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

pub fn parse_date(value: &str) -> Result<NaiveDate> {
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|_| AppError::validation("date", "invalid_date"))?;
    if value.len() != 10 || value.starts_with("0000") || date.to_string() != value {
        return Err(AppError::validation("date", "invalid_date"));
    }
    Ok(date)
}

pub fn normalize(value: &str) -> String {
    value
        .nfkd()
        .case_fold()
        .filter(|ch| !is_combining_mark(*ch))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn io_error(error: std::io::Error) -> AppError {
    let (code, key) = match error.kind() {
        std::io::ErrorKind::StorageFull => (ErrorCode::InsufficientSpace, "storage_full"),
        std::io::ErrorKind::AlreadyExists => (ErrorCode::Conflict, "destination_exists"),
        std::io::ErrorKind::PermissionDenied => (ErrorCode::Io, "file_access_denied"),
        std::io::ErrorKind::NotFound => (ErrorCode::Io, "file_not_found"),
        _ => (ErrorCode::Io, "file_operation_failed"),
    };
    AppError::new(code, key)
}

pub const NORMALIZE_SQL_FUNCTION: &str = "financial_normalize";
