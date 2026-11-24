<script lang="ts">
  import { Button } from '#lib/components/ui/button/index.js';
  import { Label } from '#lib/components/ui/label/index.js';
  import { Input } from '#lib/components/ui/input/index.js';
  import { Card } from '#lib/components/ui/card/index.js';
  import { Badge } from '#lib/components/ui/badge/index.js';
  import { NativeCheckbox } from '#lib/components/ui/checkbox/index.js';
  import { page } from '$app/state';
  import { goto } from '$app/navigation';
  import { commands } from '../../../../lib/drivers/runtime';
  import { session, today, pageRequest } from '../../../../lib/stores/session.svelte';
  import { privacy } from '../../../../lib/stores/privacy.svelte';
  import { query, createTask } from '../../../../lib/stores/task.svelte';
  import Heading from '../../../../lib/components/Heading.svelte';
  import Feedback from '../../../../lib/components/Feedback.svelte';
  import Modal from '../../../../lib/components/Modal.svelte';
  let asOf=$state(today());
  const id=$derived(page.params.id??'new');
  const data=query(async()=>id==='new'?null:session.call(commands.assetGroupGet(session.scope,{id,asOf})));
  let listCursor=$state<string|null>(null);
  const assets=query(()=>session.call(commands.assetsList(session.scope,{asOf,page:pageRequest(listCursor,80)})));
  const task=createTask();
  let name=$state('');
  let archived=$state(false);
  let selection=$state<string[]>([]);
  let remove=$state(false);
  let loaded=$state('');
  $effect(()=>{
    const detail=data.value;
    if(loaded===id || (id!=='new' && detail?.group.id!==id))return;
    name=detail?.group.draft.name??'';archived=detail?.group.draft.archived??false;selection=[...(detail?.group.assetIds??[])];loaded=id;
  });
  function save(event:SubmitEvent){
    event.preventDefault();void task.run(async()=>{
      const group=data.value?.group;
      const result=await session.call(commands.assetGroupSave(session.mutation(),{target:group?{id:group.id,version:group.version}:null,draft:{name,archived},assetIds:selection}));
      if(id==='new')await goto('/assets/groups/'+result.value.group.id);else data.refresh();
    });
  }
</script>
<Heading title={data.value?.group.draft.name??'New asset group'}><Button variant="outline"  href="/assets">Assets</Button>{#if id!=='new'}<Label class="field">As of<Input type="date" bind:value={asOf}/></Label>{/if}</Heading>
<Feedback error={data.error || assets.error || task.error} loading={data.loading || assets.loading}/>
{#if data.value}<Card as="section" class="mb-4"><h2 class="section-title">Group valuation</h2><p class="numeric my-2 text-xl">{privacy.money(data.value.value)}</p>
  {#if data.value.missingRates!=='0'}<Badge variant="secondary" >Some amounts use a different currency.</Badge>{/if}
  <ul class="flex flex-wrap gap-3">{#each data.value.native as value (value.currency)}<li>{privacy.money(value)}</li>{/each}</ul>
</Card>{/if}
{#if loaded===id}<Card as="form" id="asset-group-editor" class="space-y-4" onsubmit={save}>
  <Label class="field">Name<Input required bind:value={name}/></Label>
  <Label class="check"><NativeCheckbox  bind:checked={archived}/> Archived</Label>
  <fieldset><legend class="section-title">Holdings</legend>
    <p class="my-2 text-sm text-muted-foreground">{selection.length} selected</p>
    <div class="grid gap-2 sm:grid-cols-2">
      {#each assets.value?.rows??[] as row (row.id)}
        <Label class="check"><NativeCheckbox  checked={selection.includes(row.id)} onchange={event=>{selection=event.currentTarget.checked?[...selection,row.id]:selection.filter(key=>key!==row.id);}}/> {row.draft.name}</Label>
      {:else}<p class="text-muted-foreground">No holdings available.</p>{/each}
    </div>
    {#if listCursor}<Button variant="outline" type="button" class="mt-3" onclick={()=>listCursor=null}>First holdings</Button>{/if}
    {#if assets.value?.nextCursor}<Button variant="outline" type="button" class="mt-3" onclick={()=>listCursor=assets.value?.nextCursor??null}>Next holdings</Button>{/if}
  </fieldset>
  <div class="flex gap-2"><Button variant="default" type="submit"  disabled={task.busy}>Save group</Button>{#if data.value}<Button variant="outline" type="button" onclick={()=>remove=true}>Delete group</Button>{/if}</div>
</Card>{/if}
{#if remove}<Modal confirm title="Delete asset group" close={()=>remove=false}><p>Delete this group?</p><Feedback error={task.error}/>

  {#snippet footer()}
    <Button variant="destructive" disabled={task.busy} onclick={()=>void task.run(async()=>{await session.call(commands.assetGroupDelete(session.mutation(),{id,confirm:true}));await goto('/assets');})}>Delete group</Button>
  {/snippet}
</Modal>{/if}
