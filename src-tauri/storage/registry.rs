use crate::domain::{AppError, ErrorCode, LedgerDescriptor, Protection, Result, StoragePolicy};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};
use uuid::Uuid;

use super::files::{self, FileLease};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Entry {
    pub instance: Uuid,
    pub path: PathBuf,
    pub label: String,
    pub protection: Protection,
    pub storage_policy: StoragePolicy,
    pub quick_unlock: bool,
}

impl Entry {
    pub fn describe(&self) -> LedgerDescriptor {
        LedgerDescriptor {
            ledger_instance_id: self.instance.to_string(),
            label: self.label.clone(),
            protection: self.protection,
            storage_policy: self.storage_policy,
            quick_unlock: self.quick_unlock,
        }
    }
}

const MAX_ENTRIES: usize = 512;
const MAX_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryData {
    version: u8,
    ledgers: Vec<Entry>,
}

pub(super) struct Registry {
    path: PathBuf,
    ledgers_dir: PathBuf,
    data: RegistryData,
    _lease: FileLease,
}

impl Registry {
    pub fn open(app_data: &Path) -> Result<Self> {
        files::ensure_private_dir(app_data)?;
        let app_data = app_data.canonicalize().map_err(files::io_error)?;
        let lease = FileLease::sidecar(&app_data.join("registry.lease"))?;
        let ledgers_dir = app_data.join("ledgers");
        files::ensure_private_dir(&ledgers_dir)?;
        let path = app_data.join("ledger-registry.json");
        let data: RegistryData = if files::metadata_exists(&path)? {
            files::read_json(&path, MAX_BYTES)?
        } else {
            RegistryData {
                version: 1,
                ledgers: Vec::new(),
            }
        };
        let mut instances = HashSet::new();
        let mut paths = HashSet::new();
        if data.version != 1
            || data.ledgers.len() > MAX_ENTRIES
            || data.ledgers.iter().any(|entry| {
                !entry.path.is_absolute()
                    || entry.label.len() > 256
                    || (entry.protection == Protection::Plain && entry.quick_unlock)
                    || (entry.storage_policy == StoragePolicy::Managed
                        && entry.path != ledgers_dir.join(format!("{}.db", entry.instance)))
                    || !instances.insert(entry.instance)
                    || !paths.insert(&entry.path)
            })
        {
            return Err(AppError::new(
                ErrorCode::Integrity,
                "ledger_registry_invalid",
            ));
        }
        Ok(Self {
            path,
            ledgers_dir,
            data,
            _lease: lease,
        })
    }

    pub fn list(&self) -> Vec<LedgerDescriptor> {
        self.data.ledgers.iter().map(Entry::describe).collect()
    }

    pub fn local_state(&self, name: &str) -> PathBuf {
        self.path.parent().expect("registry root").join(name)
    }

    pub fn managed_path(&self, id: Uuid) -> PathBuf {
        self.ledgers_dir.join(format!("{id}.db"))
    }

    pub fn get(&self, id: Uuid) -> Result<Entry> {
        self.data
            .ledgers
            .iter()
            .find(|entry| entry.instance == id)
            .cloned()
            .ok_or_else(|| AppError::validation("ledgerInstanceId", "ledger_not_registered"))
    }

    pub fn find(&self, path: &Path) -> Result<Option<Entry>> {
        let identity = files::identity(path)?;
        let mut exact = None;
        for entry in &self.data.ledgers {
            if entry.path == path {
                exact = Some(entry.clone());
            } else if files::checked_path(&entry.path, true)
                .and_then(|known| files::identity(&known))
                .is_ok_and(|known| known == identity)
            {
                return Err(AppError::new(ErrorCode::Duplicate, "ledger_file_alias"));
            }
        }
        Ok(exact)
    }

    pub fn replace(&mut self, entry: Entry) -> Result<()> {
        let index = self
            .data
            .ledgers
            .iter()
            .position(|item| item.instance == entry.instance)
            .ok_or_else(|| AppError::validation("ledgerInstanceId", "ledger_not_registered"))?;
        let previous = std::mem::replace(&mut self.data.ledgers[index], entry);
        let result = files::write_json(&self.path, &self.data, true);
        if result.is_err() {
            self.data.ledgers[index] = previous;
        }
        result
    }

    pub fn add(&mut self, entry: Entry) -> Result<()> {
        if self.data.ledgers.len() >= MAX_ENTRIES {
            return Err(AppError::validation(
                "ledgerInstanceId",
                "ledger_registry_limit",
            ));
        }
        if self.find(&entry.path)?.is_some() {
            return Err(AppError::new(
                ErrorCode::Duplicate,
                "ledger_already_registered",
            ));
        }
        self.data.ledgers.push(entry);
        let result = files::write_json(&self.path, &self.data, true);
        if result.is_err() {
            self.data.ledgers.pop();
        }
        result
    }
}
