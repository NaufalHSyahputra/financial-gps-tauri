use std::io::{Read, Write};

use aes_gcm::{Aes256Gcm, KeyInit, Nonce, Tag, aead::AeadInPlace};
use argon2::{Algorithm, Argon2, Block, Params, Version};
use zeroize::Zeroizing;

use crate::{
    domain::io_error,
    domain::{AppError, ErrorCode, Result},
};

pub const FRAME_BYTES: usize = 1024 * 1024;
pub const MAX_ARCHIVE_BYTES: u64 = 384 * 1024 * 1024;
const MAGIC: &[u8; 8] = b"FINGPSBK";
const HEADER_SIZE: usize = 46;
const PARAMETERS: [u8; 14] = [1, 1, 0, 1, 0, 0, 0, 0, 0, 3, 0, 0, 0, 4];

type DatabaseKey = Zeroizing<[u8; 32]>;

fn bad_credential() -> AppError {
    AppError::new(
        ErrorCode::BadCredentialOrCorrupt,
        "bad_credential_or_corrupt",
    )
}

pub fn derive(passphrase: &str, salt: &[u8]) -> Result<DatabaseKey> {
    if passphrase.trim().is_empty() {
        return Err(AppError::validation(
            "backupPassphrase",
            "credential_required",
        ));
    }
    let params = Params::new(65_536, 3, 4, Some(32)).map_err(|_| bad_credential())?;
    let mut memory = Zeroizing::new(vec![Block::default(); params.block_count()]);
    let mut key = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into_with_memory(passphrase.as_bytes(), salt, &mut *key, &mut memory)
        .map_err(|_| bad_credential())?;
    Ok(key)
}
fn load(reader: &mut impl Read, bytes: &mut [u8]) -> Result<()> {
    reader.read_exact(bytes).map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            bad_credential()
        } else {
            io_error(error)
        }
    })
}
fn interrupted() -> AppError {
    AppError::new(ErrorCode::Cancelled, "operation_cancelled")
}
fn frame_bind(
    header: &[u8; HEADER_SIZE],
    index: u32,
    descriptor: &[u8; 5],
) -> ([u8; 12], [u8; HEADER_SIZE + 9]) {
    let index = index.to_be_bytes();
    let mut nonce = [0; 12];
    nonce[..8].copy_from_slice(&header[38..46]);
    nonce[8..].copy_from_slice(&index);
    let mut aad = [0; HEADER_SIZE + 9];
    aad[..HEADER_SIZE].copy_from_slice(header);
    aad[HEADER_SIZE..HEADER_SIZE + 4].copy_from_slice(&index);
    aad[HEADER_SIZE + 4..].copy_from_slice(descriptor);
    (nonce, aad)
}

pub fn encrypt<R: Read, W: Write>(
    mut payload: R,
    mut out: W,
    passphrase: &str,
    entropy: [u8; 24],
    cancelled: &dyn Fn() -> bool,
) -> Result<()> {
    let (salt, prefix) = entropy.split_at(16);
    let mut header = [0u8; HEADER_SIZE];
    header[..8].copy_from_slice(MAGIC);
    header[8..22].copy_from_slice(&PARAMETERS);
    header[22..38].copy_from_slice(salt);
    header[38..].copy_from_slice(prefix);
    let key = derive(passphrase, salt)?;
    let cipher = Aes256Gcm::new((&*key).into());
    out.write_all(&header).map_err(io_error)?;
    let mut buffer = Zeroizing::new(vec![0u8; FRAME_BYTES]);
    let mut index = 0u32;
    let mut total = 0u64;
    loop {
        if cancelled() {
            return Err(interrupted());
        }
        let mut size = 0;
        while size < buffer.len() {
            match payload.read(&mut buffer[size..]).map_err(io_error)? {
                0 => break,
                n => size += n,
            }
        }
        total += size as u64;
        if total > MAX_ARCHIVE_BYTES {
            return Err(AppError::validation("backup", "archive_size_limit"));
        }
        let mut descriptor = [0u8; 5];
        descriptor[..4].copy_from_slice(&(size as u32).to_be_bytes());
        descriptor[4] = u8::from(size == 0);
        let (nonce, aad) = frame_bind(&header, index, &descriptor);
        let tag = cipher
            .encrypt_in_place_detached(Nonce::from_slice(&nonce), &aad, &mut buffer[..size])
            .map_err(|_| bad_credential())?;
        out.write_all(&descriptor).map_err(io_error)?;
        out.write_all(&buffer[..size]).map_err(io_error)?;
        out.write_all(&tag).map_err(io_error)?;
        if size == 0 {
            break;
        }
        index = index.checked_add(1).ok_or_else(bad_credential)?;
    }
    out.flush().map_err(io_error)
}

/// The sink must be private until the authenticated terminal frame has been verified.
pub fn decrypt<R: Read, W: Write>(
    mut input: R,
    mut output: W,
    passphrase: &str,
    cancelled: &dyn Fn() -> bool,
) -> Result<()> {
    let mut header = [0u8; HEADER_SIZE];
    load(&mut input, &mut header)?;
    if &header[..8] != MAGIC || header[8..22] != PARAMETERS {
        return Err(bad_credential());
    }
    let key = derive(passphrase, &header[22..38])?;
    let cipher = Aes256Gcm::new((&*key).into());
    let mut buffer = Zeroizing::new(vec![0u8; FRAME_BYTES]);
    let (mut total, mut index) = (0u64, 0u32);
    loop {
        if cancelled() {
            return Err(interrupted());
        }
        let mut descriptor = [0u8; 5];
        load(&mut input, &mut descriptor)?;
        let size = u32::from_be_bytes(descriptor[..4].try_into().expect("frame length")) as usize;
        total += size as u64;
        let final_frame = descriptor[4] == 1;
        if descriptor[4] > 1
            || size > FRAME_BYTES
            || final_frame != (size == 0)
            || total > MAX_ARCHIVE_BYTES
        {
            return Err(bad_credential());
        }
        let mut tag = [0u8; 16];
        load(&mut input, &mut buffer[..size])?;
        load(&mut input, &mut tag)?;
        let (nonce, aad) = frame_bind(&header, index, &descriptor);
        cipher
            .decrypt_in_place_detached(
                Nonce::from_slice(&nonce),
                &aad,
                &mut buffer[..size],
                Tag::from_slice(&tag),
            )
            .map_err(|_| bad_credential())?;
        if final_frame {
            if input.read(&mut [0u8; 1]).map_err(io_error)? != 0 {
                return Err(bad_credential());
            }
            return output.flush().map_err(io_error);
        }
        output.write_all(&buffer[..size]).map_err(io_error)?;
        index = index.checked_add(1).ok_or_else(bad_credential)?;
    }
}
