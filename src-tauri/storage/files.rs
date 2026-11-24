use crate::{
    crypto::{KeyWrapper, bad_credential},
    domain::{AppError, ErrorCode, Result},
};
#[cfg(not(unix))]
pub use same_file::Handle as FileIdentity;

#[cfg(unix)]
#[derive(PartialEq, Eq)]
pub struct FileIdentity(u64, u64);
use serde::{Serialize, de::DeserializeOwned};
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{ErrorKind, Read, Write},
    path::{Component, Path, PathBuf},
};

pub use crate::domain::io_error;

fn unsafe_path() -> AppError {
    AppError::validation("destination", "unsafe_ledger_path")
}

fn link_or_reparse(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

pub(super) fn validate_components(path: &Path) -> Result<()> {
    check_components(path)
}

fn check_components(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        let metadata = fs::symlink_metadata(ancestor).map_err(io_error)?;
        if link_or_reparse(&metadata) {
            return Err(unsafe_path());
        }
    }
    Ok(())
}

pub fn checked_path(path: &Path, existing: bool) -> Result<PathBuf> {
    checked_path_extension(path, existing, "db")
}

pub fn checked_path_extension(path: &Path, existing: bool, extension: &str) -> Result<PathBuf> {
    checked_file(path, existing, &[extension])
}

pub fn checked_file(path: &Path, existing: bool, extensions: &[&str]) -> Result<PathBuf> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
        || (!extensions.is_empty()
            && !path.extension().is_some_and(|ext| {
                extensions
                    .iter()
                    .any(|candidate| ext.eq_ignore_ascii_case(candidate))
            }))
        || path.file_name().and_then(|name| name.to_str()).is_none()
    {
        return Err(unsafe_path());
    }
    let parent = path.parent().ok_or_else(unsafe_path)?;
    let to_check = if existing { path } else { parent };
    check_components(to_check)?;
    if existing && !fs::metadata(path).map_err(io_error)?.is_file() {
        return Err(unsafe_path());
    }
    let canonical = fs::canonicalize(to_check).map_err(io_error)?;
    Ok(if existing {
        canonical
    } else {
        canonical.join(path.file_name().ok_or_else(unsafe_path)?)
    })
}

pub(super) fn checked_directory(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute()
        || path.file_name().is_none()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err(unsafe_path());
    }
    check_components(path)?;
    if !fs::metadata(path).map_err(io_error)?.is_dir() {
        return Err(unsafe_path());
    }
    fs::canonicalize(path).map_err(io_error)
}

pub fn ensure_private_dir(path: &Path) -> Result<()> {
    for ancestor in path.ancestors().filter(|path| !path.as_os_str().is_empty()) {
        match fs::symlink_metadata(ancestor) {
            Ok(_) => {
                check_components(ancestor)?;
                break;
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(io_error(error)),
        }
    }
    fs::create_dir_all(path).map_err(io_error)?;
    check_components(path)?;
    if !fs::metadata(path).map_err(io_error)?.is_dir() {
        return Err(unsafe_path());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(io_error)?;
    }
    Ok(())
}

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

fn check_optional_file(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() && !link_or_reparse(&meta) => Ok(true),
        Ok(_) => Err(unsafe_path()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_error(error)),
    }
}

pub fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut result = OsString::from(path.as_os_str());
    result.push(suffix);
    PathBuf::from(result)
}

