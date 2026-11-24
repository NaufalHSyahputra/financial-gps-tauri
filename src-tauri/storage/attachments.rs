use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use rusqlite::{Connection, OptionalExtension, params};
use zeroize::Zeroizing;

use super::{
    ApprovedPath, files, io_error,
    snapshot::{Object, hash},
};
use crate::{
    crypto::{DatabaseKey, attachment},
    domain::{AppError, ErrorCode, Result},
    ledger_engine::{conflict, dto, integrity, invalid, missing, new_id, utc_timestamp},
};

pub const MAX_ATTACHMENT: usize = 10_000_000;
const MARKER: &str = ".financial-gps-attachments";

dto! {
    pub enum AttachmentState {Staged,Ready,Deleting,Missing}
    pub struct AttachmentSummary {
        pub id:String,pub transaction_id:String,pub display_name:String,pub media_type:String,
        pub size:String,pub digest:String,pub state:AttachmentState,pub created_at:String,
    }
    pub struct AttachmentWriteResult {pub attachments:Vec<AttachmentSummary>}
    pub struct AttachmentOpenResult {pub id:String,pub opened:bool}
}

pub(crate) struct Vault<'a> {
    pub root: PathBuf,
    pub uuid: &'a str,
    pub key: Option<&'a DatabaseKey>,
}
impl Vault<'_> {
    fn prepare_root(&self) -> Result<()> {
        files::ensure_private_dir(&self.root)?;
        let marker = self.root.join(MARKER);
        if files::metadata_exists(&marker)? {
            let mut contents = String::new();
            File::open(&marker)
                .map_err(io_error)?
                .take(64)
                .read_to_string(&mut contents)
                .map_err(io_error)?;
            if contents != self.uuid {
                return Err(integrity("attachment_directory_identity_mismatch"));
            }
            return Ok(());
        }
        if fs::read_dir(&self.root).map_err(io_error)?.next().is_some() {
            return Err(conflict("attachment_directory_not_empty"));
        }
        let mut file = tempfile::NamedTempFile::new_in(&self.root).map_err(io_error)?;
        file.write_all(self.uuid.as_bytes()).map_err(io_error)?;
        file.as_file().sync_all().map_err(io_error)?;
        file.persist_noclobber(&marker)
            .map_err(|error| io_error(error.error))?;
        sync_dir(&self.root)
    }

    fn path(&self, key: &str, staged: bool) -> Result<PathBuf> {
        if uuid::Uuid::parse_str(key)
            .ok()
            .is_none_or(|uuid| uuid.to_string() != key)
        {
            return Err(integrity("invalid_attachment_storage_key"));
        }
        let suffix = if staged { "part" } else { "blob" };
        Ok(self.root.join(format!("{key}.{suffix}")))
    }

    fn read_path(&self, id: &str, key: &str, staged: bool) -> Result<Option<Zeroizing<Vec<u8>>>> {
        let path = self.path(key, staged)?;
        if !files::metadata_exists(&path)? {
            return Ok(None);
        }
        let mut bytes = Zeroizing::new(Vec::new());
        File::open(path)
            .map_err(io_error)?
            .take((MAX_ATTACHMENT + 30) as u64)
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        if bytes.len() > MAX_ATTACHMENT + 29 {
            return Err(integrity("attachment_size_mismatch"));
        }
        if let Some(key) = self.key {
            Ok(Some(attachment::decrypt(key, id, &bytes)?))
        } else {
            Ok(Some(bytes))
        }
    }

    fn store(&self, id: &str, key: &str, bytes: &[u8], staged: bool) -> Result<()> {
        self.prepare_root()?;
        let destination = self.path(key, staged)?;
        let mut file = tempfile::NamedTempFile::new_in(&self.root).map_err(io_error)?;
        if let Some(secret) = self.key {
            file.write_all(&attachment::encrypt(secret, id, bytes)?)
                .map_err(io_error)?;
        } else {
            file.write_all(bytes).map_err(io_error)?;
        }
        file.as_file().sync_all().map_err(io_error)?;
        file.persist_noclobber(destination)
            .map_err(|error| io_error(error.error))?;
        sync_dir(&self.root)
    }
    fn publish(&self, key: &str) -> Result<()> {
        let staging = self.path(key, true)?;
        let destination = self.path(key, false)?;
        if files::metadata_exists(&destination)? {
            return Err(conflict("attachment_destination_already_exists"));
        }
        fs::rename(staging, destination).map_err(io_error)?;
        sync_dir(&self.root)
    }
    fn remove_files(&self, key: &str) -> Result<()> {
        for staged in [true, false] {
            let path = self.path(key, staged)?;
            if files::metadata_exists(&path)? {
                fs::remove_file(path).map_err(io_error)?;
            }
        }
        if self.root.exists() {
            sync_dir(&self.root)?;
        }
        Ok(())
    }

    pub(crate) fn prepare(&self, pending: &Pending) -> Result<()> {
        self.store(&pending.id, &pending.id, &pending.bytes, true)
    }
    pub(crate) fn promote(&self, pending: &Pending) -> Result<()> {
        self.publish(&pending.id)
    }
    pub(crate) fn abort(&self, pending: &Pending) {
        let _ = self.remove_files(&pending.id);
    }

    pub(crate) fn recover(&self, conn: &Connection) -> Result<()> {
        let mut query = conn
            .prepare("SELECT id,storage_key,sha256,size_bytes,state FROM attachments")
            .map_err(crate::storage::sql_error)?;
        let rows = query
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(crate::storage::sql_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(crate::storage::sql_error)?;
        if rows.is_empty() && !self.root.exists() {
            return Ok(());
        }
        self.prepare_root()?;
        let keys = rows
            .iter()
            .map(|row| row.1.clone())
            .collect::<std::collections::HashSet<_>>();
        for (id, key, digest, size, state) in rows {
            match state.as_str() {
                "deleting" => {
                    self.remove_files(&key)?;
                    conn.execute("DELETE FROM attachments WHERE id=?1", [id])
                        .map_err(crate::storage::sql_error)?;
                }
                "staged" | "ready" => {
                    let verified = |staged| {
                        self.read_path(&id, &key, staged).is_ok_and(|result| {
                            result.is_some_and(|bytes| {
                                bytes.len() as i64 == size && hash(&bytes) == digest
                            })
                        })
                    };
                    let ready = if verified(false) {
                        true
                    } else if state == "staged"
                        && verified(true)
                        && !files::metadata_exists(&self.path(&key, false)?)?
                    {
                        self.publish(&key)?;
                        true
                    } else {
                        false
                    };
                    conn.execute(
                        "UPDATE attachments SET state=?2 WHERE id=?1 AND state<>?2",
                        params![id, if ready { "ready" } else { "missing" }],
                    )
                    .map_err(crate::storage::sql_error)?;
                }
                _ => {}
            }
        }
        for entry in fs::read_dir(&self.root).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            let current = entry.path();
            if let (Some(key), Some(extension)) = (
                current.file_stem().and_then(|v| v.to_str()),
                current.extension().and_then(|v| v.to_str()),
            ) && matches!(extension, "blob" | "part")
                && self.path(key, extension == "part").is_ok()
                && !keys.contains(key)
                && files::metadata_exists(&current)?
                && entry
                    .metadata()
                    .map_err(io_error)?
                    .modified()
                    .map_err(io_error)?
                    .elapsed()
                    .is_ok_and(|age| age > Duration::from_secs(86_400))
            {
                fs::remove_file(current).map_err(io_error)?;
            }
        }
        sync_dir(&self.root)
    }
    pub(crate) fn get_bytes(
        &self,
        conn: &Connection,
        id: &str,
    ) -> Result<(AttachmentSummary, Zeroizing<Vec<u8>>)> {
        let info = get(&crate::storage::sqlite_view(conn), id)?;
        if !matches!(info.state, AttachmentState::Ready) {
            return Err(conflict("attachment_not_ready"));
        }
        self.prepare_root()?;
        let key = key(conn, id)?;
        let bytes = self
            .read_path(id, &key, false)?
            .ok_or_else(|| missing("attachmentFile"))?;
        if bytes.len().to_string() != info.size || hash(&bytes) != info.digest {
            return Err(integrity("attachment_hash_mismatch"));
        }
        Ok((info, bytes))
    }
    pub(crate) fn inventory(&self, conn: &Connection) -> Result<Vec<Object>> {
        let mut stmt = conn
            .prepare("SELECT id FROM attachments ORDER BY id")
            .map_err(crate::storage::sql_error)?;
        let ids = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(crate::storage::sql_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(crate::storage::sql_error)?;
        let mut total = 0u64;
        let mut objects = Vec::new();
        for id in ids {
            let (_, bytes) = self.get_bytes(conn, &id)?;
            total += bytes.len() as u64;
            if total > crate::crypto::archive::MAX_ARCHIVE_BYTES {
                return Err(invalid("backup", "archive_size_limit"));
            }
            objects.push(Object { id, bytes });
        }
        Ok(objects)
    }
    pub(crate) fn restore(&self, conn: &Connection, objects: &[Object]) -> Result<()> {
        let mut stored = Vec::new();
        for object in objects {
            let key = key(conn, &object.id)?;
            match self.store(&object.id, &key, &object.bytes, false) {
                Ok(()) => stored.push(key),
                Err(error) => {
                    for key in stored {
                        let _ = self.remove_files(&key);
                    }
                    return Err(error);
                }
            }
        }
        Ok(())
    }
    pub(super) fn adopt_synced(&self, conn: &Connection, objects: &[Object]) -> Result<()> {
        for object in objects {
            let key = key(conn, &object.id)?;
            if let Some(bytes) = self.read_path(&object.id, &key, false)? {
                if bytes.as_slice() != object.bytes.as_slice() {
                    return Err(conflict("sync_attachment_identity_conflict"));
                }
            } else {
                self.store(&object.id, &key, &object.bytes, false)?;
            }
        }
        Ok(())
    }
    pub(crate) fn delete_blob(&self, conn: &Connection, id: &str) -> Result<()> {
        self.remove_files(&key(conn, id)?)
    }
}
pub(super) fn sync_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)
        .map_err(io_error)?
        .sync_all()
        .map_err(io_error)?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
