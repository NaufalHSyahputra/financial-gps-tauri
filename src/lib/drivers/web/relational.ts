import { initialize_ledger, verify_ledger, financial_normalize, ledger_schema_version, upgrade_ledger } from './generated/web_engine';
import { issue } from './errors';
import { type Opfs, type SqliteDb, sqlPort } from './opfs';
import type { SqlValue } from '@sqlite.org/sqlite-wasm';

const schemaQuery = "SELECT type,name,sql FROM sqlite_schema WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' ORDER BY type,name";
const localSetting = "key LIKE 'secret.%' OR key LIKE 'backup.%' OR key LIKE 'fx.%enabled'";
const invalidImage = () => issue('Integrity', 'invalid_turso_sync_state');
const limit = 384 * 1024 * 1024;
const signature = new TextEncoder().encode('FGSYNC\0\x01');
const quote = (value: string) => '"' + value.replaceAll('"', '""') + '"';
const schema = (db: SqliteDb) => db.selectArrays(schemaQuery) as [string, string, string][];

function assertSchema(db: SqliteDb, known: ReturnType<typeof schema>) {
  if (JSON.stringify(schema(db)) !== JSON.stringify(known)) throw issue('Integrity', 'backup_schema_mismatch');
}

export function trusted(opfs: Opfs, version = ledger_schema_version()) {
  const db = opfs.memory();
  try {
    db.exec('BEGIN');
    initialize_ledger(sqlPort(opfs.sqlite, db), JSON.stringify({
      uuid: '00000000-0000-4000-8000-000000000001',
      name: 'Relational snapshot', currency: 'USD', createdAtMs: 0
    }));
    db.exec('COMMIT');
    upgrade_ledger(sqlPort(opfs.sqlite, db), 0, version);
    return db;
  } catch (error) {
    db.close();
    throw error;
  }
}

export function isPopulated(db: SqliteDb): boolean {
  if (db.selectValue('SELECT revision<>0 FROM ledger_meta')) return true;
  const seeded = new Set(['currencies', 'ledger_meta', 'ledger_settings', 'schema_migrations', 'valid_transaction_state']);
  return schema(db).some(([type, name, ddl]) => type === 'table' && ddl.endsWith('STRICT') && !seeded.has(name)
    && Boolean(db.selectValue(`SELECT EXISTS(SELECT 1 FROM ${quote(name)})`)));
}

export function copyRelations(
  opfs: Opfs, source: SqliteDb, target: SqliteDb,
  options: { preserve?: boolean; rebuildSearch: boolean; newUuid?: string; after?: (db: SqliteDb) => void }
) {
  const reference = trusted(opfs);
  const known = schema(reference);
  reference.close();
  const sourceVersion = Number(source.selectValue('PRAGMA user_version'));
  if (sourceVersion !== ledger_schema_version()) {
    verify_ledger(sqlPort(opfs.sqlite, source));
    const legacy = trusted(opfs, sourceVersion);
    try { assertSchema(source, schema(legacy)); }
    finally { legacy.close(); }
    upgrade_ledger(sqlPort(opfs.sqlite, source), Date.now());
  }
  assertSchema(source, known);
  assertSchema(target, known);
  verify_ledger(sqlPort(opfs.sqlite, source));
  const triggers = known.filter(([type]) => type === 'trigger');
  target.exec('PRAGMA foreign_keys=OFF; BEGIN IMMEDIATE');
  try {
    const local = options.preserve ? target.selectArrays(`SELECT key,value_json FROM ledger_settings WHERE ${localSetting}`) : [];
    const revision = options.preserve ? BigInt(String(target.selectValue('SELECT CAST(revision AS TEXT) FROM ledger_meta'))) : null;
    const uuid = options.newUuid ?? (options.preserve ? String(target.selectValue('SELECT ledger_uuid FROM ledger_meta')) : null);
    triggers.forEach(([, name]) => target.exec(`DROP TRIGGER ${quote(name)}`));
    for (const [type, name, ddl] of known) {
      if (type !== 'table' || !ddl.endsWith('STRICT')) continue;
      const table = quote(name);
      const base = target.selectValues('SELECT name FROM pragma_table_xinfo(?1) WHERE hidden=0 ORDER BY cid', [name]);
      const columns = ['rowid', ...base.map(column => quote(String(column)))].join(',');
      const marks = Array.from({ length: base.length + 1 }, () => '?').join(',');
      target.exec(`DELETE FROM ${table}`);
      const rows = source.prepare(`SELECT ${columns} FROM ${table} ORDER BY rowid`);
      const write = target.prepare(`INSERT INTO ${table}(${columns}) VALUES(${marks})`);
      try {
        while (rows.step()) write.reset().bind(rows.get([])).step();
      } finally {
        rows.finalize();
        write.finalize();
      }
    }
    if (uuid) target.exec({ sql: 'UPDATE ledger_meta SET ledger_uuid=?1', bind: [uuid] });
    if (revision !== null) target.exec({ sql: 'UPDATE ledger_meta SET revision=max(revision,?1)', bind: [revision + 1n] });
    target.exec(`DELETE FROM ledger_settings WHERE ${localSetting}`);
    for (const setting of local) target.exec({ sql: 'INSERT INTO ledger_settings VALUES(?1,?2)', bind: setting });
    target.exec('DELETE FROM transaction_search');
    if (options.rebuildSearch) {
      const read = target.prepare('SELECT t.rowid,t.id,t.description,t.notes,p.name FROM transactions t LEFT JOIN payees p ON p.id=t.payee_id ORDER BY t.rowid');
      const insert = target.prepare('INSERT INTO transaction_search(rowid,transaction_id,description,notes,payee) VALUES(?1,?2,?3,?4,?5)');
      try {
        while (read.step()) {
          const row = read.get([]) as SqlValue[];
          const normalize = (value: SqlValue) => value === null ? null : financial_normalize(String(value));
          insert.reset().bind([row[0], row[1], normalize(row[2]), normalize(row[3]), normalize(row[4])]).step();
        }
      } finally {
        read.finalize(); insert.finalize();
      }
    }
    triggers.forEach(([, , ddl]) => target.exec(ddl));
    verify_ledger(sqlPort(opfs.sqlite, target));
    validatePostings(target);
    options.after?.(target);
    target.exec('COMMIT');
  } catch (error) {
    target.exec('ROLLBACK');
    throw error;
  } finally {
    target.exec('PRAGMA foreign_keys=ON');
  }
}

