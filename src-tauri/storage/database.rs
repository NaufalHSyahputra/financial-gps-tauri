use crate::{
    crypto::{DatabaseKey, bad_credential},
    domain::{AppError, ErrorCode, Result},
};
use parking_lot::Mutex;
use rusqlite::{Connection, InterruptHandle, OpenFlags, Transaction, ffi};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

use super::{
    clean_shutdown,
    files::{self, FileIdentity, FileLease},
    migration::{self, LedgerMetadata},
};

const BUSY_TIMEOUT: Duration = Duration::from_millis(5000);
const READER_COUNT: usize = 2;

pub fn sql_error(error: rusqlite::Error) -> AppError {
    use rusqlite::ErrorCode as Sql;
    let (code, key) = match error.sqlite_error_code() {
        Some(Sql::DatabaseBusy | Sql::DatabaseLocked) => (ErrorCode::Busy, "ledger_busy"),
        Some(Sql::OperationInterrupted) => (ErrorCode::Cancelled, "operation_cancelled"),
        Some(Sql::DiskFull) => (ErrorCode::InsufficientSpace, "storage_full"),
        Some(Sql::CannotOpen | Sql::SystemIoFailure | Sql::ReadOnly) => {
            (ErrorCode::Io, "database_io_error")
        }
        Some(Sql::NotADatabase) => return bad_credential(),
        Some(Sql::ConstraintViolation) => (ErrorCode::Integrity, "database_constraint_failed"),
        _ => (ErrorCode::Integrity, "database_integrity_error"),
    };
    AppError::new(code, key)
}

fn configure(path: &Path, key: Option<&DatabaseKey>, reader: bool) -> Result<Connection> {
    let access = if reader {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    } else {
        OpenFlags::SQLITE_OPEN_READ_WRITE
    };
    let conn = Connection::open_with_flags(
        path,
        access
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW
            | OpenFlags::SQLITE_OPEN_EXRESCODE,
    )
    .map_err(sql_error)?;
    if let Some(key) = key {
        let mut raw_key = Zeroizing::new([0_u8; 67]);
        raw_key[..2].copy_from_slice(b"x'");
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for (index, byte) in key.iter().enumerate() {
            raw_key[2 + index * 2] = HEX[(byte >> 4) as usize];
            raw_key[3 + index * 2] = HEX[(byte & 0xf) as usize];
        }
        raw_key[66] = b'\'';
        // sqlite3_key copies the codec input. Passing it directly avoids SQL statement copies.
        let status = unsafe { ffi::sqlite3_key(conn.handle(), raw_key.as_ptr().cast(), 67) };
        drop(raw_key);
        if status != ffi::SQLITE_OK {
            return Err(bad_credential());
        }
    }
    conn.query_row("SELECT count(*) FROM sqlite_schema", [], |row| {
        row.get::<_, i64>(0)
    })
    .map_err(sql_error)?;
    conn.busy_timeout(BUSY_TIMEOUT).map_err(sql_error)?;
    conn.execute_batch(
        "PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL; PRAGMA temp_store=MEMORY; PRAGMA cache_size=-16384; PRAGMA trusted_schema=OFF;",
    ).map_err(sql_error)?;
    for (pragma, expected) in [
        ("foreign_keys", 1),
        ("synchronous", 2),
        ("busy_timeout", 5000),
        ("temp_store", 2),
    ] {
        let actual: i64 = conn
            .pragma_query_value(None, pragma, |row| row.get(0))
            .map_err(sql_error)?;
        if actual != expected {
            return Err(AppError::new(
                ErrorCode::Integrity,
                "database_configuration_invalid",
            ));
        }
    }
    use rusqlite::functions::FunctionFlags;
    conn.create_scalar_function(
        crate::domain::NORMALIZE_SQL_FUNCTION,
        1,
        FunctionFlags::SQLITE_UTF8
            | FunctionFlags::SQLITE_DETERMINISTIC
            | FunctionFlags::SQLITE_DIRECTONLY,
        |context| Ok(crate::domain::normalize(context.get_raw(0).as_str()?)),
    )
    .map_err(sql_error)?;
    if reader {
        verify_wal(&conn)?;
        conn.pragma_update(None, "query_only", true)
            .map_err(sql_error)?;
        let query_only: bool = conn
            .pragma_query_value(None, "query_only", |row| row.get(0))
            .map_err(sql_error)?;
        if !query_only {
            return Err(AppError::new(
                ErrorCode::Integrity,
                "database_configuration_invalid",
            ));
        }
    }
    Ok(conn)
}

