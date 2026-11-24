use std::sync::Arc;

use crate::ledger_engine::workflows;
use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Typescript;
use tauri::{Emitter, Manager, Runtime, State, WebviewWindow, Wry};
use tauri_plugin_dialog::DialogExt;
use tauri_specta::{Builder, ErrorHandlingMode, collect_commands};
use uuid::Uuid;

mod jobs;
mod phase_four;
mod phase_three;
mod storage_mode;

use crate::{
    crypto::Credential,
    domain::{
        AppError, ErrorCode, LedgerDescriptor, LedgerSession, LifecycleState, OpenResult,
        Protection, Result, Scope,
    },
    storage::{
        CreateLedger, LedgerManager, OpenTarget, PathGrant, PathGrants, PathPurpose, SCHEMA_VERSION,
    },
};

struct Desktop {
    ledgers: Result<LedgerManager>,
    grants: PathGrants,
    previews: workflows::PreviewStore,
    jobs: jobs::JobStore,
    phase_four: phase_four::Stages,
    maintenance_wakeup: tokio::sync::Notify,
    sync_wakeup: tokio::sync::Notify,
}

impl Desktop {
    fn ledgers(&self) -> Result<&LedgerManager> {
        self.ledgers.as_ref().map_err(Clone::clone)
    }

    fn check_module(&self, command: &str) -> Result<()> {
        use crate::storage::preferences::Feature;
        let module = match command.split('_').next() {
            Some("budget" | "budgets") => Feature::Budgets,
            Some("goal" | "goals") => Feature::Goals,
            Some("asset" | "assets") => Feature::Assets,
            Some("recurring") => Feature::Recurring,
            Some("group" | "groups") => Feature::SharedGroups,
            _ => return Ok(()),
        };
        self.ledgers()?.require_feature(module)
    }

    fn session_changed<T>(&self, label: &str, operation: impl FnOnce() -> Result<T>) -> Result<T> {
        let previous = self.ledgers()?.session_scope(label)?;
        let result = operation();
        if self
            .ledgers()?
            .session_scope(label)
            .is_ok_and(|next| next != previous)
        {
            self.previews.invalidate(label);
            self.maintenance_wakeup.notify_one();
            self.sync_wakeup.notify_one();
            if let Some(scope) = previous.as_ref() {
                self.jobs.invalidate(label, scope);
                self.phase_four.invalidate(scope);
            }
        }
        result
    }
}

fn authorize(window: &str) -> Result<()> {
    if window != "main" {
        return Err(AppError::new(ErrorCode::Validation, "unauthorized_window"));
    }
    Ok(())
}

async fn blocking<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|_| AppError::new(ErrorCode::Integrity, "local_worker_failed"))?
}

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
struct SchemaRange {
    minimum: u32,
    maximum: u32,
}

use crate::storage::preferences::AppPreferences;

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
struct Bootstrap {
    session: Option<LedgerSession>,
    prefs: AppPreferences,
    schema_range: SchemaRange,
}

#[derive(Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ChooseFile {
    purpose: PathPurpose,
    multiple: Option<bool>,
}

#[derive(Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateRequest {
    name: String,
    destination: Option<String>,
    base_currency: String,
    timezone: Option<String>,
    protection: Protection,
    credential: Option<Credential>,
    remember_key: bool,
}

#[derive(Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OpenRequest {
    ledger_instance_id: Option<String>,
    open_grant: Option<String>,
    request_id: String,
}

#[derive(Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UnlockRequest {
    unlock_challenge: String,
    credential: Option<Credential>,
}

#[tauri::command]
#[specta::specta]
async fn app_bootstrap<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<Desktop>>,
) -> Result<Bootstrap> {
    authorize(window.label())?;
    let label = window.label().to_owned();
    let state = Arc::clone(state.inner());
    phase_three::blocking_read(move || {
        let manager = state.ledgers()?;
        Ok(Bootstrap {
            session: manager.session(&label)?,
            prefs: manager.preferences()?,
            schema_range: SchemaRange {
                minimum: core_engine::migrations::INITIAL_VERSION,
                maximum: SCHEMA_VERSION,
            },
        })
    })
    .await
}

#[tauri::command]
#[specta::specta]
async fn ledger_list<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<Desktop>>,
) -> Result<Vec<LedgerDescriptor>> {
    authorize(window.label())?;
    let state = Arc::clone(state.inner());
    phase_three::blocking_read(move || state.ledgers()?.list()).await
}

