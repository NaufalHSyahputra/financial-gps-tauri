use crate::domain::{AppError, ErrorCode, Result};
use rusqlite::{Connection, TransactionBehavior};

use std::{
    fs::{self, File},
    io,
    path::Path,
};

use super::{
    database::sql_error,
    files::{self, io_error},
    sqlite_view,
};

use core_engine::migrations;

pub use core_engine::migrations::{LedgerMetadata, SCHEMA_VERSION, UPGRADES, Upgrade};

pub fn verify(conn: &Connection) -> Result<()> {
    migrations::verify(&sqlite_view(conn))
}

pub fn validate(conn: &Connection) -> Result<()> {
    migrations::validate(&sqlite_view(conn))
}

pub(super) fn quick_check(conn: &Connection) -> Result<()> {
    migrations::quick_check(&sqlite_view(conn))
}

pub fn initialize(conn: &mut Connection, meta: LedgerMetadata<'_>) -> Result<()> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sql_error)?;
    migrations::initialize(&sqlite_view(&tx), meta)?;
    tx.commit().map_err(sql_error)
}

pub(super) fn apply_upgrades(
    conn: &mut Connection,
    path: &Path,
    upgrades: &[Upgrade],
) -> Result<bool> {
    let (version, pending) = migrations::pending(&sqlite_view(conn), upgrades)?;
    if pending.is_empty() {
        return Ok(false);
    }
    let has_backup = pending.iter().any(|upgrade| upgrade.irreversible);
    if has_backup {
        safety_copy(conn, path, version)?;
    }
    upgrade_transaction(conn, upgrades).map_err(|error| {
        if has_backup {
            AppError::new(error.code, "migration_failed_safety_copy_available")
        } else {
            AppError::new(error.code, "migration_rolled_back")
        }
    })?;
    Ok(true)
}

pub(super) fn upgrade_memory(conn: &mut Connection) -> Result<()> {
    upgrade_memory_to(conn, SCHEMA_VERSION)
}

pub(super) fn upgrade_memory_to(conn: &mut Connection, target: u32) -> Result<()> {
    let upgrades = migrations::upgrades_to(target)?;
    let (_, pending) = migrations::pending(&sqlite_view(conn), upgrades)?;
    if !pending.is_empty() {
        upgrade_transaction(conn, upgrades)?;
    }
    Ok(())
}

fn upgrade_transaction(conn: &mut Connection, upgrades: &[Upgrade]) -> Result<()> {
    conn.execute_batch("PRAGMA foreign_keys=OFF; PRAGMA legacy_alter_table=ON;")
        .map_err(sql_error)?;
    let result = (|| -> Result<()> {
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        migrations::apply(
            &sqlite_view(&tx),
            upgrades,
            chrono::Utc::now().timestamp_millis(),
        )?;
        tx.commit().map_err(sql_error)
    })();
    let restore = conn
        .execute_batch("PRAGMA legacy_alter_table=OFF; PRAGMA foreign_keys=ON;")
        .map_err(sql_error);
    result.and(restore)
}

fn safety_copy(conn: &mut Connection, path: &Path, version: u32) -> Result<()> {
    // Use the same SQLCipher pages and wrapper, with neither a new key nor a plaintext intermediate.
    // Holding SQLite's exclusive pager lock prevents an external SQLite writer racing the checkpoint and copy.
    conn.pragma_update(None, "locking_mode", "EXCLUSIVE")
        .map_err(sql_error)?;
    conn.transaction_with_behavior(TransactionBehavior::Exclusive)
        .map_err(sql_error)?
        .commit()
        .map_err(sql_error)?;
    let remaining: (i64, i64, i64) = conn
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(sql_error)?;
    if remaining != (0, 0, 0) {
        return Err(AppError::busy());
    }
    let directory = path
        .parent()
        .ok_or_else(|| AppError::new(ErrorCode::Io, "unsafe_ledger_path"))?;
    let staging = tempfile::Builder::new()
        .prefix(".fgps-migration-staging-")
        .tempdir_in(directory)
        .map_err(io_error)?;
    let copy = |source: &Path, destination: &Path| -> Result<()> {
        let mut input = File::open(source).map_err(io_error)?;
        let mut output = tempfile::NamedTempFile::new_in(staging.path()).map_err(io_error)?;
        io::copy(&mut input, &mut output).map_err(io_error)?;
        output.as_file().sync_all().map_err(io_error)?;
        output
            .persist_noclobber(destination)
            .map_err(|error| io_error(error.error))?;
        Ok(())
    };
    let backup_db = staging.path().join("ledger.db");
    copy(path, &backup_db)?;
    if files::has_wrapper(path)? {
        copy(
            &files::sibling(path, ".keys"),
            &files::sibling(&backup_db, ".keys"),
        )?;
    }
    #[cfg(unix)]
    File::open(staging.path())
        .and_then(|file| file.sync_all())
        .map_err(io_error)?;
    let published = directory.join(format!(
        ".fgps-pre-migration-v{version}-{}",
        uuid::Uuid::new_v4()
    ));
    fs::rename(staging.path(), &published).map_err(io_error)?;
    #[cfg(unix)]
    File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(io_error)?;
    Ok(())
}

#[cfg(test)]
mod cash_tests {
    use super::*;

