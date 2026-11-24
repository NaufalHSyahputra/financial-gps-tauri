<script lang="ts">
  import RowActions from '#lib/components/RowActions.svelte';
  import TransactionCategoryIcon from '#lib/icons/TransactionCategoryIcon.svelte';
  import { Button } from '#lib/components/ui/button/index.js';
  import { Card } from '#lib/components/ui/card/index.js';
  import { Badge } from '#lib/components/ui/badge/index.js';
  import { Label } from '#lib/components/ui/label/index.js';
  import { NativeSelect } from '#lib/components/ui/native-select/index.js';
  import { Input } from '#lib/components/ui/input/index.js';
  import { Textarea } from '#lib/components/ui/textarea/index.js';
  import { NativeCheckbox } from '#lib/components/ui/checkbox/index.js';
  import { goto } from '$app/navigation';
  import { page } from '$app/state';
  import { commands, type Direction, type TxDraft_Deserialize, type TxStatus, type TxDetail_Serialize, type InstallmentPreview, type InstallmentScope, type TransferCandidate } from '../drivers/runtime';
  import { session, today, pageRequest } from '../stores/session.svelte';
  import { query, createTask } from '../stores/task.svelte';
  import { privacy } from '../stores/privacy.svelte';
  import { jobs } from '../stores/jobs.svelte';
  import { minorToInput } from '../money';
  import Heading from '../components/Heading.svelte';
  import Feedback from '../components/Feedback.svelte';
  import MoneyField from '../components/MoneyField.svelte';
  import Modal from '../components/Modal.svelte';

  let {id}: {id:string}=$props();
  const accounts=query(()=>session.call(commands.accountsList(session.scope,{})));
  const categories=query(()=>session.call(commands.categoriesList(session.scope,{page:pageRequest(null,200)})));
  const payees=query(()=>session.call(commands.payeesList(session.scope,{page:pageRequest(null,200)})));
  const data=query(()=>session.call(commands.transactionGet(session.scope,{id})));
  const files=query(()=>session.call(commands.attachmentsList(session.scope,{transactionId:id})));
  const task=createTask();
  const snapshot=$derived(data.value);
  let loaded=$state('');
  let accountId=$state(page.url.searchParams.get('account')??'');
  let value=$state('');
  let description=$state('');
  let notes=$state('');
  let bankDate=$state(today());
  let billDate=$state('');
  let reportDate=$state('');
  let payee=$state('');
  let category=$state('');
  let direction=$state<Direction>('debit');
  let status=$state<TxStatus>('posted');
  let ignored=$state(false);
  let pnl=$state(false);
  let flagged=$state(false);
  let due=$state('');
  let splits=$state<{key:string;categoryId:string;value:string}[]>([]);
  const currency=$derived(accounts.value?.find(item=>item.id===accountId)?.account.currencyCode ?? snapshot?.draft.amount.currency ?? session.active?.baseCurrency ?? '');
  const protectedSource=$derived(!!snapshot?.sources.length || !!snapshot?.row.transferId);
  let installmentCount=$state(2);
  let installmentDate=$state(today());
  let installmentScope=$state<InstallmentScope>('all');
  let plan=$state<InstallmentPreview|null>(null);
  let deleting=$state(false);
  let transfer=$state(false);
  const link=query(async()=>{
    const linkId=snapshot?.row.transferId;
    return linkId?session.call(commands.transferGet(session.scope,{linkId})):null;
  });
  let candidates=$state<TransferCandidate[]>([]);
  let candidate=$state('');
  let matchAccount=$state('');
  let matchWindow=$state<number|undefined>();
  let matchText=$state('');
  let attachmentDelete=$state('');
  let rename=$state<{id:string;displayName:string}|null>(null);
  const tags=$derived([...new Set(notes.match(/#[\p{L}\p{N}_-]+/gu)??[])]);
  $effect(()=>{
    const item=snapshot;
    if(!item || loaded===item.row.version) return;
    const scale=privacy.scale(item.row.amount.currency);
    if(scale===undefined) return;
    load(item,scale);
  });
  function load(item:TxDetail_Serialize,scale:number){
    const draft=item.draft;
    loaded=item.row.version;
    accountId=draft.accountId;value=minorToInput(draft.amount.minor,scale);description=draft.description;notes=draft.notes;
    bankDate=draft.bankDate;billDate=draft.effectiveBillDate??'';reportDate=draft.reportingDateOverride??'';
    payee=draft.payeeId??'';category=draft.categoryId??'';direction=draft.direction;status=draft.status;
    ignored=draft.ignored;pnl=draft.excludeFromPnl;flagged=draft.flagged;due=draft.flagDueDate??'';
    splits=draft.splits.map(row=>({key:crypto.randomUUID(),categoryId:row.categoryId,value:minorToInput(row.minor,scale)}));
    installmentDate=draft.bankDate;
  }
  const remaining=$derived.by(()=>{
    try{
      return {currency,minor:(BigInt(privacy.amount(currency,value).minor)-splits.reduce((sum,row)=>sum+BigInt(privacy.amount(currency,row.value||'0').minor),0n)).toString()};
    }catch{return null;}
  });
  function posting():TxDraft_Deserialize{
    const amount=privacy.amount(currency,value);
    if(splits.length && (splits.length<2 || remaining?.minor!=='0' || splits.some(row=>BigInt(privacy.amount(currency,row.value).minor)<=0n))) throw new Error('Use at least two positive splits that add up to the amount.');
    return { ...snapshot?.draft, accountId,bankDate,amount,direction,status,description,notes,payeeId:payee||null,categoryId:splits.length?null:category||null,
      effectiveBillDate:billDate||null,reportingDateOverride:reportDate||null,ignored,excludeFromPnl:pnl,flagged,flagDueDate:flagged&&due?due:null,
      splits:splits.map(row=>({categoryId:row.categoryId,minor:privacy.amount(currency,row.value).minor})) };
  }
  function save(event:SubmitEvent) {
    event.preventDefault();
    void task.run(async()=>{
      await session.call(commands.transactionSave(session.mutation(),{target:snapshot?{id,version:snapshot.row.version}:null,draft:posting()}));
      data.refresh();
    });
  }
  function remove(){
    if(!snapshot)return;
    const target={id,version:snapshot.row.version};
    void task.run(async()=>{await session.call(commands.transactionDelete(session.mutation(),{target,confirm:true}));await goto('/transactions');});
  }
  function post(){
    if(!snapshot)return;
    const target={id,version:snapshot.row.version};
    void task.run(async()=>{await session.call(commands.transactionPost(session.mutation(),{target,bookingDate:bankDate}));data.refresh();});
  }
  function previewInstallments(){
    void task.run(async()=>{
      plan=await session.call(commands.installmentPreview(session.scope,{series:snapshot?.draft.installment?.seriesId,
        scope:installmentScope,draft:{posting:posting(),purchaseDate:installmentDate,occurrenceCount:installmentCount,anchorId:snapshot?.row.id}}));
    });
  }
  function applyInstallments(){
    if(!plan)return;
    const previewToken=plan.previewToken;
    void task.run(async()=>{await session.call(commands.installmentApply(session.mutation(),{previewToken}));plan=null;await goto('/transactions');});
  }
  function transferChoices(){
    void task.run(async()=>{
      candidates=(await session.call(commands.transferCandidates(session.scope,{
        anchorId:id,criteria:{accountId:matchAccount||null,dateWindowDays:matchWindow??null,text:matchText.trim()||null},page:pageRequest()
      }) )).rows.filter(item => item.posting.amount.currency === currency);
      if(!candidates.some(row=>row.posting.id===candidate))candidate='';
    });
  }
  function saveTransfer(event:SubmitEvent){
    event.preventDefault();
    void task.run(async()=>{
      if(snapshot){
        const chosen=candidates.find(item=>item.posting.id===candidate)?.posting;
        if(!chosen)throw new Error('Choose a matching transaction.');
        const anchor={kind:'existing' as const,target:{id,version:snapshot.row.version}};
        const other={kind:'existing' as const,target:{id:chosen.id,version:chosen.version}};
        await session.call(commands.transferSave(session.mutation(),{source:snapshot.row.direction==='debit'?anchor:other,destination:snapshot.row.direction==='credit'?anchor:other}));
        transfer=false;data.refresh();

      }
    });
  }
  function unlink(){
    const linkId=snapshot?.row.transferId;
    if(linkId) void task.run(async()=>{await session.call(commands.transferUnlink(session.mutation(),{linkId}));data.refresh();});
  }
  function addAttachment(){
    void task.run(async()=>{
      const grants=await session.call(commands.fileChoose({purpose:'attachmentRead',multiple:true}));
      if(!grants.length)return;
      const ticket=await session.call(commands.attachmentAddStart(session.mutation(),{transactionId:id,grants:grants.map(item=>item.id)}));
      await jobs.result(ticket,'attachmentAdd');files.refresh();
    });
  }
  function saveAttachment(attachmentId:string){
    void task.run(async()=>{
      const [grant]=await session.call(commands.fileChoose({purpose:'attachmentWrite'}));
      if(grant)await session.call(commands.attachmentSaveAs(session.scope,{id:attachmentId,saveGrant:grant.id}));
    });
  }
  function openAttachment(attachmentId:string){
    void task.run(async()=>{await session.call(commands.attachmentOpen(session.scope,{id:attachmentId}));});
  }
</script>
<Heading title={snapshot?.row.description??'Transaction'}>
  {#if snapshot}<div class="flex items-center gap-2 text-sm text-muted-foreground">
    <TransactionCategoryIcon row={snapshot.row}/>
    {snapshot.row.transferId?'Transfer':snapshot.row.category?.name??'Uncategorized'}
  </div>{/if}
  <Button variant="outline"  href="/transactions">All transactions</Button>
</Heading>
<Feedback error={task.error||data.error||accounts.error||files.error||link.error} loading={data.loading}/>
{#if snapshot && accounts.value}
  {#if !accounts.value.length}<Card as="p" class="">Create an <a class="underline" href="/accounts">account</a> before adding transactions.</Card>
  {:else}
    {#if snapshot}
      <Card class="mb-4 flex flex-wrap items-center gap-4">
        <span class="numeric text-xl">{privacy.money(snapshot.row.amount)}</span><Badge variant="secondary" class="capitalize">{snapshot.row.status}</Badge>
        <span class="text-sm text-muted-foreground">Reporting date {privacy.date(snapshot.row.reportingDate)}</span>
        {#if snapshot.sources.length}<Badge variant="secondary" >Imported</Badge>{/if}
        {#if snapshot.draft.aliasId}<Button variant="outline"  href={'/transactions?account='+snapshot.row.accountId+'&alias='+snapshot.draft.aliasId}>Linked card activity</Button>{/if}
        {#if snapshot.row.transferId}<Button variant="outline" onclick={unlink}>Unlink transfer</Button>
        {:else}<Button variant="outline" onclick={()=>{transfer=!transfer;if(transfer)transferChoices();}}>Find transfer match</Button>{/if}
      </Card>
    {/if}
    {#if link.value}
      {@const pair=link.value.pair}
      {@const other=pair.source.row.id===id?pair.destination.row:pair.source.row}
      <Card as="section" class="mb-4">
        <h2 class="section-title">Linked transfer</h2>
        <p class="mt-2"><a class="underline" href={'/transactions/'+other.id}>Other side of the transfer</a> · <span class="numeric">{privacy.money(other.amount)}</span></p>
      </Card>
    {/if}
    {#if transfer}
      <Card as="form" class="mb-4 space-y-4" onsubmit={saveTransfer}>
        <h2 class="section-title">Match an existing transaction</h2>
          <div class="form-grid">
            <Label class="field">Other account<NativeSelect bind:value={matchAccount}><option value="">All accounts</option>{#each accounts.value.filter(item=>item.id!==accountId && item.account.currencyCode===currency) as item (item.id)}<option value={item.id}>{item.account.name}</option>{/each}</NativeSelect></Label>
            <Label class="field">Days to look around the date<Input type="number" min={0} max={90} bind:value={matchWindow}/></Label>
            <Label class="field">Find by description<Input type="search" bind:value={matchText}/></Label>
            <Button variant="outline" type="button" class="self-end" disabled={task.busy} onclick={transferChoices}>Look for matches</Button>
          </div>
          <Label class="field">Matching transaction<NativeSelect required bind:value={candidate}><option value="">Choose a match</option>
            {#each candidates as item (item.posting.id)}<option value={item.posting.id}>{item.posting.description} · {privacy.date(item.posting.bankDate)} · {privacy.money(item.posting.amount)}</option>{/each}
          </NativeSelect></Label>
          {#if !candidates.length&&!task.busy}<p class="text-sm text-muted-foreground">No matching transactions found.</p>{/if}
        <Button variant="default" type="submit"  disabled={task.busy}>Save transfer</Button>
      </Card>
    {/if}
      <Card as="form" id="transaction-editor" class="space-y-4" onsubmit={save}>
        <div class="form-grid">
          <Label class="field">Account<NativeSelect bind:value={accountId} required disabled={protectedSource}><option value="">Choose account</option>{#each accounts.value as item (item.id)}<option value={item.id}>{item.account.name} · {item.account.currencyCode}</option>{/each}</NativeSelect></Label>
          <MoneyField bind:value disabled={protectedSource}/>
          <Label class="field">Description<Input bind:value={description} maxlength={256} required/></Label>
          <Label class="field">Transaction date<Input type="date" bind:value={bankDate} disabled={protectedSource} required/></Label>
          <Label class="field">Direction<NativeSelect bind:value={direction} disabled={protectedSource}><option value="debit">Money out</option><option value="credit">Money in</option></NativeSelect></Label>
          <Label class="field">Status<NativeSelect bind:value={status} disabled={protectedSource}><option value="posted">Confirmed</option><option value="pending">Pending</option><option value="projected">Expected</option></NativeSelect></Label>
          <Label class="field">Person or store<NativeSelect bind:value={payee}><option value="">No person or store</option>{#each payees.value?.rows??[] as item (item.id)}<option value={item.id}>{item.draft.name}</option>{/each}</NativeSelect></Label>
          <Label class="field">Category<NativeSelect bind:value={category} disabled={!!splits.length||!!snapshot?.row.transferId}><option value="">Uncategorized</option>{#each categories.value?.rows.filter(item=>item.draft.kind!=='transfer')??[] as item (item.id)}<option value={item.id}>{item.draft.name}</option>{/each}</NativeSelect></Label>
          <Label class="field">Bill date (optional)<Input type="date" bind:value={billDate}/></Label>
          <Label class="field">Use a different date in reports<Input type="date" bind:value={reportDate}/></Label>
        </div>
        <Label class="field">Notes<Textarea bind:value={notes} rows={3} maxlength={1000}></Textarea></Label>
        {#if tags.length}<ul class="flex flex-wrap gap-2" aria-label="Tags">{#each tags as tag (tag)}<li><Badge variant="secondary">{tag}</Badge></li>{/each}</ul>{/if}
        <div class="flex flex-wrap items-center gap-4">
          <Label class="check"><NativeCheckbox  bind:checked={ignored} disabled={!!snapshot?.row.transferId}/> Ignore transaction</Label>
          <Label class="check"><NativeCheckbox  bind:checked={pnl}/> Leave out of income and expense totals</Label>
          <Label class="check"><NativeCheckbox  bind:checked={flagged}/> Flag</Label>
          {#if flagged}<Label class="field">Flag due date<Input type="date" bind:value={due}/></Label>{/if}
        </div>
        {#if !snapshot?.row.transferId}
          <section class="space-y-3 border-t border-line pt-3">
            <div class="flex items-center justify-between gap-3"><h2 class="section-title">Category splits</h2><Button variant="outline" type="button" onclick={()=>splits=[...splits,{key:crypto.randomUUID(),categoryId:'',value:''}]}>Add split</Button></div>
            {#each splits as split (split.key)}
              <div class="grid items-end gap-3 sm:grid-cols-[1fr_1fr_auto]">
                <Label class="field">Category<NativeSelect required bind:value={split.categoryId}><option value="">Choose category</option>{#each categories.value?.rows.filter(item=>item.draft.kind!=='transfer')??[] as item (item.id)}<option value={item.id}>{item.draft.name}</option>{/each}</NativeSelect></Label>
                <MoneyField bind:value={split.value}/>
                <RowActions deleteLabel="Remove split" onDelete={()=>splits=splits.filter(row=>row.key!==split.key)}/>
              </div>
            {/each}
            {#if splits.length}<p class="text-sm">Remaining <span class="numeric">{privacy.money(remaining)}</span></p>{/if}
          </section>
        {/if}
        <div class="flex flex-wrap gap-2">
          <Button variant="default" type="submit"  disabled={task.busy}>Save transaction</Button>
          {#if snapshot && !snapshot.row.transferId && snapshot.row.entryKind!=='opening'}<Button variant="outline" type="button" onclick={()=>deleting=true}>Delete</Button>{/if}
          {#if snapshot?.row.status==='projected' && !protectedSource}<Button variant="outline" type="button" onclick={post}>Mark as confirmed</Button>{/if}
        </div>
      </Card>
    {#if snapshot}
      <Card as="section" class="mt-4">
        <div class="mb-3 flex items-center justify-between gap-3"><h2 class="section-title">Attachments</h2><Button variant="outline" onclick={addAttachment} disabled={task.busy}>Add files…</Button></div>
        {#each files.value??[] as file (file.id)}
          <div class="flex flex-wrap items-center gap-2 border-t border-line py-3">
            <span class="mr-auto">{file.displayName} <Badge variant="secondary" class="capitalize">{file.state}</Badge></span>
            <RowActions itemName={file.displayName} editLabel="Rename" onEdit={()=>rename={id:file.id,displayName:file.displayName}}
              deleteLabel="Remove" onDelete={()=>attachmentDelete=file.id} extraActions={[
                {id:'open',label:'Open',disabled:file.state!=='ready',onclick:()=>openAttachment(file.id)},
                {id:'save',label:'Save as…',disabled:file.state!=='ready',onclick:()=>saveAttachment(file.id)}
              ]}/>
          </div>
        {:else}<p class="text-sm text-muted-foreground">No attachments.</p>{/each}
      </Card>
    {/if}
  {/if}
{/if}
{#if deleting}<Modal confirm title="Delete transaction?" close={()=>deleting=false}><p>This removes the transaction from the activity file.</p><Button variant="default" class="mt-4" onclick={remove}>Confirm deletion</Button><Feedback error={task.error}/></Modal>{/if}
{#if plan}<Modal title="Installment preview" close={()=>plan=null}>
  <ul class="max-h-72 divide-y divide-line overflow-auto">{#each plan.rows as item (item.number)}<li class="flex justify-between py-2"><span>{item.number} · {privacy.date(item.bankDate)}</span><span class="numeric">{privacy.money(item.amount)}</span></li>{/each}</ul>
  <Feedback error={task.error}/><Button variant="default" class="mt-4" onclick={applyInstallments}>Apply series</Button>
</Modal>{/if}
{#if rename}<Modal title="Rename attachment" close={()=>rename=null}>
  <Label class="field">File name<Input bind:value={rename.displayName}/></Label>
  <Button variant="default" class="mt-4" onclick={()=>{const input=$state.snapshot(rename);if(input)void task.run(async()=>{await session.call(commands.attachmentRename(session.mutation(),input));rename=null;files.refresh();});}}>Save name</Button>
  <Feedback error={task.error}/>
</Modal>{/if}
{#if attachmentDelete}<Modal confirm title="Remove attachment?" close={()=>attachmentDelete=''}>
  <Button variant="default"  onclick={()=>task.run(async()=>{await session.call(commands.attachmentDelete(session.mutation(),{id:attachmentDelete,confirm:true}));attachmentDelete='';files.refresh();})}>Confirm removal</Button><Feedback error={task.error}/>
</Modal>{/if}
