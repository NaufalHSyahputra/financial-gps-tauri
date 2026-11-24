<script lang="ts">
  import { goto } from '$app/navigation';
  import RowActions from '#lib/components/RowActions.svelte';
  import Modal from '#lib/components/Modal.svelte';
  import { Label } from '#lib/components/ui/label/index.js';
  import { Input } from '#lib/components/ui/input/index.js';
  import { Button } from '#lib/components/ui/button/index.js';
  import { Badge } from '#lib/components/ui/badge/index.js';
  import { Card } from '#lib/components/ui/card/index.js';
  import { commands } from '../../lib/drivers/runtime';
  import { session, today, pageRequest } from '../../lib/stores/session.svelte';
  import { privacy } from '../../lib/stores/privacy.svelte';
  import { query,createTask } from '../../lib/stores/task.svelte';
  import Heading from '../../lib/components/Heading.svelte';
  import Feedback from '../../lib/components/Feedback.svelte';
  const task=createTask();
  let removal=$state<{ id:string; name:string; kind:'asset'|'group' }>();
  function remove() {
    if (!removal) return;
    const { id,kind }=removal;
    void task.run(async()=>{
      const command=kind==='asset'?commands.assetDelete:commands.assetGroupDelete;
      await session.call(command(session.mutation(),{id,confirm:true}));
      removal=undefined;
      (kind==='asset'?assets:groups).refresh();
    });
  }
  let asOf=$state(today());
  let assetCursor=$state<string|null>(null);
  let groupCursor=$state<string|null>(null);
  const assets=query(()=>session.call(commands.assetsList(session.scope,{asOf,page:pageRequest(assetCursor,48)})));
  const groups=query(()=>session.call(commands.assetGroupsList(session.scope,{page:pageRequest(groupCursor,48)})));
</script>
<Heading title="Assets" subtitle="Track holdings, valuations and investment activity."><Label class="field">As of<Input type="date" bind:value={asOf} onchange={()=>assetCursor=null}/></Label><Button variant="outline"  href="/assets/groups/new">New asset group</Button><Button variant="default"  href="/assets/new">New asset</Button></Heading>
<Feedback error={assets.error || groups.error || task.error} loading={assets.loading || groups.loading}/>
<div class="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
  {#each assets.value?.rows??[] as item (item.id)}
    <Card as="article" class="flex items-center gap-2 p-0 hover:border-status md:p-0">
      <a href={'/assets/'+item.id} aria-label={item.draft.name} class="tactile min-h-11 min-w-0 flex-1 self-stretch rounded-l-xl p-3 no-underline hover:bg-ground focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring">
      <div class="flex flex-wrap items-center justify-between gap-2"><h2 class="section-title">{item.draft.name}</h2><Badge variant="secondary" >{item.archivedOn?'Archived':item.draft.kind.replace(/([A-Z])/g,' $1')}</Badge></div>
      <p class="numeric my-3 text-xl">{privacy.money(item.value)}</p>
      <p class="text-sm text-muted-foreground">Valued {privacy.date(item.valuedOn)} {item.valuationSource??''}</p>
      </a>
      <div class="shrink-0 pr-2"><RowActions itemName={item.draft.name}
        onEdit={()=>void goto('/assets/'+item.id+'#asset-editor')}
        onDelete={()=>removal={id:item.id,name:item.draft.name,kind:'asset'}} extraActions={[
          {id:'history',label:'View valuations and trades',onclick:()=>void goto('/assets/'+item.id+'#asset-history')},
          {id:'archive',label:item.archivedOn?'Restore asset':'Archive asset',disabled:task.busy,onclick:()=>void task.run(async()=>{
            await session.call(commands.assetArchive(session.mutation(),{id:item.id,archivedOn:item.archivedOn?null:today()})); assets.refresh();
          })}
        ]}/></div>
    </Card>
  {:else}{#if !assets.loading}<Card as="p" class="">No assets found.</Card>{/if}{/each}
</div>
{#if assetCursor}<Button variant="outline" class="mt-3" onclick={()=>assetCursor=null}>First assets</Button>{/if}
{#if assets.value?.nextCursor}<Button variant="outline" class="mt-3" onclick={()=>assetCursor=assets.value?.nextCursor??null}>Next assets</Button>{/if}
<Card as="section" class="mt-5"><h2 class="section-title mb-3">Asset groups</h2>
  <ul class="grid gap-2 sm:grid-cols-2">{#each groups.value?.rows??[] as item (item.id)}<li class="flex min-w-0 items-center justify-between gap-2 border-b py-1">
    <a href={'/assets/groups/'+item.id} class="min-w-0 break-words hover:underline">{item.draft.name} · {item.assetIds.length} assets{item.draft.archived?' · Archived':''}</a>
    <RowActions itemName={item.draft.name} onEdit={()=>void goto('/assets/groups/'+item.id+'#asset-group-editor')}
      onDelete={()=>removal={id:item.id,name:item.draft.name,kind:'group'}}/>
  </li>{:else}<li class="text-muted-foreground">No asset groups found.</li>{/each}</ul>
  {#if groupCursor}<Button variant="outline" class="mt-3" onclick={()=>groupCursor=null}>First groups</Button>{/if}
  {#if groups.value?.nextCursor}<Button variant="outline" class="mt-3" onclick={()=>groupCursor=groups.value?.nextCursor??null}>Next groups</Button>{/if}
</Card>

{#if removal}
  <Modal confirm title={removal.kind==='asset'?'Delete asset?':'Delete asset group?'} close={()=>removal=undefined}>
    <p>Delete {removal.name}?</p><Feedback error={task.error}/>
    <Button variant="destructive" class="mt-3" disabled={task.busy} onclick={remove}>Confirm deletion</Button>
  </Modal>
{/if}
