use std::{
    collections::BTreeMap,
    io::{Cursor, Read, Write},
    time::Duration,
};

use rusqlite::{Connection, Transaction, backup::Backup, params_from_iter};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::{
    io_error,
    migration::{self, LedgerMetadata},
    sql_error,
};
use crate::{
    crypto::{Credential, archive},
    domain::{AppError, ErrorCode, Result},
    ledger_engine::{dto, integrity, invalid},
};

const MAX_DATABASE: usize = 256 * 1024 * 1024;
const MAX_MANIFEST: usize = 1024 * 1024;

dto! {
    pub struct AttachmentManifest {pub id:String,pub entry:String,pub size:String,pub sha256:String}
    pub struct BackupManifest {
        pub format_version:u8,pub schema_version:u32,pub ledger_uuid:String,pub source_revision:String,
        pub created_at:String,pub database_entry:String,pub database_size:String,pub database_sha256:String,pub counts:BTreeMap<String,String>,
        pub attachments:Vec<AttachmentManifest>,
    }
}

pub struct Object {
    pub id: String,
    pub bytes: Zeroizing<Vec<u8>>,
}
pub struct RestoredArchive {
    pub manifest: BackupManifest,
    pub database: Connection,
    pub objects: Vec<Object>,
}

pub fn hash(bytes: &[u8]) -> String {
    use std::fmt::Write;
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            write!(out, "{byte:02x}").expect("write to String");
            out
        })
}

fn trusted(version: u32) -> Result<Connection> {
    let mut db = Connection::open_in_memory().map_err(sql_error)?;
    db.execute_batch(
        "PRAGMA trusted_schema=OFF; PRAGMA foreign_keys=ON; PRAGMA temp_store=MEMORY;",
    )
    .map_err(sql_error)?;
    migration::initialize(
        &mut db,
        LedgerMetadata {
            uuid: "00000000-0000-4000-8000-000000000001",
            name: "Backup snapshot",
            timezone: None,
            currency: "USD",
            created_at_ms: 0,
        },
    )?;
    migration::upgrade_memory_to(&mut db, version)?;
    Ok(db)
}

fn schema(db: &Connection) -> Result<BTreeMap<(String, String), String>> {
    let mut statement = db
        .prepare(
            "SELECT type,name,sql FROM sqlite_schema
             WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' ORDER BY type,name",
        )
        .map_err(sql_error)?;
    statement
        .query_map([], |row| Ok(((row.get(0)?, row.get(1)?), row.get(2)?)))
        .map_err(sql_error)?
        .collect::<rusqlite::Result<_>>()
        .map_err(sql_error)
}
fn tables(known: &BTreeMap<(String, String), String>) -> Vec<&str> {
    known
        .iter()
        .filter_map(|((kind, name), definition)| {
            (kind == "table" && definition.ends_with("STRICT")).then_some(name.as_str())
        })
        .collect()
}
/// A new device can join a populated remote only if the local ledger has no user rows.
pub(super) fn has_user_data(db: &Connection) -> Result<bool> {
    if db
        .query_row("SELECT revision<>0 FROM ledger_meta", [], |row| {
            row.get::<_, bool>(0)
        })
        .map_err(sql_error)?
    {
        return Ok(true);
    }
    for table in tables(&schema(db)?) {
        if matches!(
            table,
            "currencies"
                | "ledger_meta"
                | "ledger_settings"
                | "schema_migrations"
                | "valid_transaction_state"
        ) {
            continue;
        }
        if db
            .query_row(
                &format!("SELECT EXISTS(SELECT 1 FROM {})", identifier(table)),
                [],
                |row| row.get::<_, bool>(0),
            )
            .map_err(sql_error)?
        {
            return Ok(true);
        }
    }
    Ok(false)
}
fn identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}
fn limits(db: &Connection) -> Result<usize> {
    let page: i64 = db
        .pragma_query_value(None, "page_size", |row| {
            use rusqlite::types::ValueRef;
            match row.get_ref(0)? {
                ValueRef::Integer(value) => Ok(value),
                ValueRef::Text(value) => std::str::from_utf8(value)
                    .ok()
                    .and_then(|value| value.parse().ok())
                    .ok_or(rusqlite::Error::InvalidQuery),
                _ => Err(rusqlite::Error::InvalidQuery),
            }
        })
        .map_err(sql_error)?;
    let count: i64 = db
        .pragma_query_value(None, "page_count", |row| row.get(0))
        .map_err(sql_error)?;
    page.checked_mul(count)
        .and_then(|size| usize::try_from(size).ok())
        .filter(|size| *size <= MAX_DATABASE)
        .ok_or_else(|| invalid("backup", "backup_database_size_limit"))
}

