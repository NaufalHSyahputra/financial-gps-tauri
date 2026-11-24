use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use crate::{
    domain::{AppError, ErrorCode, Result, Scope},
    ledger_engine::{Write, dto, export::FileResult, fx::FxRefreshResult, invalid, new_id},
};
use parking_lot::Mutex;
use tauri::{Emitter, Runtime, WebviewWindow};

dto! {
    pub enum JobKind { FxRefresh, TransactionsExport, ImportPreview, ImportCommit, RulePreview, RuleApply, AttachmentAdd, BackupCreate, RestorePreview, RestoreApply, LedgerChangeProtection }
    #[derive(PartialEq, Eq)]
    pub enum JobState { Queued, Running, Publishing, Complete, Failed, Cancelled }
    #[serde(tag="kind",content="value")]
    pub enum JobOutput {
        FxRefresh(Write<FxRefreshResult>), TransactionsExport(FileResult),
        ImportPreview(crate::importers::ImportPreview), ImportCommit(Write<crate::importers::ImportBatch>),
        RulePreview(crate::rules::RulePreview), RuleApply(Write<crate::rules::RuleApplyResult>),
        AttachmentAdd(Write<crate::storage::attachments::AttachmentWriteResult>),
        BackupCreate(crate::storage::backups::BackupSummary),
        RestorePreview(super::phase_four::RestorePreview), RestoreApply(crate::domain::LedgerSession),
        LedgerChangeProtection(crate::domain::LedgerSession),
    }
    pub struct JobTicket { pub id:String, pub kind:JobKind, pub state:JobState }
    pub struct CancelResult { pub id:String, pub cancelled:bool, pub state:JobState }
}
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct JobSnapshot {
    pub id: String,
    pub session_id: Option<String>,
    pub kind: JobKind,
    pub state: JobState,
    pub seq: u32,
    pub done: Option<u32>,
    pub cancellable: bool,
    pub output: Option<JobOutput>,
    pub error: Option<AppError>,
}
#[derive(Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub(super) struct Progress {
    job_id: String,
    session_id: Option<String>,
    seq: u32,
    kind: JobKind,
    phase: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    done: Option<u32>,
    state: JobState,
    cancellable: bool,
}
impl tauri_specta::Event for Progress {
    const NAME: &'static str = "ledger-job-progress";
}
struct Entry {
    scope: Option<Scope>,
    window: String,
    snapshot: JobSnapshot,
    updated: Instant,
}
#[derive(Default)]
pub struct JobStore(Mutex<HashMap<String, Entry>>);

impl JobStore {
    pub fn start(&self, label: &str, scope: Option<&Scope>, kind: JobKind) -> Result<JobTicket> {
        let mut jobs = self.0.lock();
        jobs.retain(|_, job| {
            job.snapshot.cancellable
                || job.snapshot.state == JobState::Publishing
                || job.updated.elapsed() < Duration::from_secs(600)
        });
        if jobs.len() >= 32
            || jobs
                .values()
                .filter(|job| {
                    matches!(
                        job.snapshot.state,
                        JobState::Queued | JobState::Running | JobState::Publishing
                    )
                })
                .count()
                >= 2
        {
            return Err(AppError::busy());
        }
        let id = new_id();
        let snapshot = JobSnapshot {
            id: id.clone(),
            session_id: scope.map(|scope| scope.session_id.clone()),
            kind: kind.clone(),
            state: JobState::Queued,
            seq: 0,
            done: None,
            cancellable: true,
            output: None,
            error: None,
        };
        jobs.insert(
            id.clone(),
            Entry {
                scope: scope.cloned(),
                window: label.to_owned(),
                snapshot,
                updated: Instant::now(),
            },
        );
        Ok(JobTicket {
            id,
            kind,
            state: JobState::Queued,
        })
    }
    pub fn running<R: Runtime>(
        &self,
        window: &WebviewWindow<R>,
        id: &str,
        phase: &'static str,
    ) -> Result<()> {
        self.transition(window, id, JobState::Running, phase, true)
    }
    pub fn publish<R: Runtime>(&self, window: &WebviewWindow<R>, id: &str) -> Result<()> {
        self.transition(window, id, JobState::Publishing, "publishing", false)
    }
    fn transition<R: Runtime>(
        &self,
        window: &WebviewWindow<R>,
        id: &str,
        state: JobState,
        phase: &'static str,
        cancellable: bool,
    ) -> Result<()> {
        let mut jobs = self.0.lock();
        let job = jobs
            .get_mut(id)
            .filter(|job| job.snapshot.cancellable)
            .ok_or_else(cancelled)?;
        job.snapshot.state = state;
        job.snapshot.cancellable = cancellable;
        let update = job.progress(phase);
        drop(jobs);
        let _ = window.emit("ledger-job-progress", update);
        Ok(())
    }
    pub fn import_progress<R: Runtime>(&self, window: &WebviewWindow<R>, id: &str, done: u32) {
        let mut jobs = self.0.lock();
        let Some(job) = jobs.get_mut(id).filter(|job| {
            job.snapshot.state == JobState::Running
                && matches!(job.snapshot.kind, JobKind::ImportPreview)
                && job.window == window.label()
        }) else {
            return;
        };
        job.snapshot.done = Some(done);
        if job.updated.elapsed() < Duration::from_millis(250) {
            return;
        }
        let update = job.progress("parsing");
        drop(jobs);
        let _ = window.emit("ledger-job-progress", update);
    }

