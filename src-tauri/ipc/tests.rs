use super::*;
use serde_json::{Value, json};
use tauri::{
    ipc::{CallbackFn, InvokeBody},
    test::{INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder},
    webview::InvokeRequest,
};

fn invoke(
    window: &WebviewWindow<MockRuntime>,
    origin: &str,
    command: &str,
    body: Value,
) -> std::result::Result<Value, Value> {
    get_ipc_response(
        window,
        InvokeRequest {
            cmd: command.to_owned(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: origin.parse().unwrap(),
            body: InvokeBody::Json(body),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_owned(),
        },
    )
    .map(|response| response.deserialize().unwrap())
}

#[test]
fn native_session_refresh_waits_for_saturated_readers() {
    use std::{sync::mpsc, thread, time::Duration};

    let root = tempfile::tempdir().unwrap();
    let manager = Arc::new(LedgerManager::new(root.path()).unwrap());
    let session = manager
        .create(
            "main",
            CreateLedger {
                name: "Personal activity".into(),
                destination: None,
                base_currency: "IDR".into(),
                timezone: None,
                protection: Protection::Plain,
                credential: None,
                remember_key: false,
            },
        )
        .unwrap();
    let (started, wait_started) = mpsc::channel();
    let mut readers = Vec::new();
    for _ in 0..2 {
        let manager = Arc::clone(&manager);
        let scope = session.scope.clone();
        let started = started.clone();
        let (release, wait_release) = mpsc::channel();
        readers.push((
            release,
            thread::spawn(move || {
                manager
                    .read_domain("main", &scope, |_| {
                        started.send(()).unwrap();
                        wait_release.recv_timeout(Duration::from_secs(3)).unwrap();
                        Ok(())
                    })
                    .unwrap();
            }),
        ));
        wait_started.recv_timeout(Duration::from_secs(3)).unwrap();
    }
    let (busy, wait_busy) = mpsc::channel();
    let release_readers = thread::spawn(move || {
        let first_attempt = wait_busy.recv_timeout(Duration::from_secs(3));
        if first_attempt.is_ok() {
            thread::sleep(Duration::from_millis(650));
        }
        for (release, _) in &readers {
            release.send(()).unwrap();
        }
        for (_, reader) in readers {
            reader.join().unwrap();
        }
        first_attempt.unwrap();
    });

    let result = tauri::async_runtime::block_on(phase_three::blocking_read(move || {
        let result = manager.session("main");
        if result
            .as_ref()
            .is_err_and(|error| error.code == ErrorCode::Busy)
        {
            let _ = busy.send(());
        }
        result
    }));
    release_readers.join().unwrap();
    assert_eq!(result.unwrap().unwrap().scope, session.scope);
}

#[test]
fn bindings_match_command_contract() {
    let temp = tempfile::tempdir().unwrap();
    let generated = temp.path().join("bindings.ts");
    export_bindings(&generated).unwrap();
    let checked_in = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../src/lib/drivers/native/bindings.ts");
    assert_eq!(
        std::fs::read_to_string(generated)
            .unwrap()
            .replace("\r\n", "\n"),
        std::fs::read_to_string(checked_in)
            .unwrap()
            .replace("\r\n", "\n"),
        "Run cargo run --features binding-generator --bin generate-bindings to update the IPC bindings",
    );
}

#[cfg(unix)]
#[test]
fn android_app_data_alias_opens_managed_storage_but_file_grants_still_reject_it() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    let app_data = data.join("app.financialgps.desktop");
    std::fs::create_dir_all(&app_data).unwrap();
    let alias = root.path().join("user-zero");
    std::os::unix::fs::symlink(&data, &alias).unwrap();
    let reported = alias.join("app.financialgps.desktop");

    let path = resolve_android_app_data(&reported).unwrap();
    assert_eq!(path, app_data.canonicalize().unwrap());
    let manager = LedgerManager::new(&path).unwrap();
    assert!(manager.list().unwrap().is_empty());
    let session = manager
        .create(
            "main",
            CreateLedger {
                name: "Personal activity".into(),
                destination: None,
                base_currency: "IDR".into(),
                timezone: None,
                protection: Protection::Plain,
                credential: None,
                remember_key: false,
            },
        )
        .unwrap();
    manager.close("main", &session.scope).unwrap();
    let file = format!("{}.db", session.scope.ledger_instance_id);
    let grants = PathGrants::default();
    assert_eq!(
        grants
            .issue(
                "main",
                PathPurpose::LedgerOpen,
                &reported.join("ledgers").join(&file)
            )
            .err()
            .expect("user-selected paths must reject symlinked parents")
            .fields
            .unwrap()["destination"],
        "unsafe_ledger_path"
    );
    grants
        .issue(
            "main",
            PathPurpose::LedgerOpen,
            &path.join("ledgers").join(file),
        )
        .unwrap();
}

