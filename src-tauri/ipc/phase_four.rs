use std::io::{Read, Write as IoWrite};

use super::{
    jobs::{self, JobKind, JobOutput, JobTicket},
    phase_three::{page_response, scoped_read, scoped_write},
    *,
};
use crate::{
    ledger_engine::{
        Context, DateRange, Deleted, Money, Mutation, Page, PageReq, VersionedId, Write,
        corrections, dto, export::FileResult, invalid, revision, staging::Stage,
        transactions::LedgerFilter,
    },
    planning::{assets, calendar, cash_plan, cycle_budgets, loans, recurring},
    rules::{self, *},
    storage::{
        ApprovedPath,
        attachments::{self, AttachmentOpenResult, AttachmentSummary},
        backups::BackupSummary,
        io_error,
        snapshot::{self, BackupManifest, RestoredArchive},
    },
};

#[derive(Default)]
pub(super) struct Stages {
    rules: Stage<RulePlan>,
    opened: parking_lot::Mutex<Vec<(std::time::Instant, tempfile::NamedTempFile)>>,
    restore: parking_lot::Mutex<Option<RestoreStage>>,
}

impl Stages {
    pub(super) fn purge_expired(&self) {
        let now = std::time::Instant::now();
        self.opened.lock().retain(|(expires, _)| *expires > now);
        let mut restore = self.restore.lock();
        if restore.as_ref().is_some_and(|entry| entry.expires <= now) {
            *restore = None;
        }
    }
    pub(super) fn invalidate(&self, scope: &Scope) {
        self.rules.invalidate(scope);
        self.opened.lock().clear();
        let mut restore = self.restore.lock();
        if restore
            .as_ref()
            .is_some_and(|entry| entry.scope.as_ref() == Some(scope))
        {
            *restore = None;
        }
    }
}

core_engine::planning_types!();

core_engine::planning_operations!();

fn read_file(grant: &ApprovedPath, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(grant.revalidate()?)
        .map_err(io_error)?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    grant.revalidate()?;
    if bytes.len() > limit {
        return Err(invalid("file", "import_size_limit"));
    }
    Ok(bytes)
}

fn write_file(
    grant: &ApprovedPath,
    bytes: &[u8],
    count: usize,
    ctx: &Context<'_>,
) -> Result<FileResult> {
    Ok(FileResult {
        file_name: crate::storage::attachments::save_as(grant, bytes)?,
        row_count: count.to_string(),
        revision: revision(&ctx.tx)?,
    })
}

fn start<R: Runtime>(
    window: WebviewWindow<R>,
    desktop: Arc<Desktop>,
    scope: Scope,
    kind: JobKind,
    phase: &'static str,
    operation: impl FnOnce(&Desktop, &WebviewWindow<R>, &str, &str) -> Result<JobOutput>
    + Send
    + 'static,
) -> Result<JobTicket> {
    let label = window.label().to_owned();
    authorize(&label)?;
    if desktop.ledgers()?.session_scope(&label)?.as_ref() != Some(&scope) {
        return Err(AppError::new(ErrorCode::StaleSession, "stale_session"));
    }
    spawn(window, desktop, Some(&scope), kind, phase, operation)
}

