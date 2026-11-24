import { runtime, type SetStorageMode, type Scope, type TursoCredentials } from './runtime';

export async function testConnection(input: TursoCredentials) {
  const result = await (await runtime()).secrets.validateTurso(input);
  if (result.status === 'error') throw result.error;
}

export async function testSavedConnection(scope: Scope) {
  const result = await (await runtime()).secrets.testSavedTurso(scope);
  if (result.status === 'error') throw result.error;
}

export async function saveStorageMode(scope: Scope, input: SetStorageMode) {
  const result = await (await runtime()).secrets.setStorageMode(scope, input);
  if (result.status === 'error') throw result.error;
  return result.data;
}

export function storageMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (error && typeof error === 'object' && 'messageKey' in error) {
    const source = error as import('./contracts').AppError;
    if (source.fields?.url === 'invalid_turso_endpoint') return 'Enter a Turso database URL using libsql:// or https:// and a turso.io hostname.';
    if (source.fields?.authToken === 'invalid_turso_token') return 'Enter a Turso auth token without spaces or line breaks.';
    if (source.fields?.mode === 'turso_requires_managed_ledger') return 'Cloud Sync requires activity saved in the app.';
    const messages: Record<string, string> = {
      turso_auth_rejected: 'Turso rejected the token. Check that it is valid for this database.',
      turso_unreachable: 'Could not reach Turso. Check your connection and try again.',
      turso_concurrent_ledger_edits: 'Both versions changed. Review the latest versions before choosing what to keep.',
      turso_token_missing: 'Enter a valid Turso token in Update credentials.',
      turso_not_configured: 'Turn on Cloud Sync to test its connection.',
      disable_sync_before_changing_endpoint: 'Switch to Local Only before enrolling with a different Turso database.',
      ledger_busy: 'Sync is already running. Try again in a moment.',
      keychain_unavailable: 'The secure credential vault is unavailable.'
    };
    return messages[source.messageKey] ?? 'We couldn’t change your sync settings. Try again.';
  }
  return 'The storage operation could not be completed.';
}