fn verify_wal(conn: &Connection) -> Result<()> {
    let mode: String = conn
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .map_err(sql_error)?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(AppError::new(ErrorCode::Integrity, "wal_unavailable"));
    }
    Ok(())
}

#[derive(Clone)]
pub struct LedgerInfo {
    pub uuid: String,
    pub name: String,
    pub base_currency: String,
    pub revision: String,
}

#[derive(Default)]
struct Reader {
    connection: Mutex<Option<Connection>>,
    interrupt: Mutex<Option<InterruptHandle>>,
}

enum IntegrityStatus {
    Pending,
    Verified,
    Failed(AppError),
}

struct Verification {
    status: Arc<Mutex<IntegrityStatus>>,
    cancelled: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    interrupt: Arc<Mutex<Option<InterruptHandle>>>,
}

impl Verification {
    fn new(pending: bool) -> Self {
        Self {
            status: Arc::new(Mutex::new(if pending {
                IntegrityStatus::Pending
            } else {
                IntegrityStatus::Verified
            })),
            cancelled: Arc::new(AtomicBool::new(false)),
            worker: None,
            interrupt: Default::default(),
        }
    }

    fn start(
        &mut self,
        path: PathBuf,
        key: Option<DatabaseKey>,
        identity: Arc<FileIdentity>,
        report_failure: impl Fn(AppError) + Send + Sync + 'static,
    ) {
        if !matches!(*self.status.lock(), IntegrityStatus::Pending) || self.worker.is_some() {
            return;
        }
        let status = self.status.clone();
        let cancelled = self.cancelled.clone();
        let interrupt = self.interrupt.clone();
        let report_failure = Arc::new(report_failure);
        let report_worker_failure = report_failure.clone();
        let worker = thread::Builder::new()
            .name("ledger-integrity".into())
            .spawn(move || {
                // Give initial dashboard and transaction reads priority over the full scan.
                thread::park_timeout(Duration::from_secs(1));
                while !cancelled.load(Ordering::Acquire) {
                    let result = (|| {
                        if files::identity(&path)? != *identity {
                            return Err(AppError::new(ErrorCode::Integrity, "ledger_file_changed"));
                        }
                        let mut reader = configure(&path, key.as_ref(), true)?;
                        if files::identity(&path)? != *identity {
                            return Err(AppError::new(ErrorCode::Integrity, "ledger_file_changed"));
                        }
                        let stopping = cancelled.clone();
                        reader
                            .progress_handler(1_000, Some(move || stopping.load(Ordering::Acquire)))
                            .map_err(crate::storage::sql_error)?;
                        *interrupt.lock() = Some(reader.get_interrupt_handle());
                        if cancelled.load(Ordering::Acquire) {
                            return Ok(());
                        }
                        migration::quick_check(&reader)
                    })();
                    *interrupt.lock() = None;
                    if cancelled.load(Ordering::Acquire) {
                        return;
                    }
                    match result {
                        Ok(()) => *status.lock() = IntegrityStatus::Verified,
                        Err(error) if error.code == ErrorCode::Busy => {
                            thread::park_timeout(Duration::from_millis(250));
                            continue;
                        }
                        Err(_) => {
                            let error = background_integrity_error();
                            *status.lock() = IntegrityStatus::Failed(error.clone());
                            report_worker_failure(error);
                        }
                    }
                    return;
                }
            });
        match worker {
            Ok(worker) => self.worker = Some(worker),
            Err(_) => {
                let error = background_integrity_error();
                *self.status.lock() = IntegrityStatus::Failed(error.clone());
                report_failure(error);
            }
        }
    }

