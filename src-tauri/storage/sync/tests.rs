use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use base64::{Engine, engine::general_purpose::STANDARD_NO_PAD};
use rusqlite::{params_from_iter, types::Value as Sql};
use serde_json::{Value, json};
use tempfile::TempDir;

use super::*;
use crate::{
    crypto::Credential,
    domain::Protection,
    storage::{CreateLedger, LedgerManager, OpenTarget, ledger::ManagedReplica},
};

const URL: &str = "libsql://phase-two-test.turso.io";
const TOKEN: &str = "fixture-auth-token-never-persist";

#[derive(Default)]
struct FixtureVault(Mutex<std::collections::HashMap<Uuid, Zeroizing<String>>>);
impl TursoSecretStore for FixtureVault {
    fn enroll(&self, instance: Uuid, token: &str) -> Result<()> {
        self.0
            .lock()
            .insert(instance, Zeroizing::new(token.to_owned()));
        Ok(())
    }
    fn retrieve(&self, instance: Uuid) -> Result<Zeroizing<String>> {
        self.0.lock().get(&instance).cloned().ok_or_else(corrupt)
    }
    fn revoke(&self, instance: Uuid) -> Result<()> {
        self.0.lock().remove(&instance);
        Ok(())
    }
}

#[derive(Default)]
struct WireState {
    offline: AtomicBool,
    lose_commit_response: AtomicBool,
    stop: AtomicBool,
    requests: AtomicUsize,
}

struct Wire {
    address: SocketAddr,
    state: Arc<WireState>,
    worker: Option<JoinHandle<()>>,
    _directory: TempDir,
}

impl Wire {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let database = Connection::open(directory.path().join("turso.db")).unwrap();
        let state = Arc::new(WireState::default());
        let worker_state = state.clone();
        let worker = thread::spawn(move || {
            for stream in listener.incoming() {
                if worker_state.stop.load(Ordering::Acquire) {
                    return;
                }
                let Ok(stream) = stream else { return };
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                worker_state.requests.fetch_add(1, Ordering::Relaxed);
                if let Err(error) = request(&database, &worker_state, stream) {
                    assert!(
                        matches!(
                            error.kind(),
                            std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
                        ),
                        "{error}"
                    );
                }
            }
        });
        Self {
            address,
            state,
            worker: Some(worker),
            _directory: directory,
        }
    }
    fn connect(&self, url: &str, token: &SyncToken) -> Result<Turso> {
        assert_eq!(
            turso::endpoint(url)?.as_str(),
            "https://phase-two-test.turso.io/"
        );
        Ok(Turso::connect_loopback(self.address, token))
    }
}
impl Drop for Wire {
    fn drop(&mut self) {
        self.state.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(self.address);
        self.worker.take().unwrap().join().unwrap();
    }
}

fn request(db: &Connection, control: &WireState, stream: TcpStream) -> std::io::Result<()> {
    let mut stream = BufReader::new(stream);
    let mut line = String::new();
    stream.read_line(&mut line)?;
    assert_eq!(line.trim(), "POST /v3/pipeline HTTP/1.1");
    let mut len = 0;
    let mut auth = String::new();
    loop {
        line.clear();
        stream.read_line(&mut line)?;
        if line == "\r\n" || line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                len = value.trim().parse().unwrap();
            } else if name.eq_ignore_ascii_case("authorization") {
                auth = value.trim().to_owned();
            }
        }
    }
    let mut body = vec![0; len];
    stream.read_exact(&mut body)?;
    let output = stream.get_mut();
    if control.offline.load(Ordering::Acquire) {
        return respond(output, "503 Service Unavailable", "{}");
    }
    if auth != format!("Bearer {TOKEN}") {
        return respond(output, "401 Unauthorized", "{}");
    }
    let input: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(input["requests"][0]["type"], "execute");
    assert_eq!(input["requests"][1]["type"], "close");
    let stmt = &input["requests"][0]["stmt"];
    let sql = stmt["sql"].as_str().unwrap();
    let result = match execute(db, sql, stmt["args"].as_array().unwrap()) {
        Ok(result) => json!({"type":"ok","response":{"type":"execute","result":result}}),
        Err(error) => {
            json!({"type":"error","error":{"code":"SQLITE_ERROR","message":error.to_string()}})
        }
    };
    if sql.starts_with("INSERT INTO fgps_sync_commits_v1")
        && control.lose_commit_response.swap(false, Ordering::AcqRel)
    {
        return respond(output, "503 Service Unavailable", "{}");
    }
    let response = json!({"baton":null,"base_url":null,"results":[result,{"type":"ok","response":{"type":"close"}}]});
    respond(output, "200 OK", &response.to_string())
}