/// Copy base columns using only the schema constructed in this process. Existing destination data must be disposable.
pub(crate) fn copy_tables(
    source: &Connection,
    destination: &mut Connection,
    new_uuid: Option<&str>,
    cancelled: &dyn Fn() -> bool,
) -> Result<()> {
    copy_tables_with(
        source,
        destination,
        new_uuid,
        cancelled,
        SearchIndex::Rebuild,
        |_| Ok(()),
        |_, ()| Ok(()),
    )
}

pub(super) enum SearchIndex {
    Empty,
    Rebuild,
}

pub(super) fn copy_tables_with<T>(
    source: &Connection,
    destination: &mut Connection,
    new_uuid: Option<&str>,
    cancelled: &dyn Fn() -> bool,
    search_index: SearchIndex,
    before: impl FnOnce(&Transaction<'_>) -> Result<T>,
    after: impl FnOnce(&Transaction<'_>, T) -> Result<()>,
) -> Result<()> {
    limits(source)?;
    let known = schema(&trusted(migration::SCHEMA_VERSION)?)?;
    if schema(source)? != known || schema(destination)? != known {
        return Err(integrity("backup_schema_mismatch"));
    }
    migration::verify(source)?;
    let triggers = known
        .iter()
        .filter(|((kind, _), _)| kind == "trigger")
        .collect::<Vec<_>>();
    destination
        .pragma_update(None, "foreign_keys", false)
        .map_err(sql_error)?;
    let result = (|| {
        let tx = destination.transaction().map_err(sql_error)?;
        let local = before(&tx)?;
        for ((_, name), _) in &triggers {
            tx.execute_batch(&format!("DROP TRIGGER {};", identifier(name)))
                .map_err(sql_error)?;
        }
        for table in tables(&known) {
            if cancelled() {
                return Err(cancel_error());
            }
            let columns = tx
                .prepare("SELECT name FROM pragma_table_xinfo(?1) WHERE hidden=0 ORDER BY cid")
                .map_err(sql_error)?
                .query_map([table], |row| row.get::<_, String>(0))
                .map_err(sql_error)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(sql_error)?;
            let columns_sql = format!(
                "rowid,{}",
                columns
                    .iter()
                    .map(|column| identifier(column))
                    .collect::<Vec<_>>()
                    .join(",")
            );
            let placeholders = vec!["?"; columns.len() + 1].join(",");
            let table = identifier(table);
            tx.execute(&format!("DELETE FROM {table}"), [])
                .map_err(sql_error)?;
            let mut select = source
                .prepare(&format!("SELECT {columns_sql} FROM {table} ORDER BY rowid"))
                .map_err(sql_error)?;
            let mut insert = tx
                .prepare(&format!(
                    "INSERT INTO {table}({columns_sql}) VALUES({placeholders})"
                ))
                .map_err(sql_error)?;
            let mut rows = select.query([]).map_err(sql_error)?;
            while let Some(row) = rows.next().map_err(sql_error)? {
                if cancelled() {
                    return Err(cancel_error());
                }
                let values = (0..=columns.len())
                    .map(|index| row.get::<_, rusqlite::types::Value>(index))
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(sql_error)?;
                insert
                    .execute(params_from_iter(values))
                    .map_err(sql_error)?;
            }
        }
        if let Some(id) = new_uuid {
            tx.execute("UPDATE ledger_meta SET ledger_uuid=?1", [id])
                .map_err(sql_error)?;
        }
        tx.execute("DELETE FROM ledger_settings WHERE key LIKE 'secret.%' OR key LIKE 'backup.%' OR key LIKE 'fx.%enabled'",[]).map_err(sql_error)?;
        tx.execute("DELETE FROM transaction_search", [])
            .map_err(sql_error)?;
        if matches!(search_index, SearchIndex::Rebuild) {
            let mut postings = tx
                .prepare(
                    "SELECT t.rowid,t.id,t.description,t.notes,p.name
                FROM transactions t LEFT JOIN payees p ON p.id=t.payee_id ORDER BY t.rowid",
                )
                .map_err(sql_error)?;
            let mut rows = postings.query([]).map_err(sql_error)?;
            let mut insert=tx.prepare("INSERT INTO transaction_search(rowid,transaction_id,description,notes,payee) VALUES(?1,?2,?3,?4,?5)").map_err(sql_error)?;
            while let Some(row) = rows.next().map_err(sql_error)? {
                if cancelled() {
                    return Err(cancel_error());
                }
                let normalize = crate::domain::normalize;
                let payee = row.get::<_, Option<String>>(4).map_err(sql_error)?;
                insert
                    .execute(rusqlite::params![
                        row.get::<_, i64>(0).map_err(sql_error)?,
                        row.get::<_, String>(1).map_err(sql_error)?,
                        normalize(row.get_ref(2).map_err(sql_error)?.as_str().map_err(|_| {
                            AppError::new(ErrorCode::Integrity, "unexpected_database_value")
                        })?),
                        normalize(row.get_ref(3).map_err(sql_error)?.as_str().map_err(|_| {
                            AppError::new(ErrorCode::Integrity, "unexpected_database_value")
                        })?),
                        payee.as_deref().map(normalize)
                    ])
                    .map_err(sql_error)?;
            }
        }
        for (_, sql) in triggers {
            tx.execute_batch(sql).map_err(sql_error)?;
        }
        after(&tx, local)?;
        validate_semantics(&tx)?;
        migration::validate(&tx)?;
        tx.commit().map_err(sql_error)?;
        Ok(())
    })();
    destination
        .pragma_update(None, "foreign_keys", true)
        .map_err(sql_error)?;
    result
}

fn validate_semantics(db: &Connection) -> Result<()> {
    let invalid:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM transactions t
        JOIN transaction_splits s ON s.transaction_id=t.id GROUP BY t.id HAVING count(*)<2 OR sum(s.amount_minor)<>t.amount_minor
        OR t.category_id IS NOT NULL OR t.entry_kind<>'ordinary')
        OR EXISTS(SELECT 1 FROM transfer_links l JOIN transactions d ON d.id=l.debit_id JOIN transactions c ON c.id=l.credit_id
        WHERE d.direction<>'debit' OR c.direction<>'credit' OR d.account_id=c.account_id OR d.ignored OR c.ignored
        OR d.category_id IS NOT NULL OR c.category_id IS NOT NULL OR d.entry_kind<>'ordinary' OR c.entry_kind<>'ordinary'
        OR (d.currency_code=c.currency_code AND (d.amount_minor<>c.amount_minor OR l.applied_rate_decimal IS NOT NULL))
        OR (d.currency_code<>c.currency_code AND l.applied_rate_decimal IS NULL)
        OR EXISTS(SELECT 1 FROM transaction_splits s WHERE s.transaction_id IN(d.id,c.id)))",
        [],|row|row.get(0)).map_err(sql_error)?;
    if invalid {
        return Err(integrity("invalid_archive_postings"));
    }
    Ok(())
}
fn cancel_error() -> AppError {
    AppError::new(ErrorCode::Cancelled, "operation_cancelled")
}

