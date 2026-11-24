<script lang="ts" module>
  import type { Component } from 'svelte';

  export interface RowOverflowAction {
    id: string;
    label: string;
    icon?: Component<{ class?: string }>;
    onclick: () => void;
    disabled?: boolean;
    destructive?: boolean;
    separatorBefore?: boolean;
  }
</script>

<script lang="ts">
  import { tick } from 'svelte';
  import { MediaQuery } from 'svelte/reactivity';
  import type { HTMLButtonAttributes } from 'svelte/elements';
  import { Button, buttonVariants } from '#lib/components/ui/button/index.js';
  import * as Menu from '#lib/components/ui/dropdown-menu/index.js';
  import * as Tooltip from '#lib/components/ui/tooltip/index.js';
  import { rowActionIcons } from '#lib/icons/registry.js';
  import Modal from './Modal.svelte';

  let {
    itemName, onEdit, editLabel = 'Edit', editDisabled, onDelete, deleteLabel = 'Delete',
    deleteDisabled, extraActions = [], moreLabel = 'More actions'
  }: {
    itemName?: string; onEdit?: () => void; editLabel?: string; editDisabled?: boolean;
    onDelete?: () => void; deleteLabel?: string; deleteDisabled?: boolean;
    extraActions?: RowOverflowAction[]; moreLabel?: string;
  } = $props();

  const mobile = new MediaQuery('(width < 48rem)');
  const visibleExtraActions = $derived(extraActions);
  const moreName = $derived(accessibleName(moreLabel, itemName));
  let sheet = $state(false);
  let menu = $state(false);
  let trigger = $state<HTMLButtonElement | null>(null);
  const control = 'row-action tactile shrink-0 rounded-lg text-muted-foreground max-md:min-h-11 max-md:min-w-11';

  function accessibleName(label: string, name?: string) {
    return name ? label + ' ' + name : label;
  }

  function isolate(event: Event) {
    event.stopPropagation();
    event.preventDefault();
  }

  function closeSheet() {
    sheet = false;
    void tick().then(() => trigger?.focus());
  }

  function choose(action: RowOverflowAction) {
    sheet = false;
    menu = false;
    void tick().then(action.onclick);
  }
</script>

{#snippet direct(label: string, action: () => void, disabled: boolean | undefined, danger = false)}
  {@const Icon = danger ? rowActionIcons.delete : rowActionIcons.edit}
  <Tooltip.Root disabled={mobile.current}>
    <Tooltip.Trigger>
      {#snippet child({ props })}
        <Button {...props} variant="ghost" size="icon" {disabled} aria-label={accessibleName(label, itemName)}
          class={[control, danger && 'row-action-delete']}
          onkeydown={event=>{ (props.onkeydown as ((event: KeyboardEvent) => void) | undefined)?.(event); event.stopPropagation(); }}
          onpointerdown={event=>event.stopPropagation()}
          onclick={event=>{ isolate(event); action(); }}>
          <span aria-hidden="true"><Icon class="size-[18px]"/></span>
        </Button>
      {/snippet}
    </Tooltip.Trigger>
    {#if !mobile.current}<Tooltip.Content>{label}</Tooltip.Content>{/if}
  </Tooltip.Root>
{/snippet}

{#snippet overflowTrigger(props: HTMLButtonAttributes)}
  <button {...props} bind:this={trigger} type="button" data-slot="button"
    class={buttonVariants({ variant:'ghost', size:'icon', class:control })} aria-label={moreName}
    onpointerdown={event=>{ props.onpointerdown?.(event); event.stopPropagation(); }}
    onpointerup={event=>{ props.onpointerup?.(event); event.stopPropagation(); }}
    onkeydown={event=>{ props.onkeydown?.(event); event.stopPropagation(); }}
    onclick={event=>{ props.onclick?.(event); isolate(event); if (mobile.current) sheet = true; }}>
    <span aria-hidden="true"><rowActionIcons.more class="size-[18px]"/></span>
  </button>
{/snippet}

{#if onEdit || onDelete || visibleExtraActions.length}
  <div class="inline-flex shrink-0 items-center justify-end gap-1 whitespace-nowrap" role="group"
    aria-label={itemName ? 'Actions for ' + itemName : 'Row actions'} data-row-actions>
    <Tooltip.Provider delayDuration={300}>
      {#if onEdit}{@render direct(editLabel, onEdit, editDisabled)}{/if}
      {#if onDelete}{@render direct(deleteLabel, onDelete, deleteDisabled, true)}{/if}
      {#if visibleExtraActions.length}
        {#if mobile.current}
          {@render overflowTrigger({ 'aria-haspopup': 'dialog', 'aria-expanded': sheet })}
        {:else}
          <Menu.Root bind:open={menu}>
            <Tooltip.Root disabled={menu}>
              <Menu.Trigger>
                {#snippet child({ props: menuProps })}
                  <Tooltip.Trigger {...menuProps}>
                    {#snippet child({ props })}{@render overflowTrigger(props)}{/snippet}
                  </Tooltip.Trigger>
                {/snippet}
              </Menu.Trigger>
              <Tooltip.Content>{moreLabel}</Tooltip.Content>
            </Tooltip.Root>
            <Menu.Content align="end" class="z-[60] max-w-[calc(100vw-2rem)]">
              {#each visibleExtraActions as action (action.id)}
                {#if action.separatorBefore}<Menu.Separator/>{/if}
                <Menu.Item disabled={action.disabled} variant={action.destructive ? 'destructive' : 'default'}
                  onclick={event=>event.stopPropagation()} onSelect={()=>choose(action)}>
                  {#if action.icon}<action.icon class="size-[18px]"/>{/if}{action.label}
                </Menu.Item>
              {/each}
            </Menu.Content>
          </Menu.Root>
        {/if}
      {/if}
    </Tooltip.Provider>
  </div>
{/if}

{#if sheet && mobile.current}
  <Modal title={itemName ?? moreLabel} close={closeSheet} mobileOnly>
    <div class="grid gap-1">
      {#each visibleExtraActions as action (action.id)}
        {#if action.separatorBefore}<hr class="my-1"/>{/if}
        <Button variant="ghost" class={['tactile row-menu-option w-full justify-start whitespace-normal text-left',
          action.destructive && 'text-destructive']} disabled={action.disabled}
          onclick={event=>{ isolate(event); choose(action); }}>
          {#if action.icon}<action.icon class="size-[18px]"/>{/if}{action.label}
        </Button>
      {/each}
    </div>
  </Modal>
{/if}
