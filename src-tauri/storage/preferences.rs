use super::files;
use crate::domain::Result;
pub use core_engine::preferences::*;
use std::path::Path;

pub(super) fn get(file: &Path) -> Result<AppPreferences> {
    if !files::metadata_exists(file)? {
        return Ok(AppPreferences::default());
    }
    let mut prefs = files::read_json::<AppPreferences>(file, 32 * 1024)?;
    prefs.language = "en".into();
    Ok(prefs)
}

pub(super) fn set(file: &Path, patch: AppPreferencesPatch) -> Result<AppPreferences> {
    let prefs = apply_patch(get(file)?, patch)?;
    files::write_json(file, &prefs, true)?;
    Ok(prefs)
}