#[tauri::command]
#[specta::specta]
async fn file_choose<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<Desktop>>,
    input: ChooseFile,
) -> Result<Vec<PathGrant>> {
    authorize(window.label())?;
    let multiple = input.multiple.unwrap_or(false);
    if multiple && input.purpose != PathPurpose::AttachmentRead {
        return Err(AppError::validation("multiple", "one_file_at_a_time"));
    }
    let (description, extensions, default): (&str, &[&str], Option<&str>) = match input.purpose {
        PathPurpose::LedgerOpen => ("Activity file", &["db"], None),
        PathPurpose::LedgerCreate => ("Activity file", &["db"], Some("ledger.db")),
        PathPurpose::TransactionExport => ("Transaction CSV", &["csv"], Some("transactions.csv")),
        PathPurpose::RulePackRead => ("Categorization rules", &["json"], None),
        PathPurpose::RulePackWrite => ("Categorization rules", &["json"], Some("rules.json")),
        PathPurpose::AttachmentRead => (
            "Receipt or document",
            &["jpg", "jpeg", "png", "webp", "gif", "heic", "pdf"],
            None,
        ),
        PathPurpose::AttachmentWrite => (
            "Receipt or document",
            &["jpg", "jpeg", "png", "webp", "gif", "heic", "pdf"],
            Some("receipt"),
        ),
        PathPurpose::BackupWrite => (
            "Encrypted activity backup",
            &["fgpsbackup"],
            Some("ledger.fgpsbackup"),
        ),
        PathPurpose::BackupDirectory => ("Scheduled backup folder", &[], None),
        PathPurpose::RestoreRead => ("Encrypted activity backup", &["fgpsbackup"], None),
    };
    let picker = window.dialog().file().add_filter(description, extensions);
    let (send, receive) = tokio::sync::oneshot::channel();
    if multiple {
        picker.pick_files(move |files| {
            let _ = send.send(files.unwrap_or_default());
        });
    } else {
        let callback = move |chosen: Option<tauri_plugin_dialog::FilePath>| {
            let _ = send.send(chosen.into_iter().collect::<Vec<_>>());
        };
        if input.purpose == PathPurpose::BackupDirectory {
            #[cfg(desktop)]
            picker.pick_folder(callback);
            #[cfg(mobile)]
            return Err(AppError::validation(
                "destination",
                "custom_backup_folder_unavailable",
            ));
        } else if let Some(default) = default {
            picker.set_file_name(default).save_file(callback);
        } else {
            picker.pick_file(callback);
        }
    }
    let selected = receive
        .await
        .map_err(|_| AppError::new(ErrorCode::Cancelled, "file_dialog_closed"))?;
    if selected.len() > 10 {
        return Err(AppError::validation("files", "too_many_attachments"));
    }
    let paths = selected
        .into_iter()
        .map(|chosen| {
            chosen
                .into_path()
                .map_err(|_| AppError::validation("grant", "local_file_required"))
        })
        .collect::<Result<Vec<_>>>()?;
    let state = Arc::clone(state.inner());
    let label = window.label().to_owned();
    blocking(move || {
        paths
            .into_iter()
            .map(|path| state.grants.issue(&label, input.purpose, &path))
            .collect()
    })
    .await
}

#[tauri::command]
#[specta::specta]
async fn ledger_create<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<Desktop>>,
    input: CreateRequest,
) -> Result<LedgerSession> {
    authorize(window.label())?;
    let label = window.label().to_owned();
    let state = Arc::clone(state.inner());
    blocking(move || {
        let destination = input
            .destination
            .as_deref()
            .map(|id| state.grants.consume(&label, PathPurpose::LedgerCreate, id))
            .transpose()?;
        state.session_changed(&label, || {
            state.ledgers()?.create(
                &label,
                CreateLedger {
                    name: input.name,
                    destination,
                    base_currency: input.base_currency,
                    timezone: input.timezone,
                    protection: input.protection,
                    credential: input.credential,
                    remember_key: input.remember_key,
                },
            )
        })
    })
    .await
}

fn parse_id(value: &str, field: &str) -> Result<Uuid> {
    Uuid::parse_str(value).map_err(|_| AppError::validation(field, "invalid_id"))
}

