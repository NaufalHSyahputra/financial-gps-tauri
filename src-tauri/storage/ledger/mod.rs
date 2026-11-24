use crate::ledger_engine::{Context, Deleted, Mutation, Write, export::FileResult, invalid};
use crate::{
    crypto::{
        BackupSecretStore, Credential, KeyWrapper, OsKeychain, QuickUnlockStore, bad_credential,
        native_secret_vault::{NativeSecretVault, TursoSecretStore},
    },
    domain::{
        AppError, CurrencyCode, ErrorCode, LedgerDescriptor, LedgerSession, LifecycleState,
        OpenResult, Protection, Result, Scope, StoragePolicy, UnlockChallenge,
    },
};
use chrono::Utc;
use chrono_tz::Tz;
use parking_lot::{Mutex, MutexGuard, RwLock};
use rusqlite::Transaction;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use uuid::Uuid;
use zeroize::Zeroizing;

use super::{
    attachments::{self, AttachmentSummary, AttachmentWriteResult, Pending, Vault},
    database::Database,
    files::{self, FileIdentity, FileLease, NewLedger},
    grants::ApprovedPath,
    migration::LedgerMetadata,
    preferences::{self, AppPreferences, AppPreferencesPatch},
    registry::{Entry, Registry},
    replacement,
    snapshot::{self, RestoredArchive},
    sync::{self, Replica, SetStorageMode, SyncController, SyncStatus, SyncToken},
};

crate::ledger_engine::dto! {
    pub struct ProtectionStatus {pub protection:Protection,pub quick_unlock:bool}
}
impl crate::ledger_engine::ReceiptValue for ProtectionStatus {
    type Reference = Self;
    fn reference(&self, _: &Context<'_>) -> Self {
        self.clone()
    }
    fn reload(_: &Context<'_>, value: Self) -> Result<Self> {
        Ok(value)
    }
}

pub struct CreateLedger {
    pub name: String,
    pub destination: Option<ApprovedPath>,
    pub base_currency: String,
    pub timezone: Option<String>,
    pub protection: Protection,
    pub credential: Option<Credential>,
    pub remember_key: bool,
}

pub struct ReplacementSource<'a> {
    pub expected_revision: &'a str,
    pub protection: Protection,
    pub credential: Option<Credential>,
    pub safety_password: &'a Credential,
    pub archive: RestoredArchive,
}

pub struct ProtectionCredentials {
    pub protection: Protection,
    pub old: Option<Credential>,
    pub new: Option<Credential>,
    pub safety: Option<Credential>,
}

pub enum OpenTarget {
    Registered(Uuid),
    Chosen(ApprovedPath),
}

struct Selected {
    entry: Entry,
    registered: bool,
    identity: Arc<FileIdentity>,
    window: String,
    challenge: Uuid,
    expires: Instant,
}

impl Selected {
    fn new(entry: Entry, registered: bool, identity: Arc<FileIdentity>, window: &str) -> Self {
        Self {
            entry,
            registered,
            identity,
            window: window.to_owned(),
            challenge: Uuid::new_v4(),
            expires: Instant::now() + Duration::from_secs(300),
        }
    }

    fn descriptor(&self) -> UnlockChallenge {
        UnlockChallenge {
            id: self.challenge.to_string(),
            ledger: self.entry.describe(),
        }
    }

    fn check_identity(&self) -> Result<()> {
        let path = files::checked_path(&self.entry.path, true)?;
        if path != self.entry.path || files::identity(&path)? != *self.identity {
            return Err(AppError::new(ErrorCode::Conflict, "ledger_file_changed"));
        }
        Ok(())
    }
}

struct Active {
    entry: Entry,
    identity: Arc<FileIdentity>,
    window: String,
    session: LedgerSession,
    accepting: AtomicBool,
    database: RwLock<Option<Database>>,
}

enum State {
    Unselected,
    Resolving,
    Locked(Selected),
    Opening,
    Ready(Arc<Active>),
    Quiescing,
}

