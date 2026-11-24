<script lang="ts">
  import { goto } from '$app/navigation';
  import RowActions from '#lib/components/RowActions.svelte';
  import * as Popover from '#lib/components/ui/popover/index.js';
  import { Button } from '#lib/components/ui/button/index.js';
  import { Label } from '#lib/components/ui/label/index.js';
  import { Input } from '#lib/components/ui/input/index.js';
  import { Card } from '#lib/components/ui/card/index.js';
  import { Table, TableHeader, TableRow, TableHead, TableBody, TableCell } from '#lib/components/ui/table/index.js';
  import { NativeSelect } from '#lib/components/ui/native-select/index.js';
  import { NativeCheckbox } from '#lib/components/ui/checkbox/index.js';
  import { page } from '$app/state';
  import { commands, type Method, type Expense_Serialize, type Settlement_Serialize, type Member_Serialize } from '../../../lib/drivers/runtime';
  import { session, today } from '../../../lib/stores/session.svelte';
  import { privacy } from '../../../lib/stores/privacy.svelte';
  import { query, createTask } from '../../../lib/stores/task.svelte';
  import { minorToInput } from '../../../lib/money';
  import Heading from '../../../lib/components/Heading.svelte';
  import Feedback from '../../../lib/components/Feedback.svelte';
  import Modal from '../../../lib/components/Modal.svelte';
  import MoneyField from '../../../lib/components/MoneyField.svelte';
  import CurrencySelect from '../../../lib/components/CurrencySelect.svelte';
  import GroupSettings from '../../../lib/views/GroupSettings.svelte';
  const id=$derived(page.params.id??'');
  let asOf=$state(today());
  const data=query(()=>session.call(commands.groupGet(session.scope,{id,asOf})));
  const task=createTask();
  let mode=$state<'group'|'member'|'expense'|'settlement'|'remove'|null>(null);
  $effect(()=>{
    if (page.url.searchParams.get('edit')==='1') {
      mode='group';
      void goto(page.url.pathname,{replace:true,shallow:true});
    }
  });
  let deletion=$state<{kind:'member'|'expense'|'settlement';id:string}>();
  let target=$state('');
  let description=$state(''),currency=$state('USD'),amount=$state(''),date=$state(''),from=$state(''),to=$state(''),link=$state('');
  let method=$state<Method>('equal');
  let values=$state<Record<string,string>>({});
  let selected=$state<string[]>([]);
  const self=$derived(data.value?.members.find(row=>row.draft.isSelf));
  function label(id:string){return data.value?.members.find(row=>row.id===id)?.draft.label??'Member';}
  function decimal(minor:string){return minorToInput(minor,privacy.scale(currency)??0);}
  function member(row?:Member_Serialize){
    target=row?.id??'';description=row?.draft.label??'';mode='member';
  }
  function expense(row?:Expense_Serialize){
    target=row?.id??'';description=row?.description??'';date=row?.date??asOf;from=row?.payerId??self?.id??'';link=row?.transactionId??'';
    currency=row?.amount.currency??session.active?.baseCurrency??'IDR';amount=row?decimal(row.amount.minor):'';method=row?.method??'equal';
    selected=row?.shares.map(record=>record.memberId)??data.value?.members.map(record=>record.id)??[];
    values=Object.fromEntries(row?.shares.map(record=>[record.memberId,method==='exact'?decimal(record.minor):record.percentage??''])??[]);
    mode='expense';
  }
  function settlement(row?:Settlement_Serialize){
    target=row?.id??'';const draft=row?.draft;
    description=draft?.notes??'';date=draft?.date??asOf;from=draft?.fromMemberId??'';to=draft?.toMemberId??'';link=draft?.transactionId??'';
    currency=draft?.amount.currency??session.active?.baseCurrency??'IDR';amount=draft?decimal(draft.amount.minor):'';mode='settlement';
  }
  function remove(kind:'member'|'expense'|'settlement',key:string){deletion={kind,id:key};mode='remove';}
  function submit(event:SubmitEvent){
    event.preventDefault();void task.run(async()=>{
      const group=data.value?.group;
      if(!group)return;
      const previous=target?{id:target,version:group.version}:null;
      const mutation=session.mutation();
      if(mode==='member')await session.call(commands.groupMemberSave(mutation,{groupId:id,target:previous,draft:{label:description,isSelf:target===self?.id}}));
      if(mode==='expense')await session.call(commands.groupExpenseSave(mutation,{target:previous,draft:{
        groupId:id,payerId:from,date,description,transactionId:link||null,amount:privacy.amount(currency,amount),method,
        shares:selected.map(memberId=>({memberId,value:method==='exact'?privacy.amount(currency,values[memberId]??'').minor:method==='percentage'?values[memberId]??'':null}))
      }}));
      if(mode==='settlement')await session.call(commands.groupSettlementSave(mutation,{target:previous,draft:{groupId:id,fromMemberId:from,toMemberId:to,date,notes:description,amount:privacy.amount(currency,amount),transactionId:link||null}}));
      if(mode==='remove' && deletion){
        if(deletion.kind==='member')await session.call(commands.groupMemberRemove(mutation,{groupId:id,memberId:deletion.id}));
        else await session.call((deletion.kind==='expense'?commands.groupExpenseRemove:commands.groupSettlementRemove)(mutation,{id:deletion.id,confirm:true}));
      }
      mode=null;data.refresh();
    });
  }