async fn open<R: Runtime>(
    window: WebviewWindow<R>,
    state: Arc<Desktop>,
    input: OpenRequest,
) -> Result<OpenResult> {
    authorize(window.label())?;
    let label = window.label().to_owned();
    let request = parse_id(&input.request_id, "requestId")?;
    blocking(move || {
        state.session_changed(&label, || {
            match (input.ledger_instance_id, input.open_grant) {
                (Some(id), None) => {
                    let instance = parse_id(&id, "ledgerInstanceId")?;
                    state
                        .ledgers()?
                        .open(&label, request, format!("instance:{instance}"), || {
                            Ok(OpenTarget::Registered(instance))
                        })
                }
                (None, Some(grant)) => {
                    state
                        .ledgers()?
                        .open(&label, request, format!("grant:{grant}"), || {
                            Ok(OpenTarget::Chosen(state.grants.consume(
                                &label,
                                PathPurpose::LedgerOpen,
                                &grant,
                            )?))
                        })
                }
                _ => Err(AppError::validation(
                    "ledgerInstanceId",
                    "select_one_ledger",
                )),
            }
        })
    })
    .await
}

#[tauri::command]
#[specta::specta]
async fn ledger_open<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<Desktop>>,
    input: OpenRequest,
) -> Result<OpenResult> {
    open(window, Arc::clone(state.inner()), input).await
}

#[tauri::command]
#[specta::specta]
async fn ledger_switch<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<Desktop>>,
    input: OpenRequest,
) -> Result<OpenResult> {
    open(window, Arc::clone(state.inner()), input).await
}

#[tauri::command]
#[specta::specta]
async fn ledger_unlock<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<Desktop>>,
    input: UnlockRequest,
) -> Result<LedgerSession> {
    authorize(window.label())?;
    let label = window.label().to_owned();
    let state = Arc::clone(state.inner());
    blocking(move || {
        state.session_changed(&label, || {
            state
                .ledgers()?
                .unlock(&label, &input.unlock_challenge, input.credential)
        })
    })
    .await
}

#[tauri::command]
#[specta::specta]
fn ledger_integrity_status<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<Desktop>>,
    scope: Scope,
) -> Result<bool> {
    authorize(window.label())?;
    state.ledgers()?.integrity_verified(window.label(), &scope)
}

#[tauri::command]
#[specta::specta]
async fn ledger_lock<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<Desktop>>,
    scope: Scope,
) -> Result<LifecycleState> {
    authorize(window.label())?;
    let label = window.label().to_owned();
    let state = Arc::clone(state.inner());
    blocking(move || state.session_changed(&label, || state.ledgers()?.lock(&label, &scope))).await
}

#[tauri::command]
#[specta::specta]
async fn ledger_close<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<Desktop>>,
    scope: Scope,
) -> Result<LifecycleState> {
    authorize(window.label())?;
    let label = window.label().to_owned();
    let state = Arc::clone(state.inner());
    blocking(move || state.session_changed(&label, || state.ledgers()?.close(&label, &scope))).await
}

