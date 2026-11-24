use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead};
use hkdf::Hkdf;
use zeroize::Zeroizing;

use super::{DatabaseKey, bad_credential, random};
use crate::domain::Result;

pub(crate) fn derive(key: &DatabaseKey) -> Result<DatabaseKey> {
    let mut out = Zeroizing::new([0u8; 32]);
    Hkdf::<sha2_v010::Sha256>::new(Some(b"Financial GPS attachments"), key.as_slice())
        .expand(b"attachment object keys v1", &mut *out)
        .map_err(|_| bad_credential())?;
    Ok(out)
}
fn aad(id: &str) -> Vec<u8> {
    let mut aad = b"Financial GPS attachment v1\0".to_vec();
    aad.extend_from_slice(id.as_bytes());
    aad
}
pub(crate) fn encrypt(key: &DatabaseKey, id: &str, bytes: &[u8]) -> Result<Vec<u8>> {
    let mut nonce = [0u8; 12];
    random(&mut nonce)?;
    let cipher = Aes256Gcm::new((&**key).into());
    let bytes = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            aes_gcm::aead::Payload {
                msg: bytes,
                aad: &aad(id),
            },
        )
        .map_err(|_| bad_credential())?;
    let mut out = Vec::with_capacity(13 + bytes.len());
    out.push(1);
    out.extend(nonce);
    out.extend(bytes);
    Ok(out)
}
pub(crate) fn decrypt(key: &DatabaseKey, id: &str, bytes: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    if bytes.len() < 29 || bytes[0] != 1 {
        return Err(bad_credential());
    }
    let cipher = Aes256Gcm::new((&**key).into());
    let plain = cipher
        .decrypt(
            Nonce::from_slice(&bytes[1..13]),
            aes_gcm::aead::Payload {
                msg: &bytes[13..],
                aad: &aad(id),
            },
        )
        .map_err(|_| bad_credential())?;
    Ok(Zeroizing::new(plain))
}
