import initEngine, { WebEngine, derive_key, initialize_ledger, verify_ledger, upgrade_ledger, ledger_schema_version } from './generated/web_engine';
import type * as C from '../contracts';
import type {
  AppPreferences_Serialize, CreateRequest_Deserialize, LedgerDescriptor, LedgerSession,
  Mutation_Deserialize, OpenRequest_Deserialize, OpenResult_Serialize, PathGrant, PathPurpose,
  Scope, SetStorageMode, TursoCredentials, UnlockRequest_Deserialize
} from '../contracts';
import { Opfs, sqlPort, type SqliteDb } from './opfs';
import { asError, issue } from './errors';
import { BrowserRecords, openRecords, WebSecretVault } from './secret-vault';
import { assemble, checkImage, copyRelations, disassemble, isPopulated, sha256, trusted, type LedgerImage } from './relational';
import { Sync, endpoint, stateKey, type Binding, type SyncReplica } from './sync';
import type { Request, Response } from './protocol';
import { Jobs, type JobObserver } from './jobs';
import { encodeBackup, decodeBackup } from './backup';

const send = (value: Response, transfers: Transferable[] = []) => self.postMessage(value, { transfer: transfers });
const now = () => new Date().toISOString();
const stale = () => issue('StaleSession', 'stale_ledger_session');
const unauthorized = () => issue('BadCredentialOrCorrupt', 'bad_credential_or_corrupt');
const text = new TextEncoder();
const decodeText = new TextDecoder();
const noFile = () => issue('Validation', 'invalid_input', { grant: 'invalid_or_expired_path_grant' });

type Profile = LedgerDescriptor & { salt?: number[]; uuid: string; storageId?: string };
type Active = { entry: Profile; db: SqliteDb; scope: Scope; key?: CryptoKey };
type Grant = { item: PathGrant; file?: File; expires: number };
type RequestArgs = Record<string, unknown>;

class Queue {
  private tail: Promise<unknown> = Promise.resolve();
  run<T>(work: () => T | Promise<T>): Promise<T> {
    const result = this.tail.then(work);
    this.tail = result.catch(() => {});
    return result;
  }
}

function matching(left?: Scope, right?: Scope) {
  return left?.sessionId === right?.sessionId && left?.ledgerInstanceId === right?.ledgerInstanceId;
}

function secretSlot(entry: Profile, name: string) { return `${name}:${entry.ledgerInstanceId}`; }
const publicProfile = ({ uuid: _uuid, salt: _salt, storageId: _storage, ...entry }: Profile): LedgerDescriptor => entry;

function derive(credential: string, salt: Uint8Array<ArrayBuffer>) {
  if (credential.length < 4 || text.encode(credential).length > 1024) throw unauthorized();
  return new Uint8Array(derive_key(credential, salt));
}

function importKey(bytes: Uint8Array<ArrayBuffer>) {
  return crypto.subtle.importKey('raw', bytes, 'AES-GCM', false, ['encrypt', 'decrypt']);
}

class WorkerRuntime implements SyncReplica {
  private readonly queue = new Queue();
  private readonly engine = new WebEngine();
  private readonly vault: WebSecretVault;
  private readonly sync: Sync;
  private readonly jobs = new Jobs(
    value => send({ type: 'event', name: 'ledgerJobProgress', value }),
    (id, scope, control) => send({ type: 'job-control', id, scope, control })
  );
  private readonly grants = new Map<string, Grant>();
  private readonly requests = new Map<string, { reference: string; result: OpenResult_Serialize }>();
  private profiles: Profile[] = [];
  private readonly epoch = crypto.randomUUID();
  private sequence = 0;
  private prefs!: AppPreferences_Serialize;
  private selected?: Active;
  private locked?: { entry: Profile; id: string; expires: number; archive?: File };

  constructor(private readonly opfs: Opfs, private readonly records: BrowserRecords) {
    this.vault = new WebSecretVault(records);
    this.sync = new Sync(this, (scope, status) =>
      send({ type: 'event', name: 'ledgerSyncStatus', value: [scope, status] }));
  }

  async initialize() {
    this.profiles = await this.records.get<Profile[]>('ledger.registry') ?? [];
    const stored = await this.records.get<AppPreferences_Serialize>('app.preferences');
    if (stored) this.engine.set_preferences(JSON.stringify(stored));
    this.prefs = JSON.parse(this.engine.preferences());
    setInterval(() => {
      const scope = this.selected?.scope;
      if (scope) {
        void this.sync.run(scope).catch(() => {});
        void this.queue.run(async () => {
          if (matching(scope, this.selected?.scope)) await this.runSchedule(this.require(scope));
          if (!this.prefs.hiddenFeatures.includes('recurring') && matching(scope, this.selected?.scope)) {
            await this.executeCore('recurring_generate_due', { scope }, true);
          }
        }).catch(() => {});
      }
    }, 15_000);
  }

  grant(purpose: PathPurpose, name: string, file?: File): PathGrant {
    const time = Date.now();
    for (const [id, value] of this.grants) if (value.expires < time) this.grants.delete(id);
    const expires = time + 300_000;
    const item = { id: crypto.randomUUID(), purpose, displayName: name, expiresAt: new Date(expires).toISOString() };
    if (this.grants.size >= 256) throw issue('Busy', 'ledger_busy');
    this.grants.set(item.id, { item, file, expires });
    return item;
  }

  private consume(purpose: PathPurpose, id: string) {
    const grant = this.grants.get(id);
    if (!grant || grant.item.purpose !== purpose || grant.expires <= Date.now()) throw noFile();
    this.grants.delete(id);
    return grant;
  }

  private async saveProfiles(profiles: Profile[]) {
    await this.records.set('ledger.registry', profiles);
    this.profiles = profiles;
  }

  private require(scope?: Scope): Active {
    if (!this.selected) throw issue('Locked', 'ledger_locked');
    if (!scope || !matching(scope, this.selected.scope)) throw stale();
    return this.selected;
  }

  active(scope: Scope) { this.require(scope); }

  private info(active: Active): LedgerSession {
    const [uuid, name, baseCurrency, revision] =
      active.db.selectArray('SELECT ledger_uuid,name,display_currency,CAST(revision AS TEXT) FROM ledger_meta WHERE singleton=1') ?? [];
    return {
      scope: active.scope, ledgerUuid: String(uuid), name: String(name), baseCurrency: String(baseCurrency),
      revision: String(revision), protection: active.entry.protection
    };
  }

  private async ready(entry: Profile, db: SqliteDb, key?: CryptoKey) {
    try {
      const port = sqlPort(this.opfs.sqlite, db);
      if (upgrade_ledger(port, Date.now()) && key) await this.opfs.saveProtected(this.storageId(entry), db, key);
      verify_ledger(port);
      this.locked = undefined;
      this.selected = { entry, db, key, scope: { ledgerInstanceId: entry.ledgerInstanceId, sessionId: `${this.epoch}:${++this.sequence}` } };
      return this.info(this.selected);
    } catch (error) {
      db.close();
      throw error;
    }
  }

  private closeActive() {
    const active = this.selected;
    if (!active) return;
    this.engine.invalidate(JSON.stringify(active.scope));
    active.db.close();
    this.selected = undefined;
    this.requests.clear();
  }

