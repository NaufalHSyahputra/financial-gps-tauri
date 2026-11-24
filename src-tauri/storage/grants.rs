use crate::domain::{AppError, ErrorCode, Result};
use chrono::{Duration as CalendarDuration, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use uuid::Uuid;

use super::files::{self, FileIdentity};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum PathPurpose {
    LedgerOpen,
    LedgerCreate,
    TransactionExport,
    RulePackRead,
    RulePackWrite,
    AttachmentRead,
    AttachmentWrite,
    BackupWrite,
    BackupDirectory,
    RestoreRead,
}

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PathGrant {
    pub id: String,
    pub purpose: PathPurpose,
    pub display_name: String,
    pub expires_at: String,
}

const TTL: Duration = Duration::from_secs(120);

struct Grant {
    path: ApprovedPath,
    window: String,
    expires: Instant,
}

pub struct ApprovedPath {
    canonical: PathBuf,
    purpose: PathPurpose,
    identity: FileIdentity,
    replace_identity: Option<FileIdentity>,
}

impl ApprovedPath {
    pub fn revalidate(&self) -> Result<&Path> {
        let existing = self.purpose.existing();
        let canonical = checked(&self.canonical, self.purpose)?;
        let identity = path_identity(&canonical, existing)?;
        if canonical != self.canonical
            || identity != self.identity
            || (self.purpose.output() && output_identity(&canonical)? != self.replace_identity)
        {
            return Err(AppError::new(ErrorCode::Conflict, "selected_path_changed"));
        }
        Ok(&self.canonical)
    }

    pub(super) fn backup_directory(&self) -> Result<&Path> {
        if self.purpose != PathPurpose::BackupDirectory {
            return Err(AppError::validation(
                "destinationGrant",
                "invalid_or_expired_path_grant",
            ));
        }
        self.revalidate()
    }

    pub fn export_target(&self) -> Result<(&Path, bool)> {
        if !self.purpose.output() {
            return Err(AppError::validation(
                "grant",
                "invalid_or_expired_path_grant",
            ));
        }
        Ok((self.revalidate()?, self.replace_identity.is_some()))
    }
}

impl PathPurpose {
    fn existing(self) -> bool {
        matches!(
            self,
            Self::LedgerOpen
                | Self::RulePackRead
                | Self::AttachmentRead
                | Self::BackupDirectory
                | Self::RestoreRead
        )
    }
    fn output(self) -> bool {
        matches!(
            self,
            Self::TransactionExport
                | Self::RulePackWrite
                | Self::AttachmentWrite
                | Self::BackupWrite
        )
    }
}
fn checked(path: &Path, purpose: PathPurpose) -> Result<PathBuf> {
    if purpose == PathPurpose::BackupDirectory {
        return files::checked_directory(path);
    }
    let extensions: &[&str] = match purpose {
        PathPurpose::LedgerOpen | PathPurpose::LedgerCreate => &["db"],
        PathPurpose::TransactionExport => &["csv"],
        PathPurpose::RulePackRead | PathPurpose::RulePackWrite => &["json"],
        PathPurpose::AttachmentRead | PathPurpose::AttachmentWrite => {
            &["jpg", "jpeg", "png", "webp", "gif", "heic", "pdf"]
        }
        PathPurpose::BackupWrite | PathPurpose::RestoreRead => &["fgpsbackup"],
        PathPurpose::BackupDirectory => &[],
    };
    files::checked_file(path, purpose.existing(), extensions)
}

fn output_identity(path: &Path) -> Result<Option<FileIdentity>> {
    if files::metadata_exists(path)? {
        Ok(Some(files::identity(path)?))
    } else {
        Ok(None)
    }
}

fn path_identity(path: &Path, existing: bool) -> Result<FileIdentity> {
    files::identity(if existing {
        path
    } else {
        path.parent().expect("validated file path")
    })
}

#[derive(Default)]
pub struct PathGrants(Mutex<HashMap<Uuid, Grant>>);

impl PathGrants {
    pub fn issue(&self, window: &str, purpose: PathPurpose, chosen: &Path) -> Result<PathGrant> {
        let existing = purpose.existing();
        let path = checked(chosen, purpose)?;
        let display_name = path
            .file_name()
            .expect("validated file path")
            .to_string_lossy()
            .into_owned();
        let identity = path_identity(&path, existing)?;
        let replace_identity = if purpose.output() {
            output_identity(&path)?
        } else {
            None
        };
        let id = Uuid::new_v4();
        let now = Instant::now();
        let mut grants = self.0.lock();
        grants.retain(|_, grant| grant.expires > now);
        if grants.len() >= 16 {
            return Err(AppError::busy());
        }
        grants.insert(
            id,
            Grant {
                path: ApprovedPath {
                    canonical: path,
                    purpose,
                    identity,
                    replace_identity,
                },
                window: window.to_owned(),
                expires: now + TTL,
            },
        );
        Ok(PathGrant {
            id: id.to_string(),
            purpose,
            display_name,
            expires_at: (Utc::now() + CalendarDuration::seconds(TTL.as_secs() as i64)).to_rfc3339(),
        })
    }

    pub fn consume(&self, window: &str, purpose: PathPurpose, id: &str) -> Result<ApprovedPath> {
        let invalid = || AppError::validation("grant", "invalid_or_expired_path_grant");
        let id = Uuid::parse_str(id).map_err(|_| invalid())?;
        let mut grants = self.0.lock();
        let grant = grants.get(&id).ok_or_else(invalid)?;
        if grant.window != window
            || grant.path.purpose != purpose
            || grant.expires <= Instant::now()
        {
            return Err(invalid());
        }
        let path = grants.remove(&id).ok_or_else(invalid)?.path;
        path.revalidate()?;
        Ok(path)
    }
}
