use std::io::{Cursor, Read};

use rusqlite::Transaction;
use uuid::Uuid;
use zeroize::Zeroizing;

use super::{MAX_IMAGE, corrupt};
use crate::{
    domain::{AppError, Result},
    storage::{
        attachments::Vault,
        snapshot::{self, Object, RestoredArchive},
    },
};

const MAGIC: &[u8; 8] = b"FGSYNC\0\x01";

pub(in crate::storage) fn capture(
    tx: &Transaction<'_>,
    vault: &Vault<'_>,
    cancelled: &dyn Fn() -> bool,
) -> Result<Zeroizing<Vec<u8>>> {
    // Export from trusted relational tables, excluding connection-local metadata.
    let image = snapshot::relational_snapshot(tx, cancelled)?;
    let objects = vault.inventory(tx)?;
    let total = image.len() + 16 + objects.iter().map(|o| 20 + o.bytes.len()).sum::<usize>();
    if total > MAX_IMAGE {
        return Err(AppError::validation("ledger", "turso_sync_image_limit"));
    }
    let mut data = Zeroizing::new(Vec::with_capacity(total));
    data.extend_from_slice(MAGIC);
    data.extend_from_slice(&(image.len() as u32).to_be_bytes());
    data.extend_from_slice(&(objects.len() as u32).to_be_bytes());
    data.extend_from_slice(&image);
    for object in objects {
        let id = Uuid::parse_str(&object.id).map_err(|_| corrupt())?;
        data.extend_from_slice(id.as_bytes());
        data.extend_from_slice(&(object.bytes.len() as u32).to_be_bytes());
        data.extend_from_slice(&object.bytes);
    }
    Ok(data)
}

pub(in crate::storage) fn decode(bytes: &[u8]) -> Result<RestoredArchive> {
    if bytes.len() > MAX_IMAGE || !bytes.starts_with(MAGIC) {
        return Err(corrupt());
    }
    let mut cursor = Cursor::new(&bytes[MAGIC.len()..]);
    let len = read_u32(&mut cursor)?;
    let count = read_u32(&mut cursor)?;
    let image = slice(&mut cursor, len)?;
    let mut database = snapshot::load_database(image)?;
    let mut objects = Vec::new();
    for _ in 0..count {
        let mut id = [0; 16];
        cursor.read_exact(&mut id).map_err(|_| corrupt())?;
        let len = read_u32(&mut cursor)?;
        if len == 0 || len > crate::storage::attachments::MAX_ATTACHMENT {
            return Err(corrupt());
        }
        objects.push(Object {
            id: Uuid::from_bytes(id).to_string(),
            bytes: Zeroizing::new(slice(&mut cursor, len)?.to_vec()),
        });
    }
    if cursor.position() as usize != cursor.get_ref().len() {
        return Err(corrupt());
    }
    let manifest = snapshot::manifest(&database, image, &objects, chrono::Utc::now())?;
    super::super::migration::upgrade_memory(&mut database)?;
    Ok(RestoredArchive {
        manifest,
        database,
        objects,
    })
}

fn read_u32(input: &mut Cursor<&[u8]>) -> Result<usize> {
    let mut data = [0; 4];
    input.read_exact(&mut data).map_err(|_| corrupt())?;
    Ok(u32::from_be_bytes(data) as usize)
}
fn slice<'a>(cursor: &mut Cursor<&'a [u8]>, len: usize) -> Result<&'a [u8]> {
    let start = cursor.position() as usize;
    let end = start.checked_add(len).ok_or_else(corrupt)?;
    let bytes = cursor.get_ref().get(start..end).ok_or_else(corrupt)?;
    cursor.set_position(end as u64);
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{migration, sqlite_view};
    use rusqlite::Connection;

    #[test]
    fn sync_snapshots_upgrade_v1_v2_and_preserve_v3_category_icons() {
        for version in 1..=migration::SCHEMA_VERSION {
            let mut ledger = Connection::open_in_memory().unwrap();
            migration::initialize(
                &mut ledger,
                migration::LedgerMetadata {
                    uuid: "00000000-0000-4000-8000-000000000001",
                    name: "Remote device",
                    timezone: None,
                    currency: "IDR",
                    created_at_ms: 0,
                },
            )
            .unwrap();
            migration::upgrade_memory_to(&mut ledger, version).unwrap();
            ledger.execute("INSERT INTO accounts(id,name,kind,currency_code) VALUES('bank','Old Bank','checking','IDR')", []).unwrap();
            ledger
                .execute(
                    "INSERT INTO categories(id,name,kind) VALUES('food','Groceries','expense')",
                    [],
                )
                .unwrap();
            let expected =
                (version == migration::SCHEMA_VERSION).then_some("shopping-cart".to_owned());
            if let Some(icon) = &expected {
                ledger
                    .execute("UPDATE categories SET icon=?1 WHERE id='food'", [icon])
                    .unwrap();
            }
            let bytes = ledger.serialize("main").unwrap();
            let mut payload = MAGIC.to_vec();
            payload.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
            payload.extend_from_slice(&0_u32.to_be_bytes());
            payload.extend_from_slice(&bytes);

            let received = decode(&payload).unwrap();
            assert_eq!(received.manifest.schema_version, version);
            migration::verify(&received.database).unwrap();
            assert_eq!(
                core_engine::migrations::current_version(&sqlite_view(&received.database)).unwrap(),
                migration::SCHEMA_VERSION
            );
            let data = received.database.query_row(
                "SELECT a.name,c.icon FROM accounts a CROSS JOIN categories c WHERE a.id='bank' AND c.id='food'",
                [], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            ).unwrap();
            assert_eq!(data, ("Old Bank".to_owned(), expected));
        }
    }
}