  private async create(input: CreateRequest_Deserialize, image?: LedgerImage): Promise<LedgerSession> {
    const name = input.name.trim();
    if (!name || text.encode(name).length > 256 || /\p{Cc}/u.test(name)) {
      throw issue('Validation', 'invalid_input', { name: 'invalid_ledger_name' });
    }
    if (input.timezone) {
      try { new Intl.DateTimeFormat('en', { timeZone: input.timezone }); }
      catch { throw issue('Validation', 'invalid_input', { timezone: 'invalid_iana_timezone' }); }
    }
    const protectedLedger = input.protection === 'encrypted';
    if (protectedLedger !== !!input.credential || (!protectedLedger && input.rememberKey)) {
      throw issue('Validation', 'invalid_input', { credential: 'invalid_credential_options' });
    }
    const grant = input.destination ? this.consume('ledgerCreate', input.destination) : undefined;
    const id = crypto.randomUUID();
    const salt = protectedLedger ? crypto.getRandomValues(new Uint8Array(16)) : undefined;
    const entry: Profile = {
      ledgerInstanceId: id, uuid: crypto.randomUUID(), label: name, protection: input.protection,
      storagePolicy: 'managed', quickUnlock: input.rememberKey, salt: salt ? Array.from(salt) : undefined
    };
    const raw = salt ? await derive(input.credential!, salt) : undefined;
    const key = raw && await importKey(raw);
    const db = protectedLedger ? this.opfs.memory() : await this.opfs.create(id);
    try {
      db.exec('BEGIN IMMEDIATE');
      initialize_ledger(sqlPort(this.opfs.sqlite, db), JSON.stringify({
        uuid: entry.uuid, name, timezone: input.timezone, currency: input.baseCurrency, createdAtMs: Date.now()
      }));
      db.exec('COMMIT');
      upgrade_ledger(sqlPort(this.opfs.sqlite, db), Date.now());
      if (image) {
        const source = this.opfs.fromBytes(image.database);
        try {
          copyRelations(this.opfs, source, db, {
            rebuildSearch: true, newUuid: entry.uuid,
            after: db => { db.exec({ sql: 'UPDATE ledger_meta SET name=?1', bind: [name] }); }
          });
          for (const [id, file] of image.files) await this.storeAttachment(entry, key, id, file);
        } finally { source.close(); }
      }
      if (key) await this.opfs.saveProtected(id, db, key);
      if (entry.quickUnlock && raw) await this.vault.store(secretSlot(entry, 'ledger'), raw);
      let output: Uint8Array<ArrayBuffer> | undefined;
      if (grant) {
        if (protectedLedger) {
          const fullImage = await this.snapshot({ entry, db, key, scope: { ledgerInstanceId: id, sessionId: '' } }, true);
          output = (await encodeBackup(this.opfs, fullImage, input.credential!)).archive;
        } else output = this.opfs.image(db);
      }
      await this.saveProfiles([...this.profiles, entry]);
      this.closeActive();
      const session = await this.ready(entry, db, key);
      if (grant && output) this.output(grant, output, protectedLedger ? 'application/octet-stream' : 'application/vnd.sqlite3');
      return session;
    } catch (error) {
      db.close();
      await this.vault.revoke(secretSlot(entry, 'ledger')).catch(() => {});
      if (protectedLedger) await this.opfs.remove(`${id}.protected`).catch(() => {});
      else this.opfs.removePlain(id);
      throw error;
    } finally { raw?.fill(0); }
  }

  private async open(input: OpenRequest_Deserialize): Promise<OpenResult_Serialize> {
    const reference = String(input.ledgerInstanceId ?? input.openGrant);
    const receipt = this.requests.get(input.requestId);
    if (receipt) {
      if (receipt.reference !== reference) throw issue('Conflict', 'request_id_reused');
      return receipt.result;
    }
    let entry: Profile | undefined;
    if (input.openGrant && !input.ledgerInstanceId) {
      const file = this.consume('ledgerOpen', input.openGrant).file;
      if (!file || file.size > 400 * 1024 * 1024) throw noFile();
      if (decodeText.decode(await file.slice(0, 8).arrayBuffer()) === 'FINGPSBK') {
        this.closeActive();
        entry = {
          ledgerInstanceId: crypto.randomUUID(), uuid: crypto.randomUUID(), label: file.name,
          protection: 'encrypted', storagePolicy: 'portable', quickUnlock: false
        };
        const challenge = { id: crypto.randomUUID(), entry, archive: file, expires: Date.now() + 300_000 };
        this.locked = challenge;
        const result: OpenResult_Serialize = { state: 'locked', session: null, unlockChallenge: { id: challenge.id, ledger: publicProfile(entry) } };
        this.requests.set(input.requestId, { reference, result });
        return result;
      }
      if (file.size > 256 * 1024 * 1024) throw noFile();
      const bytes = new Uint8Array(await file.arrayBuffer());
      const source = this.opfs.fromBytes(bytes);
      try {
        const image = { database: bytes, files: new Map<string, Uint8Array<ArrayBuffer>>() };
        await checkImage(this.opfs, image);
        const [name, currency] = source.selectArray('SELECT name,display_currency FROM ledger_meta') ?? [];
        const session = await this.create({ name: String(name), baseCurrency: String(currency), protection: 'plain', rememberKey: false }, image);
        return { state: 'ready', session, unlockChallenge: null };
      } finally { source.close(); }
    }
    entry = this.profiles.find(item => item.ledgerInstanceId === input.ledgerInstanceId);
    if (!entry) throw issue('Validation', 'invalid_input', { ledgerInstanceId: 'ledger_not_registered' });
    this.closeActive();
    let result: OpenResult_Serialize;
    if (entry.protection === 'encrypted') {
      const challenge = { id: crypto.randomUUID(), expires: Date.now() + 300_000, entry };
      this.locked = challenge;
      result = { state: 'locked', session: null, unlockChallenge: { id: challenge.id, ledger: publicProfile(entry) } };
    } else {
      result = { state: 'ready', session: await this.ready(entry, this.opfs.openPlain(this.storageId(entry))), unlockChallenge: null };
    }
    this.requests.set(input.requestId, { reference, result });
    return result;
  }

  private async unlock(input: UnlockRequest_Deserialize) {
    const challenge = this.locked;
    if (!challenge || challenge.id !== input.unlockChallenge || challenge.expires < Date.now()) throw stale();
    const entry = challenge.entry;
    if (challenge.archive) {
      if (!input.credential) throw unauthorized();
      const { image } = await decodeBackup(this.opfs, new Uint8Array(await challenge.archive.arrayBuffer()), input.credential);
      const source = this.opfs.fromBytes(image.database);
      try {
        const [name, baseCurrency, timezone] = source.selectArray('SELECT name,display_currency,timezone_override FROM ledger_meta')!;
        return await this.create({
          name: String(name), baseCurrency: String(baseCurrency), protection: 'encrypted', credential: input.credential,
          timezone: timezone == null ? null : String(timezone), rememberKey: false
        }, image);
      } finally { source.close(); }
    }
    const raw = input.credential
      ? await derive(input.credential, Uint8Array.from(entry.salt!))
      : await this.vault.retrieve(secretSlot(entry, 'ledger'));
    if (!raw) throw unauthorized();
    try {
      const key = await importKey(raw);
      return this.ready(entry, await this.opfs.openProtected(this.storageId(entry), key), key);
    } finally { raw.fill(0); }
  }

