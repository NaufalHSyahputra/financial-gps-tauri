<script lang="ts">
  import XIcon from '@lucide/svelte/icons/x';
  import { MediaQuery } from 'svelte/reactivity';
  import { Button } from '#lib/components/ui/button/index.js';
  import { Input } from '#lib/components/ui/input/index.js';
  import * as Drawer from '#lib/components/ui/drawer/index.js';
  import * as Popover from '#lib/components/ui/popover/index.js';
  import { categoryIconGroups, resolveCategoryIcon } from './category-icons';
  import CategoryIcon from './CategoryIcon.svelte';

  let { value = $bindable<string | null>(null), name, kind }: {
    value?: string | null; name: string; kind: string;
  } = $props();
  const mobile = new MediaQuery('(width < 48rem)');
  let open = $state(false);
  let search = $state('');
  const choice = $derived(resolveCategoryIcon(name, kind, value));
  const matching = $derived.by(() => {
    const terms = search.trim().toLowerCase().split(/\s+/).filter(Boolean);
    return categoryIconGroups.map(group => ({
      ...group,
      icons: group.icons.filter(item => {
        const haystack = [group.label, item.id, item.label, ...item.keywords].join(' ').toLowerCase();
        return terms.every(term => haystack.includes(term));
      })
    })).filter(group => group.icons.length);
  });

  function choose(icon: string | null) {
    value = icon;
    open = false;
  }

  function moveFocus(event: KeyboardEvent) {
    const button = event.currentTarget as HTMLButtonElement;
    const parent = button.closest('[data-icon-options]');
    if (!parent) return;
    const buttons = Array.from(parent.querySelectorAll<HTMLButtonElement>('[data-icon-choice]'));
    const index = buttons.indexOf(button);
    const columns = getComputedStyle(button.parentElement!).gridTemplateColumns.split(' ').length;
    const next = event.key === 'ArrowLeft' ? index - 1 : event.key === 'ArrowRight' ? index + 1
      : event.key === 'ArrowUp' ? index - columns : event.key === 'ArrowDown' ? index + columns
      : event.key === 'Home' ? 0 : event.key === 'End' ? buttons.length - 1 : null;
    if (next === null) return;
    event.preventDefault();
    buttons[Math.max(0, Math.min(buttons.length - 1, next))]?.focus();
  }

  $effect(() => { if (!open) search = ''; });
</script>

{#snippet trigger()}
  <CategoryIcon {name} {kind} icon={value} size="sm"/>
  <span>Category icon <span class="block text-xs text-muted-foreground">{value ? choice.label : 'Automatic · ' + choice.label}</span></span>
{/snippet}

{#snippet picker()}
  <div class="grid shrink-0 gap-2 p-4">
    <Input type="search" aria-label="Search icons" placeholder="Search icons" bind:value={search}/>
    <Button type="button" variant="outline" aria-pressed={value === null} class="min-h-11 justify-start" onclick={()=>choose(null)}>
      Use automatic icon
    </Button>
  </div>
  <div class="min-h-0 space-y-4 overflow-y-auto px-4 pb-4" data-icon-options>
    {#each matching as group (group.label)}
      <section aria-label={group.label}>
        <h3 class="mb-2 text-sm font-medium text-muted-foreground">{group.label}</h3>
        <div class="grid grid-cols-3 gap-2">
          {#each group.icons as item (item.id)}
            <Button type="button" variant="ghost" class="min-h-11 min-w-11 flex-col gap-1 px-1 py-2 text-center"
              aria-pressed={value===item.id} data-icon-choice={item.id} onkeydown={moveFocus}
              onclick={()=>choose(item.id)}>
              <CategoryIcon {kind} icon={item.id} size="sm"/>
              <span class="text-xs/4">{item.label}</span>
            </Button>
          {/each}
        </div>
      </section>
    {:else}
      <p class="text-sm text-muted-foreground">No matching icons</p>
    {/each}
  </div>
{/snippet}

{#if mobile.current}
  <Drawer.NestedRoot bind:open shouldScaleBackground={false} handleOnly>
    <Drawer.Trigger>
      {#snippet child({ props })}
        <Button {...props} type="button" variant="outline" class="min-h-11 justify-start text-left">{@render trigger()}</Button>
      {/snippet}
    </Drawer.Trigger>
    <Drawer.Content class="z-[60]">
      <Drawer.Header class="flex shrink-0 flex-row items-center justify-between gap-2 border-b border-border px-4 py-2 !text-left">
        <Drawer.Title>Choose category icon</Drawer.Title>
        <Drawer.Close>
          {#snippet child({ props })}<Button {...props} type="button" variant="ghost" size="icon" class="shrink-0 max-md:size-11" aria-label="Close icon picker"><XIcon class="size-4" aria-hidden /></Button>{/snippet}
        </Drawer.Close>
      </Drawer.Header>
      {@render picker()}
    </Drawer.Content>
  </Drawer.NestedRoot>
{:else}
  <Popover.Root bind:open>
    <Popover.Trigger>
      {#snippet child({ props })}
        <Button {...props} type="button" variant="outline" class="min-h-11 justify-start text-left">{@render trigger()}</Button>
      {/snippet}
    </Popover.Trigger>
    <Popover.Content align="start" aria-label="Choose category icon"
      class="max-h-[min(34rem,72dvh,var(--bits-popover-content-available-height))] w-[min(26rem,calc(100vw-2rem))] gap-0 p-0">
      {@render picker()}
    </Popover.Content>
  </Popover.Root>
{/if}
