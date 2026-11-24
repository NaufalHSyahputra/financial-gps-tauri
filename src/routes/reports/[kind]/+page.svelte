<script lang="ts">
  import { Button } from '#lib/components/ui/button/index.js';
  import { Card } from '#lib/components/ui/card/index.js';
  import { Label } from '#lib/components/ui/label/index.js';
  import { Input } from '#lib/components/ui/input/index.js';
  import { NativeSelect } from '#lib/components/ui/native-select/index.js';
  import { Table, TableHeader, TableRow, TableHead, TableBody, TableCell } from '#lib/components/ui/table/index.js';
  import { page } from '$app/state';
  import { commands, type DateBasis, type Direction, type Granularity } from '../../../lib/drivers/runtime';
  import { session, today } from '../../../lib/stores/session.svelte';
  import { privacy } from '../../../lib/stores/privacy.svelte';
  import { query, createTask } from '../../../lib/stores/task.svelte';
  import { reports } from '../../../lib/navigation';
  import { chartPoints } from '../../../lib/reporting';
  import Heading from '../../../lib/components/Heading.svelte';
  import Feedback from '../../../lib/components/Feedback.svelte';
  import ReportChart from '../../../lib/components/ReportChart.svelte';

  const kind=$derived(reports.find(item=>item.path===page.params.kind) ?? reports[0]);
  let from=$state(today().slice(0,4)+'-01-01');
  let to=$state(today());
  let granularity=$state<Granularity>('month');
  let accountId=$state('');
  let categoryId=$state('');
  const accounts=query(()=>session.call(commands.accountsList(session.scope,{})));
  const categories=query(()=>session.call(commands.categoriesList(session.scope,{page:{limit:200}})));
  const settings=query(()=>session.call(commands.layoutGet(session.scope,{view:kind.kind})));
  const task=createTask();
  const graph=$derived(!settings.value?.items.length||settings.value.items.some(item=>item.id==='chart'));
  const report=query(()=>session.call(commands.reportGet(session.scope,{kind:kind.kind,granularity,currency:session.active?.baseCurrency,
    filter:{range:{from,to},accountIds:accountId?[accountId]:[],categoryIds:categoryId?[categoryId]:[]}})));
  const series=$derived(chartPoints(report.value?.points??[],kind.kind));
  function toggleChart(){
    void task.run(async()=>{
      const items=graph ? [{id:'table',column:0,row:0,width:12,height:1}] : [{id:'chart',column:0,row:0,width:12,height:1},{id:'table',column:0,row:1,width:12,height:1}];
      await session.call(commands.layoutSet(session.mutation(),{view:kind.kind,layout:{items}}));
      settings.refresh();
    });
  }
  function drill(bucket:string,category?:string|null,basis:DateBasis='reporting',direction?:Direction) {
    const last=new Date(bucket+'T00:00:00Z');
    if(granularity==='week')last.setUTCDate(last.getUTCDate()+6);
    if(granularity==='month')last.setUTCMonth(last.getUTCMonth()+1,0);
    const bucketEnd=last.toISOString().slice(0,10);
    const params=new URLSearchParams({from:bucket<from?from:bucket,to:bucketEnd>to?to:bucketEnd,basis});
    if(kind.kind==='incomeVsExpense'||kind.kind==='categoryTrends')params.set('pnl','1');
    if(category)params.set('category',category);
    if(accountId)params.set('account',accountId);
    if(direction)params.set('direction',direction);
    return '/transactions?'+params;
  }
</script>
<Heading title={kind.label}><Button variant="outline" onclick={toggleChart} disabled={task.busy}>{graph?'Hide chart':'Show chart'}</Button></Heading>
<nav class="mb-5 flex flex-wrap gap-2" aria-label="Reports">
  {#each reports as item (item.kind)}<Button variant="outline"  aria-current={item.kind===kind.kind?'page':undefined} href={'/reports/'+item.path}>{item.label}</Button>{/each}
</nav>
<Card class="mb-5 grid gap-3 sm:grid-cols-3 xl:grid-cols-6">
  <Label class="field">From<Input type="date" bind:value={from}/></Label>
  <Label class="field">Through<Input type="date" bind:value={to}/></Label>
  <Label class="field">Group by<NativeSelect bind:value={granularity}><option value="month">Month</option><option value="week">Week</option><option value="day">Day</option></NativeSelect></Label>
  <Label class="field">Account<NativeSelect bind:value={accountId}><option value="">All accounts</option>{#each accounts.value??[] as item (item.id)}<option value={item.id}>{item.account.name}</option>{/each}</NativeSelect></Label>
  <Label class="field">Category<NativeSelect bind:value={categoryId}><option value="">All categories</option>{#each categories.value?.rows??[] as item (item.id)}<option value={item.id}>{item.draft.name}</option>{/each}</NativeSelect></Label>
</Card>
<Feedback error={report.error||task.error} loading={report.loading}/>
{#if report.value}
  <p class="mb-3 text-sm text-muted-foreground">Totals include amounts in {session.active?.baseCurrency}.</p>
  {#if graph}<Card class="mb-5"><ReportChart {series} label={kind.label}/></Card>{/if}
  <div class="table-wrap">
    <Table contained={false}><TableHeader><TableRow><TableHead>Period</TableHead>{#if kind.kind==='incomeVsExpense'||kind.kind==='categoryTrends'}<TableHead>Income</TableHead><TableHead>Expense</TableHead>{:else if kind.kind==='cashFlow'}<TableHead>Recorded</TableHead><TableHead>Expected</TableHead>{:else}<TableHead>Net worth</TableHead>{/if}</TableRow></TableHeader>
    <TableBody>
      {#each report.value.points as point (point.date)}
        <TableRow><TableCell><a href={drill(point.date,categoryId,kind.kind==='cashFlow'?'bank':'reporting')} class="underline">{privacy.date(point.date)}</a></TableCell>
          {#if kind.kind==='incomeVsExpense'||kind.kind==='categoryTrends'}<TableCell class="numeric"><a href={drill(point.date,categoryId,'reporting','credit')} aria-label="View income transactions">{privacy.money(point.income?.reliable)}</a></TableCell><TableCell class="numeric"><a href={drill(point.date,categoryId,'reporting','debit')} aria-label="View expense transactions">{privacy.money(point.expense?.reliable)}</a></TableCell>
          {:else if kind.kind==='cashFlow'}<TableCell class="numeric">{privacy.money(point.booked?.reliable)}</TableCell><TableCell class="numeric"><a href={drill(point.date,categoryId,'effective')} aria-label="View forecast transactions">{privacy.money(point.forecast?.reliable)}</a></TableCell>{:else}<TableCell class="numeric">{privacy.money(point.netWorth?.reliable)}</TableCell>{/if}
        </TableRow>
        {#each point.categories as category,index (index)}
          <TableRow class="bg-ground"><TableCell colspan={2}><a href={drill(point.date,category.categoryId)} class="underline">{categories.value?.rows.find(item=>item.id===category.categoryId)?.draft.name ?? 'Uncategorized'}</a></TableCell><TableCell class="numeric" colspan={2}>{privacy.money(category.amount.reliable)}</TableCell></TableRow>
        {/each}
      {/each}
    </TableBody></Table>
  </div>
{/if}
