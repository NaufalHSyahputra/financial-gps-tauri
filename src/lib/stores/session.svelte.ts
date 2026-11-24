import { goto } from '$app/navigation';
import type { AppError, LedgerSession, LifecycleState, Mutation, Scope } from '../drivers/runtime';

export type Reply<T> = Promise<{status:'ok';data:T} | {status:'error';error:AppError}>;

class SessionStore {
  active = $state<LedgerSession | null>(null);
  lifecycle = $state<LifecycleState>('unselected');
  epoch = $state(0);
  financialDateTick = $state(0);
  timezone = $state<string|null>(null);

  set(value: LedgerSession | null, lifecycle: LifecycleState = value ? 'ready' : 'unselected') {
    if (this.active?.scope.sessionId !== value?.scope.sessionId || this.lifecycle !== lifecycle) {
      this.epoch++;
      this.timezone = null;
    }
    this.active = value;
    this.lifecycle = lifecycle;
  }

  get scope(): Scope {
    if (!this.active) throw new Error('Open your activity to continue.');
    return { ...this.active.scope };
  }

  mutation(): Mutation {
    return { scope: this.scope, requestId: crypto.randomUUID() };
  }

  async call<T>(request: Reply<T>, scoped = true): Promise<T> {
    const epoch = this.epoch;
    const result = await request;
    if (scoped && epoch !== this.epoch) throw new Error('You switched to another activity.');
    if (result.status === 'error') {
      if (result.error.code === 'StaleSession' || (scoped && result.error.code === 'Locked')) {
        this.set(null, 'locked');
        void goto('/');
      }
      throw result.error;
    }
    return result.data;
  }
}

export const session = new SessionStore();
export function message(error: unknown) {
  if (error instanceof Error) return error.message;
  if (error && typeof error === 'object' && 'messageKey' in error) {
    const source = error as AppError;
    const friendly: Record<string, string> = {
      feature_unavailable: 'This feature is paused.',
      custom_backup_folder_unavailable: 'Use app storage for scheduled backups on this device.',
      unsupported_currency: 'Choose a supported currency.',
      same_currency_accounts_required: 'Choose two accounts in the same currency.',
      currency_mismatch: 'Choose amounts in the account’s currency.',
      invalid_date: 'Enter a valid date.',
      invalid_balance_change: 'Check the amount and enter a reason if needed.',
      invalid_splits: 'Check that the parts add up to the transaction amount.',
      active_group_with_self_required: 'Choose a group that includes you.',
      account_has_history: 'This account already has activity. Keep its current type and currency.',
      account_has_history_archive_instead: 'Archive this account to hide it from the account list.',
      stale_ledger_revision: 'Your activity changed. Refresh and try again.',
      financial_calendar_required: 'Set up a payday calendar first.',
      financial_calendar_history_unavailable: 'This date precedes the available payday calendar.',
      cycle_budget_category_unavailable: 'Each expense category can belong to only one budget per cycle.',
      cycle_budget_name_already_used: 'A budget with this name already exists in the cycle.',
      cash_plan_posting_ineligible: 'Choose an eligible posted IDR transaction from a liquid account.',
      cash_plan_posting_already_linked: 'That transaction is already linked to a plan entry.',
      posting_has_audited_dependencies: 'Review the linked financial records before changing this transaction.',
      posting_correction_requires_ordinary_posted: 'Only ordinary posted transactions can use this correction.',
      posting_requires_specialized_correction: 'This transaction needs its original workflow for corrections.',
      refund_requires_unclassified_credit: 'Choose an uncategorized posted credit to record a refund.',
      refund_original_ineligible: 'Choose an earlier eligible purchase in the same currency.',
      refund_exceeds_purchase: 'The combined refunds exceed the original purchase.',
      transfer_fee_requires_review: 'Review the transfer and its fee together before changing either.',
      opening_snapshot_same_day_ordering_required: 'An existing posting shares this date. Choose a snapshot date with clear ordering.',
      opening_snapshot_future_date: 'Choose today or an earlier date for the snapshot.',
      staged_occurrence_amount_changed: 'This expected transaction uses a previous amount. Review the current recurring amount before posting.',
      fee_category_required: 'Choose an expense category for the transfer fee.',
      opening_snapshot_requires_review: 'Review the opening snapshot before changing it.',
      cash_plan_stage_rule_ineligible: 'Choose a recurring IDR expense without category splits.',
      cash_plan_stage_overlaps: 'Forecast stages must have non-overlapping dates.',
      cash_plan_stage_invalid_range: 'End the stage on or after its start date.',
      historical_card_checkpoint_required: 'A historical card checkpoint is required to correct this opening balance.',
      invalid_payday_anchor: 'Choose a payday from 1 to 31.',
      optional_module_disabled: 'Turn on this feature in Settings to use it.',
    };
    const known = [source.messageKey, ...Object.values(source.fields ?? {})].find(key => friendly[key]);
    if (known) return friendly[known];
    const byCode: Partial<Record<AppError['code'], string>> = {
      Validation: 'Check the fields and try again.',
      Conflict: 'Refresh the current page and try again.',
      Busy: 'Another task is running. Try again in a moment.',
      StaleRevision: 'Your activity changed. Refresh and try again.',
      StaleSession: 'Open your activity again and retry.',
      Locked: 'Unlock your activity to continue.',
      Integrity: 'We couldn’t verify the activity file.',
      Offline: 'Check your connection and try again.',
      BadCredentialOrCorrupt: 'Check the passphrase and verify that the file opens.',
      Cancelled: 'The task was cancelled.',
    };
    return byCode[source.code] ?? `Something went wrong. Reference ${source.correlationId}.`;
  }
  return 'The operation could not be completed.';
}
export const pageRequest = (cursor?: string | null, limit = 100) => ({ cursor, limit });
export function today() {
  const date = new Date();
  if (session.timezone) {
    const parts = new Intl.DateTimeFormat('en-CA', { timeZone: session.timezone, year: 'numeric', month: '2-digit', day: '2-digit' }).formatToParts(date);
    return ['year', 'month', 'day'].map(type => parts.find(part => part.type === type)?.value).join('-');
  }
  return `${date.getFullYear()}-${String(date.getMonth()+1).padStart(2,'0')}-${String(date.getDate()).padStart(2,'0')}`;
}
