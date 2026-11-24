use serde::{Deserialize, Serialize};
use specta::Type;
use std::collections::BTreeMap;
use uuid::Uuid;

pub type Result<T> = std::result::Result<T, AppError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum ErrorCode {
    Validation,
    Locked,
    StaleSession,
    StaleRevision,
    Offline,
    Conflict,
    Duplicate,
    BadCredentialOrCorrupt,
    Busy,
    Io,
    Integrity,
    InsufficientSpace,
    Cancelled,
    UnsupportedSchema,
}

#[derive(Debug, Clone, Serialize, Type, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[error("{code:?} ({correlation_id})")]
pub struct AppError {
    pub code: ErrorCode,
    pub message_key: &'static str,
    pub fields: Option<BTreeMap<String, String>>,
    pub retryable: bool,
    pub correlation_id: String,
}

impl AppError {
    pub fn new(code: ErrorCode, message_key: &'static str) -> Self {
        Self {
            code,
            message_key,
            fields: None,
            retryable: matches!(
                code,
                ErrorCode::BadCredentialOrCorrupt
                    | ErrorCode::Busy
                    | ErrorCode::Io
                    | ErrorCode::InsufficientSpace
                    | ErrorCode::StaleSession
                    | ErrorCode::StaleRevision
                    | ErrorCode::Offline
            ),
            correlation_id: Uuid::new_v4().to_string(),
        }
    }

    pub fn validation(field: &str, issue: &'static str) -> Self {
        Self {
            fields: Some(BTreeMap::from([(field.to_owned(), issue.to_owned())])),
            ..Self::new(ErrorCode::Validation, "invalid_input")
        }
    }

    pub fn busy() -> Self {
        Self::new(ErrorCode::Busy, "ledger_busy")
    }
}
