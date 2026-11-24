<script lang="ts">
  import RowActions from '#lib/components/RowActions.svelte';
  import CategoryIcon from '#lib/icons/CategoryIcon.svelte';
  import { Progress } from '#lib/components/ui/progress/index.js';
  import { Label } from '#lib/components/ui/label/index.js';
  import { Input } from '#lib/components/ui/input/index.js';
  import { Button } from '#lib/components/ui/button/index.js';
  import { Card } from '#lib/components/ui/card/index.js';
  import { Badge } from '#lib/components/ui/badge/index.js';
  import { NativeSelect } from '#lib/components/ui/native-select/index.js';
  import { NativeCheckbox } from '#lib/components/ui/checkbox/index.js';
  import { commands, type Budget, type Suggestion } from '../../lib/drivers/runtime';
  import { session, today, pageRequest } from '../../lib/stores/session.svelte';
  import { privacy } from '../../lib/stores/privacy.svelte';
  import { query, createTask } from '../../lib/stores/task.svelte';
  import Heading from '../../lib/components/Heading.svelte';
  import Feedback from '../../lib/components/Feedback.svelte';
  import Modal from '../../lib/components/Modal.svelte';
  import MoneyField from '../../lib/components/MoneyField.svelte';
  import CurrencySelect from '../../lib/components/CurrencySelect.svelte';
  import { minorToInput } from '../../lib/money';

  let month = $state(today().slice(0,7));
  let mode = $state<'edit'|'copy'|'suggest'|'delete'|null>(null);
  let categoryId = $state('');
  let amount = $state('');
  let override = $state(true);
  let sourceMonth = $state('');
  let adjustment = $state('0');
  let overwrite = $state(false);
  let months = $state(3);
  let suggestions = $state<Suggestion[]>([]);
  const planner = query(()=>session.call(commands.budgetsGet(session.scope,{month})));
  const categories = query(()=>session.call(commands.categoriesList(session.scope,{page:pageRequest(null,200)})));
  const task = createTask();
  let currency = $state('USD');
  function edit(row?:Budget) {
    categoryId=row?.categoryId??''; override=row?.monthlyOverride??true;currency=row?.limit.currency??session.active?.baseCurrency??'IDR';
    amount=row?minorToInput(row.limit.minor,privacy.scale(row.limit.currency)??0):'';
    mode='edit';
  }
  function action(event:SubmitEvent) {
    event.preventDefault();
    void task.run(async()=>{
      if(mode==='edit') await session.call(commands.budgetsSave(session.mutation(),{entries:[{key:{categoryId,month:override?month:null},amount:privacy.amount(currency,amount)}],overwrite:true}));
      if(mode==='delete') await session.call(commands.budgetsDelete(session.mutation(),{keys:[{categoryId,month:override?month:null}]}));
      if(mode==='copy') await session.call(commands.budgetsCopy(session.mutation(),{fromMonth:sourceMonth,toMonth:month,adjustPercent:adjustment,overwrite}));
      if(mode==='suggest') {
        suggestions=await session.call(commands.budgetSuggestions(session.scope,{months}));
        return;
      }
      mode=null; planner.refresh();
    });
  }
  function bar(minor:string, limit:string) {
    if (BigInt(limit)<=0n) return 0;
    const ratio=BigInt(minor)*100n/BigInt(limit);
    return Number(ratio<0n?0n:ratio>100n?100n:ratio);
  }
</script>
<Heading title="Budgets" subtitle="Plan spending against actual and projected expenses.">
  <Label class="field">Month<Input type="month" bind:value={month} required/></Label>
  <Button variant="outline" onclick={()=>{sourceMonth='';adjustment='0';overwrite=false;mode='copy';}}>Copy month</Button>
  <Button variant="outline" onclick={()=>{suggestions=[];mode='suggest';}}>Historical suggestions</Button>
  <Button variant="default"  onclick={()=>edit()}>Set limit</Button>
