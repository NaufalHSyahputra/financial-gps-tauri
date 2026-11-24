<script lang="ts">
  const modalForm = $props.id();
  import RowActions from '#lib/components/RowActions.svelte';
  import { Progress } from '#lib/components/ui/progress/index.js';
  import { Label } from '#lib/components/ui/label/index.js';
  import { Input } from '#lib/components/ui/input/index.js';
  import { Button } from '#lib/components/ui/button/index.js';
  import { Card } from '#lib/components/ui/card/index.js';
  import { Badge } from '#lib/components/ui/badge/index.js';
  import { NativeSelect } from '#lib/components/ui/native-select/index.js';
  import { commands, type Goal_Serialize, type Source, type StoredState } from '../../lib/drivers/runtime';
  import { session, today, pageRequest } from '../../lib/stores/session.svelte';
  import { privacy } from '../../lib/stores/privacy.svelte';
  import { query, createTask } from '../../lib/stores/task.svelte';
  import { minorToInput } from '../../lib/money';
  import Heading from '../../lib/components/Heading.svelte';
  import Feedback from '../../lib/components/Feedback.svelte';
  import Modal from '../../lib/components/Modal.svelte';
  import MoneyField from '../../lib/components/MoneyField.svelte';
  import CurrencySelect from '../../lib/components/CurrencySelect.svelte';
  let asOf=$state(today());
  let cursor=$state<string|null>(null);
  let edit=$state(false);
  let target=$state<Goal_Serialize>();
  let remove=$state<Goal_Serialize>();
  let name=$state(''), amount=$state(''), currency=$state('USD'), start=$state(''), end=$state(''), reference=$state(''), current=$state('');
  let source=$state<Source['kind']>('manual');
  let status=$state<StoredState>('active');
  const goals=query(()=>session.call(commands.goalsList(session.scope,{asOf,page:pageRequest(cursor,40)})));
  const refs=query(async()=>{
    if(!edit) return [];
    if(source==='account') return (await session.call(commands.accountsList(session.scope,{}))).map(row=>({id:row.id,name:row.account.name}));
    if(source==='asset') return (await session.call(commands.assetsList(session.scope,{page:pageRequest(null,200)}))).rows.map(row=>({id:row.id,name:row.draft.name}));
    if(source==='assetGroup') return (await session.call(commands.assetGroupsList(session.scope,{page:pageRequest(null,200)}))).rows.map(row=>({id:row.id,name:row.draft.name}));
    return [];
  });
  const task=createTask();
  function decimal(money:Goal_Serialize['current']) { return minorToInput(money.minor,privacy.scale(money.currency)??0); }
  function open(row?:Goal_Serialize){
    target=row; name=row?.draft.name??''; amount=row?decimal(row.draft.target):''; currency=row?.draft.target.currency??session.active?.baseCurrency??'IDR';
    start=row?.draft.startsOn??today(); end=row?.draft.targetOn??today(); status=row?.draft.state??'active';
    const prior=row?.draft.source; source=prior?.kind??'manual'; current=prior?.kind==='manual'?decimal(prior.current):'0';
    reference=prior?.kind==='account'?prior.accountId:prior?.kind==='asset'?prior.assetId:prior?.kind==='assetGroup'?prior.groupId:'';
    edit=true;
  }
  function save(event:SubmitEvent) {
    event.preventDefault(); void task.run(async()=>{
      const tracking:Source=source==='manual'?{kind:source,current:privacy.amount(currency,current)}:source==='account'?{kind:source,accountId:reference}:source==='asset'?{kind:source,assetId:reference}:source==='assetGroup'?{kind:source,groupId:reference}:{kind:source};
      await session.call(commands.goalSave(session.mutation(),{target:target?{id:target.id,version:target.version}:null,draft:{name,target:privacy.amount(currency,amount),startsOn:start,targetOn:end,source:tracking,state:status}}));
      edit=false;goals.refresh();
    });
  }
