import type { PathGrant, PathPurpose } from '../contracts';
import { issue } from './errors';

export type Grant = { item: PathGrant; file?: File; expires: number };
export const noFile = () => issue('Validation', 'invalid_input', { grant: 'invalid_or_expired_path_grant' });

export class WorkerGrants {
  private readonly pending = new Map<string, Grant>();

  issue(purpose: PathPurpose, name: string, file?: File): PathGrant {
    const time = Date.now();
    for (const [id, value] of this.pending) if (value.expires < time) this.pending.delete(id);
    const expires = time + 300_000;
    const item = { id: crypto.randomUUID(), purpose, displayName: name, expiresAt: new Date(expires).toISOString() };
    if (this.pending.size >= 256) throw issue('Busy', 'ledger_busy');
    this.pending.set(item.id, { item, file, expires });
    return item;
  }

  consume(purpose: PathPurpose, id: string) {
    const grant = this.pending.get(id);
    if (!grant || grant.item.purpose !== purpose || grant.expires <= Date.now()) throw noFile();
    this.pending.delete(id);
    return grant;
  }
}
