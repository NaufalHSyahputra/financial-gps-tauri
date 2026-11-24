<script lang="ts">
  import RowActions from '#lib/components/RowActions.svelte';
  import { Button } from '#lib/components/ui/button/index.js';
  import { Label } from '#lib/components/ui/label/index.js';
  import { Input } from '#lib/components/ui/input/index.js';
  import { Card } from '#lib/components/ui/card/index.js';
  import { NativeSelect } from '#lib/components/ui/native-select/index.js';
  import { Table, TableHeader, TableRow, TableHead, TableBody, TableCell } from '#lib/components/ui/table/index.js';
  import { page } from '$app/state';
  import { goto } from '$app/navigation';
  import { commands, type AssetKind, type GrowthInterval, type HistoryView, type HistoryItem_Serialize, type TradeKind, type GrowthDraft } from '../../../lib/drivers/runtime';
  import { session, today, pageRequest } from '../../../lib/stores/session.svelte';
  import { privacy } from '../../../lib/stores/privacy.svelte';
  import { query, createTask } from '../../../lib/stores/task.svelte';
  import { minorToInput } from '../../../lib/money';
  import Heading from '../../../lib/components/Heading.svelte';
  import Feedback from '../../../lib/components/Feedback.svelte';
  import Modal from '../../../lib/components/Modal.svelte';
  import MoneyField from '../../../lib/components/MoneyField.svelte';
  import CurrencySelect from '../../../lib/components/CurrencySelect.svelte';
  type GrowthForm={date:string;interval:GrowthInterval;kind:GrowthDraft['change']['kind'];amount:string;key:string};
  const id=$derived(page.params.id??'new');
  let asOf=$state(today());
  let view=$state<HistoryView>('valuations');
  let since=$state('');
  let cursor=$state<string|null>(null);
  const asset=query(async()=>id==='new'?null:session.call(commands.assetGet(session.scope,{id,asOf})));
  const history=query(async()=>id==='new'?null:session.call(commands.assetHistory(session.scope,{assetId:id,view,range:{from:since||null,to:asOf||null},page:pageRequest(cursor,40)})));
  const task=createTask();
  let name=$state(''),currency=$state('USD'),loaded=$state('');
  let kind=$state<AssetKind>('other');
  let growth=$state<GrowthForm[]>([]);
  let modal=$state<'valuation'|'trade'|'remove'|'deleteAsset'|null>(null);
  let item=$state<HistoryItem_Serialize>();
  let date=$state(''),amount=$state(''),fees=$state('0'),units=$state(''),newUnits=$state(''),link=$state('');
  let tradeKind=$state<TradeKind['kind']>('buy');
  function decimal(minor:string,code:string){return minorToInput(minor,privacy.scale(code)??0);}
  $effect(()=>{
    const row=asset.value;
    if (loaded===id || (id!=='new' && row?.id!==id)) return;
    name=row?.draft.name??''; kind=row?.draft.kind??'other';currency=row?.draft.currency??session.active?.baseCurrency??'IDR';
    growth=row?.draft.growth.map(value=>({date:value.startsOn,interval:value.interval,kind:value.change.kind,amount:value.change.kind==='percentage'?value.change.value:decimal(value.change.value.minor,value.change.value.currency),key:crypto.randomUUID()}))??[];
    loaded=id;
  });
  function save(event:SubmitEvent) {
    event.preventDefault();void task.run(async()=>{
      const row=asset.value;
      const rules:GrowthDraft[]=growth.map(value=>({startsOn:value.date,interval:value.interval,change:value.kind==='percentage'?{kind:'percentage',value:value.amount}:{kind:'fixed',value:privacy.amount(currency,value.amount)}}));
      const result=await session.call(commands.assetSave(session.mutation(),{target:row?{id:row.id,version:row.version}:null,draft:{name,kind,currency,growth:rules}}));
      if(id==='new')await goto('/assets/'+result.value.id);else asset.refresh();
    });
  }
  function open(kind:'valuation'|'trade',row?:HistoryItem_Serialize){
    item=row;date=today();amount='';fees='0';units='';newUnits='';link='';tradeKind='buy';
    if(row?.kind==='valuation'){date=row.value.draft.valuedOn;amount=decimal(row.value.draft.value.minor,currency);}
    if(row?.kind==='trade'){
      const trade=row.value.draft.trade;
      date=row.value.draft.tradedOn;link=row.value.draft.linkedTransactionId??'';tradeKind=trade.kind;
      if(trade.kind==='split'){units=trade.oldUnits;newUnits=trade.newUnits;}
      else {units=trade.units;amount=decimal(trade.gross.minor,currency);fees=decimal(trade.fees.minor,currency);}
    }
    modal=kind;
  }
  function record(event:SubmitEvent){
    event.preventDefault();void task.run(async()=>{
      const target=item?{id:item.value.id,version:item.value.version}:null;
      if(modal==='valuation') await session.call(commands.assetValuationSave(session.mutation(),{target,draft:{assetId:id,valuedOn:date,value:privacy.amount(currency,amount)}}));
      if(modal==='trade'){
        const trade:TradeKind=tradeKind==='split'?{kind:tradeKind,oldUnits:units,newUnits}:{kind:tradeKind,units,gross:privacy.amount(currency,amount),fees:privacy.amount(currency,fees)};
        await session.call(commands.assetTradeSave(session.mutation(),{target,draft:{assetId:id,tradedOn:date,trade,linkedTransactionId:link||null}}));
      }
      if(modal==='remove' && item){
        const input={id:item.value.id,confirm:true};
        await session.call(item.kind==='trade'?commands.assetTradeDelete(session.mutation(),input):commands.assetValuationDelete(session.mutation(),input));
      }
      if(modal==='deleteAsset'){
        await session.call(commands.assetDelete(session.mutation(),{id,confirm:true}));await goto('/assets');
      }
      modal=null;asset.refresh();history.refresh();
    });
  }
