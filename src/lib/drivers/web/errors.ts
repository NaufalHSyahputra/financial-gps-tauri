import type { AppError_Serialize } from '../contracts';

export function issue(
  code: AppError_Serialize['code'],
  messageKey: string,
  fields: Record<string, string> | null = null
): AppError_Serialize {
  return {
    code, messageKey, fields, correlationId: crypto.randomUUID(),
    retryable: ['Busy', 'Io', 'Offline', 'StaleSession', 'StaleRevision', 'InsufficientSpace', 'BadCredentialOrCorrupt'].includes(code)
  };
}

export function asError(value: unknown): AppError_Serialize {
  if (typeof value === 'string') {
    try { value = JSON.parse(value); } catch { return issue('Integrity', 'database_integrity_error'); }
  }
  if (typeof value === 'object' && value && 'messageKey' in value && 'code' in value) {
    return value as AppError_Serialize;
  }
  if (value instanceof DOMException && value.name === 'QuotaExceededError') {
    return issue('InsufficientSpace', 'storage_full');
  }
  return issue('Io', 'browser_storage_operation_failed');
}
