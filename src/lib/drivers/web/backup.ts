import { decrypt_archive, encrypt_archive, ledger_schema_version } from './generated/web_engine';
import type { BackupManifest } from '../contracts';
import { issue } from './errors';
import { type Opfs } from './opfs';
import { checkImage, sha256, type LedgerImage } from './relational';

const invalidArchive = () => issue('Integrity', 'invalid_backup_manifest');
const jsonLimit = 1024 * 1024;

export async function encodeBackup(opfs: Opfs, image: LedgerImage, password: string) {
  await checkImage(opfs, image);
  const db = opfs.fromBytes(image.database);
  let manifest: BackupManifest;
  try {
    const [uuid, revision] = db.selectArray('SELECT ledger_uuid,CAST(revision AS TEXT) FROM ledger_meta')!;
    const tables = db.selectValues("SELECT name FROM sqlite_schema WHERE type='table' AND substr(sql,-6)='STRICT' ORDER BY name");
    const counts = Object.fromEntries(tables.map(table => [
      String(table), String(db.selectValue(`SELECT count(*) FROM "${String(table).replaceAll('"', '""')}"`))
    ]));
    const attachments = [];
    for (const [id, contents] of image.files) {
      attachments.push({ id, entry: `attachments/${id}.blob`, size: String(contents.byteLength), sha256: await sha256(contents) });
    }
    manifest = {
      formatVersion: 1, schemaVersion: Number(db.selectValue('PRAGMA user_version')), ledgerUuid: String(uuid), sourceRevision: String(revision),
      createdAt: new Date().toISOString(), databaseEntry: 'ledger.db', databaseSize: String(image.database.byteLength),
      databaseSha256: await sha256(image.database), counts, attachments
    };
  } finally { db.close(); }
  const metadata = new TextEncoder().encode(JSON.stringify(manifest));
  if (metadata.byteLength > jsonLimit) throw invalidArchive();
  const length = 4 + metadata.byteLength + image.database.byteLength + [...image.files.values()].reduce((n, v) => n + v.byteLength, 0);
  if (length > 384 * 1024 * 1024) throw issue('Validation', 'archive_size_limit');
  const payload = new Uint8Array(length);
  new DataView(payload.buffer).setUint32(0, metadata.byteLength);
  payload.set(metadata, 4);
  payload.set(image.database, 4 + metadata.byteLength);
  let offset = 4 + metadata.byteLength + image.database.byteLength;
  for (const contents of image.files.values()) {
    payload.set(contents, offset);
    offset += contents.byteLength;
  }
  try {
    return {
      archive: new Uint8Array(encrypt_archive(payload, password, crypto.getRandomValues(new Uint8Array(24)))),
      manifest
    };
  } finally { payload.fill(0); }
}

export async function decodeBackup(opfs: Opfs, archive: Uint8Array<ArrayBuffer>, password: string) {
  if (archive.byteLength > 400 * 1024 * 1024) throw invalidArchive();
  const payload = decrypt_archive(archive, password);
  try {
    if (payload.byteLength < 4) throw invalidArchive();
    const size = new DataView(payload.buffer, payload.byteOffset, payload.byteLength).getUint32(0);
    if (size > jsonLimit || size + 4 > payload.byteLength) throw invalidArchive();
    const manifest: BackupManifest = JSON.parse(new TextDecoder().decode(payload.subarray(4, 4 + size)));
    if (manifest.formatVersion !== 1 || !Number.isInteger(manifest.schemaVersion) || manifest.schemaVersion < 1 || manifest.schemaVersion > ledger_schema_version()) throw issue('UnsupportedSchema', 'unsupported_backup_version');
    let offset = 4 + size;
    const read = (count: string, maximum: number) => {
      const length = Number(count);
      if (!/^\d+$/.test(count) || !length || length > maximum || offset + length > payload.byteLength) throw invalidArchive();
      const data = payload.slice(offset, offset + length);
      offset += length;
      return data;
    };
    if (manifest.databaseEntry !== 'ledger.db') throw invalidArchive();
    const database = read(manifest.databaseSize, 256 * 1024 * 1024);
    if (await sha256(database) !== manifest.databaseSha256) throw invalidArchive();
    const files: LedgerImage['files'] = new Map();
    for (const item of manifest.attachments) {
      if (files.has(item.id) || item.entry !== `attachments/${item.id}.blob`) throw invalidArchive();
      const bytes = read(item.size, 10_000_000);
      if (await sha256(bytes) !== item.sha256) throw invalidArchive();
      files.set(item.id, bytes);
    }
    if (offset !== payload.byteLength) throw invalidArchive();
    const image = { database, files };
    await checkImage(opfs, image);
    const db = opfs.fromBytes(database);
    try {
      const [uuid, revision] = db.selectArray('SELECT ledger_uuid,CAST(revision AS TEXT) FROM ledger_meta')!;
      const tables = db.selectValues("SELECT name FROM sqlite_schema WHERE type='table' AND substr(sql,-6)='STRICT' ORDER BY name");
      if (uuid !== manifest.ledgerUuid || revision !== manifest.sourceRevision
        || Number(db.selectValue('PRAGMA user_version')) !== manifest.schemaVersion
        || Object.keys(manifest.counts).length !== tables.length
        || tables.some(table => manifest.counts[String(table)] !== String(db.selectValue(`SELECT count(*) FROM "${String(table).replaceAll('"', '""')}"`)))) {
        throw invalidArchive();
      }
    } finally { db.close(); }
    return { image, manifest };
  } finally { payload.fill(0); }
}