</script>
<Heading title={asset.value?.draft.name??'New asset'}><Button variant="outline"  href="/assets">Assets</Button>{#if id!=='new'}<Label class="field">As of<Input type="date" bind:value={asOf} onchange={()=>cursor=null}/></Label>{/if}</Heading>
<Feedback error={asset.error || task.error} loading={asset.loading}/>
{#if asset.value}
<Card as="section" class="mb-5"><h2 class="section-title">Holding summary</h2><dl class="my-3 grid grid-cols-2 gap-3 md:grid-cols-5">
  <div><dt class="text-muted-foreground">Value</dt><dd>{privacy.money(asset.value.value)}</dd></div><div><dt class="text-muted-foreground">Cost basis</dt><dd>{privacy.money(asset.value.holding.costBasis)}</dd></div>
  <div><dt class="text-muted-foreground">Realized</dt><dd>{privacy.money(asset.value.holding.realized)}</dd></div><div><dt class="text-muted-foreground">Unrealized</dt><dd>{privacy.money(asset.value.unrealized)}</dd></div>
  <div><dt class="text-muted-foreground">Units</dt><dd>{privacy.masked?'••••':asset.value.holding.units}</dd></div>
</dl><p class="text-sm text-muted-foreground">Last valuation {privacy.date(asset.value.valuedOn)} {asset.value.valuationSource??''}</p></Card>
{/if}
{#if loaded===id}
<Card as="form" id="asset-editor" class="mb-5 space-y-4" onsubmit={save}>
  <h2 class="section-title">Asset details</h2>
  <div class="form-grid"><Label class="field">Name<Input bind:value={name} required/></Label><Label class="field">Type<NativeSelect bind:value={kind}><option value="realEstate">Real estate</option><option value="vehicle">Vehicle</option><option value="valuable">Valuable</option><option value="investment">Investment</option><option value="other">Other</option></NativeSelect></Label><CurrencySelect bind:value={currency} disabled={id!=='new'}/></div>
  <fieldset class="space-y-3"><legend class="section-title mb-2">Growth rules</legend>
    {#each growth as rule (rule.key)}<div class="form-grid rounded border border-line p-3">
      <Label class="field">Starts on<Input type="date" bind:value={rule.date} required/></Label>
      <Label class="field">Cadence<NativeSelect bind:value={rule.interval}><option>daily</option><option>weekly</option><option>monthly</option><option>yearly</option></NativeSelect></Label>
      <Label class="field">Change<NativeSelect bind:value={rule.kind}><option value="percentage">Percentage</option><option value="fixed">Fixed amount</option></NativeSelect></Label>
      <MoneyField label={rule.kind==='percentage'?'Percentage change':'Amount change'} bind:value={rule.amount}/>
      <RowActions deleteLabel="Remove growth rule" onDelete={()=>growth=growth.filter(row=>row.key!==rule.key)}/>
    </div>{/each}
    <Button variant="outline" type="button" onclick={()=>growth=[...growth,{date:today(),interval:'yearly',kind:'percentage',amount:'',key:crypto.randomUUID()}]}>Add growth rule</Button>
  </fieldset>
  <div class="flex flex-wrap gap-2"><Button variant="default" type="submit"  disabled={task.busy}>Save asset</Button>
    {#if asset.value}<Button variant="outline" type="button" disabled={task.busy} onclick={()=>void task.run(async()=>{await session.call(commands.assetArchive(session.mutation(),{id,archivedOn:asset.value?.archivedOn?null:today()}));asset.refresh();})}>{asset.value.archivedOn?'Restore asset':'Archive asset'}</Button><Button variant="outline" type="button" onclick={()=>modal='deleteAsset'}>Delete asset</Button>{/if}
  </div>
</Card>
{/if}
{#if asset.value}
<Card as="section" id="asset-history"><div class="mb-3 flex flex-wrap items-center justify-between gap-3"><h2 class="section-title">Valuations and trades</h2><div class="flex flex-wrap gap-2"><Button variant="outline" onclick={()=>open('valuation')}>Add valuation</Button><Button variant="outline" onclick={()=>open('trade')}>Record trade</Button></div></div>
  <div class="mb-3 flex flex-wrap gap-3"><Label class="field">History<NativeSelect bind:value={view} onchange={()=>cursor=null}><option value="valuations">Valuations</option><option value="trades">Trades</option></NativeSelect></Label><Label class="field">From<Input type="date" bind:value={since} onchange={()=>cursor=null}/></Label></div>
  <Feedback error={history.error} loading={history.loading}/>
  <div class="table-scroll"><Table contained={false}><TableHeader><TableRow><TableHead>Date</TableHead><TableHead>Event</TableHead><TableHead>Value</TableHead><TableHead class="w-px whitespace-nowrap text-right">Actions</TableHead></TableRow></TableHeader><TableBody>
    {#each history.value?.rows??[] as row (row.value.id)}
      <TableRow><TableCell>{privacy.date(row.kind==='trade'?row.value.draft.tradedOn:row.value.draft.valuedOn)}</TableCell><TableCell>{row.kind==='trade'?row.value.draft.trade.kind:'Manual valuation'}</TableCell>
        <TableCell class="numeric">{row.kind==='valuation'?privacy.money(row.value.draft.value):row.value.draft.trade.kind!=='split'?privacy.money(row.value.draft.trade.gross):privacy.masked?'••••':row.value.draft.trade.oldUnits+' → '+row.value.draft.trade.newUnits}</TableCell>
        <TableCell class="text-right"><RowActions itemName={row.kind+' on '+privacy.date(row.kind==='trade'?row.value.draft.tradedOn:row.value.draft.valuedOn)} onEdit={()=>open(row.kind,row)}
          onDelete={()=>{item=row;modal='remove';}}/></TableCell></TableRow>
    {:else}<TableRow><TableCell colspan={4}>No matching activity.</TableCell></TableRow>{/each}
  </TableBody></Table></div>
  {#if cursor}<Button variant="outline" class="mt-3" onclick={()=>cursor=null}>First records</Button>{/if}{#if history.value?.nextCursor}<Button variant="outline" class="mt-3" onclick={()=>cursor=history.value?.nextCursor??null}>More records</Button>{/if}
</Card>
{/if}
{#if modal}<Modal title={modal==='valuation'?'Manual valuation':modal==='trade'?'Record trade':modal==='remove'?'Delete record':'Delete asset'} close={()=>modal=null}><form class="space-y-3" onsubmit={record}>
  {#if modal==='valuation'||modal==='trade'}
    <Label class="field">Date<Input type="date" bind:value={date} required/></Label>
    {#if modal==='trade'}
      <Label class="field">Trade type<NativeSelect bind:value={tradeKind}><option value="buy">Buy</option><option value="sell">Sell</option><option value="split">Split</option></NativeSelect></Label>
      <MoneyField label={tradeKind==='split'?'Old units':'Units'} bind:value={units}/>
      {#if tradeKind==='split'}<MoneyField label="New units" bind:value={newUnits}/>{:else}<MoneyField label="Gross amount" bind:value={amount}/><MoneyField label="Fees" bind:value={fees}/>{/if}
      <Label class="field">Linked transaction ID (optional)<Input bind:value={link}/></Label>
    {:else}<MoneyField label="Market value" bind:value={amount}/>{/if}
  {:else}<p>{modal==='remove'?'Delete this history record?':'Delete this asset and its history?'}</p>{/if}
  <Feedback error={task.error}/><Button variant="outline" type="submit" class={modal==='remove'||modal==='deleteAsset'?'danger':'primary'} disabled={task.busy}>{modal==='remove'||modal==='deleteAsset'?'Delete':'Save'}</Button>
</form></Modal>{/if}
