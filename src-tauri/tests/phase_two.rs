use financial_gps::{
    crypto::{Credential, DatabaseKey, QuickUnlockStore},
    domain::{
        AppError, CurrencyCode, CurrencyScale, ErrorCode, LedgerSession, LifecycleState, Money,
        OpenResult, Protection, Result, parse_date,
    },
    storage::{CreateLedger, LedgerManager, OpenTarget, PathGrants, PathPurpose},
};
use rusqlite::{Connection, Transaction, params};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};
use tempfile::TempDir;
use uuid::Uuid;

const MAIN: &str = "main";

fn create_input(name: &str) -> CreateLedger {
    CreateLedger {
        name: name.into(),
        destination: None,
        base_currency: "IDR".into(),
        timezone: Some("Asia/Jakarta".into()),
        protection: Protection::Plain,
        credential: None,
        remember_key: false,
    }
}

fn credential(value: &str) -> Credential {
    serde_json::from_value(serde_json::json!(value)).unwrap()
}

fn registered(manager: &LedgerManager, session: &LedgerSession) -> OpenResult {
    let instance = Uuid::parse_str(&session.scope.ledger_instance_id).unwrap();
    manager
        .open(MAIN, Uuid::new_v4(), instance.to_string(), || {
            Ok(OpenTarget::Registered(instance))
        })
        .unwrap()
}

fn managed_db(root: &Path, session: &LedgerSession) -> PathBuf {
    root.join("ledgers")
        .join(format!("{}.db", session.scope.ledger_instance_id))
}

fn fixture() -> (TempDir, LedgerManager, LedgerSession) {
    let root = tempfile::tempdir().unwrap();
    let manager = LedgerManager::new(root.path()).unwrap();
    let session = manager.create(MAIN, create_input("Fixture")).unwrap();
    (root, manager, session)
}

fn accounts(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute_batch(
        "INSERT INTO accounts(id,name,kind,currency_code) VALUES
         ('a','Checking','checking','IDR'), ('b','Savings','savings','IDR'), ('c','International','checking','USD');
         INSERT INTO categories(id,name,kind) VALUES ('one','Food','expense'), ('two','Other','expense');",
    )
}

fn assemble(
    tx: &Transaction<'_>,
    id: &str,
    account: &str,
    currency: &str,
    direction: &str,
    amount: i64,
) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO transactions (id,account_id,currency_code,amount_minor,direction,bank_date,effective_date,
          applied_mode,status,description,assembly_state,created_at_ms)
         VALUES (?1,?2,?3,?4,?5,'2026-01-01','2026-01-02','cash','posted','Fixture',0,0)",
        params![id, account, currency, amount, direction],
    )?;
    Ok(())
}

fn seal(tx: &Transaction<'_>, id: &str) -> rusqlite::Result<()> {
    tx.execute("UPDATE transactions SET assembly_state=1 WHERE id=?1", [id])?;
    Ok(())
}

