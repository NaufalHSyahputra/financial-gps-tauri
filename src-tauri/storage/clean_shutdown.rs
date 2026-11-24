//! A consumed checkpoint receipt is a scheduling hint for integrity validation.
//! It is tied to a local inode and is never part of an exported ledger.
use std::path::Path;

use crate::{crypto::DatabaseKey, domain::Result};

#[cfg(unix)]
use {
    super::files,
    crate::crypto::attachment,
    hkdf::Hkdf,
    serde::{Deserialize, Serialize},
    std::{
        fs::{self, File},
        io::ErrorKind,
        os::unix::fs::MetadataExt,
    },
    zeroize::Zeroizing,
};

#[cfg(unix)]
#[derive(Deserialize, Serialize, PartialEq, Eq)]
pub(super) struct Fingerprint {
    device: u64,
    inode: u64,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

#[cfg(not(unix))]
#[derive(PartialEq, Eq)]
pub(super) struct Fingerprint;

#[cfg(unix)]
#[derive(Deserialize, Serialize)]
pub(super) struct Receipt {
    fingerprint: Fingerprint,
    pub(super) ledger_uuid: String,
    pub(super) revision: String,
    schema: u32,
}

#[cfg(not(unix))]
pub(super) struct Receipt {
    pub(super) ledger_uuid: String,
    pub(super) revision: String,
}

#[cfg(unix)]
const SUFFIX: &str = ".fgps-clean";
#[cfg(unix)]
const DOMAIN: &str = "Financial GPS clean checkpoint receipt v1";

#[cfg(unix)]
fn authentication_key(key: &DatabaseKey) -> Option<DatabaseKey> {
    let mut derived = Zeroizing::new([0; 32]);
    Hkdf::<sha2_v010::Sha256>::new(Some(DOMAIN.as_bytes()), key.as_slice())
        .expand(b"local receipt authentication", &mut *derived)
        .ok()?;
    Some(derived)
}

pub(super) fn fingerprint(path: &Path) -> Option<Fingerprint> {
    #[cfg(unix)]
    {
        let metadata = fs::symlink_metadata(path).ok()?;
        if !metadata.is_file() {
            return None;
        }
        Some(Fingerprint {
            device: metadata.dev(),
            inode: metadata.ino(),
            size: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

#[cfg(unix)]
fn has_no_journals(path: &Path) -> bool {
    ["-wal", "-shm", "-journal"].iter().all(|suffix| {
        matches!(
            files::metadata_exists(&files::sibling(path, suffix)),
            Ok(false)
        )
    })
}

pub(super) fn no_pending_wal(path: &Path) -> bool {
    #[cfg(unix)]
    {
        match fs::symlink_metadata(files::sibling(path, "-wal")) {
            Ok(metadata) => metadata.is_file() && metadata.len() == 0,
            Err(error) => error.kind() == ErrorKind::NotFound,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
}

pub(super) fn consume(path: &Path, key: Option<&DatabaseKey>) -> Result<Option<Receipt>> {
    #[cfg(unix)]
    {
        let marker = files::sibling(path, SUFFIX);
        let contents = if files::metadata_exists(&marker).unwrap_or(false) {
            files::read_json::<Vec<u8>>(&marker, 8192).ok()
        } else {
            None
        };
        // The old receipt must be durably invalid before any connection can modify the ledger.
        match fs::remove_file(marker) {
            Ok(()) => File::open(path.parent().expect("ledger has a parent"))
                .and_then(|directory| directory.sync_all())
                .map_err(files::io_error)?,
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(files::io_error(error)),
        }
        let Some(bytes) = contents.filter(|_| has_no_journals(path)) else {
            return Ok(None);
        };
        let plaintext = match key {
            None => Zeroizing::new(bytes),
            Some(key) => {
                let Some(decoded) = authentication_key(key)
                    .and_then(|key| attachment::decrypt(&key, DOMAIN, &bytes).ok())
                else {
                    return Ok(None);
                };
                decoded
            }
        };
        let receipt = serde_json::from_slice::<Receipt>(&plaintext)
            .ok()
            .filter(|receipt| {
                receipt.schema == super::migration::SCHEMA_VERSION
                    && fingerprint(path).as_ref() == Some(&receipt.fingerprint)
            });
        Ok(receipt)
    }
    #[cfg(not(unix))]
    {
        let _ = (path, key);
        Ok(None)
    }
}

pub(super) fn publish(
    path: &Path,
    key: Option<&DatabaseKey>,
    ledger_uuid: String,
    revision: String,
) {
    #[cfg(unix)]
    {
        if !has_no_journals(path) {
            return;
        }
        let Some(fingerprint) = fingerprint(path) else {
            return;
        };
        let receipt = Receipt {
            fingerprint,
            ledger_uuid,
            revision,
            schema: super::migration::SCHEMA_VERSION,
        };
        let Ok(plaintext) = serde_json::to_vec(&receipt).map(Zeroizing::new) else {
            return;
        };
        let bytes = match key {
            None => plaintext,
            Some(key) => {
                let Some(encoded) = authentication_key(key)
                    .and_then(|key| attachment::encrypt(&key, DOMAIN, &plaintext).ok())
                else {
                    return;
                };
                Zeroizing::new(encoded)
            }
        };
        let _ = files::write_json(&files::sibling(path, SUFFIX), &bytes.as_slice(), true);
    }
    #[cfg(not(unix))]
    let _ = (path, key, ledger_uuid, revision);
}
