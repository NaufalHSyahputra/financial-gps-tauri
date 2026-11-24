<script lang="ts">
  import RowActions from '#lib/components/RowActions.svelte';
  import { Button } from '#lib/components/ui/button/index.js';
  import { Card } from '#lib/components/ui/card/index.js';
  import { Label } from '#lib/components/ui/label/index.js';
  import { NativeSelect } from '#lib/components/ui/native-select/index.js';
  import { Table, TableHeader, TableRow, TableHead, TableBody, TableCell } from '#lib/components/ui/table/index.js';
  import { Input } from '#lib/components/ui/input/index.js';
  import { NativeCheckbox } from '#lib/components/ui/checkbox/index.js';
  import { commands, type Rule, type RuleDraft, type Predicate, type RulePreview, type OverridePolicy, type Child, type Action, type CollisionPolicy } from '../../lib/drivers/runtime';
  import { session,pageRequest } from '../../lib/stores/session.svelte';
  import { query,createTask } from '../../lib/stores/task.svelte';
  import { jobs } from '../../lib/stores/jobs.svelte';
  import Heading from '../../lib/components/Heading.svelte';
  import Feedback from '../../lib/components/Feedback.svelte';
  import Modal from '../../lib/components/Modal.svelte';
  import PredicateEditor from '../../lib/components/PredicateEditor.svelte';

  const rules=query(()=>session.call(commands.rulesList(session.scope)));
  const categories=query(()=>session.call(commands.categoriesList(session.scope,{page:pageRequest(null,200)})));
  const payees=query(()=>session.call(commands.payeesList(session.scope,{page:pageRequest(null,200)})));
  const task=createTask();
  let form=$state(false);
  let selected=$state<Rule|null>(null);
  let children=$state<{id:string;child:Child}[]>([]);
  let draft=$state<RuleDraft>(blank());
  let preview=$state<RulePreview|null>(null);
  let policy=$state<OverridePolicy>('preserve');
  let previewSaved=$state(false);
  let deleting=$state<Rule|null>(null);
  let collision=$state<CollisionPolicy>('reject');
  let done=$state('');
  function predicate():Predicate {return {field:'description',operator:'contains',value:{type:'text',value:''}};}
  function blank():RuleDraft{return {name:'',priority:0,enabled:true,applyExisting:false,overwriteExistingCategory:false,schemaVersion:1,conditions:{op:'and',children:[]},actions:[{kind:'description',value:''}]};}
  function edit(rule?:Rule){
    selected=rule??null;draft=rule?JSON.parse(JSON.stringify(rule.draft)) as RuleDraft:blank();
    if(!rule)draft.priority=rules.value?.length??0;
    children=(rule?draft.conditions.children:[{kind:'predicate',value:predicate()} as Child]).map(child=>({id:crypto.randomUUID(),child}));
    form=true;
  }
  function payload():RuleDraft{
    const output=$state.snapshot(draft);
    output.conditions.children=children.map(item=>$state.snapshot(item.child));
    return output;
  }
  function save(event:SubmitEvent){
    event.preventDefault();
    void task.run(async()=>{
      await session.call(commands.ruleSave(session.mutation(),{target:selected?{id:selected.id,version:selected.version}:null,draft:payload()}));
      form=false;rules.refresh();
    });
  }
  function dryRun(id?:string){
    void task.run(async()=>{
      const ticket=await session.call(commands.rulePreviewStart(session.scope,{draftOrIds:id?{kind:'saved',ids:[id]}:{kind:'draft',draft:payload()},filter:{},sampleLimit:25,overridePolicy:policy}));
      preview=await jobs.result(ticket,'rulePreview');previewSaved=!!id;
    });
  }
  function apply(){
    const current=preview;
    if(!current)return;
    void task.run(async()=>{
      const ticket=await session.call(commands.ruleApplyStart({...session.mutation(),expectedRevision:current.revision},{previewToken:current.previewToken,overridePolicy:policy}));
      const result=await jobs.result(ticket,'ruleApply');done=result.value.changed+' transactions updated.';preview=null;
    });
  }
  function reorder(index:number,delta:number){
    if(!rules.value)return;
    const ids=rules.value.map(item=>item.id);
    if(index+delta<0||index+delta>=ids.length)return;
    [ids[index],ids[index+delta]]=[ids[index+delta],ids[index]];
    void task.run(async()=>{await session.call(commands.rulesReorder(session.mutation(),{orderedIds:ids}));rules.refresh();});
  }
  function pack(direction:'import'|'export'){
    void task.run(async()=>{
      const [grant]=await session.call(commands.fileChoose({purpose:direction==='import'?'rulePackRead':'rulePackWrite'}));
      if(!grant)return;
      await session.call(commands.rulePackTransfer(session.mutation(),{direction,grant:grant.id,collisionPolicy:collision}));
      rules.refresh();done=direction==='import'?'Rule pack imported.':'Rule pack exported.';
    });
  }
  function changeAction(index:number,kind:Action['kind']){
    draft.actions[index]=kind==='ignore'?{kind,value:true}:{kind,value:''};
  }