fn execute(db: &Connection, sql: &str, args: &[Value]) -> rusqlite::Result<Value> {
    let parameters: Vec<Sql> = args
        .iter()
        .map(|value| match value["type"].as_str().unwrap() {
            "integer" => Sql::Integer(value["value"].as_str().unwrap().parse().unwrap()),
            "text" => Sql::Text(value["value"].as_str().unwrap().to_owned()),
            "blob" => Sql::Blob(
                STANDARD_NO_PAD
                    .decode(value["base64"].as_str().unwrap())
                    .unwrap(),
            ),
            _ => panic!("unsupported fixture parameter"),
        })
        .collect();
    let mut stmt = db.prepare(sql)?;
    let cols = stmt.column_count();
    if cols == 0 {
        let changes = stmt.execute(params_from_iter(parameters))?;
        return Ok(json!({"rows":[],"affected_row_count":changes}));
    }
    let mut cursor = stmt.query(params_from_iter(parameters))?;
    let mut rows = Vec::new();
    while let Some(row) = cursor.next()? {
        rows.push(
            (0..cols)
                .map(|index| match row.get::<_, Sql>(index)? {
                    Sql::Integer(value) => Ok(json!({"type":"integer","value":value.to_string()})),
                    Sql::Text(value) => Ok(json!({"type":"text","value":value})),
                    Sql::Blob(value) => {
                        Ok(json!({"type":"blob","base64":STANDARD_NO_PAD.encode(value)}))
                    }
                    Sql::Null => Ok(json!({"type":"null"})),
                    Sql::Real(value) => Ok(json!({"type":"float","value":value})),
                })
                .collect::<rusqlite::Result<Vec<_>>>()?,
        );
    }
    Ok(json!({"rows":rows,"affected_row_count":0}))
}

fn respond(stream: &mut TcpStream, status: &str, body: &str) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn token(value: &str) -> SyncToken {
    SyncToken::new(Zeroizing::new(value.to_owned())).unwrap()
}
fn mode(value: &str) -> SetStorageMode {
    SetStorageMode::TursoSync {
        url: URL.into(),
        auth_token: token(value),
    }
}
fn create(root: &Path, encrypted: bool) -> (LedgerManager, Scope) {
    let manager = LedgerManager::new(root).unwrap();
    let session = manager
        .create(
            "main",
            CreateLedger {
                name: "Sync ledger".into(),
                destination: None,
                base_currency: "USD".into(),
                timezone: None,
                protection: if encrypted {
                    Protection::Encrypted
                } else {
                    Protection::Plain
                },
                credential: encrypted.then(|| {
                    serde_json::from_value::<Credential>(json!("test sync passphrase")).unwrap()
                }),
                remember_key: false,
            },
        )
        .unwrap();
    (manager, session.scope)
}
fn replica<'a>(manager: &'a LedgerManager, scope: &'a Scope) -> ManagedReplica<'a> {
    ManagedReplica {
        manager,
        scope,
        window: "main",
    }
}
fn value<T: rusqlite::types::FromSql>(manager: &LedgerManager, scope: &Scope, sql: &str) -> T {
    manager
        .read_domain("main", scope, |tx| {
            tx.query_row(sql, [], |row| row.get(0)).map_err(sql_error)
        })
        .unwrap()
}
fn write(manager: &LedgerManager, scope: &Scope, sql: &str) -> Result<()> {
    manager.write_domain("main", scope, |tx| tx.execute_batch(sql).map_err(sql_error))
}

