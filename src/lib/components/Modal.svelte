<script lang="ts">
  import { MediaQuery } from 'svelte/reactivity';
  import type { Snippet } from 'svelte';
  import { Button } from '#lib/components/ui/button/index.js';
  import * as Dialog from '#lib/components/ui/dialog/index.js';
  import * as Drawer from '#lib/components/ui/drawer/index.js';
  import * as AlertDialog from '#lib/components/ui/alert-dialog/index.js';

  let { title, close, children, footer, mobileOnly = false, confirm = false }:
    { title:string; close:()=>void; children:Snippet; footer?:Snippet; mobileOnly?:boolean; confirm?:boolean } = $props();
  const mobile = new MediaQuery('(width < 48rem)');
  const change = (open: boolean) => { if (!open) close(); };
  $effect(() => { if (mobileOnly && !mobile.current) close(); });
</script>

{#snippet body()}
  <div class="modal-body min-h-0 overflow-y-auto overscroll-contain px-4 py-4 md:px-5">{@render children()}</div>
  {#if footer}<div class="modal-footer shrink-0 border-t border-border px-4 pt-3 md:px-5">{@render footer()}</div>{/if}
{/snippet}

{#if mobile.current}
  <Drawer.Root open onOpenChange={change} shouldScaleBackground={false} handleOnly>
    <Drawer.Content>
      <Drawer.Header class="flex shrink-0 items-center justify-between border-b border-border px-4 py-1 text-left">
        <Drawer.Title>{title}</Drawer.Title>
        <Button type="button" variant="ghost" onclick={close} aria-label="Close dialog">×</Button>
      </Drawer.Header>
      {@render body()}
    </Drawer.Content>
  </Drawer.Root>
{:else if !mobileOnly}
  {#if confirm}
    <AlertDialog.Root open onOpenChange={change}>
      <AlertDialog.Content class="data-[size=default]:max-w-[min(36rem,calc(100%-2rem))]! flex max-h-[90dvh] w-full flex-col gap-0 p-0 sm:max-w-xl">
        <AlertDialog.Header class="flex shrink-0 flex-row items-center justify-between border-b border-border px-5 py-2">
          <AlertDialog.Title>{title}</AlertDialog.Title>
          <AlertDialog.Cancel variant="ghost" aria-label="Close dialog">×</AlertDialog.Cancel>
        </AlertDialog.Header>
        {@render body()}
      </AlertDialog.Content>
    </AlertDialog.Root>
  {:else}
    <Dialog.Root open onOpenChange={change}>
      <Dialog.Content showCloseButton={false} class="flex max-h-[90dvh] w-full flex-col gap-0 p-0 sm:max-w-2xl">
        <Dialog.Header class="flex shrink-0 flex-row items-center justify-between border-b border-border px-5 py-2">
          <Dialog.Title>{title}</Dialog.Title>
          <Button type="button" variant="ghost" onclick={close} aria-label="Close dialog">×</Button>
        </Dialog.Header>
        {@render body()}
      </Dialog.Content>
    </Dialog.Root>
  {/if}
{/if}