fn spawn<R: Runtime>(
    window: WebviewWindow<R>,
    desktop: Arc<Desktop>,
    scope: Option<&Scope>,
    kind: JobKind,
    phase: &'static str,
    operation: impl FnOnce(&Desktop, &WebviewWindow<R>, &str, &str) -> Result<JobOutput>
    + Send
    + 'static,
) -> Result<JobTicket> {
    let label = window.label().to_owned();
    authorize(&label)?;
    let ticket = desktop.jobs.start(&label, scope, kind)?;
    let id = ticket.id.clone();
    tauri::async_runtime::spawn(async move {
        let worker = desktop.clone();
        let worker_window = window.clone();
        let worker_id = id.clone();
        let result = blocking(move || {
            worker.jobs.running(&worker_window, &worker_id, phase)?;
            operation(&worker, &worker_window, &label, &worker_id)
        })
        .await;
        desktop.jobs.finish(&window, &id, result);
    });
    Ok(ticket)
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn rule_preview_start<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<Desktop>>,
    scope: Scope,
    input: RuleStart,
) -> Result<JobTicket> {
    let worker_scope = scope.clone();
    start(
        window,
        Arc::clone(state.inner()),
        scope,
        JobKind::RulePreview,
        "evaluating",
        move |state, _, label, id| {
            state
                .ledgers()?
                .read_cancellable(label, &worker_scope, |tx, stopping| {
                    let ctx = Context::system(crate::storage::SqliteSession(tx), &worker_scope)?;
                    rules::preview(
                        &ctx,
                        &state.phase_four.rules,
                        input.draft_or_ids,
                        input.filter,
                        input.sample_limit,
                        input.override_policy.unwrap_or(OverridePolicy::Preserve),
                        &|| stopping() || state.jobs.is_cancelled(id),
                    )
                    .map(JobOutput::RulePreview)
                })
        },
    )
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn rule_apply_start<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<Desktop>>,
    mutation: Mutation,
    input: RuleApply,
) -> Result<JobTicket> {
    start(
        window,
        Arc::clone(state.inner()),
        mutation.scope.clone(),
        JobKind::RuleApply,
        "evaluating",
        move |state, window, label, id| {
            state.ledgers()?.write_domain(label, &mutation.scope, |tx| {
                let ctx = Context::system(crate::storage::SqliteSession(tx), &mutation.scope)?;
                let result = ctx.mutate(
                    "rule_apply_start",
                    &mutation,
                    &input,
                    &["transactions", "rules"],
                    |ctx| {
                        rules::apply(
                            ctx,
                            &state.phase_four.rules,
                            &input.preview_token,
                            &input.override_policy,
                            &|| state.jobs.is_cancelled(id),
                        )
                    },
                )?;
                state.jobs.publish(window, id)?;
                Ok(JobOutput::RuleApply(result))
            })
        },
    )
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn rule_pack_transfer<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<Desktop>>,
    mutation: Mutation,
    input: RulePackTransfer,
) -> Result<RuleTransferResult> {
    authorize(window.label())?;
    let label = window.label().to_owned();
    let state = Arc::clone(state.inner());
    blocking(move || match input.direction {
        FileDirection::Import => state
            .ledgers()?
            .write_domain(&label, &mutation.scope, |tx| {
                let ctx = Context::system(crate::storage::SqliteSession(tx), &mutation.scope)?;
                ctx.mutate("rule_pack_transfer", &mutation, &input, &["rules"], |ctx| {
                    let grant =
                        state
                            .grants
                            .consume(&label, PathPurpose::RulePackRead, &input.grant)?;
                    let pack = serde_json::from_slice(&read_file(&grant, 1024 * 1024)?)
                        .map_err(|_| invalid("file", "invalid_rule_pack"))?;
                    rules::import_pack(
                        ctx,
                        pack,
                        input
                            .collision_policy
                            .as_ref()
                            .unwrap_or(&CollisionPolicy::Skip),
                    )
                })
                .map(RuleTransferResult::Imported)
            }),
        FileDirection::Export => state.ledgers()?.read_domain(&label, &mutation.scope, |tx| {
            let ctx = Context::system(crate::storage::SqliteSession(tx), &mutation.scope)?;
            let grant = state
                .grants
                .consume(&label, PathPurpose::RulePackWrite, &input.grant)?;
            let pack = rules::export_pack(&ctx, input.rule_ids.as_deref())?;
            let bytes =
                serde_json::to_vec_pretty(&pack).map_err(crate::ledger_engine::codec_error)?;
            write_file(&grant, &bytes, pack.rules.len(), &ctx).map(RuleTransferResult::Exported)
        }),
    })
    .await
}

dto! {
    pub struct AttachmentList {pub transaction_id:String}
    pub struct AttachmentAdd {pub transaction_id:String,pub grants:Vec<String>}
    pub struct AttachmentRename {pub id:String,pub display_name:String}
    pub struct AttachmentSaveAs {pub id:String,pub save_grant:String}
    pub enum RestoreWarning { NewLedgerIdentity, LocalCredentialsExcluded }
    pub struct RestorePreview {pub preview_token:String,pub manifest:BackupManifest,pub warnings:Vec<RestoreWarning>}
    #[serde(tag="kind",rename_all_fields="camelCase")]
    pub enum RestoreTarget { New { name:String, destination:Option<String> }, Replace { target_id:String } }
    pub struct RestoreConfirmationQuery {pub preview_token:String,pub target_id:String}
    pub struct ConfirmToken {pub token:String,pub revision:String}
}