    fn verified(&self) -> Result<bool> {
        match &*self.status.lock() {
            IntegrityStatus::Pending => Ok(false),
            IntegrityStatus::Verified => Ok(true),
            IntegrityStatus::Failed(error) => Err(error.clone()),
        }
    }

    fn stop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        if let Some(handle) = self.interrupt.lock().as_ref() {
            handle.interrupt();
        }
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            if worker.join().is_err() {
                *self.status.lock() = IntegrityStatus::Failed(background_integrity_error());
            }
        }
    }
}

impl Drop for Verification {
    fn drop(&mut self) {
        self.stop();
    }
}

fn background_integrity_error() -> AppError {
    AppError::new(ErrorCode::Integrity, "background_ledger_integrity_failed")
}

fn data_version(conn: &Connection) -> Result<i64> {
    conn.pragma_query_value(None, "data_version", |row| row.get(0))
        .map_err(sql_error)
}

fn ledger_info(connection: &Connection) -> Result<LedgerInfo> {
    let info = connection
        .query_row(
            "SELECT ledger_uuid,name,display_currency,revision FROM ledger_meta WHERE singleton=1",
            [],
            |row| {
                Ok(LedgerInfo {
                    uuid: row.get(0)?,
                    name: row.get(1)?,
                    base_currency: row.get(2)?,
                    revision: row.get::<_, i64>(3)?.to_string(),
                })
            },
        )
        .map_err(sql_error)?;
    if uuid::Uuid::parse_str(&info.uuid).is_err() {
        return Err(AppError::new(
            ErrorCode::Integrity,
            "ledger_identity_invalid",
        ));
    }
    Ok(info)
}

struct Writer {
    connection: Connection,
    last_checkpoint: Instant,
}

pub struct Database {
    verification: Verification,
    path: PathBuf,
    key: Option<DatabaseKey>,
    opening_data_version: i64,
    readers: [Reader; READER_COUNT],
    pub(super) attachment_key: Option<DatabaseKey>,
    pub(super) file_fence: Mutex<()>,
    writer: Mutex<Writer>,
    // Declare the lease last so every connection closes before the lease is released.
    _lease: FileLease,
    pub info: LedgerInfo,
    pub(super) identity: Arc<FileIdentity>,
}

impl Database {
    pub fn open(
        path: &Path,
        lease: FileLease,
        key: Option<&DatabaseKey>,
        new_metadata: Option<LedgerMetadata<'_>>,
    ) -> Result<Self> {
        Self::open_with(path, lease, key, new_metadata, |_| Ok(()))
    }

    pub(super) fn reopen(path: &Path, lease: FileLease, key: Option<&DatabaseKey>) -> Result<Self> {
        Self::open_internal(path, lease, key, None, true, |_| Ok(()))
    }

    pub(super) fn open_with(
        path: &Path,
        lease: FileLease,
        key: Option<&DatabaseKey>,
        new_metadata: Option<LedgerMetadata<'_>>,
        populate: impl FnOnce(&mut Connection) -> Result<()>,
    ) -> Result<Self> {
        Self::open_internal(path, lease, key, new_metadata, false, populate)
    }