#[test]
fn migration_strict_currency_scales_and_connection_policy() {
    let (_root, manager, session) = fixture();
    let pragmas = [
        ("foreign_keys", 1),
        ("synchronous", 2),
        ("busy_timeout", 5000),
        ("temp_store", 2),
        ("trusted_schema", 0),
        (
            "user_version",
            financial_gps::storage::SCHEMA_VERSION.into(),
        ),
    ];
    let verify = |tx: &Transaction<'_>, reader: bool| -> rusqlite::Result<()> {
        for (name, expected) in pragmas {
            assert_eq!(
                tx.pragma_query_value(None, name, |row| row.get::<_, i64>(0))?,
                expected,
                "{name}"
            );
        }
        assert_eq!(
            tx.pragma_query_value(None, "query_only", |row| row.get::<_, bool>(0))?,
            reader
        );
        assert_eq!(
            tx.pragma_query_value(None, "journal_mode", |row| row.get::<_, String>(0))?,
            "wal"
        );
        assert!(
            !tx.pragma_query_value(None, "cipher_version", |row| row.get::<_, String>(0))?
                .is_empty()
        );
        Ok(())
    };
    manager
        .read(MAIN, &session.scope, |tx| {
            verify(tx, true)?;
            let currencies = tx
                .prepare("SELECT code,scale FROM currencies ORDER BY code")?
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })?
                .collect::<rusqlite::Result<HashMap<_, _>>>()?;
            assert_eq!(
                currencies,
                HashMap::from([
                    ("AUD".into(), 2),
                    ("BHD".into(), 3),
                    ("CNY".into(), 2),
                    ("EUR".into(), 2),
                    ("GBP".into(), 2),
                    ("IDR".into(), 0),
                    ("JPY".into(), 0),
                    ("KRW".into(), 0),
                    ("KWD".into(), 3),
                    ("OMR".into(), 3),
                    ("SGD".into(), 2),
                    ("USD".into(), 2),
                ])
            );
            assert!(tx.execute("DELETE FROM currencies", []).is_err());
            let untyped_tables: i64 = tx.query_row(
                "SELECT count(*) FROM pragma_table_list WHERE schema='main' AND type='table'
              AND name NOT LIKE 'sqlite_%' AND strict=0",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(untyped_tables, 0);
            Ok(())
        })
        .unwrap();
    manager
        .write(MAIN, &session.scope, |tx| {
            verify(tx, false)?;
            assert!(
                tx.execute("INSERT INTO currencies VALUES ('ZZZ','two','Invalid')", [])
                    .is_err()
            );
            assert!(
                tx.execute("UPDATE currencies SET scale=2 WHERE code='IDR'", [])
                    .is_err()
            );
            tx.execute("UPDATE currencies SET scale=scale WHERE code='IDR'", [])?;
            Ok(())
        })
        .unwrap();

    let invalid = manager.create(
        MAIN,
        CreateLedger {
            base_currency: "ZZZ".into(),
            ..create_input("Invalid")
        },
    );
    assert_eq!(invalid.unwrap_err().code, ErrorCode::Validation);
    assert_eq!(manager.list().unwrap().len(), 1);
}

#[test]
fn migration_checksum_and_schema_version_are_validated_on_open() {
    let (root, manager, session) = fixture();
    manager.close(MAIN, &session.scope).unwrap();
    let db = Connection::open(managed_db(root.path(), &session)).unwrap();
    db.execute("UPDATE schema_migrations SET sha256=?1", ["f".repeat(64)])
        .unwrap();
    let id = Uuid::parse_str(&session.scope.ledger_instance_id).unwrap();
    let open = || {
        manager.open(MAIN, Uuid::new_v4(), id.to_string(), || {
            Ok(OpenTarget::Registered(id))
        })
    };
    assert_eq!(
        open().unwrap_err().message_key,
        "migration_checksum_mismatch"
    );
    db.pragma_update(None, "user_version", 42).unwrap();
    assert_eq!(open().unwrap_err().code, ErrorCode::UnsupportedSchema);
}

