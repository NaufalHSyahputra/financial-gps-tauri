import sqliteBinary from '@sqlite.org/sqlite-wasm/sqlite3.wasm?url';
import engineBinary from './generated/web_engine_bg.wasm?url';
import type { ChooseFile_Deserialize, DatabaseDriver, DriverEvents, DriverResult, PathGrant, RuntimeDriver, Scope } from '../contracts';
import { operations } from '../operations';
import { asError, issue } from './errors';
import { BrowserRecords, openRecords } from './secret-vault';
import type { EventName, EventValue, Request, Response } from './protocol';

type RequestBody<T = Request> = T extends Request ? Omit<T, 'id'> : never;
type DirectoryOptions = { mode?: 'readwrite' };
type SaveOptions = { suggestedName: string };
type FileWindow = Window & {
  showSaveFilePicker?: (options: SaveOptions) => Promise<FileSystemFileHandle>;
  showDirectoryPicker?: (options: DirectoryOptions) => Promise<FileSystemDirectoryHandle>;
};

const saveNames: Partial<Record<ChooseFile_Deserialize['purpose'], string>> = {
  ledgerCreate: 'ledger.db',
  transactionExport: 'transactions.csv',
  rulePackWrite: 'rules.json',
  attachmentWrite: 'attachment',
  backupWrite: 'ledger.fgpsbackup'
};
const readTypes: Partial<Record<ChooseFile_Deserialize['purpose'], string>> = {
  ledgerOpen: '.db,.sqlite,.sqlite3,.fgpsbackup',
  rulePackRead: '.json',
  attachmentRead: 'image/*,application/pdf',
  restoreRead: '.fgpsbackup'
};

function pickFiles(accept: string, multiple: boolean): Promise<File[]> {
  return new Promise(resolve => {
    const picker = document.createElement('input');
    picker.type = 'file';
    picker.accept = accept;
    picker.multiple = multiple;
    picker.addEventListener('change', () => resolve(Array.from(picker.files ?? [])), { once: true });
    picker.addEventListener('cancel', () => resolve([]), { once: true });
    picker.click();
  });
}

export class WebStandaloneDriver implements RuntimeDriver {
  private readonly worker = new Worker(new URL('./db.worker.ts', import.meta.url), { type: 'module' });
  private readonly pending = new Map<number, (response: DriverResult<unknown>) => void>();
  private readonly subscribers = new Map<EventName, Set<(value: never) => void>>();
  private readonly outputFiles = new Map<string, FileSystemFileHandle | FileSystemDirectoryHandle>();
  private ready?: Promise<void>;
  private sequence = 0;
  private readonly controllers = new Map<string, { buffer: Int32Array<SharedArrayBuffer>; scope?: Scope }>();
  private failure?: ReturnType<typeof issue>;

  constructor() {
    if (import.meta.env.PROD && 'serviceWorker' in navigator) {
      void navigator.serviceWorker.register('/service-worker.js', { type: 'module' }).catch(() => {});
    }
    this.worker.onmessage = ({ data }: MessageEvent<Response>) => {
      if (data.type === 'reply') {
        this.pending.get(data.id)?.(data.result);
        this.pending.delete(data.id);
      } else if (data.type === 'job-control') {
        this.controllers.set(data.id, { buffer: new Int32Array(data.control), scope: data.scope });
      } else if (data.type === 'event') {
        if (data.name === 'ledgerJobProgress' && !data.value.cancellable) this.controllers.delete(data.value.jobId);
        for (const callback of this.subscribers.get(data.name) ?? []) callback(data.value as never);
      } else {
        void this.save(data).catch(error => window.dispatchEvent(new ErrorEvent('error', { error: asError(error) })));
      }
    };
    this.worker.onerror = () => this.disconnect('worker_failed');
    this.worker.onmessageerror = () => this.disconnect('invalid_worker_response');
  }

  private initialize(): Promise<void> {
    return this.ready ??= Promise.all([fetch(sqliteBinary), fetch(engineBinary)])
      .then(responses => Promise.all(responses.map(response => {
        if (!response.ok) throw issue('Io', 'browser_engine_asset_unavailable');
        return response.arrayBuffer();
      })))
      .then(async ([sqlite, engine]) => {
        const result = await this.request({ type: 'initialize', sqlite, engine }, [sqlite, engine]);
        if (result.status === 'error') throw result.error;
      })
      .catch(error => {
        if (asError(error).messageKey === 'browser_activity_in_use') this.ready = undefined;
        throw error;
      });
  }

  private disconnect(message: string) {
    this.failure = issue('Io', message);
    for (const resolve of this.pending.values()) resolve({ status: 'error', error: this.failure });
    this.pending.clear();
    this.worker.terminate();
  }

  /** Release the OPFS access handles before replacing a worker instance. */
  close() { this.disconnect('worker_closed'); }

  private request<T>(body: RequestBody, transfers: Transferable[] = []): Promise<DriverResult<T>> {
    return new Promise(resolve => {
      if (this.failure) return resolve({ status: 'error', error: this.failure });
      const id = ++this.sequence;
      this.pending.set(id, resolve as (result: DriverResult<unknown>) => void);
      try {
        this.worker.postMessage({ ...body, id }, transfers);
      } catch (error) {
        this.pending.delete(id);
        resolve({ status: 'error', error: asError(error) });
      }
    });
  }

