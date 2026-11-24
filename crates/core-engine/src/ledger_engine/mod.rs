mod receipts;
pub mod staging;
pub use receipts::ReceiptValue;
pub mod accounts;
pub mod catalog;
pub mod corrections;
pub mod export;
pub mod fx;
pub mod reports;
pub mod transactions;
pub mod transfers;
pub mod workflows;

use crate::domain::normalize;
use crate::sql::{OptionalExtension, Transaction, params};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use unicode_normalization::char::is_combining_mark;
use uuid::Uuid;

use crate::domain::{AppError, Clock, ErrorCode, Result, Scope, SystemClock, parse_date};

#[macro_export]
macro_rules! dto {
    ($($item:item)*) => {$(
        #[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        $item
    )*};
}
pub use dto;

dto! {
    pub struct Money { pub currency: String, pub minor: String }
    pub struct VersionedId { pub id: String, pub version: String }
    pub struct Mutation { pub scope: Scope, pub request_id: String, pub expected_revision: Option<String> }
    pub struct Write<T> { pub value: T, pub revision: String, pub affected: Vec<String> }
    pub struct Deleted { pub id: String }
    pub struct DateRange { pub from: Option<String>, pub to: Option<String> }
    pub struct PageReq { pub cursor: Option<String>, pub limit: u16 }
    pub struct Page<T> { pub rows: Vec<T>, #[serde(default,skip_serializing_if="Option::is_none")] pub next_cursor: Option<String>, pub revision: String }
    pub enum DateBasis { Bank, Reporting, Effective }
    pub enum Direction { Debit, Credit }
    pub enum TxStatus { Posted, Pending, Projected }
    pub enum AccountingMode { Cash, Accrual }
}

impl DateBasis {
    pub(crate) fn column(&self) -> &'static str {
        match self {
            Self::Bank => "bank_date",
            Self::Reporting => "reporting_date",
            Self::Effective => "effective_date",
        }
    }
}

impl Direction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Debit => "debit",
            Self::Credit => "credit",
        }
    }
    pub fn opposite(&self) -> Self {
        match self {
            Self::Debit => Self::Credit,
            Self::Credit => Self::Debit,
        }
    }
}

impl TxStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Posted => "posted",
            Self::Pending => "pending",
            Self::Projected => "projected",
        }
    }
}

impl AccountingMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Cash => "cash",
            Self::Accrual => "accrual",
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Cursor {
    key: String,
    id: String,
    revision: String,
    session: String,
    fingerprint: String,
}

pub struct ReadPage {
    pub limit: usize,
    pub after: Option<(String, String)>,
    revision: String,
    session: String,
    fingerprint: String,
}

impl ReadPage {
    pub fn new(ctx: &Context<'_>, page: &PageReq, filter: &impl Serialize) -> Result<Self> {
        if !(1..=200).contains(&page.limit) {
            return Err(invalid("limit", "page_limit_out_of_range"));
        }
        let fingerprint = digest(filter)?;
        let revision = revision(&ctx.tx)?;
        let session = ctx.scope.session_id.clone();
        let after = page
            .cursor
            .as_ref()
            .map(|token| {
                if token.len() > 16_384 {
                    return Err(invalid("cursor", "invalid_cursor"));
                }
                let bytes = URL_SAFE_NO_PAD
                    .decode(token)
                    .map_err(|_| invalid("cursor", "invalid_cursor"))?;
                let cursor: Cursor = serde_json::from_slice(&bytes)
                    .map_err(|_| invalid("cursor", "invalid_cursor"))?;
                if cursor.revision != revision
                    || cursor.fingerprint != fingerprint
                    || cursor.session != session
                {
                    return Err(stale_revision());
                }
                Ok((cursor.key, cursor.id))
            })
            .transpose()?;
        Ok(Self {
            limit: page.limit.into(),
            after,
            revision,
            session,
            fingerprint,
        })
    }

    pub fn finish<T>(
        self,
        mut rows: Vec<T>,
        key: impl FnOnce(&T) -> (String, String),
    ) -> Result<Page<T>> {
        let next_cursor = if rows.len() > self.limit {
            rows.truncate(self.limit);
            let (key, id) = key(rows.last().expect("validated nonzero page size"));
            let cursor = Cursor {
                key,
                id,
                revision: self.revision.clone(),
                session: self.session,
                fingerprint: self.fingerprint,
            };
            Some(URL_SAFE_NO_PAD.encode(serde_json::to_vec(&cursor).map_err(codec_error)?))
        } else {
            None
        };
        Ok(Page {
            rows,
            next_cursor,
            revision: self.revision,
        })
    }
}

pub struct Context<'tx> {
    pub tx: Transaction<'tx>,
    pub scope: &'tx Scope,
    pub today: NaiveDate,
    pub now_ms: i64,
    pub(crate) currency_conversion: bool,
}

impl<'tx> Context<'tx> {
    pub fn new(
        tx: impl crate::sql::SqlTransaction + 'tx,
        scope: &'tx Scope,
        clock: &impl Clock,
    ) -> Result<Self> {
        let tx = Transaction::new(tx);
        let zone: Option<String> =
            tx.query_row("SELECT timezone_override FROM ledger_meta", [], |r| {
                r.get(0)
            })?;
        let now = clock.now();
        let today = if let Some(zone) = zone {
            let tz = zone
                .parse::<chrono_tz::Tz>()
                .map_err(|_| integrity("invalid_stored_timezone"))?;
            now.with_timezone(&tz).date_naive()
        } else {
            clock.local_date(now)
        };
        Ok(Self {
            tx,
            scope,
            today,
            now_ms: now.timestamp_millis(),
            currency_conversion: true,
        })
    }