struct OpenReceipt {
    window: String,
    request: Uuid,
    target: String,
    result: OpenResult,
}

struct Inner {
    registry: Registry,
    state: State,
    generation: u64,
    receipts: VecDeque<OpenReceipt>,
}

pub struct LedgerManager {
    inner: Mutex<Inner>,
    keychain: Arc<dyn QuickUnlockStore>,
    epoch: Uuid,
    backup_secrets: Arc<dyn BackupSecretStore>,
    scheduled_backup: Mutex<()>,
    sync: SyncController,
    sync_vault: Arc<dyn TursoSecretStore>,
    integrity_alerts: Option<tokio::sync::mpsc::Sender<(Scope, AppError)>>,
}

fn stale() -> AppError {
    AppError::new(ErrorCode::StaleSession, "stale_ledger_session")
}

fn unavailable() -> AppError {
    AppError::new(ErrorCode::Locked, "ledger_locked")
}

fn valid_name(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

mod attachment_ops;
mod credentials;
mod lifecycle;
mod maintenance;
mod session;

use attachment_ops::{attachment_root, entry_vault};
#[cfg(test)]
pub(super) use maintenance::ManagedReplica;

impl LedgerManager {
    pub fn new(app_data: &Path) -> Result<Self> {
        Self::with_keychain(app_data, Arc::new(OsKeychain))
    }

    pub fn with_keychain(app_data: &Path, keychain: Arc<dyn QuickUnlockStore>) -> Result<Self> {
        Self::with_secret_stores(app_data, keychain, Arc::new(OsKeychain))
    }

    pub fn with_secret_stores(
        app_data: &Path,
        keychain: Arc<dyn QuickUnlockStore>,
        backup_secrets: Arc<dyn BackupSecretStore>,
    ) -> Result<Self> {
        let mut registry = Registry::open(app_data)?;
        for descriptor in registry.list() {
            let instance = Uuid::parse_str(&descriptor.ledger_instance_id).expect("registry UUID");
            let entry = registry.get(instance)?;
            replacement::Replacement::recover(&mut registry, &entry, &attachment_root(&entry))?;
        }
        Ok(Self {
            inner: Mutex::new(Inner {
                registry,
                state: State::Unselected,
                generation: 0,
                receipts: VecDeque::new(),
            }),
            keychain,
            epoch: Uuid::new_v4(),
            backup_secrets,
            scheduled_backup: Mutex::new(()),
            sync: SyncController::default(),
            sync_vault: Arc::new(NativeSecretVault),
            integrity_alerts: None,
        })
    }

    pub(crate) fn set_integrity_alerts(
        &mut self,
        alerts: tokio::sync::mpsc::Sender<(Scope, AppError)>,
    ) {
        self.integrity_alerts = Some(alerts);
    }

    fn inner(&self) -> Result<MutexGuard<'_, Inner>> {
        self.inner.try_lock().ok_or_else(AppError::busy)
    }

    pub fn preferences(&self) -> Result<AppPreferences> {
        let registry = &self.inner()?.registry;
        preferences::get(&registry.local_state("app-preferences.json"))
    }

    pub(crate) fn require_feature(&self, feature: preferences::Feature) -> Result<()> {
        if !self.preferences()?.enables(feature) {
            return Err(AppError {
                fields: Some(std::collections::BTreeMap::from([(
                    "module".into(),
                    feature.key().into(),
                )])),
                ..AppError::new(ErrorCode::Conflict, "optional_module_disabled")
            });
        }
        Ok(())
    }

    pub fn update_preferences(&self, patch: AppPreferencesPatch) -> Result<AppPreferences> {
        let inner = self
            .inner
            .try_lock_for(Duration::from_secs(2))
            .ok_or_else(AppError::busy)?;
        preferences::set(&inner.registry.local_state("app-preferences.json"), patch)
    }

    pub fn list(&self) -> Result<Vec<LedgerDescriptor>> {
        Ok(self.inner()?.registry.list())
    }
}
