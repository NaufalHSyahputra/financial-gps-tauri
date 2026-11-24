use super::{
    attachments, files, io_error,
    registry::{Entry, Registry},
};
use crate::{
    domain::{Protection, Result},
    ledger_engine::{conflict, integrity},
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[derive(Clone, Copy, Serialize, Deserialize)]
enum Phase {
    Staging,
    Swapping,
    Published,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Journal {
    version: u8,
    id: Uuid,
    instance: Uuid,
    phase: Phase,
    old_wrapper: bool,
    old_attachments: bool,
    new_attachments: bool,
    protection: Protection,
    label: String,
}

pub(super) struct Replacement {
    journal: Journal,
    journal_path: PathBuf,
    items: [Item; 3],
}
struct Item {
    live: PathBuf,
    old: PathBuf,
    staged: PathBuf,
    was_present: bool,
    will_be_present: bool,
    directory: bool,
}

fn present(path: &Path, directory: bool) -> Result<bool> {
    if !directory {
        return files::metadata_exists(path);
    }
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(integrity("unsafe_attachment_directory")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_error(error)),
    }
}
fn delete(path: &Path, directory: bool) -> Result<()> {
    if present(path, directory)? {
        if directory {
            fs::remove_dir_all(path).map_err(io_error)?;
        } else {
            fs::remove_file(path).map_err(io_error)?;
        }
        attachments::sync_dir(path.parent().expect("derived product path"))?;
    }
    Ok(())
}
fn mv(from: &Path, to: &Path, directory: bool) -> Result<()> {
    if present(to, directory)? {
        return Err(conflict("restore_destination_collision"));
    }
    fs::rename(from, to).map_err(io_error)?;
    attachments::sync_dir(to.parent().expect("derived product path"))
}
fn journal_dir(registry: &Registry) -> PathBuf {
    registry
        .managed_path(Uuid::nil())
        .parent()
        .and_then(Path::parent)
        .expect("managed app root")
        .join("restore-journal")
}

impl Replacement {
    pub(super) fn begin(
        registry: &Registry,
        entry: &Entry,
        root: &Path,
        protection: Protection,
        label: &str,
        new_attachments: bool,
    ) -> Result<Self> {
        let dir = journal_dir(registry);
        files::ensure_private_dir(&dir)?;
        let journal = Journal {
            version: 1,
            id: Uuid::new_v4(),
            instance: entry.instance,
            phase: Phase::Staging,
            old_wrapper: files::has_wrapper(&entry.path)?,
            old_attachments: present(root, true)?,
            new_attachments,
            protection,
            label: label.to_owned(),
        };
        let replacement = Self::from_journal(registry, entry, root, journal)?;
        for item in &replacement.items {
            if present(&item.staged, item.directory)? || present(&item.old, item.directory)? {
                return Err(conflict("restore_destination_collision"));
            }
        }
        replacement.persist(false)?;
        Ok(replacement)
    }

    fn from_journal(
        registry: &Registry,
        entry: &Entry,
        root: &Path,
        journal: Journal,
    ) -> Result<Self> {
        if journal.version != 1
            || journal.instance != entry.instance
            || journal.label.is_empty()
            || journal.label.len() > 256
            || journal.label.chars().any(char::is_control)
        {
            return Err(integrity("invalid_restore_journal"));
        }
        // Validate the live parent even if a crash left the ledger temporarily absent.
        if files::checked_path(&entry.path, false)? != entry.path {
            return Err(integrity("unsafe_restore_paths"));
        }
        if let Some(parent) = root.parent().filter(|parent| parent.exists()) {
            files::validate_components(parent)?;
        }
        let suffix = format!(".restore-{}", journal.id);
        let staged_db = files::sibling(&entry.path, &format!("{suffix}.new.db"));
        let parked_db = files::sibling(&entry.path, &format!("{suffix}.old.db"));
        let items = [
            Item {
                live: entry.path.clone(),
                old: parked_db.clone(),
                staged: staged_db.clone(),
                was_present: true,
                will_be_present: true,
                directory: false,
            },
            Item {
                live: files::sibling(&entry.path, ".keys"),
                old: files::sibling(&parked_db, ".keys"),
                staged: files::sibling(&staged_db, ".keys"),
                was_present: journal.old_wrapper,
                will_be_present: journal.protection == Protection::Encrypted,
                directory: false,
            },
            Item {
                live: root.to_owned(),
                old: files::sibling(root, &format!("{suffix}.old")),
                staged: files::sibling(root, &format!("{suffix}.new")),
                was_present: journal.old_attachments,
                will_be_present: journal.new_attachments,
                directory: true,
            },
        ];
        Ok(Self {
            journal,
            journal_path: journal_dir(registry).join(format!("{}.json", entry.instance)),
            items,
        })
    }

    pub(super) fn staged_db(&self) -> &Path {
        &self.items[0].staged
    }
    pub(super) fn staged_vault(&self) -> &Path {
        &self.items[2].staged
    }

    fn persist(&self, replace: bool) -> Result<()> {
        files::write_json(&self.journal_path, &self.journal, replace)?;
        attachments::sync_dir(self.journal_path.parent().expect("journal root"))
    }

    pub(super) fn swap(&mut self) -> Result<()> {
        for item in &self.items {
            if present(&item.live, item.directory)? != item.was_present
                || present(&item.staged, item.directory)? != item.will_be_present
            {
                return Err(conflict("restore_files_changed"));
            }
        }
        for base in [&self.items[0].live, &self.items[0].staged] {
            for suffix in ["-wal", "-shm", "-journal"] {
                if files::metadata_exists(&files::sibling(base, suffix))? {
                    return Err(conflict("restore_requires_closed_sqlite_handles"));
                }
            }
        }
        self.journal.phase = Phase::Swapping;
        self.persist(true)?;
        for item in &self.items {
            if item.was_present {
                mv(&item.live, &item.old, item.directory)?;
            }
            if item.will_be_present {
                mv(&item.staged, &item.live, item.directory)?;
            }
        }
        Ok(())
    }

    pub(super) fn commit(&mut self, registry: &mut Registry, entry: &Entry) -> Result<()> {
        self.journal.phase = Phase::Published;
        self.persist(true)?;
        self.finish(registry, entry)
    }

    fn finish(&self, registry: &mut Registry, entry: &Entry) -> Result<()> {
        for item in &self.items {
            if present(&item.live, item.directory)? != item.will_be_present {
                return Err(integrity("published_restore_files_missing"));
            }
        }
        let mut entry = entry.clone();
        entry.protection = self.journal.protection;
        entry.label = self.journal.label.clone();
        entry.quick_unlock = false;
        registry.replace(entry)?;
        for item in &self.items {
            delete(&item.old, item.directory)?;
        }
        self.cleanup()
    }
    pub(super) fn rollback(&self) -> Result<()> {
        match self.journal.phase {
            Phase::Staging => {}
            Phase::Swapping => {
                for suffix in ["-wal", "-shm", "-journal"] {
                    if files::metadata_exists(&files::sibling(&self.items[0].old, suffix))? {
                        return Err(integrity("unexpected_parked_restore_sidecars"));
                    }
                    // Reopen verification of the new database can create sidecars.
                    if present(&self.items[0].old, false)? {
                        delete(&files::sibling(&self.items[0].live, suffix), false)?;
                    }
                }
                for item in self.items.iter().rev() {
                    if present(&item.old, item.directory)? {
                        delete(&item.live, item.directory)?;
                        mv(&item.old, &item.live, item.directory)?;
                    } else if !item.was_present
                        && item.will_be_present
                        && !present(&item.staged, item.directory)?
                    {
                        delete(&item.live, item.directory)?;
                    }
                }
            }
            Phase::Published => return Err(integrity("restore_publish_already_recorded")),
        }
        self.cleanup()
    }
    fn cleanup(&self) -> Result<()> {
        for item in &self.items {
            delete(&item.staged, item.directory)?;
        }
        for suffix in ["-wal", "-shm", "-journal", ".lease"] {
            delete(&files::sibling(&self.items[0].staged, suffix), false)?;
        }
        delete(&self.journal_path, false)
    }
    pub(super) fn recover(registry: &mut Registry, entry: &Entry, root: &Path) -> Result<()> {
        let dir = journal_dir(registry);
        if !dir.exists() {
            return Ok(());
        }
        files::validate_components(&dir)?;
        let path = dir.join(format!("{}.json", entry.instance));
        if !files::metadata_exists(&path)? {
            return Ok(());
        }
        let journal = files::read_json(&path, 4096)?;
        let replacement = Self::from_journal(registry, entry, root, journal)?;
        if matches!(replacement.journal.phase, Phase::Published) {
            replacement.finish(registry, entry)
        } else {
            replacement.rollback()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::StoragePolicy;

    #[test]
    fn restart_rolls_back_a_partially_swapped_portable_ledger() {
        let app = tempfile::tempdir().unwrap();
        let portable = tempfile::tempdir().unwrap();
        let mut registry = Registry::open(app.path()).unwrap();
        let path = portable.path().canonicalize().unwrap().join("ledger.db");
        fs::write(&path, b"previous").unwrap();
        let root = files::sibling(&path, ".attachments");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("original"), b"attachment").unwrap();
        let entry = Entry {
            instance: Uuid::new_v4(),
            path: path.clone(),
            label: "Original".into(),
            protection: Protection::Plain,
            storage_policy: StoragePolicy::Portable,
            quick_unlock: false,
        };
        registry.add(entry.clone()).unwrap();
        let mut replacement = Replacement::begin(
            &registry,
            &entry,
            &root,
            Protection::Encrypted,
            "Restored",
            true,
        )
        .unwrap();
        fs::write(replacement.staged_db(), b"new").unwrap();
        fs::write(&replacement.items[1].staged, b"replacement wrapper").unwrap();
        fs::create_dir(replacement.staged_vault()).unwrap();
        replacement.journal.phase = Phase::Swapping;
        replacement.persist(true).unwrap();
        let database = &replacement.items[0];
        mv(&database.live, &database.old, false).unwrap();
        mv(&database.staged, &database.live, false).unwrap();
        let wrapper = &replacement.items[1];
        mv(&wrapper.staged, &wrapper.live, false).unwrap();
        drop(registry);
        let mut registry = Registry::open(app.path()).unwrap();
        Replacement::recover(&mut registry, &entry, &root).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"previous");
        assert_eq!(fs::read(root.join("original")).unwrap(), b"attachment");
        assert!(!wrapper.live.exists());
        assert!(!replacement.journal_path.exists());
        assert_eq!(registry.get(entry.instance).unwrap().label, "Original");
    }
}
