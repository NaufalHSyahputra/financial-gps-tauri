pub mod attachments;
pub mod backups;
mod clean_shutdown;
mod database;
mod files;
mod grants;
mod ledger;
mod migration;
pub mod preferences;
mod registry;
mod replacement;
pub mod schedule;
pub mod snapshot;

pub(crate) use files::io_error;
pub use grants::{ApprovedPath, PathGrant, PathGrants, PathPurpose};
pub use ledger::{
    CreateLedger, LedgerManager, OpenTarget, ProtectionCredentials, ProtectionStatus,
    ReplacementSource,
};
pub use migration::SCHEMA_VERSION;

mod sql_adapter;
pub use database::sql_error;
pub use sql_adapter::{SqliteConnection, SqliteSession, view as sqlite_view};

mod sync;
pub use sync::{LedgerStorageMode, SetStorageMode, SyncPhase, SyncStatus, SyncToken};
