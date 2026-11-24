import type { Scope, SetStorageMode, SyncStatus_Serialize, TursoCredentials } from '../contracts';
import { asError, issue } from './errors';
import { sha256 } from './relational';

export type Binding = { url: string; device: string; local: number; acknowledged: number; base: number; last_success_at?: string | null };
type Resolution = { generation: string; discardVersion?: string };
type TypedValue = { type: 'integer' | 'text'; value: string } | { type: 'blob'; base64: string } | { type: 'null' };
type Commit = { generation: number; id: string; device: string; version: number; digest: string; parts: number };

export const stateKey = 'secret.sync.v1';
const chunkSize = 256 * 1024;
const maxImage = 384 * 1024 * 1024;
const corrupt = () => issue('Integrity', 'invalid_turso_sync_state');
const conflict = (generation: number) => issue('Conflict', 'turso_concurrent_ledger_edits', { remoteGeneration: String(generation) });
const integer = (value: number): TypedValue => ({ type: 'integer', value: String(value) });
const text = (value: string): TypedValue => ({ type: 'text', value });

export function endpoint(input: string): URL {
  if (typeof input !== 'string' || !(input.startsWith('https://') || input.startsWith('libsql://'))) {
    throw issue('Validation', 'invalid_input', { url: 'invalid_turso_endpoint' });
  }
  const match = /^(?:https|libsql):\/\/([a-z\d-]+(?:\.[a-z\d-]+)*\.turso\.io)\/?$/i.exec(input);
  if (!match || input.length > 260 || match[1].split('.').some(label => label.length > 63 || label.startsWith('-') || label.endsWith('-'))) {
    throw issue('Validation', 'invalid_input', { url: 'invalid_turso_endpoint' });
  }
  return new URL(`https://${match[1].toLowerCase()}/`);
}

function readInt(cell: TypedValue | undefined) {
  if (cell?.type !== 'integer' || !/^\d+$/.test(cell.value)) throw corrupt();
  const value = Number(cell.value);
  if (!Number.isSafeInteger(value)) throw corrupt();
  return value;
}

class Turso {
  private readonly origin: URL;
  constructor(url: string, private readonly token: string) {
    this.origin = endpoint(url);
    if (!/^[!-~]{1,8192}$/.test(token)) throw issue('Validation', 'invalid_input', { authToken: 'invalid_turso_token' });
  }

  private async execute(sql: string, args: TypedValue[] = []) {
    const address = new URL('/v3/pipeline', this.origin);
    if (endpoint(this.origin.href).origin !== address.origin || address.protocol !== 'https:') throw corrupt();
    let response: Response;
    try {
      response = await fetch(address, {
        method: 'POST', headers: { 'content-type': 'application/json', authorization: `Bearer ${this.token}` },
        body: JSON.stringify({ requests: [{ type: 'execute', stmt: { sql, args, want_rows: true } }, { type: 'close' }] }),
        redirect: 'error', credentials: 'omit', referrerPolicy: 'no-referrer', cache: 'no-store', signal: AbortSignal.timeout(15_000)
      });
    } catch {
      throw issue('Offline', 'turso_unreachable');
    }
    if (response.status === 401 || response.status === 403) throw issue('Validation', 'turso_auth_rejected');
    if (response.status >= 500 || response.status === 429) throw issue('Offline', 'turso_unreachable');
    if (!response.ok) throw issue('Io', 'turso_protocol_rejected');
    const reader = response.body?.getReader();
    if (!reader) throw corrupt();
    let total = 0;
    const decoder = new TextDecoder();
    let json = '';
    try {
      for (;;) {
        const { value, done } = await reader.read();
        if (done) break;
        total += value.byteLength;
        if (total > 512 * 1024) throw corrupt();
        json += decoder.decode(value, { stream: true });
      }
      json += decoder.decode();
    } finally { await reader.cancel().catch(() => {}); }
    const result = JSON.parse(json).results?.[0];
    if (result?.type === 'error') {
      if (['SQLITE_AUTH', 'SQLITE_PERM', 'SQLITE_READONLY'].includes(result.error?.code)) throw issue('Validation', 'turso_auth_rejected');
      throw issue('Io', 'turso_sql_rejected');
    }
    if (result?.response?.type !== 'execute' || !Array.isArray(result.response.result?.rows)) throw corrupt();
    return result.response.result as { rows: TypedValue[][]; affected_row_count: number };
  }

