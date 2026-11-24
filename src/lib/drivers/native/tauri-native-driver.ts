import { listen } from '@tauri-apps/api/event';
import { commands, events as nativeEvents } from './bindings';
import type { DriverEvents, RuntimeDriver } from '../contracts';

export class TauriNativeDriver implements RuntimeDriver {
  readonly commands = commands;

  readonly secrets = {
    testSavedTurso: commands.ledgerSyncProbeSaved,
    validateTurso: commands.tursoCredentialsValidate,
    setStorageMode: commands.ledgerStorageModeSet,
    setQuickUnlock: commands.ledgerKeychainSet
  };

  readonly events: DriverEvents = {
    ledgerJobProgress: listener => nativeEvents.ledgerJobProgress.listen(event => listener(event.payload)),
    ledgerIntegrityAlert: listener => listen('ledger-integrity-alert', event => listener(event.payload as Parameters<typeof listener>[0])),
    ledgerSyncStatus: listener => listen('ledger-sync-status', event => listener(event.payload as Parameters<typeof listener>[0]))
  };
}
