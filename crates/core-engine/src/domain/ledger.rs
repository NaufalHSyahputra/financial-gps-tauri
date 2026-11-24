use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum Protection {
    Plain,
    Encrypted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum StoragePolicy {
    Managed,
    Portable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LedgerDescriptor {
    pub ledger_instance_id: String,
    pub label: String,
    pub protection: Protection,
    pub storage_policy: StoragePolicy,
    pub quick_unlock: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum LifecycleState {
    Unselected,
    Resolving,
    Locked,
    Opening,
    Ready,
    Quiescing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Scope {
    pub session_id: String,
    pub ledger_instance_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LedgerSession {
    pub scope: Scope,
    pub ledger_uuid: String,
    pub name: String,
    pub base_currency: String,
    pub revision: String,
    pub protection: Protection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UnlockChallenge {
    pub id: String,
    pub ledger: LedgerDescriptor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct OpenResult {
    pub state: LifecycleState,
    pub session: Option<LedgerSession>,
    pub unlock_challenge: Option<UnlockChallenge>,
}
