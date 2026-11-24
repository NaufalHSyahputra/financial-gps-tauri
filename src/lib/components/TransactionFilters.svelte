<script lang="ts" module>
  import type { SortField } from '../drivers/runtime';
  export const transactionSorts: [SortField,string][]=[['date','Date'],['amount','Amount'],['description','Description'],['payee','Payee'],['category','Category'],['account','Account'],['type','Direction'],['status','Status'],['flagDueDate','Flag due date']];
</script>

<script lang="ts">
  import { tick, untrack } from 'svelte';
  import { MediaQuery } from 'svelte/reactivity';
  import { commands, type AccountChoice, type Category_Serialize, type Payee, type GroupSummary, type LedgerFilter_Deserialize, type DateBasis, type TxStatus, type Direction, type FlagState } from '../drivers/runtime';
  import { session, message } from '../stores/session.svelte';
  import { privacy } from '../stores/privacy.svelte';
  import { query } from '../stores/task.svelte';
  import { Button } from '#lib/components/ui/button/index.js';
  import { Badge } from '#lib/components/ui/badge/index.js';
  import { Input } from '#lib/components/ui/input/index.js';
  import { Label } from '#lib/components/ui/label/index.js';
  import { NativeSelect } from '#lib/components/ui/native-select/index.js';
  import { NativeCheckbox } from '#lib/components/ui/checkbox/index.js';
  import * as Popover from '#lib/components/ui/popover/index.js';
  import * as Select from '#lib/components/ui/select/index.js';
  import CategoryIcon from '#lib/icons/CategoryIcon.svelte';
  import Modal from './Modal.svelte';
  import MoneyField from './MoneyField.svelte';

  let { filter, accounts, categories, payees, groups, running = $bindable(false), selecting = $bindable(false), commitFilter, clearSelection, onError }: {
    filter: LedgerFilter_Deserialize; accounts?: AccountChoice[]; categories?: Category_Serialize[];
    payees?: Payee[]; groups?: GroupSummary[]; running: boolean; selecting: boolean;
    commitFilter: (next: LedgerFilter_Deserialize) => void; clearSelection: () => void;
    onError: (error: string) => void;
  } = $props();
  const mobile = new MediaQuery('(width < 48rem)');
  const initial = untrack(() => $state.snapshot(filter));
  let filterSheet=$state(false);
  let filterPopover=$state(false);
  const filterForm=$props.id();
  async function openFilters(field?:string){
    filterSheet=true;
    if (!field) return;
    await tick();
    document.getElementById(filterForm)?.querySelector<HTMLElement>('[name="'+field+'"]')?.focus();
  }
  function quick(value:Direction|''){
    direction=value;
    commitFilter({...filter,direction:value||undefined});
  }
  let text=$state('');
  let from=$state(initial.range?.from ?? '');
  let to=$state(initial.range?.to ?? '');
  let basis=$state<DateBasis>(initial.basis ?? 'reporting');
  let account=$state(initial.accountIds?.[0] ?? '');
  let category=$state(initial.categoryIds?.[0] ?? '');
  let status=$state<TxStatus|''>('');
  let direction=$state<Direction|''>(initial.direction??'');
  let sort=$state<SortField>(initial.sort?.field??'date');
  let ascending=$state(initial.sort?.ascending??false);
  let tags=$state('');
  let payee=$state('');
  let statement=$state(initial.statementId??'');
  let alias=$state(initial.aliasIds?.[0]??'');
  let flag=$state<FlagState|''>('');
  let transfer=$state<''|'yes'|'no'>('');
  let excluded=$state<''|'yes'|'no'>('');
  let entry=$state<''|'ordinary'|'opening'|'adjustment'>('');
  let unbilled=$state(false);
  let includeOpening=$state(true);
  let uncat=$state(false);
  let pnl=$state(initial.pnlOnly ?? false);
  let group=$state('');
  let minimum=$state('');
  let maximum=$state('');
  let currency=$state('');

  const statements=query(()=>account && accounts?.find(item=>item.id===account)?.kind==='creditCard'?
    session.call(commands.cardStatementsList(session.scope,{accountId:account,range:{}})):Promise.resolve([]));
  function apply(event?:SubmitEvent) {
    event?.preventDefault();
    try {
      const tri=(value:''|'yes'|'no')=>value===''?undefined:value==='yes';
      const bounds=currency&&(minimum||maximum)?{currency,minimum:minimum?privacy.amount(currency,minimum).minor:null,maximum:maximum?privacy.amount(currency,maximum).minor:null}:undefined;
      commitFilter({basis,range:{from:from||undefined,to:to||undefined},accountIds:account?[account]:[],categoryIds:category?[category]:[],
        text:text||undefined,status:status||undefined,direction:direction||undefined,sort:{field:sort,ascending},tags:tags.split(/[, ]+/).filter(Boolean),
        payeeIds:payee?[payee]:[],groupIds:group?[group]:[],aliasIds:alias?[alias]:[],statementId:statement||undefined,
        flagState:flag||undefined,transfer:tri(transfer),ignored:tri(excluded),unbilled,uncategorized:uncat,pnlOnly:pnl,
        entryKind:entry||undefined,includeOpening,nativeAmount:bounds});
      if (event) { filterSheet=false; filterPopover=false; }
    } catch(cause) {onError(message(cause));}
  }
