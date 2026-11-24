import type { DriverEvents, DriverResult, PathPurpose, Scope } from '../contracts';

export type Request =
  | { id: number; type: 'initialize'; sqlite: ArrayBuffer; engine: ArrayBuffer }
  | { id: number; type: 'invoke'; command: string; args: Record<string, unknown> }
  | { id: number; type: 'grant'; purpose: PathPurpose; file?: File; name: string };

export type EventName = keyof DriverEvents;
export type EventValue<K extends EventName> = Parameters<Parameters<DriverEvents[K]>[0]>[0];
export type Response =
  | { type: 'job-control'; id: string; scope?: Scope; control: SharedArrayBuffer }
  | { type: 'reply'; id: number; result: DriverResult<unknown> }
  | { [K in EventName]: { type: 'event'; name: K; value: EventValue<K> } }[EventName]
  | { type: 'file'; grant: string; name: string; bytes: Uint8Array<ArrayBuffer>; media: string; open?: boolean };
