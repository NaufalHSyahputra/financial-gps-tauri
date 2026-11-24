<script lang="ts">
  import RowActions from '#lib/components/RowActions.svelte';
  import { Button } from '#lib/components/ui/button/index.js';
  import { Label } from '#lib/components/ui/label/index.js';
  import { Input } from '#lib/components/ui/input/index.js';
  import { NativeSelect } from '#lib/components/ui/native-select/index.js';
  import { NativeCheckbox } from '#lib/components/ui/checkbox/index.js';
  import { Card } from '#lib/components/ui/card/index.js';
  import AccountTypeIcon from '#lib/icons/AccountTypeIcon.svelte';
  import { commands, type DashboardWidget, type LayoutItem } from '../../lib/drivers/runtime';
  import { session, today } from '../../lib/stores/session.svelte';
  import { query, createTask } from '../../lib/stores/task.svelte';
  import { privacy } from '../../lib/stores/privacy.svelte';
  import { chartPoints, primary } from '../../lib/reporting';
  import { reports } from '../../lib/navigation';
  import Heading from '../../lib/components/Heading.svelte';
  import Modal from '../../lib/components/Modal.svelte';
  import Feedback from '../../lib/components/Feedback.svelte';
  import MobileDashboard from '../../lib/views/MobileDashboard.svelte';
  import ReportChart from '../../lib/components/ReportChart.svelte';

  let showReports=$state(false);

  const all: {id:DashboardWidget; label:string}[]=[{id:'netWorth',label:'Net worth'}, {id:'incomeVsExpense',label:'Income vs. expense'}, {id:'cashFlow',label:'Cash flow'}, {id:'accounts',label:'Account balances'}, {id:'credit',label:'Available credit'}];
  const config=query(()=>showReports ? session.call(commands.layoutGet(session.scope,{view:'dashboard'})) : Promise.resolve(null));
  const defaults=all.map((item,index)=>({id:item.id,row:Math.floor(index/2),column:index%2*6,width:6,height:1}));
  const layout=$derived(config.value?.items.length?[...config.value.items].filter(item=>all.some(widget=>widget.id===item.id)).sort((a,b)=>a.row-b.row||a.column-b.column):defaults);
  const widgets=$derived(layout.map(item=>item.id as DashboardWidget));
  const accounts=query(()=>showReports ? session.call(commands.accountChoices(session.scope,{})) : Promise.resolve(null));
  let date=$state(today());
  let account=$state('');
  let editing=$state(false);
  let wide=$state<DashboardWidget[]>([]);
  let draft=$state<DashboardWidget[]>([]);
  const task=createTask();
  const data=query(async()=>!showReports||config.loading?null:session.call(commands.dashboardGet(session.scope,{asOf:date,widgets,accountIds:account?[account]:[]})));
  function open(){draft=[...widgets];wide=layout.filter(item=>item.width>6).map(item=>item.id as DashboardWidget);editing=true;}
  function move(index:number, delta:number){
    const next=[...draft]; const target=index+delta;
    if(target<0||target>=next.length) return;
    [next[index],next[target]]=[next[target],next[index]];
    draft=next;
  }
  function save() {
    void task.run(async()=>{
      let row=0,column=0;
      const items:LayoutItem[]=draft.map(id=>{
        const width=wide.includes(id)?12:6;
        if(column+width>12){row++;column=0;}
        const item={id,row,column,width,height:1};
        column+=width;
        return item;
      });
      await session.call(commands.layoutSet(session.mutation(),{view:'dashboard',layout:{items}}));
      config.refresh();editing=false;
    });
  }
  async function toggleMask(){
    await task.run(async()=>{await privacy.toggle();});
  }