  private async persist(active: Active) {
    if (!active.key) return;
    try {
      await this.opfs.saveProtected(this.storageId(active.entry), active.db, active.key);
    } catch (error) {
      active.db.close();
      try {
        active.db = await this.opfs.openProtected(this.storageId(active.entry), active.key);
      } catch {
        this.selected = undefined;
        send({ type: 'event', name: 'ledgerIntegrityAlert', value: [active.scope, issue('Integrity', 'ledger_integrity_check_failed')] });
      }
      throw error;
    }
  }

  private async executeCore(command: string, args: RequestArgs, write = Boolean(args.mutation)) {
    const scope = (args.mutation as Mutation_Deserialize | undefined)?.scope ?? args.scope as Scope;
    const active = this.require(scope);
    const port = sqlPort(this.opfs.sqlite, active.db);
    const data: unknown = JSON.parse(this.engine.execute(port, command, JSON.stringify(args)));
    if (write) await this.persist(active);
    return data;
  }

  private output(grant: Grant, bytes: Uint8Array<ArrayBuffer>, media: string, open = false) {
    send({ type: 'file', grant: grant.item.id, name: grant.item.displayName, bytes, media, open }, [bytes.buffer]);
  }

  async invoke(command: string, args: RequestArgs): Promise<unknown> {
    if (command.startsWith('import_') || command.startsWith('fx_')) throw issue('Conflict', 'feature_unavailable');
    const scope = args.scope as Scope;
    switch (command) {
      case 'job_status': {
        if (scope) this.require(scope);
        return this.jobs.get(String(args.id), scope).status;
      }
      case 'job_cancel': {
        if (scope) this.require(scope);
        return this.jobs.cancel(String(args.id), scope);
      }
      case 'turso_credentials_validate': {
        const credentials = args.input as TursoCredentials;
        endpoint(credentials.url);
        await this.sync.validate(credentials);
        return null;
      }
      case 'ledger_storage_mode_set': {
        const input = args.input as SetStorageMode;
        if (input.mode === 'turso-sync') endpoint(input.url);
        return this.sync.set(scope, input);
      }
      case 'ledger_sync_probe_saved': await this.sync.probeSaved(scope); return null;
      case 'ledger_sync_status': return this.sync.status(scope);
      case 'ledger_sync_now': return this.sync.run(scope);
      case 'ledger_sync_publish_local': return this.sync.run(scope, { generation: args.expectedRemoteGeneration as string });
      case 'ledger_sync_accept_remote': {
        const input = args.input as { expectedRemoteGeneration: string; expectedLocalVersion: string };
        return this.sync.run(scope, { generation: input.expectedRemoteGeneration, discardVersion: input.expectedLocalVersion });
      }
      default: return this.queue.run(() => this.local(command, args));
    }
  }

  private async local(command: string, args: RequestArgs) {
    switch (command) {
      case 'attachments_list': case 'attachment_rename': case 'attachment_delete':
      case 'attachment_save_as': case 'attachment_open': return this.attachments(command, args);
      case 'import_preview_start': case 'import_commit_start': case 'rule_preview_start': case 'rule_apply_start':
      case 'transactions_export_start': case 'fx_refresh_start': case 'attachment_add_start':
      case 'backup_create_start': case 'restore_preview_start': case 'restore_apply_start':
      case 'ledger_change_protection': return this.start(command, args);
      case 'import_template_file':
      case 'rule_pack_transfer': {
        const input = args.input as C.TemplateFile | C.RulePackTransfer_Deserialize;
        const reading = input.direction === 'import';
        const purpose = command === 'import_template_file' ? reading ? 'templateRead' : 'templateWrite' : reading ? 'rulePackRead' : 'rulePackWrite';
        const grant = this.consume(purpose, input.grant);
        const limit = command === 'import_template_file' ? 128 * 1024 : 1024 * 1024;
        if (grant.file && grant.file.size > limit) throw issue('Validation', 'file_size_limit');
        return this.coreFile(command, args, grant, 'application/octet-stream', reading);
      }
      case 'backup_list': return this.listBackups(this.require(args.scope as Scope), args.input as C.PageQuery);
      case 'backup_remove': {
        const mutation = args.mutation as Mutation_Deserialize;
        return this.removeBackup(this.require(mutation.scope), mutation, args.input as C.ConfirmedId_Deserialize);
      }
      case 'backup_schedule_get': return this.getSchedule(this.require(args.scope as Scope));
      case 'backup_schedule_set': {
        const mutation = args.mutation as Mutation_Deserialize;
        return this.setSchedule(this.require(mutation.scope), mutation, args.input as C.BackupScheduleSet_Deserialize);
      }
      case 'restore_confirm_replacement': return this.confirmRestore(args.scope as Scope, args.input as C.RestoreConfirmationQuery);
      case 'ledger_keychain_set': {
        const mutation = args.mutation as Mutation_Deserialize;
        return this.quickUnlock(this.require(mutation.scope), mutation, args.input as C.KeychainChange_Deserialize);
      }
      case 'app_bootstrap':
        return { session: this.selected ? this.info(this.selected) : null, prefs: this.prefs, schemaRange: { minimum: 1, maximum: ledger_schema_version() } };
      case 'ledger_list': return this.profiles.map(publicProfile);
      case 'ledger_create': return this.create(args.input as CreateRequest_Deserialize);
      case 'ledger_open':
      case 'ledger_switch': return this.open(args.input as OpenRequest_Deserialize);
      case 'ledger_unlock': return this.unlock(args.input as UnlockRequest_Deserialize);
      case 'ledger_close':
      case 'ledger_lock': {
        const active = this.require(args.scope as Scope);
        this.closeActive();
        if (command === 'ledger_lock' && active.entry.protection === 'encrypted') {
          this.locked = { id: crypto.randomUUID(), entry: active.entry, expires: Date.now() + 300_000 };
          return 'locked';
        }
        this.locked = undefined;
        return 'unselected';
      }
      case 'ledger_integrity_status': {
        this.require(args.scope as Scope);
        return true;
      }
      case 'app_preferences_get': return this.prefs;
      case 'app_preferences_set': {
        const next = this.engine.patch_preferences(JSON.stringify(args.patch));
        const prefs: AppPreferences_Serialize = JSON.parse(next);
        await this.records.set('app.preferences', prefs);
        this.engine.set_preferences(next);
        this.prefs = prefs;
        return prefs;
      }
      default: return this.executeCore(command, args, Boolean(args.mutation) || command === 'fx_provider_configure');
    }
  }

  private async coreFile(command: string, args: RequestArgs, grant: Grant, media: string, write: boolean, observer?: JobObserver) {
    const scope = (args.mutation as Mutation_Deserialize | undefined)?.scope ?? args.scope as Scope;
    const active = this.require(scope);
    if (grant.file && grant.file.size > 50 * 1024 * 1024) throw issue('Validation', 'statement_size_limit');
    const bytes = grant.file ? new Uint8Array(await grant.file.arrayBuffer()) : new Uint8Array();
    const data = JSON.parse(this.engine.execute_file(sqlPort(this.opfs.sqlite, active.db), command, JSON.stringify(args), grant.item.displayName, bytes, observer));
    if (write) await this.persist(active);
    if (data.bytes) {
      this.output(grant, Uint8Array.from(data.bytes), media);
      if (command !== 'transactions_export_start') return { exported: data.result };
      return data.result;
    }
    return data;
  }

