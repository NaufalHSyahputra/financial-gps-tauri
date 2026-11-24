use core_engine::{
    domain::{AppError, ErrorCode, Result, Scope},
    importers::{self as imports, *},
    ledger_engine::{
        Context, DateBasis, DateRange, Deleted, Money, Mutation, Page, PageReq, VersionedId, Write,
        accounts::{self, *},
        catalog::{self, *},
        dto,
        export::*,
        fx::{self, *},
        reports::{self, *},
        staging::Stage,
        transactions::{self, *},
        transfers::{self, *},
        workflows::{self, *},
    },
    planning::{assets, budgets, goals, loans, recurring, shared},
    preferences::{self, AppPreferences, Feature},
    rules::{self, *},
    sql::SqlTransaction,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, from_str, to_string, to_value};
use std::collections::BTreeMap;
use wasm_bindgen::prelude::*;

use crate::{error_value, invalid_request, sqlite};

#[wasm_bindgen]
extern "C" {
    pub type JsJobObserver;
    #[wasm_bindgen(method, catch)]
    fn cancelled(this: &JsJobObserver) -> std::result::Result<bool, JsValue>;
    #[wasm_bindgen(method, catch)]
    fn publishing(this: &JsJobObserver) -> std::result::Result<bool, JsValue>;
    #[wasm_bindgen(method, catch)]
    fn progress(this: &JsJobObserver, done: u32) -> std::result::Result<(), JsValue>;
}

#[derive(Default)]
struct Stages {
    imports: Stage<ImportPlan>,
    undos: Stage<UndoPlan>,
    rules: Stage<RulePlan>,
}

#[wasm_bindgen]
#[derive(Default)]
pub struct WebEngine {
    previews: PreviewStore,
    phase_four: Stages,
    prefs: AppPreferences,
}

#[wasm_bindgen]
impl WebEngine {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_preferences(&mut self, json: &str) -> std::result::Result<(), JsValue> {
        self.prefs = from_str(json).map_err(|e| error_value(invalid_request(e)))?;
        Ok(())
    }

    pub fn preferences(&self) -> std::result::Result<String, JsValue> {
        to_string(&self.prefs).map_err(|e| error_value(invalid_request(e)))
    }

    pub fn patch_preferences(&self, json: &str) -> std::result::Result<String, JsValue> {
        let result = || {
            let patch = from_str(json).map_err(invalid_request)?;
            to_string(&preferences::apply_patch(self.prefs.clone(), patch)?)
                .map_err(invalid_request)
        };
        result().map_err(error_value)
    }

    pub fn execute_file(
        &self,
        db: sqlite::JsDatabase,
        command: &str,
        json: &str,
        name: &str,
        bytes: &[u8],
        observer: Option<JsJobObserver>,
    ) -> std::result::Result<String, JsValue> {
        let work = || -> Result<String> {
            self.check_module(command)?;
            let request = from_str(json).map_err(invalid_request)?;
            to_string(&self.extra(&db, command, &request, name, bytes, observer.as_ref())?)
                .map_err(invalid_request)
        };
        work().map_err(error_value)
    }

    pub fn invalidate(&self, scope: &str) -> std::result::Result<(), JsValue> {
        let scope = from_str(scope).map_err(|e| error_value(invalid_request(e)))?;
        self.previews.invalidate("main");
        self.phase_four.imports.invalidate(&scope);
        self.phase_four.undos.invalidate(&scope);
        self.phase_four.rules.invalidate(&scope);
        Ok(())
    }

    pub fn execute(
        &self,
        db: sqlite::JsDatabase,
        command: &str,
        json: &str,
    ) -> std::result::Result<String, JsValue> {
        let work = || -> Result<String> {
            self.check_module(command)?;
            let request = from_str(json).map_err(invalid_request)?;
            let result = self.dispatch(&db, command, &request)?;
            to_string(&result).map_err(invalid_request)
        };
        work().map_err(error_value)
    }
}

fn field<T: DeserializeOwned>(request: &Value, key: &str) -> Result<T> {
    serde_json::from_value(request.get(key).cloned().unwrap_or(Value::Null))
        .map_err(invalid_request)
}