</script>
<MobileDashboard {toggleMask}/>
  <Feedback error={task.error && !editing ? task.error : ''}/>
  <Button variant="outline" class="mt-5" type="button" aria-expanded={showReports} onclick={()=>showReports=!showReports}>
    {showReports?'Hide dashboard widgets':'Show dashboard widgets'}
  </Button>
{#if showReports}
<div class="max-md:mt-4">
<Heading title="Dashboard widgets" subtitle={session.active?.name}>
  <Label class="field"><span class="sr-only">As of</span><Input type="date" bind:value={date} /></Label>
  <Label class="field"><span class="sr-only">Account</span><NativeSelect bind:value={account}><option value="">All accounts</option>{#each accounts.value??[] as item (item.id)}<option value={item.id}>{item.name}</option>{/each}</NativeSelect></Label>
  <Button variant="outline" type="button" onclick={open}>Customize</Button>
</Heading>
<Feedback error={data.error || config.error || accounts.error} loading={data.loading}/>
{#if data.value}
  <div class="grid grid-cols-1 gap-4 xl:grid-cols-2">
    {#each widgets as id (id)}
      <Card as="section" class={layout.find(item=>item.id===id)?.width===12?'xl:col-span-2':undefined}>
        <div class="mb-4 flex items-center justify-between gap-3">
          <h2 class="section-title">{all.find(item=>item.id===id)?.label}</h2>
          {#if id==='accounts'}<a href="/accounts" class="text-sm underline">All accounts</a>
          {:else if id!=='credit'}<a href={'/reports/'+reports.find(item=>item.kind===id)?.path} class="text-sm underline">View report</a>{/if}
        </div>
        {#if id==='accounts'}
          <ul class="divide-y divide-line">
          {#each data.value.accounts as item (item.id)}
            <li class="flex items-center justify-between gap-2 py-2.5"><a href={'/accounts/'+item.id} class="truncate hover:underline"><AccountTypeIcon kind={item.account.kind} wallet={item.account.paymentMethod==='Digital wallet'}/> {item.account.name}</a><span class="numeric">{privacy.money(item.balance)}</span></li>
          {:else}<li class="text-sm text-muted-foreground">Add an account to see your balances.</li>{/each}
          </ul>
        {:else if id==='credit'}
          {#each data.value.credit as credit,index (index)}
            <div class="flex justify-between gap-3 border-b border-line py-2.5"><span>Available</span><span class="numeric">{privacy.money(credit.available)} <small class="text-muted-foreground">of {privacy.money(credit.limit)}</small></span></div>
          {:else}<p class="text-sm text-muted-foreground">No credit limits configured.</p>{/each}
        {:else}
          {@const view=data.value[id]}
          {#if view}
            <p class="numeric mb-3 text-2xl font-medium">{privacy.money(view.points.length ? primary(view.points[view.points.length-1],id)?.reliable : null)}</p>
            <ReportChart series={chartPoints(view.points,id)} label={all.find(item=>item.id===id)?.label ?? 'Report'}/>
          {/if}
        {/if}
      </Card>
    {/each}
  </div>
{/if}
</div>
{/if}
{#if editing}
  <Modal title="Dashboard widgets" close={()=>editing=false}>
    <Feedback error={task.error}/>
    {#each draft as id,i (id)}<div class="flex items-center justify-between gap-2 border-b border-line py-2">
      <span>{all.find(item=>item.id===id)?.label}</span>
      <div class="flex flex-wrap items-center gap-1"><Label class="check mr-2"><NativeCheckbox  checked={wide.includes(id)} onchange={event=>wide=event.currentTarget.checked?[...wide,id]:wide.filter(item=>item!==id)}/> Full width</Label><RowActions itemName={all.find(item=>item.id===id)?.label} deleteLabel="Remove" deleteDisabled={draft.length===1}
        onDelete={()=>draft=draft.filter(item=>item!==id)} extraActions={[
          {id:'up',label:'Move up',disabled:i===0,onclick:()=>move(i,-1)},
          {id:'down',label:'Move down',disabled:i===draft.length-1,onclick:()=>move(i,1)}
        ]}/></div>
    </div>{/each}
    <div class="mt-4 flex flex-wrap gap-2">{#each all.filter(item=>!draft.includes(item.id)) as item (item.id)}<Button variant="outline" onclick={()=>draft=[...draft,item.id]}>Add {item.label}</Button>{/each}</div>
    <Button variant="default" class="mt-4" disabled={task.busy} onclick={save}>Save layout</Button>
  </Modal>
{/if}