#[test]
fn split_assembly_requires_positive_matching_lines_and_a_sealed_commit() {
    let (_root, manager, session) = fixture();
    manager.write(MAIN, &session.scope, accounts).unwrap();
    let unfinished = manager.write(MAIN, &session.scope, |tx| {
        assemble(tx, "unfinished", "a", "IDR", "debit", 100)
    });
    assert_eq!(unfinished.unwrap_err().code, ErrorCode::Integrity);
    let qty = manager
        .read(MAIN, &session.scope, |tx| {
            tx.query_row("SELECT count(*) FROM transactions", [], |row| {
                row.get::<_, i64>(0)
            })
        })
        .unwrap();
    assert_eq!(qty, 0);
    for values in [&[100][..], &[50, 49], &[50, 51]] {
        let rejected = manager.write(MAIN, &session.scope, |tx| {
            assemble(tx, "split", "a", "IDR", "debit", 100)?;
            for (index, amount) in values.iter().enumerate() {
                tx.execute(
                    "INSERT INTO transaction_splits VALUES ('split',?1,'one',?2)",
                    params![index as i64 + 1, amount],
                )?;
            }
            seal(tx, "split")
        });
        assert_eq!(rejected.unwrap_err().code, ErrorCode::Integrity);
    }
    manager
        .write(MAIN, &session.scope, |tx| {
            assemble(tx, "split", "a", "IDR", "debit", 100)?;
            assert!(
                tx.execute(
                    "INSERT INTO transaction_splits VALUES ('split',1,'one',0)",
                    []
                )
                .is_err()
            );
            tx.execute_batch(
                "INSERT INTO transaction_splits VALUES ('split',1,'one',60), ('split',2,'two',40)",
            )?;
            tx.execute(
                "UPDATE transactions SET category_id='one' WHERE id='split'",
                [],
            )?;
            assert!(seal(tx, "split").is_err());
            tx.execute(
                "UPDATE transactions SET category_id=NULL WHERE id='split'",
                [],
            )?;
            seal(tx, "split")?;
            assert!(
                tx.execute(
                    "DELETE FROM transaction_splits WHERE transaction_id='split' AND position=1",
                    []
                )
                .is_err()
            );
            assert!(
                tx.execute(
                    "UPDATE transaction_splits SET amount_minor=60 WHERE position=2",
                    []
                )
                .is_err()
            );
            assert!(
                tx.execute(
                    "UPDATE transactions SET amount_minor=1 WHERE id='split'",
                    []
                )
                .is_err()
            );
            Ok(())
        })
        .unwrap();
    manager
        .write(MAIN, &session.scope, |tx| {
            tx.execute_batch(
                "UPDATE transactions SET assembly_state=0 WHERE id='split';
           DELETE FROM transaction_splits WHERE transaction_id='split';",
            )?;
            seal(tx, "split")
        })
        .unwrap();
}