  private async versioned<T>(active: Active, command: string, mutation: Mutation_Deserialize, input: unknown, affected: string[], work: () => T) {
    if (!/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i.test(mutation.requestId)) throw issue('Validation', 'invalid_request_id');
    const digest = await sha256(text.encode(JSON.stringify(input)));
    const before = active.db.selectArray('SELECT command,input_hash,result_json FROM operation_receipts WHERE request_id=?1', [mutation.requestId]);
    if (before) {
      if (before[0] !== command || before[1] !== digest) throw issue('Conflict', 'request_id_reused');
      return JSON.parse(String(before[2])) as { value: T; revision: string; affected: string[] };
    }
    const previous = this.info(active).revision;
    if (mutation.expectedRevision != null && mutation.expectedRevision !== previous) throw issue('StaleRevision', 'stale_ledger_revision');
    active.db.exec('BEGIN IMMEDIATE');
    let result;
    try {
      const value = work();
      active.db.exec("UPDATE ledger_meta SET revision=revision+1 WHERE singleton=1; UPDATE ledger_settings SET value_json=json_set(value_json,'$.local',json_extract(value_json,'$.local')+1) WHERE key='secret.sync.v1'");
      result = { value, revision: this.info(active).revision, affected };
      active.db.exec({
        sql: 'INSERT INTO operation_receipts VALUES(?1,?2,?3,?4,?5)',
        bind: [mutation.requestId, command, digest, JSON.stringify(result), Date.now()]
      });
      active.db.exec('COMMIT');
    } catch (error) {
      active.db.exec('ROLLBACK');
      throw error;
    }
    await this.persist(active);
    return result;
  }

  private attachmentRows(active: Active, match: 'id' | 'transaction_id', id: string): C.AttachmentSummary[] {
    const query = `SELECT id,transaction_id,display_name,media_type,CAST(size_bytes AS TEXT),sha256,state,created_at_ms FROM attachments WHERE ${match}=?1 ORDER BY created_at_ms,id`;
    return active.db.selectArrays(query, [id]).map(([id, transactionId, displayName, mediaType, size, digest, state, created]) => ({
      id: String(id), transactionId: String(transactionId), displayName: String(displayName), mediaType: String(mediaType),
      size: String(size), digest: String(digest), state: state as C.AttachmentState, createdAt: new Date(Number(created)).toISOString()
    }));
  }

  private attachment(active: Active, id: string) {
    const value = this.attachmentRows(active, 'id', id)[0];
    if (!value) throw issue('Validation', 'missing_resource', { attachmentId: 'not_found' });
    return value;
  }

  private async addAttachments(active: Active, mutation: Mutation_Deserialize, input: C.AttachmentAdd, publish: () => void) {
    if (!active.db.selectValue('SELECT 1 FROM transactions WHERE id=?1', [input.transactionId])) {
      throw issue('Validation', 'missing_resource', { transactionId: 'not_found' });
    }
    const previous = active.db.selectArray('SELECT result_json FROM operation_receipts WHERE request_id=?1', [mutation.requestId]);
    if (previous) return this.versioned(active, 'attachment_add_start', mutation, input, ['attachments'], () => { throw issue('Integrity', 'invalid_operation_receipt'); });
    if (!input.grants.length || new Set(input.grants).size !== input.grants.length || input.grants.length + this.attachmentRows(active, 'transaction_id', input.transactionId).length > 10) {
      throw issue('Validation', 'attachment_count_limit');
    }
    const staged: { id: string; file: File; bytes: Uint8Array<ArrayBuffer>; mime: string; digest: string }[] = [];
    for (const id of input.grants) {
      const file = this.consume('attachmentRead', id).file;
      if (!file || file.size === 0 || file.size > 10_000_000) throw issue('Validation', 'attachment_size_limit');
      const bytes = new Uint8Array(await file.arrayBuffer());
      const mime = attachmentType(file.name, bytes);
      staged.push({ id: crypto.randomUUID(), file, bytes, mime, digest: await sha256(bytes) });
    }
    publish();
    try {
      for (const item of staged) await this.storeAttachment(active.entry, active.key, item.id, item.bytes);
      return await this.versioned(active, 'attachment_add_start', mutation, input, ['attachments'], () => {
        for (const item of staged) {
          active.db.exec({
            sql: "INSERT INTO attachments VALUES(?1,?2,?1,?3,?4,?5,?6,'ready',?7)",
            bind: [item.id, input.transactionId, item.file.name, item.mime, item.bytes.byteLength, item.digest, Date.now()]
          });
        }
        return { attachments: staged.map(item => this.attachment(active, item.id)) };
      });
    } catch (error) {
      await Promise.all(staged.map(item => this.opfs.remove(this.blobName(active.entry, item.id)).catch(() => {})));
      throw error;
    }
  }

  private async attachments(command: string, args: RequestArgs) {
    const input = args.input as Record<string, string>;
    const mutation = args.mutation as Mutation_Deserialize | undefined;
    const active = this.require(mutation?.scope ?? args.scope as Scope);
    if (command === 'attachments_list') {
      if (!active.db.selectValue('SELECT 1 FROM transactions WHERE id=?1', [input.transactionId])) {
        throw issue('Validation', 'missing_resource', { transactionId: 'not_found' });
      }
      return this.attachmentRows(active, 'transaction_id', input.transactionId);
    }
    if (command === 'attachment_rename') return this.versioned(active, command, mutation!, input, ['attachments'], () => {
      const before = this.attachment(active, input.id);
      const displayName = input.displayName.trim();
      if (!displayName || text.encode(displayName).length > 256 || /[/\\\p{Cc}]/u.test(displayName) || displayName.split('.').pop() !== before.displayName.split('.').pop()) {
        throw issue('Validation', 'invalid_attachment_name_or_extension');
      }
      active.db.exec({ sql: 'UPDATE attachments SET display_name=?2 WHERE id=?1', bind: [input.id, displayName] });
      return this.attachment(active, input.id);
    });
    if (command === 'attachment_delete') {
      const result = await this.versioned(active, command, mutation!, input, ['attachments'], () => {
        this.attachment(active, input.id);
        active.db.exec({ sql: 'DELETE FROM attachments WHERE id=?1', bind: [input.id] });
        return { id: input.id };
      });
      await this.opfs.remove(this.blobName(active.entry, input.id)).catch(() => {});
      return result;
    }
    const before = this.attachment(active, input.id);
    const bytes = await this.readAttachment(active.entry, active.key, input.id);
    if (before.digest !== await sha256(bytes)) throw issue('Integrity', 'attachment_hash_mismatch');
    if (command === 'attachment_save_as') {
      const grant = this.consume('attachmentWrite', input.saveGrant);
      this.output(grant, bytes, before.mediaType);
      return { fileName: grant.item.displayName, rowCount: '1', revision: this.info(active).revision };
    }
    const grant = { item: { id: '', displayName: before.displayName, purpose: 'attachmentWrite' as const, expiresAt: now() }, expires: Date.now() };
    this.output(grant, bytes, before.mediaType, true);
    return { id: input.id, opened: true };
  }

