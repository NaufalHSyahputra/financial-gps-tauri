pub mod archive;
pub mod native_secret_vault;
use native_secret_vault::{NativeSecretVault, delete, keychain_error};
pub(crate) mod attachment;
use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{AeadInPlace, OsRng, rand_core::RngCore},
};
use argon2::{Algorithm, Argon2, Block, Params, Version};
use base64::{Engine, engine::general_purpose::STANDARD_NO_PAD};
use serde::{Deserialize, Deserializer, Serialize};
use specta::Type;
use zeroize::Zeroizing;

use crate::domain::{AppError, ErrorCode, Result};
use uuid::Uuid;

pub type DatabaseKey = Zeroizing<[u8; 32]>;

#[derive(Type)]
#[specta(transparent)]
pub struct Credential(#[specta(type = String)] Zeroizing<String>);

impl Credential {
    pub(crate) fn copy(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<'de> Deserialize<'de> for Credential {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        deserializer.deserialize_string(CredentialVisitor)
    }
}

struct CredentialVisitor;

impl serde::de::Visitor<'_> for CredentialVisitor {
    type Value = Credential;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a credential of at most 1024 bytes")
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> std::result::Result<Credential, E> {
        self.visit_string(value.to_owned())
    }

    fn visit_string<E: serde::de::Error>(
        self,
        value: String,
    ) -> std::result::Result<Credential, E> {
        let value = Zeroizing::new(value);
        if value.len() > 1024 {
            return Err(E::custom("credential too long"));
        }
        Ok(Credential(value))
    }
}

const WRAPPER_VERSION: u8 = 1;
const MEMORY_KIB: u32 = 65_536;
const ITERATIONS: u32 = 3;
const LANES: u32 = 4;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct KeyWrapper {
    version: u8,
    memory_kib: u32,
    iterations: u32,
    lanes: u32,
    salt: [u8; 16],
    nonce: [u8; 12],
    ciphertext: [u8; 32],
    tag: [u8; 16],
}

fn random<const N: usize>(bytes: &mut [u8; N]) -> Result<()> {
    OsRng
        .try_fill_bytes(bytes)
        .map_err(|_| AppError::new(ErrorCode::Io, "system_randomness_unavailable"))
}

pub(crate) fn bad_credential() -> AppError {
    AppError::new(
        ErrorCode::BadCredentialOrCorrupt,
        "bad_credential_or_corrupt",
    )
}

impl KeyWrapper {
    pub fn enroll(credential: &Credential) -> Result<(Self, DatabaseKey)> {
        if credential.0.trim().is_empty() {
            return Err(AppError::validation("credential", "credential_required"));
        }
        let mut key = Zeroizing::new([0; 32]);
        let mut wrapper = Self {
            version: WRAPPER_VERSION,
            memory_kib: MEMORY_KIB,
            iterations: ITERATIONS,
            lanes: LANES,
            salt: [0; 16],
            nonce: [0; 12],
            ciphertext: [0; 32],
            tag: [0; 16],
        };
        random(&mut key)?;
        random(&mut wrapper.salt)?;
        random(&mut wrapper.nonce)?;
        let wrapping_key = wrapper.derive(credential)?;
        let cipher = Aes256Gcm::new((&*wrapping_key).into());
        let mut encrypted = key.clone();
        let tag = cipher
            .encrypt_in_place_detached(
                Nonce::from_slice(&wrapper.nonce),
                &wrapper.aad(),
                &mut *encrypted,
            )
            .map_err(|_| bad_credential())?;
        wrapper.ciphertext.copy_from_slice(&*encrypted);
        wrapper.tag.copy_from_slice(&tag);
        Ok((wrapper, key))
    }

    pub fn unlock(&self, credential: &Credential) -> Result<DatabaseKey> {
        let wrapping_key = self.derive(credential)?;
        let cipher = Aes256Gcm::new((&*wrapping_key).into());
        let mut key = Zeroizing::new(self.ciphertext);
        cipher
            .decrypt_in_place_detached(
                Nonce::from_slice(&self.nonce),
                &self.aad(),
                &mut *key,
                (&self.tag).into(),
            )
            .map_err(|_| bad_credential())?;
        Ok(key)
    }

    pub fn validate(&self) -> Result<()> {
        if self.version != WRAPPER_VERSION
            || !(MEMORY_KIB..=131_072).contains(&self.memory_kib)
            || !(ITERATIONS..=5).contains(&self.iterations)
            || !(1..=LANES).contains(&self.lanes)
        {
            return Err(bad_credential());
        }
        Ok(())
    }

    fn derive(&self, credential: &Credential) -> Result<DatabaseKey> {
        self.validate()?;
        let params = Params::new(self.memory_kib, self.iterations, self.lanes, Some(32))
            .map_err(|_| bad_credential())?;
        let mut wrapping_key = Zeroizing::new([0; 32]);
        let mut blocks = Zeroizing::new(vec![Block::default(); params.block_count()]);
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
            .hash_password_into_with_memory(
                credential.0.as_bytes(),
                &self.salt,
                &mut *wrapping_key,
                &mut *blocks,
            )
            .map_err(|_| bad_credential())?;
        Ok(wrapping_key)
    }

    fn aad(&self) -> Vec<u8> {
        let mut aad = b"Financial GPS ledger wrapper\0argon2id-v19\0aes-256-gcm\0".to_vec();
        aad.push(self.version);
        for value in [self.memory_kib, self.iterations, self.lanes] {
            aad.extend(value.to_be_bytes());
        }
        aad.extend(self.salt);
        aad.extend(self.nonce);
        aad
    }
}

pub trait QuickUnlockStore: Send + Sync {
    fn remember(&self, instance: Uuid, key: &DatabaseKey) -> Result<()>;
    fn retrieve(&self, instance: Uuid) -> Result<DatabaseKey>;
    fn revoke(&self, instance: Uuid) -> Result<()>;
}

pub struct OsKeychain;

const KEYCHAIN_KEY_PREFIX: &[u8; 6] = b"fgps1:";

impl QuickUnlockStore for OsKeychain {
    fn remember(&self, instance: Uuid, key: &DatabaseKey) -> Result<()> {
        // Some Secret Service backends require valid UTF-8 for keyring's text/plain item.
        let mut encoded = Zeroizing::new([0; 49]);
        encoded[..KEYCHAIN_KEY_PREFIX.len()].copy_from_slice(KEYCHAIN_KEY_PREFIX);
        STANDARD_NO_PAD
            .encode_slice(key.as_slice(), &mut encoded[KEYCHAIN_KEY_PREFIX.len()..])
            .map_err(|_| keychain_codec_error())?;
        NativeSecretVault::entry("app.financialgps.desktop.ledger", instance)?
            .set_secret(&*encoded)
            .map_err(keychain_error)
    }

    fn retrieve(&self, instance: Uuid) -> Result<DatabaseKey> {
        let secret = Zeroizing::new(
            NativeSecretVault::entry("app.financialgps.desktop.ledger", instance)?
                .get_secret()
                .map_err(keychain_error)?,
        );
        decode_keychain_key(&secret)
    }

    fn revoke(&self, instance: Uuid) -> Result<()> {
        delete(NativeSecretVault::entry(
            "app.financialgps.desktop.ledger",
            instance,
        )?)
    }
}

fn decode_keychain_key(secret: &[u8]) -> Result<DatabaseKey> {
    let mut key = Zeroizing::new([0; 32]);
    if secret.len() == key.len() {
        // Older releases stored 32 raw bytes on platforms that accepted them.
        key.copy_from_slice(secret);
        return Ok(key);
    }
    let encoded = secret
        .strip_prefix(KEYCHAIN_KEY_PREFIX)
        .ok_or_else(bad_credential)?;
    if encoded.len() != 43
        || STANDARD_NO_PAD.decode_slice(encoded, &mut *key).ok() != Some(key.len())
    {
        return Err(bad_credential());
    }
    Ok(key)
}

fn keychain_codec_error() -> AppError {
    AppError::new(ErrorCode::Integrity, "keychain_encoding_failed")
}

pub trait BackupSecretStore: Send + Sync {
    fn remember(&self, instance: Uuid, credential: &Credential) -> Result<()>;
    fn retrieve(&self, instance: Uuid) -> Result<Credential>;
    fn revoke(&self, instance: Uuid) -> Result<()>;
}
impl BackupSecretStore for OsKeychain {
    fn remember(&self, instance: Uuid, credential: &Credential) -> Result<()> {
        remember_backup(instance, credential)
    }
    fn retrieve(&self, instance: Uuid) -> Result<Credential> {
        retrieve_backup(instance)
    }
    fn revoke(&self, instance: Uuid) -> Result<()> {
        revoke_backup(instance)
    }
}

fn remember_backup(instance: Uuid, credential: &Credential) -> Result<()> {
    if credential.0.trim().is_empty() {
        return Err(AppError::validation(
            "backupPassphrase",
            "credential_required",
        ));
    }
    NativeSecretVault::entry("app.financialgps.desktop.backup", instance)?
        .set_password(&credential.0)
        .map_err(keychain_error)
}
fn retrieve_backup(instance: Uuid) -> Result<Credential> {
    let secret = Zeroizing::new(
        NativeSecretVault::entry("app.financialgps.desktop.backup", instance)?
            .get_password()
            .map_err(keychain_error)?,
    );
    if secret.is_empty() || secret.len() > 1024 {
        return Err(bad_credential());
    }
    Ok(Credential(secret))
}
fn revoke_backup(instance: Uuid) -> Result<()> {
    delete(NativeSecretVault::entry(
        "app.financialgps.desktop.backup",
        instance,
    )?)
}

#[cfg(test)]
mod keychain_tests {
    use super::*;

    #[test]
    fn keychain_accepts_legacy_binary_and_versioned_text_keys() {
        let key = [0xff; 32];
        assert_eq!(*decode_keychain_key(&key).unwrap(), key);
        let encoded = format!("fgps1:{}", STANDARD_NO_PAD.encode(key));
        assert_eq!(*decode_keychain_key(encoded.as_bytes()).unwrap(), key);
        assert!(decode_keychain_key(b"fgps1:invalid").is_err());
    }
}