#[test]
fn transfer_link_protects_leg_structure_and_reports_dates_independently() {
    let (_root, manager, session) = fixture();
    manager.write(MAIN, &session.scope, |tx| {
        accounts(tx)?;
        assemble(tx, "debit", "a", "IDR", "debit", 100)?;
        assemble(tx, "credit", "b", "IDR", "credit", 100)?;
        assemble(tx, "foreign", "c", "USD", "credit", 110)?;
        assemble(tx, "mismatch", "b", "IDR", "credit", 99)?;
        let link = "INSERT INTO transfer_links(id,debit_id,credit_id,created_at_ms) VALUES('link','debit',?1,0)";
        assert!(tx.execute(link, ["credit"]).is_err());
        for id in ["debit", "credit", "foreign", "mismatch"] {
            seal(tx, id)?;
        }
        assert!(tx.execute(link, ["mismatch"]).is_err());
        assert!(tx.execute(link, ["foreign"]).is_err());
        tx.execute(link, ["credit"])?;
        for sql in [
            "UPDATE transactions SET amount_minor=110 WHERE id='credit'",
            "UPDATE transactions SET assembly_state=0 WHERE id='debit'",
            "UPDATE transactions SET category_id='one' WHERE id='debit'",
            "UPDATE transactions SET ignored=1 WHERE id='credit'",
            "DELETE FROM transactions WHERE id='credit'",
            "UPDATE transfer_links SET created_at_ms=1 WHERE id='link'",
        ] {
            assert!(tx.execute(sql, []).is_err(), "{sql}");
        }
        for (sql, expected) in [
            ("SELECT 1", "2026-01-01"),
            ("UPDATE transactions SET applied_mode='accrual' WHERE id='debit'", "2026-01-02"),
            ("UPDATE transactions SET effective_bill_date='2026-01-03' WHERE id='debit'", "2026-01-03"),
            ("UPDATE transactions SET reporting_date_override='2026-01-04' WHERE id='debit'", "2026-01-04"),
            ("UPDATE transactions SET applied_mode='cash' WHERE id='debit'", "2026-01-04"),
            ("UPDATE transactions SET reporting_date_override=NULL WHERE id='debit'", "2026-01-03"),
        ] {
            tx.execute_batch(sql)?;
            let dates = tx.query_row("SELECT bank_date, reporting_date FROM transactions WHERE id='debit'", [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
            assert_eq!(dates, ("2026-01-01".to_owned(), expected.to_owned()));
        }
        assert!(tx.execute("UPDATE transactions SET reporting_date='2026-01-05' WHERE id='debit'", []).is_err());
        tx.execute_batch(
            "DELETE FROM transfer_links WHERE id='link';
             INSERT INTO transfer_links(id,debit_id,credit_id,applied_rate_decimal,rate_date,rate_source,created_at_ms)
                VALUES ('fx','debit','foreign','0.011','2026-01-01','manual',0);",
        )?;
        Ok(())
    }).unwrap();
}

#[test]
fn fts_joins_by_uuid_after_vacuum() {
    let (root, manager, session) = fixture();
    let id = Uuid::new_v4().to_string();
    manager.write(MAIN, &session.scope, |tx| {
        accounts(tx)?;
        assemble(tx, &id, "a", "IDR", "debit", 100)?;
        seal(tx, &id)?;
        tx.execute("INSERT INTO transaction_search(rowid,transaction_id,description) VALUES(987,?1,'Kopi Jakarta')", [&id])?;
        Ok(())
    }).unwrap();
    manager.close(MAIN, &session.scope).unwrap();
    Connection::open(managed_db(root.path(), &session))
        .unwrap()
        .execute_batch("VACUUM")
        .unwrap();
    let session = registered(&manager, &session).session.unwrap();
    let result =
        manager
            .read(MAIN, &session.scope, |tx| {
                tx.query_row(
        "SELECT t.id FROM transaction_search s JOIN transactions t ON t.id=s.transaction_id
         WHERE transaction_search MATCH 'jakarta'", [], |row| row.get::<_, String>(0),
    )
            })
            .unwrap();
    assert_eq!(result, id);
}

#[derive(Default)]
struct TestKeychain(Mutex<HashMap<Uuid, DatabaseKey>>);

impl QuickUnlockStore for TestKeychain {
    fn remember(&self, instance: Uuid, key: &DatabaseKey) -> Result<()> {
        self.0.lock().unwrap().insert(instance, key.clone());
        Ok(())
    }

    fn retrieve(&self, instance: Uuid) -> Result<DatabaseKey> {
        self.0
            .lock()
            .unwrap()
            .get(&instance)
            .cloned()
            .ok_or_else(|| AppError::new(ErrorCode::Io, "keychain_unavailable"))
    }

    fn revoke(&self, instance: Uuid) -> Result<()> {
        self.0.lock().unwrap().remove(&instance);
        Ok(())
    }
}

#[test]
fn encrypted_ledger_rejects_wrong_passphrase_and_rotates_scopes_on_unlock() {
    let root = tempfile::tempdir().unwrap();
    let keychain = Arc::new(TestKeychain::default());
    let manager = LedgerManager::with_keychain(root.path(), keychain.clone()).unwrap();
    let first = manager
        .create(
            MAIN,
            CreateLedger {
                protection: Protection::Encrypted,
                credential: Some(credential("correct horse")),
                remember_key: true,
                ..create_input("Encrypted ledger")
            },
        )
        .unwrap();
    let db_path = managed_db(root.path(), &first);
    manager.write(MAIN, &first.scope, accounts).unwrap();
    let wal = fs::read(db_path.with_extension("db-wal")).unwrap();
    assert!(wal.len() > 32);
    assert!(
        !wal.windows(b"Checking".len())
            .any(|bytes| bytes == b"Checking")
    );
    assert_eq!(
        manager.lock(MAIN, &first.scope).unwrap(),
        LifecycleState::Locked
    );
    let header = fs::read(&db_path).unwrap();
    assert_ne!(&header[..16], b"SQLite format 3\0");
    let wrong_key = Connection::open(&db_path).unwrap();
    assert!(
        wrong_key
            .query_row("SELECT name FROM ledger_meta", [], |row| row
                .get::<_, String>(0))
            .is_err()
    );
    drop(wrong_key);
    let sidecar = fs::read(db_path.with_extension("db.keys")).unwrap();
    assert!(
        !sidecar
            .windows(b"correct horse".len())
            .any(|bytes| bytes == b"correct horse")
    );
    let challenge = registered(&manager, &first).unlock_challenge.unwrap();
    assert_eq!(
        manager
            .unlock("another", &challenge.id, None)
            .unwrap_err()
            .code,
        ErrorCode::StaleSession
    );
    let error = manager
        .unlock(MAIN, &challenge.id, Some(credential("incorrect")))
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::BadCredentialOrCorrupt);
    assert_eq!(manager.state().unwrap(), LifecycleState::Locked);
    let second = manager
        .unlock(MAIN, &challenge.id, Some(credential("correct horse")))
        .unwrap();
    assert_ne!(first.scope.session_id, second.scope.session_id);
    assert_eq!(
        first.scope.ledger_instance_id,
        second.scope.ledger_instance_id
    );
    assert_eq!(
        manager
            .read(MAIN, &first.scope, |_| Ok(()))
            .unwrap_err()
            .code,
        ErrorCode::StaleSession
    );
    assert_eq!(
        manager
            .read("another", &second.scope, |_| Ok(()))
            .unwrap_err()
            .code,
        ErrorCode::StaleSession
    );
    let rows = manager
        .read(MAIN, &second.scope, |tx| {
            tx.query_row("SELECT count(*) FROM accounts", [], |row| {
                row.get::<_, i64>(0)
            })
        })
        .unwrap();
    assert_eq!(rows, 3);
    manager.close(MAIN, &second.scope).unwrap();
    drop(manager);
    let manager = LedgerManager::with_keychain(root.path(), keychain.clone()).unwrap();
    let challenge = registered(&manager, &first).unlock_challenge.unwrap();
    let third = manager.unlock(MAIN, &challenge.id, None).unwrap();
    assert_ne!(third.scope.session_id, second.scope.session_id);
    manager.lock(MAIN, &third.scope).unwrap();
    keychain
        .revoke(Uuid::parse_str(&first.scope.ledger_instance_id).unwrap())
        .unwrap();
    let challenge = registered(&manager, &first).unlock_challenge.unwrap();
    assert_eq!(
        manager
            .unlock(MAIN, &challenge.id, None)
            .unwrap_err()
            .message_key,
        "keychain_unavailable"
    );
    manager
        .unlock(MAIN, &challenge.id, Some(credential("correct horse")))
        .unwrap();
}

#[test]
fn grants_are_single_use_window_and_purpose_bound_and_aliases_are_rejected() {
    let (root, manager, first) = fixture();
    let grants = PathGrants::default();
    let portable_path = root.path().join("portable.db");
    let grant = grants
        .issue(MAIN, PathPurpose::LedgerCreate, &portable_path)
        .unwrap();
    assert_eq!(grant.display_name, "portable.db");
    assert!(
        grants
            .consume("another", PathPurpose::LedgerCreate, &grant.id)
            .is_err()
    );
    assert!(
        grants
            .consume(MAIN, PathPurpose::LedgerOpen, &grant.id)
            .is_err()
    );
    let destination = grants
        .consume(MAIN, PathPurpose::LedgerCreate, &grant.id)
        .unwrap();
    assert!(
        grants
            .consume(MAIN, PathPurpose::LedgerCreate, &grant.id)
            .is_err()
    );
    let second = manager
        .create(
            MAIN,
            CreateLedger {
                destination: Some(destination),
                ..create_input("Portable")
            },
        )
        .unwrap();
    assert!(portable_path.is_file());
    assert_ne!(
        first.scope.ledger_instance_id,
        second.scope.ledger_instance_id
    );
    assert_eq!(
        manager
            .read(MAIN, &first.scope, |_| Ok(()))
            .unwrap_err()
            .code,
        ErrorCode::StaleSession
    );

    let alias = root.path().join("alias.db");
    fs::hard_link(&portable_path, &alias).unwrap();
    let grant = grants.issue(MAIN, PathPurpose::LedgerOpen, &alias).unwrap();
    let approved = grants
        .consume(MAIN, PathPurpose::LedgerOpen, &grant.id)
        .unwrap();
    let open = manager.open(MAIN, Uuid::new_v4(), grant.id, || {
        Ok(OpenTarget::Chosen(approved))
    });
    assert_eq!(open.unwrap_err().code, ErrorCode::Duplicate);
    assert_eq!(manager.session(MAIN).unwrap().unwrap(), second);

    #[cfg(unix)]
    {
        let symlink = root.path().join("symlink.db");
        std::os::unix::fs::symlink(&portable_path, &symlink).unwrap();
        assert!(
            grants
                .issue(MAIN, PathPurpose::LedgerOpen, &symlink)
                .is_err()
        );
    }

    let other_root = tempfile::tempdir().unwrap();
    let other = LedgerManager::new(other_root.path()).unwrap();
    let grant = grants
        .issue(MAIN, PathPurpose::LedgerOpen, &portable_path)
        .unwrap();
    let approved = grants
        .consume(MAIN, PathPurpose::LedgerOpen, &grant.id)
        .unwrap();
    let open = other.open(MAIN, Uuid::new_v4(), grant.id, || {
        Ok(OpenTarget::Chosen(approved))
    });
    assert_eq!(open.unwrap_err().code, ErrorCode::Busy);

    manager.close(MAIN, &second.scope).unwrap();
    let grant = grants
        .issue(MAIN, PathPurpose::LedgerOpen, &portable_path)
        .unwrap();
    let imported = other
        .open(MAIN, Uuid::new_v4(), grant.id.clone(), || {
            Ok(OpenTarget::Chosen(
                grants
                    .consume(MAIN, PathPurpose::LedgerOpen, &grant.id)
                    .unwrap(),
            ))
        })
        .unwrap()
        .session
        .unwrap();
    assert_eq!(imported.ledger_uuid, second.ledger_uuid);
    assert_ne!(
        imported.scope.ledger_instance_id,
        second.scope.ledger_instance_id
    );
}

#[test]
fn bounded_pool_and_lock_drain_keep_commit_bound_to_original_ledger() {
    let (_root, manager, original) = fixture();
    let manager = Arc::new(manager);
    let (started, wait_started) = mpsc::channel();
    let (release, wait_release) = mpsc::channel();
    let reader = |gate: Arc<Mutex<mpsc::Receiver<()>>>| {
        let manager = manager.clone();
        let scope = original.scope.clone();
        let started = started.clone();
        thread::spawn(move || {
            manager.read(MAIN, &scope, |tx| {
                let _ = tx.query_row("SELECT count(*) FROM accounts", [], |row| {
                    row.get::<_, i64>(0)
                })?;
                started.send(()).unwrap();
                gate.lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(3))
                    .unwrap();
                Ok(())
            })
        })
    };
    let gate = Arc::new(Mutex::new(wait_release));
    let read_a = reader(gate.clone());
    let read_b = reader(gate);
    wait_started.recv_timeout(Duration::from_secs(3)).unwrap();
    wait_started.recv_timeout(Duration::from_secs(3)).unwrap();
    assert_eq!(
        manager
            .read(MAIN, &original.scope, |_| Ok(()))
            .unwrap_err()
            .code,
        ErrorCode::Busy
    );
    release.send(()).unwrap();
    release.send(()).unwrap();
    read_a.join().unwrap().unwrap();
    read_b.join().unwrap().unwrap();

    let (staged, wait_staged) = mpsc::channel();
    let (commit, wait_commit) = mpsc::channel();
    let writer = {
        let manager = manager.clone();
        let scope = original.scope.clone();
        thread::spawn(move || {
            manager.write(MAIN, &scope, |tx| {
                accounts(tx)?;
                staged.send(()).unwrap();
                wait_commit.recv_timeout(Duration::from_secs(5)).unwrap();
                Ok(())
            })
        })
    };
    wait_staged.recv_timeout(Duration::from_secs(3)).unwrap();
    assert_eq!(
        manager
            .write(MAIN, &original.scope, |_| Ok(()))
            .unwrap_err()
            .code,
        ErrorCode::Busy
    );
    let (switch_started, wait_switch) = mpsc::channel();
    let switch = {
        let manager = manager.clone();
        thread::spawn(move || {
            switch_started.send(()).unwrap();
            manager.create(MAIN, create_input("Switched")).unwrap()
        })
    };
    wait_switch.recv_timeout(Duration::from_secs(3)).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while manager.state().is_ok() {
        assert!(
            std::time::Instant::now() < deadline,
            "switch did not begin draining"
        );
        thread::yield_now();
    }
    assert_eq!(
        manager
            .read(MAIN, &original.scope, |_| Ok(()))
            .unwrap_err()
            .code,
        ErrorCode::Busy
    );
    commit.send(()).unwrap();
    writer.join().unwrap().unwrap();
    let next = switch.join().unwrap();
    let count = |session: &LedgerSession| {
        manager
            .read(MAIN, &session.scope, |tx| {
                tx.query_row("SELECT count(*) FROM accounts", [], |row| {
                    row.get::<_, i64>(0)
                })
            })
            .unwrap()
    };
    assert_eq!(count(&next), 0);
    assert_eq!(count(&registered(&manager, &original).session.unwrap()), 3);
}