#[test]
fn turso_mode_validates_origin_and_auth_before_enrolling_then_revokes_on_disable() {
    for input in [
        "http://x.turso.io",
        "https://turso.io",
        "https://x.turso.io.evil.org",
        "http://localhost",
        "https://x.turso.io:443",
        "https://x.turso.io/@evil.org",
        "https://evil.org@x.turso.io",
        "libsql://x.turso.io?authToken=oops",
        "https://%78.turso.io",
        "https://-x.turso.io",
    ] {
        assert_eq!(
            turso::endpoint(input).unwrap_err().code,
            ErrorCode::Validation,
            "{input}"
        );
    }
    assert_eq!(
        turso::endpoint("libsql://X.turso.io/").unwrap().as_str(),
        "https://x.turso.io/"
    );
    let dir = tempfile::tempdir().unwrap();
    let wire = Wire::start();
    let vault = FixtureVault::default();
    let controller = SyncController::default();
    let (db, scope) = create(dir.path(), false);
    let id = Uuid::parse_str(&scope.ledger_instance_id).unwrap();
    let replica = replica(&db, &scope);
    assert!(
        dir.path()
            .join("ledgers")
            .join(format!("{id}.db"))
            .is_file()
    );
    let status = controller
        .run_using::<Turso>(&scope, &replica, &vault, None, |_, _| {
            panic!("isolated mode connected")
        })
        .unwrap();
    assert_eq!(status.mode, LedgerStorageMode::LocalIsolated);
    assert_eq!(wire.state.requests.load(Ordering::Relaxed), 0);
    let portable = controller
        .set_using::<Turso>(&scope, false, &replica, &vault, mode(TOKEN), |_, _| {
            panic!("portable ledger connected")
        })
        .unwrap_err();
    assert_eq!(
        portable.fields.unwrap()["mode"],
        "turso_requires_managed_ledger"
    );
    let err = controller
        .set_using(
            &scope,
            true,
            &replica,
            &vault,
            mode("wrong-token"),
            |url, token| wire.connect(url, token),
        )
        .unwrap_err();
    assert_eq!(err.message_key, "turso_auth_rejected");
    assert!(vault.retrieve(id).is_err());
    assert_eq!(
        controller.status(&scope, &replica).unwrap().mode,
        LedgerStorageMode::LocalIsolated
    );
    let saved = controller
        .set_using(&scope, true, &replica, &vault, mode(TOKEN), |url, token| {
            wire.connect(url, token)
        })
        .unwrap();
    assert_eq!(saved.mode, LedgerStorageMode::TursoSync);
    assert_eq!(vault.retrieve(id).unwrap().as_str(), TOKEN);
    let settings: String = value(
        &db,
        &scope,
        "SELECT value_json FROM ledger_settings WHERE key='secret.sync.v1'",
    );
    assert!(!settings.contains(TOKEN));
    let count = wire.state.requests.load(Ordering::Relaxed);
    controller
        .set_using::<Turso>(
            &scope,
            true,
            &replica,
            &vault,
            SetStorageMode::LocalIsolated,
            |_, _| panic!("disable connected"),
        )
        .unwrap();
    assert!(vault.retrieve(id).is_err());
    assert_eq!(
        db.sync_now("main", &scope).unwrap().mode,
        LedgerStorageMode::LocalIsolated
    );
    assert_eq!(wire.state.requests.load(Ordering::Relaxed), count);
}