    pub fn system(tx: impl crate::sql::SqlTransaction + 'tx, scope: &'tx Scope) -> Result<Self> {
        Self::new(tx, scope, &SystemClock)
    }

    pub fn without_currency_conversion(mut self) -> Self {
        self.currency_conversion = false;
        self
    }

    pub fn mutate<T: ReceiptValue>(
        &self,
        command: &str,
        meta: &Mutation,
        input: &impl Serialize,
        affected: &[&str],
        work: impl FnOnce(&Self) -> Result<T>,
    ) -> Result<Write<T>> {
        if self.scope != &meta.scope {
            return Err(AppError::new(
                ErrorCode::StaleSession,
                "stale_ledger_session",
            ));
        }
        let request = Uuid::parse_str(&meta.request_id)
            .map_err(|_| invalid("requestId", "invalid_id"))?
            .to_string();
        let hash = digest(&(meta.expected_revision.as_ref(), input))?;
        if let Some((old_command, old_hash, result)) = self
            .tx
            .query_row(
                "SELECT command,input_hash,result_json FROM operation_receipts WHERE request_id=?1",
                [&request],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?
        {
            if command != old_command || hash != old_hash {
                return Err(conflict("request_id_reused"));
            }
            return receipts::replay(self, &result, &request);
        }
        let current = revision(&self.tx)?;
        if meta
            .expected_revision
            .as_ref()
            .is_some_and(|expected| expected != &current)
        {
            return Err(stale_revision());
        }
        let value = work(self)?;
        self.tx.execute(
            "UPDATE ledger_meta SET revision=revision+1 WHERE singleton=1",
            [],
        )?;
        let result = Write {
            value,
            revision: revision(&self.tx)?,
            affected: affected.iter().map(|x| (*x).to_owned()).collect(),
        };
        let json = receipts::encode(self, &result)?;
        self.tx.execute(
            "INSERT INTO operation_receipts VALUES (?1,?2,?3,?4,?5)",
            params![request, command, hash, json, self.now_ms],
        )?;
        Ok(result)
    }
}

pub fn revision(tx: &Transaction<'_>) -> Result<String> {
    Ok(
        tx.query_row("SELECT CAST(revision AS TEXT) FROM ledger_meta", [], |r| {
            r.get(0)
        })?,
    )
}

pub fn check_revision(ctx: &Context<'_>, version: &VersionedId) -> Result<()> {
    if revision(&ctx.tx)? != version.version {
        return Err(stale_revision());
    }
    Ok(())
}

pub fn check_version(actual: &str, target: &VersionedId) -> Result<()> {
    if actual != target.version {
        return Err(stale_revision());
    }
    Ok(())
}

pub fn stale_revision() -> AppError {
    AppError::new(ErrorCode::StaleRevision, "stale_ledger_revision")
}
pub fn invalid(field: &str, issue: &'static str) -> AppError {
    AppError::validation(field, issue)
}
pub fn conflict(issue: &'static str) -> AppError {
    AppError::new(ErrorCode::Conflict, issue)
}
pub fn integrity(issue: &'static str) -> AppError {
    AppError::new(ErrorCode::Integrity, issue)
}
pub fn missing(resource: &str) -> AppError {
    invalid(resource, "record_not_found")
}
pub fn new_id() -> String {
    Uuid::new_v4().to_string()
}
pub fn codec_error(_: serde_json::Error) -> AppError {
    integrity("invalid_ledger_payload")
}
pub fn digest(value: &impl Serialize) -> Result<String> {
    use std::fmt::Write;
    let mut hash = String::with_capacity(64);
    for byte in Sha256::digest(serde_json::to_vec(value).map_err(codec_error)?) {
        write!(hash, "{byte:02x}").expect("writing to string");
    }
    Ok(hash)
}
pub fn date_option(value: Option<&str>) -> Result<Option<NaiveDate>> {
    value.map(parse_date).transpose()
}
pub fn date_range(range: &DateRange) -> Result<(Option<String>, Option<String>)> {
    let from = date_option(range.from.as_deref())?;
    let to = date_option(range.to.as_deref())?;
    if from.zip(to).is_some_and(|(from, to)| from > to) {
        return Err(invalid("range", "invalid_date_range"));
    }
    Ok((range.from.clone(), range.to.clone()))
}
pub fn name(value: &str) -> Result<&str> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 256 || value.chars().any(char::is_control) {
        return Err(invalid("name", "invalid_name"));
    }
    Ok(value)
}
pub fn minor(value: &str) -> Result<i64> {
    value
        .parse::<i64>()
        .ok()
        .filter(|n| n.to_string() == value)
        .ok_or_else(|| invalid("minor", "invalid_minor_units"))
}
pub fn positive(value: &str) -> Result<i64> {
    let amount = minor(value)?;
    if !(1..=9_000_000_000_000).contains(&amount) {
        return Err(invalid("minor", "posting_amount_out_of_range"));
    }
    Ok(amount)
}
pub fn scale(tx: &Transaction<'_>, currency: &str) -> Result<u32> {
    tx.query_row(
        "SELECT scale FROM currencies WHERE code=?1",
        [currency],
        |r| r.get(0),
    )
    .optional()?
    .ok_or_else(|| invalid("currency", "unsupported_currency"))
}
pub fn money(currency: &str, minor: impl ToString) -> Money {
    Money {
        currency: currency.to_owned(),
        minor: minor.to_string(),
    }
}
pub fn utc_timestamp(ms: i64) -> Result<String> {
    chrono::DateTime::<Utc>::from_timestamp_millis(ms)
        .map(|dt| dt.to_rfc3339())
        .ok_or_else(|| integrity("invalid_stored_timestamp"))
}