#[derive(Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BackupCreate {
    request_id: String,
    destination: Option<String>,
    backup_passphrase: Credential,
}
#[derive(Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RestorePreviewRequest {
    file_grant: Option<String>,
    scoped_backup_id: Option<String>,
    backup_passphrase: Credential,
}
#[derive(Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RestoreApply {
    request_id: String,
    preview_token: String,
    target: RestoreTarget,
    protection: Protection,
    output_credential: Option<Credential>,
    confirm: Option<String>,
    safety_backup_passphrase: Option<Credential>,
}

struct RestoreStage {
    token: String,
    expires: std::time::Instant,
    scope: Option<Scope>,
    archive: RestoredArchive,
    confirmation: Option<(Scope, ConfirmToken)>,
}

page_response!(BackupPage, BackupSummary);
scoped_read!(
    attachments_list,
    AttachmentList,
    Vec<AttachmentSummary>,
    |ctx, args, _desktop, _window| attachments::list(&ctx.tx, &args.transaction_id)
);
scoped_write!(
    attachment_rename,
    AttachmentRename,
    AttachmentSummary,
    &["attachments"],
    |ctx, args, _desktop, _window| attachments::rename(&ctx.tx, &args.id, &args.display_name)
);
scoped_read!(
    backup_list,
    PageQuery,
    BackupPage,
    |ctx, args, desktop, window| desktop.ledgers()?.backup_list(&window, ctx, &args.page)
);
scoped_write!(
    backup_remove,
    ConfirmedId,
    Deleted,
    &["backups"],
    |ctx, args, desktop, window| desktop.ledgers()?.backup_remove(
        &window,
        ctx,
        &args.id,
        args.confirm.unwrap_or(false)
    )
);

