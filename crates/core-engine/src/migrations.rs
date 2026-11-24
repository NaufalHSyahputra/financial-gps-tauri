use crate::{
    domain::{AppError, ErrorCode, Result},
    sql::{Transaction, params},
};

pub const INITIAL_VERSION: u32 = 1;
pub const UPGRADES: &[Upgrade] = &[
    Upgrade {
        sql: include_str!("../migrations/0002_cash_accounts.sql"),
        irreversible: false,
    },
    Upgrade {
        sql: include_str!("../migrations/0003_category_icons.sql"),
        irreversible: false,
    },
];
pub const SCHEMA_VERSION: u32 = INITIAL_VERSION + UPGRADES.len() as u32;
pub const INITIAL_SQL: &str = include_str!("../migrations/0001_init.sql");

#[derive(Clone, Copy)]
pub struct Upgrade {
    pub sql: &'static str,
    pub irreversible: bool,
}

pub struct LedgerMetadata<'a> {
    pub uuid: &'a str,
    pub name: &'a str,
    pub timezone: Option<&'a str>,
    pub currency: &'a str,
    pub created_at_ms: i64,
}

pub fn checksum(sql: &str) -> String {
    crate::domain::sha256(sql.as_bytes())
}

fn unsupported() -> AppError {
    AppError::new(ErrorCode::UnsupportedSchema, "unsupported_ledger_schema")
}

pub fn upgrades_to(version: u32) -> Result<&'static [Upgrade]> {
    let count = version
        .checked_sub(INITIAL_VERSION)
        .ok_or_else(unsupported)?;
    UPGRADES.get(..count as usize).ok_or_else(unsupported)
}

pub fn current_version(conn: &Transaction<'_>) -> Result<u32> {
    Ok(conn.query_row("PRAGMA user_version", [], |row| row.get(0))?)
}

fn verify_history(conn: &Transaction<'_>, version: u32, upgrades: &[Upgrade]) -> Result<()> {
    let mut query = conn
        .prepare("SELECT version, sha256 FROM schema_migrations ORDER BY version")
        .map_err(|_| AppError::new(ErrorCode::Integrity, "migration_history_invalid"))?;
    let mut rows = query.query([])?;
    let initial = std::iter::once(INITIAL_SQL);
    let applied = upgrades
        .iter()
        .take((version - INITIAL_VERSION) as usize)
        .map(|m| m.sql);
    for (index, sql) in initial.chain(applied).enumerate() {
        let Some(row) = rows.next()? else {
            return Err(AppError::new(
                ErrorCode::Integrity,
                "migration_checksum_mismatch",
            ));
        };
        let recorded = row
            .get_ref(1)?
            .as_str()
            .map_err(|_| AppError::new(ErrorCode::Integrity, "unexpected_database_value"))?;
        if row.get::<_, u32>(0)? != INITIAL_VERSION + index as u32 || recorded != checksum(sql) {
            return Err(AppError::new(
                ErrorCode::Integrity,
                "migration_checksum_mismatch",
            ));
        }
    }
    if rows.next()?.is_some() {
        return Err(AppError::new(
            ErrorCode::Integrity,
            "migration_checksum_mismatch",
        ));
    }
    Ok(())
}

pub fn verify_supported(conn: &Transaction<'_>) -> Result<u32> {
    let (version, _) = pending(conn, UPGRADES)?;
    Ok(version)
}

pub fn verify(conn: &Transaction<'_>) -> Result<()> {
    let version = current_version(conn)?;
    if version != SCHEMA_VERSION {
        return Err(unsupported());
    }
    verify_history(conn, version, UPGRADES)
}

pub fn initialize(tx: &Transaction<'_>, meta: LedgerMetadata<'_>) -> Result<()> {
    tx.execute_batch(
        "CREATE TABLE schema_migrations (
            version INTEGER PRIMARY KEY CHECK(version>0),
            sha256 TEXT NOT NULL CHECK(length(sha256)=64),
            applied_at_ms INTEGER NOT NULL
        ) STRICT;",
    )?;
    tx.execute_batch(INITIAL_SQL)?;
    let currency_exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM currencies WHERE code=?1)",
        [meta.currency],
        |row| row.get(0),
    )?;
    if !currency_exists {
        return Err(AppError::validation("baseCurrency", "unsupported_currency"));
    }
    tx.execute(
        "INSERT INTO ledger_meta(singleton, ledger_uuid, name, timezone_override, display_currency, created_at_ms)
         VALUES(1, ?1, ?2, ?3, ?4, ?5)",
        params![meta.uuid, meta.name, meta.timezone, meta.currency, meta.created_at_ms],
    )?;
    tx.execute(
        "INSERT INTO schema_migrations VALUES (?1, ?2, ?3)",
        params![INITIAL_VERSION, checksum(INITIAL_SQL), meta.created_at_ms],
    )?;
    tx.execute_batch(&format!("PRAGMA user_version = {INITIAL_VERSION}"))?;
    verify_history(tx, INITIAL_VERSION, UPGRADES)?;
    validate(tx)
}

pub fn quick_check(conn: &Transaction<'_>) -> Result<()> {
    let quick: String = conn.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    if quick != "ok" {
        return Err(AppError::new(
            ErrorCode::Integrity,
            "ledger_integrity_check_failed",
        ));
    }
    Ok(())
}

pub fn validate(conn: &Transaction<'_>) -> Result<()> {
    quick_check(conn)?;
    if conn
        .prepare("PRAGMA foreign_key_check")?
        .query([])?
        .next()?
        .is_some()
    {
        return Err(AppError::new(
            ErrorCode::Integrity,
            "ledger_integrity_check_failed",
        ));
    }
    Ok(())
}

/// Validates the installed history and determines which migration suffix remains.
pub fn pending<'a>(
    conn: &Transaction<'_>,
    upgrades: &'a [Upgrade],
) -> Result<(u32, &'a [Upgrade])> {
    let version = current_version(conn)?;
    let latest = INITIAL_VERSION + upgrades.len() as u32;
    if !(INITIAL_VERSION..=latest).contains(&version) {
        return Err(unsupported());
    }
    verify_history(conn, version, upgrades)?;
    Ok((version, &upgrades[(version - INITIAL_VERSION) as usize..]))
}

/// Applies migrations in the caller's write transaction, after any required safety copy.
pub fn apply(tx: &Transaction<'_>, upgrades: &[Upgrade], applied_at_ms: i64) -> Result<()> {
    let (version, remaining) = pending(tx, upgrades)?;
    for (offset, upgrade) in remaining.iter().enumerate() {
        let next = version + 1 + offset as u32;
        tx.execute_batch(upgrade.sql)?;
        tx.execute(
            "INSERT INTO schema_migrations VALUES (?1, ?2, ?3)",
            params![next, checksum(upgrade.sql), applied_at_ms],
        )?;
        tx.execute_batch(&format!("PRAGMA user_version = {next}"))?;
    }
    verify_history(tx, INITIAL_VERSION + upgrades.len() as u32, upgrades)?;
    validate(tx)
}