</script>
<Heading title="Goals"><Label class="field">As of<Input type="date" bind:value={asOf} onchange={()=>cursor=null}/></Label><Button variant="default"  onclick={()=>open()}>New goal</Button></Heading>
<Feedback error={goals.error} loading={goals.loading}/>
<div class="grid gap-3 md:grid-cols-2">
  {#each goals.value?.rows??[] as row (row.id)}
    <Card as="article" >
      <header class="flex items-center justify-between gap-2"><h2 class="section-title">{row.draft.name}</h2><Badge variant="secondary" >{row.state}</Badge></header>
      <p class="numeric my-3">{privacy.money(row.current)} <span class="text-muted-foreground">of</span> {privacy.money(row.draft.target)}</p>
      {#if !privacy.masked}
        <p class="text-sm">{row.progressPercent}% {row.pace? '· '+row.pace:''}</p>
        {@const whole=BigInt(/^-?\d+/.exec(row.progressPercent)?.[0]??'0')}
        <Progress class="my-2 h-2" value={Number(whole<0n?0n:whole>100n?100n:whole)} aria-label="Goal progress"/>
      {/if}
      <dl class="my-3 grid grid-cols-2 gap-2 text-sm"><div><dt class="text-muted-foreground">Monthly contribution</dt><dd>{privacy.money(row.monthlyContribution)}</dd></div><div><dt class="text-muted-foreground">Target date</dt><dd>{privacy.date(row.draft.targetOn)}</dd></div></dl>
      {#if row.missingRates!=='0'}<Badge variant="secondary" >Some amounts use a different currency.</Badge>{/if}
      <div class="flex justify-end"><RowActions itemName={row.draft.name} onEdit={()=>open(row)} onDelete={()=>remove=row}/></div>
    </Card>
  {:else}{#if !goals.loading}<Card as="p" class="">No goals found.</Card>{/if}{/each}
</div>
{#if cursor}<Button variant="outline" class="mt-3" onclick={()=>cursor=null}>First page</Button>{/if}
{#if goals.value?.nextCursor}<Button variant="outline" class="mt-3" onclick={()=>cursor=goals.value?.nextCursor??null}>Next page</Button>{/if}
{#if edit}<Modal title={target?'Edit goal':'New goal'} close={()=>edit=false}>
  <form id={modalForm + '-goal'} class="space-y-3" onsubmit={save}>
    <Label class="field">Name<Input bind:value={name} required/></Label>
    <div class="form-grid"><CurrencySelect bind:value={currency}/><MoneyField label="Target amount" bind:value={amount}/></div>
    <div class="form-grid"><Label class="field">Start date<Input type="date" bind:value={start} required/></Label><Label class="field">Target date<Input type="date" bind:value={end} required/></Label></div>
    <Label class="field">Tracking source<NativeSelect bind:value={source} onchange={()=>reference=''}><option value="manual">Manual</option><option value="account">Account</option><option value="asset">Asset</option><option value="assetGroup">Asset group</option><option value="netWorth">Net worth</option></NativeSelect></Label>
    {#if source==='manual'}<MoneyField label="Current amount" bind:value={current}/>
    {:else if source!=='netWorth'}<Label class="field">Source<NativeSelect bind:value={reference} required><option value="">Select a source</option>{#each refs.value??[] as row (row.id)}<option value={row.id}>{row.name}</option>{/each}</NativeSelect></Label>{/if}
    <Label class="field">Status<NativeSelect bind:value={status}><option>active</option><option>completed</option><option>paused</option><option>archived</option></NativeSelect></Label>
    <Feedback error={task.error || refs.error}/>
  </form>
  {#snippet footer()}
    <Button variant="default" type="submit" form={modalForm + '-goal'}  disabled={task.busy}>Save goal</Button>
  {/snippet}
</Modal>{/if}
{#if remove}<Modal confirm title="Delete goal" close={()=>remove=undefined}><p>Delete {remove.draft.name}?</p><Feedback error={task.error}/>

  {#snippet footer()}
    <Button variant="destructive" disabled={task.busy} onclick={()=>void task.run(async()=>{if(!remove)return;await session.call(commands.goalDelete(session.mutation(),{id:remove.id,confirm:true}));remove=undefined;goals.refresh();})}>Delete goal</Button>
  {/snippet}
</Modal>{/if}
