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

    pub fn session_scope(&self, window: &str) -> Result<Option<Scope>> {
        let inner = self.inner()?;
        Ok(match &inner.state {
            State::Ready(active) if active.window == window => Some(active.session.scope.clone()),
            _ => None,
        })
    }

    pub fn session(&self, window: &str) -> Result<Option<LedgerSession>> {
        let session = {
            let inner = self.inner()?;
            match &inner.state {
                State::Ready(active) if active.window == window => Some(active.session.clone()),
                _ => None,
            }
        };
        let Some(mut session) = session else {
            return Ok(None);
        };
        self.read(window,&session.scope,|tx| {
            (session.name,session.base_currency,session.revision)=tx.query_row(
                "SELECT name,display_currency,CAST(revision AS TEXT) FROM ledger_meta WHERE singleton=1",
                [],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
            )?;
            Ok(())
        })?;
        Ok(Some(session))
    }

    pub fn state(&self) -> Result<LifecycleState> {
        Ok(match self.inner()?.state {
            State::Unselected => LifecycleState::Unselected,
            State::Resolving => LifecycleState::Resolving,
            State::Locked(_) => LifecycleState::Locked,
            State::Opening => LifecycleState::Opening,
            State::Ready(_) => LifecycleState::Ready,
            State::Quiescing => LifecycleState::Quiescing,
        })
    }

    pub fn create(&self, window: &str, draft: CreateLedger) -> Result<LedgerSession> {
        self.create_or_restore(window, draft, None)
    }

    pub fn restore_new(
        &self,
        window: &str,
        mut draft: CreateLedger,
        archive: RestoredArchive,
    ) -> Result<LedgerSession> {
        let (currency, timezone) = archive
            .database
            .query_row(
                "SELECT display_currency,timezone_override FROM ledger_meta",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(crate::storage::sql_error)?;
        draft.base_currency = currency;
        draft.timezone = timezone;
        self.create_or_restore(window, draft, Some(archive))
    }

    pub fn restore_replace(
        &self,
        window: &str,
        scope: &Scope,
        source: ReplacementSource<'_>,
        cancelled: &dyn Fn() -> bool,
        publish: impl FnOnce() -> Result<()>,
    ) -> Result<LedgerSession> {
        self.restore_replace_impl(window, scope, source, cancelled, publish, &[])
    }

    fn restore_replace_impl(
        &self,
        window: &str,
        scope: &Scope,
        source: ReplacementSource<'_>,
        cancelled: &dyn Fn() -> bool,
        publish: impl FnOnce() -> Result<()>,
        local_settings: &[(String, String)],
    ) -> Result<LedgerSession> {
        let _sync_gate = self.sync.try_gate()?;
        let sync_binding = self.read_domain(window, scope, |tx| sync::load(tx))?;
        let ReplacementSource {
            expected_revision,
            protection,
            credential,
            safety_password,
            archive,
        } = source;
        if (protection == Protection::Encrypted) != credential.is_some() {
            return Err(invalid("outputCredential", "invalid_credential_options"));
        }
        let backup =
            self.backup_create(window, scope, None, safety_password, cancelled, || Ok(()))?;
        if backup.source_revision.as_deref() != Some(expected_revision) {
            return Err(crate::ledger_engine::stale_revision());
        }
        let mut inner = self.inner()?;
        let active = inner.scoped(window, scope)?;
        let entry = active.entry.clone();
        let uuid = &active.session.ledger_uuid;
        let (name, currency, timezone) = archive
            .database
            .query_row(
                "SELECT name,display_currency,timezone_override FROM ledger_meta",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .map_err(crate::storage::sql_error)?;
        if !valid_name(&name) {
            return Err(invalid("name", "invalid_ledger_name"));
        }
        let mut staged = replacement::Replacement::begin(
            &inner.registry,
            &entry,
            &attachment_root(&entry),
            protection,
            &name,
            !archive.objects.is_empty(),
        )?;
        let prepare = (|| {
            let path = staged.staged_db().to_owned();
            let mut file = NewLedger::reserve(path.clone())?;
            let lease = FileLease::ledger(&path)?;
            let key = if let Some(credential) = credential {
                let (wrapper, key) = KeyWrapper::enroll(&credential)?;
                file.write_wrapper(&wrapper)?;
                Some(key)
            } else {
                None
            };
            let db = Database::open_with(
                &path,
                lease,
                key.as_ref(),
                Some(LedgerMetadata {
                    uuid,
                    name: &name,
                    timezone: timezone.as_deref(),
                    currency: &currency,
                    created_at_ms: Utc::now().timestamp_millis(),
                }),
                |destination| {
                    snapshot::copy_tables(&archive.database, destination, Some(uuid), cancelled)?;
                    let tx = destination
                        .transaction()
                        .map_err(crate::storage::sql_error)?;
                    for (key, value) in local_settings {
                        tx.execute("INSERT INTO ledger_settings VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json",
                            rusqlite::params![key,value]).map_err(crate::storage::sql_error)?;
                    }
                    if let Some(binding) = sync_binding.as_ref() {
                        sync::store(&tx, binding)?;
                        sync::dirty(&tx)?;
                    }
                    tx.commit().map_err(crate::storage::sql_error)?;
                    Ok(())
                },
            )?;
            let vault = Vault {
                root: staged.staged_vault().to_owned(),
                uuid,
                key: db.attachment_key.as_ref(),
            };
            db.read(|tx| vault.restore(tx, &archive.objects))?;
            db.close_staging();
            file.sync_parent()?;
            file.publish();
            Ok(key)
        })();
        let key = match prepare {
            Ok(key) => key,
            Err(error) => {
                staged.rollback()?;
                return Err(error);
            }
        };
        let generation = inner.next_generation()?;
        if let Err(error) = publish().and_then(|()| inner.quiesce_at(Some(expected_revision))) {
            staged.rollback()?;
            return Err(error);
        }
        inner.state = State::Opening;
        let installed = (|| {
            let _parked_lease = FileLease::ledger(&entry.path)?;
            if files::identity(&entry.path)? != *active.identity {
                return Err(crate::ledger_engine::conflict("ledger_file_changed"));
            }
            staged.swap()?;
            #[cfg(not(unix))]
            drop(_parked_lease); // Windows uses a path-based lease.
            let lease = FileLease::ledger(&entry.path)?;
            let db = Database::open(&entry.path, lease, key.as_ref(), None)?;
            if db.info.uuid != *uuid {
                return Err(crate::ledger_engine::integrity(
                    "restored_ledger_identity_mismatch",
                ));
            }
            let vault = entry_vault(&entry, &db);
            db.read(|tx| {
                for object in &archive.objects {
                    vault.get_bytes(tx, &object.id)?;
                }
                Ok(())
            })?;
            Ok(db)
        })();
        match installed {
            Ok(db) => {
                if let Err(error) = staged.commit(&mut inner.registry, &entry) {
                    db.close();
                    inner.state = State::Unselected;
                    return Err(error);
                }
                if entry.quick_unlock {
                    let _ = self.keychain.revoke(entry.instance);
                }
                let updated = inner.registry.get(entry.instance)?;
                Ok(self.ready(&mut inner, generation, updated, window, db))
            }
            Err(error) => {
                inner.state = State::Unselected;
                staged.rollback()?;
                if entry.protection == Protection::Encrypted {
                    inner.state = State::Locked(Selected::new(
                        entry.clone(),
                        true,
                        Arc::new(files::identity(&entry.path)?),
                        window,
                    ));
                } else {
                    let reopen = (|| {
                        let lease = FileLease::ledger(&entry.path)?;
                        Database::open(&entry.path, lease, None, None)
                    })();
                    if let Ok(db) = reopen {
                        self.ready(&mut inner, generation, entry, window, db);
                    }
                }
                Err(error)
            }
        }
    }

    pub fn change_protection(
        &self,
        window: &str,
        mutation: &Mutation,
        credentials: ProtectionCredentials,
        cancelled: &dyn Fn() -> bool,
        publish: impl FnOnce() -> Result<()>,
    ) -> Result<LedgerSession> {
        let ProtectionCredentials {
            protection,
            old,
            new,
            safety,
        } = credentials;
        let scope = &mutation.scope;
        let (current,local)=self.with_vault(window,scope,|db,_,_|{
            let active=self.inner()?.scoped(window,scope)?;
            if active.entry.protection==Protection::Encrypted {
                let credential=old.as_ref().ok_or_else(||invalid("oldCredential","credential_required"))?;
                let key=files::read_wrapper(&active.entry.path)?.unlock(credential)?;
                let derived=crate::crypto::attachment::derive(&key)?;
                if db.attachment_key.as_ref().is_none_or(|key|**key!=*derived) {return Err(bad_credential());}
            } else if old.is_some(){return Err(invalid("oldCredential","ledger_is_plain"));}
            if protection==Protection::Plain&&active.entry.protection==Protection::Plain {
                return Err(invalid("protection","ledger_is_plain"));
            }
            db.read(|tx|{
                let revision=crate::ledger_engine::native_revision(tx)?;
                if mutation.expected_revision.as_ref().is_some_and(|expected|expected!=&revision){
                    return Err(crate::ledger_engine::stale_revision());
                }
                let mut query=tx.prepare("SELECT key,value_json FROM ledger_settings WHERE key LIKE 'backup.%' OR key LIKE 'fx.%enabled'").map_err(crate::storage::sql_error)?;
                let rows=query.query_map([],|row|Ok((row.get(0)?,row.get(1)?))).map_err(crate::storage::sql_error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(crate::storage::sql_error)?;
                Ok((revision,rows))
            })
        })?;
        let (image, objects) = self.backup_image(window, scope, cancelled)?;
        let archive = snapshot::materialize(&image, objects)?;
        if archive.manifest.source_revision != current {
            return Err(crate::ledger_engine::stale_revision());
        }
        let remembered = self
            .read_domain(window, scope, |tx| {
                self.scheduled_backup_password(&Context::system(
                    crate::storage::SqliteSession(tx),
                    scope,
                )?)
            })
            .ok();
        let fallback = safety
            .as_ref()
            .or(remembered.as_ref())
            .or(old.as_ref())
            .or(new.as_ref())
            .ok_or_else(|| invalid("backupPassphrase", "credential_required"))?;
        let output = new.as_ref().map(Credential::copy);
        self.restore_replace_impl(
            window,
            scope,
            ReplacementSource {
                expected_revision: &current,
                protection,
                credential: output,
                safety_password: fallback,
                archive,
            },
            cancelled,
            publish,
            &local,
        )
    }

    pub fn set_keychain(
        &self,
        window: &str,
        mutation: &Mutation,
        enabled: bool,
        credential: Option<&Credential>,
    ) -> Result<Write<ProtectionStatus>> {
        let mut inner = self.inner()?;
        let active = inner.scoped(window, &mutation.scope)?;
        let slot = active.database.try_read().ok_or_else(AppError::busy)?;
        let db = slot.as_ref().ok_or_else(stale)?;
        let old_entry = inner.registry.get(active.entry.instance)?;
        let mut updated = false;
        let mut result = db.write(|tx| {
            let ctx = Context::system(crate::storage::SqliteSession(tx), &mutation.scope)?;
            ctx.mutate(
                "ledger_keychain_set",
                mutation,
                &enabled,
                &["protection"],
                |_| {
                    if old_entry.protection != Protection::Encrypted {
                        return Err(invalid("enabled", "keychain_requires_encryption"));
                    }
                    if enabled {
                        let credential = credential
                            .ok_or_else(|| invalid("credential", "credential_required"))?;
                        let key = files::read_wrapper(&old_entry.path)?.unlock(credential)?;
                        let derived = crate::crypto::attachment::derive(&key)?;
                        if db
                            .attachment_key
                            .as_ref()
                            .is_none_or(|key| **key != *derived)
                        {
                            return Err(bad_credential());
                        }
                        self.keychain.remember(old_entry.instance, &key)?;
                    } else {
                        self.keychain.revoke(old_entry.instance)?;
                    }
                    updated = true;
                    Ok(ProtectionStatus {
                        protection: old_entry.protection,
                        quick_unlock: enabled,
                    })
                },
            )
        })?;
        if updated || old_entry.quick_unlock != enabled {
            let mut entry = old_entry;
            entry.quick_unlock = enabled;
            inner.registry.replace(entry)?;
        }
        result.value.quick_unlock = enabled;
        Ok(result)
    }

    fn create_or_restore(
        &self,
        window: &str,
        draft: CreateLedger,
        archive: Option<RestoredArchive>,
    ) -> Result<LedgerSession> {
        let name = draft.name.trim();
        if !valid_name(name) {
            return Err(AppError::validation("name", "invalid_ledger_name"));
        }
        let currency = CurrencyCode::parse(&draft.base_currency)?;
        if draft
            .timezone
            .as_ref()
            .is_some_and(|zone| zone.parse::<Tz>().is_err())
        {
            return Err(AppError::validation("timezone", "invalid_iana_timezone"));
        }
        if (draft.protection == Protection::Encrypted) != draft.credential.is_some()
            || (draft.protection == Protection::Plain && draft.remember_key)
        {
            return Err(AppError::validation(
                "credential",
                "invalid_credential_options",
            ));
        }

        let mut inner = self.inner()?;
        let instance = Uuid::new_v4();
        let (path, storage_policy) = match draft.destination {
            Some(ref grant) => (grant.revalidate()?.to_owned(), StoragePolicy::Portable),
            None => (
                inner.registry.managed_path(instance),
                StoragePolicy::Managed,
            ),
        };
        let generation = inner.next_generation()?;
        let mut file = NewLedger::reserve(path.clone())?;
        inner.quiesce()?;
        inner.state = State::Opening;
        let result = (|| {
            let lease = FileLease::ledger(&path)?;
            let key = if let Some(credential) = draft.credential {
                let (wrapper, key) = KeyWrapper::enroll(&credential)?;
                file.write_wrapper(&wrapper)?;
                Some(key)
            } else {
                None
            };
            let uuid = Uuid::new_v4().to_string();
            let db = Database::open_with(
                &path,
                lease,
                key.as_ref(),
                Some(LedgerMetadata {
                    uuid: &uuid,
                    name,
                    timezone: draft.timezone.as_deref(),
                    currency: currency.as_str(),
                    created_at_ms: Utc::now().timestamp_millis(),
                }),
                |writer| {
                    if let Some(source) = archive.as_ref() {
                        snapshot::copy_tables(&source.database, writer, Some(&uuid), &|| false)?;
                        writer
                            .execute("UPDATE ledger_meta SET name=?1 WHERE singleton=1", [name])
                            .map_err(crate::storage::sql_error)?;
                    }
                    Ok(())
                },
            )?;
            file.sync_parent()?;
            let entry = Entry {
                instance,
                path,
                label: name.to_owned(),
                protection: draft.protection,
                storage_policy,
                quick_unlock: draft.remember_key,
            };
            let vault = entry_vault(&entry, &db);
            if let Some(source) = archive.as_ref() {
                db.read(|tx| vault.restore(tx, &source.objects))?;
            }
            if draft.remember_key {
                self.keychain
                    .remember(instance, key.as_ref().expect("validated protection"))?;
            }
            if let Err(error) = inner.registry.add(entry.clone()) {
                if draft.remember_key {
                    let _ = self.keychain.revoke(instance);
                }
                return Err(error);
            }
            file.publish();
            Ok((entry, db))
        })();
        match result {
            Ok((entry, db)) => Ok(self.ready(&mut inner, generation, entry, window, db)),
            Err(error) => {
                inner.state = State::Unselected;
                Err(error)
            }
        }
    }

    pub fn open(
        &self,
        window: &str,
        request: Uuid,
        target_ref: String,
        target: impl FnOnce() -> Result<OpenTarget>,
    ) -> Result<OpenResult> {
        let mut inner = self.inner()?;
        if let Some(receipt) = inner
            .receipts
            .iter()
            .find(|r| r.window == window && r.request == request)
        {
            if receipt.target != target_ref {
                return Err(AppError::new(ErrorCode::Conflict, "request_id_reused"));
            }
            return inner
                .current_open(window)
                .filter(|current| *current == receipt.result)
                .ok_or_else(stale);
        }

        let selected = match target()? {
            OpenTarget::Registered(instance) => {
                let entry = inner.registry.get(instance)?;
                let path = files::checked_path(&entry.path, true)?;
                let known = inner.registry.find(&path)?;
                if known.as_ref().map(|entry| entry.instance) != Some(instance) {
                    return Err(AppError::new(ErrorCode::Conflict, "ledger_file_changed"));
                }
                Self::resolve(window, &path, Some(entry), inner.active_at(&path))?
            }
            OpenTarget::Chosen(grant) => {
                let path = grant.revalidate()?;
                Self::resolve(
                    window,
                    path,
                    inner.registry.find(path)?,
                    inner.active_at(path),
                )?
            }
        };
        let generation = inner.next_generation()?;
        inner.quiesce()?;
        inner.state = State::Resolving;

        if selected.entry.protection == Protection::Encrypted {
            inner.state = State::Locked(selected);
        } else {
            inner.state = State::Opening;
            let result = (|| {
                selected.check_identity()?;
                let lease = FileLease::ledger(&selected.entry.path)?;
                selected.check_identity()?;
                let db = Database::reopen(&selected.entry.path, lease, None)?;
                db.write(|tx| entry_vault(&selected.entry, &db).recover(tx))?;
                let entry = inner.register(&selected, &db)?;
                Ok((entry, db))
            })();
            match result {
                Ok((entry, db)) => {
                    self.ready(&mut inner, generation, entry, window, db);
                }
                Err(error) => {
                    inner.state = State::Unselected;
                    return Err(error);
                }
            }
        }
        let result = inner
            .current_open(window)
            .expect("open returned Ready or Locked");
        if inner.receipts.len() == 32 {
            inner.receipts.pop_front();
        }
        inner.receipts.push_back(OpenReceipt {
            window: window.to_owned(),
            request,
            target: target_ref,
            result: result.clone(),
        });
        Ok(result)
    }

    fn resolve(
        window: &str,
        path: &Path,
        registered: Option<Entry>,
        active: Option<&Active>,
    ) -> Result<Selected> {
        if let Some(active) = active {
            let selected =
                Selected::new(active.entry.clone(), true, active.identity.clone(), window);
            selected.check_identity()?;
            return Ok(selected);
        }

        let _lease = FileLease::ledger(path)?;
        let identity = Arc::new(files::identity(path)?);
        let protection = match (files::is_plain(path)?, files::has_wrapper(path)?) {
            (true, false) => Protection::Plain,
            (false, true) => Protection::Encrypted,
            (true, true) | (false, false) => return Err(bad_credential()),
        };
        if registered
            .as_ref()
            .is_some_and(|entry| entry.protection != protection)
        {
            return Err(bad_credential());
        }
        let known = registered.is_some();
        let entry = registered.unwrap_or_else(|| Entry {
            instance: Uuid::new_v4(),
            path: path.to_owned(),
            label: path
                .file_name()
                .expect("validated file path")
                .to_string_lossy()
                .chars()
                .filter(|ch| !ch.is_control())
                .take(64)
                .collect(),
            protection,
            storage_policy: StoragePolicy::Portable,
            quick_unlock: false,
        });
        let selected = Selected::new(entry, known, identity, window);
        selected.check_identity()?;
        Ok(selected)
    }

    pub fn unlock(
        &self,
        window: &str,
        challenge: &str,
        credential: Option<Credential>,
    ) -> Result<LedgerSession> {
        let mut inner = self.inner()?;
        let State::Locked(selected) = &inner.state else {
            return Err(stale());
        };
        if selected.window != window
            || Uuid::parse_str(challenge).ok() != Some(selected.challenge)
            || selected.expires <= Instant::now()
        {
            return Err(stale());
        }
        let generation = inner.next_generation()?;
        let State::Locked(selected) = std::mem::replace(&mut inner.state, State::Opening) else {
            unreachable!("state validated under the same mutex");
        };
        let result = (|| {
            selected.check_identity()?;
            let lease = FileLease::ledger(&selected.entry.path)?;
            selected.check_identity()?;
            let wrapper = files::read_wrapper(&selected.entry.path)?;
            let key = if let Some(credential) = credential {
                wrapper.unlock(&credential)?
            } else if selected.entry.quick_unlock {
                self.keychain.retrieve(selected.entry.instance)?
            } else {
                return Err(AppError::validation("credential", "credential_required"));
            };
            let db = Database::reopen(&selected.entry.path, lease, Some(&key))?;
            db.write(|tx| entry_vault(&selected.entry, &db).recover(tx))?;
            let entry = inner.register(&selected, &db)?;
            Ok((entry, db))
        })();
        match result {
            Ok((entry, db)) => Ok(self.ready(&mut inner, generation, entry, window, db)),
            Err(error) => {
                inner.state = State::Locked(selected);
                Err(error)
            }
        }
    }

    fn ready(
        &self,
        inner: &mut Inner,
        generation: u64,
        entry: Entry,
        window: &str,
        mut db: Database,
    ) -> LedgerSession {
        let mut generator = Sha256::new();
        generator.update(self.epoch.as_bytes());
        generator.update(generation.to_be_bytes());
        let mut opaque_id = [0; 16];
        opaque_id.copy_from_slice(&generator.finalize()[..16]);
        let scope = Scope {
            session_id: Uuid::from_bytes(opaque_id).to_string(),
            ledger_instance_id: entry.instance.to_string(),
        };
        let session = LedgerSession {
            scope,
            ledger_uuid: db.info.uuid.clone(),
            name: db.info.name.clone(),
            base_currency: db.info.base_currency.clone(),
            revision: db.info.revision.clone(),
            protection: entry.protection,
        };
        let alerts = self.integrity_alerts.clone();
        let alert_scope = session.scope.clone();
        db.start_verification(move |error| {
            if let Some(alerts) = &alerts {
                let _ = alerts.try_send((alert_scope.clone(), error));
            }
        });
        inner.state = State::Ready(Arc::new(Active {
            entry,
            identity: db.identity.clone(),
            window: window.to_owned(),
            session: session.clone(),
            accepting: AtomicBool::new(true),
            database: RwLock::new(Some(db)),
        }));
        session
    }

    pub fn lock(&self, window: &str, scope: &Scope) -> Result<LifecycleState> {
        let mut inner = self.inner()?;
        let active = inner.scoped(window, scope)?;
        let selected = Selected::new(
            inner.registry.get(active.entry.instance)?,
            true,
            active.identity.clone(),
            window,
        );
        inner.quiesce()?;
        inner.state = State::Locked(selected);
        Ok(LifecycleState::Locked)
    }

    pub fn close(&self, window: &str, scope: &Scope) -> Result<LifecycleState> {
        let mut inner = self.inner()?;
        inner.scoped(window, scope)?;
        inner.quiesce()?;
        Ok(LifecycleState::Unselected)
    }

    pub fn integrity_verified(&self, window: &str, scope: &Scope) -> Result<bool> {
        self.with_database(window, scope, |database, _| database.integrity_verified())
    }

    pub(crate) fn shutdown(&self) {
        let _ = self.inner.lock().quiesce();
    }

    pub fn read<T>(
        &self,
        window: &str,
        scope: &Scope,
        work: impl FnOnce(&Transaction<'_>) -> rusqlite::Result<T>,
    ) -> Result<T> {
        self.with_database(window, scope, |database, _| {
            database.read(|tx| work(tx).map_err(crate::storage::sql_error))
        })
    }

    pub fn write<T>(
        &self,
        window: &str,
        scope: &Scope,
        work: impl FnOnce(&Transaction<'_>) -> rusqlite::Result<T>,
    ) -> Result<T> {
        self.with_database(window, scope, |database, _| {
            database.write(|tx| work(tx).map_err(crate::storage::sql_error))
        })
    }

    pub fn read_domain<T>(
        &self,
        window: &str,
        scope: &Scope,
        work: impl FnOnce(&Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        self.with_database(window, scope, |database, _| database.read(work))
    }

    pub fn write_domain<T>(
        &self,
        window: &str,
        scope: &Scope,
        work: impl FnOnce(&Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        self.with_database(window, scope, |database, _| database.write(work))
    }

    pub fn read_cancellable<T>(
        &self,
        window: &str,
        scope: &Scope,
        work: impl FnOnce(&Transaction<'_>, &dyn Fn() -> bool) -> Result<T>,
    ) -> Result<T> {
        self.with_database(window, scope, |database, accepting| {
            database.read(|tx| work(tx, &|| !accepting.load(Ordering::Acquire)))
        })
    }

    pub fn attachment_add(
        &self,
        window: &str,
        mutation: &Mutation,
        transaction: &str,
        input: &impl Serialize,
        grants: impl FnOnce() -> Result<Vec<ApprovedPath>>,
        publish: impl FnOnce() -> Result<()>,
    ) -> Result<Write<AttachmentWriteResult>> {
        self.with_vault(window, &mutation.scope, |db, vault, _| {
            let mut prepared = Vec::new();
            let result = db.write(|tx| {
                let ctx = Context::system(crate::storage::SqliteSession(tx), &mutation.scope)?;
                ctx.mutate(
                    "attachment_add_start",
                    mutation,
                    input,
                    &["attachments"],
                    |ctx| {
                        let grants = grants()?;
                        if grants.is_empty() || grants.len() > 10 {
                            return Err(invalid("grants", "attachment_count_limit"));
                        }
                        for grant in &grants {
                            prepared.push(Pending::read(grant)?);
                        }
                        attachments::insert(&ctx.tx, transaction, &prepared, ctx.now_ms)?;
                        for file in &prepared {
                            vault.prepare(file)?;
                        }
                        let rows = prepared
                            .iter()
                            .map(|file| attachments::get(&ctx.tx, &file.id))
                            .collect::<Result<_>>()?;
                        publish()?;
                        Ok(AttachmentWriteResult { attachments: rows })
                    },
                )
            });
            let mut result = match result {
                Ok(value) => value,
                Err(error) => {
                    for file in &prepared {
                        vault.abort(file);
                    }
                    return Err(error);
                }
            };
            if !prepared.is_empty() {
                for file in &prepared {
                    vault.promote(file)?;
                }
                db.write(|tx| attachments::mark_ready(tx, &prepared))?;
            } else if result
                .value
                .attachments
                .iter()
                .any(|row| matches!(row.state, attachments::AttachmentState::Staged))
            {
                db.write(|tx| vault.recover(tx))?;
            }
            result.value.attachments = db.read(|tx| {
                result
                    .value
                    .attachments
                    .iter()
                    .map(|row| attachments::get(&crate::storage::sqlite_view(tx), &row.id))
                    .collect()
            })?;
            Ok(result)
        })
    }

    pub fn attachment_delete(
        &self,
        window: &str,
        mutation: &Mutation,
        id: &str,
        input: &impl Serialize,
        confirmed: bool,
    ) -> Result<Write<Deleted>> {
        self.with_vault(window, &mutation.scope, |db, vault, _| {
            let result = db.write(|tx| {
                Context::system(crate::storage::SqliteSession(tx), &mutation.scope)?.mutate(
                    "attachment_delete",
                    mutation,
                    input,
                    &["attachments"],
                    |ctx| {
                        if !confirmed {
                            return Err(invalid("confirm", "confirmation_required"));
                        }
                        attachments::mark_deleting(&ctx.tx, id)?;
                        Ok(Deleted { id: id.to_owned() })
                    },
                )
            })?;
            db.write(|tx| {
                if tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM attachments WHERE id=?1 AND state='deleting')",
                        [id],
                        |r| r.get::<_, bool>(0),
                    )
                    .map_err(crate::storage::sql_error)?
                {
                    vault.delete_blob(tx, id)?;
                    tx.execute("DELETE FROM attachments WHERE id=?1", [id])
                        .map_err(crate::storage::sql_error)?;
                }
                Ok(())
            })?;
            Ok(result)
        })
    }

    pub fn attachment_save(
        &self,
        window: &str,
        scope: &Scope,
        id: &str,
        target: &ApprovedPath,
    ) -> Result<FileResult> {
        self.with_vault(window, scope, |db, vault, _| {
            db.read(|tx| {
                let (_, bytes) = vault.get_bytes(tx, id)?;
                Ok(FileResult {
                    file_name: attachments::save_as(target, &bytes)?,
                    row_count: "1".to_owned(),
                    revision: crate::ledger_engine::native_revision(tx)?,
                })
            })
        })
    }

    pub fn attachment_bytes<T>(
        &self,
        window: &str,
        scope: &Scope,
        id: &str,
        work: impl FnOnce(AttachmentSummary, Zeroizing<Vec<u8>>) -> Result<T>,
    ) -> Result<T> {
        self.with_vault(window, scope, |db, vault, _| {
            db.read(|tx| {
                let (summary, bytes) = vault.get_bytes(tx, id)?;
                work(summary, bytes)
            })
        })
    }

    fn backup_root(&self, window: &str, scope: &Scope) -> Result<PathBuf> {
        let inner = self.inner()?;
        let active = inner.scoped(window, scope)?;
        let managed = inner.registry.managed_path(active.entry.instance);
        Ok(managed
            .parent()
            .and_then(Path::parent)
            .expect("managed registry root")
            .join("backups")
            .join(active.entry.instance.to_string()))
    }

    fn custom_backup_root(path: &Path, scope: &Scope) -> Result<PathBuf> {
        let chosen = files::checked_directory(path)?;
        if chosen != path {
            return Err(invalid(
                "destinationGrant",
                "scheduled_backup_directory_changed",
            ));
        }
        Ok(chosen.join(format!("financial-gps-{}", scope.ledger_instance_id)))
    }

    fn backup_roots(&self, window: &str, ctx: &Context<'_>) -> Result<Vec<PathBuf>> {
        let mut roots = vec![self.backup_root(window, ctx.scope)?];
        if let Some(path) = super::schedule::destination_root(ctx)? {
            roots.push(Self::custom_backup_root(&path, ctx.scope)?);
        }
        Ok(roots)
    }

    fn backup_location(&self, window: &str, ctx: &Context<'_>, id: &str) -> Result<PathBuf> {
        for root in self.backup_roots(window, ctx)? {
            if super::backups::contains(&root, id)? {
                return Ok(root);
            }
        }
        Err(crate::ledger_engine::missing("backupId"))
    }

    pub fn backup_list(
        &self,
        window: &str,
        ctx: &Context<'_>,
        page: &crate::ledger_engine::PageReq,
    ) -> Result<crate::ledger_engine::Page<super::backups::BackupSummary>> {
        let cursor = crate::ledger_engine::ReadPage::new(ctx, page, &"backup_list")?;
        let after = cursor.after.as_ref().map(|(_, id)| id.as_str());
        let mut backups = Vec::new();
        for root in self.backup_roots(window, ctx)? {
            backups.extend(super::backups::list(&root)?);
        }
        backups.sort_unstable_by(|a, b| b.id.cmp(&a.id));
        backups.dedup_by(|a, b| a.id == b.id);
        let rows = backups
            .into_iter()
            .filter(|row| after.is_none_or(|after| row.id.as_str() < after))
            .take(cursor.limit + 1)
            .collect();
        cursor.finish(rows, |row| (row.id.clone(), row.id.clone()))
    }

    pub fn backup_remove(
        &self,
        window: &str,
        ctx: &Context<'_>,
        id: &str,
        confirmed: bool,
    ) -> Result<Deleted> {
        if !confirmed {
            return Err(invalid("confirm", "confirmation_required"));
        }
        let root = self.backup_location(window, ctx, id)?;
        super::backups::remove(&root, id)?;
        Ok(Deleted { id: id.to_owned() })
    }

    pub fn backup_file(&self, window: &str, scope: &Scope, id: &str) -> Result<std::fs::File> {
        self.read_domain(window, scope, |tx| {
            let ctx = Context::system(crate::storage::SqliteSession(tx), scope)?;
            super::backups::file(&self.backup_location(window, &ctx, id)?, id)
        })
    }

    pub fn backup_create(
        &self,
        window: &str,
        scope: &Scope,
        target: Option<&ApprovedPath>,
        password: &Credential,
        cancelled: &dyn Fn() -> bool,
        publish: impl FnOnce() -> Result<()>,
    ) -> Result<super::backups::BackupSummary> {
        let root = self.backup_root(window, scope)?;
        let (image, objects) = self.backup_image(window, scope, cancelled)?;
        super::backups::create(&root, target, &image, &objects, password, cancelled, || {
            self.with_database(window, scope, |_, _| publish())
        })
    }

    pub fn backup_image(
        &self,
        window: &str,
        scope: &Scope,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(Zeroizing<Vec<u8>>, Vec<snapshot::Object>)> {
        self.with_vault(window, scope, |db, vault, stopping| {
            db.write(|tx| vault.recover(tx))?;
            db.read(|tx| {
                let image = snapshot::snapshot(tx, db.attachment_key.is_some(), &|| {
                    stopping() || cancelled()
                })?;
                if stopping() || cancelled() {
                    return Err(AppError::new(ErrorCode::Cancelled, "operation_cancelled"));
                }
                Ok((image, vault.inventory(tx)?))
            })
        })
    }

    pub fn run_recurring_generation(&self, window: &str) -> Result<usize> {
        if !self.preferences()?.enables(preferences::Feature::Recurring) {
            return Ok(0);
        }
        let Some(scope) = self.session_scope(window)? else {
            return Ok(0);
        };
        self.write_domain(window, &scope, |tx| {
            crate::planning::recurring::generate_due(&Context::system(
                crate::storage::SqliteSession(tx),
                &scope,
            )?)
        })
    }

    pub fn set_backup_schedule(
        &self,
        window: &str,
        mutation: &Mutation,
        change: &super::schedule::ScheduleChange,
        grant_id: Option<&str>,
        destination: impl FnOnce() -> Result<Option<ApprovedPath>>,
        credential: Option<&Credential>,
    ) -> Result<Write<super::schedule::BackupSchedule>> {
        self.write_domain(window, &mutation.scope, |tx| {
            let ctx = Context::system(crate::storage::SqliteSession(tx), &mutation.scope)?;
            ctx.mutate(
                "backup_schedule_set",
                mutation,
                &(change, grant_id),
                &["backups"],
                |ctx| {
                    let destination = destination()?;
                    super::schedule::set(
                        ctx,
                        change,
                        credential,
                        self.backup_secrets.as_ref(),
                        destination.as_ref(),
                    )
                },
            )
        })
    }
    pub(crate) fn scheduled_backup_password(&self, ctx: &Context<'_>) -> Result<Credential> {
        super::schedule::password(ctx, self.backup_secrets.as_ref())
    }

    pub fn run_scheduled_backup(&self, window: &str) -> Result<()> {
        self.run_scheduled_backup_at(window, &crate::domain::SystemClock)
    }

    pub fn run_scheduled_backup_at(
        &self,
        window: &str,
        clock: &impl crate::domain::Clock,
    ) -> Result<()> {
        let _guard = self
            .scheduled_backup
            .try_lock()
            .ok_or_else(AppError::busy)?;
        let Some(scope) = self.session_scope(window)? else {
            return Ok(());
        };
        let run = self.read_domain(window, &scope, |tx| {
            super::schedule::due(&Context::new(
                crate::storage::SqliteSession(tx),
                &scope,
                clock,
            )?)
        })?;
        let Some(run) = run else {
            return Ok(());
        };
        let password = self.read_domain(window, &scope, |tx| {
            self.scheduled_backup_password(&Context::new(
                crate::storage::SqliteSession(tx),
                &scope,
                clock,
            )?)
        });
        let result = password.and_then(|password| {
            let root = match run.destination.as_deref() {
                Some(path) => Self::custom_backup_root(path, &scope)?,
                None => self.backup_root(window, &scope)?,
            };
            let (image, objects) = self.backup_image(window, &scope, &|| {
                self.session_scope(window).ok().flatten().as_ref() != Some(&scope)
            })?;
            let result = super::backups::create(
                &root,
                None,
                &image,
                &objects,
                &password,
                &|| self.session_scope(window).ok().flatten().as_ref() != Some(&scope),
                || {
                    self.read_domain(window, &scope, |tx| {
                        super::schedule::approve(
                            &Context::new(crate::storage::SqliteSession(tx), &scope, clock)?,
                            &run,
                        )
                    })
                },
            )?;
            if let Some(days) = run.retain_days {
                let cutoff = clock.now() - chrono::Duration::days(i64::from(days));
                for old in super::backups::list(&root)?
                    .into_iter()
                    .filter(|item| item.id != result.id)
                {
                    if chrono::DateTime::parse_from_rfc3339(&old.created_at)
                        .is_ok_and(|date| date < cutoff)
                    {
                        super::backups::remove(&root, &old.id)?;
                    }
                }
            }
            Ok(result)
        });
        self.write_domain(window, &scope, |tx| {
            super::schedule::finish(
                &Context::new(crate::storage::SqliteSession(tx), &scope, clock)?,
                &run,
                &result,
            )
        })?;
        result.map(|_| ())
    }

    pub fn validate_turso_credentials(url: &str, token: &SyncToken) -> Result<()> {
        SyncController::validate(url, token)
    }

    pub fn storage_mode_set(
        &self,
        window: &str,
        scope: &Scope,
        mode: SetStorageMode,
    ) -> Result<SyncStatus> {
        let managed =
            self.inner()?.scoped(window, scope)?.entry.storage_policy == StoragePolicy::Managed;
        self.sync.set(
            scope,
            managed,
            &ManagedReplica {
                manager: self,
                window,
                scope,
            },
            self.sync_vault.as_ref(),
            mode,
        )
    }

    pub fn sync_probe_saved(&self, window: &str, scope: &Scope) -> Result<()> {
        self.sync.probe_saved(
            scope,
            &ManagedReplica {
                manager: self,
                window,
                scope,
            },
            self.sync_vault.as_ref(),
        )
    }

    pub fn sync_status(&self, window: &str, scope: &Scope) -> Result<SyncStatus> {
        self.sync.status(
            scope,
            &ManagedReplica {
                manager: self,
                window,
                scope,
            },
        )
    }

    pub fn sync_now(&self, window: &str, scope: &Scope) -> Result<SyncStatus> {
        self.sync.run(
            scope,
            &ManagedReplica {
                manager: self,
                window,
                scope,
            },
            self.sync_vault.as_ref(),
        )
    }

    pub fn sync_publish_local(
        &self,
        window: &str,
        scope: &Scope,
        expected_generation: &str,
    ) -> Result<SyncStatus> {
        let expected = expected_generation
            .parse::<i64>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| invalid("remoteGeneration", "invalid_sync_generation"))?;
        self.sync.publish_local(
            scope,
            &ManagedReplica {
                manager: self,
                window,
                scope,
            },
            self.sync_vault.as_ref(),
            expected,
        )
    }

    pub fn sync_accept_remote(
        &self,
        window: &str,
        scope: &Scope,
        generation: &str,
        local_version: &str,
    ) -> Result<SyncStatus> {
        let parse = |value: &str, field| {
            value
                .parse::<i64>()
                .ok()
                .filter(|value| *value > 0)
                .ok_or_else(|| invalid(field, "invalid_sync_generation"))
        };
        self.sync.accept_remote(
            scope,
            &ManagedReplica {
                manager: self,
                window,
                scope,
            },
            self.sync_vault.as_ref(),
            parse(generation, "remoteGeneration")?,
            parse(local_version, "localVersion")?,
        )
    }

    fn with_vault<T>(
        &self,
        window: &str,
        scope: &Scope,
        work: impl FnOnce(&Database, &Vault<'_>, &dyn Fn() -> bool) -> Result<T>,
    ) -> Result<T> {
        let active = self.inner()?.scoped(window, scope)?;
        let slot = active.database.try_read().ok_or_else(AppError::busy)?;
        if !active.accepting.load(Ordering::Acquire) {
            return Err(stale());
        }
        let db = slot.as_ref().ok_or_else(stale)?;
        let _fence = db.file_fence.try_lock().ok_or_else(AppError::busy)?;
        work(db, &entry_vault(&active.entry, db), &|| {
            !active.accepting.load(Ordering::Acquire)
        })
    }

    fn with_database<T>(
        &self,
        window: &str,
        scope: &Scope,
        work: impl FnOnce(&Database, &AtomicBool) -> Result<T>,
    ) -> Result<T> {
        let active = self.inner()?.scoped(window, scope)?;
        let slot = active.database.try_read().ok_or_else(AppError::busy)?;
        if !active.accepting.load(Ordering::Acquire) {
            return Err(stale());
        }
        work(slot.as_ref().ok_or_else(stale)?, &active.accepting)
    }
}

pub(super) struct ManagedReplica<'a> {
    pub(super) manager: &'a LedgerManager,
    pub(super) window: &'a str,
    pub(super) scope: &'a Scope,
}

impl Replica for ManagedReplica<'_> {
    fn active(&self) -> Result<()> {
        self.manager
            .with_database(self.window, self.scope, |_, _| Ok(()))
    }

    fn load(&self) -> Result<Option<sync::Binding>> {
        self.manager
            .read_domain(self.window, self.scope, |tx| sync::load(tx))
    }

    fn update(&self, work: impl FnOnce(&rusqlite::Connection) -> Result<()>) -> Result<()> {
        self.manager
            .with_database(self.window, self.scope, |db, _| {
                db.write_sync_metadata(|tx| work(tx))
            })
    }

    fn capture(&self) -> Result<(sync::Binding, Zeroizing<Vec<u8>>)> {
        self.manager
            .with_vault(self.window, self.scope, |db, vault, stopping| {
                db.read(|tx| {
                    let state = sync::load(tx)?.ok_or_else(sync::corrupt)?;
                    Ok((state, sync::capture(tx, vault, stopping)?))
                })
            })
    }

    fn apply(&self, incoming: sync::Incoming, bytes: &[u8]) -> Result<()> {
        let archive = sync::decode(bytes)?;
        self.manager
            .with_vault(self.window, self.scope, |db, vault, stopping| {
                db.install_sync_image(&archive, vault, incoming, stopping)
            })
    }
}

fn attachment_root(entry: &Entry) -> PathBuf {
    match entry.storage_policy {
        StoragePolicy::Managed => entry
            .path
            .parent()
            .and_then(Path::parent)
            .expect("managed ledger path")
            .join("attachments")
            .join(entry.instance.to_string()),
        StoragePolicy::Portable => files::sibling(&entry.path, ".attachments"),
    }
}
fn entry_vault<'a>(entry: &Entry, db: &'a Database) -> Vault<'a> {
    Vault {
        root: attachment_root(entry),
        uuid: &db.info.uuid,
        key: db.attachment_key.as_ref(),
    }
}

impl Drop for LedgerManager {
    fn drop(&mut self) {
        let _ = self.inner.get_mut().quiesce();
    }
}

impl Inner {
    fn active_at(&self, path: &Path) -> Option<&Active> {
        match &self.state {
            State::Ready(active) if active.entry.path == path => Some(active),
            _ => None,
        }
    }

    fn next_generation(&mut self) -> Result<u64> {
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or_else(|| AppError::new(ErrorCode::Integrity, "session_generation_exhausted"))?;
        Ok(self.generation)
    }

    fn register(&mut self, selected: &Selected, database: &Database) -> Result<Entry> {
        let mut entry = selected.entry.clone();
        if !selected.registered {
            if !valid_name(&database.info.name) {
                return Err(AppError::new(ErrorCode::Integrity, "ledger_name_invalid"));
            }
            entry.label = database.info.name.clone();
            self.registry.add(entry.clone())?;
        }
        Ok(entry)
    }

    fn current_open(&self, window: &str) -> Option<OpenResult> {
        match &self.state {
            State::Ready(active) if active.window == window => Some(OpenResult {
                state: LifecycleState::Ready,
                session: Some(active.session.clone()),
                unlock_challenge: None,
            }),
            State::Locked(selected)
                if selected.window == window && selected.expires > Instant::now() =>
            {
                Some(OpenResult {
                    state: LifecycleState::Locked,
                    session: None,
                    unlock_challenge: Some(selected.descriptor()),
                })
            }
            _ => None,
        }
    }

    fn scoped(&self, window: &str, scope: &Scope) -> Result<Arc<Active>> {
        match &self.state {
            State::Ready(active) if active.window == window && active.session.scope == *scope => {
                Ok(active.clone())
            }
            State::Ready(_) => Err(stale()),
            State::Locked(_) | State::Unselected => Err(unavailable()),
            _ => Err(AppError::busy()),
        }
    }

    fn quiesce(&mut self) -> Result<()> {
        self.quiesce_at(None)
    }

    fn quiesce_at(&mut self, expected_revision: Option<&str>) -> Result<()> {
        let previous = std::mem::replace(&mut self.state, State::Quiescing);
        if let State::Ready(active) = previous {
            active.accepting.store(false, Ordering::Release);
            if let Some(slot) = active.database.try_read()
                && let Some(db) = slot.as_ref()
            {
                db.interrupt_readers();
            }
            let drained = match active.database.try_write_for(Duration::from_secs(5)) {
                Some(mut slot) => (|| {
                    if let Some(expected) = expected_revision {
                        let actual = slot
                            .as_ref()
                            .ok_or_else(stale)?
                            .read(crate::ledger_engine::native_revision)?;
                        if actual != expected {
                            return Err(crate::ledger_engine::stale_revision());
                        }
                    }
                    if let Some(db) = slot.take() {
                        db.close();
                    }
                    Ok(())
                })(),
                None => Err(AppError::busy()),
            };
            if let Err(error) = drained {
                active.accepting.store(true, Ordering::Release);
                self.state = State::Ready(active);
                return Err(error);
            }
        }
        self.state = State::Unselected;
        Ok(())
    }
}