#[test]
fn exact_money_and_gregorian_dates() {
    for (code, scale, amount, expected) in [
        ("IDR", CurrencyScale::Zero, "15000", 15000),
        ("USD", CurrencyScale::Two, "-42.17", -4217),
        ("BHD", CurrencyScale::Three, "4.125", 4125),
        ("USD", CurrencyScale::Two, "92233720368547758.07", i64::MAX),
    ] {
        let money = Money::from_major(CurrencyCode::parse(code).unwrap(), scale, amount).unwrap();
        assert_eq!(money.minor, expected);
        assert_eq!(money.to_major(scale).to_string(), amount);
    }
    let usd = || CurrencyCode::parse("USD").unwrap();
    assert!(Money::from_major(usd(), CurrencyScale::Two, "0.001").is_err());
    assert!(Money::from_major(usd(), CurrencyScale::Two, "92233720368547758.08").is_err());
    assert!(Money::from_major(usd(), CurrencyScale::Two, "1e2").is_err());
    assert!(parse_date("2024-02-29").is_ok());
    assert!(parse_date("2025-02-29").is_err());
    assert!(parse_date("2025-2-8").is_err());
}

#[test]
fn sqlite_full_rolls_back_prior_rows_and_keeps_ledger_usable() {
    let (_root, manager, session) = fixture();
    let scope = &session.scope;
    manager.write_domain(MAIN, scope, |tx| {
        tx.execute_batch("CREATE TABLE injected_storage_exhaustion(id INTEGER PRIMARY KEY, payload BLOB) STRICT;").map_err(financial_gps::storage::sql_error)?;
        Ok(())
    }).unwrap();
    let failure = manager
        .write_domain(MAIN, scope, |tx| {
            let pages: i64 = tx
                .pragma_query_value(None, "page_count", |row| row.get(0))
                .map_err(financial_gps::storage::sql_error)?;
            tx.pragma_update(None, "max_page_count", pages)
                .map_err(financial_gps::storage::sql_error)?;
            tx.execute(
                "UPDATE ledger_meta SET name='must roll back' WHERE singleton=1",
                [],
            )
            .map_err(financial_gps::storage::sql_error)?;
            tx.execute(
                "INSERT INTO injected_storage_exhaustion(payload) VALUES (zeroblob(8388608))",
                [],
            )
            .map_err(financial_gps::storage::sql_error)?;
            Ok(())
        })
        .unwrap_err();
    assert_eq!(failure.code, ErrorCode::InsufficientSpace);
    manager
        .read_domain(MAIN, scope, |tx| {
            let name: String = tx
                .query_row("SELECT name FROM ledger_meta", [], |row| row.get(0))
                .map_err(financial_gps::storage::sql_error)?;
            assert_eq!(name, session.name);
            let rows: i64 = tx
                .query_row(
                    "SELECT count(*) FROM injected_storage_exhaustion",
                    [],
                    |row| row.get(0),
                )
                .map_err(financial_gps::storage::sql_error)?;
            assert_eq!(rows, 0);
            Ok(())
        })
        .unwrap();
    manager
        .write_domain(MAIN, scope, |tx| {
            tx.pragma_update(None, "max_page_count", 1_000_000)
                .map_err(financial_gps::storage::sql_error)?;
            tx.execute(
                "UPDATE ledger_meta SET name='after retry' WHERE singleton=1",
                [],
            )
            .map_err(financial_gps::storage::sql_error)?;
            Ok(())
        })
        .unwrap();
}