</Heading>
<Feedback error={planner.error || categories.error} loading={planner.loading}/>
<div class="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
  {#each planner.value?.rows??[] as row (row.categoryId)}
    <Card as="section" >
      <div class="flex flex-wrap items-center justify-between gap-2"><h2 class="section-title flex items-center gap-2"><CategoryIcon name={row.name} icon={row.icon} size="sm"/><span class="break-words">{row.name}</span></h2><Badge variant="secondary" >{row.monthlyOverride?'Month override':'Default'}</Badge></div>
      <dl class="my-4 grid grid-cols-2 gap-3">
        <div><dt class="text-muted-foreground">Limit</dt><dd class="numeric">{privacy.money(row.limit)}</dd></div>
        <div><dt class="text-muted-foreground">Remaining</dt><dd class="numeric">{privacy.money(row.remaining)}</dd></div>
        <div><dt class="text-muted-foreground">Actual</dt><dd class="numeric">{privacy.money(row.actual)}</dd></div>
        <div><dt class="text-muted-foreground">Expected</dt><dd class="numeric">{privacy.money(row.projected)}</dd></div>
      </dl>
      {#if !privacy.masked}
        <Progress class="mb-2 h-1.5" value={bar(row.actual.minor,row.limit.minor)} aria-label="Actual spending"/>
        <Progress class="mb-4 h-1.5 [&_[data-slot=progress-indicator]]:bg-muted-foreground" value={bar(row.projected.minor,row.limit.minor)} aria-label="Expected spending"/>
      {/if}
      <div class="flex flex-wrap items-center gap-2">
        {#if !privacy.masked && row.carryoverEligible}<Badge variant="secondary" >Positive carryover eligible</Badge>{/if}
        {#if row.missingRates!=='0'}<Badge variant="secondary" >Some transactions use a different currency.</Badge>{/if}
        <div class="ml-auto"><RowActions itemName={row.name} onEdit={()=>edit(row)} deleteLabel="Remove limit for"
          onDelete={()=>{categoryId=row.categoryId;override=row.monthlyOverride;mode='delete';}}/></div>
      </div>
    </Card>
  {:else}{#if !planner.loading}<Card as="p" class="">No limits for this month.</Card>{/if}{/each}
</div>
{#if mode}
<Modal confirm={mode==='delete'} title={mode==='edit'?'Set budget limit':mode==='copy'?'Copy budget month':mode==='delete'?'Remove limit':'Historical suggestions'} close={()=>mode=null}>
  <form class="space-y-3" onsubmit={action}>
    {#if mode==='edit'}
      <Label class="field">Category<NativeSelect bind:value={categoryId} required><option value="">Select a category</option>{#each categories.value?.rows.filter(row=>row.draft.kind==='expense'&&!row.draft.ignored)??[] as category (category.id)}<option value={category.id}>{category.draft.name}</option>{/each}</NativeSelect></Label>
      <div class="form-grid"><CurrencySelect bind:value={currency}/><MoneyField label="Limit" bind:value={amount}/></div>
      <Label class="check"><NativeCheckbox  bind:checked={override}/> Apply to {month} only</Label>
    {:else if mode==='copy'}
      <Label class="field">Copy from<Input type="month" bind:value={sourceMonth} required/></Label>
      <p class="text-sm text-muted-foreground">Destination month is {month}.</p>
      <MoneyField label="Adjustment percent" bind:value={adjustment}/>
      <Label class="check"><NativeCheckbox  bind:checked={overwrite}/> Replace existing month overrides</Label>
    {:else if mode==='delete'}
      <p>Remove the {override?'month override':'default limit'} for this category?</p>
    {:else}
      <Label class="field">Months of history<Input type="number" min={1} max={24} bind:value={months} required/></Label>
      <ul class="space-y-2">
        {#each suggestions as item (item.categoryId)}
          <li class="flex flex-wrap items-center justify-between gap-2"><span>{categories.value?.rows.find(row=>row.id===item.categoryId)?.draft.name??'Category'}</span><span class="numeric">{privacy.money(item.average)}</span><Button variant="outline" type="button" onclick={()=>{categoryId=item.categoryId;override=true;currency=item.average.currency;amount=minorToInput(item.average.minor,privacy.scale(item.average.currency)??0);mode='edit';}}>Use suggestion</Button></li>
        {/each}
      </ul>
    {/if}
    <Feedback error={task.error}/><Button variant="default" type="submit"  disabled={task.busy}>{mode==='suggest'?'Calculate suggestions':mode==='delete'?'Remove limit':'Save'}</Button>
  </form>
</Modal>
{/if}