  async probe() {
    if (readInt((await this.execute('SELECT 1')).rows[0]?.[0]) !== 1) throw corrupt();
  }

  async initialize() {
    await this.execute(`CREATE TABLE IF NOT EXISTS fgps_sync_commits_v1(
      generation INTEGER PRIMARY KEY CHECK(generation>0), id TEXT NOT NULL UNIQUE, device TEXT NOT NULL,
      local_version INTEGER NOT NULL CHECK(local_version>0), digest TEXT NOT NULL, parts INTEGER NOT NULL CHECK(parts>0),
      UNIQUE(device,local_version)) STRICT`);
    await this.execute(`CREATE TABLE IF NOT EXISTS fgps_sync_parts_v1(
      id TEXT NOT NULL, part INTEGER NOT NULL, payload BLOB NOT NULL, PRIMARY KEY(id,part)) STRICT`);
  }

  private async commit(where: string, args?: TypedValue[]): Promise<Commit | undefined> {
    const [row] = (await this.execute(`SELECT generation,id,device,local_version,digest,parts FROM fgps_sync_commits_v1 ${where}`, args)).rows;
    if (!row) return;
    const extract = (cell: TypedValue | undefined) => {
      if (cell?.type !== 'text') throw corrupt();
      return cell.value;
    };
    const item = { generation: readInt(row[0]), id: extract(row[1]), device: extract(row[2]), version: readInt(row[3]), digest: extract(row[4]), parts: readInt(row[5]) };
    if (!item.generation || !item.version || !item.parts || item.parts > Math.ceil(maxImage / chunkSize) || !/^[\da-f]{64}$/i.test(item.digest)) throw corrupt();
    return item;
  }

  head() { return this.commit('ORDER BY generation DESC LIMIT 1'); }
  latest(device: string) { return this.commit('WHERE device=?1 ORDER BY local_version DESC LIMIT 1', [text(device)]); }

  async publish(base: number, binding: Binding, bytes: Uint8Array<ArrayBuffer>, active: () => void) {
    const id = crypto.randomUUID();
    let accepted = false;
    try {
      for (let offset = 0; offset < bytes.byteLength; offset += chunkSize) {
        active();
        const block = bytes.subarray(offset, offset + chunkSize);
        await this.execute('INSERT INTO fgps_sync_parts_v1 VALUES(?1,?2,?3)', [
          text(id), integer(offset / chunkSize), { type: 'blob', base64: block.toBase64({ omitPadding: true }) }
        ]);
      }
      const digest = await sha256(bytes);
      active();
      accepted = (await this.execute(`INSERT INTO fgps_sync_commits_v1 SELECT ?1+1,?2,?3,?4,?5,?6
        WHERE ?1=(SELECT coalesce(max(generation),0) FROM fgps_sync_commits_v1)
        AND ?6=(SELECT count(*) FROM fgps_sync_parts_v1 WHERE id=?2) ON CONFLICT DO NOTHING`, [
        integer(base), text(id), text(binding.device), integer(binding.local), text(digest), integer(Math.ceil(bytes.byteLength / chunkSize))
      ])).affected_row_count === 1;
      return accepted;
    } finally {
      if (!accepted) {
        try {
          active();
          await this.execute('DELETE FROM fgps_sync_parts_v1 WHERE id=?1 AND NOT EXISTS(SELECT 1 FROM fgps_sync_commits_v1 WHERE id=?1)', [text(id)]);
        } catch { /* unreferenced chunks are never published */ }
      }
    }
  }