    fn open_internal(
        path: &Path,
        lease: FileLease,
        key: Option<&DatabaseKey>,
        new_metadata: Option<LedgerMetadata<'_>>,
        allow_defer: bool,
        populate: impl FnOnce(&mut Connection) -> Result<()>,
    ) -> Result<Self> {
        let identity = files::identity(path)?;
        let before = clean_shutdown::fingerprint(path);
        let receipt = clean_shutdown::consume(path, key)?;
        let mut writer = configure(path, key, false)?;
        let migrated = new_metadata.is_none()
            && migration::apply_upgrades(&mut writer, path, migration::UPGRADES)?;
        if migrated {
            drop(writer);
            writer = configure(path, key, false)?;
        }
        let initial_data_version = data_version(&writer)?;
        let existing = new_metadata.is_none();
        let defer = if existing
            && allow_defer
            && !migrated
            && before.is_some()
            && before == clean_shutdown::fingerprint(path)
            && let Some(receipt) = receipt
        {
            let info = ledger_info(&writer)?;
            receipt.ledger_uuid == info.uuid && receipt.revision == info.revision
        } else {
            false
        };
        writer
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(sql_error)?;
        verify_wal(&writer)?;
        writer
            .pragma_update(None, "wal_autocheckpoint", 1000)
            .map_err(sql_error)?;
        if let Some(meta) = new_metadata {
            migration::initialize(&mut writer, meta)?;
            if !migration::UPGRADES.is_empty()
                && migration::apply_upgrades(&mut writer, path, migration::UPGRADES)?
            {
                drop(writer);
                writer = configure(path, key, false)?;
                verify_wal(&writer)?;
            }
            lease.sync()?;
        }
        let defer = defer
            && before == clean_shutdown::fingerprint(path)
            && clean_shutdown::no_pending_wal(path)
            && initial_data_version == data_version(&writer)?;
        if existing && !defer {
            migration::quick_check(&writer)?;
        }
        populate(&mut writer)?;
        let info = ledger_info(&writer)?;
        let opening_data_version = if existing {
            initial_data_version
        } else {
            data_version(&writer)?
        };
        if identity != files::identity(path)? {
            return Err(AppError::new(ErrorCode::Conflict, "ledger_file_changed"));
        }
        Ok(Self {
            verification: Verification::new(defer),
            path: path.to_owned(),
            key: key.cloned(),
            opening_data_version,
            readers: Default::default(),
            attachment_key: key.map(crate::crypto::attachment::derive).transpose()?,
            file_fence: Mutex::new(()),
            writer: Mutex::new(Writer {
                connection: writer,
                last_checkpoint: Instant::now(),
            }),
            _lease: lease,
            identity: Arc::new(identity),
            info,
        })
    }

    pub(super) fn start_verification(
        &mut self,
        report_failure: impl Fn(AppError) + Send + Sync + 'static,
    ) {
        self.verification.start(
            self.path.clone(),
            self.key.clone(),
            self.identity.clone(),
            report_failure,
        );
    }

    pub(super) fn integrity_verified(&self) -> Result<bool> {
        self.verification.verified()
    }

    pub fn read<T>(&self, work: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        self.integrity_verified()?;
        let (reader, mut connection) = self
            .readers
            .iter()
            .find_map(|reader| {
                reader
                    .connection
                    .try_lock()
                    .map(|connection| (reader, connection))
            })
            .ok_or_else(AppError::busy)?;
        if connection.is_none() {
            let conn = configure(&self.path, self.key.as_ref(), true)?;
            if files::identity(&self.path)? != *self.identity {
                return Err(AppError::new(ErrorCode::Conflict, "ledger_file_changed"));
            }
            *reader.interrupt.lock() = Some(conn.get_interrupt_handle());
            *connection = Some(conn);
        }
        let mut backend =
            crate::storage::SqliteConnection(connection.as_mut().expect("reader initialized"));
        let snapshot = backend
            .begin_on_worker(core_engine::sql::TransactionMode::Read)?
            .into_native();
        let value = work(&snapshot)?;
        self.integrity_verified()?;
        snapshot.commit().map_err(sql_error)?;
        Ok(value)
    }

    pub fn write<T>(&self, work: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        self.write_internal(true, work)
    }