    pub fn is_cancelled(&self, id: &str) -> bool {
        self.0
            .lock()
            .get(id)
            .is_none_or(|job| job.snapshot.state == JobState::Cancelled)
    }
    pub fn finish<R: Runtime>(
        &self,
        window: &WebviewWindow<R>,
        id: &str,
        result: Result<JobOutput>,
    ) {
        let mut jobs = self.0.lock();
        let Some(job) = jobs.get_mut(id) else {
            return;
        };
        if job.snapshot.state == JobState::Cancelled {
            return;
        }
        job.snapshot.cancellable = false;
        match result {
            Ok(output) => {
                job.snapshot.state = JobState::Complete;
                job.snapshot.output = Some(output);
            }
            Err(error) => {
                job.snapshot.state = if matches!(
                    error.code,
                    ErrorCode::Cancelled | ErrorCode::StaleSession | ErrorCode::Locked
                ) {
                    JobState::Cancelled
                } else {
                    JobState::Failed
                };
                job.snapshot.error = Some(error);
            }
        }
        let update = job.progress("finished");
        drop(jobs);
        let _ = window.emit("ledger-job-progress", update);
    }
    pub fn status(&self, label: &str, scope: Option<&Scope>, id: &str) -> Result<JobSnapshot> {
        let jobs = self.0.lock();
        let job = jobs
            .get(id)
            .filter(|job| job.window == label && job.scope.as_ref() == scope)
            .ok_or_else(|| invalid("jobId", "job_not_found"))?;
        Ok(job.snapshot.clone())
    }
    pub fn cancel<R: Runtime>(
        &self,
        window: &WebviewWindow<R>,
        scope: Option<&Scope>,
        id: &str,
    ) -> Result<CancelResult> {
        let mut jobs = self.0.lock();
        let job = jobs
            .get_mut(id)
            .filter(|job| job.window == window.label() && job.scope.as_ref() == scope)
            .ok_or_else(|| invalid("jobId", "job_not_found"))?;
        let cancel = job.snapshot.cancellable;
        let progress = cancel.then(|| job.cancel());
        let result = CancelResult {
            id: id.to_owned(),
            cancelled: cancel,
            state: job.snapshot.state.clone(),
        };
        drop(jobs);
        if let Some(progress) = progress {
            let _ = window.emit("ledger-job-progress", progress);
        }
        Ok(result)
    }
    pub fn invalidate(&self, label: &str, scope: &Scope) {
        let mut jobs = self.0.lock();
        jobs.retain(|_, job| job.window != label || job.scope.as_ref() != Some(scope));
    }
}
impl Entry {
    fn progress(&mut self, phase: &'static str) -> Progress {
        self.snapshot.seq += 1;
        self.updated = Instant::now();
        Progress {
            job_id: self.snapshot.id.clone(),
            session_id: self.snapshot.session_id.clone(),
            seq: self.snapshot.seq,
            done: self.snapshot.done,
            kind: self.snapshot.kind.clone(),
            phase,
            state: self.snapshot.state.clone(),
            cancellable: self.snapshot.cancellable,
        }
    }
    fn cancel(&mut self) -> Progress {
        self.snapshot.cancellable = false;
        self.snapshot.state = JobState::Cancelled;
        self.progress("cancelled")
    }
}
pub fn cancelled() -> AppError {
    AppError::new(ErrorCode::Cancelled, "operation_cancelled")
}