pub(crate) struct Pending {
    pub id: String,
    pub display: String,
    pub mime: &'static str,
    pub bytes: Zeroizing<Vec<u8>>,
}
impl Pending {
    pub(crate) fn read(grant: &ApprovedPath) -> Result<Self> {
        let path = grant.revalidate()?;
        let display = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| invalid("file", "invalid_attachment_name"))?;
        let mut bytes = Zeroizing::new(Vec::new());
        File::open(path)
            .map_err(io_error)?
            .take((MAX_ATTACHMENT + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        if bytes.is_empty() || bytes.len() > MAX_ATTACHMENT {
            return Err(invalid("file", "attachment_size_limit"));
        }
        let mime = validate_signature(display, &bytes)?;
        Ok(Self {
            id: new_id(),
            display: display.to_owned(),
            mime,
            bytes,
        })
    }
}

pub fn validate_signature(name: &str, bytes: &[u8]) -> Result<&'static str> {
    let extension = Path::new(name)
        .extension()
        .and_then(|part| part.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mime = match extension.as_str() {
        "jpg" | "jpeg" if bytes.starts_with(&[0xff, 0xd8, 0xff]) => "image/jpeg",
        "png" if bytes.starts_with(b"\x89PNG\r\n\x1a\n") => "image/png",
        "gif" if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") => "image/gif",
        "webp" if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") => "image/webp",
        "heic"
            if bytes.get(4..8) == Some(b"ftyp")
                && bytes.get(8..12).is_some_and(|brand| {
                    matches!(brand, b"heic" | b"heix" | b"hevc" | b"hevx")
                }) =>
        {
            "image/heic"
        }
        "pdf" if bytes.starts_with(b"%PDF-") => "application/pdf",
        _ => return Err(invalid("file", "attachment_signature_mismatch")),
    };
    Ok(mime)
}

const SELECT: &str = "SELECT id,transaction_id,display_name,media_type,CAST(size_bytes AS TEXT),sha256,state,created_at_ms FROM attachments";
fn read(row: &core_engine::sql::Row<'_>) -> core_engine::sql::Result<(AttachmentSummary, i64)> {
    let state = match row.get_ref(6)?.as_str()? {
        "staged" => AttachmentState::Staged,
        "ready" => AttachmentState::Ready,
        "deleting" => AttachmentState::Deleting,
        "missing" => AttachmentState::Missing,
        _ => return Err(core_engine::sql::Error::InvalidQuery),
    };
    Ok((
        AttachmentSummary {
            id: row.get(0)?,
            transaction_id: row.get(1)?,
            display_name: row.get(2)?,
            media_type: row.get(3)?,
            size: row.get(4)?,
            digest: row.get(5)?,
            state,
            created_at: String::new(),
        },
        row.get(7)?,
    ))
}
pub fn list(
    conn: &core_engine::sql::Transaction<'_>,
    transaction: &str,
) -> Result<Vec<AttachmentSummary>> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM transactions WHERE id=?1)",
        [transaction],
        |row| row.get(0),
    )?;
    if !exists {
        return Err(missing("transactionId"));
    }
    conn.prepare(&format!(
        "{SELECT} WHERE transaction_id=?1 ORDER BY created_at_ms,id"
    ))?
    .query_map([transaction], read)?
    .map(|value| {
        let (mut record, ms) = value?;
        record.created_at = utc_timestamp(ms)?;
        Ok(record)
    })
    .collect()
}
pub fn get(conn: &core_engine::sql::Transaction<'_>, id: &str) -> Result<AttachmentSummary> {
    use core_engine::sql::OptionalExtension;
    let (mut row, ms) = conn
        .query_row(&format!("{SELECT} WHERE id=?1"), [id], read)
        .optional()?
        .ok_or_else(|| missing("attachmentId"))?;
    row.created_at = utc_timestamp(ms)?;
    Ok(row)
}
fn key(conn: &Connection, id: &str) -> Result<String> {
    conn.query_row(
        "SELECT storage_key FROM attachments WHERE id=?1",
        [id],
        |row| row.get(0),
    )
    .optional()
    .map_err(crate::storage::sql_error)?
    .ok_or_else(|| missing("attachmentId"))
}
pub(crate) fn insert(
    conn: &core_engine::sql::Transaction<'_>,
    transaction: &str,
    pending: &[Pending],
    now: i64,
) -> Result<()> {
    let current = list(conn, transaction)?;
    if pending.is_empty() || current.len() + pending.len() > 10 {
        return Err(invalid("files", "attachment_count_limit"));
    }
    for file in pending {
        conn.execute(
            "INSERT INTO attachments VALUES(?1,?2,?1,?3,?4,?5,?6,'staged',?7)",
            core_engine::sql::params![
                file.id,
                transaction,
                file.display,
                file.mime,
                file.bytes.len() as i64,
                hash(&file.bytes),
                now
            ],
        )?;
    }
    Ok(())
}
pub(crate) fn mark_ready(conn: &Connection, pending: &[Pending]) -> Result<()> {
    for file in pending {
        conn.execute(
            "UPDATE attachments SET state='ready' WHERE id=?1 AND state='staged'",
            [&file.id],
        )
        .map_err(crate::storage::sql_error)?;
    }
    Ok(())
}
pub(crate) fn mark_deleting(conn: &core_engine::sql::Transaction<'_>, id: &str) -> Result<()> {
    if conn.execute("UPDATE attachments SET state='deleting' WHERE id=?1", [id])? == 0 {
        return Err(missing("attachmentId"));
    }
    Ok(())
}
pub fn rename(
    conn: &core_engine::sql::Transaction<'_>,
    id: &str,
    name: &str,
) -> Result<AttachmentSummary> {
    let name = name.trim();
    let before = get(conn, id)?;
    let old = Path::new(&before.display_name).extension();
    let extension = Path::new(name).extension();
    if name.is_empty()
        || name.len() > 256
        || name.contains(['/', '\\'])
        || name.chars().any(char::is_control)
        || extension != old
    {
        return Err(invalid(
            "displayName",
            "invalid_attachment_name_or_extension",
        ));
    }
    conn.execute(
        "UPDATE attachments SET display_name=?2 WHERE id=?1",
        core_engine::sql::params![id, name],
    )?;
    get(conn, id)
}
pub fn save_as(target: &ApprovedPath, bytes: &[u8]) -> Result<String> {
    let (destination, replace) = target.export_target()?;
    let parent = destination
        .parent()
        .ok_or_else(|| invalid("grant", "invalid_export_destination"))?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(io_error)?;
    file.write_all(bytes).map_err(io_error)?;
    file.as_file().sync_all().map_err(io_error)?;
    target.export_target()?;
    if replace {
        file.persist(destination)
            .map_err(|error| io_error(error.error))?;
    } else {
        file.persist_noclobber(destination)
            .map_err(|error| io_error(error.error))?;
    }
    sync_dir(parent)?;
    destination
        .file_name()
        .and_then(|v| v.to_str())
        .map(str::to_owned)
        .ok_or_else(|| AppError::new(ErrorCode::Io, "invalid_destination"))
}

impl crate::ledger_engine::ReceiptValue for AttachmentSummary {
    type Reference = String;
    fn reference(&self, _: &crate::ledger_engine::Context<'_>) -> String {
        self.id.clone()
    }
    fn reload(ctx: &crate::ledger_engine::Context<'_>, id: String) -> Result<Self> {
        get(&ctx.tx, &id)
    }
}
impl crate::ledger_engine::ReceiptValue for AttachmentWriteResult {
    type Reference = Vec<String>;
    fn reference(&self, _: &crate::ledger_engine::Context<'_>) -> Self::Reference {
        self.attachments.iter().map(|row| row.id.clone()).collect()
    }
    fn reload(ctx: &crate::ledger_engine::Context<'_>, ids: Self::Reference) -> Result<Self> {
        Ok(Self {
            attachments: ids
                .iter()
                .map(|id| get(&ctx.tx, id))
                .collect::<Result<_>>()?,
        })
    }
}
