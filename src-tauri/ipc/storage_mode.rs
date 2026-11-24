use super::*;
use crate::storage::{SetStorageMode, SyncStatus, SyncToken};

#[derive(Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TursoCredentials {
    url: String,
    auth_token: SyncToken,
}

#[tauri::command]
#[specta::specta]
pub(super) async fn turso_credentials_validate<R: Runtime>(
    window: WebviewWindow<R>,
    input: TursoCredentials,
) -> Result<()> {
    authorize(window.label())?;
    blocking(move || LedgerManager::validate_turso_credentials(&input.url, &input.auth_token)).await
}

#[tauri::command]
#[specta::specta]
pub(super) async fn ledger_storage_mode_set<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<Desktop>>,
    scope: Scope,
    input: SetStorageMode,
) -> Result<SyncStatus> {
    authorize(window.label())?;
    let label = window.label().to_owned();
    let desktop = Arc::clone(state.inner());
    let event_scope = scope.clone();
    let result = blocking(move || {
        let status = desktop.ledgers()?.storage_mode_set(&label, &scope, input)?;
        desktop.sync_wakeup.notify_one();
        Ok(status)
    })
    .await?;
    let _ = window.emit("ledger-sync-status", (&event_scope, &result));
    Ok(result)
}

#[tauri::command]
#[specta::specta]
pub(super) async fn ledger_sync_status<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<Desktop>>,
    scope: Scope,
) -> Result<SyncStatus> {
    authorize(window.label())?;
    let label = window.label().to_owned();
    let desktop = Arc::clone(state.inner());
    blocking(move || desktop.ledgers()?.sync_status(&label, &scope)).await
}

#[tauri::command]
#[specta::specta]
pub(super) async fn ledger_sync_now<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<Desktop>>,
    scope: Scope,
) -> Result<SyncStatus> {
    authorize(window.label())?;
    let label = window.label().to_owned();
    let desktop = Arc::clone(state.inner());
    let event_scope = scope.clone();
    let result = blocking(move || desktop.ledgers()?.sync_now(&label, &scope)).await?;
    let _ = window.emit("ledger-sync-status", (&event_scope, &result));
    Ok(result)
}

#[tauri::command]
#[specta::specta]
pub(super) async fn ledger_sync_publish_local<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<Desktop>>,
    scope: Scope,
    expected_remote_generation: String,
) -> Result<SyncStatus> {
    authorize(window.label())?;
    let label = window.label().to_owned();
    let desktop = Arc::clone(state.inner());
    let event_scope = scope.clone();
    let result = blocking(move || {
        desktop
            .ledgers()?
            .sync_publish_local(&label, &scope, &expected_remote_generation)
    })
    .await?;
    let _ = window.emit("ledger-sync-status", (&event_scope, &result));
    Ok(result)
}

#[tauri::command]
#[specta::specta]
pub(super) async fn ledger_sync_probe_saved<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<Desktop>>,
    scope: Scope,
) -> Result<()> {
    authorize(window.label())?;
    let label = window.label().to_owned();
    let desktop = Arc::clone(state.inner());
    blocking(move || desktop.ledgers()?.sync_probe_saved(&label, &scope)).await
}

#[derive(Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct AcceptRemote {
    expected_remote_generation: String,
    expected_local_version: String,
}

#[tauri::command]
#[specta::specta]
pub(super) async fn ledger_sync_accept_remote<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<Desktop>>,
    scope: Scope,
    input: AcceptRemote,
) -> Result<SyncStatus> {
    authorize(window.label())?;
    let label = window.label().to_owned();
    let desktop = Arc::clone(state.inner());
    let event_scope = scope.clone();
    let result = blocking(move || {
        desktop.ledgers()?.sync_accept_remote(
            &label,
            &scope,
            &input.expected_remote_generation,
            &input.expected_local_version,
        )
    })
    .await?;
    let _ = window.emit("ledger-sync-status", (&event_scope, &result));
    Ok(result)
}