  async pull(commit: Commit, active: () => void) {
    const parts: Uint8Array<ArrayBuffer>[] = [];
    let len = 0;
    for (let i = 0; i < commit.parts; i++) {
      active();
      const value = (await this.execute('SELECT payload FROM fgps_sync_parts_v1 WHERE id=?1 AND part=?2', [text(commit.id), integer(i)])).rows[0]?.[0];
      if (value?.type !== 'blob') throw corrupt();
      const chunk = Uint8Array.fromBase64(value.base64);
      len += chunk.length;
      if (!chunk.length || chunk.length > chunkSize || len > maxImage) throw corrupt();
      parts.push(chunk);
    }
    const bytes = new Uint8Array(len);
    let offset = 0;
    parts.forEach(part => { bytes.set(part, offset); offset += part.length; });
    if (await sha256(bytes) !== commit.digest) throw corrupt();
    return bytes;
  }
}

export type SyncReplica = {
  active(scope: Scope): void;
  binding(scope: Scope): Promise<Binding | null>;
  update(scope: Scope, mutate: (state: Binding | null, populated: () => boolean) => Binding | null): Promise<void>;
  capture(scope: Scope): Promise<{ state: Binding; image: Uint8Array<ArrayBuffer> }>;
  apply(scope: Scope, base: number, remote: number, image: Uint8Array<ArrayBuffer>, discardVersion?: number): Promise<void>;
  storeToken(scope: Scope, token: string): Promise<void>;
  getToken(scope: Scope): Promise<string>;
  revokeToken(scope: Scope): Promise<void>;
};

export class Sync {
  private busy = false;
  private last?: { session: string; phase: SyncStatus_Serialize['phase']; error: SyncStatus_Serialize['error'] };
  constructor(private replica: SyncReplica, private emit: (scope: Scope, status: SyncStatus_Serialize) => void) {}

  async validate(input: TursoCredentials) {
    await new Turso(endpoint(input.url).href, input.authToken).probe();
  }

  async probeSaved(scope: Scope) {
    if (this.busy) throw issue('Busy', 'ledger_busy');
    this.busy = true;
    try {
      const state = await this.replica.binding(scope);
      if (!state) throw issue('Validation', 'turso_not_configured');
      const token = await this.replica.getToken(scope);
      this.replica.active(scope);
      await new Turso(state.url, token).probe();
    } finally { this.busy = false; }
  }

  async status(scope: Scope): Promise<SyncStatus_Serialize> {
    const binding = await this.replica.binding(scope);
    const last = binding && this.last?.session === scope.sessionId ? this.last : undefined;
    return {
      mode: binding ? 'turso-sync' : 'local-isolated', url: binding?.url ?? null,
      pending: !!binding && binding.local > binding.acknowledged,
      remoteGeneration: binding ? String(binding.base) : null,
      pendingCount: String(binding ? binding.local - binding.acknowledged : 0),
      localVersion: binding ? String(binding.local) : null,
      lastSuccessAt: binding?.last_success_at ?? null,
      phase: last?.phase ?? 'idle', error: last?.error ?? null
    };
  }

  async set(scope: Scope, mode: SetStorageMode) {
    if (this.busy) throw issue('Busy', 'ledger_busy');
    this.busy = true;
    try {
      const previous = await this.replica.binding(scope);
      if (mode.mode === 'local-isolated') {
        if (previous) {
          await this.replica.revokeToken(scope);
          await this.replica.update(scope, () => null);
        }
      } else {
        const url = endpoint(mode.url).href;
        if (previous && previous.url !== url) throw issue('Conflict', 'disable_sync_before_changing_endpoint');
        const remote = new Turso(url, mode.authToken);
        await remote.probe();
        this.replica.active(scope);
        await remote.initialize();
        this.replica.active(scope);
        const head = await remote.head();
        this.replica.active(scope);
        await this.replica.storeToken(scope, mode.authToken);
        if (!previous) {
          try {
            await this.replica.update(scope, (_, populated) => ({
              url, device: crypto.randomUUID(), local: Number(!head || populated()), acknowledged: 0, base: 0
            }));
          } catch (error) {
            await this.replica.revokeToken(scope).catch(() => {});
            throw error;
          }
        }
      }
      this.last = undefined;
      const status = await this.status(scope);
      this.emit(scope, status);
      return status;
    } finally { this.busy = false; }
  }