#[tauri::command]
#[specta::specta]
pub(crate) async fn attachment_add_start<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<Desktop>>,
    mutation: Mutation,
    input: AttachmentAdd,
) -> Result<JobTicket> {
    start(
        window,
        Arc::clone(state.inner()),
        mutation.scope.clone(),
        JobKind::AttachmentAdd,
        "staging",
        move |state, window, label, id| {
            state
                .ledgers()?
                .attachment_add(
                    label,
                    &mutation,
                    &input.transaction_id,
                    &input,
                    || {
                        if input.grants.is_empty() || input.grants.len() > 10 {
                            return Err(invalid("grants", "attachment_count_limit"));
                        }
                        input
                            .grants
                            .iter()
                            .map(|grant| {
                                state
                                    .grants
                                    .consume(label, PathPurpose::AttachmentRead, grant)
                            })
                            .collect()
                    },
                    || state.jobs.publish(window, id),
                )
                .map(JobOutput::AttachmentAdd)
        },
    )
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn attachment_delete<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<Desktop>>,
    mutation: Mutation,
    input: ConfirmedId,
) -> Result<Write<Deleted>> {
    authorize(window.label())?;
    let label = window.label().to_owned();
    let state = Arc::clone(state.inner());
    blocking(move || {
        state.ledgers()?.attachment_delete(
            &label,
            &mutation,
            &input.id,
            &input,
            input.confirm.unwrap_or(true),
        )
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn attachment_save_as<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<Desktop>>,
    scope: Scope,
    input: AttachmentSaveAs,
) -> Result<FileResult> {
    authorize(window.label())?;
    let label = window.label().to_owned();
    let state = Arc::clone(state.inner());
    blocking(move || {
        let target =
            state
                .grants
                .consume(&label, PathPurpose::AttachmentWrite, &input.save_grant)?;
        state
            .ledgers()?
            .attachment_save(&label, &scope, &input.id, &target)
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn attachment_open<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<Desktop>>,
    scope: Scope,
    input: Id,
) -> Result<AttachmentOpenResult> {
    authorize(window.label())?;
    let label = window.label().to_owned();
    let state = Arc::clone(state.inner());
    blocking(move || {
        state
            .ledgers()?
            .attachment_bytes(&label, &scope, &input.id, |entry, bytes| {
                let mut opened = state.phase_four.opened.lock();
                if opened.len() >= 20 {
                    return Err(AppError::busy());
                }
                let extension = match entry.media_type.as_str() {
                    "image/jpeg" => ".jpg",
                    "image/png" => ".png",
                    "image/gif" => ".gif",
                    "image/webp" => ".webp",
                    "image/heic" => ".heic",
                    "application/pdf" => ".pdf",
                    _ => return Err(invalid("id", "invalid_attachment_media_type")),
                };
                attachments::validate_signature(&entry.display_name, &bytes)?;
                let mut file = tempfile::Builder::new()
                    .prefix("financial-gps-")
                    .suffix(extension)
                    .tempfile()
                    .map_err(io_error)?;
                file.write_all(&bytes).map_err(io_error)?;
                native_open(file.path())?;
                opened.push((
                    std::time::Instant::now() + std::time::Duration::from_secs(600),
                    file,
                ));
                Ok(AttachmentOpenResult {
                    id: entry.id,
                    opened: true,
                })
            })
    })
    .await
}

fn native_open(path: &std::path::Path) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(io_error)
            .map(|mut child| {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            })
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("/usr/bin/open")
            .arg(path)
            .status()
            .map_err(io_error)?
            .success()
            .then_some(())
            .ok_or_else(|| AppError::new(ErrorCode::Io, "attachment_open_failed"))
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "shell32")]
        unsafe extern "system" {
            fn ShellExecuteW(
                hwnd: isize,
                operation: *const u16,
                file: *const u16,
                parameters: *const u16,
                directory: *const u16,
                show: i32,
            ) -> isize;
        }
        let path = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        // The file argument is a private, application-created path. No command line is interpolated.
        let result = unsafe {
            ShellExecuteW(
                0,
                std::ptr::null(),
                path.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1,
            )
        };
        (result > 32)
            .then_some(())
            .ok_or_else(|| AppError::new(ErrorCode::Io, "attachment_open_failed"))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = path;
        Err(AppError::new(
            ErrorCode::Io,
            "attachment_native_open_unavailable",
        ))
    }
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn backup_create_start<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<Desktop>>,
    scope: Scope,
    input: BackupCreate,
) -> Result<JobTicket> {
    Uuid::parse_str(&input.request_id).map_err(|_| invalid("requestId", "invalid_id"))?;
    let worker_scope = scope.clone();
    start(
        window,
        Arc::clone(state.inner()),
        scope,
        JobKind::BackupCreate,
        "snapshotting",
        move |state, window, label, id| {
            let target = input
                .destination
                .as_deref()
                .map(|grant| state.grants.consume(label, PathPurpose::BackupWrite, grant))
                .transpose()?;
            state
                .ledgers()?
                .backup_create(
                    label,
                    &worker_scope,
                    target.as_ref(),
                    &input.backup_passphrase,
                    &|| state.jobs.is_cancelled(id),
                    || state.jobs.publish(window, id),
                )
                .map(JobOutput::BackupCreate)
        },
    )
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn restore_preview_start<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<Desktop>>,
    input: RestorePreviewRequest,
) -> Result<JobTicket> {
    let label = window.label();
    authorize(label)?;
    let scope = state.ledgers()?.session_scope(label)?;
    spawn(
        window,
        Arc::clone(state.inner()),
        None,
        JobKind::RestorePreview,
        "verifying",
        move |state, _, label, id| {
            let (file, source) = match (input.file_grant, input.scoped_backup_id) {
                (Some(id), None) => {
                    let grant = state.grants.consume(label, PathPurpose::RestoreRead, &id)?;
                    let file = std::fs::File::open(grant.revalidate()?).map_err(io_error)?;
                    grant.revalidate()?;
                    (file, None)
                }
                (None, Some(id)) => {
                    let scope = scope.ok_or_else(|| invalid("scope", "scope_required"))?;
                    (
                        state.ledgers()?.backup_file(label, &scope, &id)?,
                        Some(scope),
                    )
                }
                _ => return Err(invalid("fileGrant", "restore_source_required")),
            };
            let archive = snapshot::open(file, &input.backup_passphrase, &|| {
                state.jobs.is_cancelled(id)
            })?;
            if source.is_some() && state.ledgers()?.session_scope(label)? != source {
                return Err(AppError::new(ErrorCode::StaleSession, "stale_session"));
            }
            if state.jobs.is_cancelled(id) {
                return Err(jobs::cancelled());
            }
            let token = crate::ledger_engine::new_id();
            let manifest = archive.manifest.clone();
            *state.phase_four.restore.lock() = Some(RestoreStage {
                token: token.clone(),
                expires: std::time::Instant::now() + std::time::Duration::from_secs(300),
                scope: source,
                archive,
                confirmation: None,
            });
            Ok(JobOutput::RestorePreview(RestorePreview {
                preview_token: token,
                manifest,
                warnings: vec![
                    RestoreWarning::NewLedgerIdentity,
                    RestoreWarning::LocalCredentialsExcluded,
                ],
            }))
        },
    )
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn restore_confirm_replacement<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<Desktop>>,
    scope: Scope,
    input: RestoreConfirmationQuery,
) -> Result<ConfirmToken> {
    authorize(window.label())?;
    let label = window.label().to_owned();
    let state = Arc::clone(state.inner());
    blocking(move || {
        state.ledgers()?.read_domain(&label, &scope, |tx| {
            if input.target_id != scope.ledger_instance_id {
                return Err(invalid("targetId", "invalid_restore_target"));
            }
            let mut staged = state.phase_four.restore.lock();
            let stage = staged
                .as_mut()
                .filter(|entry| {
                    entry.token == input.preview_token
                        && entry.expires > std::time::Instant::now()
                        && entry.scope.as_ref().is_none_or(|source| source == &scope)
                })
                .ok_or_else(|| invalid("previewToken", "invalid_or_expired_preview"))?;
            let confirmation = ConfirmToken {
                token: crate::ledger_engine::new_id(),
                revision: crate::ledger_engine::native_revision(tx)?,
            };
            stage.confirmation = Some((scope.clone(), confirmation.clone()));
            Ok(confirmation)
        })
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn restore_apply_start<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<Desktop>>,
    input: RestoreApply,
) -> Result<JobTicket> {
    Uuid::parse_str(&input.request_id).map_err(|_| invalid("requestId", "invalid_id"))?;
    spawn(
        window,
        Arc::clone(state.inner()),
        None,
        JobKind::RestoreApply,
        "validating",
        move |state, window, label, id| {
            if (input.protection == Protection::Encrypted) != input.output_credential.is_some() {
                return Err(invalid("outputCredential", "invalid_credential_options"));
            }
            let current = state.ledgers()?.session_scope(label)?;
            let mut staged = state.phase_four.restore.lock();
            let source = staged
                .as_ref()
                .filter(|entry| {
                    entry.token == input.preview_token
                        && entry.expires > std::time::Instant::now()
                        && (entry.scope.is_none() || entry.scope == current)
                })
                .ok_or_else(|| invalid("previewToken", "invalid_or_expired_preview"))?;
            match input.target {
                RestoreTarget::New { name, destination } => {
                    if input.confirm.is_some() || input.safety_backup_passphrase.is_some() {
                        return Err(invalid("target", "invalid_new_restore_options"));
                    }
                    let destination = destination
                        .as_deref()
                        .map(|grant| {
                            state
                                .grants
                                .consume(label, PathPurpose::LedgerCreate, grant)
                        })
                        .transpose()?;
                    state.jobs.publish(window, id)?;
                    let archive = staged.take().expect("validated preview").archive;
                    drop(staged);
                    let draft = CreateLedger {
                        name,
                        destination,
                        base_currency: String::new(),
                        timezone: None,
                        protection: input.protection,
                        credential: input.output_credential,
                        remember_key: false,
                    };
                    state
                        .session_changed(label, || {
                            state.ledgers()?.restore_new(label, draft, archive)
                        })
                        .map(JobOutput::RestoreApply)
                }
                RestoreTarget::Replace { target_id } => {
                    let (scope, confirmed) = source
                        .confirmation
                        .as_ref()
                        .filter(|(scope, confirmed)| {
                            current.as_ref() == Some(scope)
                                && scope.ledger_instance_id == target_id
                                && input.confirm.as_deref() == Some(&confirmed.token)
                        })
                        .ok_or_else(|| invalid("confirm", "confirmation_required"))?;
                    let (scope, revision) = (scope.clone(), confirmed.revision.clone());
                    drop(staged);
                    let password = match input.safety_backup_passphrase {
                        Some(password) => password,
                        None => state.ledgers()?.read_domain(label, &scope, |tx| {
                            state.ledgers()?.scheduled_backup_password(&Context::system(
                                crate::storage::SqliteSession(tx),
                                &scope,
                            )?)
                        })?,
                    };
                    let archive = {
                        let mut staged = state.phase_four.restore.lock();
                        if !staged
                            .as_ref()
                            .is_some_and(|entry| entry.token == input.preview_token)
                        {
                            return Err(invalid("previewToken", "invalid_or_expired_preview"));
                        }
                        staged.take().expect("validated preview").archive
                    };
                    state
                        .session_changed(label, || {
                            state.ledgers()?.restore_replace(
                                label,
                                &scope,
                                crate::storage::ReplacementSource {
                                    expected_revision: &revision,
                                    protection: input.protection,
                                    credential: input.output_credential,
                                    safety_password: &password,
                                    archive,
                                },
                                &|| state.jobs.is_cancelled(id),
                                || state.jobs.publish(window, id),
                            )
                        })
                        .map(JobOutput::RestoreApply)
                }
            }
        },
    )
}

use crate::planning::{budgets, goals, shared};

use crate::storage::schedule::{self, BackupSchedule, ScheduleChange};
#[derive(Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BackupScheduleSet {
    schedule: Option<schedule::Schedule>,
    destination_grant: Option<String>,
    remember_secret: Option<bool>,
    backup_passphrase: Option<Credential>,
}
scoped_read!(backup_schedule_get, BackupSchedule, |ctx| schedule::get(
    ctx
));

#[tauri::command]
#[specta::specta]
pub(crate) async fn backup_schedule_set<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<Desktop>>,
    mutation: Mutation,
    input: BackupScheduleSet,
) -> Result<Write<BackupSchedule>> {
    authorize(window.label())?;
    let label = window.label().to_owned();
    let state = Arc::clone(state.inner());
    blocking(move || {
        let change = ScheduleChange {
            schedule: input.schedule,
            remember_secret: input.remember_secret,
        };
        state.ledgers()?.set_backup_schedule(
            &label,
            &mutation,
            &change,
            input.destination_grant.as_deref(),
            || {
                input
                    .destination_grant
                    .as_deref()
                    .map(|id| {
                        state
                            .grants
                            .consume(&label, PathPurpose::BackupDirectory, id)
                    })
                    .transpose()
            },
            input.backup_passphrase.as_ref(),
        )
    })
    .await
}