fn online(
    source: &Connection,
    destination: &mut Connection,
    cancelled: &dyn Fn() -> bool,
) -> Result<()> {
    use rusqlite::backup::StepResult;
    let operation = Backup::new(source, destination).map_err(sql_error)?;
    let mut waits = 0;
    loop {
        if cancelled() {
            return Err(cancel_error());
        }
        match operation.step(256).map_err(sql_error)? {
            StepResult::Done => return Ok(()),
            StepResult::More => {}
            _ => {
                waits += 1;
                if waits >= 50 {
                    return Err(AppError::busy());
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}
pub fn snapshot(
    tx: &Transaction<'_>,
    encrypted: bool,
    cancelled: &dyn Fn() -> bool,
) -> Result<Zeroizing<Vec<u8>>> {
    limits(tx)?;
    let mut output = Connection::open_in_memory().map_err(sql_error)?;
    if encrypted {
        let mut bridge = trusted(migration::SCHEMA_VERSION)?;
        copy_tables(tx, &mut bridge, None, cancelled)?;
        online(&bridge, &mut output, cancelled)?;
    } else {
        online(tx, &mut output, cancelled)?;
        // Clear machine-local settings on the snapshot, after the consistent online copy.
        output.execute("DELETE FROM ledger_settings WHERE key LIKE 'secret.%' OR key LIKE 'backup.%' OR key LIKE 'fx.%enabled'",[]).map_err(sql_error)?;
    }
    serialize_snapshot(&mut output)
}

pub(super) fn relational_snapshot(
    tx: &Transaction<'_>,
    cancelled: &dyn Fn() -> bool,
) -> Result<Zeroizing<Vec<u8>>> {
    let mut output = trusted(migration::SCHEMA_VERSION)?;
    copy_tables_with(
        tx,
        &mut output,
        None,
        cancelled,
        SearchIndex::Empty,
        |_| Ok(()),
        |_, ()| Ok(()),
    )?;
    serialize_snapshot(&mut output)
}

fn serialize_snapshot(output: &mut Connection) -> Result<Zeroizing<Vec<u8>>> {
    migration::verify(output)?;
    limits(output)?;
    let mut bytes = Zeroizing::new(output.serialize("main").map_err(sql_error)?.to_vec());
    if bytes.len() > 19 {
        bytes[18] = 1;
        bytes[19] = 1;
    }
    Ok(bytes)
}

pub(super) fn load_database(bytes: &[u8]) -> Result<Connection> {
    if bytes.len() > MAX_DATABASE || bytes.len() < 100 || &bytes[..16] != b"SQLite format 3\0" {
        return Err(integrity("invalid_backup_database"));
    }
    let mut db = Connection::open_in_memory().map_err(sql_error)?;
    db.execute_batch(
        "PRAGMA trusted_schema=OFF; PRAGMA foreign_keys=ON; PRAGMA temp_store=MEMORY;",
    )
    .map_err(sql_error)?;
    db.deserialize_read_exact("main", bytes, bytes.len(), false)
        .map_err(sql_error)?;
    let version = core_engine::migrations::verify_supported(&super::sqlite_view(&db))?;
    let known = schema(&trusted(version)?)?;
    if schema(&db)? != known {
        return Err(integrity("backup_schema_mismatch"));
    }
    migration::validate(&db)?;
    validate_semantics(&db)?;
    let full: String = db
        .pragma_query_value(None, "integrity_check", |row| row.get(0))
        .map_err(sql_error)?;
    if full != "ok" {
        return Err(integrity("archive_integrity_check_failed"));
    }
    Ok(db)
}
pub fn manifest(
    db: &Connection,
    bytes: &[u8],
    objects: &[Object],
    now: chrono::DateTime<chrono::Utc>,
) -> Result<BackupManifest> {
    let version = core_engine::migrations::verify_supported(&super::sqlite_view(db))?;
    let known = schema(&trusted(version)?)?;
    if schema(db)? != known {
        return Err(integrity("backup_schema_mismatch"));
    }
    let mut counts = BTreeMap::new();
    for table in tables(&known) {
        let count: i64 = db
            .query_row(
                &format!("SELECT count(*) FROM {}", identifier(table)),
                [],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        counts.insert(table.to_owned(), count.to_string());
    }
    let (uuid, revision) = db
        .query_row(
            "SELECT ledger_uuid,CAST(revision AS TEXT) FROM ledger_meta",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(sql_error)?;
    let mut attachments = Vec::with_capacity(objects.len());
    let mut seen = std::collections::BTreeSet::new();
    for object in objects {
        if uuid::Uuid::parse_str(&object.id).is_err() || !seen.insert(object.id.clone()) {
            return Err(integrity("invalid_archive_attachment_identity"));
        }
        let hash = hash(&object.bytes);
        let size = object.bytes.len().to_string();
        let expected = db
            .query_row(
                "SELECT sha256,CAST(size_bytes AS TEXT),state FROM attachments WHERE id=?1",
                [&object.id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .map_err(sql_error)?;
        if expected != (hash.clone(), size.clone(), "ready".to_owned()) {
            return Err(integrity("archive_attachment_hash_mismatch"));
        }
        attachments.push(AttachmentManifest {
            id: object.id.clone(),
            entry: format!("attachments/{}.blob", object.id),
            size,
            sha256: hash,
        });
    }
    if counts.get("attachments").map(String::as_str) != Some(attachments.len().to_string().as_str())
    {
        return Err(integrity("archive_attachment_inventory_mismatch"));
    }
    Ok(BackupManifest {
        format_version: 1,
        schema_version: version,
        ledger_uuid: uuid,
        source_revision: revision,
        created_at: now.to_rfc3339(),
        database_entry: "ledger.db".into(),
        database_size: bytes.len().to_string(),
        database_sha256: hash(bytes),
        counts,
        attachments,
    })
}

pub(super) fn materialize(image: &[u8], objects: Vec<Object>) -> Result<RestoredArchive> {
    let database = load_database(image)?;
    let manifest = manifest(&database, image, &objects, chrono::Utc::now())?;
    Ok(RestoredArchive {
        manifest,
        database,
        objects,
    })
}
pub fn write<W: Write>(
    image: &[u8],
    objects: &[Object],
    output: W,
    password: &Credential,
    cancelled: &dyn Fn() -> bool,
) -> Result<BackupManifest> {
    let db = load_database(image)?;
    let manifest = manifest(&db, image, objects, chrono::Utc::now())?;
    let json = serde_json::to_vec(&manifest).map_err(crate::ledger_engine::codec_error)?;
    if json.len() > MAX_MANIFEST {
        return Err(invalid("backup", "manifest_size_limit"));
    }
    let length = (json.len() as u32).to_be_bytes();
    let payload = Cursor::new(length.as_slice())
        .chain(Cursor::new(json.as_slice()))
        .chain(Cursor::new(image));
    let mut ordered: Vec<&[u8]> = objects
        .iter()
        .map(|object| object.bytes.as_slice())
        .collect();
    ordered.reverse();
    archive::encrypt(
        payload.chain(ObjectReader {
            ordered,
            current: &[],
        }),
        output,
        password,
        cancelled,
    )?;
    Ok(manifest)
}
struct ObjectReader<'a> {
    ordered: Vec<&'a [u8]>,
    current: &'a [u8],
}
impl Read for ObjectReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        while self.current.is_empty() {
            let Some(next) = self.ordered.pop() else {
                return Ok(0);
            };
            self.current = next;
        }
        self.current.read(buf)
    }
}

pub fn open<R: Read>(
    input: R,
    password: &Credential,
    cancelled: &dyn Fn() -> bool,
) -> Result<RestoredArchive> {
    let mut payload = Zeroizing::new(Vec::new());
    archive::decrypt(input, &mut *payload, password, cancelled)?;
    let mut reader = Cursor::new(payload.as_slice());
    let mut size = [0u8; 4];
    reader.read_exact(&mut size).map_err(io_error)?;
    let size = u32::from_be_bytes(size) as usize;
    if size > MAX_MANIFEST {
        return Err(integrity("invalid_backup_manifest"));
    }
    let json = take(&mut reader, size)?;
    let supplied: BackupManifest =
        serde_json::from_slice(json).map_err(crate::ledger_engine::codec_error)?;
    if supplied.format_version != 1
        || !(core_engine::migrations::INITIAL_VERSION..=migration::SCHEMA_VERSION)
            .contains(&supplied.schema_version)
    {
        return Err(AppError::new(
            ErrorCode::UnsupportedSchema,
            "unsupported_backup_version",
        ));
    }
    if supplied.database_entry != "ledger.db"
        || supplied.attachments.iter().any(|item| {
            uuid::Uuid::parse_str(&item.id)
                .map(|id| id.to_string())
                .ok()
                .as_ref()
                != Some(&item.id)
                || item.entry != format!("attachments/{}.blob", item.id)
        })
    {
        return Err(integrity("invalid_archive_entry"));
    }
    let bytes = take(&mut reader, length(&supplied.database_size, MAX_DATABASE)?)?;
    if hash(bytes) != supplied.database_sha256 {
        return Err(integrity("archive_database_hash_mismatch"));
    }
    let mut db = load_database(bytes)?;
    let mut objects = Vec::new();
    for item in &supplied.attachments {
        let data = take(&mut reader, length(&item.size, 10_000_000)?)?;
        if hash(data) != item.sha256 {
            return Err(integrity("archive_attachment_hash_mismatch"));
        }
        objects.push(Object {
            id: item.id.clone(),
            bytes: Zeroizing::new(data.to_vec()),
        });
    }
    if reader.position() != payload.len() as u64 {
        return Err(integrity("archive_trailing_payload"));
    }
    let declared = chrono::DateTime::parse_from_rfc3339(&supplied.created_at)
        .map_err(|_| integrity("invalid_backup_manifest"))?
        .with_timezone(&chrono::Utc);
    let actual = manifest(&db, bytes, &objects, declared)?;
    if actual.schema_version != supplied.schema_version
        || actual.counts != supplied.counts
        || actual.ledger_uuid != supplied.ledger_uuid
        || actual.source_revision != supplied.source_revision
    {
        return Err(integrity("archive_manifest_mismatch"));
    }
    migration::upgrade_memory(&mut db)?;
    Ok(RestoredArchive {
        manifest: supplied,
        database: db,
        objects,
    })
}
fn take<'a>(reader: &mut Cursor<&'a [u8]>, size: usize) -> Result<&'a [u8]> {
    let offset = usize::try_from(reader.position()).map_err(|_| integrity("archive_size_limit"))?;
    let end = offset
        .checked_add(size)
        .ok_or_else(|| integrity("archive_size_limit"))?;
    let bytes = reader
        .get_ref()
        .get(offset..end)
        .ok_or_else(|| integrity("archive_truncated_payload"))?;
    reader.set_position(end as u64);
    Ok(bytes)
}
fn length(value: &str, bound: usize) -> Result<usize> {
    value
        .parse::<usize>()
        .ok()
        .filter(|n| *n <= bound && n.to_string() == value)
        .ok_or_else(|| integrity("archive_size_limit"))
}