  private start(command: string, args: RequestArgs) {
    const mutation = args.mutation as Mutation_Deserialize | undefined;
    const scope = mutation?.scope ?? args.scope as Scope | undefined;
    if (scope) this.require(scope);
    const input = args.input as Record<string, unknown>;
    const kinds = {
      import_preview_start: 'importPreview', import_commit_start: 'importCommit', rule_preview_start: 'rulePreview',
      rule_apply_start: 'ruleApply', transactions_export_start: 'transactionsExport', fx_refresh_start: 'fxRefresh',
      attachment_add_start: 'attachmentAdd', backup_create_start: 'backupCreate', restore_preview_start: 'restorePreview',
      restore_apply_start: 'restoreApply', ledger_change_protection: 'ledgerChangeProtection'
    } as const;
    const kind = kinds[command as keyof typeof kinds];
    const ticketScope = ['restorePreview', 'restoreApply', 'ledgerChangeProtection'].includes(kind) ? undefined : scope;
    return this.jobs.start(ticketScope, kind, kind, async (_signal, observer) => {
      const publish = () => { if (!observer.publishing()) throw issue('Cancelled', 'operation_cancelled'); };
      return this.queue.run(async () => {
        if (observer.cancelled()) throw issue('Cancelled', 'operation_cancelled');
        switch (command) {
          case 'import_preview_start': return this.coreFile(command, args, this.consume('statementImport', String(input.fileGrant)), '', false, observer);
          case 'transactions_export_start': {
            const target = this.consume('transactionExport', String(input.saveGrant));
            return this.coreFile(command, args, target, 'text/csv', false, observer);
          }
          case 'attachment_add_start': return this.addAttachments(this.require(scope), mutation!, input as unknown as C.AttachmentAdd, publish);
          case 'backup_create_start': {
            const active = this.require(scope);
            const request = input as unknown as C.BackupCreate_Deserialize;
            const grant = request.destination ? this.consume('backupWrite', request.destination) : undefined;
            publish();
            return this.createBackup(active, request.backupPassphrase, grant);
          }
          case 'restore_preview_start': return this.previewRestore(this.selected?.scope, input as unknown as C.RestorePreviewRequest_Deserialize);
          case 'restore_apply_start': {
            publish();
            return this.applyRestore(this.selected?.scope, input as unknown as C.RestoreApply_Deserialize);
          }
          case 'ledger_change_protection': {
            publish();
            return this.changeProtection(this.require(scope), mutation!, input as unknown as C.ProtectionChange_Deserialize);
          }
          default: {
            const active = this.require(scope);
            const data: unknown = JSON.parse(this.engine.execute_file(sqlPort(this.opfs.sqlite, active.db), command, JSON.stringify(args), '', new Uint8Array(), observer));
            if (mutation) await this.persist(active);
            return data;
          }
        }
      });
    });
  }

  private readonly restoreStages = new Map<string, {
    scope?: Scope; image: LedgerImage; manifest: C.BackupManifest; expires: number;
    confirm?: { id: string; target: string; revision: string };
  }>();

  private async backups(entry: Profile) {
    return await this.records.get<C.BackupSummary_Serialize[]>(`backups:${entry.ledgerInstanceId}`) ?? [];
  }

  private backupName(entry: Profile, id: string) {
    if (!/^\d{13}-[0-9a-f-]{36}$/.test(id)) throw issue('Validation', 'invalid_backup_id');
    return `${entry.ledgerInstanceId}-${id}.fgpsbackup`;
  }

  private async createBackup(active: Active, password: string, target?: Grant) {
    const image = await this.snapshot(active, true);
    const { archive, manifest } = await encodeBackup(this.opfs, image, password);
    const id = `${Date.now()}-${crypto.randomUUID()}`;
    const summary = {
      id, fileName: target?.item.displayName ?? `${id}.fgpsbackup`, createdAt: now(), bytes: String(archive.byteLength),
      sourceRevision: manifest.sourceRevision, managed: !target
    };
    if (target) {
      this.output(target, archive, 'application/octet-stream');
      return summary;
    }
    const name = this.backupName(active.entry, id);
    const list = [summary, ...await this.backups(active.entry)];
    await this.opfs.write(name, [archive]);
    try { await this.records.set(`backups:${active.entry.ledgerInstanceId}`, list.slice(0, 10)); }
    catch (error) { await this.opfs.remove(name).catch(() => {}); throw error; }
    for (const old of list.slice(10)) await this.opfs.remove(this.backupName(active.entry, old.id)).catch(() => {});
    return summary;
  }

  private async listBackups(active: Active, input: C.PageQuery) {
    const page = input.page;
    const limit = page.limit;
    if (limit < 1 || limit > 200) throw issue('Validation', 'invalid_page_size');
    const current = this.info(active).revision;
    const list = await this.backups(active.entry);
    const offset = page.cursor ? list.findIndex(item => item.id === page.cursor) + 1 : 0;
    if (page.cursor && offset === 0) throw issue('StaleRevision', 'stale_cursor');
    const rows = list.slice(offset, offset + limit);
    return { rows, revision: current, nextCursor: offset + limit < list.length ? rows.at(-1)?.id : undefined };
  }

  private async removeBackup(active: Active, mutation: Mutation_Deserialize, input: C.ConfirmedId_Deserialize) {
    if (!input.confirm) throw issue('Validation', 'confirmation_required');
    const list = await this.backups(active.entry);
    const next = list.filter(item => item.id !== input.id);
    if (next.length === list.length) throw issue('Validation', 'missing_resource', { backupId: 'not_found' });
    if (mutation.expectedRevision != null && mutation.expectedRevision !== this.info(active).revision) throw issue('StaleRevision', 'stale_ledger_revision');
    await this.records.set(`backups:${active.entry.ledgerInstanceId}`, next);
    await this.opfs.remove(this.backupName(active.entry, input.id)).catch(() => {});
    return this.versioned(active, 'backup_remove', mutation, input, ['backups'], () => ({ id: input.id }));
  }

  private restoreStage(id: string, scope?: Scope) {
    const stage = this.restoreStages.get(id);
    if (!stage || stage.expires <= Date.now() || (stage.scope && !matching(stage.scope, scope))) throw issue('StaleSession', 'restore_preview_expired');
    return stage;
  }

  private async previewRestore(scope: Scope | undefined, input: C.RestorePreviewRequest_Deserialize) {
    if (Boolean(input.fileGrant) === Boolean(input.scopedBackupId)) throw issue('Validation', 'backup_source_required');
    const file = input.fileGrant
      ? this.consume('restoreRead', input.fileGrant).file
      : await this.opfs.read(this.backupName(this.require(scope).entry, input.scopedBackupId!));
    if (!file || file.size > 400 * 1024 * 1024) throw issue('Validation', 'archive_size_limit');
    const { manifest, image } = await decodeBackup(this.opfs, new Uint8Array(await file.arrayBuffer()), input.backupPassphrase);
    const previewToken = crypto.randomUUID();
    this.restoreStages.clear();
    this.restoreStages.set(previewToken, { scope: input.scopedBackupId ? scope : undefined, manifest, image, expires: Date.now() + 300_000 });
    return { manifest, previewToken, warnings: ['newLedgerIdentity', 'localCredentialsExcluded'] };
  }