#[test]
fn launcher_commands_round_trip_through_tauri_and_enforce_acl() {
    let temp = tempfile::tempdir().unwrap();
    let command_builder = commands!(MockRuntime);
    let app = mock_builder()
        .invoke_handler(command_builder.invoke_handler())
        .manage(Arc::new(Desktop {
            ledgers: LedgerManager::new(temp.path()),
            grants: PathGrants::default(),
            previews: Default::default(),
            jobs: Default::default(),
            phase_four: Default::default(),
            maintenance_wakeup: Default::default(),
            sync_wakeup: Default::default(),
        }))
        .build(tauri::generate_context!())
        .unwrap();
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let origin = if cfg!(windows) {
        "http://tauri.localhost"
    } else {
        "tauri://localhost"
    };
    let invoke_local = |command: &str, body: Value| invoke(&window, origin, command, body);

    let bootstrap = invoke_local("app_bootstrap", json!({})).unwrap();
    assert!(bootstrap["session"].is_null());
    assert_eq!(
        bootstrap["schemaRange"]["maximum"],
        crate::storage::SCHEMA_VERSION
    );
    assert_eq!(invoke_local("ledger_list", json!({})).unwrap(), json!([]));
    assert!(invoke(&window, "https://example.invalid", "ledger_list", json!({})).is_err());
    assert!(invoke_local("plugin:dialog|open", json!({})).is_err());
    assert!(
        invoke_local(
            "ledger_create",
            json!({"input": {
                "name": "Untrusted path", "baseCurrency": "USD", "protection": "plain",
                "rememberKey": false, "path": "/arbitrary.db",
            }})
        )
        .is_err()
    );

    let session = invoke_local(
        "ledger_create",
        json!({"input": {
            "name": "Personal", "baseCurrency": "IDR", "protection": "encrypted",
            "credential": "manual unlock test phrase", "rememberKey": false,
        }}),
    )
    .unwrap();
    let scope = &session["scope"];
    assert_eq!(session["revision"], "0");
    let local = invoke_local("ledger_sync_status", json!({ "scope": scope })).unwrap();
    assert_eq!(local["mode"], "local-isolated");
    assert_eq!(local["phase"], "idle");
    assert_eq!(local["pending"], false);
    assert_eq!(
        invoke_local(
            "ledger_storage_mode_set",
            json!({"scope":scope,"input":{"mode":"local-isolated"}})
        )
        .unwrap(),
        local
    );
    assert_eq!(
        invoke_local("ledger_sync_now", json!({ "scope": scope })).unwrap(),
        local
    );
    let invalid = invoke_local(
        "turso_credentials_validate",
        json!({"input":{"url":"https://test.turso.io.evil.org","authToken":"fixture-token"}}),
    )
    .unwrap_err();
    assert_eq!(invalid["code"], "Validation");
    assert!(
        invoke(
            &window,
            "https://example.invalid",
            "ledger_sync_status",
            json!({"scope":scope})
        )
        .is_err()
    );

    assert_eq!(
        invoke_local("app_bootstrap", json!({})).unwrap()["session"],
        session
    );
    let descriptor = invoke_local("ledger_list", json!({})).unwrap()[0].clone();
    assert_eq!(descriptor["label"], "Personal");
    assert!(descriptor.get("path").is_none());
    assert_eq!(
        invoke_local("ledger_lock", json!({"scope": scope})).unwrap(),
        "locked"
    );
    let open = invoke_local(
        "ledger_open",
        json!({"input": {
            "ledgerInstanceId": scope["ledgerInstanceId"], "requestId": Uuid::new_v4(),
        }}),
    )
    .unwrap();
    assert_eq!(open["state"], "locked");
    let challenge = &open["unlockChallenge"]["id"];
    let unlocked = invoke_local(
        "ledger_unlock",
        json!({"input": {
            "unlockChallenge": challenge, "credential": "manual unlock test phrase",
        }}),
    )
    .unwrap();
    assert_ne!(unlocked["scope"], *scope);
    let stale = invoke_local("ledger_close", json!({"scope": scope})).unwrap_err();
    assert_eq!(stale["code"], "StaleSession");
    assert_eq!(
        invoke_local("ledger_close", json!({"scope": unlocked["scope"]})).unwrap(),
        "unselected"
    );
}

