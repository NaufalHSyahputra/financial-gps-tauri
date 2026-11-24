<script lang="ts">
  import RowActions from '#lib/components/RowActions.svelte';
  import AccountTypeIcon from '#lib/icons/AccountTypeIcon.svelte';
  import { Button } from '#lib/components/ui/button/index.js';
  import { Card } from '#lib/components/ui/card/index.js';
  import { Label } from '#lib/components/ui/label/index.js';
  import { Input } from '#lib/components/ui/input/index.js';
  import { NativeSelect } from '#lib/components/ui/native-select/index.js';
  import { Table, TableHeader, TableRow, TableHead, TableBody, TableCell } from '#lib/components/ui/table/index.js';
  import { Badge } from '#lib/components/ui/badge/index.js';
  import { goto } from '$app/navigation';
  import { commands, type AccountKind, type AccountDraft_Deserialize, type Amortization, type RatePeriod, type LoanKind, type StatementSource, type CorrectionRequest_Deserialize, type CorrectionPreview_Serialize } from '../drivers/runtime';
  import { session, today, pageRequest } from '../stores/session.svelte';
  import { privacy } from '../stores/privacy.svelte';
  import { query,createTask } from '../stores/task.svelte';
  import { minorToInput } from '../money';
  import Feedback from '../components/Feedback.svelte';
  import Heading from '../components/Heading.svelte';
  import CreditMeter from '../components/CreditMeter.svelte';
  import CurrencySelect from '../components/CurrencySelect.svelte';
  import MoneyField from '../components/MoneyField.svelte';
  import Modal from '../components/Modal.svelte';

  let {id}: {id:string}=$props();
  const data=query(()=>id==='new'?Promise.resolve(null):session.call(commands.accountGet(session.scope,{id})));
  const lines=query(()=>session.call(commands.cardLinesList(session.scope)));
  const task=createTask();
  let name=$state('');
  let institution=$state('');
  let currency=$state('IDR');
  let kind=$state<AccountKind>('checking');
  let payment=$state('');
  let line=$state('');
  let limit=$state('');
  let closeDay=$state<number|undefined>();
  let dueDay=$state<number|undefined>();
  let mode=$state<'cash'|'accrual'|''>('');
  let loaded=$state('');
  const openingHistory=query(()=>id==='new'?Promise.resolve([]):session.call(commands.postingAuditList(session.scope,{id})));
  let openingReview=$state<{request:CorrectionRequest_Deserialize;preview:CorrectionPreview_Serialize}>();
  let balanceAction=$state<'opening'|'adjustment'|null>(null);
  let value=$state('');
  let date=$state(today());
  let reason=$state('');
  let statementForm=$state(false);
  let removingStatement=$state('');
  let statementId=$state('');
  let closeOn=$state(today());
  let dueOn=$state('');
  let statementSource=$state<StatementSource>('manual');
  let loanForm=$state(false);
  let principal=$state('');
  let interest=$state('');
  let period=$state<RatePeriod>('yearlyEffective');
  let amortization=$state<Amortization>('price');
  let loanKind=$state<LoanKind>('personal');
  let months=$state(12);
  let firstPayment=$state(today());
  let scheduleCursor=$state<string>();
  const statements=query(()=>data.value?.account.kind==='creditCard'?session.call(commands.cardStatementsList(session.scope,{accountId:id,range:{}})):Promise.resolve([]));
  const loan=query(()=>data.value?.account.kind==='loan'?session.call(commands.loanGet(session.scope,{accountId:id})).catch(error=>{
    if(error?.messageKey==='invalid_input'||error?.messageKey==='not_found')return null;
    throw error;
  }):Promise.resolve(null));
  const schedule=query(()=>loan.value?session.call(commands.loanSchedule(session.scope,{accountId:id,page:pageRequest(scheduleCursor,60)})):Promise.resolve(null));
  $effect(()=>{
    const item=data.value;
    if(!item||item.version===loaded)return;
    const scale=privacy.scale(item.account.currencyCode);
    if(scale===undefined)return;
    loaded=item.version;
    const draft=item.account;
    name=draft.name;institution=draft.institution??'';currency=draft.currencyCode;kind=draft.kind;
    payment=draft.paymentMethod??'';line=draft.creditLineId??'';limit=draft.cardLimitMinor?minorToInput(draft.cardLimitMinor,scale):'';
    closeDay=draft.closeDay??undefined;dueDay=draft.dueDay??undefined;mode=draft.accountingOverride??'';
  });
  function save(event:SubmitEvent){
    event.preventDefault();
    void task.run(async()=>{
      const draft:AccountDraft_Deserialize={name,institution:institution||null,kind,currencyCode:currency,paymentMethod:kind==='checking'?payment||null:null};
      if(kind==='creditCard')Object.assign(draft,{closeDay,dueDay,creditLineId:line||null,cardLimitMinor:!line&&limit?privacy.amount(currency,limit).minor:null,accountingOverride:mode||null});
      const result=await session.call(commands.accountSave(session.mutation(),{target:data.value?{id,version:data.value.version}:null,draft}));
      if(id==='new')await goto('/accounts/'+result.value.id);else data.refresh();
    });
  }
  function saveBalance(event:SubmitEvent){
    event.preventDefault();
    void task.run(async()=>{
      const input={accountId:id,date,reason:reason||null,signedMoney:privacy.amount(currency,value)};
      if(balanceAction==='opening' && data.value?.account.kind!=='creditCard' && data.value?.account.kind!=='loan'){
        const request: CorrectionRequest_Deserialize={action:{opening:{accountId:id,amount:input.signedMoney,date}},reason};
        const preview=await session.call(commands.postingCorrectionPreview(session.scope,request));
        openingReview={request,preview};
        return;
      }
      await session.call(balanceAction==='opening'?commands.accountSetOpening(session.mutation(),input):commands.accountAdjustBalance(session.mutation(),input));
      balanceAction=null;data.refresh();openingHistory.refresh();
    });
  }
  function confirmOpening(){
    const reviewed=openingReview;
    if(!reviewed)return;
    void task.run(async()=>{
      await session.call(commands.postingCorrectionApply({...session.mutation(),expectedRevision:reviewed.preview.revision},
        {request:reviewed.request,token:reviewed.preview.token}));
      openingReview=undefined;balanceAction=null;data.refresh();openingHistory.refresh();
    });
  }
  function saveStatement(event:SubmitEvent){
    event.preventDefault();
    const target=statements.value?.find(item=>item.id===statementId);
    void task.run(async()=>{
      await session.call(commands.cardStatementSave(session.mutation(),{target:target?{id:target.id,version:target.version}:null,draft:{accountId:id,closeOn,dueOn,source:statementSource}}));
      statements.refresh();statementForm=false;
    });
  }
  function showLoan(){
    const terms=loan.value?.terms;
    if(terms){principal=minorToInput(terms.principal.minor,privacy.scale(currency)??0);interest=terms.interestRatePercent;period=terms.ratePeriod;amortization=terms.amortization;loanKind=terms.kind;months=terms.termMonths;firstPayment=terms.firstPaymentDate;}
    loanForm=true;
  }
  function saveLoan(event:SubmitEvent){
    event.preventDefault();
    void task.run(async()=>{
      await session.call(commands.loanSave(session.mutation(),{target:loan.value?{id,version:loan.value.version}:null,terms:{accountId:id,principal:privacy.amount(currency,principal),interestRatePercent:interest,ratePeriod:period,amortization,kind:loanKind,termMonths:months,firstPaymentDate:firstPayment}}));
      scheduleCursor=undefined;loan.refresh();loanForm=false;
    });
  }