pub fn identity(path: &Path) -> Result<FileIdentity> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = fs::metadata(path).map_err(io_error)?;
        Ok(FileIdentity(metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    FileIdentity::from_path(path).map_err(io_error)
}

pub struct FileLease(File);

impl FileLease {
    pub fn ledger(path: &Path) -> Result<Self> {
        // SQLite uses byte-range locks. Unix flock is a separate lock namespace.
        #[cfg(unix)]
        let lease = Self::lock(private_options().open(path).map_err(io_error)?)?;
        #[cfg(not(unix))]
        let lease = Self::sidecar(&sibling(path, ".lease"))?;

        for suffix in ["-wal", "-shm", "-journal", ".keys"] {
            check_optional_file(&sibling(path, suffix))?;
        }
        Ok(lease)
    }

    pub fn sidecar(path: &Path) -> Result<Self> {
        check_optional_file(path)?;
        Self::lock(
            private_options()
                .create(true)
                .truncate(false)
                .open(path)
                .map_err(io_error)?,
        )
    }

    fn lock(file: File) -> Result<Self> {
        match file.try_lock() {
            Ok(()) => Ok(Self(file)),
            Err(std::fs::TryLockError::WouldBlock) => Err(AppError::busy()),
            Err(std::fs::TryLockError::Error(error)) => Err(io_error(error)),
        }
    }

    pub fn sync(&self) -> Result<()> {
        self.0.sync_all().map_err(io_error)
    }
}

pub fn has_wrapper(path: &Path) -> Result<bool> {
    check_optional_file(&sibling(path, ".keys"))
}

pub fn read_wrapper(path: &Path) -> Result<KeyWrapper> {
    if !has_wrapper(path)? {
        return Err(bad_credential());
    }
    let wrapper: KeyWrapper =
        read_json(&sibling(path, ".keys"), 4096).map_err(|_| bad_credential())?;
    wrapper.validate()?;
    Ok(wrapper)
}

pub fn is_plain(path: &Path) -> Result<bool> {
    let mut file = File::open(path).map_err(io_error)?;
    let mut header = [0; 16];
    match file.read_exact(&mut header) {
        Ok(()) => Ok(&header == b"SQLite format 3\0"),
        Err(error) if error.kind() == ErrorKind::UnexpectedEof => {
            Err(AppError::new(ErrorCode::Integrity, "ledger_header_invalid"))
        }
        Err(error) => Err(io_error(error)),
    }
}

pub fn read_json<T: DeserializeOwned>(path: &Path, max: u64) -> Result<T> {
    let mut contents = Vec::new();
    File::open(path)
        .map_err(io_error)?
        .take(max + 1)
        .read_to_end(&mut contents)
        .map_err(io_error)?;
    if contents.len() as u64 <= max
        && let Ok(value) = serde_json::from_slice(&contents)
    {
        return Ok(value);
    }
    Err(AppError::new(
        ErrorCode::Integrity,
        "invalid_local_metadata",
    ))
}

pub fn write_json(path: &Path, value: &impl Serialize, replace: bool) -> Result<()> {
    let parent = path.parent().ok_or_else(unsafe_path)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(io_error)?;
    serde_json::to_writer(&mut temp, value).map_err(|error| {
        error.io_error_kind().map_or_else(
            || AppError::new(ErrorCode::Integrity, "invalid_local_metadata"),
            |kind| io_error(kind.into()),
        )
    })?;
    temp.flush().map_err(io_error)?;
    temp.as_file().sync_all().map_err(io_error)?;
    if replace {
        temp.persist(path).map_err(|error| io_error(error.error))?;
    } else {
        temp.persist_noclobber(path)
            .map_err(|error| io_error(error.error))?;
    }
    // Once published, an fsync error cannot be rolled back safely.
    sync_directory(parent);
    Ok(())
}

fn sync_directory(path: &Path) {
    #[cfg(unix)]
    let _ = File::open(path).and_then(|file| file.sync_all());
    #[cfg(not(unix))]
    let _ = path;
}

pub struct NewLedger {
    path: PathBuf,
    published: bool,
    wrapper_written: bool,
}

impl NewLedger {
    pub fn reserve(path: PathBuf) -> Result<Self> {
        for suffix in ["-wal", "-shm", "-journal", ".keys"] {
            if check_optional_file(&sibling(&path, suffix))? {
                return Err(AppError::new(
                    ErrorCode::Conflict,
                    "destination_sidecar_exists",
                ));
            }
        }
        private_options()
            .create_new(true)
            .open(&path)
            .map_err(io_error)?;
        Ok(Self {
            path,
            published: false,
            wrapper_written: false,
        })
    }

    pub fn write_wrapper(&mut self, wrapper: &KeyWrapper) -> Result<()> {
        write_json(&sibling(&self.path, ".keys"), wrapper, false)?;
        self.wrapper_written = true;
        Ok(())
    }

    pub fn sync_parent(&self) -> Result<()> {
        #[cfg(unix)]
        File::open(self.path.parent().ok_or_else(unsafe_path)?)
            .and_then(|directory| directory.sync_all())
            .map_err(io_error)?;
        Ok(())
    }

    pub fn publish(mut self) {
        self.published = true;
    }
}

impl Drop for NewLedger {
    fn drop(&mut self) {
        if !self.published {
            if self.wrapper_written {
                let _ = fs::remove_file(sibling(&self.path, ".keys"));
            }
            for suffix in ["-wal", "-shm", "-journal", ""] {
                let _ = fs::remove_file(sibling(&self.path, suffix));
            }
            if let Some(parent) = self.path.parent() {
                sync_directory(parent);
            }
        }
    }
}

pub fn metadata_exists(path: &Path) -> Result<bool> {
    check_optional_file(path)
}
