mod image;
mod turso;

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Deserializer, Serialize};
use specta::Type;
use uuid::Uuid;
use zeroize::Zeroizing;

use super::sql_error;
use crate::{
    crypto::native_secret_vault::TursoSecretStore,
    domain::{AppError, ErrorCode, Result, Scope},
};
use turso::Turso;

pub(super) use image::{capture, decode};
pub(super) const MAX_IMAGE: usize = 384 * 1024 * 1024;
const STATE_KEY: &str = "secret.sync.v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "kebab-case")]
pub enum LedgerStorageMode {
    LocalIsolated,
    TursoSync,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum SyncPhase {
    Idle,
    Syncing,
    Offline,
    Error,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub mode: LedgerStorageMode,
    pub url: Option<String>,
    pub phase: SyncPhase,
    pub pending: bool,
    pub pending_count: String,
    pub local_version: Option<String>,
    pub last_success_at: Option<String>,
    pub remote_generation: Option<String>,
    pub error: Option<AppError>,
}

#[derive(Type)]
#[specta(transparent)]
pub struct SyncToken(#[specta(type = String)] Zeroizing<String>);

impl SyncToken {
    pub fn new(value: Zeroizing<String>) -> Result<Self> {
        if value.is_empty() || value.len() > 8192 || !value.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(AppError::validation("authToken", "invalid_turso_token"));
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}
impl<'de> Deserialize<'de> for SyncToken {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        Self::new(Zeroizing::new(String::deserialize(deserializer)?))
            .map_err(|_| serde::de::Error::custom("invalid Turso token"))
    }
}

#[derive(Deserialize, Type)]
#[serde(tag = "mode", rename_all = "kebab-case", deny_unknown_fields)]
pub enum SetStorageMode {
    LocalIsolated,
    TursoSync {
        url: String,
        #[serde(rename = "authToken")]
        auth_token: SyncToken,
    },
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Binding {
    pub(super) url: String,
    device: Uuid,
    local: i64,
    acknowledged: i64,
    base: i64,
    #[serde(default)]
    last_success_at: Option<String>,
}

impl Binding {
    fn pending(&self) -> bool {
        self.local > self.acknowledged
    }
}

pub(super) fn corrupt() -> AppError {
    AppError::new(ErrorCode::Integrity, "invalid_turso_sync_state")
}

fn conflict() -> AppError {
    AppError::new(ErrorCode::Conflict, "turso_concurrent_ledger_edits")
}

pub(super) fn load(conn: &Connection) -> Result<Option<Binding>> {
    let json: Option<String> = conn
        .query_row(
            "SELECT value_json FROM ledger_settings WHERE key=?1",
            [STATE_KEY],
            |r| r.get(0),
        )
        .optional()
        .map_err(sql_error)?;
    json.map(|json| {
        let state: Binding = serde_json::from_str(&json).map_err(|_| corrupt())?;
        if state.device.is_nil()
            || state.base < 0
            || state.acknowledged < 0
            || state.local < state.acknowledged
            || turso::endpoint(&state.url)?.as_str() != state.url
        {
            return Err(corrupt());
        }
        Ok(state)
    })
    .transpose()
}

pub(super) fn store(conn: &Connection, state: &Binding) -> Result<()> {
    conn.execute(
        "INSERT INTO ledger_settings VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json",
        params![STATE_KEY, serde_json::to_string(state).map_err(|_| corrupt())?],
    ).map_err(sql_error)?;
    Ok(())
}

pub(super) fn dirty(conn: &Connection) -> Result<()> {
    if let Some(mut state) = load(conn)? {
        state.local = state.local.checked_add(1).ok_or_else(corrupt)?;
        store(conn, &state)?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct Commit {
    generation: i64,
    id: Uuid,
    device: Uuid,
    version: i64,
    digest: String,
    parts: usize,
}

trait Remote {
    fn probe(&self) -> Result<()>;
    fn initialize(&self) -> Result<()>;
    fn head(&self) -> Result<Option<Commit>>;
    fn latest(&self, device: Uuid) -> Result<Option<Commit>>;
    fn publish(
        &self,
        base: i64,
        device: Uuid,
        version: i64,
        image: &[u8],
        active: &dyn Fn() -> Result<()>,
    ) -> Result<bool>;
    fn pull(&self, commit: &Commit, active: &dyn Fn() -> Result<()>) -> Result<Zeroizing<Vec<u8>>>;
}

#[derive(Clone, Copy)]
enum Resolution {
    Keep(i64),
    Accept { generation: i64, local_version: i64 },
}

#[derive(Clone, Copy)]
pub(super) struct Incoming {
    base: i64,
    generation: i64,
    discard_version: Option<i64>,
}

pub(super) trait Replica {
    fn active(&self) -> Result<()>;
    fn load(&self) -> Result<Option<Binding>>;
    fn update(&self, work: impl FnOnce(&Connection) -> Result<()>) -> Result<()>;
    fn capture(&self) -> Result<(Binding, Zeroizing<Vec<u8>>)>;
    fn apply(&self, incoming: Incoming, bytes: &[u8]) -> Result<()>;
}

#[derive(Default)]
pub(super) struct SyncController {
    gate: Mutex<()>,
    phase: Mutex<Option<(String, SyncPhase, Option<AppError>)>>,
}

impl SyncController {
    pub(super) fn try_gate(&self) -> Result<parking_lot::MutexGuard<'_, ()>> {
        self.gate.try_lock().ok_or_else(AppError::busy)
    }

    pub(super) fn validate(url: &str, token: &SyncToken) -> Result<()> {
        Turso::connect(url, token)?.probe()
    }

    pub(super) fn set(
        &self,
        scope: &Scope,
        managed: bool,
        replica: &impl Replica,
        vault: &dyn TursoSecretStore,
        mode: SetStorageMode,
    ) -> Result<SyncStatus> {
        self.set_using(scope, managed, replica, vault, mode, Turso::connect)
    }

    fn set_using<R: Remote>(
        &self,
        scope: &Scope,
        managed: bool,
        replica: &impl Replica,
        vault: &dyn TursoSecretStore,
        mode: SetStorageMode,
        connect: impl FnOnce(&str, &SyncToken) -> Result<R>,
    ) -> Result<SyncStatus> {
        let _gate = self.try_gate()?;
        let instance = Uuid::parse_str(&scope.ledger_instance_id).map_err(|_| corrupt())?;
        let existing = replica.load()?;
        match mode {
            SetStorageMode::LocalIsolated => {
                if existing.is_some() {
                    vault.revoke(instance)?;
                    replica.update(|conn| {
                        conn.execute("DELETE FROM ledger_settings WHERE key=?1", [STATE_KEY])
                            .map_err(sql_error)?;
                        Ok(())
                    })?;
                }
            }
            SetStorageMode::TursoSync { url, auth_token } => {
                if !managed {
                    return Err(AppError::validation(
                        "mode",
                        "turso_requires_managed_ledger",
                    ));
                }
                let url = turso::endpoint(&url)?.to_string();
                if existing.as_ref().is_some_and(|state| state.url != url) {
                    return Err(AppError::new(
                        ErrorCode::Conflict,
                        "disable_sync_before_changing_endpoint",
                    ));
                }
                let remote = connect(&url, &auth_token)?;
                remote.probe()?;
                remote.initialize()?;
                let head = remote.head()?;
                replica.active()?;
                vault.enroll(instance, auth_token.as_str())?;
                if existing.is_none() {
                    let saved = replica.update(|conn| {
                        let local =
                            i64::from(head.is_none() || super::snapshot::has_user_data(conn)?);
                        store(
                            conn,
                            &Binding {
                                url,
                                device: Uuid::new_v4(),
                                local,
                                acknowledged: 0,
                                base: 0,
                                last_success_at: None,
                            },
                        )
                    });
                    if saved.is_err() {
                        let _ = vault.revoke(instance);
                    }
                    saved?;
                }
            }
        }
        *self.phase.lock() = None;
        self.status(scope, replica)
    }

    pub(super) fn probe_saved(
        &self,
        scope: &Scope,
        replica: &impl Replica,
        vault: &dyn TursoSecretStore,
    ) -> Result<()> {
        let _gate = self.try_gate()?;
        let state = replica
            .load()?
            .ok_or_else(|| AppError::new(ErrorCode::Validation, "turso_not_configured"))?;
        let instance = Uuid::parse_str(&scope.ledger_instance_id).map_err(|_| corrupt())?;
        let token = SyncToken::new(vault.retrieve(instance)?)?;
        replica.active()?;
        Self::validate(&state.url, &token)
    }

    pub(super) fn status(&self, scope: &Scope, replica: &impl Replica) -> Result<SyncStatus> {
        let state = replica.load()?;
        let (phase, error) = self
            .phase
            .lock()
            .as_ref()
            .filter(|(session, _, _)| session == &scope.session_id && state.is_some())
            .map_or((SyncPhase::Idle, None), |(_, phase, error)| {
                (*phase, error.clone())
            });
        Ok(SyncStatus {
            mode: if state.is_some() {
                LedgerStorageMode::TursoSync
            } else {
                LedgerStorageMode::LocalIsolated
            },
            url: state.as_ref().map(|binding| binding.url.clone()),
            phase,
            pending: state.as_ref().is_some_and(Binding::pending),
            pending_count: state
                .as_ref()
                .map_or(0, |state| state.local - state.acknowledged)
                .to_string(),
            local_version: state.as_ref().map(|state| state.local.to_string()),
            remote_generation: state.as_ref().map(|binding| binding.base.to_string()),
            last_success_at: state.and_then(|state| state.last_success_at),
            error,
        })
    }

    pub(super) fn run(
        &self,
        scope: &Scope,
        replica: &impl Replica,
        vault: &dyn TursoSecretStore,
    ) -> Result<SyncStatus> {
        self.run_using(scope, replica, vault, None, Turso::connect)
    }

    pub(super) fn publish_local(
        &self,
        scope: &Scope,
        replica: &impl Replica,
        vault: &dyn TursoSecretStore,
        expected: i64,
    ) -> Result<SyncStatus> {
        self.run_using(
            scope,
            replica,
            vault,
            Some(Resolution::Keep(expected)),
            Turso::connect,
        )
    }

    pub(super) fn accept_remote(
        &self,
        scope: &Scope,
        replica: &impl Replica,
        vault: &dyn TursoSecretStore,
        generation: i64,
        local_version: i64,
    ) -> Result<SyncStatus> {
        self.run_using(
            scope,
            replica,
            vault,
            Some(Resolution::Accept {
                generation,
                local_version,
            }),
            Turso::connect,
        )
    }

    fn run_using<R: Remote>(
        &self,
        scope: &Scope,
        replica: &impl Replica,
        vault: &dyn TursoSecretStore,
        resolution: Option<Resolution>,
        connect: impl FnOnce(&str, &SyncToken) -> Result<R>,
    ) -> Result<SyncStatus> {
        let _gate = self.try_gate()?;
        let Some(state) = replica.load()? else {
            if resolution.is_some() {
                return Err(conflict());
            }
            return self.status(scope, replica);
        };
        *self.phase.lock() = Some((scope.session_id.clone(), SyncPhase::Syncing, None));
        let result = (|| {
            let instance = Uuid::parse_str(&scope.ledger_instance_id).map_err(|_| corrupt())?;
            let token = SyncToken::new(vault.retrieve(instance)?)?;
            replica.active()?;
            synchronize(replica, &connect(&state.url, &token)?, resolution)
        })();
        let phase = match result.as_ref().map_err(|err| err.code) {
            Ok(()) => SyncPhase::Idle,
            Err(ErrorCode::Offline) => SyncPhase::Offline,
            Err(_) => SyncPhase::Error,
        };
        *self.phase.lock() = Some((scope.session_id.clone(), phase, result.err()));
        self.status(scope, replica)
    }
}

fn synchronize(
    replica: &impl Replica,
    remote: &impl Remote,
    resolution: Option<Resolution>,
) -> Result<()> {
    let mut state = replica.load()?.ok_or_else(corrupt)?;
    replica.active()?;
    let head = remote.head()?;
    let generation = head.as_ref().map_or(0, |head| head.generation);

    // Recover a successful remote commit if the response or local acknowledgement was lost.
    let receipt = match &head {
        Some(head) if head.device == state.device => Some(head.clone()),
        Some(_) if state.pending() => {
            replica.active()?;
            remote.latest(state.device)?
        }
        _ => None,
    };
    if let Some(receipt) = receipt
        && receipt.generation > state.base
    {
        if receipt.version <= state.acknowledged || receipt.version > state.local {
            return Err(corrupt());
        }
        acknowledge(replica, receipt.generation, receipt.version)?;
        state = replica.load()?.ok_or_else(corrupt)?;
    }
    if generation < state.base {
        // A head read racing another writer is retried on the next pass.
        return Err(AppError::new(
            ErrorCode::Conflict,
            "turso_remote_history_changed",
        ));
    }
    let discard_version = match resolution {
        Some(Resolution::Keep(expected)) => {
            if generation != expected || generation <= state.base || !state.pending() {
                return Err(conflict_at(generation));
            }
            return publish(replica, remote, state.base, generation);
        }
        Some(Resolution::Accept {
            generation: expected,
            local_version,
        }) => {
            if generation != expected
                || generation <= state.base
                || !state.pending()
                || local_version != state.local
            {
                return Err(conflict_at(generation));
            }
            Some(local_version)
        }
        None => None,
    };
    if generation > state.base {
        if state.pending() && discard_version.is_none() {
            return Err(conflict_at(generation));
        }
        replica.active()?;
        let head = head.as_ref().ok_or_else(corrupt)?;
        let image = remote.pull(head, &|| replica.active())?;
        // Recheck the head after downloading a version the user chose to replace local edits.
        if discard_version.is_some() {
            replica.active()?;
            let current = remote.head()?.map_or(0, |head| head.generation);
            if current != generation {
                return Err(conflict_at(current));
            }
        }
        replica.apply(
            Incoming {
                base: state.base,
                generation,
                discard_version,
            },
            &image,
        )?;
    } else if state.pending() {
        publish(replica, remote, state.base, generation)?;
    }
    Ok(())
}

fn conflict_at(generation: i64) -> AppError {
    AppError {
        fields: Some(std::collections::BTreeMap::from([(
            "remoteGeneration".to_owned(),
            generation.to_string(),
        )])),
        ..conflict()
    }
}

fn publish(replica: &impl Replica, remote: &impl Remote, base: i64, generation: i64) -> Result<()> {
    let (captured, bytes) = replica.capture()?;
    if captured.base != base || !captured.pending() {
        return Err(conflict());
    }
    if !remote.publish(generation, captured.device, captured.local, &bytes, &|| {
        replica.active()
    })? {
        return Err(conflict_at(generation));
    }
    acknowledge(
        replica,
        generation.checked_add(1).ok_or_else(corrupt)?,
        captured.local,
    )
}

fn acknowledge(replica: &impl Replica, generation: i64, version: i64) -> Result<()> {
    replica.update(|conn| {
        let mut state = load(conn)?.ok_or_else(corrupt)?;
        if generation <= state.base || version <= state.acknowledged || version > state.local {
            return Err(corrupt());
        }
        state.base = generation;
        state.acknowledged = version;
        state.last_success_at = Some(chrono::Utc::now().to_rfc3339());
        store(conn, &state)
    })
}

pub(super) fn advance(conn: &Connection, incoming: Incoming) -> Result<Binding> {
    let mut state = load(conn)?.ok_or_else(corrupt)?;
    let unchanged = incoming
        .discard_version
        .map_or_else(|| !state.pending(), |version| state.local == version);
    if state.base != incoming.base || !unchanged {
        return Err(conflict_at(incoming.generation));
    }
    state.base = incoming.generation;
    state.acknowledged = state.local;
    state.last_success_at = Some(chrono::Utc::now().to_rfc3339());
    Ok(state)
}

#[cfg(test)]
mod tests;