</script>
<Heading title={data.value?.group.name??'Shared group'}>
  <Button variant="outline"  href="/groups">All groups</Button>
  <Label class="field">As of<Input type="date" bind:value={asOf}/></Label>
  {#if data.value && !data.value.group.archived}<Button variant="outline" onclick={()=>settlement()}>Pay back</Button><Button variant="default"  onclick={()=>expense()}>Split expense</Button>{/if}
</Heading>
<Feedback error={data.error || task.error} loading={data.loading}/>
{#if data.value}
  {@const archived=data.value.group.archived}
  <div class="mb-5 grid gap-4 lg:grid-cols-2">
    <Card as="section" ><h2 class="section-title">Balances</h2><p class="text-sm text-muted-foreground">Positive balances are owed to the member. Negative balances are owed by the member.</p>
      <div class="table-scroll mt-2"><Table contained={false}><TableHeader><TableRow><TableHead>Member</TableHead><TableHead>Balance by currency</TableHead></TableRow></TableHeader><TableBody>
        {#each data.value.members as row (row.id)}
          <TableRow><TableHead scope="row">{row.draft.label}</TableHead><TableCell>{#each data.value.balances.filter(value=>value.memberId===row.id) as balance (balance.amount.currency)}<div class="numeric">{privacy.money(balance.amount)}</div>{:else}—{/each}</TableCell>
          </TableRow>
        {/each}
      </TableBody></Table></div>
    </Card>
    <Card as="section" ><div class="flex flex-wrap items-center justify-between gap-2"><h2 class="section-title">Members</h2><div class="flex flex-wrap gap-2">
      {#if !archived}<Button variant="outline" onclick={()=>member()}>Add member</Button>{/if}<Button variant="outline" onclick={()=>mode='group'}>Edit group</Button>
      <Button variant="outline" disabled={task.busy} onclick={()=>void task.run(async()=>{await session.call(commands.groupArchive(session.mutation(),{id,archived:!archived}));data.refresh();})}>{archived?'Restore group':'Archive group'}</Button>
    </div></div>
    <ul class="mt-3 divide-y divide-line">{#each data.value.members as row (row.id)}<li class="flex flex-wrap items-center justify-between gap-2 py-2"><span>{row.draft.label}{row.draft.isSelf?' · You':''}</span>{#if !archived}<RowActions itemName={row.draft.label} onEdit={()=>member(row)} deleteLabel="Remove"
        onDelete={row.draft.isSelf ? undefined : ()=>remove('member',row.id)}/>{/if}</li>{/each}</ul>
    </Card>
  </div>
  <Card as="section" class="mb-5"><h2 class="section-title mb-3">Expenses</h2><div class="table-scroll"><Table contained={false}><TableHeader><TableRow><TableHead>Date</TableHead><TableHead>Expense</TableHead><TableHead>Paid by</TableHead><TableHead>Amount</TableHead><TableHead>Shares</TableHead><TableHead class="w-px whitespace-nowrap text-right">Actions</TableHead></TableRow></TableHeader><TableBody>
    {#each data.value.expenses as row (row.id)}
      <TableRow><TableCell>{privacy.date(row.date)}</TableCell><TableHead scope="row">{#if row.transactionId}<a href={'/transactions/'+row.transactionId}>{row.description}</a>{:else}{row.description}{/if}</TableHead><TableCell>{label(row.payerId)}</TableCell><TableCell class="numeric">{privacy.money(row.amount)}</TableCell><TableCell><Popover.Root>
        <Popover.Trigger>
          {#snippet child({ props })}<Button {...props} variant="ghost" aria-label={'Allocation for '+row.description}>{row.method} · {row.shares.length}</Button>{/snippet}
        </Popover.Trigger>
        <Popover.Content align="start" class="max-w-[calc(100vw-2rem)]">
          <ul class="divide-y">{#each row.shares as share (share.memberId)}
            <li class="flex justify-between gap-2 py-2"><span>{label(share.memberId)}</span><span class="numeric">{privacy.money({currency:row.amount.currency,minor:share.minor})}</span></li>
          {/each}</ul>
        </Popover.Content>
      </Popover.Root></TableCell><TableCell class="text-right"><RowActions itemName={row.description} onEdit={archived ? undefined : ()=>expense(row)}
        onDelete={archived ? undefined : ()=>remove('expense',row.id)} extraActions={row.transactionId ?
          [{id:'transaction',label:'View transaction',onclick:()=>void goto('/transactions/'+row.transactionId)}] : []}/></TableCell></TableRow>
    {:else}<TableRow><TableCell colspan={6}>No expenses recorded.</TableCell></TableRow>{/each}
  </TableBody></Table></div></Card>
  <Card as="section" ><h2 class="section-title mb-3">Paybacks</h2><div class="table-scroll"><Table contained={false}><TableHeader><TableRow><TableHead>Date</TableHead><TableHead>From</TableHead><TableHead>To</TableHead><TableHead>Amount</TableHead><TableHead class="w-px whitespace-nowrap text-right">Actions</TableHead></TableRow></TableHeader><TableBody>
    {#each data.value.settlements as row (row.id)}
      <TableRow><TableCell>{privacy.date(row.draft.date)}</TableCell><TableCell>{label(row.draft.fromMemberId)}</TableCell><TableCell>{label(row.draft.toMemberId)}</TableCell><TableCell class="numeric">{privacy.money(row.draft.amount)}</TableCell>
      <TableCell class="text-right"><RowActions itemName={'payback on '+privacy.date(row.draft.date)} onEdit={archived ? undefined : ()=>settlement(row)}
        onDelete={archived ? undefined : ()=>remove('settlement',row.id)} extraActions={row.draft.transactionId ?
          [{id:'transaction',label:'View transaction',onclick:()=>void goto('/transactions/'+row.draft.transactionId)}] : []}/></TableCell></TableRow>
    {:else}<TableRow><TableCell colspan={5}>No paybacks yet.</TableCell></TableRow>{/each}
  </TableBody></Table></div></Card>
{/if}
{#if mode==='group' && data.value}<GroupSettings group={data.value.group} self={self?.draft.label} close={()=>mode=null} saved={()=>{mode=null;data.refresh();}}/>
{:else if mode}
  <Modal confirm={mode==='remove'} title={mode==='member'?'Group member':mode==='expense'?'Split expense':mode==='settlement'?'Pay back':'Remove '+deletion?.kind} close={()=>mode=null}>
    <form class="space-y-3" onsubmit={submit}>
      {#if mode==='remove'}<p>Remove this {deletion?.kind} from the group?</p>
      {:else if mode==='member'}<Label class="field">Name<Input bind:value={description} required/></Label>
      {:else}
        <div class="form-grid">
          <Label class="field">Date<Input type="date" bind:value={date} required/></Label>
          <Label class="field">{mode==='expense'?'Paid by':'From'}<NativeSelect bind:value={from} required><option value="">Select member</option>{#each data.value?.members??[] as row (row.id)}<option value={row.id}>{row.draft.label}</option>{/each}</NativeSelect></Label>
          {#if mode==='settlement'}<Label class="field">To<NativeSelect bind:value={to} required><option value="">Select member</option>{#each data.value?.members??[] as row (row.id)}<option value={row.id} disabled={row.id===from}>{row.draft.label}</option>{/each}</NativeSelect></Label>{/if}
          <CurrencySelect bind:value={currency}/><MoneyField bind:value={amount}/>
        </div>
        <Label class="field">{mode==='expense'?'Description':'Notes'}<Input bind:value={description} required={mode==='expense'}/></Label>
        <Label class="field">Linked transaction ID (optional)<Input bind:value={link}/></Label>
        {#if mode==='expense'}<Label class="field">Split method<NativeSelect bind:value={method} onchange={()=>values={}}><option value="equal">Equal</option><option value="percentage">Percentage</option><option value="exact">Exact amounts</option></NativeSelect></Label>
          <fieldset class="space-y-2"><legend class="section-title">Participants</legend>
            {#each data.value?.members??[] as row (row.id)}
              <div class="flex flex-wrap items-end gap-3"><Label class="check"><NativeCheckbox  checked={selected.includes(row.id)} onchange={event=>selected=event.currentTarget.checked?[...selected,row.id]:selected.filter(key=>key!==row.id)}/> {row.draft.label}</Label>
                {#if selected.includes(row.id) && method!=='equal'}<MoneyField label={row.draft.label+(method==='percentage'?' · Percent':' · Exact share')} bind:value={values[row.id]}/>{/if}</div>
            {/each}
          </fieldset>
        {/if}
      {/if}
      <Feedback error={task.error}/><Button variant="outline" type="submit" class={mode==='remove'?'danger':'primary'} disabled={task.busy || mode==='expense' && !selected.length}>{mode==='remove'?'Remove':'Save'}</Button>
    </form>
  </Modal>
{/if}