</script>
<Heading title={id==='new'?'New account':data.value?.account.name??'Account'}>
  <Button variant="outline"  href="/accounts">All accounts</Button>{#if id!=='new'}<Button variant="outline"  href={'/transactions?account='+id}>Transactions</Button>{/if}
</Heading>
<Feedback error={data.error||task.error} loading={data.loading&&id!=='new'}/>
{#if data.value}
  <Card class="mb-4 flex flex-wrap items-center gap-3">
    <AccountTypeIcon kind={data.value.account.kind} wallet={data.value.account.paymentMethod==='Digital wallet'}/>
    <p class="numeric mr-auto text-2xl">{privacy.money(data.value.balance)}</p>
    <Button variant="outline" onclick={()=>{balanceAction='opening';openingReview=undefined;value='';reason='';date=today();}}>Set starting balance</Button>
    <Button variant="outline" onclick={()=>{balanceAction='adjustment';value='';reason='';date=today();}}>Correct balance</Button>
    <Button variant="outline" onclick={()=>task.run(async()=>{
      const account=data.value;
      if(account)await session.call(commands.accountArchive(session.mutation(),{target:{id,version:account.version},archivedOn:account.archivedOn?null:today()}));
      data.refresh();
    })}>{data.value.archivedOn?'Restore account':'Archive'}</Button>
  </Card>
  {#if data.value.credit}<Card class="mb-4 max-w-xl"><h2 class="section-title">Credit card balance and limit</h2><CreditMeter credit={data.value.credit}/></Card>{/if}
{/if}
{#if id==='new'||data.value}
  <Card as="form" id="account-editor" class="space-y-4" onsubmit={save}>
    <h2 class="section-title">Account settings</h2>
    <div class="form-grid">
      <Label class="field">Name<Input required bind:value={name}/></Label><Label class="field">Institution<Input bind:value={institution}/></Label>
      <Label class="field">Account type<NativeSelect bind:value={kind}><option value="cash">Cash</option><option value="checking">Bank account</option><option value="savings">Savings</option><option value="creditCard">Credit card</option><option value="loan">Loan</option></NativeSelect></Label>
      <CurrencySelect bind:value={currency} disabled={id!=='new'}/>
      {#if kind==='checking'}<Label class="field">Payment method<Input bind:value={payment}/></Label>{/if}
      {#if kind==='creditCard'}
        <Label class="field">Close day<Input type="number" min={1} max={31} bind:value={closeDay}/></Label>
        <Label class="field">Due day<Input type="number" min={1} max={31} bind:value={dueDay}/></Label>
        <Label class="field">Credit line<NativeSelect bind:value={line}><option value="">Individual limit</option>{#each lines.value?.filter(item=>item.draft.currencyCode===currency)??[] as item (item.id)}<option value={item.id}>{item.draft.name}</option>{/each}</NativeSelect></Label>
        {#if !line}<MoneyField label="Credit limit" bind:value={limit} required={false}/>{/if}
        <Label class="field">Accounting mode<NativeSelect bind:value={mode}><option value="">Use activity setting</option><option value="cash">When it is billed</option><option value="accrual">When the purchase happens</option></NativeSelect></Label>
      {/if}
    </div>
    <Button variant="default" type="submit"  disabled={task.busy}>Save account</Button>
  </Card>
{/if}
{#if data.value?.account.kind==='creditCard'}
  <Card as="section" class="mt-4"><div class="mb-3 flex items-center justify-between"><h2 class="section-title">Statement cycles</h2><Button variant="outline" onclick={()=>{statementId='';closeOn=today();dueOn='';statementSource='manual';statementForm=true;}}>Add statement</Button></div>
    <Feedback error={statements.error}/>
    <div class="table-wrap"><Table contained={false}><TableHeader><TableRow><TableHead>Close date</TableHead><TableHead>Due date</TableHead><TableHead>Source</TableHead><TableHead class="w-px whitespace-nowrap text-right">Actions</TableHead></TableRow></TableHeader><TableBody>
    {#each statements.value??[] as item (item.id)}<TableRow><TableCell>{privacy.date(item.draft.closeOn)}</TableCell><TableCell>{privacy.date(item.draft.dueOn)}</TableCell><TableCell>{item.draft.source}</TableCell>
      <TableCell class="text-right"><RowActions itemName={'statement closing '+privacy.date(item.draft.closeOn)}
        onEdit={()=>{statementId=item.id;closeOn=item.draft.closeOn;dueOn=item.draft.dueOn;statementSource=item.draft.source;statementForm=true;}}
        onDelete={()=>removingStatement=item.id} extraActions={[
          {id:'transactions',label:'View transactions',onclick:()=>void goto('/transactions?account='+id+'&statement='+item.id)}
        ]}/></TableCell></TableRow>
    {:else}<TableRow><TableCell colspan={4}>No statements registered.</TableCell></TableRow>{/each}
    </TableBody></Table></div>
  </Card>
{/if}
{#if data.value?.account.kind==='loan'}
  <Card as="section" class="mt-4"><div class="mb-3 flex items-center justify-between"><h2 class="section-title">Loan payment plan</h2><Button variant="outline" onclick={showLoan}>{loan.value?'Edit loan terms':'Set loan terms'}</Button></div>
    <Feedback error={loan.error||schedule.error} loading={schedule.loading}/>
    {#if schedule.value}<div class="table-wrap"><Table contained={false}><TableHeader><TableRow><TableHead>Payment</TableHead><TableHead>Due</TableHead><TableHead>Principal</TableHead><TableHead>Interest</TableHead><TableHead>Payment amount</TableHead><TableHead>Remaining</TableHead></TableRow></TableHeader><TableBody>
    {#each schedule.value.rows as item (item.number)}<TableRow><TableCell>{item.number}</TableCell><TableCell>{privacy.date(item.dueDate)} {#if item.overdue}<Badge variant="secondary" >Overdue</Badge>{/if}</TableCell>
      <TableCell class="numeric">{privacy.money(item.principal)}</TableCell><TableCell class="numeric">{privacy.money(item.interest)}</TableCell><TableCell class="numeric">{privacy.money(item.payment)}</TableCell><TableCell class="numeric">{privacy.money(item.balance)}</TableCell></TableRow>{/each}
    </TableBody></Table></div>
    <div class="mt-3 flex gap-2"><Button variant="outline" disabled={!scheduleCursor} onclick={()=>scheduleCursor=undefined}>First page</Button><Button variant="outline" disabled={!schedule.value.nextCursor} onclick={()=>scheduleCursor=schedule.value?.nextCursor}>Next page</Button></div>{/if}
  </Card>
{/if}
{#if openingHistory.value?.length}
  <Card as="section" class="mt-4">
    <h2 class="section-title">Balance history</h2>
    <ul class="mt-2 divide-y divide-line">
      {#each openingHistory.value as entry (entry.id)}
        <li class="py-2 text-sm">
          <p class="font-medium">{entry.action === 'opening' ? 'Opening snapshot' : 'Balance adjustment'} · {new Date(Number(entry.recordedAtMs)).toLocaleString()}</p>
          <p>{entry.reason}</p>
          {#if entry.after}<p>{entry.after.row.bankDate} · {privacy.money(entry.after.row.amount)}</p>{/if}
        </li>
      {/each}
    </ul>
  </Card>
{/if}
{#if balanceAction}
  {@const snapshot = balanceAction === 'opening' && data.value?.account.kind !== 'creditCard' && data.value?.account.kind !== 'loan'}
  <Modal title={snapshot?'Set opening snapshot':balanceAction==='opening'?'Set starting balance':'Correct balance'}
    close={()=>{balanceAction=null;openingReview=undefined;}}>
    {#if openingReview}
      <div class="space-y-3">
        <p>Use {privacy.money(privacy.amount(currency,value))} as the starting balance on {date}. Earlier transactions remain in activity and are excluded from this account balance.</p>
        {#if openingReview.preview.before}<p>Previous opening entry · {openingReview.preview.before.row.bankDate} · {privacy.money(openingReview.preview.before.row.amount)}</p>{/if}
        <Feedback error={task.error}/>
        <div class="flex flex-wrap gap-2">
          <Button variant="outline" type="button" onclick={()=>openingReview=undefined}>Back</Button>
          <Button disabled={task.busy} onclick={confirmOpening}>Confirm snapshot</Button>
        </div>
      </div>
    {:else}
      <form class="space-y-3" onsubmit={saveBalance}>
        <MoneyField label="Signed amount" bind:value/>
        <Label class="field">Date<Input type="date" bind:value={date} required/></Label>
        {#if snapshot || balanceAction==='adjustment'}<Label class="field">Reason<Input required maxlength={500} bind:value={reason}/></Label>{/if}
        <Feedback error={task.error}/><Button type="submit" disabled={task.busy}>{snapshot?'Review snapshot':'Save'}</Button>
      </form>
    {/if}
  </Modal>
{/if}
{#if statementForm}<Modal title="Card statement" close={()=>statementForm=false}>
  <form class="space-y-3" onsubmit={saveStatement}><Label class="field">Close date<Input type="date" bind:value={closeOn} required/></Label><Label class="field">Due date<Input type="date" bind:value={dueOn} required/></Label>
    <Feedback error={task.error}/><Button variant="default" type="submit"  disabled={task.busy}>Save statement</Button>
  </form>
</Modal>{/if}
{#if loanForm}<Modal title="Loan terms" close={()=>loanForm=false}><form class="space-y-4" onsubmit={saveLoan}>
  <div class="form-grid"><MoneyField label="Principal" bind:value={principal}/><MoneyField label="Interest rate %" bind:value={interest}/>
    <Label class="field">Rate period<NativeSelect bind:value={period}><option value="yearlyEffective">Effective annual rate</option><option value="yearlyNominal">Simple annual rate</option><option value="monthly">Monthly</option></NativeSelect></Label>
    <Label class="field">Amortization<NativeSelect bind:value={amortization}><option value="price">Price</option><option value="sac">SAC</option></NativeSelect></Label>
    <Label class="field">Loan type<NativeSelect bind:value={loanKind}>{#each ['mortgage','auto','student','personal','other'] as type (type)}<option value={type}>{type}</option>{/each}</NativeSelect></Label>
    <Label class="field">Months<Input type="number" bind:value={months} min={1} max={1200} required/></Label><Label class="field">First payment<Input type="date" bind:value={firstPayment} required/></Label>
  </div><Feedback error={task.error}/><Button variant="default" type="submit"  disabled={task.busy}>Save terms</Button>
</form></Modal>{/if}

{#if removingStatement}
  <Modal confirm title="Delete card statement?" close={()=>removingStatement=''}>
    <Feedback error={task.error}/>
    <Button variant="destructive" class="mt-3" disabled={task.busy} onclick={()=>void task.run(async()=>{
      await session.call(commands.cardStatementDelete(session.mutation(),{id:removingStatement}));
      removingStatement=''; statements.refresh();
    })}>Delete statement</Button>
  </Modal>
{/if}
