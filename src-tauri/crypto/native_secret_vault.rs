//! Secrets are addressed by installation-local ledger instance and purpose.
use crate::domain::{AppError, ErrorCode, Result};
use uuid::Uuid;
use zeroize::Zeroizing;

#[cfg(any(target_os = "android", target_os = "ios"))]
use keyring_core as keyring;

pub struct NativeSecretVault;

pub trait TursoSecretStore: Send + Sync {
    fn enroll(&self, instance: Uuid, token: &str) -> Result<()>;
    fn retrieve(&self, instance: Uuid) -> Result<Zeroizing<String>>;
    fn revoke(&self, instance: Uuid) -> Result<()>;
}

impl NativeSecretVault {
    pub(super) fn entry(service: &str, instance: Uuid) -> Result<keyring::Entry> {
        let account = instance.to_string();
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        let result = keyring::Entry::new(service, &account);
        #[cfg(any(target_os = "android", target_os = "ios"))]
        let result = keyring::Entry::new_in_store(&mobile_store()?, service, &account);
        result.map_err(keychain_error)
    }
}

#[cfg(any(target_os = "android", target_os = "ios"))]
fn mobile_store() -> Result<std::sync::Arc<keyring::api::CredentialStore>> {
    use std::sync::Arc;
    static STORE: parking_lot::Mutex<Option<Arc<keyring::api::CredentialStore>>> =
        parking_lot::Mutex::new(None);
    let mut cached = STORE.lock();
    if let Some(store) = cached.as_ref() {
        return Ok(Arc::clone(store));
    }
    #[cfg(target_os = "android")]
    let store = android_native_keyring_store::by_store::Store::new();
    #[cfg(target_os = "ios")]
    let store = apple_native_keyring_store::protected::Store::new();
    let store: Arc<keyring::api::CredentialStore> = store.map_err(keychain_error)?;
    *cached = Some(Arc::clone(&store));
    Ok(store)
}

pub(super) fn keychain_error(_: keyring::Error) -> AppError {
    AppError::new(ErrorCode::Io, "keychain_unavailable")
}

pub(super) fn delete(entry: keyring::Entry) -> Result<()> {
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(keychain_error(error)),
    }
}

const TURSO_SERVICE: &str = "app.financialgps.desktop.turso";

impl TursoSecretStore for NativeSecretVault {
    fn enroll(&self, instance: Uuid, token: &str) -> Result<()> {
        Self::entry(TURSO_SERVICE, instance)?
            .set_password(token)
            .map_err(keychain_error)
    }
    fn retrieve(&self, instance: Uuid) -> Result<Zeroizing<String>> {
        Self::entry(TURSO_SERVICE, instance)?
            .get_password()
            .map(Zeroizing::new)
            .map_err(|error| match error {
                keyring::Error::NoEntry => {
                    AppError::new(ErrorCode::Conflict, "turso_token_missing")
                }
                other => keychain_error(other),
            })
    }
    fn revoke(&self, instance: Uuid) -> Result<()> {
        delete(Self::entry(TURSO_SERVICE, instance)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{BackupSecretStore, Credential, OsKeychain, QuickUnlockStore};

    #[test]
    #[ignore = "requires an unlocked OS credential service"]
    fn native_secret_vault_round_trips_and_revokes_distinct_secret_slots() {
        let id = Uuid::new_v4();
        let vault = NativeSecretVault;
        let quick = OsKeychain;
        let token = format!("test-token-{}", Uuid::new_v4());
        let secret: Credential = serde_json::from_str("\"test backup secret\"").unwrap();
        let key = Zeroizing::new([0xde; 32]);
        let result = (|| {
            vault.enroll(id, &token)?;
            QuickUnlockStore::remember(&quick, id, &key)?;
            BackupSecretStore::remember(&quick, id, &secret)?;
            assert_eq!(vault.retrieve(id)?.as_str(), token);
            assert_eq!(*QuickUnlockStore::retrieve(&quick, id)?, *key);
            assert_eq!(
                BackupSecretStore::retrieve(&quick, id)?.0.as_str(),
                secret.0.as_str()
            );
            vault.revoke(id)?;
            assert_eq!(
                vault.retrieve(id).unwrap_err().message_key,
                "turso_token_missing"
            );
            assert_eq!(*QuickUnlockStore::retrieve(&quick, id)?, *key);
            Ok::<_, AppError>(())
        })();
        let token_result = vault.revoke(id);
        let key_result = QuickUnlockStore::revoke(&quick, id);
        let backup_result = BackupSecretStore::revoke(&quick, id);
        result.unwrap();
        token_result.unwrap();
        key_result.unwrap();
        backup_result.unwrap();
    }
}