    pub(super) fn write_sync_metadata<T>(
        &self,
        work: impl FnOnce(&Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        self.write_internal(false, work)
    }

    fn write_internal<T>(
        &self,
        track_sync: bool,
        work: impl FnOnce(&Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        self.integrity_verified()?;
        let mut writer = self.writer.try_lock().ok_or_else(AppError::busy)?;
        let mut backend = crate::storage::SqliteConnection(&mut writer.connection);
        let tx = backend
            .begin_on_worker(core_engine::sql::TransactionMode::Immediate)?
            .into_native();
        let before = tx.total_changes();
        let value = work(&tx)?;
        if track_sync && tx.total_changes() != before {
            super::sync::dirty(&tx)?;
        }
        self.integrity_verified()?;
        tx.commit().map_err(sql_error)?;
        if writer.last_checkpoint.elapsed() >= Duration::from_secs(2) {
            writer.last_checkpoint = Instant::now();
            let _ = writer
                .connection
                .query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |row| {
                    row.get::<_, i32>(0)
                });
        }
        Ok(value)
    }

    pub(super) fn install_sync_image(
        &self,
        archive: &super::snapshot::RestoredArchive,
        vault: &super::attachments::Vault<'_>,
        incoming: super::sync::Incoming,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<()> {
        self.integrity_verified()?;
        let mut writer = self.writer.try_lock().ok_or_else(AppError::busy)?;
        super::snapshot::copy_tables_with(
            &archive.database,
            &mut writer.connection,
            Some(&self.info.uuid),
            cancelled,
            super::snapshot::SearchIndex::Rebuild,
            |tx| {
                let binding = super::sync::advance(tx, incoming)?;
                let revision: i64 = tx
                    .query_row("SELECT revision FROM ledger_meta", [], |row| row.get(0))
                    .map_err(sql_error)?;
                let mut settings = tx.prepare(
                    "SELECT key,value_json FROM ledger_settings WHERE key LIKE 'secret.%' OR key LIKE 'backup.%' OR key LIKE 'fx.%enabled'",
                ).map_err(sql_error)?;
                let settings = settings
                    .query_map([], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })
                    .map_err(sql_error)?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(sql_error)?;
                Ok((binding, revision, settings))
            },
            |tx, (binding, revision, settings)| {
                for (key, value) in settings {
                    tx.execute(
                        "INSERT INTO ledger_settings VALUES(?1,?2)",
                        rusqlite::params![key, value],
                    )
                    .map_err(sql_error)?;
                }
                super::sync::store(tx, &binding)?;
                let revision = revision.checked_add(1).ok_or_else(super::sync::corrupt)?;
                tx.execute(
                    "UPDATE ledger_meta SET revision=max(revision,?1)",
                    [revision],
                )
                .map_err(sql_error)?;
                vault.adopt_synced(tx, &archive.objects)?;
                self.integrity_verified()?;
                if cancelled() {
                    return Err(AppError::new(
                        ErrorCode::StaleSession,
                        "stale_ledger_session",
                    ));
                }
                Ok(())
            },
        )
    }

    pub fn interrupt_readers(&self) {
        for reader in &self.readers {
            if let Some(interrupt) = reader.interrupt.lock().as_ref() {
                interrupt.interrupt();
            }
        }
    }

    pub fn close(self) {
        self.finish_close(true);
    }

    pub(super) fn close_staging(self) {
        self.finish_close(false);
    }

    fn finish_close(mut self, record_receipt: bool) {
        self.verification.stop();
        let Self {
            readers,
            writer,
            verification,
            path,
            key,
            opening_data_version,
            _lease,
            identity,
            ..
        } = self;
        drop(readers);
        let writer = writer.into_inner().connection;
        let unchanged = data_version(&writer).ok() == Some(opening_data_version);
        let checkpoint = writer.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((
                row.get::<_, i32>(0)?,
                row.get::<_, i32>(1)?,
                row.get::<_, i32>(2)?,
            ))
        });
        let info = if record_receipt
            && unchanged
            && matches!(checkpoint, Ok((0, 0, 0)))
            && matches!(verification.verified(), Ok(true))
        {
            ledger_info(&writer).ok()
        } else {
            None
        };
        if writer.close().is_ok()
            && let Some(info) = info
            && files::identity(&path).is_ok_and(|current| current == *identity)
            && _lease.sync().is_ok()
        {
            clean_shutdown::publish(&path, key.as_ref(), info.uuid, info.revision);
        }
    }
}

#[cfg(test)]
mod migration_tests {
    use super::*;
    use crate::crypto::{Credential, KeyWrapper};

