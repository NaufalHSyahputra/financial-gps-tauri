import { session } from './stores/session.svelte';
import { privacy } from './stores/privacy.svelte';
import { theme } from './stores/theme.svelte';
import { goto } from '$app/navigation';
import { getContext, setContext } from 'svelte';
import {
  commands,
  type AppError,
  type CreateRequest_Deserialize,
  type LedgerDescriptor,
  type LedgerSession,
  type OpenRequest_Deserialize,
  type PathGrant,
  type UnlockChallenge
} from './drivers/runtime';

const launcherKey = Symbol('ledger launcher');
const startupTimeoutMs = 5000;

class StartupTimeout extends Error {}

const messages: Record<string, string> = {
  bad_credential_or_corrupt: 'The credential is incorrect or the activity or key file is damaged.',
  browser_activity_in_use: 'Financial GPS is already open in another tab. Close that tab, then try again here.',
  browser_engine_asset_unavailable: 'The app files could not load. Reload this page and try again.',
  browser_storage_operation_failed: 'Browser storage is unavailable. Check your browser storage settings and try again.',
  worker_failed: 'The browser database stopped responding. Reload this page and try again.',
  credential_required: 'Enter the passphrase or PIN for this activity.',
  destination_exists: 'That file already exists. Choose a different filename.',
  destination_sidecar_exists: 'A file that belongs to an activity is already at that location. Choose a different filename.',
  file_access_denied: 'This file or folder is read-only or inaccessible.',
  file_not_found: 'The activity file could not be found. Locate the file and open it again.',
  invalid_ledger_name: 'Enter a name of up to 256 bytes with no control characters.',
  invalid_or_expired_path_grant: 'Choose the file again. The previous selection has expired or was used.',
  keychain_unavailable: 'The secure credential vault is unavailable. Use your passphrase or PIN to unlock.',
  ledger_busy: 'The activity file is busy or open in another app instance. Try again shortly.',
  ledger_file_alias: 'This file already appears in your saved activity under a different name.',
  ledger_header_invalid: 'The selected file has an invalid database header.',
  ledger_integrity_check_failed: 'We couldn’t verify the activity file.',
  migration_failed_safety_copy_available: 'The upgrade rolled back. Keep the original activity file and its .fgps-pre-migration safety copy. Open the original with the previous app version or contact support.',
  migration_rolled_back: 'The upgrade rolled back. Open the original activity file with the previous app version or contact support.',
  migration_checksum_mismatch: 'The activity file has an unexpected format for this version of the app.',
  stale_ledger_session: 'This activity is no longer open. Open it again from Your activity.',
  storage_full: 'Free disk space and try again.',
  unsafe_ledger_path: 'Choose a local .db file in a folder without symbolic links.',
  unsupported_ledger_schema: 'This file was created with a file format this version does not support.'
};

function failure(error: unknown): string {
  if (error instanceof StartupTimeout) {
    return 'Financial GPS did not respond. Try again or restart Financial GPS.';
  }
  if (error instanceof Error) return error.message;
  if (typeof error === 'object' && error !== null && 'messageKey' in error) {
    const appError = error as AppError;
    const issue = Object.values(appError.fields ?? {})[0];
    const message = (issue && messages[issue]) || messages[appError.messageKey];
    return message || `The operation failed. Reference ${appError.correlationId}.`;
  }
  return 'Financial GPS could not complete this request.';
}

async function call<T>(promise: Promise<{ status: 'ok'; data: T } | { status: 'error'; error: AppError }>): Promise<T> {
  const result = await promise;
  if (result.status === 'error') throw result.error;
  return result.data;
}

async function readLauncher() {
  const bootstrap = await call(commands.appBootstrap());
  const ledgers = await call(commands.ledgerList());
  return { ...bootstrap, ledgers };
}

export class Launcher {
  get session() { return session.active; }
  set session(value: LedgerSession | null) { session.set(value); }
  challenge = $state<UnlockChallenge | null>(null);
  ledgers = $state<LedgerDescriptor[]>([]);
  busy = $state(false);
  loaded = $state(false);
  error = $state('');

  async run(action: () => Promise<void>) {
    if (this.busy) return;
    this.busy = true;
    this.error = '';
    try {
      await action();
    } catch (error) {
      this.error = failure(error);
    } finally {
      this.busy = false;
    }
  }

  async refreshLedgers() {
    this.ledgers = await call(commands.ledgerList());
  }

  async refresh() {
    const epoch = session.epoch;
    const { session: active, ledgers } = await readLauncher();
    if (session.epoch !== epoch) return;
    this.session = active;
    this.ledgers = ledgers;
  }

  async initialize() {
    await this.run(async () => {
      let timer: number | undefined;
      try {
        const timeout = new Promise<never>((_, reject) => {
          timer = window.setTimeout(() => reject(new StartupTimeout()), startupTimeoutMs);
        });
        const { session, prefs, ledgers } = await Promise.race([readLauncher(), timeout]);
        privacy.apply(prefs);
        theme.hydrate(prefs.theme);
        this.session = session;
        this.ledgers = ledgers;
        this.loaded = true;
      } finally {
        window.clearTimeout(timer);
      }
    });
  }

  async chooseDestination(): Promise<PathGrant | undefined> {
    const [grant] = await call(commands.fileChoose({ purpose: 'ledgerCreate' }));
    return grant;
  }

  async create(input: CreateRequest_Deserialize, enroll?: (ledger: LedgerSession) => Promise<void>) {
    this.challenge = null;
    try {
      this.session = await call(commands.ledgerCreate(input));
    } catch (error) {
      await this.refresh().catch(() => {});
      throw error;
    }
    this.ledgers = await call(commands.ledgerList());
    if (enroll && this.session) await enroll(this.session);
    await goto('/dashboard');
  }

  async chooseExisting() {
    const [grant] = await call(commands.fileChoose({ purpose: 'ledgerOpen' }));
    if (grant) await this.open({ openGrant: grant.id });
  }

  async open(target: Omit<OpenRequest_Deserialize, 'requestId'>) {
    this.challenge = null;
    const request = { ...target, requestId: crypto.randomUUID() };
    const command = this.session ? commands.ledgerSwitch : commands.ledgerOpen;
    try {
      const result = await call(command(request));
      session.set(result.session, result.state);
      this.challenge = result.unlockChallenge;
    } catch (error) {
      await this.refresh().catch(() => {});
      throw error;
    }
    if (this.challenge) {
      await goto('/unlock');
    } else {
      this.ledgers = await call(commands.ledgerList());
      await goto('/dashboard');
    }
  }

  async unlock(credential?: string) {
    if (!this.challenge) return;
    this.session = await call(commands.ledgerUnlock({
      unlockChallenge: this.challenge.id,
      credential
    }));
    this.challenge = null;
    await goto('/dashboard');
    this.ledgers = await call(commands.ledgerList());
  }

  async lock() {
    if (!this.session) return;
    const { scope } = this.session;
    await call(commands.ledgerLock(scope));
    session.set(null, 'locked');
    this.challenge = null;
    await goto('/');
  }

  async close() {
    if (!this.session) return;
    await call(commands.ledgerClose(this.session.scope));
    this.session = null;
    this.challenge = null;
    await goto('/');
  }
}

export function provideLauncher(): Launcher {
  return setContext(launcherKey, new Launcher());
}

export function useLauncher(): Launcher {
  return getContext(launcherKey);
}