  private confirmRestore(scope: Scope, input: C.RestoreConfirmationQuery) {
    const active = this.require(scope);
    const stage = this.restoreStage(input.previewToken, scope);
    if (input.targetId !== active.entry.ledgerInstanceId) throw stale();
    const id = crypto.randomUUID();
    stage.confirm = { id, target: input.targetId, revision: this.info(active).revision };
    return { token: id, revision: stage.confirm.revision };
  }

  private async applyRestore(scope: Scope | undefined, input: C.RestoreApply_Deserialize) {
    const stage = this.restoreStage(input.previewToken, scope);
    const source = this.opfs.fromBytes(stage.image.database);
    try {
      const [baseCurrency, timezone] = source.selectArray('SELECT display_currency,timezone_override FROM ledger_meta')!;
      if (input.target.kind === 'new') {
        const session = await this.create({
          name: input.target.name, destination: input.target.destination, baseCurrency: String(baseCurrency),
          timezone: timezone == null ? null : String(timezone), protection: input.protection,
          credential: input.outputCredential, rememberKey: false
        }, stage.image);
        this.restoreStages.delete(input.previewToken);
        return session;
      }
      const active = this.require(scope);
      if (!stage.confirm || stage.confirm.id !== input.confirm || stage.confirm.target !== input.target.targetId ||
        stage.confirm.target !== active.entry.ledgerInstanceId || stage.confirm.revision !== this.info(active).revision) {
        throw issue('StaleRevision', 'stale_restore_confirmation');
      }
      if (!input.safetyBackupPassphrase) throw issue('Validation', 'credential_required');
      await this.createBackup(active, input.safetyBackupPassphrase);
      const result = await this.replace(active, stage.image, input.protection, input.outputCredential ?? undefined);
      this.restoreStages.delete(input.previewToken);
      return result;
    } finally { source.close(); }
  }

  private async replace(active: Active, image: LedgerImage, protection: C.Protection, credential?: string) {
    const encrypted = protection === 'encrypted';
    if (encrypted !== Boolean(credential)) throw issue('Validation', 'invalid_credential_options');
    const salt = encrypted ? crypto.getRandomValues(new Uint8Array(16)) : undefined;
    const raw = salt ? derive(credential!, salt) : undefined;
    const key = raw && await importKey(raw);
    raw?.fill(0);
    const entry: Profile = {
      ...active.entry, storageId: crypto.randomUUID(), protection, salt: salt ? Array.from(salt) : undefined, quickUnlock: false
    };
    const id = this.storageId(entry);
    const db = encrypted ? this.opfs.memory() : await this.opfs.create(id);
    const source = this.opfs.fromBytes(image.database);
    const originalFiles = active.db.selectValues('SELECT id FROM attachments').map(String);
    const local = active.db.selectArrays("SELECT key,value_json FROM ledger_settings WHERE key LIKE 'secret.%' OR key LIKE 'backup.%' OR key LIKE 'fx.%enabled'");
    const revision = this.info(active).revision;
    try {
      const [name, currency, timezone] = source.selectArray('SELECT name,display_currency,timezone_override FROM ledger_meta')!;
      entry.label = String(name);
      db.exec('BEGIN IMMEDIATE');
      initialize_ledger(sqlPort(this.opfs.sqlite, db), JSON.stringify({
        uuid: entry.uuid, name, timezone, currency, createdAtMs: Date.now()
      }));
      db.exec('COMMIT');
      upgrade_ledger(sqlPort(this.opfs.sqlite, db), Date.now());
      copyRelations(this.opfs, source, db, {
        newUuid: entry.uuid, rebuildSearch: true, after: db => {
          for (const setting of local) db.exec({ sql: 'INSERT INTO ledger_settings VALUES(?1,?2)', bind: setting });
          db.exec({ sql: 'UPDATE ledger_meta SET revision=max(revision,?1)', bind: [BigInt(revision) + 1n] });
          const state = this.readBinding(db);
          if (state) this.writeBinding(db, { ...state, local: state.local + 1 });
        }
      });
      for (const [attachment, bytes] of image.files) await this.storeAttachment(entry, key, attachment, bytes);
      if (key) await this.opfs.saveProtected(id, db, key);
      await this.saveProfiles(this.profiles.map(previous => previous.ledgerInstanceId === entry.ledgerInstanceId ? entry : previous));
    } catch (error) {
      db.close();
      await this.removeStorage(entry, [...image.files.keys()]).catch(() => {});
      throw error;
    } finally { source.close(); }
    this.closeActive();
    const session = await this.ready(entry, db, key);
    await this.vault.revoke(secretSlot(active.entry, 'ledger')).catch(() => {});
    await this.removeStorage(active.entry, originalFiles).catch(() => {});
    return session;
  }

  private async removeStorage(entry: Profile, ids: string[]) {
    const id = this.storageId(entry);
    if (entry.protection === 'encrypted') await this.opfs.remove(`${id}.protected`).catch(() => {});
    else this.opfs.removePlain(id);
    for (const attachment of ids) await this.opfs.remove(this.blobName(entry, attachment)).catch(() => {});
  }

  private async verifyProtection(active: Active, credential?: string) {
    if (active.entry.protection !== 'encrypted' || !credential) throw unauthorized();
    const raw = derive(credential, Uint8Array.from(active.entry.salt!));
    try {
      const key = await importKey(raw);
      (await this.opfs.openProtected(this.storageId(active.entry), key)).close();
      return raw.slice();
    } finally { raw.fill(0); }
  }

  private async changeProtection(active: Active, mutation: Mutation_Deserialize, input: C.ProtectionChange_Deserialize) {
    if (mutation.expectedRevision != null && mutation.expectedRevision !== this.info(active).revision) throw issue('StaleRevision', 'stale_ledger_revision');
    if (active.entry.protection === 'encrypted') (await this.verifyProtection(active, input.oldCredential ?? undefined)).fill(0);
    const password = input.backupPassphrase ?? input.oldCredential ?? input.newCredential;
    if (!password) throw issue('Validation', 'credential_required');
    await this.createBackup(active, password);
    return this.replace(active, await this.snapshot(active, true), input.protection, input.newCredential ?? undefined);
  }

  private async quickUnlock(active: Active, mutation: Mutation_Deserialize, input: C.KeychainChange_Deserialize) {
    if (mutation.expectedRevision != null && mutation.expectedRevision !== this.info(active).revision) throw issue('StaleRevision', 'stale_ledger_revision');
    if (active.entry.protection !== 'encrypted' || (!input.enabled && input.credential)) throw issue('Validation', 'invalid_credential_options');
    const entry = { ...active.entry, quickUnlock: input.enabled };
    if (input.enabled) {
      const key = await this.verifyProtection(active, input.credential ?? undefined);
      try { await this.vault.store(secretSlot(entry, 'ledger'), key); } finally { key.fill(0); }
    } else await this.vault.revoke(secretSlot(entry, 'ledger'));
    await this.saveProfiles(this.profiles.map(item => item.ledgerInstanceId === entry.ledgerInstanceId ? entry : item));
    active.entry = entry;
    return this.versioned(active, 'ledger_keychain_set', mutation, { enabled: input.enabled }, ['ledger'],
      () => ({ protection: entry.protection, quickUnlock: entry.quickUnlock }));
  }