</script>

<div class="mobile-scroll-strip -mx-4 mb-3 px-4 py-1 md:hidden" role="group" aria-label="Quick filters">
  {#each [['','All'],['debit','Expense'],['credit','Income']] as [value,label] (label)}
    <Button variant="ghost" type="button" class="mobile-chip" aria-pressed={(filter.direction??'')===value} onclick={()=>quick(value as Direction|'')}>{label}</Button>
  {/each}
  <Button variant="ghost" type="button" class="mobile-chip" onclick={()=>void openFilters('account')}>Account</Button>
  <Button variant="ghost" type="button" class="mobile-chip" onclick={()=>void openFilters('from')}>Date</Button>
  <Button variant="ghost" type="button" class="mobile-chip" onclick={()=>void openFilters()}>Filters</Button>
</div>
<div class="mb-3 flex flex-wrap items-center gap-2 md:hidden">
  <Button variant="outline"  type="button" onclick={()=>void openFilters('search')}>Search activity</Button>
  <Button variant="outline" type="button" aria-pressed={selecting} onclick={()=>{selecting=!selecting;if(!selecting)clearSelection();}}>{selecting?'Done selecting':'Select'}</Button>
</div>
{#snippet filters(mobile: boolean)}
<form id={mobile?filterForm:undefined} onsubmit={apply}>
  <div class="grid gap-3 sm:grid-cols-2">
    {#if mobile}<Label class="field">Search<Input type="search" name="search" bind:value={text} placeholder="Description, notes, person, or store"/></Label>{/if}
    <Label class="field">Account<NativeSelect name="account" bind:value={account} onchange={()=>{statement='';alias='';}}><option value="">All accounts</option>{#each accounts??[] as item (item.id)}<option value={item.id}>{item.name}</option>{/each}</NativeSelect></Label>
    <Label class="field">Date basis<NativeSelect bind:value={basis}><option value="reporting">Reporting date</option><option value="bank">Bank date</option><option value="effective">Effective date</option></NativeSelect></Label>
    <Label class="field">From<Input type="date" name="from" bind:value={from}/></Label>
    <Label class="field">Through<Input type="date" bind:value={to}/></Label>
  </div>
    <div class="mt-3 grid gap-3 sm:grid-cols-2">
      <Label class="field">Category<NativeSelect bind:value={category}><option value="">All</option>{#each categories??[] as item (item.id)}<option value={item.id}>{item.draft.name}</option>{/each}</NativeSelect></Label>
      <Label class="field">Person or store<NativeSelect bind:value={payee}><option value="">All</option>{#each payees??[] as item (item.id)}<option value={item.id}>{item.draft.name}</option>{/each}</NativeSelect></Label>
      {#if privacy.visible('groups')}<Label class="field">Shared group<NativeSelect bind:value={group}><option value="">All</option>{#each groups??[] as item (item.id)}<option value={item.id}>{item.name}</option>{/each}</NativeSelect></Label>{/if}
      <Label class="field">Status<NativeSelect bind:value={status}><option value="">All</option><option value="posted">Confirmed</option><option value="pending">Pending</option><option value="projected">Expected</option></NativeSelect></Label>
      <Label class="field">Direction<NativeSelect bind:value={direction}><option value="">Both</option><option value="debit">Money out</option><option value="credit">Money in</option></NativeSelect></Label>
      <Label class="field">Entry type<NativeSelect bind:value={entry}><option value="">All</option><option value="ordinary">Regular transaction</option><option value="opening">Starting balance</option><option value="adjustment">Adjustment</option></NativeSelect></Label>
      <Label class="field">Tags<Input bind:value={tags} placeholder="Comma-separated tags"/></Label>
      <Label class="field">Statement<NativeSelect bind:value={statement}><option value="">All</option>{#each statements.value??[] as item (item.id)}<option value={item.id}>{item.draft.closeOn}</option>{/each}</NativeSelect></Label>
      <Label class="field">Linked card alias ID<Input bind:value={alias}/></Label>
      <Label class="field">Flag<NativeSelect bind:value={flag}><option value="">Any</option><option value="flagged">Flagged</option><option value="overdue">Overdue</option><option value="dueWithinWeek">Due this week</option></NativeSelect></Label>
      <Label class="field">Transfer<NativeSelect bind:value={transfer}><option value="">Any</option><option value="yes">Linked</option><option value="no">Unlinked</option></NativeSelect></Label>
      <Label class="field">Ignored<NativeSelect bind:value={excluded}><option value="">Any</option><option value="yes">Only ignored</option><option value="no">Hide ignored</option></NativeSelect></Label>
      <Label class="field">Sort by<NativeSelect bind:value={sort}>{#each transactionSorts as [value,label] (value)}<option value={value}>{label}</option>{/each}</NativeSelect></Label>
      <Label class="field">Amount currency<NativeSelect bind:value={currency}><option value="">No amount filter</option>{#each privacy.currencies as item (item.code)}<option value={item.code}>{item.code}</option>{/each}</NativeSelect></Label>
      <MoneyField label="Minimum" bind:value={minimum} required={false}/><MoneyField label="Maximum" bind:value={maximum} required={false}/>
    </div>
    <div class="mt-3 flex flex-wrap gap-4">
      <Label class="check"><NativeCheckbox  bind:checked={unbilled}/> Only unbilled</Label>
      <Label class="check"><NativeCheckbox  bind:checked={uncat}/> Uncategorized only</Label>
      <Label class="check"><NativeCheckbox  bind:checked={pnl}/> Income and expenses only</Label>
      <Label class="check"><NativeCheckbox  bind:checked={includeOpening}/> Include opening balances</Label>
      <Label class="check"><NativeCheckbox  bind:checked={ascending}/> Ascending</Label>
    </div>
  <div class="mt-3 flex flex-wrap items-center gap-4">{#if !mobile}<Button variant="default" type="submit" >Apply filters</Button>{/if}<Label class="check"><NativeCheckbox  bind:checked={running}/> Running balance</Label></div>
</form>
{/snippet}
{#if filterSheet}
  <Modal title="Activity filters" close={()=>filterSheet=false} mobileOnly>
    {@render filters(true)}
    {#snippet footer()}
      <Button variant="default" type="submit" class="w-full" form={filterForm}>Apply filters</Button>
    {/snippet}
  </Modal>
{:else if !mobile.current}
  <div class="mb-3 flex flex-wrap items-center gap-2">
    <form class="flex min-w-0 flex-1 items-center gap-2" onsubmit={event=>{event.preventDefault();commitFilter({...filter,text:text.trim()||undefined});}}>
      <Input aria-label="Search" type="search" bind:value={text} placeholder="Search transactions"/>
      <Button variant="outline" type="submit">Apply search</Button>
    </form>
    <Select.Root type="single" value={filter.direction??'all'} onValueChange={value=>quick(value==='debit'||value==='credit'?value:'')}>
      <Select.Trigger aria-label="Transaction direction" class="min-h-9">
        {filter.direction==='debit'?'Money out':filter.direction==='credit'?'Money in':'All directions'}
      </Select.Trigger>
      <Select.Content>
        <Select.Item value="all" label="All directions">All directions</Select.Item>
        <Select.Item value="debit" label="Money out">Money out</Select.Item>
        <Select.Item value="credit" label="Money in">Money in</Select.Item>
      </Select.Content>
    </Select.Root>
    <Popover.Root bind:open={filterPopover}>
      <Popover.Trigger>
        {#snippet child({ props })}<Button {...props} variant="outline">Filters</Button>{/snippet}
      </Popover.Trigger>
      <Popover.Content align="end" class="max-h-[min(70dvh,var(--bits-popover-content-available-height))] w-[min(32rem,calc(100vw-2rem))] overflow-y-auto p-4">
        {@render filters(false)}
      </Popover.Content>
    </Popover.Root>
  </div>
  <div class="mb-3 flex flex-wrap gap-2 empty:hidden" aria-label="Active filters">
    {#if filter.text}<Badge variant="secondary">Search · {filter.text}</Badge>{/if}
    {#each filter.accountIds??[] as id (id)}
      <Badge variant="secondary">{accounts?.find(account=>account.id===id)?.name??'Account'}</Badge>
    {/each}
    {#if filter.direction}<Badge variant="secondary">{filter.direction==='credit'?'Money in':'Money out'}</Badge>{/if}
    {#if filter.statementId}<Badge variant="secondary">Statement</Badge>{/if}
    {#if filter.tags?.length}<Badge variant="secondary">Tags · {filter.tags.join(', ')}</Badge>{/if}
    {#if filter.range?.from || filter.range?.to}<Badge variant="secondary">{filter.range?.from??'Any date'} – {filter.range?.to??'Any date'}</Badge>{/if}
    {#if filter.status}<Badge variant="secondary" class="capitalize">{filter.status}</Badge>{/if}
    {#each filter.categoryIds??[] as id (id)}
      {@const item=categories?.find(item=>item.id===id)}
      <Badge variant="secondary"><CategoryIcon name={item?.draft.name} kind={item?.draft.kind} icon={item?.draft.icon} size="sm"/>{item?.draft.name??'Category'}</Badge>
    {/each}
  </div>
{/if}
