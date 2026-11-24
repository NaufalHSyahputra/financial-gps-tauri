import { commands, events, type Scope, type SyncStatus_Serialize } from '../drivers/runtime';
import { session } from './session.svelte';

class StorageSync {
  private current = $state<{ scope: Scope; status: SyncStatus_Serialize }>();
  private sequence = 0;

  get status() {
    return this.current && this.matches(this.current.scope) ? this.current.status : undefined;
  }

  get conflict() {
    const status = this.status;
    const generation = status?.error?.messageKey === 'turso_concurrent_ledger_edits'
      ? status.error.fields?.remoteGeneration : undefined;
    return generation && status?.pending && status.localVersion
      ? { generation, localVersion: status.localVersion, base: status.remoteGeneration }
      : undefined;
  }

  private matches(scope: Scope) {
    return session.active?.scope.sessionId === scope.sessionId &&
      session.active.scope.ledgerInstanceId === scope.ledgerInstanceId;
  }

  receive(scope: Scope, status: SyncStatus_Serialize) {
    if (this.matches(scope)) {
      this.sequence++;
      this.current = { scope, status };
    }
  }

  async refresh() {
    const scope = session.active?.scope;
    if (!scope) return;
    const sequence = ++this.sequence;
    const result = await commands.ledgerSyncStatus({ ...scope });
    if (result.status === 'ok' && this.sequence === sequence) this.receive(scope, result.data);
  }

  watch() {
    let stopped = false;
    let polling = false;
    let unsubscribe: (() => void) | undefined;
    const poll = async () => {
      if (stopped || polling) return;
      polling = true;
      try { await this.refresh(); } catch { /* The next poll can recover from a transport error. */ }
      finally { polling = false; }
    };
    void events.ledgerSyncStatus(([scope, status]) => {
      if (!stopped) this.receive(scope, status);
    }).then(stop => {
      if (stopped) stop();
      else unsubscribe = stop;
    }).catch(() => {});
    const timer = setInterval(poll, 2000);
    void poll();
    return () => {
      stopped = true;
      clearInterval(timer);
      unsubscribe?.();
    };
  }
}

export const storageSync = new StorageSync();