use crate::storage::{
    ProtectionStatus,
    preferences::{self, AppPreferencesPatch, LayoutChange, LayoutQuery, LayoutSpec},
};

#[tauri::command]
#[specta::specta]
pub(crate) async fn app_preferences_get<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<Desktop>>,
) -> Result<AppPreferences> {
    authorize(window.label())?;
    let state = Arc::clone(state.inner());
    super::phase_three::blocking_read(move || state.ledgers()?.preferences()).await
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn app_preferences_set<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<Desktop>>,
    patch: AppPreferencesPatch,
) -> Result<AppPreferences> {
    authorize(window.label())?;
    let state = Arc::clone(state.inner());
    blocking(move || state.ledgers()?.update_preferences(patch)).await
}

scoped_read!(
    layout_get,
    LayoutQuery,
    LayoutSpec,
    |ctx, args, _desktop, _window| preferences::layout(ctx, &args.view)
);
scoped_write!(
    layout_set,
    LayoutChange,
    LayoutSpec,
    &["layout"],
    |ctx, args, _desktop, _window| preferences::layout_save(ctx, &args)
);

#[derive(Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProtectionChange {
    protection: Protection,
    old_credential: Option<Credential>,
    new_credential: Option<Credential>,
    backup_passphrase: Option<Credential>,
}
#[derive(Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KeychainChange {
    enabled: bool,
    credential: Option<Credential>,
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn ledger_change_protection<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<Desktop>>,
    mutation: Mutation,
    input: ProtectionChange,
) -> Result<JobTicket> {
    authorize(window.label())?;
    Uuid::parse_str(&mutation.request_id).map_err(|_| invalid("requestId", "invalid_id"))?;
    if state.ledgers()?.session_scope(window.label())?.as_ref() != Some(&mutation.scope) {
        return Err(AppError::new(
            ErrorCode::StaleSession,
            "stale_ledger_session",
        ));
    }
    spawn(
        window,
        Arc::clone(state.inner()),
        None,
        JobKind::LedgerChangeProtection,
        "snapshotting",
        move |state, window, label, id| {
            state
                .session_changed(label, || {
                    state.ledgers()?.change_protection(
                        label,
                        &mutation,
                        crate::storage::ProtectionCredentials {
                            protection: input.protection,
                            old: input.old_credential,
                            new: input.new_credential,
                            safety: input.backup_passphrase,
                        },
                        &|| state.jobs.is_cancelled(id),
                        || state.jobs.publish(window, id),
                    )
                })
                .map(JobOutput::LedgerChangeProtection)
        },
    )
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn ledger_keychain_set<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<Desktop>>,
    mutation: Mutation,
    input: KeychainChange,
) -> Result<Write<ProtectionStatus>> {
    authorize(window.label())?;
    let label = window.label().to_owned();
    let state = Arc::clone(state.inner());
    blocking(move || {
        state
            .ledgers()?
            .set_keychain(&label, &mutation, input.enabled, input.credential.as_ref())
    })
    .await
}