  private async invoke(command: string, args: Record<string, unknown>): Promise<DriverResult<unknown>> {
    try {
      await this.initialize();
      // Command DTOs use the same JSON representation as native IPC. This also
      // strips UI reactivity proxies before the worker's structured clone.
      return await this.request({ type: 'invoke', command, args: JSON.parse(JSON.stringify(args)) });
    } catch (error) {
      return { status: 'error', error: asError(error) };
    }
  }

  private async choose(input: ChooseFile_Deserialize): Promise<DriverResult<PathGrant[]>> {
    try {
      await this.initialize();
      const files: PathGrant[] = [];
      const purpose = input.purpose;
      if (purpose === 'statementImport' || purpose === 'templateRead' || purpose === 'templateWrite') {
        throw issue('Conflict', 'feature_unavailable');
      }
      const suggest = saveNames[purpose];
      const register = async (name: string, file?: File) => {
        const grant = await this.request<PathGrant>({ type: 'grant', name, file, purpose });
        if (grant.status === 'error') throw grant.error;
        files.push(grant.data);
        return grant.data.id;
      };
      const host = window as FileWindow;
      if (purpose === 'backupDirectory') {
        const directory = host.showDirectoryPicker ? await host.showDirectoryPicker({ mode: 'readwrite' }) : undefined;
        const id = await register(directory?.name ?? 'Browser storage');
        if (directory) {
          this.outputFiles.set(id, directory);
          const db = await openRecords();
          try { await new BrowserRecords(db).set(`backup-directory:${id}`, directory); }
          finally { db.close(); }
        }
      } else if (suggest) {
        const file = host.showSaveFilePicker ? await host.showSaveFilePicker({ suggestedName: suggest }) : undefined;
        const id = await register(file?.name ?? suggest);
        if (file) this.outputFiles.set(id, file);
      } else {
        for (const file of await pickFiles(readTypes[purpose] ?? '', input.multiple ?? false)) {
          await register(file.name, file);
        }
      }
      return { status: 'ok', data: files };
    } catch (error) {
      if (error instanceof DOMException && error.name === 'AbortError') return { status: 'ok', data: [] };
      return { status: 'error', error: asError(error) };
    }
  }

  private async save(output: Extract<Response, { type: 'file' }>) {
    const destination = this.outputFiles.get(output.grant);
    if (destination) {
      if (destination.kind === 'file') this.outputFiles.delete(output.grant);
      const file = destination.kind === 'directory'
        ? await (destination as FileSystemDirectoryHandle).getFileHandle(output.name, { create: true })
        : destination as FileSystemFileHandle;
      const stream = await file.createWritable();
      await stream.write(output.bytes);
      await stream.close();
      return;
    }
    const url = URL.createObjectURL(new Blob([output.bytes], { type: output.media }));
    const link = document.createElement('a');
    link.href = url;
    link.download = output.open ? '' : output.name;
    link.target = '_blank';
    link.rel = 'noopener';
    link.click();
    setTimeout(() => URL.revokeObjectURL(url), 30_000);
  }

  readonly commands = Object.fromEntries(Object.entries(operations).map(([key, [command, ...names]]) => {
    if (key === 'fileChoose') return [key, (input: ChooseFile_Deserialize) => this.choose(input)];
    return [key, (...args: unknown[]) => {
      if (key === 'jobCancel') {
        const control = this.controllers.get(String(args[0]));
        const scope = args[1] as Scope | undefined;
        if (control && control.scope?.sessionId === scope?.sessionId && control.scope?.ledgerInstanceId === scope?.ledgerInstanceId) {
          Atomics.compareExchange(control.buffer, 0, 0, 1);
        }
      }
      if (key === 'ledgerCreate') void navigator.storage.persist?.().catch(() => false);
      return this.invoke(command, Object.fromEntries(names.map((name, index) => [name, args[index]])));
    }];
  })) as unknown as DatabaseDriver;

  private async subscribe<K extends EventName>(name: K, callback: (value: EventValue<K>) => void) {
    const listeners = this.subscribers.get(name) ?? new Set();
    this.subscribers.set(name, listeners);
    listeners.add(callback as (value: never) => void);
    return () => { listeners.delete(callback as (value: never) => void); };
  }

  readonly events: DriverEvents = {
    ledgerJobProgress: callback => this.subscribe('ledgerJobProgress', callback),
    ledgerIntegrityAlert: callback => this.subscribe('ledgerIntegrityAlert', callback),
    ledgerSyncStatus: callback => this.subscribe('ledgerSyncStatus', callback)
  };

  readonly secrets = {
    testSavedTurso: this.commands.ledgerSyncProbeSaved,
    validateTurso: this.commands.tursoCredentialsValidate,
    setStorageMode: this.commands.ledgerStorageModeSet,
    setQuickUnlock: this.commands.ledgerKeychainSet
  };
}