macro_rules! commands {
    ($runtime:ident) => {
        Builder::<$runtime>::new()
            .error_handling(ErrorHandlingMode::Result)
            .events(tauri_specta::collect_events![jobs::Progress])
            .commands(collect_commands![
                app_bootstrap::<$runtime>,
                ledger_list::<$runtime>,
                file_choose::<$runtime>,
                ledger_create::<$runtime>,
                ledger_open::<$runtime>,
                ledger_switch::<$runtime>,
                ledger_unlock::<$runtime>,
                ledger_lock::<$runtime>,
                ledger_integrity_status::<$runtime>,
                ledger_close::<$runtime>,
                storage_mode::turso_credentials_validate::<$runtime>,
                storage_mode::ledger_storage_mode_set::<$runtime>,
                storage_mode::ledger_sync_probe_saved::<$runtime>,
                storage_mode::ledger_sync_status::<$runtime>,
                storage_mode::ledger_sync_now::<$runtime>,
                storage_mode::ledger_sync_publish_local::<$runtime>,
                storage_mode::ledger_sync_accept_remote::<$runtime>,
                phase_three::ledger_settings_get::<$runtime>,
                phase_three::ledger_settings_set::<$runtime>,
                phase_three::accounts_list::<$runtime>,
                phase_three::account_choices::<$runtime>,
                phase_three::account_get::<$runtime>,
                phase_three::account_save::<$runtime>,
                phase_three::account_archive::<$runtime>,
                phase_three::account_delete::<$runtime>,
                phase_three::account_set_opening::<$runtime>,
                phase_three::account_adjust_balance::<$runtime>,
                phase_three::card_lines_list::<$runtime>,
                phase_three::card_line_save::<$runtime>,
                phase_three::card_line_delete::<$runtime>,
                phase_three::card_statements_list::<$runtime>,
                phase_three::card_statement_save::<$runtime>,
                phase_three::card_statement_delete::<$runtime>,
                phase_three::transactions_list::<$runtime>,
                phase_three::transaction_get::<$runtime>,
                phase_three::transaction_save::<$runtime>,
                phase_three::transaction_post::<$runtime>,
                phase_three::transaction_delete::<$runtime>,
                phase_three::transactions_bulk_preview::<$runtime>,
                phase_three::transactions_bulk_apply::<$runtime>,
                phase_three::installment_preview::<$runtime>,
                phase_three::installment_apply::<$runtime>,
                phase_three::transfer_get::<$runtime>,
                phase_three::transfer_candidates::<$runtime>,
                phase_three::transfer_save::<$runtime>,
                phase_three::transfer_unlink::<$runtime>,
                phase_three::transfer_delete::<$runtime>,
                phase_three::category_groups_list::<$runtime>,
                phase_three::categories_list::<$runtime>,
                phase_three::payees_list::<$runtime>,
                phase_three::category_group_save::<$runtime>,
                phase_three::category_save::<$runtime>,
                phase_three::payee_save::<$runtime>,
                phase_three::category_group_delete::<$runtime>,
                phase_three::category_delete::<$runtime>,
                phase_three::payee_delete::<$runtime>,
                phase_three::payee_merge::<$runtime>,
                phase_three::currencies_list::<$runtime>,
                phase_three::dashboard_get::<$runtime>,
                phase_three::report_get::<$runtime>,
                phase_three::calendar_get::<$runtime>,
                phase_three::search_global::<$runtime>,
                phase_three::transactions_export_start::<$runtime>,
                phase_four::rules_list::<$runtime>,
                phase_four::rule_get::<$runtime>,
                phase_four::rule_save::<$runtime>,
                phase_four::rule_delete::<$runtime>,
                phase_four::rules_reorder::<$runtime>,
                phase_four::recurring_list::<$runtime>,
                phase_four::recurring_get::<$runtime>,
                phase_four::recurring_save::<$runtime>,
                phase_four::recurring_archive::<$runtime>,
                phase_four::recurring_occurrences::<$runtime>,
                phase_four::recurring_occurrence_apply::<$runtime>,
                phase_four::loan_get::<$runtime>,
                phase_four::loan_save::<$runtime>,
                phase_four::loan_schedule::<$runtime>,
                phase_four::loan_payment_allocate::<$runtime>,
                phase_four::assets_list::<$runtime>,
                phase_four::asset_get::<$runtime>,
                phase_four::asset_save::<$runtime>,
                phase_four::asset_archive::<$runtime>,
                phase_four::asset_delete::<$runtime>,
                phase_four::asset_groups_list::<$runtime>,
                phase_four::asset_group_get::<$runtime>,
                phase_four::asset_group_save::<$runtime>,
                phase_four::asset_group_delete::<$runtime>,
                phase_four::asset_history::<$runtime>,
                phase_four::asset_valuation_save::<$runtime>,
                phase_four::asset_valuation_delete::<$runtime>,
                phase_four::asset_trade_save::<$runtime>,
                phase_four::asset_trade_delete::<$runtime>,
                phase_four::rule_preview_start::<$runtime>,
                phase_four::rule_apply_start::<$runtime>,
                phase_four::rule_pack_transfer::<$runtime>,
                phase_four::attachments_list::<$runtime>,
                phase_four::attachment_rename::<$runtime>,
                phase_four::backup_list::<$runtime>,
                phase_four::backup_remove::<$runtime>,
                phase_four::attachment_add_start::<$runtime>,
                phase_four::attachment_delete::<$runtime>,
                phase_four::attachment_save_as::<$runtime>,
                phase_four::attachment_open::<$runtime>,
                phase_four::backup_create_start::<$runtime>,
                phase_four::restore_preview_start::<$runtime>,
                phase_four::restore_confirm_replacement::<$runtime>,
                phase_four::restore_apply_start::<$runtime>,
                phase_four::budgets_get::<$runtime>,
                phase_four::budgets_save::<$runtime>,
                phase_four::budgets_delete::<$runtime>,
                phase_four::budgets_copy::<$runtime>,
                phase_four::financial_calendar_get::<$runtime>,
                phase_four::financial_calendar_save::<$runtime>,
                phase_four::budget_cycle_get::<$runtime>,
                phase_four::budget_cycle_edit::<$runtime>,
                phase_four::cash_plan_get::<$runtime>,
                phase_four::cash_plan_edit::<$runtime>,
                phase_four::posting_correction_preview::<$runtime>,
                phase_four::posting_correction_apply::<$runtime>,
                phase_four::posting_audit_list::<$runtime>,
                phase_four::budget_suggestions::<$runtime>,
                phase_four::goals_list::<$runtime>,
                phase_four::goal_get::<$runtime>,
                phase_four::goal_save::<$runtime>,
                phase_four::goal_delete::<$runtime>,
                phase_four::groups_list::<$runtime>,
                phase_four::group_get::<$runtime>,
                phase_four::group_save::<$runtime>,
                phase_four::group_archive::<$runtime>,
                phase_four::group_member_save::<$runtime>,
                phase_four::group_member_remove::<$runtime>,
                phase_four::group_expense_save::<$runtime>,
                phase_four::group_expense_remove::<$runtime>,
                phase_four::group_settlement_save::<$runtime>,
                phase_four::group_settlement_remove::<$runtime>,
                phase_four::backup_schedule_get::<$runtime>,
                phase_four::backup_schedule_set::<$runtime>,
                phase_four::app_preferences_get::<$runtime>,
                phase_four::app_preferences_set::<$runtime>,
                phase_four::layout_get::<$runtime>,
                phase_four::layout_set::<$runtime>,
                phase_four::ledger_change_protection::<$runtime>,
                phase_four::ledger_keychain_set::<$runtime>,
                phase_three::job_status::<$runtime>,
                phase_three::job_cancel::<$runtime>,
            ])
    };
}

