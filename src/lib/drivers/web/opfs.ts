import sqlite3InitModule, { type Database, type SAHPoolUtil, type SqlValue } from '@sqlite.org/sqlite-wasm';
import { financial_normalize } from './generated/web_engine';
import { issue } from './errors';

type Cell =
  | { type: 'null' }
  | { type: 'integer' | 'text'; value: string }
  | { type: 'real'; value: number }
  | { type: 'blob'; value: number[] };

export type Sqlite = Awaited<ReturnType<typeof sqlite3InitModule>>;
export type SqliteDb = Database;

function unpack(cell: Cell): SqlValue {
  if (cell.type === 'null') return null;
  if (cell.type === 'integer') return BigInt(cell.value);
  if (cell.type === 'blob') return Uint8Array.from(cell.value);
  return cell.value as string | number;
}

function sqliteError(error: unknown): never {
  const message = error instanceof Error ? error.message : '';
  if (message.includes('integer overflow')) throw 'integer overflow';
  if (/busy|locked/i.test(message)) throw 'busy';
  if (/constraint/i.test(message)) throw 'constraint';
  throw 'database error';
}

/** Synchronous connection object passed to the shared Rust SQL port. */
export function sqlPort(sqlite: Sqlite, db: SqliteDb) {
  return {
    executeBatch(sql: string) {
      try { db.exec(sql); } catch (error) { sqliteError(error); }
    },
    prepare(sql: string) {
      try {
        const stmt = db.prepare(sql);
        return {
          bind(args: string) {
            try {
              stmt.reset();
              const values = (JSON.parse(args) as Cell[]).map(unpack);
              if (values.length) stmt.bind(values);
            }
            catch (error) { sqliteError(error); }
          },
          next() {
            try {
              if (!stmt.step()) return;
              const cells: Cell[] = [];
              for (let i = 0; i < stmt.columnCount; i++) {
                const type = sqlite.capi.sqlite3_column_type(stmt.pointer!, i);
                const value = stmt.get(i);
                switch (type) {
                  case sqlite.capi.SQLITE_INTEGER: cells.push({ type: 'integer', value: String(value) }); break;
                  case sqlite.capi.SQLITE_FLOAT: cells.push({ type: 'real', value: Number(value) }); break;
                  case sqlite.capi.SQLITE_TEXT: cells.push({ type: 'text', value: String(value) }); break;
                  case sqlite.capi.SQLITE_BLOB: cells.push({ type: 'blob', value: Array.from(value as Uint8Array) }); break;
                  default: cells.push({ type: 'null' });
                }
              }
              return JSON.stringify(cells);
            } catch (error) { sqliteError(error); }
          },
          execute() {
            try { stmt.step(); return Number(db.changes()); }
            catch (error) { sqliteError(error); }
          },
          close() { stmt.finalize(); }
        };
      } catch (error) { sqliteError(error); }
    }
  };
}

export function configure(db: SqliteDb) {
  db.exec('PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL; PRAGMA busy_timeout=5000; PRAGMA temp_store=MEMORY; PRAGMA trusted_schema=OFF;');
  db.createFunction({
    name: 'financial_normalize', arity: 1, deterministic: true, directOnly: true,
    xFunc: (_context: number, value: SqlValue) => {
      if (typeof value !== 'string') throw new Error('Expected SQLite text');
      return financial_normalize(value);
    }
  });
  return db;
}

const ledgerContext = (id: string) => new TextEncoder().encode(`financial-gps-ledger:v1:${id}`);

export class Opfs {
  private constructor(readonly sqlite: Sqlite, readonly pool: SAHPoolUtil, private readonly root: FileSystemDirectoryHandle) {}

