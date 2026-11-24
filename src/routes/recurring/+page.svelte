<script lang="ts">
  const modalForm = $props.id();
  import { goto } from '$app/navigation';
  import RowActions from '#lib/components/RowActions.svelte';
  import { Button } from '#lib/components/ui/button/index.js';
  import { Card } from '#lib/components/ui/card/index.js';
  import { Table, TableHeader, TableRow, TableHead, TableBody, TableCell } from '#lib/components/ui/table/index.js';
  import { Label } from '#lib/components/ui/label/index.js';
  import { Input } from '#lib/components/ui/input/index.js';
  import { NativeSelect } from '#lib/components/ui/native-select/index.js';
  import { Textarea } from '#lib/components/ui/textarea/index.js';
  import { NativeCheckbox } from '#lib/components/ui/checkbox/index.js';
  import { commands, type RecurringDetail_Serialize, type RecurringDraft_Deserialize, type Occurrence_Serialize, type OccurrenceAction } from '../../lib/drivers/runtime';
  import { session, today, pageRequest } from '../../lib/stores/session.svelte';
  import { privacy } from '../../lib/stores/privacy.svelte';
  import { query, createTask } from '../../lib/stores/task.svelte';
  import { minorToInput } from '../../lib/money';
  import Heading from '../../lib/components/Heading.svelte';
  import Feedback from '../../lib/components/Feedback.svelte';
  import Modal from '../../lib/components/Modal.svelte';
  import MoneyField from '../../lib/components/MoneyField.svelte';
  function empty():RecurringDraft_Deserialize {return {accountId:'',amount:{currency:'',minor:'0'},direction:'debit',description:'',notes:'',frequency:'monthly',startsOn:today(),weekendPolicy:'none',active:true};}
  let draft=$state(empty());
  let amount=$state('');
  let splits=$state<{key:string;categoryId:string;amount:string}[]>([]);
  let edited=$state<RecurringDetail_Serialize>();
  let edit=$state(false);
  let archive=$state<RecurringDetail_Serialize>();
  let inspected=$state('');
  let from=$state(today().slice(0,7)+'-01');
  let to=$state(today().slice(0,4)+'-12-31');
  let action=$state<{row:Occurrence_Serialize;kind:OccurrenceAction}>();
  let matchId=$state('');
  let cursor=$state<string|null>(null);
  let occurrenceCursor=$state<string|null>(null);
  const list=query(()=>session.call(commands.recurringList(session.scope,{page:pageRequest(cursor,50)})));
  const accounts=query(()=>session.call(commands.accountsList(session.scope,{includeArchived:!!edited})));
  const categories=query(()=>session.call(commands.categoriesList(session.scope,{page:pageRequest(null,200)})));
  const payees=query(()=>session.call(commands.payeesList(session.scope,{page:pageRequest(null,200)})));
  const occurrences=query(async()=> inspected ? session.call(commands.recurringOccurrences(session.scope,{ruleId:inspected,range:{from,to},page:pageRequest(occurrenceCursor,50)})) : null);
  const task=createTask();
  function open(row?:RecurringDetail_Serialize) {
    edited=row; draft=row?structuredClone(row.draft):empty();
    const scale=privacy.scale(row?.draft.amount.currency??'')??0;
    splits=row?.draft.splits.map(split=>({key:crypto.randomUUID(),categoryId:split.categoryId,amount:minorToInput(split.minor,scale)}))??[];
    amount=row?minorToInput(row.draft.amount.minor,privacy.scale(row.draft.amount.currency)??0):'';
    edit=true;
  }
  function save(event:SubmitEvent){
    event.preventDefault(); void task.run(async()=>{
      const currency=accounts.value?.find(row=>row.id===draft.accountId)?.account.currencyCode;
      if(!currency) throw new Error('Select an account.');
      const total=privacy.amount(currency,amount);
      const categories=splits.map(row=>({categoryId:row.categoryId,minor:privacy.amount(currency,row.amount).minor}));
      if(categories.length && (categories.length<2 || categories.some(row=>BigInt(row.minor)<=0n) || categories.reduce((sum,row)=>sum+BigInt(row.minor),0n)!==BigInt(total.minor)))throw new Error('Use at least two positive splits that add up to the total amount.');
      await session.call(commands.recurringSave(session.mutation(),{target:edited?{id:edited.id,version:edited.version}:null,draft:{...draft,amount:total,endsOn:draft.endsOn||null,categoryId:categories.length?null:draft.categoryId||null,payeeId:draft.payeeId||null,splits:categories}}));
      edit=false;list.refresh();occurrences.refresh();
    });
  }
  function apply(event:SubmitEvent){
    event.preventDefault(); void task.run(async()=>{
      if(!action) return;
      await session.call(commands.recurringOccurrenceApply(session.mutation(),{ruleId:action.row.ruleId,nominalDate:action.row.nominalDate,action:action.kind,transactionId:action.kind==='match'?matchId:null}));
      action=undefined;occurrences.refresh();list.refresh();
    });
  }
