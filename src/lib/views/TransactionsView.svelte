<script lang="ts">
  import TransactionFilters, { transactionSorts } from '#lib/components/TransactionFilters.svelte';
  import RowActions from '#lib/components/RowActions.svelte';
  import TransactionCategoryIcon from '#lib/icons/TransactionCategoryIcon.svelte';
  import { Button } from '#lib/components/ui/button/index.js';
  import { Input } from '#lib/components/ui/input/index.js';
  import { NativeSelect } from '#lib/components/ui/native-select/index.js';
  import { NativeCheckbox } from '#lib/components/ui/checkbox/index.js';
  import { Card } from '#lib/components/ui/card/index.js';
  import { Table, TableHeader, TableRow, TableHead, TableBody, TableCell } from '#lib/components/ui/table/index.js';
  import { Badge } from '#lib/components/ui/badge/index.js';
  import { page } from '$app/state';
  import { goto } from '$app/navigation';
  import { tick } from 'svelte';
  import { MediaQuery } from 'svelte/reactivity';
  import { commands, type LedgerFilter_Deserialize, type TxRow_Serialize, type BulkAction_Deserialize, type MutationPreview } from '../drivers/runtime';
  import { session, pageRequest, message, today } from '../stores/session.svelte';
  import { privacy } from '../stores/privacy.svelte';
  import { jobs } from '../stores/jobs.svelte';
  import { query, createTask } from '../stores/task.svelte';
  import Heading from '../components/Heading.svelte';
  import Feedback from '../components/Feedback.svelte';
  import Modal from '../components/Modal.svelte';
  import MobileTransactionRow from '../components/MobileTransactionRow.svelte';

  const mobile=new MediaQuery('(width < 48rem)');
  const requested=()=>page.url.searchParams;
  function initial():LedgerFilter_Deserialize{
    const params=requested();
    const basis=params.get('basis');
    const direction=params.get('direction');
    const field=transactionSorts.find(([key])=>key===params.get('sort'))?.[0]??'date';
    return {basis:basis==='bank'||basis==='effective'?basis:'reporting',accountIds:params.getAll('account'),categoryIds:params.getAll('category'),
      range:{from:params.get('from')||undefined,to:params.get('to')||undefined},pnlOnly:params.get('pnl')==='1',statementId:params.get('statement'),aliasIds:params.getAll('alias'),direction:direction==='debit'||direction==='credit'?direction:undefined,sort:{field,ascending:params.get('ascending')==='1'}};
  }
  let selecting=$state(false);
  let running=$state(false);
  const grouped=$derived.by(()=>{
    const groups: {date:string; rows:TxRow_Serialize[]}[]=[];
    for (const row of rows) {
      const date=filter.basis==='bank'?row.bankDate:filter.basis==='effective'?row.effectiveDate:row.reportingDate;
      const latest=groups.at(-1);
      if(latest?.date===date) latest.rows.push(row);
      else groups.push({date,rows:[row]});
    }
    return groups;
  });
  function dateLabel(date:string) {
    const current=today();
    if(date===current)return 'Today';
    const day=new Date(date+'T00:00:00Z');
    if(day.getTime()+86400000===Date.parse(current+'T00:00:00Z'))return 'Yesterday';
    return new Intl.DateTimeFormat(undefined,{weekday:'short',month:'short',day:'numeric',year:date.slice(0,4)===current.slice(0,4)?undefined:'numeric',timeZone:'UTC'}).format(day);
  }
  let filter=$state<LedgerFilter_Deserialize>(initial());
  const accounts=query(()=>session.call(commands.accountChoices(session.scope,{includeArchived:true})));
  const categories=query(()=>session.call(commands.categoriesList(session.scope,{page:pageRequest(null,200),includeArchived:true})));
  const payees=query(()=>session.call(commands.payeesList(session.scope,{page:pageRequest(null,200)})));
  const groups=query(()=>privacy.visible('groups')?session.call(commands.groupsList(session.scope,{page:pageRequest(null,200)})):Promise.resolve(null));
  const task=createTask();
  let rows=$state.raw<TxRow_Serialize[]>([]);
  let cursor=$state<string>();
  let scroller=$state<HTMLDivElement>();
  let y=$state(0);
  let revision='';
  let focused=$state<string|null>(null);
  let selected=$state<string[]>([]);
  let firstCursor=$state<string|undefined>();
  let previous=$state<(string|undefined)[]>([]);
  let loading=$state(false);
  let error=$state('');
  let request=0;
  const rowHeight=49;
  const first=$derived(Math.min(rows.length,Math.max(0,Math.floor(y/rowHeight)-7)));
  const last=$derived(Math.min(rows.length,first+30));
  const windowRows=$derived.by(()=>{
    const indices=rows.slice(first,last).map((_,index)=>first+index);
    const focusedIndex=focused?rows.findIndex(row=>row.id===focused):-1;
    if(focusedIndex!==-1 && (focusedIndex<first || focusedIndex>=last)) indices.push(focusedIndex);
    indices.sort((a,b)=>a-b);
    return indices.map((index,position)=>({row:rows[index],index,skipped:index-(indices[position-1]??-1)-1}));
  });
  const trailing=$derived(rows.length-(windowRows.at(-1)?.index??-1)-1);
  async function moveFocus(event:KeyboardEvent) {
    if(event.key!=='ArrowDown' && event.key!=='ArrowUp')return;
    if(!(event.target instanceof HTMLElement) || !focused)return;
    const index=rows.findIndex(row=>row.id===focused);
    const targetIndex=index+(event.key==='ArrowDown'?1:-1);
    if(targetIndex<0 || targetIndex>=rows.length)return;
    event.preventDefault();
    const next=rows[targetIndex];
    focused=next.id;
    await tick();
    const selector=event.target.tagName==='INPUT'?'input':'a';
    scroller?.querySelector<HTMLElement>('[data-row="'+CSS.escape(next.id)+'"] '+selector)?.focus();
  }
  let bulk=$state<BulkAction_Deserialize['kind']>('categorize');
  let actionValue=$state('');
  let draftPreview=$state<MutationPreview | null>(null);
  let notice=$state('');
  async function fetchPage(start?:string, append=false) {
    const serial=++request;
    loading=true;error='';
    try {
      const result=await session.call(commands.transactionsList(session.scope,{filter:$state.snapshot(filter), page:pageRequest(start,150),runningBalance:running}));
      if(serial!==request) return;
      if(append && revision!==result.revision) {
        selected=[];previous=[];focused=null;
        if(firstCursor)firstCursor=undefined;else void fetchPage();
        return;
      }
      rows=append?[...rows,...result.rows]:result.rows;
      cursor=result.nextCursor;
      revision=result.revision;
      if(!append){focused=null;y=0;scroller?.scrollTo(0,0);}
    } catch(cause) {if(serial===request)error=message(cause);}
    finally {if(serial===request)loading=false;}
  }
  $effect(()=>{
    JSON.stringify(filter);running;firstCursor;
    void fetchPage(firstCursor);
    return ()=>{request++;};
  });
  $effect(()=>{
    if(!mobile.current && y>0 && !loading && cursor && last>=rows.length-4 && rows.length<750) void fetchPage(cursor,true);
  });
  function commitFilter(next:LedgerFilter_Deserialize) {
    selected=[];previous=[];firstCursor=undefined;
    filter=next;
    const params=new URLSearchParams({basis:next.basis??'reporting',sort:next.sort?.field??'date'});
    if(next.sort?.ascending)params.set('ascending','1');
    if(next.statementId)params.set('statement',next.statementId);
    for(const id of next.aliasIds??[])params.append('alias',id);
    if(next.direction)params.set('direction',next.direction);
    for(const id of next.accountIds??[])params.append('account',id);
    for(const id of next.categoryIds??[])params.append('category',id);
    if(next.range?.from)params.set('from',next.range.from);
    if(next.range?.to)params.set('to',next.range.to);
    if(next.pnlOnly)params.set('pnl','1');
    void goto('/transactions?'+params,{shallow:true,replace:true});
  }
  function previewBulk(){
    void task.run(async()=>{
      let action:BulkAction_Deserialize;
      switch(bulk){
        case 'categorize':action={kind:bulk,categoryId:actionValue||null};break;
        case 'assignGroup':action={kind:bulk,groupId:actionValue};break;
        case 'addTag':case 'removeTag':action={kind:bulk,tag:actionValue};break;
        case 'flag':action={kind:bulk,flagged:true,dueDate:actionValue||undefined};break;
        case 'delete':action={kind:bulk};break;
      }
      draftPreview=await session.call(commands.transactionsBulkPreview(session.scope,{selection:{kind:'ids',ids:[...selected]},action}));
    });
  }
  function applyBulk(){
    const preview=draftPreview;
    if(!preview)return;
    void task.run(async()=>{
      const result=await session.call(commands.transactionsBulkApply({...session.mutation(),expectedRevision:preview.revision},{previewToken:preview.previewToken,confirm:true}));
      draftPreview=null;selected=[];notice=result.value.changedIds.length+' transactions updated.';
      await fetchPage(firstCursor);
    });
  }
  let deletingRow=$state<TxRow_Serialize>();
  function deleteRow() {
    if (!deletingRow) return;
    const {id,version}=deletingRow;
    void task.run(async()=>{
      await session.call(commands.transactionDelete(session.mutation(),{target:{id,version},confirm:true}));
      deletingRow=undefined;
      selected=selected.filter(selectedId=>selectedId!==id);
      await fetchPage(firstCursor);
    });
  }
  function exportCsv(rowId?: string){
    void task.run(async()=>{
      const [grant]=await session.call(commands.fileChoose({purpose:'transactionExport'}));
      if(!grant)return;
      const ticket=await session.call(commands.transactionsExportStart(session.scope,{saveGrant:grant.id,filter:$state.snapshot(filter),
        selection:rowId?{kind:'ids',ids:[rowId]}:selected.length?{kind:'ids',ids:[...selected]}:undefined}));
      const result=await jobs.result(ticket,'transactionsExport');
      notice=result.rowCount+' transactions exported.';
    });
  }