  static async open(binary: ArrayBuffer) {
    const init = sqlite3InitModule as (options: { wasmBinary: ArrayBuffer }) => Promise<Sqlite>;
    const sqlite = await init({ wasmBinary: binary });
    const root = await (await navigator.storage.getDirectory()).getDirectoryHandle('financial-gps', { create: true });
    // SQLite supports retrying a preserved VFS even though its typings omit the option.
    const poolOptions = {
      directory: '.financial-gps-sqlite',
      preserveOnInitFailure: true,
      forceReinitIfPreviouslyFailed: true
    };
    try {
      const pool = await sqlite.installOpfsSAHPoolVfs(poolOptions);
      return new Opfs(sqlite, pool, root);
    } catch (error) {
      if (error instanceof DOMException && error.name === 'NoModificationAllowedError') {
        throw issue('Busy', 'browser_activity_in_use');
      }
      throw error;
    }
  }

  file(id: string) { return `/${id}.db`; }

  async create(id: string) {
    await this.pool.reserveMinimumCapacity(this.pool.getFileCount() + 4);
    return configure(new this.pool.OpfsSAHPoolDb(this.file(id)));
  }

  openPlain(id: string) {
    if (!this.pool.getFileNames().includes(this.file(id))) throw issue('Io', 'file_not_found');
    return configure(new this.pool.OpfsSAHPoolDb(this.file(id)));
  }

  async importPlain(id: string, contents: Uint8Array<ArrayBuffer>) {
    await this.pool.reserveMinimumCapacity(this.pool.getFileCount() + 4);
    await this.pool.importDb(this.file(id), contents);
    return this.openPlain(id);
  }

  removePlain(id: string) { this.pool.unlink(this.file(id)); }

  memory() { return configure(new this.sqlite.oo1.DB(':memory:', 'c')); }

  fromBytes(bytes: Uint8Array<ArrayBuffer>) {
    const db = this.memory();
    const { capi, wasm } = this.sqlite;
    const pointer = wasm.allocFromTypedArray(bytes);
    try {
      db.checkRc(capi.sqlite3_deserialize(
        db.pointer!, 'main', pointer, bytes.byteLength, bytes.byteLength,
        capi.SQLITE_DESERIALIZE_FREEONCLOSE | capi.SQLITE_DESERIALIZE_RESIZEABLE
      ));
      return db;
    } catch (error) {
      db.close();
      throw error;
    }
  }

  image(db: SqliteDb): Uint8Array<ArrayBuffer> {
    return this.sqlite.capi.sqlite3_js_db_export(db.pointer!);
  }

  async saveProtected(id: string, db: SqliteDb, key: CryptoKey) {
    const iv = crypto.getRandomValues(new Uint8Array(12));
    const image = this.image(db);
    let cipher: ArrayBuffer;
    try {
      cipher = await crypto.subtle.encrypt({ name: 'AES-GCM', iv, additionalData: ledgerContext(id) }, key, image);
    } finally {
      image.fill(0);
    }
    await this.write(`${id}.protected`, [iv, cipher]);
  }

  async openProtected(id: string, key: CryptoKey) {
    const encrypted = new Uint8Array(await (await this.read(`${id}.protected`)).arrayBuffer());
    if (encrypted.byteLength < 28) throw issue('BadCredentialOrCorrupt', 'bad_credential_or_corrupt');
    let image: Uint8Array<ArrayBuffer>;
    try {
      image = new Uint8Array(await crypto.subtle.decrypt({
        name: 'AES-GCM', iv: encrypted.subarray(0, 12), additionalData: ledgerContext(id)
      }, key, encrypted.subarray(12)));
    } catch {
      throw issue('BadCredentialOrCorrupt', 'bad_credential_or_corrupt');
    }
    try { return this.fromBytes(image); } finally { image.fill(0); }
  }

  async write(name: string, chunks: BlobPart[]) {
    const handle = await this.root.getFileHandle(name, { create: true });
    const target = await handle.createWritable();
    try {
      for (const chunk of chunks) await target.write(chunk);
      await target.close();
    } catch (error) {
      await target.abort().catch(() => {});
      throw error;
    }
  }

  async read(name: string) { return (await this.root.getFileHandle(name)).getFile(); }
  async remove(name: string) { await this.root.removeEntry(name); }
}
