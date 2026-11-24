import { commands, events, type JobOutput_Serialize, type JobTicket, type Progress_Serialize, type Scope } from '../drivers/runtime';
import { session } from './session.svelte';

type JobValues = { [Item in JobOutput_Serialize as Item["kind"]]: Item["value"] };

class JobsStore {
  progress = $state<Record<string, Progress_Serialize>>({});
  private tracked = new Map<string, Scope | null>();

  clear() { this.progress = {}; this.tracked.clear(); }

  async listen() {
    return events.ledgerJobProgress(payload => {
      if (!this.tracked.has(payload.jobId)) return;
      if (payload.sessionId !== (this.tracked.get(payload.jobId)?.sessionId ?? null)) return;
      if ((this.progress[payload.jobId]?.seq ?? -1) >= payload.seq) return;
      this.progress[payload.jobId] = payload;
    });
  }

  async result<K extends JobOutput_Serialize['kind']>(ticket: JobTicket, kind: K, scope: Scope | null = session.scope): Promise<JobValues[K]> {
    const epoch = session.epoch;
    this.tracked.set(ticket.id, scope);
    this.progress[ticket.id] = { jobId:ticket.id, sessionId:scope?.sessionId ?? null, seq:0, kind:ticket.kind, phase:'queued', state:ticket.state, cancellable:true };
    try {
      while (epoch === session.epoch && this.tracked.has(ticket.id)) {
        try {
          const item = await session.call(commands.jobStatus(ticket.id, scope), scope !== null);
          if (item.output?.kind === kind) return item.output.value as JobValues[K];
          if (item.error) throw item.error;
          if (item.state === 'cancelled') throw new Error('The operation was cancelled.');
          if (item.state === 'failed' || item.state === 'complete') throw new Error('The job returned an unexpected result.');
          if (item.seq >= (this.progress[ticket.id]?.seq ?? -1)) {
            this.progress[ticket.id] = { ...this.progress[ticket.id], state:item.state, seq:item.seq, done:item.done ?? undefined, cancellable:item.cancellable };
          }
        } catch (error) {
          if (!error || typeof error !== 'object' || !('code' in error) || error.code !== 'Busy') throw error;
        }
        await new Promise(resolve => setTimeout(resolve, 350));
      }
      throw new Error('This task belongs to activity you closed.');
    } finally {
      this.tracked.delete(ticket.id);
      delete this.progress[ticket.id];
    }
  }

  async cancel(id: string) {
    if (this.tracked.has(id)) await session.call(commands.jobCancel(id, this.tracked.get(id) ?? null), false);
  }
}
export const jobs = new JobsStore();
