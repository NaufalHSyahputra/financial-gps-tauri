<script lang="ts">
  const modalForm = $props.id();
  import RowActions from '#lib/components/RowActions.svelte';
  import CategoryIcon from '#lib/icons/CategoryIcon.svelte';
  import CategoryIconPicker from '#lib/icons/CategoryIconPicker.svelte';
  import { Button } from '#lib/components/ui/button/index.js';
  import { Card } from '#lib/components/ui/card/index.js';
  import { Label } from '#lib/components/ui/label/index.js';
  import { Input } from '#lib/components/ui/input/index.js';
  import { NativeCheckbox } from '#lib/components/ui/checkbox/index.js';
  import { Table, TableHeader, TableRow, TableHead, TableBody, TableCell } from '#lib/components/ui/table/index.js';
  import { NativeSelect } from '#lib/components/ui/native-select/index.js';
  import { commands, type CategoryKind, type CategoryDraft_Serialize, type PayeeDraft_Serialize, type VersionedId } from '../drivers/runtime';
  import { session,pageRequest } from '../stores/session.svelte';
  import { query,createTask } from '../stores/task.svelte';
  import Heading from '../components/Heading.svelte';
  import Feedback from '../components/Feedback.svelte';
  import Modal from '../components/Modal.svelte';
  let { mode }: {mode:'categories'|'payees'}=$props();
  const categoryMode=$derived(mode==='categories');
  const noun=$derived(categoryMode?'category':'person or store');
  let search=$state('');
  let includeArchived=$state(false);
  let cursor=$state<string>();
  const options=()=>({query:search||null,includeArchived,page:pageRequest(cursor)});
  const categories=query(()=>categoryMode?session.call(commands.categoriesList(session.scope,options())):Promise.resolve(null));
  const payees=query(()=>categoryMode?Promise.resolve(null):session.call(commands.payeesList(session.scope,options())));
  const groups=query(()=>categoryMode?session.call(commands.categoryGroupsList(session.scope,{page:pageRequest(null,200)})):Promise.resolve(null));
  const task=createTask();
  const page=$derived(categoryMode?categories.value:payees.value);
  let edit=$state(false);
  let groupEdit=$state(false);
  let target=$state<VersionedId|null>(null);
  let name=$state('');
  let groupId=$state('');
  let groupOrder=$state(0);
  let kind=$state<CategoryKind>('expense');
  let icon=$state<string|null>(null);
  let archived=$state(false);
  let hidden=$state(false);
  let ignored=$state(false);
  let removal=$state<{id:string;name:string;group?:boolean}|null>(null);
  let replacement=$state('');
  let merging=$state(false);
  let mergeFrom=$state<string[]>([]);
  function categoryDraft(draft: CategoryDraft_Serialize | PayeeDraft_Serialize): draft is CategoryDraft_Serialize {
    return 'kind' in draft;
  }
  function refresh(){categories.refresh();payees.refresh();groups.refresh();}
  function editEntry(next?:{id:string;version:string;draft:{name:string;archived:boolean;kind?:CategoryKind;icon?:string|null;groupId?:string|null;hidden?:boolean;ignored?:boolean}}) {
    target=next?{id:next.id,version:next.version}:null;
    name=next?.draft.name??'';archived=next?.draft.archived??false;groupId=next?.draft.groupId??'';
    icon=next?.draft.icon??null;
    kind=next?.draft.kind??'expense';hidden=next?.draft.hidden??false;ignored=next?.draft.ignored??false;edit=true;
  }
  function save(event:SubmitEvent) {
    event.preventDefault();
    void task.run(async()=>{
      if(categoryMode)await session.call(commands.categorySave(session.mutation(),{target,draft:{name,kind,icon,archived,hidden,ignored,groupId:groupId||null}}));
      else await session.call(commands.payeeSave(session.mutation(),{target,draft:{name,archived}}));
      edit=false;refresh();
    });
  }
  function remove(){
    const item=removal;
    if(!item)return;
    void task.run(async()=>{
      const input={id:item.id,replacementId:replacement||null,confirm:true};
      if(item.group)await session.call(commands.categoryGroupDelete(session.mutation(),input));
      else if(categoryMode)await session.call(commands.categoryDelete(session.mutation(),input));
      else await session.call(commands.payeeDelete(session.mutation(),input));
      removal=null;refresh();
    });
  }