impl WebEngine {
    fn check_module(&self, command: &str) -> Result<()> {
        let module = match command.split('_').next() {
            Some("budget" | "budgets") => Feature::Budgets,
            Some("goal" | "goals") => Feature::Goals,
            Some("asset" | "assets") => Feature::Assets,
            Some("recurring") => Feature::Recurring,
            Some("group" | "groups") => Feature::SharedGroups,
            Some("fx") if matches!(command, "fx_provider_configure" | "fx_refresh_start") => {
                Feature::FxProvider
            }
            _ => return Ok(()),
        };
        if !self.prefs.enables(module) {
            return Err(AppError {
                fields: Some(BTreeMap::from([("module".into(), module.key().into())])),
                ..AppError::new(ErrorCode::Conflict, "optional_module_disabled")
            });
        }
        Ok(())
    }

    fn dispatch(&self, db: &sqlite::JsDatabase, command: &str, request: &Value) -> Result<Value> {
        // These declarations are also expanded by the native endpoint using its scope and IPC macros.
        macro_rules! scoped_read {
            ($command:ident, $result:ty, |$ctx:ident| $work:expr) => {
                if command == stringify!($command) {
                    let scope: Scope = field(request, "scope")?;
                    return read(db, &scope, |$ctx| -> Result<$result> { $work });
                }
            };
            ($command:ident, $input_ty:ty, $result:ty, |$ctx:ident,$args:ident,$state:ident,$window:ident| $work:expr) => {
                if command == stringify!($command) {
                    let scope: Scope = field(request, "scope")?;
                    let $args: $input_ty = field(request, "input")?;
                    let $state = self;
                    let $window = "main".to_owned();
                    return read(db, &scope, |$ctx| -> Result<$result> {
                        ($work).map(Into::into)
                    });
                }
            };
        }

        macro_rules! scoped_write {
            ($command:ident, $input_ty:ty, $result:ty, $affected:expr, |$ctx:ident,$args:ident,$state:ident,$window:ident| $work:expr) => {
                if command == stringify!($command) {
                    let mutation: Mutation = field(request, "mutation")?;
                    let $args: $input_ty = field(request, "input")?;
                    let $state = self;
                    let $window = "main".to_owned();
                    return write(db, &mutation.scope, |ctx| {
                        ctx.mutate(
                            stringify!($command),
                            &mutation,
                            &$args,
                            $affected,
                            |$ctx| -> Result<$result> { $work },
                        )
                    });
                }
            };
        }

        core_engine::ledger_operations!();
        core_engine::planning_operations!();
        self.extra(db, command, request, "", &[], None)
    }
}

fn read<T: Serialize>(
    db: &sqlite::JsDatabase,
    scope: &Scope,
    work: impl FnOnce(&Context<'_>) -> Result<T>,
) -> Result<Value> {
    transaction(db, scope, false, work)
}

fn write<T: Serialize>(
    db: &sqlite::JsDatabase,
    scope: &Scope,
    work: impl FnOnce(&Context<'_>) -> Result<T>,
) -> Result<Value> {
    transaction(db, scope, true, work)
}

fn transaction<T: Serialize>(
    db: &sqlite::JsDatabase,
    scope: &Scope,
    write: bool,
    work: impl FnOnce(&Context<'_>) -> Result<T>,
) -> Result<Value> {
    let session = sqlite::Session(db);
    session.execute_batch(if write { "BEGIN IMMEDIATE" } else { "BEGIN" })?;
    let result = (|| {
        let ctx = Context::system(&session, scope)?.without_currency_conversion();
        let counter = |ctx: &Context<'_>| {
            ctx.tx
                .query_row("SELECT total_changes()", [], |row| row.get::<_, i64>(0))
        };
        let before = if write { counter(&ctx)? } else { 0 };
        let value = to_value(work(&ctx)?).map_err(invalid_request)?;
        if write && before != counter(&ctx)? {
            ctx.tx.execute_batch("UPDATE ledger_settings SET value_json=json_set(value_json,'$.local',json_extract(value_json,'$.local')+1) WHERE key='secret.sync.v1'")?;
        }
        session.execute_batch("COMMIT")?;
        Ok(value)
    })();
    if result.is_err() {
        let _ = session.execute_batch("ROLLBACK");
    }
    result
}

macro_rules! page_response {
    ($response:ident,$row:ty) => {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        pub struct $response {
            rows: Vec<$row>,
            #[serde(skip_serializing_if = "Option::is_none")]
            next_cursor: Option<String>,
            revision: String,
        }
        impl From<Page<$row>> for $response {
            fn from(page: Page<$row>) -> Self {
                Self {
                    rows: page.rows,
                    next_cursor: page.next_cursor,
                    revision: page.revision,
                }
            }
        }
    };
}

core_engine::ledger_types!();
core_engine::planning_types!();

#[path = "extras.rs"]
pub(crate) mod extras;
