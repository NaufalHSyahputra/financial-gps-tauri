<script lang="ts">
  import { goto } from '$app/navigation';
  import RowActions from '#lib/components/RowActions.svelte';
  import { Label } from '#lib/components/ui/label/index.js';
  import { NativeCheckbox } from '#lib/components/ui/checkbox/index.js';
  import { Button } from '#lib/components/ui/button/index.js';
  import { Badge } from '#lib/components/ui/badge/index.js';
  import { Card } from '#lib/components/ui/card/index.js';
  import { Input } from '#lib/components/ui/input/index.js';
  import { commands, type AccountKind, type AccountDetail_Serialize, type CardLineDetail_Serialize } from '../../lib/drivers/runtime';
  import { session,today } from '../../lib/stores/session.svelte';
  import { privacy } from '../../lib/stores/privacy.svelte';
  import { query,createTask } from '../../lib/stores/task.svelte';
  import { minorToInput } from '../../lib/money';
  import Feedback from '../../lib/components/Feedback.svelte';
  import Heading from '../../lib/components/Heading.svelte';
  import CreditMeter from '../../lib/components/CreditMeter.svelte';
  import CurrencySelect from '../../lib/components/CurrencySelect.svelte';
  import MoneyField from '../../lib/components/MoneyField.svelte';
  import AccountTypeIcon from '#lib/icons/AccountTypeIcon.svelte';
  import Modal from '../../lib/components/Modal.svelte';
  const kinds:{code:AccountKind;label:string}[]=[{code:'cash',label:'Cash'},{code:'checking',label:'Banks & wallets'},{code:'savings',label:'Savings'},{code:'creditCard',label:'Credit cards'},{code:'loan',label:'Debts & Loans'}];
  let archived=$state(false);
  let line=$state(false);
  let editedLine=$state<CardLineDetail_Serialize>();
  let removingLine=$state<CardLineDetail_Serialize>();
  let removingAccount=$state<AccountDetail_Serialize>();
  let name=$state('');
  let limit=$state('');
  let currency=$state('USD');
  const accounts=query(()=>session.call(commands.accountsList(session.scope,{includeArchived:archived})));
  const lines=query(()=>session.call(commands.cardLinesList(session.scope)));
  const task=createTask();
  function openLine(item?: CardLineDetail_Serialize) {
    editedLine=item;
    name=item?.draft.name??'';
    currency=item?.draft.currencyCode??session.active?.baseCurrency??'IDR';
    limit=item ? minorToInput(item.draft.limitMinor,privacy.scale(currency)??0) : '';
    line=true;
  }
  function save(event:SubmitEvent){event.preventDefault();void task.run(async()=>{
    await session.call(commands.cardLineSave(session.mutation(),{target:editedLine?{id:editedLine.id,version:editedLine.version}:null,draft:{name,currencyCode:currency,limitMinor:privacy.amount(currency,limit).minor}}));line=false;lines.refresh();
  });}
