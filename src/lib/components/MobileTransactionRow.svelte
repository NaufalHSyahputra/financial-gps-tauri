<script lang="ts">
  import type { Snippet } from 'svelte';
  import { Label } from '#lib/components/ui/label/index.js';
  import { NativeCheckbox } from '#lib/components/ui/checkbox/index.js';
  import type { TxRow_Serialize } from '../drivers/runtime';
  import { privacy } from '../stores/privacy.svelte';
  import TransactionCategoryIcon from '#lib/icons/TransactionCategoryIcon.svelte';

  let { row, account = '', payee = '', running = false, selected, select, actions }: {
    row: TxRow_Serialize; account?: string; payee?: string;
    running?: boolean; selected?: boolean; select?: (checked: boolean) => void; actions?: Snippet;
  } = $props();
  const incoming = $derived(row.direction === 'credit');
  const title = $derived(payee || row.description);
  const classification = $derived(row.transferId ? 'Transfer' : row.category?.name || 'Uncategorized');
</script>

<li class="min-w-0 border-b border-line last:border-b-0">
  <div class="flex min-w-0 items-center gap-1">
  {#if select}
    <Label class="grid size-11 shrink-0 place-items-center">
      <NativeCheckbox  aria-label={'Select '+row.description} checked={selected}
        onchange={event=>select?.(event.currentTarget.checked)} />
    </Label>
  {/if}
  <a href={'/transactions/'+row.id} class={['grid min-h-14 min-w-0 flex-1 items-center gap-2 px-3 py-2 no-underline hover:bg-ground',
    actions ? 'grid-cols-[auto_minmax(0,1fr)] sm:grid-cols-[auto_minmax(0,1fr)_minmax(0,max-content)]'
      : 'grid-cols-[auto_minmax(0,1fr)_minmax(0,max-content)]']}>
    <TransactionCategoryIcon {row} size="md"/>
    <span class="min-w-0">
      <span class="block truncate text-sm font-semibold">{title}</span>
      <span class="block truncate text-xs text-muted-foreground">{[account,classification].filter(Boolean).join(' · ')}</span>
      {#if row.status!=='posted'}<span class={["block text-xs", row.status === "pending" ? "text-pending" : "text-info"]}>{row.status==='pending'?'Pending':'Expected'}</span>{/if}
    </span>
    <span class={actions ? 'col-start-2 min-w-0 wrap-anywhere text-left sm:col-start-3 sm:max-w-36 sm:text-right' : 'max-w-36 text-right'}>
      <span class="numeric block text-sm font-semibold" class:amount-mask={privacy.masked} class:text-money-in={incoming && !privacy.masked} class:text-money-out={!incoming && !privacy.masked}>
        {privacy.money(incoming?row.amount:{...row.amount,minor:'-'+row.amount.minor})}
      </span>
      {#if running && row.runningBalance}<span class="numeric block text-xs text-muted-foreground">{privacy.money(row.runningBalance)}</span>{/if}
    </span>
  </a>
  {#if actions}<div class="shrink-0 pr-2">{@render actions()}</div>{/if}
  </div>
</li>
