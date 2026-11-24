use super::jobs::{CancelResult, JobKind, JobOutput, JobSnapshot, JobTicket};
use super::*;
use crate::ledger_engine::{
    Context, DateBasis, DateRange, Deleted, Money, Mutation, Page, PageReq, VersionedId, Write,
    accounts::*, catalog::*, currency::Currency, dto, export::*, reports::*, transactions::*,
    transfers::*, workflows::*,
};
use crate::ledger_engine::{accounts, catalog, currency, export, reports, transactions, transfers};

pub(super) async fn blocking_read<T: Send + 'static>(
    operation: impl Fn() -> Result<T> + Send + Sync + 'static,
) -> Result<T> {
    let operation = Arc::new(operation);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let attempt = Arc::clone(&operation);
        match blocking(move || attempt()).await {
            Err(error)
                if error.code == ErrorCode::Busy && tokio::time::Instant::now() < deadline =>
            {
                // Navigation can briefly fill the bounded reader pool.
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
            result => return result,
        }
    }
}

macro_rules! scoped_read {
    ($command:ident, $result:ty, |$ctx:ident| $work:expr) => {
        #[tauri::command]
        #[specta::specta]
        pub(crate) async fn $command<R: Runtime>(
            window: WebviewWindow<R>,
            state: State<'_, Arc<Desktop>>,
            scope: Scope,
        ) -> Result<$result> {
            authorize(window.label())?;
            let label = window.label().to_owned();
            let state = Arc::clone(state.inner());
            $crate::ipc::phase_three::blocking_read(move || {
                state.check_module(stringify!($command))?;
                state.ledgers()?.read_domain(&label, &scope, |tx| {
                    let $ctx = &Context::system(crate::storage::SqliteSession(tx), &scope)?
                        .without_currency_conversion();
                    ($work).map(Into::into)
                })
            })
            .await
        }
    };
    ($command:ident, $input:ty, $result:ty, |$ctx:ident,$args:ident,$desktop:ident,$window:ident| $work:expr) => {
        #[tauri::command]
        #[specta::specta]
        pub(crate) async fn $command<R: Runtime>(
            window: WebviewWindow<R>,
            state: State<'_, Arc<Desktop>>,
            scope: Scope,
            input: $input,
        ) -> Result<$result> {
            authorize(window.label())?;
            let $window = window.label().to_owned();
            let $desktop = Arc::clone(state.inner());
            let $args = input;
            $crate::ipc::phase_three::blocking_read(move || {
                $desktop.check_module(stringify!($command))?;
                $desktop.ledgers()?.read_domain(&$window, &scope, |tx| {
                    let $ctx = &Context::system(crate::storage::SqliteSession(tx), &scope)?
                        .without_currency_conversion();
                    ($work).map(Into::into)
                })
            })
            .await
        }
    };
}
macro_rules! scoped_write {
    ($command:ident, $input:ty, $result:ty, $affected:expr, |$ctx:ident,$args:ident,$desktop:ident,$window:ident| $work:expr) => {
        #[tauri::command]
        #[specta::specta]
        pub(crate) async fn $command<R: Runtime>(
            window: WebviewWindow<R>,
            state: State<'_, Arc<Desktop>>,
            mutation: Mutation,
            input: $input,
        ) -> Result<Write<$result>> {
            authorize(window.label())?;
            let $window = window.label().to_owned();
            let $desktop = Arc::clone(state.inner());
            let $args = input;
            blocking(move || {
                $desktop.check_module(stringify!($command))?;
                $desktop
                    .ledgers()?
                    .write_domain(&$window, &mutation.scope, |tx| {
                        let context =
                            Context::system(crate::storage::SqliteSession(tx), &mutation.scope)?
                                .without_currency_conversion();
                        context.mutate(stringify!($command), &mutation, &$args, $affected, |$ctx| {
                            $work
                        })
                    })
            })
            .await
        }
    };
}

pub(super) use {scoped_read, scoped_write};

core_engine::ledger_types!();

macro_rules! page_response {
    ($response:ident,$row:ty) => {
        #[derive(serde::Serialize, specta::Type)]
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

pub(super) use page_response;

core_engine::ledger_operations!();
#[tauri::command]
#[specta::specta]
pub(crate) async fn transactions_export_start<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<Desktop>>,
    scope: Scope,
    input: ExportQuery,
) -> Result<JobTicket> {
    authorize(window.label())?;
    let label = window.label().to_owned();
    if state.ledgers()?.session_scope(&label)?.as_ref() != Some(&scope) {
        return Err(AppError::new(
            ErrorCode::StaleSession,
            "stale_ledger_session",
        ));
    }
    let state = Arc::clone(state.inner());
    let target = state
        .grants
        .consume(&label, PathPurpose::TransactionExport, &input.save_grant)?;
    let ticket = state
        .jobs
        .start(&label, Some(&scope), JobKind::TransactionsExport)?;
    let id = ticket.id.clone();
    tauri::async_runtime::spawn(async move {
        let result = {
            let state = state.clone();
            let window = window.clone();
            let id = id.clone();
            blocking(move || {
                state.jobs.running(&window, &id, "exporting")?;
                let value = state
                    .ledgers()?
                    .read_cancellable(&label, &scope, |tx, stopping| {
                        let context = Context::system(crate::storage::SqliteSession(tx), &scope)?
                            .without_currency_conversion();
                        export::export(
                            &context,
                            input.selection.as_ref(),
                            input.filter.as_ref(),
                            &input.columns,
                            &target,
                            || stopping() || state.jobs.is_cancelled(&id),
                            || {
                                if stopping() {
                                    Err(super::jobs::cancelled())
                                } else {
                                    state.jobs.publish(&window, &id)
                                }
                            },
                        )
                    })?;
                Ok(JobOutput::TransactionsExport(value))
            })
            .await
        };
        state.jobs.finish(&window, &id, result);
    });
    Ok(ticket)
}

#[tauri::command]
#[specta::specta]
pub(crate) fn job_status<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<Desktop>>,
    id: String,
    scope: Option<Scope>,
) -> Result<JobSnapshot> {
    let scope = job_scope(window.label(), &state, scope)?;
    state.jobs.status(window.label(), scope.as_ref(), &id)
}
#[tauri::command]
#[specta::specta]
pub(crate) fn job_cancel<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<Desktop>>,
    id: String,
    scope: Option<Scope>,
) -> Result<CancelResult> {
    let scope = job_scope(window.label(), &state, scope)?;
    state.jobs.cancel(&window, scope.as_ref(), &id)
}
fn job_scope(label: &str, state: &Desktop, scope: Option<Scope>) -> Result<Option<Scope>> {
    authorize(label)?;
    if scope.is_some() && state.ledgers()?.session_scope(label)? != scope {
        return Err(AppError::new(
            ErrorCode::StaleSession,
            "stale_ledger_session",
        ));
    }
    Ok(scope)
}