</script>
<Heading title="Recurring" subtitle="Plan repeating transactions and check each one as it comes up."><Button variant="default"  onclick={()=>open()}>New schedule</Button></Heading>
<Feedback error={list.error || accounts.error} loading={list.loading}/>
<Card class="table-scroll"><Table contained={false}><TableHeader><TableRow><TableHead>Schedule</TableHead><TableHead>Frequency</TableHead><TableHead>Amount</TableHead><TableHead>Status</TableHead><TableHead class="w-px whitespace-nowrap text-right">Actions</TableHead></TableRow></TableHeader><TableBody>
  {#each list.value?.rows??[] as row (row.id)}
    <TableRow><TableHead scope="row">{row.draft.description}</TableHead><TableCell>{row.draft.frequency}</TableCell><TableCell class="numeric">{privacy.money(row.draft.amount)}</TableCell><TableCell>{row.draft.active?'Active':'Inactive'}</TableCell><TableCell class="text-right"><RowActions itemName={row.draft.description} onEdit={()=>open(row)} extraActions={[
      {id:'occurrences',label:'Occurrences',onclick:()=>{inspected=row.id;occurrenceCursor=null;}},
      ...(row.draft.active ? [{id:'archive',label:'Archive',destructive:true,onclick:()=>archive=row}] : [])
    ]}/></TableCell></TableRow>
  {:else}<TableRow><TableCell colspan={5}>No schedules found.</TableCell></TableRow>{/each}
</TableBody></Table></Card>
{#if cursor}<Button variant="outline" class="mt-3" onclick={()=>cursor=null}>First page</Button>{/if}
{#if list.value?.nextCursor}<Button variant="outline" class="mt-3" onclick={()=>cursor=list.value?.nextCursor??null}>Next page</Button>{/if}
{#if inspected}<Card as="section" class="mt-5">
  <h2 class="section-title">Occurrences · {list.value?.rows.find(row=>row.id===inspected)?.draft.description??'Selected schedule'}</h2>
  <div class="my-3 flex flex-wrap items-end gap-3"><Label class="field">From<Input type="date" bind:value={from} onchange={()=>occurrenceCursor=null}/></Label><Label class="field">To<Input type="date" bind:value={to} onchange={()=>occurrenceCursor=null}/></Label></div>
  <Feedback error={occurrences.error} loading={occurrences.loading}/>
  <div class="table-scroll"><Table contained={false}><TableHeader><TableRow><TableHead>Original date</TableHead><TableHead>Scheduled date</TableHead><TableHead>Status</TableHead><TableHead class="w-px whitespace-nowrap text-right">Actions</TableHead></TableRow></TableHeader><TableBody>
    {#each occurrences.value?.rows??[] as row (row.nominalDate)}
      <TableRow><TableCell>{privacy.date(row.nominalDate)}</TableCell><TableCell>{privacy.date(row.scheduledDate)}</TableCell><TableCell>{row.state}</TableCell><TableCell class="text-right"><RowActions itemName={privacy.date(row.scheduledDate)} extraActions={[
        ...(row.transactionId ? [{id:'transaction',label:'View transaction',onclick:()=>void goto('/transactions/'+row.transactionId)}] : []),
        ...(row.state==='virtual'?['generate','post','skip','match']:row.state==='matched'?['unmatch']:row.state==='generated'?['post','skip','match']:['match'])
          .filter(kind=>kind!=='post'||row.scheduledDate<=today()).map(kind=>({
            id:kind,label:kind.charAt(0).toUpperCase()+kind.slice(1),
            onclick:()=>{matchId='';action={row,kind:kind as OccurrenceAction};}
          }))
      ]}/></TableCell></TableRow>
    {:else}<TableRow><TableCell colspan={4}>Nothing scheduled for these dates.</TableCell></TableRow>{/each}
  </TableBody></Table></div>
  {#if occurrenceCursor}<Button variant="outline" class="mt-3" onclick={()=>occurrenceCursor=null}>First page</Button>{/if}
  {#if occurrences.value?.nextCursor}<Button variant="outline" class="mt-3" onclick={()=>occurrenceCursor=occurrences.value?.nextCursor??null}>Next occurrences</Button>{/if}
</Card>{/if}
{#if edit}<Modal title={edited?'Edit schedule':'New schedule'} close={()=>edit=false}><form id={modalForm + '-schedule'} class="space-y-3" onsubmit={save}>
  <Label class="field">Description<Input bind:value={draft.description} required/></Label>
  <div class="form-grid"><Label class="field">Account<NativeSelect bind:value={draft.accountId} required><option value="">Select account</option>{#each accounts.value??[] as row (row.id)}<option value={row.id}>{row.account.name} · {row.account.currencyCode}</option>{/each}</NativeSelect></Label><MoneyField bind:value={amount}/></div>
  <div class="form-grid"><Label class="field">Direction<NativeSelect bind:value={draft.direction}><option value="debit">Money out</option><option value="credit">Money in</option></NativeSelect></Label>
  <Label class="field">Frequency<NativeSelect bind:value={draft.frequency}>{#each ['weekly','biweekly','monthly','quarterly','semiannual','yearly'] as value}<option {value}>{value}</option>{/each}</NativeSelect></Label>
  <Label class="field">Starts on<Input type="date" bind:value={draft.startsOn} required/></Label><Label class="field">Ends on<Input type="date" bind:value={draft.endsOn}/></Label>
  <Label class="field">Day of the month<Input type="number" min={1} max={31} bind:value={draft.anchorDay}/></Label><Label class="field">Maximum repeats<Input type="number" min={1} bind:value={draft.countLimit}/></Label>
  <Label class="field">Weekend adjustment<NativeSelect bind:value={draft.weekendPolicy}><option value="none">None</option><option value="previousFriday">Previous Friday</option><option value="nextMonday">Next Monday</option></NativeSelect></Label>
  <Label class="field">Person or store<NativeSelect bind:value={draft.payeeId}><option value="">No person or store</option>{#each payees.value?.rows??[] as row (row.id)}<option value={row.id}>{row.draft.name}</option>{/each}</NativeSelect></Label>
  <Label class="field">Category<NativeSelect bind:value={draft.categoryId} disabled={splits.length>0}><option value="">Uncategorized</option>{#each categories.value?.rows??[] as row (row.id)}<option value={row.id}>{row.draft.name}</option>{/each}</NativeSelect></Label></div>
  <fieldset class="space-y-2"><legend class="section-title">Category splits</legend>
    {#each splits as split (split.key)}<div class="flex flex-wrap items-end gap-2">
      <Label class="field">Category<NativeSelect bind:value={split.categoryId} required><option value="">Choose category</option>{#each categories.value?.rows??[] as row (row.id)}<option value={row.id}>{row.draft.name}</option>{/each}</NativeSelect></Label>
      <MoneyField label="Split amount" bind:value={split.amount}/><RowActions deleteLabel="Remove split" onDelete={()=>splits=splits.filter(row=>row.key!==split.key)}/>
    </div>{/each}
    <Button variant="outline" type="button" onclick={()=>splits=[...splits,{key:crypto.randomUUID(),categoryId:'',amount:''}]}>Add split</Button>
  </fieldset>
  <Label class="field">Notes<Textarea bind:value={draft.notes}></Textarea></Label>
  <div class="flex flex-wrap gap-4"><Label class="check"><NativeCheckbox  bind:checked={draft.active}/> Active</Label><Label class="check"><NativeCheckbox  bind:checked={draft.skipFirst}/> Skip first</Label><Label class="check"><NativeCheckbox  bind:checked={draft.autoGenerate}/> Generate automatically</Label></div>
  {#if edited?.generationFloor}<p class="text-sm text-muted-foreground">Generated through {privacy.date(edited.generationFloor)}.</p>{/if}
  <Feedback error={task.error || categories.error || payees.error}/>
</form>
  {#snippet footer()}
    <Button variant="default" type="submit" form={modalForm + '-schedule'}  disabled={task.busy}>Save schedule</Button>
  {/snippet}
</Modal>{/if}
{#if action}<Modal title={'Occurrence · '+action.kind} close={()=>action=undefined}><form id={modalForm + '-occurrence'} class="space-y-3" onsubmit={apply}><p>{privacy.date(action.row.scheduledDate)}</p>{#if action.kind==='match'}<Label class="field">Transaction ID<Input bind:value={matchId} required/></Label>{/if}<Feedback error={task.error}/></form>

  {#snippet footer()}
    <Button variant="default" type="submit" form={modalForm + '-occurrence'}  disabled={task.busy}>Confirm {action?.kind}</Button>
  {/snippet}
</Modal>{/if}
{#if archive}<Modal title="Archive schedule" close={()=>archive=undefined}><p>Archive {archive.draft.description}?</p><Feedback error={task.error}/>

  {#snippet footer()}
    <Button variant="destructive" disabled={task.busy} onclick={()=>void task.run(async()=>{if(!archive)return;await session.call(commands.recurringArchive(session.mutation(),{id:archive.id}));archive=undefined;list.refresh();occurrences.refresh();})}>Archive schedule</Button>
  {/snippet}
</Modal>{/if}