  private getSchedule(active: Active): C.BackupSchedule_Serialize {
    const json = active.db.selectValue("SELECT value_json FROM ledger_settings WHERE key='backup.schedule'");
    return typeof json === 'string' ? JSON.parse(json) :
      { schedule: null, remembered: false, lastAttempt: null, lastSuccess: null, error: null, destinationName: null };
  }

  private async setSchedule(active: Active, mutation: Mutation_Deserialize, input: C.BackupScheduleSet_Deserialize) {
    if (mutation.expectedRevision != null && mutation.expectedRevision !== this.info(active).revision) throw issue('StaleRevision', 'stale_ledger_revision');
    if (input.schedule && (!/^([01]\d|2[0-3]):[0-5]\d$/.test(input.schedule.localTime)
      || (input.schedule.frequency === 'weekly') !== (input.schedule.weekday != null)
      || (input.schedule.weekday != null && (input.schedule.weekday < 1 || input.schedule.weekday > 7))
      || (input.schedule.retainDays != null && (input.schedule.retainDays < 1 || input.schedule.retainDays > 3660)))) {
      throw issue('Validation', 'invalid_backup_schedule');
    }
    if (input.destinationGrant && !input.schedule) throw issue('Validation', 'schedule_required');
    const grant = input.destinationGrant ? this.consume('backupDirectory', input.destinationGrant) : undefined;
    if (Boolean(input.rememberSecret) !== Boolean(input.backupPassphrase)) throw issue('Validation', 'backup_credential_option_conflict');
    const slot = secretSlot(active.entry, 'backup');
    if (input.rememberSecret) {
      const value = text.encode(input.backupPassphrase!);
      try { await this.vault.store(slot, value); } finally { value.fill(0); }
    } else if (input.rememberSecret === false) await this.vault.revoke(slot);
    const previous = this.getSchedule(active);
    const changed = JSON.stringify(previous.schedule) !== JSON.stringify(input.schedule ?? null) || previous.destinationName !== (grant?.item.displayName ?? null);
    const status: C.BackupSchedule_Serialize = {
      schedule: input.schedule ? { ...input.schedule, weekday: input.schedule.weekday ?? null, retainDays: input.schedule.retainDays ?? null } : null,
      remembered: input.rememberSecret ?? previous.remembered, error: null, lastAttempt: changed ? null : previous.lastAttempt,
      lastSuccess: changed ? null : previous.lastSuccess, destinationName: grant?.item.displayName ?? null
    };
    return this.versioned(active, 'backup_schedule_set', mutation, { ...input, backupPassphrase: undefined }, ['backups'], () => {
      this.writeSchedule(active, status);
      active.db.exec({
        sql: "INSERT INTO ledger_settings VALUES('backup.browser_destination',?1) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json",
        bind: [JSON.stringify(grant?.item.id ?? null)]
      });
      if (changed) active.db.exec("DELETE FROM ledger_settings WHERE key='backup.browser_run'");
      return status;
    });
  }

  private writeSchedule(active: Active, status: C.BackupSchedule_Serialize) {
    active.db.exec({
      sql: "INSERT INTO ledger_settings VALUES('backup.schedule',?1) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json",
      bind: [JSON.stringify(status)]
    });
  }

  private async runSchedule(active: Active) {
    const status = this.getSchedule(active);
    const schedule = status.schedule;
    if (!schedule) return;
    const timezone = active.db.selectValue('SELECT timezone_override FROM ledger_meta');
    const clock = new Intl.DateTimeFormat('sv', {
      year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit',
      hourCycle: 'h23', timeZone: typeof timezone === 'string' ? timezone : undefined
    });
    const [today, time] = clock.format(new Date()).split(' ');
    const date = new Date(`${today}T00:00:00Z`);
    const weekday = (date.getUTCDay() + 6) % 7 + 1;
    const days = schedule.weekday == null ? 0 : (weekday - schedule.weekday + 7) % 7;
    date.setUTCDate(date.getUTCDate() - days - (days === 0 && time < schedule.localTime ? schedule.weekday == null ? 1 : 7 : 0));
    const token = date.toISOString().slice(0, 10) + 'T' + schedule.localTime;
    if (active.db.selectValue("SELECT value_json FROM ledger_settings WHERE key='backup.browser_run'") === JSON.stringify(token)) return;
    status.lastAttempt = now();
    try {
      const secret = await this.vault.retrieve(secretSlot(active.entry, 'backup'));
      if (!secret) throw issue('Validation', 'credential_required');
      const password = decodeText.decode(secret);
      secret.fill(0);
      const backup = await this.createBackup(active, password);
      const target = active.db.selectValue("SELECT value_json FROM ledger_settings WHERE key='backup.browser_destination'");
      const handle = typeof target === 'string' && JSON.parse(target) ? await this.records.get<FileSystemDirectoryHandle>(`backup-directory:${JSON.parse(target)}`) : undefined;
      if (handle) {
        const dest = await handle.getFileHandle(backup.fileName, { create: true });
        const out = await dest.createWritable();
        await out.write(await this.opfs.read(this.backupName(active.entry, backup.id)));
        await out.close();
      }
      status.lastSuccess = now();
      status.error = null;
      if (schedule.retainDays) {
        const cutoff = Date.now() - schedule.retainDays * 86400_000;
        const list = await this.backups(active.entry);
        const expired = list.filter(item => Date.parse(item.createdAt) < cutoff);
        for (const item of expired) await this.opfs.remove(this.backupName(active.entry, item.id)).catch(() => {});
        if (expired.length) await this.records.set(`backups:${active.entry.ledgerInstanceId}`, list.filter(item => !expired.includes(item)));
      }
    } catch (error) { status.error = asError(error).messageKey; }
    this.writeSchedule(active, status);
    active.db.exec({
      sql: "INSERT INTO ledger_settings VALUES('backup.browser_run',?1) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json",
      bind: [JSON.stringify(token)]
    });
    await this.persist(active);
  }

  private readBinding(db: SqliteDb): Binding | null {
    const json = db.selectValue('SELECT value_json FROM ledger_settings WHERE key=?1', [stateKey]);
    if (typeof json !== 'string') return null;
    const binding: Binding = JSON.parse(json);
    if (binding.url !== endpoint(binding.url).href || ![binding.base, binding.local, binding.acknowledged].every(value => Number.isSafeInteger(value) && value >= 0) || binding.acknowledged > binding.local) {
      throw issue('Integrity', 'invalid_turso_sync_state');
    }
    return binding;
  }

  binding(scope: Scope) {
    return this.queue.run(() => this.readBinding(this.require(scope).db));
  }

  update(scope: Scope, mutate: (state: Binding | null, populated: () => boolean) => Binding | null) {
    return this.queue.run(async () => {
      const active = this.require(scope);
      const state = mutate(this.readBinding(active.db), () => isPopulated(active.db));
      this.writeBinding(active.db, state);
      await this.persist(active);
    });
  }

