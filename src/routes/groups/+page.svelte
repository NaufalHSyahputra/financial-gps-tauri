<script lang="ts">
  import RowActions from '#lib/components/RowActions.svelte';
  import { Card } from '#lib/components/ui/card/index.js';
  import { Button } from '#lib/components/ui/button/index.js';
  import { Badge } from '#lib/components/ui/badge/index.js';
  import { goto } from '$app/navigation';
  import { commands } from '../../lib/drivers/runtime';
  import { session, pageRequest } from '../../lib/stores/session.svelte';
  import { createTask, query } from '../../lib/stores/task.svelte';
  import Heading from '../../lib/components/Heading.svelte';
  import Feedback from '../../lib/components/Feedback.svelte';
  import GroupSettings from '../../lib/views/GroupSettings.svelte';
  let cursor=$state<string|null>(null), create=$state(false);
  const task=createTask();
  const groups=query(()=>session.call(commands.groupsList(session.scope,{page:pageRequest(cursor,48)})));
</script>
<Heading title="Shared groups" subtitle="Split expenses and keep track of who owes whom."><Button variant="default"  onclick={()=>create=true}>New group</Button></Heading>
<Feedback error={groups.error||task.error} loading={groups.loading}/>
<div class="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
  {#each groups.value?.rows??[] as row (row.id)}
    <Card as="article" class="relative hover:border-status">
      <div class="flex flex-wrap items-center justify-between gap-2"><h2 class="section-title"><a href={'/groups/'+row.id} class="hover:underline after:absolute after:inset-0">{row.name}</a></h2><Badge variant="secondary">{row.archived?'Archived':row.kind.replace(/([A-Z])/g,' $1')}</Badge></div>
      <div class="pointer-events-none relative mt-2 flex justify-end [&>[data-row-actions]]:pointer-events-auto"><RowActions itemName={row.name} onEdit={()=>void goto('/groups/'+row.id+'?edit=1')}
        extraActions={[
          {id:'view',label:'View balances and expenses',onclick:()=>void goto('/groups/'+row.id)},
          {id:'archive',label:row.archived?'Restore group':'Archive group',disabled:task.busy,onclick:()=>void task.run(async()=>{
            await session.call(commands.groupArchive(session.mutation(),{id:row.id,archived:!row.archived})); groups.refresh();
          })}
        ]}/></div>
    </Card>
  {:else}{#if !groups.loading}<Card as="p" class="">No shared groups found.</Card>{/if}{/each}
</div>
{#if cursor}<Button variant="outline" class="mt-3" onclick={()=>cursor=null}>First groups</Button>{/if}{#if groups.value?.nextCursor}<Button variant="outline" class="mt-3" onclick={()=>cursor=groups.value?.nextCursor??null}>Next groups</Button>{/if}
{#if create}<GroupSettings close={()=>create=false} saved={id=>{create=false;void goto('/groups/'+id);}}/>{/if}