  async run(scope: Scope, resolution?: Resolution) {
    if (this.busy) throw issue('Busy', 'ledger_busy');
    this.busy = true;
    try {
      const local = await this.replica.binding(scope);
      if (!local) {
        if (resolution) throw issue('Conflict', 'turso_concurrent_ledger_edits');
        return this.status(scope);
      }
      const validVersion = (value: string) => /^[1-9]\d*$/.test(value) && Number.isSafeInteger(Number(value));
      if (resolution && (!validVersion(resolution.generation) || (resolution.discardVersion !== undefined && !validVersion(resolution.discardVersion)))) {
        throw issue('Validation', 'invalid_sync_generation');
      }
      this.last = { session: scope.sessionId, phase: 'syncing', error: null };
      this.emit(scope, await this.status(scope));
      try {
        const token = await this.replica.getToken(scope);
        this.replica.active(scope);
        await this.reconcile(scope, new Turso(local.url, token), local, resolution);
        this.last = { session: scope.sessionId, phase: 'idle', error: null };
      } catch (error) {
        const typed = asError(error);
        this.last = { session: scope.sessionId, phase: typed.code === 'Offline' ? 'offline' : 'error', error: typed };
      }
      const status = await this.status(scope);
      this.emit(scope, status);
      return status;
    } finally { this.busy = false; }
  }

  private async reconcile(scope: Scope, remote: Turso, state: Binding, resolution?: Resolution) {
    const active = () => this.replica.active(scope);
    const ack = async (generation: number, version: number) => {
      await this.replica.update(scope, local => {
        if (!local || generation <= local.base || version <= local.acknowledged || version > local.local) throw corrupt();
        return { ...local, base: generation, acknowledged: version, last_success_at: new Date().toISOString() };
      });
      state = (await this.replica.binding(scope))!;
    };
    active();
    const head = await remote.head();
    const found = head?.generation ?? 0;
    active();
    const receipt = head?.device === state.device ? head : head && state.local > state.acknowledged ? await remote.latest(state.device) : undefined;
    if (receipt && receipt.generation > state.base) await ack(receipt.generation, receipt.version);
    if (found < state.base) throw issue('Conflict', 'turso_remote_history_changed');
    const pending = state.local > state.acknowledged;
    const discardVersion = resolution?.discardVersion === undefined ? undefined : Number(resolution.discardVersion);
    if (resolution && (Number(resolution.generation) !== found || found <= state.base || !pending ||
      (discardVersion !== undefined && discardVersion !== state.local))) throw conflict(found);
    if (found > state.base && (!resolution || discardVersion !== undefined)) {
      if (pending && discardVersion === undefined) throw conflict(found);
      const image = await remote.pull(head!, active);
      if (discardVersion !== undefined) {
        active();
        const current = (await remote.head())?.generation ?? 0;
        if (current !== found) throw conflict(current);
      }
      await this.replica.apply(scope, state.base, found, image, discardVersion);
      return;
    }
    if (!pending) return;
    const captured = await this.replica.capture(scope);
    if (captured.state.base !== state.base || captured.state.local <= captured.state.acknowledged) throw conflict(found);
    if (!await remote.publish(found, captured.state, captured.image, active)) throw conflict(found);
    await ack(found + 1, captured.state.local);
  }
}