function validatePostings(db: SqliteDb) {
  const invalid = db.selectValue(`SELECT EXISTS(SELECT 1 FROM transactions t
    JOIN transaction_splits s ON s.transaction_id=t.id GROUP BY t.id
    HAVING count(*)<2 OR sum(s.amount_minor)<>t.amount_minor OR t.category_id IS NOT NULL OR t.entry_kind<>'ordinary')
    OR EXISTS(SELECT 1 FROM transfer_links l JOIN transactions d ON d.id=l.debit_id JOIN transactions c ON c.id=l.credit_id
    WHERE d.direction<>'debit' OR c.direction<>'credit' OR d.account_id=c.account_id OR d.ignored OR c.ignored
    OR d.category_id IS NOT NULL OR c.category_id IS NOT NULL OR d.entry_kind<>'ordinary' OR c.entry_kind<>'ordinary'
    OR (d.currency_code=c.currency_code AND (d.amount_minor<>c.amount_minor OR l.applied_rate_decimal IS NOT NULL))
    OR (d.currency_code<>c.currency_code AND l.applied_rate_decimal IS NULL)
    OR EXISTS(SELECT 1 FROM transaction_splits s WHERE s.transaction_id IN(d.id,c.id)))`);
  if (invalid) throw issue('Integrity', 'invalid_archive_postings');
}

export type Files = Map<string, Uint8Array<ArrayBuffer>>;
export type LedgerImage = { database: Uint8Array<ArrayBuffer>; files: Files };

export async function checkImage(opfs: Opfs, image: LedgerImage) {
  const snapshot = opfs.fromBytes(image.database);
  try {
    verify_ledger(sqlPort(opfs.sqlite, snapshot));
    const reference = trusted(opfs, Number(snapshot.selectValue('PRAGMA user_version')));
    try { assertSchema(snapshot, schema(reference)); }
    finally { reference.close(); }
    validatePostings(snapshot);
    if (snapshot.selectValue('PRAGMA integrity_check') !== 'ok') throw invalidImage();
    const rows = snapshot.selectArrays("SELECT id,sha256,size_bytes,state FROM attachments ORDER BY id");
    if (rows.length !== image.files.size) throw invalidImage();
    for (const [id, digest, size, state] of rows) {
      const content = image.files.get(String(id));
      if (!content || state !== 'ready' || content.byteLength !== Number(size) || await sha256(content) !== digest) throw invalidImage();
    }
  } finally {
    snapshot.close();
  }
}

export async function sha256(bytes: Uint8Array<ArrayBuffer>) {
  const digest = await crypto.subtle.digest('SHA-256', bytes);
  return Array.from(new Uint8Array(digest), byte => byte.toString(16).padStart(2, '0')).join('');
}

export function assemble({ database, files }: LedgerImage) {
  const length = database.byteLength + [...files.values()].reduce((sum, file) => sum + 20 + file.byteLength, 16);
  if (length > limit) throw issue('Validation', 'turso_sync_image_limit');
  const bytes = new Uint8Array(length);
  const view = new DataView(bytes.buffer);
  bytes.set(signature);
  view.setUint32(8, database.byteLength);
  view.setUint32(12, files.size);
  bytes.set(database, 16);
  let offset = 16 + database.byteLength;
  for (const [id, content] of files) {
    const hex = id.replaceAll('-', '');
    if (!/^[0-9a-f]{32}$/.test(hex)) throw invalidImage();
    for (let i = 0; i < 16; i++) bytes[offset + i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
    view.setUint32(offset + 16, content.byteLength);
    bytes.set(content, offset + 20);
    offset += 20 + content.byteLength;
  }
  return bytes;
}

export function disassemble(bytes: Uint8Array<ArrayBuffer>): LedgerImage {
  if (bytes.byteLength > limit || bytes.byteLength < 16 || !signature.every((value, index) => value === bytes[index])) throw invalidImage();
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const dbLength = view.getUint32(8);
  const count = view.getUint32(12);
  let offset = 16 + dbLength;
  if (dbLength < 100 || offset > bytes.byteLength) throw invalidImage();
  const files: Files = new Map();
  for (let i = 0; i < count; i++) {
    if (offset + 20 > bytes.byteLength) throw invalidImage();
    const hex = Array.from(bytes.subarray(offset, offset + 16), byte => byte.toString(16).padStart(2, '0')).join('');
    const id = [hex.slice(0, 8), hex.slice(8, 12), hex.slice(12, 16), hex.slice(16, 20), hex.slice(20)].join('-');
    const length = view.getUint32(offset + 16);
    if (!length || length > 10_000_000 || offset + 20 + length > bytes.byteLength || files.has(id)) throw invalidImage();
    files.set(id, bytes.subarray(offset + 20, offset + 20 + length));
    offset += 20 + length;
  }
  if (offset !== bytes.byteLength) throw invalidImage();
  return { database: bytes.subarray(16, 16 + dbLength), files };
}
