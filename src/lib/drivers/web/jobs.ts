import type { JobKind, JobOutput_Serialize, JobSnapshot_Serialize, JobTicket, Progress_Serialize, Scope } from '../contracts';
import { asError, issue } from './errors';

type Job = { scope?: Scope; status: JobSnapshot_Serialize; phase: string; cancel: AbortController; control?: Int32Array<SharedArrayBuffer> };

export class Jobs {
  private readonly items = new Map<string, Job>();
  constructor(
    private emit: (progress: Progress_Serialize) => void,
    private share: (id: string, scope: Scope | undefined, buffer: SharedArrayBuffer) => void
  ) {}

  start(scope: Scope | undefined, kind: JobKind, phase: string, operation: (signal: AbortSignal, observer: JobObserver) => Promise<unknown>): JobTicket {
    if (this.items.size >= 100) {
      for (const [id, job] of this.items) {
        if (['complete', 'failed', 'cancelled'].includes(job.status.state)) this.items.delete(id);
        if (this.items.size < 100) break;
      }
      if (this.items.size >= 100) throw issue('Busy', 'job_queue_full');
    }
    const id = crypto.randomUUID();
    const status: JobSnapshot_Serialize = { id, sessionId: scope?.sessionId ?? null, kind, state: 'queued', seq: 0, done: null, cancellable: true, output: null, error: null };
    const control = typeof SharedArrayBuffer === 'function' ? new Int32Array(new SharedArrayBuffer(4)) : undefined;
    const job = { scope, status, phase, cancel: new AbortController(), control };
    const observer: JobObserver = {
      cancelled: () => this.cancelled(job),
      publishing: () => {
        if (observer.cancelled()) return false;
        if (control && Atomics.compareExchange(control, 0, 0, 2) === 1) return false;
        if (status.state !== 'publishing') this.publish(job, 'publishing');
        return true;
      },
      progress: done => {
        status.done = done;
        this.publish(job, status.state);
      }
    };
    if (control) this.share(id, scope, control.buffer);
    this.items.set(id, job);
    setTimeout(() => {
      void (async () => {
        try {
          if (observer.cancelled()) throw issue('Cancelled', 'operation_cancelled');
          this.publish(job, 'running');
          const value = await operation(job.cancel.signal, observer);
          if (observer.cancelled()) throw issue('Cancelled', 'operation_cancelled');
          status.output = { kind, value } as JobOutput_Serialize;
          this.publish(job, 'complete');
        } catch (error) {
          const failure = observer.cancelled() ? issue('Cancelled', 'operation_cancelled') : asError(error);
          status.error = failure;
          this.publish(job, failure.code === 'Cancelled' ? 'cancelled' : 'failed');
        }
      })();
    }, 0);
    return { id, kind, state: 'queued' };
  }

  get(id: string, scope?: Scope | null) {
    const job = this.items.get(id);
    if (!job || job.scope?.sessionId !== scope?.sessionId || job.scope?.ledgerInstanceId !== scope?.ledgerInstanceId) {
      throw issue('StaleSession', 'stale_ledger_session');
    }
    return job;
  }

  cancel(id: string, scope?: Scope | null) {
    const job = this.get(id, scope);
    const accepted = job.status.cancellable;
    if (accepted) {
      if (job.control) Atomics.compareExchange(job.control, 0, 0, 1);
      job.cancel.abort();
    }
    return { id, cancelled: accepted || job.status.state === 'cancelled', state: job.status.state };
  }

  private cancelled(job: Job) {
    return job.cancel.signal.aborted || !!job.control && Atomics.load(job.control, 0) === 1;
  }

  private publish(job: Job, state: JobSnapshot_Serialize['state']) {
    const status = job.status;
    status.state = state;
    status.cancellable = state === 'queued' || state === 'running';
    status.seq++;
    this.emit({
      jobId: status.id, sessionId: status.sessionId, kind: status.kind,
      state, seq: status.seq, phase: state === 'running' ? job.phase : state,
      done: status.done ?? undefined, cancellable: status.cancellable
    });
  }
}

export type JobObserver = { cancelled(): boolean; publishing(): boolean; progress(done: number): void };