    #[test]
    fn irreversible_upgrade_keeps_a_wal_complete_safety_copy_and_rolls_back_failure() {
        for encrypted in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("ledger.db");
            std::fs::File::create(&path).unwrap();
            let credential: Credential =
                serde_json::from_str(r#""disposable migration fixture""#).unwrap();
            let key = encrypted.then(|| {
                let (wrapper, key) = KeyWrapper::enroll(&credential).unwrap();
                files::write_json(&files::sibling(&path, ".keys"), &wrapper, false).unwrap();
                key
            });
            let lease = FileLease::ledger(&path).unwrap();
            let database = Database::open(
                &path,
                lease,
                key.as_ref(),
                Some(LedgerMetadata {
                    uuid: "00000000-0000-4000-8000-000000000001",
                    name: "Original ledger",
                    timezone: None,
                    currency: "USD",
                    created_at_ms: 0,
                }),
            )
            .unwrap();
            drop(database);

            let _lease = FileLease::ledger(&path).unwrap();
            let mut conn = configure(&path, key.as_ref(), false).unwrap();
            conn.execute_batch("PRAGMA wal_autocheckpoint=0; CREATE TABLE release_fixture(value TEXT); INSERT INTO release_fixture VALUES ('committed WAL data');").unwrap();
            assert!(files::sibling(&path, "-wal").metadata().unwrap().len() > 0);

            let invalid = migration::Upgrade {
                sql: "DROP TABLE release_fixture; SELECT value FROM deliberate_migration_failure;",
                irreversible: true,
            };
            let mut upgrades = migration::UPGRADES.to_vec();
            upgrades.push(invalid);
            let error = migration::apply_upgrades(&mut conn, &path, &upgrades).unwrap_err();
            assert_eq!(error.message_key, "migration_failed_safety_copy_available");
            migration::verify(&conn).unwrap();
            let content = |database: &Connection| -> String {
                database
                    .query_row("SELECT value FROM release_fixture", [], |row| row.get(0))
                    .unwrap()
            };
            assert_eq!(content(&conn), "committed WAL data");
            let backup_prefix = format!(".fgps-pre-migration-v{}-", migration::SCHEMA_VERSION);
            let backup = std::fs::read_dir(root.path())
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|entry| {
                    entry
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with(&backup_prefix)
                })
                .unwrap()
                .join("ledger.db");
            assert_eq!(files::is_plain(&backup).unwrap(), !encrypted);
            assert!(!files::sibling(&backup, "-wal").exists());
            let original_key = encrypted.then(|| {
                files::read_wrapper(&backup)
                    .unwrap()
                    .unlock(&credential)
                    .unwrap()
            });
            let snapshot = configure(&backup, original_key.as_ref(), false).unwrap();
            migration::verify(&snapshot).unwrap();
            migration::validate(&snapshot).unwrap();
            assert_eq!(content(&snapshot), "committed WAL data");

            let valid = migration::Upgrade {
                sql: "DROP TABLE release_fixture;",
                irreversible: true,
            };
            *upgrades.last_mut().unwrap() = valid;
            assert!(migration::apply_upgrades(&mut conn, &path, &upgrades).unwrap());
            assert_eq!(
                conn.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
                    .unwrap(),
                migration::SCHEMA_VERSION + 1
            );
            let present: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='release_fixture')",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(!present);
            assert_eq!(
                conn.pragma_query_value(None, "foreign_keys", |row| row.get::<_, u32>(0))
                    .unwrap(),
                1
            );
        }
    }
}

#[cfg(all(test, unix))]
mod clean_open_tests {
    use super::*;
    use crate::crypto::{Credential, KeyWrapper};

    fn create(path: &Path, key: Option<&DatabaseKey>) -> Database {
        std::fs::File::create(path).unwrap();
        Database::open(
            path,
            FileLease::ledger(path).unwrap(),
            key,
            Some(LedgerMetadata {
                uuid: "00000000-0000-4000-8000-000000000001",
                name: "Clean fixture",
                timezone: None,
                currency: "USD",
                created_at_ms: 0,
            }),
        )
        .unwrap()
    }

    fn reopen(path: &Path, key: Option<&DatabaseKey>) -> Database {
        Database::reopen(path, FileLease::ledger(path).unwrap(), key).unwrap()
    }