#[cfg(any(target_os = "android", all(unix, test)))]
fn resolve_android_app_data(path: &std::path::Path) -> Result<std::path::PathBuf> {
    // Android can report an OS-managed alias such as /data/user/0 for app-private
    // storage. Resolve only this trusted root, before validating ledger paths.
    path.canonicalize().map_err(crate::storage::io_error)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let commands = commands!(Wry);
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(commands.invoke_handler())
        .setup(move |app| {
            commands.mount_events(app);
            let (alerts, mut integrity_alerts) = tokio::sync::mpsc::channel::<(Scope, AppError)>(8);
            let ledgers = app
                .path()
                .app_data_dir()
                .map_err(|_| AppError::new(ErrorCode::Io, "app_data_directory_unavailable"))
                .and_then(|path| {
                    #[cfg(target_os = "android")]
                    let path = resolve_android_app_data(&path)?;
                    LedgerManager::new(&path)
                })
                .map(|mut manager| {
                    manager.set_integrity_alerts(alerts);
                    manager
                });
            let desktop = Arc::new(Desktop {
                ledgers,
                grants: PathGrants::default(),
                previews: Default::default(),
                jobs: Default::default(),
                phase_four: Default::default(),
                maintenance_wakeup: Default::default(),
                sync_wakeup: Default::default(),
            });
            app.manage(desktop.clone());
            if let Some(window) = app.get_webview_window("main") {
                tauri::async_runtime::spawn(async move {
                    while let Some(alert) = integrity_alerts.recv().await {
                        let _ = window.emit("ledger-integrity-alert", alert);
                    }
                });
            }
            let sync_desktop = Arc::clone(&desktop);
            let sync_window = app.get_webview_window("main");
            tauri::async_runtime::spawn(async move {
                loop {
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_secs(15),
                        sync_desktop.sync_wakeup.notified(),
                    )
                    .await;
                    let desktop = Arc::clone(&sync_desktop);
                    let result = blocking(move || {
                        let manager = desktop.ledgers()?;
                        Ok(if let Some(scope) = manager.session_scope("main")? {
                            let status = manager.sync_now("main", &scope)?;
                            Some((scope, status))
                        } else {
                            None
                        })
                    })
                    .await;
                    if let (Some(window), Ok(Some(event))) = (&sync_window, result) {
                        let _ = window.emit("ledger-sync-status", event);
                    }
                }
            });
            tauri::async_runtime::spawn(async move {
                loop {
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_secs(30),
                        desktop.maintenance_wakeup.notified(),
                    )
                    .await;
                    let desktop = desktop.clone();
                    let _ = blocking(move || {
                        desktop.phase_four.purge_expired();
                        let manager = desktop.ledgers()?;
                        let _ = manager.run_recurring_generation("main");
                        manager.run_scheduled_backup("main")
                    })
                    .await;
                }
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("build Financial GPS desktop")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit)
                && let Ok(manager) = app.state::<Arc<Desktop>>().ledgers()
            {
                manager.shutdown();
            }
        });
}

pub fn export_bindings(
    path: impl AsRef<std::path::Path>,
) -> std::result::Result<(), specta_typescript::Error> {
    commands!(Wry).export(Typescript::default(), path)
}

#[cfg(test)]
mod tests;