</script>
<Heading title="Accounts"><Label class="check"><NativeCheckbox  bind:checked={archived}/> Archived accounts</Label><Button variant="default"  href="/accounts/new">New account</Button></Heading>
<Feedback error={accounts.error || lines.error || task.error} loading={accounts.loading}/>
{#each kinds as kind (kind.code)}
  {@const matching=accounts.value?.filter(item=>item.account.kind===kind.code)??[]}
  {#if matching.length}
    <section class="mb-6" id={kind.code==='loan'?'loans':undefined}><h2 class="section-title mb-3">{kind.label}</h2>
      <div class="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
        {#each matching as item (item.id)}
          <Card as="article" class="relative hover:border-status">
            <div class="flex items-center justify-between gap-2"><h3 class="min-w-0 font-medium">
              <a href={'/accounts/'+item.id} class="tactile flex min-h-11 items-center gap-2 hover:underline after:absolute after:inset-0"><AccountTypeIcon kind={item.account.kind} wallet={item.account.paymentMethod==='Digital wallet'}/><span class="break-words">{item.account.name}</span></a></h3><Badge variant="secondary" >{item.account.currencyCode}</Badge></div>
            {#if item.account.institution}<p class="text-sm text-muted-foreground">{item.account.institution}</p>{/if}
            <p class="numeric mt-3 text-xl">{privacy.money(item.balance)}</p>
            {#if item.archivedOn}<Badge variant="secondary" >Archived</Badge>{/if}
            {#if item.credit && !item.credit.lineId}<CreditMeter credit={item.credit}/>{/if}
            <div class="pointer-events-none relative mt-2 flex justify-end [&>[data-row-actions]]:pointer-events-auto"><RowActions itemName={item.account.name}
              onEdit={()=>void goto('/accounts/'+item.id+'#account-editor')} onDelete={()=>removingAccount=item}
              extraActions={[
                {id:'transactions',label:'View transactions',onclick:()=>void goto('/transactions?account='+item.id)},
                {id:'archive',label:item.archivedOn?'Restore account':'Archive account',disabled:task.busy,onclick:()=>void task.run(async()=>{
                  await session.call(commands.accountArchive(session.mutation(),{target:{id:item.id,version:item.version},archivedOn:item.archivedOn?null:today()}));
                  accounts.refresh();
                })}
              ]}/></div>
          </Card>
        {/each}
      </div>
    </section>
  {/if}
{/each}
{#if !accounts.loading && accounts.value?.length===0}<Card as="p" class="mb-5">No accounts yet.</Card>{/if}
{#if accounts.value && !accounts.value.some(item=>item.account.kind==='loan')}
  <Card as="section" id="loans" class="mb-4 md:hidden">
    <h2 class="section-title mb-2">Debts &amp; Loans</h2>
    <p class="text-sm text-muted-foreground">Add a loan account to set up a payment plan.</p>
  </Card>
{/if}
<Card as="section" >
  <div class="mb-3 flex justify-between gap-3"><h2 class="section-title">Shared credit limits</h2><Button variant="outline" onclick={()=>openLine()}>New shared limit</Button></div>
  <div class="grid gap-4 sm:grid-cols-2">
    {#each lines.value??[] as item (item.id)}
      <section class="rounded-lg border border-line p-3"><h3 class="font-medium">{item.draft.name}</h3><CreditMeter credit={item.credit}/>
        <div class="flex justify-end"><RowActions itemName={item.draft.name} onEdit={()=>openLine(item)} onDelete={()=>removingLine=item}/></div>
      </section>
    {:else}<p class="text-sm text-muted-foreground">No shared lines configured.</p>{/each}
  </div>
</Card>
{#if line}<Modal title={editedLine?"Edit shared credit limit":"New shared credit limit"} close={()=>line=false}><form class="space-y-3" onsubmit={save}>
  <Label class="field">Name<Input required bind:value={name}/></Label><CurrencySelect bind:value={currency}/><MoneyField label="Credit limit" bind:value={limit}/>
  <Feedback error={task.error}/><Button variant="default" type="submit"  disabled={task.busy}>Save shared credit limit</Button>
</form></Modal>{/if}

{#if removingAccount}
  <Modal confirm title="Delete account?" close={()=>removingAccount=undefined}>
    <p>Delete {removingAccount.account.name}? Accounts with activity can be archived.</p>
    <Feedback error={task.error}/>
    <Button variant="destructive" class="mt-3" disabled={task.busy} onclick={()=>void task.run(async()=>{
      if (!removingAccount) return;
      const {id,version}=removingAccount;
      await session.call(commands.accountDelete(session.mutation(),{target:{id,version},confirm:true}));
      removingAccount=undefined; accounts.refresh();
    })}>Delete account</Button>
  </Modal>
{:else if removingLine}
  <Modal confirm title="Delete shared credit limit?" close={()=>removingLine=undefined}>
    <p>Delete {removingLine.draft.name}?</p><Feedback error={task.error}/>
    <Button variant="destructive" class="mt-3" disabled={task.busy} onclick={()=>void task.run(async()=>{
      if (!removingLine) return;
      await session.call(commands.cardLineDelete(session.mutation(),{id:removingLine.id}));
      removingLine=undefined; lines.refresh();
    })}>Delete shared limit</Button>
  </Modal>
{/if}
