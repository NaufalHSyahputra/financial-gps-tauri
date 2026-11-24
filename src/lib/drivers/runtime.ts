import type { DatabaseDriver, DriverEvents, RuntimeDriver } from './contracts';
import { operations } from './operations';

export type * from './contracts';

let selected: Promise<RuntimeDriver> | undefined;

export function runtime(): Promise<RuntimeDriver> {
  return selected ??= '__TAURI_INTERNALS__' in window
    ? import('./native/tauri-native-driver').then(({ TauriNativeDriver }) => new TauriNativeDriver())
    : import('./web/web-standalone-driver').then(({ WebStandaloneDriver }) => new WebStandaloneDriver());
}

export const commands = Object.fromEntries(
  Object.keys(operations).map(key => [key, async (...args: unknown[]) => {
    const driver = await runtime();
    const call = driver.commands[key as keyof DatabaseDriver] as (...args: unknown[]) => Promise<unknown>;
    return call(...args);
  }])
) as unknown as DatabaseDriver;

export const events: DriverEvents = {
  ledgerJobProgress: async listener => (await runtime()).events.ledgerJobProgress(listener),
  ledgerIntegrityAlert: async listener => (await runtime()).events.ledgerIntegrityAlert(listener),
  ledgerSyncStatus: async listener => (await runtime()).events.ledgerSyncStatus(listener)
};