    #[test]
    fn category_icon_migration_and_encrypted_archives_preserve_rows() {
        let password: crate::crypto::Credential =
            serde_json::from_str(r#""category icon archive password""#).unwrap();
        for version in 1..=SCHEMA_VERSION {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("ledger.db");
            let mut db = Connection::open(&path).unwrap();
            initialize(
                &mut db,
                LedgerMetadata {
                    uuid: "00000000-0000-4000-8000-000000000001",
                    name: "Category icons",
                    timezone: None,
                    currency: "IDR",
                    created_at_ms: 0,
                },
            )
            .unwrap();
            upgrade_memory_to(&mut db, version).unwrap();
            db.execute(
                "INSERT INTO categories(id,name,kind) VALUES('food','Groceries','expense')",
                [],
            )
            .unwrap();
            let expected = (version == SCHEMA_VERSION).then_some("shopping-cart".to_owned());
            if let Some(icon) = &expected {
                db.execute("UPDATE categories SET icon=?1 WHERE id='food'", [icon])
                    .unwrap();
            }
            let bytes = db.serialize("main").unwrap().to_vec();
            let mut archive = Vec::new();
            let manifest =
                super::super::snapshot::write(&bytes, &[], &mut archive, &password, &|| false)
                    .unwrap();
            assert_eq!(manifest.schema_version, version);
            let restored =
                super::super::snapshot::open(archive.as_slice(), &password, &|| false).unwrap();
            assert_eq!(
                apply_upgrades(&mut db, &path, UPGRADES).unwrap(),
                version < SCHEMA_VERSION
            );
            for connection in [&db, &restored.database] {
                verify(connection).unwrap();
                let category = connection
                    .query_row(
                        "SELECT name,icon FROM categories WHERE id='food'",
                        [],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
                    )
                    .unwrap();
                assert_eq!(category, ("Groceries".to_owned(), expected.clone()));
            }
        }
    }

    #[test]
    fn version_one_files_and_archives_keep_links_and_accept_cash() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("version-one.db");
        let mut db = Connection::open(&path).unwrap();
        initialize(
            &mut db,
            LedgerMetadata {
                uuid: "00000000-0000-4000-8000-000000000001",
                name: "Version one",
                timezone: None,
                currency: "IDR",
                created_at_ms: 0,
            },
        )
        .unwrap();
        db.execute_batch("PRAGMA foreign_keys=ON; BEGIN;
            INSERT INTO accounts(rowid,id,name,kind,currency_code) VALUES(42,'original','Old Bank','checking','IDR');
            INSERT INTO account_aliases(id,account_id,label) VALUES('alias','original','Old card');
            INSERT INTO transactions(id,account_id,currency_code,amount_minor,direction,bank_date,effective_date,
                applied_mode,status,description,assembly_state,created_at_ms)
            VALUES('txn','original','IDR',500,'debit','2026-01-01','2026-01-01','cash','posted','Old grocery',0,1);
            UPDATE transactions SET assembly_state=1 WHERE id='txn';
            INSERT INTO transaction_search(transaction_id,description) VALUES('txn','old grocery'); COMMIT;").unwrap();
        let bytes = db.serialize("main").unwrap().to_vec();
        let password: crate::crypto::Credential =
            serde_json::from_str("\"archive fixture password\"").unwrap();
        let mut archive = Vec::new();
        let manifest =
            super::super::snapshot::write(&bytes, &[], &mut archive, &password, &|| false).unwrap();
        assert_eq!(manifest.schema_version, 1);
        let restored =
            super::super::snapshot::open(archive.as_slice(), &password, &|| false).unwrap();
        assert!(apply_upgrades(&mut db, &path, UPGRADES).unwrap());
        for connection in [&mut db, &mut { restored.database }] {
            let scope = crate::domain::Scope {
                ledger_instance_id: "test".into(),
                session_id: "ready".into(),
            };
            let ctx = crate::ledger_engine::Context::system(
                super::super::SqliteSession(connection),
                &scope,
            )
            .unwrap();
            let mut draft = crate::ledger_engine::accounts::get(&ctx, "original", None)
                .unwrap()
                .account;
            draft.kind = crate::ledger_engine::accounts::AccountKind::Cash;
            draft.name = "Cash Wallet".into();
            let cash = crate::ledger_engine::accounts::save(&ctx, None, &draft).unwrap();
            assert_eq!(
                crate::ledger_engine::accounts::get(&ctx, &cash.id, None)
                    .unwrap()
                    .account
                    .kind,
                draft.kind
            );
            let linked: i64 = connection.query_row(
                "SELECT a.rowid FROM accounts a JOIN account_aliases b ON b.account_id=a.id
                 JOIN transactions t ON t.account_id=a.id JOIN transaction_search f ON f.transaction_id=t.id
                 WHERE b.id='alias' AND transaction_search MATCH 'grocery' AND t.amount_minor=500",
                [], |row| row.get(0),
            ).unwrap();
            assert_eq!(linked, 42);
            let enforcement: bool = connection
                .pragma_query_value(None, "foreign_keys", |row| row.get(0))
                .unwrap();
            assert!(enforcement);
            verify(connection).unwrap();
        }
        drop(db);
        let reopened = Connection::open(&path).unwrap();
        let kind: String = reopened
            .query_row(
                "SELECT kind FROM accounts WHERE name='Cash Wallet'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(kind, "cash");
    }
}