</script>
{#snippet rowActions(row: TxRow_Serialize)}
  <RowActions itemName={row.description} onEdit={()=>void goto('/transactions/'+row.id+'#transaction-editor')}
    onDelete={row.transferId || row.entryKind==='opening' ? undefined : ()=>deletingRow=row} extraActions={[
      {id:'details',label:'View details',onclick:()=>void goto('/transactions/'+row.id)},
      {id:'export',label:'Export CSV',disabled:task.busy,onclick:()=>exportCsv(row.id)}
    ]}/>
{/snippet}
<div class="hidden md:block"><Heading title="Transactions">
  <Button variant="outline" onclick={()=>exportCsv()} disabled={task.busy}>Export CSV</Button>
  <Button variant="outline"  href="/transactions/new?transfer=1">New transfer</Button>
  <Button variant="default"  href="/transactions/new">New transaction</Button>
</Heading></div>
<h1 class="sr-only md:hidden">Transactions</h1>
<TransactionFilters {filter} accounts={accounts.value} categories={categories.value?.rows} payees={payees.value?.rows}
  groups={groups.value?.rows} bind:running bind:selecting {commitFilter} clearSelection={()=>selected=[]} onError={value=>error=value} />
<Feedback error={error||task.error} loading={loading&&!rows.length}/>
{#if notice}<p role="status" class="mb-3 text-sm">{notice}</p>{/if}
{#if selected.length}
  <Card role="group" aria-label="Bulk transaction actions"
    class={['transaction-bulk mb-3 flex flex-wrap items-center gap-2',
      !mobile.current && 'liquid-glass-overlay fixed bottom-6 left-1/2 z-30 mb-0 w-max max-w-[calc(100%-2rem)] -translate-x-1/2 justify-center rounded-full bg-(--glass-fill) px-4 py-2 md:p-2 md:px-4']}>
    <span class="numeric text-sm">{selected.length} selected</span>
    <NativeSelect aria-label="Bulk action" class="max-w-44" bind:value={bulk} onchange={()=>actionValue=''}>
      <option value="categorize">Choose category</option><option value="addTag">Add tag</option><option value="removeTag">Remove tag</option><option value="flag">Flag</option>
      {#if privacy.visible('groups')}<option value="assignGroup">Assign group</option>{/if}<option value="delete">Delete</option>
    </NativeSelect>
    {#if bulk==='categorize'}<NativeSelect aria-label="Bulk category" class="max-w-44" bind:value={actionValue}><option value="">Clear category</option>{#each categories.value?.rows??[] as item (item.id)}<option value={item.id}>{item.draft.name}</option>{/each}</NativeSelect>
    {:else if bulk==='assignGroup'}<NativeSelect aria-label="Bulk group" class="max-w-44" bind:value={actionValue}><option value="">Choose group</option>{#each groups.value?.rows??[] as item (item.id)}<option value={item.id}>{item.name}</option>{/each}</NativeSelect>
    {:else if bulk!=='delete'}<Input aria-label={bulk==='flag'?'Flag due date':'Tag'} class="max-w-44" type={bulk==='flag'?'date':'text'} bind:value={actionValue}/>{/if}
    <Button variant="outline" onclick={previewBulk} disabled={task.busy}>Preview action</Button><Button variant="outline" onclick={()=>selected=[]}>Clear selection</Button>
  </Card>
{/if}
{#if mobile.current}
<div class="space-y-4 md:hidden">
  {#each grouped as group (group.rows[0].id)}
    <section aria-label={dateLabel(group.date)}>
      <h2 class="liquid-glass-bar glass-date sticky top-[calc(3.5rem+1px+var(--mobile-safe-top))] z-10 mb-2 rounded-lg px-2 py-1 text-sm font-semibold text-muted-foreground">{dateLabel(group.date)}</h2>
      <ul class="overflow-hidden rounded-xl border border-line bg-paper">
        {#each group.rows as row (row.id)}
          <MobileTransactionRow {row} {running} account={accounts.value?.find(item=>item.id===row.accountId)?.name}
            payee={payees.value?.rows.find(item=>item.id===row.payeeId)?.draft.name}
            selected={selected.includes(row.id)}
            select={selecting || selected.length ? checked=>selected=checked?[...selected,row.id]:selected.filter(id=>id!==row.id) : undefined}>
            {#snippet actions()}{@render rowActions(row)}{/snippet}
          </MobileTransactionRow>
        {/each}
      </ul>
    </section>
  {:else}
    {#if !loading}<Card as="p" class="text-muted-foreground">No matching transactions.</Card>{/if}
  {/each}
</div>
{:else}
<div class="table-wrap">
  <div class="overflow-auto max-h-[65dvh] min-h-64" bind:this={scroller} onscroll={event=>y=event.currentTarget.scrollTop}>
    <Table contained={false} class="w-full table-fixed">
      <TableHeader class="liquid-glass-bar sticky top-0 z-10 border-x-0 border-t-0"><TableRow><TableHead class="w-11"><span class="sr-only">Select</span></TableHead><TableHead class="w-24 lg:w-28">Date</TableHead><TableHead>Transaction</TableHead><TableHead class="w-24">Status</TableHead><TableHead class="w-24 text-right lg:w-32">Amount</TableHead>{#if running}<TableHead class="w-24 text-right lg:w-32">Balance</TableHead>{/if}<TableHead class="w-32 whitespace-nowrap text-right">Actions</TableHead></TableRow></TableHeader>
      <TableBody>
        {#each windowRows as {row,skipped} (row.id)}
          {#if skipped}<TableRow aria-hidden="true"><TableCell style={`height:${skipped*rowHeight}px`} colspan={running?7:6} class="border-none p-0" ></TableCell></TableRow>{/if}
          <TableRow style={`height:${rowHeight}px`} data-row={row.id} class="hover:bg-ground" >
            <TableCell class="py-0"><NativeCheckbox  onkeydown={moveFocus} onfocus={()=>focused=row.id} aria-label={'Select '+row.description} checked={selected.includes(row.id)} onchange={event=>selected=event.currentTarget.checked?[...selected,row.id]:selected.filter(id=>id!==row.id)}/></TableCell>
            <TableCell class="py-0">{privacy.date(filter.basis==='bank'?row.bankDate:filter.basis==='effective'?row.effectiveDate:row.reportingDate)}</TableCell>
            <TableCell class="py-0"><div class="flex min-w-0 items-center gap-2"><TransactionCategoryIcon {row}/><div class="min-w-0"><a href={'/transactions/'+row.id} onkeydown={moveFocus} onfocus={()=>focused=row.id} class="block truncate font-medium underline-offset-2 hover:underline">{row.description}</a><span class="block truncate text-xs text-muted-foreground">{[accounts.value?.find(item=>item.id===row.accountId)?.name,row.transferId?'Transfer':row.category?.name].filter(Boolean).join(' · ')}</span></div></div></TableCell>
            <TableCell class="py-0"><Badge variant="secondary" class={["capitalize", row.transferId ? "text-transfer" : row.status === "pending" ? "text-pending" : row.status === "projected" ? "text-info" : ""]}>{row.transferId?'Transfer':row.status}</Badge></TableCell>
            <TableCell class={["numeric py-0 text-right", !privacy.masked && (row.direction==='credit'?'text-money-in':'text-money-out')]}>{privacy.money(row.direction==='debit'?{...row.amount,minor:'-'+row.amount.minor}:row.amount)}</TableCell>
            {#if running}<TableCell class="numeric py-0 text-right">{privacy.money(row.runningBalance)}</TableCell>{/if}
            <TableCell class="py-0 text-right">{@render rowActions(row)}</TableCell>
          </TableRow>
        {:else}{#if !loading}<TableRow><TableCell colspan={running?7:6} class="text-muted-foreground">No matching transactions.</TableCell></TableRow>{/if}{/each}
        {#if trailing>0}<TableRow aria-hidden="true"><TableCell style={`height:${trailing*rowHeight}px`} colspan={running?7:6} class="border-none p-0" ></TableCell></TableRow>{/if}
      </TableBody>
    </Table>
  </div>
</div>
{/if}
<div class="mt-3 flex flex-wrap items-center gap-2 text-sm">
  <span class="mr-auto text-muted-foreground">{rows.length} loaded{loading?' · Loading…':''}</span>
  <Button variant="outline" disabled={!previous.length||loading} onclick={()=>{firstCursor=previous.pop();previous=[...previous];selected=[];}}>Previous batch</Button>
  {#if cursor && rows.length<750}<Button variant="outline" disabled={loading} onclick={()=>fetchPage(cursor,true)}>Load more</Button>{/if}
  <Button variant="outline" disabled={!cursor||loading} onclick={()=>{previous=[...previous,firstCursor];firstCursor=cursor;selected=[];}}>Next batch</Button>
</div>
<div class="mt-4 flex flex-wrap gap-2 md:hidden">
  <Button variant="outline" onclick={()=>exportCsv()} disabled={task.busy}>Export CSV</Button>
  <Button variant="outline"  href="/transactions/new?transfer=1">New transfer</Button>
</div>
{#if deletingRow}
  <Modal confirm title="Delete transaction?" close={()=>deletingRow=undefined}>
    <p>Delete {deletingRow.description} from this activity?</p>
    <Feedback error={task.error}/>
    <Button variant="destructive" class="mt-4" disabled={task.busy} onclick={deleteRow}>Confirm deletion</Button>
  </Modal>
{/if}
{#if draftPreview}
  <Modal confirm={bulk==='delete'} title="Review bulk action" close={()=>draftPreview=null}>
    <Feedback error={task.error}/>
    <ul class="max-h-64 overflow-auto divide-y divide-line">{#each draftPreview.rows as row (row.id)}<li class="py-2">{row.description}{#if row.blocked}<small class="ml-2 text-muted-foreground">{row.blocked.replaceAll('_',' ')}</small>{/if}</li>{/each}</ul>
    <Button variant="default" class="mt-4" onclick={applyBulk} disabled={task.busy || !draftPreview.rows.some(item=>!item.blocked)}>Confirm {bulk==='delete'?'deletion':'changes'}</Button>
  </Modal>
{/if}
