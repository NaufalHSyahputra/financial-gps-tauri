<script lang="ts">
  import { MediaQuery } from 'svelte/reactivity';
  import * as Command from '#lib/components/ui/command/index.js';
  import { goto } from '$app/navigation';
  import { commands, type SearchHit } from '../drivers/runtime';
  import { hitPath, navigation } from '../navigation';
  import { useLauncher } from '../launcher.svelte';
  import { session, pageRequest } from '../stores/session.svelte';
  import { privacy } from '../stores/privacy.svelte';
  import { theme, themeOptions } from '../stores/theme.svelte';
  import Modal from './Modal.svelte';

  let { close, openThemes }: {close:()=>void;openThemes:()=>void}=$props();
  const launcher=useLauncher();
  let query=$state('');
  let hits=$state<SearchHit[]>([]);
  const mobile = new MediaQuery('(width < 48rem)');
  const entries=$derived([
    ...navigation.flatMap(section => section.items).filter(([,path])=>privacy.visible(path.split('/')[1]))
      .map(([label,path])=>({label,group:'Navigation', run:()=>goto(path)})),
    {label:'New transaction',group:'Create',run:()=>goto('/transactions/new')},
    {label:'New transfer',group:'Create',run:()=>goto('/transactions/new?transfer=1')},
    {label:'Theme previews',group:'Appearance',run:openThemes},
    ...themeOptions.map(option=>({label:'Theme: '+option.name,group:'Appearance',run:()=>theme.select(option.value)})),
    ...launcher.ledgers.map(item=>({label:item.label, group:'Switch activity',
      run:()=>launcher.run(()=>launcher.open({ledgerInstanceId:item.ledgerInstanceId}))}))
  ]);
  const results=$derived([...entries.filter(item=>item.label.toLowerCase().includes(query.trim().toLowerCase())),
    ...hits.filter(item=>privacy.visible(hitPath(item.kind,item.id).split('/')[1]))
      .map(item=>({label:item.label,group:item.kind,run:()=>goto(hitPath(item.kind,item.id))}))].slice(0,18));
  $effect(() => {
    const text=query.trim();
    hits=[];
    if (text.length<2 || !session.active) return;
    let active=true;
    const timer=setTimeout(async()=>{
      try {
        const result=await session.call(commands.searchGlobal(session.scope,{query:text,page:pageRequest(null,15)}));
        if (active) hits=result.rows;
      } catch { /* Local actions remain available. */ }
    },200);
    return ()=>{active=false;clearTimeout(timer);};
  });
  function change(open: boolean) { if (!open) close(); }
</script>

{#snippet items()}
  <Command.Input bind:value={query} placeholder="Search" aria-label="Search commands and this activity" />
  <Command.List aria-label="Search results" class="p-2">
    {#each results as item, index (index)}
      <Command.Item value={item.group + ':' + index + ':' + item.label} class="min-h-9 rounded-lg data-selected:bg-accent max-md:min-h-11"
        onSelect={()=>{close(); void item.run();}}>
        <span class="min-w-0 flex-1 truncate">{item.label}</span>
        <span class="text-xs text-muted-foreground">{item.group}</span>
      </Command.Item>
    {:else}
      <p class="p-3 text-sm text-muted-foreground">No matching commands or records.</p>
    {/each}
  </Command.List>
{/snippet}

{#if mobile.current}
  <Modal title="Command palette" {close}>
    <Command.Root class="bg-transparent" shouldFilter={false}>{@render items()}</Command.Root>
  </Modal>
{:else}
  <Command.Dialog open onOpenChange={change} title="Command palette"
    description="Search commands and this activity" shouldFilter={false} showCloseButton class="top-1/2! -translate-y-1/2! pr-8 sm:max-w-xl">
    {@render items()}
  </Command.Dialog>
{/if}