  private writeBinding(db: SqliteDb, state: Binding | null) {
    if (state) db.exec({ sql: 'INSERT INTO ledger_settings VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json', bind: [stateKey, JSON.stringify(state)] });
    else db.exec({ sql: 'DELETE FROM ledger_settings WHERE key=?1', bind: [stateKey] });
  }

  async storeToken(scope: Scope, token: string) {
    const entry = this.require(scope).entry;
    const contents = text.encode(token);
    try { await this.vault.store(secretSlot(entry, 'turso'), contents); }
    finally { contents.fill(0); }
  }
  async getToken(scope: Scope) {
    const contents = await this.vault.retrieve(secretSlot(this.require(scope).entry, 'turso'));
    if (!contents) throw issue('Conflict', 'turso_token_missing');
    try { return decodeText.decode(contents); } finally { contents.fill(0); }
  }
  async revokeToken(scope: Scope) { await this.vault.revoke(secretSlot(this.require(scope).entry, 'turso')); }

  private async snapshot(active: Active, rebuildSearch: boolean): Promise<LedgerImage> {
    const db = trusted(this.opfs);
    try {
      copyRelations(this.opfs, active.db, db, { rebuildSearch });
      const files = new Map<string, Uint8Array<ArrayBuffer>>();
      for (const row of active.db.selectArrays('SELECT id,state,sha256 FROM attachments ORDER BY id')) {
        if (row[1] !== 'ready') throw issue('Conflict', 'attachment_not_ready');
        const file = await this.readAttachment(active.entry, active.key, String(row[0]));
        if (await sha256(file) !== row[2]) throw issue('Integrity', 'attachment_hash_mismatch');
        files.set(String(row[0]), file);
      }
      return { database: this.opfs.image(db), files };
    } finally { db.close(); }
  }

  capture(scope: Scope) {
    return this.queue.run(async () => {
      const active = this.require(scope);
      const state = this.readBinding(active.db);
      if (!state) throw issue('Integrity', 'invalid_turso_sync_state');
      return { state, image: assemble(await this.snapshot(active, false)) };
    });
  }

  async apply(scope: Scope, base: number, generation: number, bytes: Uint8Array<ArrayBuffer>, discardVersion?: number) {
    const image = disassemble(bytes);
    await checkImage(this.opfs, image);
    await this.queue.run(async () => {
      const active = this.require(scope);
      const state = this.readBinding(active.db);
      if (!state || state.base !== base || state.local !== (discardVersion ?? state.acknowledged)) {
        throw issue('Conflict', 'turso_concurrent_ledger_edits', { remoteGeneration: String(generation) });
      }
      for (const [id, file] of image.files) await this.storeAttachment(active.entry, active.key, id, file, true);
      const source = this.opfs.fromBytes(image.database);
      try {
        copyRelations(this.opfs, source, active.db, {
          preserve: true, rebuildSearch: true,
          after: db => this.writeBinding(db, { ...state, base: generation, acknowledged: state.local, last_success_at: new Date().toISOString() })
        });
        await this.persist(active);
      } finally { source.close(); }
    });
  }

  private storageId(entry: Profile) { return entry.storageId ?? entry.ledgerInstanceId; }

  private blobName(entry: Profile, id: string) {
    if (!/^[\da-f]{8}-[\da-f]{4}-[\da-f]{4}-[\da-f]{4}-[\da-f]{12}$/.test(id)) throw issue('Validation', 'invalid_attachment_id');
    return `${this.storageId(entry)}-${id}.blob`;
  }

  private async storeAttachment(entry: Profile, key: CryptoKey | undefined, id: string, bytes: Uint8Array<ArrayBuffer>, preserve = false) {
    const name = this.blobName(entry, id);
    if (preserve) {
      try {
        const previous = await this.readAttachment(entry, key, id);
        if (await sha256(previous) !== await sha256(bytes)) throw issue('Conflict', 'sync_attachment_identity_conflict');
        return;
      } catch (error) {
        if (!(error instanceof DOMException && error.name === 'NotFoundError')) throw error;
      }
    }
    if (!key) return this.opfs.write(name, [bytes]);
    const iv = crypto.getRandomValues(new Uint8Array(12));
    const ciphertext = await crypto.subtle.encrypt({ name: 'AES-GCM', iv, additionalData: text.encode(name) }, key, bytes);
    return this.opfs.write(name, [iv, ciphertext]);
  }

  private async readAttachment(entry: Profile, key: CryptoKey | undefined, id: string) {
    const name = this.blobName(entry, id);
    const bytes = new Uint8Array(await (await this.opfs.read(name)).arrayBuffer());
    if (!key) return bytes;
    return new Uint8Array(await crypto.subtle.decrypt({
      name: 'AES-GCM', iv: bytes.subarray(0, 12), additionalData: text.encode(name)
    }, key, bytes.subarray(12)));
  }
}

function attachmentType(name: string, bytes: Uint8Array): string {
  const prefix = (pattern: number[]) => pattern.every((value, index) => bytes[index] === value);
  const ascii = (start: number, end: number) => new TextDecoder().decode(bytes.subarray(start, end));
  const extension = name.split('.').pop()?.toLowerCase();
  if (extension === 'png' && prefix([137, 80, 78, 71, 13, 10, 26, 10])) return 'image/png';
  if ((extension === 'jpg' || extension === 'jpeg') && prefix([255, 216, 255])) return 'image/jpeg';
  if (extension === 'gif' && ['GIF87a', 'GIF89a'].includes(ascii(0, 6))) return 'image/gif';
  if (extension === 'webp' && ascii(0, 4) === 'RIFF' && ascii(8, 12) === 'WEBP') return 'image/webp';
  if (extension === 'pdf' && ascii(0, 5) === '%PDF-') return 'application/pdf';
  if (['heic', 'heif'].includes(extension ?? '') && ascii(4, 8) === 'ftyp' && ['heic','heix','hevc','hevx'].includes(ascii(8,12))) return 'image/heic';
  throw issue('Validation', 'attachment_signature_mismatch');
}

let runtime: Promise<WorkerRuntime> | undefined;

self.addEventListener('message', (event: MessageEvent<Request>) => {
  const request = event.data;
  async function handle() {
    if (request.type === 'initialize') {
      runtime ??= (async () => {
        await initEngine({ module_or_path: request.engine });
        const opfs = await Opfs.open(request.sqlite);
        const host = new WorkerRuntime(opfs, new BrowserRecords(await openRecords()));
        await host.initialize();
        return host;
      })().catch(error => {
        if (asError(error).messageKey === 'browser_activity_in_use') runtime = undefined;
        throw error;
      });
      await runtime;
      return null;
    }
    if (!runtime) throw issue('Io', 'browser_engine_not_initialized');
    const host = await runtime;
    if (request.type === 'grant') return host.grant(request.purpose, request.name, request.file);
    return host.invoke(request.command, request.args);
  }
  void handle().then(
    data => send({ type: 'reply', id: request.id, result: { status: 'ok', data } }),
    error => send({ type: 'reply', id: request.id, result: { status: 'error', error: asError(error) } })
  );
});
