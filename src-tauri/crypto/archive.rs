use crate::{
    crypto::{Credential, random},
    domain::Result,
};
pub use core_engine::archive::MAX_ARCHIVE_BYTES;
use std::io::{Read, Write};

pub fn encrypt<R: Read, W: Write>(
    payload: R,
    out: W,
    passphrase: &Credential,
    cancelled: &dyn Fn() -> bool,
) -> Result<()> {
    let mut entropy = [0; 24];
    random(&mut entropy)?;
    core_engine::archive::encrypt(payload, out, &passphrase.0, entropy, cancelled)
}

pub fn decrypt<R: Read, W: Write>(
    input: R,
    output: W,
    passphrase: &Credential,
    cancelled: &dyn Fn() -> bool,
) -> Result<()> {
    core_engine::archive::decrypt(input, output, &passphrase.0, cancelled)
}