#[test]
fn offline_commits_survive_reopen_and_sync_across_plain_and_sqlcipher_with_conflict_detection() {
    let dir_a = tempfile::tempdir().unwrap();
    let dir_b = tempfile::tempdir().unwrap();
    let wire = Wire::start();
    let vault = FixtureVault::default();
    let controller = SyncController::default();
    let (a, scope) = create(dir_a.path(), false);
    let (b, b_scope) = create(dir_b.path(), true);
    let set = |db: &LedgerManager, scope: &Scope| {
        controller
            .set_using(
                scope,
                true,
                &replica(db, scope),
                &vault,
                mode(TOKEN),
                |url, token| wire.connect(url, token),
            )
            .unwrap()
    };
    let run = |db: &LedgerManager, scope: &Scope| {
        controller
            .run_using(scope, &replica(db, scope), &vault, None, |url, token| {
                wire.connect(url, token)
            })
            .unwrap()
    };
    set(&a, &scope);
    wire.state.offline.store(true, Ordering::Release);
    let prior = wire.state.requests.load(Ordering::Acquire);
    write(&a, &scope, "INSERT INTO accounts(id,name,kind,currency_code) VALUES('a','Café','checking','USD');
        INSERT INTO categories(id,name,kind) VALUES('food','Groceries','expense'),('other','Other','expense');
        INSERT INTO transactions(id,account_id,currency_code,amount_minor,direction,bank_date,effective_date,reporting_date_override,applied_mode,status,description,assembly_state,created_at_ms)
        VALUES('p','a','USD',100,'debit','2026-11-01','2026-11-02','2026-11-03','cash','posted','Café groceries',0,1);
        INSERT INTO transaction_splits VALUES('p',1,'food',40),('p',2,'other',60);
        UPDATE transactions SET assembly_state=1 WHERE id='p';
        INSERT INTO transaction_search(transaction_id,description) VALUES('p','cafe groceries');").unwrap();
    assert_eq!(wire.state.requests.load(Ordering::Acquire), prior);
    let failure = run(&a, &scope);
    assert_eq!(failure.phase, SyncPhase::Offline);
    assert!(failure.pending);
    let instance = Uuid::parse_str(&scope.ledger_instance_id).unwrap();
    a.close("main", &scope).unwrap();
    drop(a);
    let a = LedgerManager::new(dir_a.path()).unwrap();
    let session = a
        .open("main", Uuid::new_v4(), instance.to_string(), || {
            Ok(OpenTarget::Registered(instance))
        })
        .unwrap()
        .session
        .expect("plain managed ledger is ready");
    let scope = session.scope;
    assert!(a.sync_status("main", &scope).unwrap().pending);
    wire.state.offline.store(false, Ordering::Release);
    wire.state
        .lose_commit_response
        .store(true, Ordering::Release);
    assert_eq!(run(&a, &scope).phase, SyncPhase::Offline);
    // The server committed, but the first response was lost. Reconcile its durable receipt.
    assert!(!run(&a, &scope).pending);
    assert_eq!(run(&a, &scope).remote_generation.as_deref(), Some("1"));
    set(&b, &b_scope);
    assert_eq!(run(&b, &b_scope).phase, SyncPhase::Idle);
    assert_eq!(
        value::<String>(
            &b,
            &b_scope,
            "SELECT reporting_date FROM transactions WHERE id='p'"
        ),
        "2026-11-03"
    );
    assert_eq!(
        value::<String>(
            &b,
            &b_scope,
            "SELECT financial_normalize(name) FROM accounts WHERE id='a'"
        ),
        "cafe"
    );
    assert_eq!(
        value::<String>(
            &b,
            &b_scope,
            "SELECT transaction_id FROM transaction_search WHERE transaction_search MATCH 'groceries'"
        ),
        "p"
    );
    assert_eq!(value::<i64>(&b, &b_scope, "PRAGMA foreign_keys"), 1);
    assert!(
        write(
            &b,
            &b_scope,
            "UPDATE accounts SET card_limit_minor='not an integer' WHERE id='a'"
        )
        .is_err()
    );
    assert!(
        write(
            &b,
            &b_scope,
            "DELETE FROM transaction_splits WHERE transaction_id='p'"
        )
        .is_err()
    );
    assert!(
        write(
            &b,
            &b_scope,
            "UPDATE currencies SET scale=3 WHERE code='USD'"
        )
        .is_err()
    );
    assert!(!b.sync_status("main", &b_scope).unwrap().pending);
    write(
        &b,
        &b_scope,
        "UPDATE accounts SET name='Written on B' WHERE id='a'",
    )
    .unwrap();
    assert_eq!(run(&b, &b_scope).phase, SyncPhase::Idle);
    assert_eq!(run(&a, &scope).phase, SyncPhase::Idle);
    assert_eq!(
        value::<String>(&a, &scope, "SELECT name FROM accounts WHERE id='a'"),
        "Written on B"
    );
    write(
        &a,
        &scope,
        "UPDATE accounts SET name='Local A' WHERE id='a'",
    )
    .unwrap();
    write(
        &b,
        &b_scope,
        "UPDATE accounts SET name='Local B' WHERE id='a'",
    )
    .unwrap();
    assert_eq!(run(&a, &scope).phase, SyncPhase::Idle);
    let conflict = run(&b, &b_scope);
    assert_eq!(conflict.phase, SyncPhase::Error);
    assert_eq!(
        conflict.error.unwrap().message_key,
        "turso_concurrent_ledger_edits"
    );
    assert!(conflict.pending);
    assert_eq!(
        value::<String>(&a, &scope, "SELECT name FROM accounts WHERE id='a'"),
        "Local A"
    );
    assert_eq!(
        value::<String>(&b, &b_scope, "SELECT name FROM accounts WHERE id='a'"),
        "Local B"
    );
    let resolve = |generation| {
        controller
            .run_using(
                &b_scope,
                &replica(&b, &b_scope),
                &vault,
                Some(Resolution::Keep(generation)),
                |url, token| wire.connect(url, token),
            )
            .unwrap()
    };
    assert_eq!(resolve(1).phase, SyncPhase::Error);
    assert_eq!(resolve(3).remote_generation.as_deref(), Some("4"));
    assert_eq!(run(&a, &scope).phase, SyncPhase::Idle);
    assert_eq!(
        value::<String>(&a, &scope, "SELECT name FROM accounts WHERE id='a'"),
        "Local B"
    );

    write(
        &a,
        &scope,
        "UPDATE transactions SET description='Local discarded' WHERE id='p'",
    )
    .unwrap();
    write(
        &b,
        &b_scope,
        "UPDATE transactions SET description='Remote marché' WHERE id='p'",
    )
    .unwrap();
    assert_eq!(run(&b, &b_scope).remote_generation.as_deref(), Some("5"));
    let conflict = run(&a, &scope);
    let local_version = conflict.local_version.unwrap().parse::<i64>().unwrap();
    let accept = |generation, local_version| {
        controller
            .run_using(
                &scope,
                &replica(&a, &scope),
                &vault,
                Some(Resolution::Accept {
                    generation,
                    local_version,
                }),
                |url, token| wire.connect(url, token),
            )
            .unwrap()
    };
    assert_eq!(accept(4, local_version).phase, SyncPhase::Error);
    assert_eq!(accept(5, local_version - 1).phase, SyncPhase::Error);
    assert_eq!(
        value::<String>(
            &a,
            &scope,
            "SELECT description FROM transactions WHERE id='p'"
        ),
        "Local discarded"
    );
    let accepted = accept(5, local_version);
    assert_eq!(accepted.remote_generation.as_deref(), Some("5"));
    assert_eq!(accepted.pending_count, "0");
    assert!(accepted.last_success_at.is_some());
    assert_eq!(
        value::<String>(
            &a,
            &scope,
            "SELECT transaction_id FROM transaction_search WHERE transaction_search MATCH 'marche'"
        ),
        "p"
    );
}

#[test]
#[ignore = "requires a disposable loopback libSQL server with JWT auth"]
fn authenticated_libsql_server_round_trips_two_managed_ledgers() {
    let address: SocketAddr = std::env::var("FGPS_TEST_SQLD_ADDRESS")
        .unwrap()
        .parse()
        .unwrap();
    assert!(address.ip().is_loopback());
    let auth = token(&std::env::var("FGPS_TEST_SQLD_TOKEN").unwrap());
    let remote = Turso::connect_loopback(address, &auth);
    assert_eq!(
        Turso::connect_loopback(address, &token("invalid-jwt"))
            .probe()
            .unwrap_err()
            .message_key,
        "turso_auth_rejected"
    );
    remote.probe().unwrap();
    remote.initialize().unwrap();
    assert!(
        remote.head().unwrap().is_none(),
        "the server must be disposable and empty"
    );
    let directory = tempfile::tempdir().unwrap();
    let a_dir = directory.path().join("a");
    let b_dir = directory.path().join("b");
    let (a, a_scope) = create(&a_dir, false);
    let (b, b_scope) = create(&b_dir, false);
    let a_replica = replica(&a, &a_scope);
    let b_replica = replica(&b, &b_scope);
    let vault = FixtureVault::default();
    let controller = SyncController::default();

    let switch_to = || SetStorageMode::TursoSync {
        url: URL.into(),
        auth_token: token(auth.as_str()),
    };
    controller
        .set_using(
            &a_scope,
            true,
            &a_replica,
            &vault,
            switch_to(),
            |_, auth| Ok(Turso::connect_loopback(address, auth)),
        )
        .unwrap();
    write(
        &a,
        &a_scope,
        "INSERT INTO accounts(id,name,kind,currency_code) VALUES('one','One','checking','USD');",
    )
    .unwrap();
    synchronize(&a_replica, &remote, None).unwrap();
    controller
        .set_using(
            &b_scope,
            true,
            &b_replica,
            &vault,
            switch_to(),
            |_, auth| Ok(Turso::connect_loopback(address, auth)),
        )
        .unwrap();
    synchronize(&b_replica, &remote, None).unwrap();
    assert_eq!(
        value::<i64>(&b, &b_scope, "SELECT count(*) FROM accounts"),
        1
    );
    write(
        &b,
        &b_scope,
        "UPDATE accounts SET name='Synced from B' WHERE id='one';",
    )
    .unwrap();
    synchronize(&b_replica, &remote, None).unwrap();
    synchronize(&a_replica, &remote, None).unwrap();
    assert_eq!(
        value::<String>(&a, &a_scope, "SELECT name FROM accounts"),
        "Synced from B"
    );
}
