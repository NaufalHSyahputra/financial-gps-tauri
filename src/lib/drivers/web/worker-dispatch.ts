import type { Mutation_Deserialize, Scope, SetStorageMode, TursoCredentials } from '../contracts';
import type { WebEngine } from './generated/web_engine';
import type { Jobs } from './jobs';
import { sqlPort, type Opfs, type SqliteDb } from './opfs';
import { endpoint, type Sync } from './sync';

export type RequestArgs = Record<string, unknown>;

export const jobKinds = {
      rule_preview_start: 'rulePreview',
      rule_apply_start: 'ruleApply', transactions_export_start: 'transactionsExport',
      attachment_add_start: 'attachmentAdd', backup_create_start: 'backupCreate', restore_preview_start: 'restorePreview',
      restore_apply_start: 'restoreApply', ledger_change_protection: 'ledgerChangeProtection'
    } as const;

export async function dispatchWorker(command: string, args: RequestArgs, {
  jobs, sync, requireScope, local
}: {
  jobs: Jobs; sync: Sync; requireScope: (scope: Scope) => void;
  local: (command: string, args: RequestArgs) => Promise<unknown>;
}): Promise<unknown> {
    const scope = args.scope as Scope;
    switch (command) {
      case 'job_status': {
        if (scope) requireScope(scope);
        return jobs.get(String(args.id), scope).status;
      }
      case 'job_cancel': {
        if (scope) requireScope(scope);
        return jobs.cancel(String(args.id), scope);
      }
      case 'turso_credentials_validate': {
        const credentials = args.input as TursoCredentials;
        endpoint(credentials.url);
        await sync.validate(credentials);
        return null;
      }
      case 'ledger_storage_mode_set': {
        const input = args.input as SetStorageMode;
        if (input.mode === 'turso-sync') endpoint(input.url);
        return sync.set(scope, input);
      }
      case 'ledger_sync_probe_saved': await sync.probeSaved(scope); return null;
      case 'ledger_sync_status': return sync.status(scope);
      case 'ledger_sync_now': return sync.run(scope);
      case 'ledger_sync_publish_local': return sync.run(scope, { generation: args.expectedRemoteGeneration as string });
      case 'ledger_sync_accept_remote': {
        const input = args.input as { expectedRemoteGeneration: string; expectedLocalVersion: string };
        return sync.run(scope, { generation: input.expectedRemoteGeneration, discardVersion: input.expectedLocalVersion });
      }
      default: return local(command, args);
    }
  }

export async function dispatchDomain<T extends { db: SqliteDb }>(
  command: string, args: RequestArgs, write: boolean, engine: WebEngine, opfs: Opfs,
  selected: (scope: Scope) => T, persist: (active: T) => Promise<void>
): Promise<unknown> {
    const scope = (args.mutation as Mutation_Deserialize | undefined)?.scope ?? args.scope as Scope;
    const active = selected(scope);
    const port = sqlPort(opfs.sqlite, active.db);
    const data: unknown = JSON.parse(engine.execute(port, command, JSON.stringify(args)));
    if (write) await persist(active);
    return data;
  }