    #[test]
    fn clean_receipt_is_single_use_bound_to_file_and_authenticated_for_sqlcipher() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("ledger.db");
        let credential: Credential = serde_json::from_str(r#""synthetic open fixture""#).unwrap();
        let (_, key) = KeyWrapper::enroll(&credential).unwrap();
        create(&path, Some(&key)).close();
        let marker = files::sibling(&path, ".fgps-clean");
        assert!(marker.exists());

        let mut db = reopen(&path, Some(&key));
        assert!(!db.integrity_verified().unwrap());
        assert!(!marker.exists());
        assert!(
            db.readers
                .iter()
                .all(|reader| reader.connection.lock().is_none())
        );
        db.read(|tx| {
            assert_eq!(
                tx.pragma_query_value(None, "foreign_keys", |row| row.get::<_, i32>(0))
                    .map_err(crate::storage::sql_error)?,
                1
            );
            Ok(())
        })
        .unwrap();
        assert!(db.readers[0].connection.lock().is_some());
        assert!(db.readers[1].connection.lock().is_none());
        db.start_verification(|_| {});
        let deadline = Instant::now() + Duration::from_secs(5);
        while !db.integrity_verified().unwrap() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(20));
        }
        db.close();
        assert!(marker.exists());

        let copied = root.path().join("copied.db");
        std::fs::copy(&path, &copied).unwrap();
        std::fs::copy(&marker, files::sibling(&copied, ".fgps-clean")).unwrap();
        assert!(reopen(&copied, Some(&key)).integrity_verified().unwrap());

        let receipt = std::fs::read_to_string(&marker).unwrap();
        let mut bytes: Vec<u8> = serde_json::from_str(&receipt).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        std::fs::write(&marker, serde_json::to_vec(&bytes).unwrap()).unwrap();
        let db = reopen(&path, Some(&key));
        assert!(db.integrity_verified().unwrap());
        db.close();
        let db = reopen(&path, Some(&key));
        assert!(!db.integrity_verified().unwrap());
        drop(db);
        assert!(reopen(&path, Some(&key)).integrity_verified().unwrap());
    }

    #[test]
    fn background_integrity_failure_blocks_reads_and_writes_and_disallows_clean_receipt() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("ledger.db");
        use std::os::unix::fs::FileExt;
        let initial = create(&path, None);
        let offset = initial.write(|tx| {
            tx.execute_batch("CREATE TABLE corruption_fixture(value INTEGER) STRICT; INSERT INTO corruption_fixture VALUES (1);").map_err(crate::storage::sql_error)?;
            let page: i64 = tx.query_row("SELECT rootpage FROM sqlite_schema WHERE name='corruption_fixture'", [], |row| row.get(0)).map_err(crate::storage::sql_error)?;
            let size: i64 = tx.pragma_query_value(None, "page_size", |row| row.get(0)).map_err(crate::storage::sql_error)?;
            Ok(((page - 1) * size) as u64)
        }).unwrap();
        initial.close();
        let mut db = reopen(&path, None);
        assert!(!db.integrity_verified().unwrap());
        // Damage a B-tree page outside the schema reads after the clean receipt is consumed.
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.write_all_at(&[0], offset).unwrap();
        file.sync_all().unwrap();
        let (notification, received) = std::sync::mpsc::channel();
        db.start_verification(move |error| {
            let _ = notification.send(error);
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while matches!(db.integrity_verified(), Ok(false)) {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            received.recv_timeout(Duration::from_secs(2)).unwrap().code,
            ErrorCode::Integrity
        );
        assert_eq!(
            db.integrity_verified().unwrap_err().code,
            ErrorCode::Integrity
        );
        assert_eq!(db.read(|_| Ok(())).unwrap_err().code, ErrorCode::Integrity);
        assert_eq!(db.write(|_| Ok(())).unwrap_err().code, ErrorCode::Integrity);
        db.close();
        assert!(!files::sibling(&path, ".fgps-clean").exists());
        let result = Database::reopen(&path, FileLease::ledger(&path).unwrap(), None);
        assert_eq!(result.err().unwrap().code, ErrorCode::Integrity);
    }
}