</script>
<Heading title={categoryMode?'Categories':'People & stores'}>
  {#if categoryMode}<Button variant="outline" onclick={()=>{name='';groupOrder=0;target=null;groupEdit=true;}}>New group</Button>
  {:else}<Button variant="outline" onclick={()=>{replacement='';mergeFrom=[];merging=true;}}>Combine duplicate names</Button>{/if}
  <Button variant="default"  onclick={()=>editEntry()}>New {noun}</Button>
</Heading>
<Card class="mb-4 flex flex-wrap items-center gap-3">
  <Label class="field grow max-w-sm">Search<Input type="search" bind:value={search} oninput={()=>cursor=undefined}/></Label>
  <Label class="check"><NativeCheckbox  bind:checked={includeArchived} onchange={()=>cursor=undefined}/> Include archived</Label>
</Card>
<Feedback error={task.error||categories.error||payees.error||groups.error} loading={categories.loading||payees.loading}/>
{#if categoryMode && groups.value?.rows.length}
  <Card as="section" class="mb-4"><h2 class="section-title mb-2">Category groups</h2>
    {#each groups.value.rows as item (item.id)}<div class="flex items-center gap-2 border-t border-line py-2"><span class="mr-auto">{item.draft.name}</span>
      <RowActions onEdit={()=>{target={id:item.id,version:item.version};name=item.draft.name;groupOrder=item.draft.sortOrder;groupEdit=true;}}
        onDelete={()=>{replacement='';removal={id:item.id,name:item.draft.name,group:true};}}/>
    </div>{/each}
  </Card>
{/if}
<div class="table-wrap"><Table contained={false}><TableHeader><TableRow><TableHead>Name</TableHead><TableHead>Status</TableHead><TableHead class="w-px whitespace-nowrap text-right">Actions</TableHead></TableRow></TableHeader><TableBody>
  {#each page?.rows??[] as item (item.id)}<TableRow><TableCell class="font-medium"><div class="flex items-center gap-2">
    {#if categoryDraft(item.draft)}<CategoryIcon name={item.draft.name} kind={item.draft.kind} icon={item.draft.icon} size="sm"/>{/if}
    <span class="break-words">{item.draft.name}</span>
  </div></TableCell><TableCell>{item.draft.archived?'Archived':'Active'}{#if categoryDraft(item.draft)} · {item.draft.kind}{/if}</TableCell>
    <TableCell class="text-right"><RowActions onEdit={()=>editEntry(item)} onDelete={()=>{replacement='';removal={id:item.id,name:item.draft.name};}}/></TableCell></TableRow>
  {:else}<TableRow><TableCell colspan={3} class="text-muted-foreground">No matching {categoryMode?'categories':'people or stores'}.</TableCell></TableRow>{/each}
</TableBody></Table></div>
<div class="mt-3 flex gap-2"><Button variant="outline" disabled={!cursor} onclick={()=>cursor=undefined}>First page</Button><Button variant="outline" disabled={!page?.nextCursor} onclick={()=>cursor=page?.nextCursor}>Next page</Button></div>
{#if edit}<Modal title={(target?'Edit ':'New ')+noun} close={()=>edit=false}><form id={modalForm + '-catalog'} class="space-y-3" onsubmit={save}>
  <Label class="field">Name<Input required bind:value={name}/></Label>
  {#if categoryMode}
    <Label class="field">Type<NativeSelect bind:value={kind}><option value="expense">Expense</option><option value="income">Income</option><option value="transfer">Transfer</option></NativeSelect></Label>
    <CategoryIconPicker bind:value={icon} {name} {kind}/>
    <Label class="field">Group<NativeSelect bind:value={groupId}><option value="">None</option>{#each groups.value?.rows??[] as group (group.id)}<option value={group.id}>{group.draft.name}</option>{/each}</NativeSelect></Label>
    <Label class="check"><NativeCheckbox  bind:checked={hidden}/> Hide category</Label><Label class="check"><NativeCheckbox  bind:checked={ignored}/> Ignore in calculations</Label>
  {/if}
  <Label class="check"><NativeCheckbox  bind:checked={archived}/> Archived</Label><Feedback error={task.error}/>
</form>
  {#snippet footer()}
    <Button variant="default" type="submit" form={modalForm + '-catalog'}  disabled={task.busy}>Save {noun}</Button>
  {/snippet}
</Modal>{/if}
{#if groupEdit}<Modal title="Category group" close={()=>groupEdit=false}><form id={modalForm + '-category-group'} class="space-y-3" onsubmit={event=>{event.preventDefault();void task.run(async()=>{
  await session.call(commands.categoryGroupSave(session.mutation(),{target,draft:{name,sortOrder:groupOrder}}));groupEdit=false;refresh();});}}>
  <Label class="field">Name<Input required bind:value={name}/></Label><Label class="field">Sort order<Input type="number" required bind:value={groupOrder}/></Label>
  <Feedback error={task.error}/>
</form>
  {#snippet footer()}
    <Button variant="default" type="submit" form={modalForm + '-category-group'} >Save group</Button>
  {/snippet}
</Modal>{/if}
{#if removal}<Modal confirm title={'Delete '+removal.name+'?'} close={()=>removal=null}>
  <Label class="field">Move related transactions to<NativeSelect bind:value={replacement}><option value="">No replacement</option>
    {#each (removal.group?groups.value?.rows:page?.rows)??[] as item (item.id)}
      {#if item.id!==removal.id}<option value={item.id}>{item.draft.name}</option>{/if}
    {/each}
  </NativeSelect></Label>
  <Feedback error={task.error}/>
  {#snippet footer()}
    <Button variant="default" onclick={remove}>Confirm deletion</Button>
  {/snippet}
</Modal>{/if}
{#if merging}<Modal title="Combine duplicate names" close={()=>merging=false}>
  <Label class="field">Keep this name<NativeSelect bind:value={replacement}><option value="">Choose a name</option>{#each payees.value?.rows??[] as item (item.id)}<option value={item.id}>{item.draft.name}</option>{/each}</NativeSelect></Label>
  <fieldset class="mt-4 space-y-2"><legend>Combine these names into it</legend>
    {#each payees.value?.rows??[] as item (item.id)}
      {#if item.id!==replacement}<Label class="check"><NativeCheckbox checked={mergeFrom.includes(item.id)} onchange={event=>mergeFrom=event.currentTarget.checked?[...mergeFrom,item.id]:mergeFrom.filter(id=>id!==item.id)} /> {item.draft.name}</Label>{/if}
    {/each}
  </fieldset><Feedback error={task.error}/>
  {#snippet footer()}
    <Button variant="default" disabled={!replacement||!mergeFrom.some(id=>id!==replacement)} onclick={()=>task.run(async()=>{
    await session.call(commands.payeeMerge(session.mutation(),{fromIds:mergeFrom.filter(id=>id!==replacement),intoId:replacement,confirm:true}));merging=false;refresh();})}>Merge and move references</Button>
  {/snippet}
</Modal>{/if}