#[test]
fn phase_three_commands_enforce_scope_and_round_trip_financial_strings() {
    let temp = tempfile::tempdir().unwrap();
    let desktop = Arc::new(Desktop {
        ledgers: LedgerManager::new(temp.path()),
        grants: PathGrants::default(),
        previews: Default::default(),
        jobs: Default::default(),
        phase_four: Default::default(),
        maintenance_wakeup: Default::default(),
        sync_wakeup: Default::default(),
    });
    let command_builder = commands!(MockRuntime);
    let app = mock_builder()
        .invoke_handler(command_builder.invoke_handler())
        .manage(desktop.clone())
        .build(tauri::generate_context!())
        .unwrap();
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let origin = if cfg!(windows) {
        "http://tauri.localhost"
    } else {
        "tauri://localhost"
    };
    let call = |command: &str, args: Value| invoke(&window, origin, command, args);
    let create = |name: &str| {
        call(
            "ledger_create",
            json!({"input":{
                "name":name,"baseCurrency":"USD","protection":"plain","rememberKey":false,
            }}),
        )
        .unwrap()
    };
    let first = create("First ledger");
    let scope = first["scope"].clone();
    let mutation = |revision: &str| json!({"scope":scope,"requestId":Uuid::new_v4(),"expectedRevision":revision});
    let args = json!({"mutation":mutation("0"),"input":{"draft":{"name":"Cash","kind":"checking","currencyCode":"USD"}}});
    let saved = call("account_save", args.clone()).unwrap();
    assert_eq!(saved["revision"], "1");
    assert_eq!(saved["value"]["balance"]["minor"], "0");
    assert_eq!(call("account_save", args).unwrap(), saved);
    let account = &saved["value"]["id"];
    let entry = call(
        "transaction_save",
        json!({"mutation":mutation("1"),"input":{"draft":{
            "accountId":account,"bankDate":"2024-02-29","amount":{"currency":"USD","minor":"345"},
            "direction":"debit","status":"posted","description":"Market shop","notes":"#fresh",
        }}}),
    )
    .unwrap();
    assert_eq!(entry["value"]["row"]["amount"]["minor"], "345");
    assert_eq!(entry["revision"], "2");
    let list=call("transactions_list",json!({"scope":scope,"input":{"filter":{"tags":["fresh"]},"page":{"limit":5},"runningBalance":true}})).unwrap();
    assert_eq!(list["rows"][0]["runningBalance"]["minor"], "-345");
    let report=call("report_get",json!({"scope":scope,"input":{"kind":"incomeVsExpense",
        "filter":{"range":{"from":"2024-02-01","to":"2024-02-29"}},"granularity":"month","currency":"USD"}})).unwrap();
    assert_eq!(report["points"][0]["expense"]["reliable"]["minor"], "345");
    assert!(
        invoke(
            &window,
            "https://example.invalid",
            "accounts_list",
            json!({"scope":scope,"input":{}})
        )
        .is_err()
    );
    assert_eq!(
        call(
            "budgets_get",
            json!({"scope":scope,"input":{"month":"2026-01"}})
        )
        .unwrap_err()["messageKey"],
        "optional_module_disabled"
    );
    call(
        "app_preferences_set",
        json!({"patch":{"hiddenFeatures":[]}}),
    )
    .unwrap();
    let path = temp.path().join("transactions.csv");
    let grant = desktop
        .grants
        .issue("main", PathPurpose::TransactionExport, &path)
        .unwrap();
    let export = call(
        "transactions_export_start",
        json!({"scope":scope,"input":{
            "selection":{"kind":"ids","ids":[entry["value"]["row"]["id"]]},"saveGrant":grant.id
        }}),
    )
    .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let status = call("job_status", json!({"scope":scope,"id":export["id"]})).unwrap();
        if status["state"] == "complete" {
            assert_eq!(status["output"]["value"]["rowCount"], "1");
            break;
        }
        assert!(
            status["state"] == "queued"
                || status["state"] == "running"
                || status["state"] == "publishing",
            "{status}"
        );
        assert!(std::time::Instant::now() < deadline, "CSV job timed out");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        std::fs::read_to_string(path)
            .unwrap()
            .contains("Market shop")
    );
    let prefs = call(
        "app_preferences_set",
        json!({"patch":{"theme":"dark","privacyMask":true}}),
    )
    .unwrap();
    assert_eq!(prefs["privacyMask"], true);
    assert_eq!(prefs["theme"], "mocha");
    assert_eq!(call("app_bootstrap", json!({})).unwrap()["prefs"], prefs);

    let goal=call("goal_save",json!({"mutation":{"scope":scope,"requestId":Uuid::new_v4()},"input":{"draft":{
        "name":"Reserve","target":{"currency":"USD","minor":"1000"},"startsOn":"2026-01-01","targetOn":"2027-01-01",
        "state":"active","source":{"kind":"manual","current":{"currency":"USD","minor":"100"}}}}})).unwrap();
    let write = |command: &str, input: Value| {
        call(
            command,
            json!({"mutation":{"scope":scope,"requestId":Uuid::new_v4()},"input":input}),
        )
        .unwrap()
    };
    let category = write(
        "category_save",
        json!({"draft":{"name":"Food","kind":"expense"}}),
    );
    write(
        "budgets_save",
        json!({"entries":[{"key":{"categoryId":category["value"]["id"],"month":"2026-01"},
      "amount":{"currency":"USD","minor":"100"}}],"overwrite":false}),
    );
    write(
        "asset_save",
        json!({"draft":{"name":"Asset","kind":"other","currency":"USD"}}),
    );
    write(
        "recurring_save",
        json!({"draft":{"accountId":account,"amount":{"currency":"USD","minor":"20"},"direction":"debit",
      "description":"Fixture","frequency":"monthly","startsOn":"2026-01-01","weekendPolicy":"none","active":true}}),
    );
    write("group_save", json!({"draft":{"name":"Trip","kind":"trip"}}));
    let module_queries = [
        ("budgets_get", json!({"month":"2026-01"})),
        ("goals_list", json!({"page":{"limit":5}})),
        ("assets_list", json!({"page":{"limit":5}})),
        ("recurring_list", json!({"page":{"limit":5}})),
        ("groups_list", json!({"page":{"limit":5}})),
    ];
    let baseline = module_queries
        .iter()
        .map(|(cmd, input)| call(cmd, json!({"scope":scope,"input":input})).unwrap())
        .collect::<Vec<_>>();
    call("app_preferences_set",json!({"patch":{"hiddenFeatures":["budgets","goals","assets","recurring","groups","fx-provider"]}})).unwrap();
    for (cmd, input) in &module_queries {
        assert_eq!(
            call(cmd, json!({"scope":scope,"input":input})).unwrap_err()["messageKey"],
            "optional_module_disabled"
        );
    }
    let delete_goal = json!({"mutation":{"scope":scope,"requestId":Uuid::new_v4()},"input":{"id":goal["value"]["id"],"confirm":true}});
    assert_eq!(
        call("goal_delete", delete_goal).unwrap_err()["messageKey"],
        "optional_module_disabled"
    );
    call(
        "app_preferences_set",
        json!({"patch":{"hiddenFeatures":[]}}),
    )
    .unwrap();
    for ((cmd, input), expected) in module_queries.iter().zip(baseline) {
        let actual = call(cmd, json!({"scope":scope,"input":input})).unwrap();
        assert_eq!(actual["rows"], expected["rows"], "{cmd}");
    }
    let again = call(
        "goal_get",
        json!({"scope":scope,"input":{"id":goal["value"]["id"]}}),
    )
    .unwrap();
    assert_eq!(again["id"], goal["value"]["id"]);
    assert_eq!(again["draft"], goal["value"]["draft"]);
    let bad_balance = call(
        "account_save",
        json!({
            "mutation":{"scope":scope,"requestId":Uuid::new_v4()},
            "input":{"draft":{"name":"Uncommitted cash","kind":"cash","currencyCode":"USD"},
                     "startingBalance":{"currency":"JPY","minor":"100"}}
        }),
    )
    .unwrap_err();
    assert_eq!(bad_balance["fields"]["currency"], "currency_mismatch");
    let after_failure = call("accounts_list", json!({"scope":scope,"input":{}})).unwrap();
    assert!(
        !after_failure
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["account"]["name"] == "Uncommitted cash")
    );
    let destination = write(
        "account_save",
        json!({"draft":{"name":"Cash wallet","kind":"cash","currencyCode":"USD"},
               "startingBalance":{"currency":"USD","minor":"525"}}),
    );
    assert_eq!(destination["value"]["account"]["kind"], "cash");
    assert_eq!(destination["value"]["balance"]["minor"], "525");
    let transfer = write(
        "transfer_save",
        json!({
            "source":{"kind":"draft","draft":{"accountId":account,"bankDate":"2024-02-29",
                "amount":{"currency":"USD","minor":"100"},"direction":"debit","status":"posted","description":"Transfer"}},
            "destination":{"kind":"draft","draft":{"accountId":destination["value"]["id"],
                "bankDate":"2024-02-29","amount":{"currency":"USD","minor":"100"},"direction":"credit",
                "status":"posted","description":"Transfer"}}
        }),
    );
    let read_transfer = || {
        call(
            "transfer_get",
            json!({"scope":scope,"input":{"linkId":transfer["value"]["id"]}}),
        )
    };
    let details = read_transfer().unwrap();
    assert!(details["rate"].is_null());
    assert_eq!(
        details["pair"]["destination"]["row"]["amount"]["minor"],
        "100"
    );
    let second = create("Second ledger");
    assert_eq!(read_transfer().unwrap_err()["code"], "StaleSession");
    assert_eq!(
        call("accounts_list", json!({"scope":scope,"input":{}})).unwrap_err()["code"],
        "StaleSession"
    );
    assert_eq!(
        call("accounts_list", json!({"scope":second["scope"],"input":{}})).unwrap(),
        json!([])
    );
}
