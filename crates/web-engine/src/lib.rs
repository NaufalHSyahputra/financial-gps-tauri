use core_engine::{
    domain::{AppError, Result, normalize},
    migrations::{self, LedgerMetadata},
    sql::{SqlTransaction, Transaction},
};
use serde::Deserialize;
use wasm_bindgen::prelude::*;

mod operations;
mod sqlite;

#[wasm_bindgen]
pub fn financial_normalize(value: &str) -> String {
    normalize(value)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Metadata {
    uuid: String,
    name: String,
    timezone: Option<String>,
    currency: String,
    created_at_ms: i64,
}

#[wasm_bindgen]
pub fn initialize_ledger(
    db: sqlite::JsDatabase,
    metadata: &str,
) -> std::result::Result<(), JsValue> {
    let work = || -> Result<()> {
        let metadata: Metadata = serde_json::from_str(metadata).map_err(invalid_request)?;
        migrations::initialize(
            &Transaction::new(sqlite::Session(&db)),
            LedgerMetadata {
                uuid: &metadata.uuid,
                name: &metadata.name,
                timezone: metadata.timezone.as_deref(),
                currency: &metadata.currency,
                created_at_ms: metadata.created_at_ms,
            },
        )
    };
    work().map_err(error_value)
}

#[wasm_bindgen]
pub fn verify_ledger(db: sqlite::JsDatabase) -> std::result::Result<(), JsValue> {
    let tx = Transaction::new(sqlite::Session(&db));
    migrations::verify_supported(&tx)
        .and_then(|_| migrations::validate(&tx))
        .map_err(error_value)
}

#[wasm_bindgen]
pub fn ledger_schema_version() -> u32 {
    migrations::SCHEMA_VERSION
}

#[wasm_bindgen]
pub fn upgrade_ledger(
    db: sqlite::JsDatabase,
    applied_at_ms: f64,
    target_version: Option<u32>,
) -> std::result::Result<bool, JsValue> {
    let session = sqlite::Session(&db);
    let tx = Transaction::new(&session);
    let work = || -> Result<bool> {
        let upgrades =
            migrations::upgrades_to(target_version.unwrap_or(migrations::SCHEMA_VERSION))?;
        let (_, pending) = migrations::pending(&tx, upgrades)?;
        if pending.is_empty() {
            return Ok(false);
        }
        session.execute_batch("PRAGMA foreign_keys=OFF; PRAGMA legacy_alter_table=ON;")?;
        let result = (|| -> Result<()> {
            session.execute_batch("BEGIN IMMEDIATE")?;
            migrations::apply(&tx, upgrades, applied_at_ms as i64)?;
            session.execute_batch("COMMIT")?;
            Ok(())
        })();
        if result.is_err() {
            let _ = session.execute_batch("ROLLBACK");
        }
        let restore =
            session.execute_batch("PRAGMA legacy_alter_table=OFF; PRAGMA foreign_keys=ON;");
        result?;
        restore?;
        Ok(true)
    };
    work().map_err(error_value)
}

pub(crate) fn invalid_request(_: serde_json::Error) -> AppError {
    AppError::validation("input", "invalid_operation_arguments")
}

pub(crate) fn error_value(error: AppError) -> JsValue {
    JsValue::from_str(&serde_json::to_string(&error).expect("serializable application error"))
}

#[wasm_bindgen]
pub fn derive_key(passphrase: &str, salt: &[u8]) -> std::result::Result<Vec<u8>, JsValue> {
    core_engine::archive::derive(passphrase, salt)
        .map(|key| key.to_vec())
        .map_err(error_value)
}

#[wasm_bindgen]
pub fn encrypt_archive(
    payload: &[u8],
    credential: &str,
    entropy: &[u8],
) -> std::result::Result<Vec<u8>, JsValue> {
    let entropy = entropy
        .try_into()
        .map_err(|_| error_value(AppError::validation("entropy", "invalid_entropy")))?;
    let mut result = Vec::new();
    core_engine::archive::encrypt(payload, &mut result, credential, entropy, &|| false)
        .map_err(error_value)?;
    Ok(result)
}

#[wasm_bindgen]
pub fn decrypt_archive(archive: &[u8], credential: &str) -> std::result::Result<Vec<u8>, JsValue> {
    let mut result = Vec::new();
    core_engine::archive::decrypt(archive, &mut result, credential, &|| false)
        .map_err(error_value)?;
    Ok(result)
}
