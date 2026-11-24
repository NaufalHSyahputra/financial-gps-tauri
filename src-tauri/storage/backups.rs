use std::{
    fs::{self, File},
    path::{Path, PathBuf},
};

use super::{
    ApprovedPath, files, io_error,
    snapshot::{self, BackupManifest, Object},
};
use crate::{
    crypto::Credential,
    domain::{AppError, ErrorCode, Result},
    ledger_engine::{dto, invalid, missing, new_id},
};

dto! {
    pub struct BackupSummary {
        pub id:String,pub file_name:String,pub created_at:String,pub bytes:String,pub source_revision:Option<String>,pub managed:bool
    }
}

const EXTENSION: &str = "fgpsbackup";
const RETAIN: usize = 10;

fn safe_id(id: &str) -> bool {
    let Some((time, uuid)) = id.split_once('-') else {
        return false;
    };
    time.len() == 13
        && time.bytes().all(|byte| byte.is_ascii_digit())
        && uuid::Uuid::parse_str(uuid).is_ok_and(|value| value.to_string() == uuid)
}
fn path(root: &Path, id: &str) -> Result<PathBuf> {
    if !safe_id(id) {
        return Err(invalid("id", "invalid_backup_id"));
    }
    Ok(root.join(format!("{id}.{EXTENSION}")))
}
pub(super) fn contains(root: &Path, id: &str) -> Result<bool> {
    let file = path(root, id)?;
    match fs::symlink_metadata(root) {
        Ok(_) => files::validate_components(root)?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(io_error(err)),
    }
    files::metadata_exists(&file)
}

pub(crate) fn file(root: &Path, id: &str) -> Result<File> {
    let file = path(root, id)?;
    if !files::metadata_exists(&file)? {
        return Err(missing("backupId"));
    }
    File::open(file).map_err(io_error)
}
pub(crate) fn list(root: &Path) -> Result<Vec<BackupSummary>> {
    let mut result = Vec::new();
    if !root.exists() {
        return Ok(result);
    }
    files::ensure_private_dir(root)?;
    for item in fs::read_dir(root).map_err(io_error)? {
        let item = item.map_err(io_error)?;
        let full = item.path();
        if full.extension().and_then(|s| s.to_str()) != Some(EXTENSION)
            || !item.file_type().map_err(io_error)?.is_file()
        {
            continue;
        }
        let Some(id) = full
            .file_stem()
            .and_then(|s| s.to_str())
            .filter(|s| safe_id(s))
        else {
            continue;
        };
        let Some(time) = id
            .split_once('-')
            .and_then(|(time, _)| time.parse::<i64>().ok())
            .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
        else {
            continue;
        };
        result.push(BackupSummary {
            id: id.to_owned(),
            file_name: item.file_name().to_string_lossy().into_owned(),
            created_at: time.to_rfc3339(),
            bytes: item.metadata().map_err(io_error)?.len().to_string(),
            source_revision: None,
            managed: true,
        });
    }
    result.sort_unstable_by(|a, b| b.id.cmp(&a.id));
    Ok(result)
}
pub(crate) fn remove(root: &Path, id: &str) -> Result<()> {
    let full = path(root, id)?;
    if !files::metadata_exists(&full)? {
        return Err(missing("backupId"));
    }
    fs::remove_file(full).map_err(io_error)?;
    super::attachments::sync_dir(root)
}

pub(crate) fn create(
    root: &Path,
    target: Option<&ApprovedPath>,
    image: &[u8],
    objects: &[Object],
    password: &Credential,
    cancelled: &dyn Fn() -> bool,
    before_publish: impl FnOnce() -> Result<()>,
) -> Result<BackupSummary> {
    let now = chrono::Utc::now();
    let id = format!("{:013}-{}", now.timestamp_millis(), new_id());
    let managed = target.is_none();
    let (destination, replace) = if let Some(grant) = target {
        let (name, replace) = grant.export_target()?;
        (name.to_owned(), replace)
    } else {
        files::ensure_private_dir(root)?;
        (path(root, &id)?, false)
    };
    let mut temp = tempfile::NamedTempFile::new_in(
        destination
            .parent()
            .ok_or_else(|| invalid("destination", "invalid_backup_destination"))?,
    )
    .map_err(io_error)?;
    let manifest = snapshot::write(image, objects, temp.as_file_mut(), password, cancelled)?;
    temp.as_file().sync_all().map_err(io_error)?;
    let verified = snapshot::open(temp.reopen().map_err(io_error)?, password, cancelled)?;
    verify_written(&manifest, &verified.manifest)?;
    drop(verified);
    let bytes = temp
        .as_file()
        .metadata()
        .map_err(io_error)?
        .len()
        .to_string();
    if let Some(grant) = target {
        grant.export_target()?;
    }
    before_publish()?;
    if replace {
        temp.persist(&destination)
            .map_err(|error| io_error(error.error))?;
    } else {
        temp.persist_noclobber(&destination)
            .map_err(|error| io_error(error.error))?;
    }
    super::attachments::sync_dir(destination.parent().expect("validated above"))?;
    if managed {
        // Keep the new generation even if the local clock moved backwards.
        for generation in list(root)?
            .into_iter()
            .filter(|item| item.id != id)
            .skip(RETAIN - 1)
        {
            remove(root, &generation.id)?;
        }
    }
    Ok(BackupSummary {
        id,
        file_name: destination
            .file_name()
            .expect("checked destination")
            .to_string_lossy()
            .into_owned(),
        created_at: manifest.created_at,
        bytes,
        source_revision: Some(manifest.source_revision),
        managed,
    })
}

fn verify_written(expected: &BackupManifest, actual: &BackupManifest) -> Result<()> {
    if actual.database_sha256 != expected.database_sha256
        || actual.source_revision != expected.source_revision
    {
        return Err(AppError::new(
            ErrorCode::Integrity,
            "backup_verification_failed",
        ));
    }
    Ok(())
}