</script>
<Heading title="Categorization rules"><Button variant="default"  onclick={()=>edit()}>New rule</Button></Heading>
<Feedback error={task.error||rules.error} loading={rules.loading}/>
{#if done}<Card as="p" class="mb-4" role="status">{done}</Card>{/if}
<Card class="mb-4 flex flex-wrap items-end gap-3">
  <Label class="field">Rule pack conflicts<NativeSelect bind:value={collision}><option value="reject">Reject import</option><option value="skip">Skip matching names</option><option value="replace">Replace matching rules</option></NativeSelect></Label>
  <Button variant="outline" onclick={()=>pack('import')}>Import rule pack…</Button><Button variant="outline" onclick={()=>pack('export')}>Export rule pack…</Button>
  <Label class="field">Retroactive preview<NativeSelect bind:value={policy}><option value="preserve">Preserve manual edits</option><option value="allowExplicit">Review replacing manual edits</option></NativeSelect></Label>
</Card>
<div class="table-wrap"><Table contained={false}><TableHeader><TableRow><TableHead>Priority</TableHead><TableHead>Rule</TableHead><TableHead>State</TableHead><TableHead class="w-px whitespace-nowrap text-right">Actions</TableHead></TableRow></TableHeader><TableBody>
  {#each rules.value??[] as rule,index (rule.id)}
    <TableRow><TableCell class="numeric">{index+1}</TableCell>
      <TableCell>{rule.draft.name}</TableCell><TableCell>{rule.draft.enabled?'Enabled':'Disabled'}</TableCell>
      <TableCell class="text-right"><RowActions itemName={rule.draft.name} onEdit={()=>edit(rule)} onDelete={()=>deleting=rule}
        extraActions={[
          {id:'preview',label:'Preview apply',disabled:!rule.draft.enabled||!rule.draft.applyExisting,onclick:()=>dryRun(rule.id)},
          {id:'up',label:'Move up',separatorBefore:true,disabled:index===0,onclick:()=>reorder(index,-1)},
          {id:'down',label:'Move down',disabled:index===(rules.value?.length??0)-1,onclick:()=>reorder(index,1)}
        ]}/></TableCell></TableRow>
  {:else}<TableRow><TableCell colspan={4}>No rules configured.</TableCell></TableRow>{/each}
</TableBody></Table></div>
{#if form}<Modal title={selected?'Edit rule':'New rule'} close={()=>form=false}>
  <form class="space-y-4" onsubmit={save}>
    <Label class="field">Rule name<Input required bind:value={draft.name}/></Label>
    <div class="flex flex-wrap gap-3"><Label class="check"><NativeCheckbox  bind:checked={draft.enabled}/> Enabled</Label><Label class="check"><NativeCheckbox  bind:checked={draft.applyExisting}/> Allow retroactive apply</Label><Label class="check"><NativeCheckbox  bind:checked={draft.overwriteExistingCategory}/> Overwrite existing category</Label></div>
    <Label class="field">Match<NativeSelect bind:value={draft.conditions.op}><option value="and">All conditions (AND)</option><option value="or">Any condition (OR)</option></NativeSelect></Label>
    {#each children as entry (entry.id)}
      {#if entry.child.kind==='predicate'}<PredicateEditor bind:predicate={entry.child.value} remove={()=>children=children.filter(item=>item.id!==entry.id)}/>
      {:else}
        {@const group=entry.child.value}
        <section class="space-y-2 rounded-lg border border-status p-3">
          <div class="flex flex-wrap items-end gap-2"><Label class="field grow">Nested group<NativeSelect bind:value={group.op}><option value="and">All (AND)</option><option value="or">Any (OR)</option></NativeSelect></Label>
            <RowActions deleteLabel="Remove group" onDelete={()=>children=children.filter(item=>item.id!==entry.id)}/></div>
          {#each group.children as item,index (index)}<PredicateEditor bind:predicate={group.children[index]} remove={()=>group.children=group.children.filter((_,i)=>i!==index)}/>{/each}
          <Button variant="outline" type="button" onclick={()=>group.children=[...group.children,predicate()]}>Add nested condition</Button>
        </section>
      {/if}
    {/each}
    <div class="flex gap-2"><Button variant="outline" type="button" onclick={()=>children=[...children,{id:crypto.randomUUID(),child:{kind:'predicate',value:predicate()}}]}>Add condition</Button><Button variant="outline" type="button" onclick={()=>children=[...children,{id:crypto.randomUUID(),child:{kind:'group',value:{op:'and',children:[predicate()]}}}]}>Add nested group</Button></div>
    <h3 class="section-title">Actions</h3>
    {#each draft.actions as action,index (index)}
      <div class="grid items-end gap-3 sm:grid-cols-[1fr_1fr_auto]">
        <Label class="field">Action<NativeSelect value={action.kind} onchange={event=>changeAction(index,event.currentTarget.value as Action['kind'])}>
          <option value="category">Set category</option><option value="payee">Set person or store</option><option value="description">Replace description</option><option value="appendNotes">Append note</option><option value="ignore">Mark ignored</option>
        </NativeSelect></Label>
        {#if action.kind==='ignore'}<Label class="check"><NativeCheckbox  bind:checked={action.value}/> Ignored</Label>
        {:else if action.kind==='category'||action.kind==='payee'}
          <Label class="field">Value<NativeSelect bind:value={action.value} required><option value="">Choose</option>
            {#each (action.kind==='category'?categories.value?.rows:payees.value?.rows)??[] as item (item.id)}<option value={item.id}>{item.draft.name}</option>{/each}
          </NativeSelect></Label>
        {:else}<Label class="field">Text<Input bind:value={action.value} required/></Label>{/if}
        <RowActions deleteLabel="Remove action" onDelete={()=>draft.actions=draft.actions.filter((_,i)=>i!==index)}/>
      </div>
    {/each}
    <Button variant="outline" type="button" onclick={()=>draft.actions=[...draft.actions,{kind:'category',value:''}]}>Add action</Button>
    <Feedback error={task.error}/><div class="flex gap-2"><Button variant="default" type="submit"  disabled={task.busy}>Save rule</Button><Button variant="outline" type="button" disabled={task.busy} onclick={()=>dryRun()}>Dry run</Button></div>
  </form>
</Modal>{/if}
{#if preview}<Modal title="Rule preview" close={()=>preview=null}>
  <p>{preview.matched} matched out of {preview.scanned}. {preview.wouldChange} would change.</p>
  <ul class="mt-3 max-h-64 divide-y divide-line overflow-auto">{#each preview.samples as row (row.id)}
    <li class="py-2 text-sm"><a class="underline" href={'/transactions/'+row.id}>View posting</a> · {row.changedFields.join(', ')||'No changes'}</li>
  {/each}</ul><Feedback error={task.error}/>
  {#if previewSaved}<Button variant="default" class="mt-4" onclick={apply} disabled={task.busy}>Apply preview</Button>{/if}
</Modal>{/if}
{#if deleting}<Modal confirm title={'Delete '+deleting.draft.name+'?'} close={()=>deleting=null}><Button variant="default"  onclick={()=>task.run(async()=>{
  if(deleting)await session.call(commands.ruleDelete(session.mutation(),{id:deleting.id}));deleting=null;rules.refresh();
})}>Confirm deletion</Button><Feedback error={task.error}/></Modal>{/if}
