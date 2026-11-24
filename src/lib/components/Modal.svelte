<script lang="ts">
  import XIcon from '@lucide/svelte/icons/x';
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
  <div class="modal-body min-h-0 overflow-y-auto overscroll-contain px-4 py-4 md:px-5">
    {@render children()}
    {#if !mobile.current && footer}<div class="mt-3 empty:hidden">{@render footer()}</div>{/if}
  </div>
  {#if mobile.current && footer}<div class="modal-footer empty:hidden">{@render footer()}</div>{/if}
{/snippet}

{#if mobile.current}
  <Drawer.Root open onOpenChange={change} shouldScaleBackground={false} handleOnly>
    <Drawer.Content>
      <Drawer.Header class="flex shrink-0 flex-row items-center justify-between gap-2 border-b border-border px-4 py-2 !text-left">
        <Drawer.Title>{title}</Drawer.Title>
        <Button type="button" variant="ghost" size="icon" class="shrink-0 max-md:size-11" onclick={close} aria-label="Close dialog"><XIcon class="size-4" aria-hidden /></Button>
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
          <AlertDialog.Cancel variant="ghost" size="icon" class="shrink-0 max-md:size-11" aria-label="Close dialog"><XIcon class="size-4" aria-hidden /></AlertDialog.Cancel>
        </AlertDialog.Header>
        {@render body()}
      </AlertDialog.Content>
    </AlertDialog.Root>
  {:else}
    <Dialog.Root open onOpenChange={change}>
      <Dialog.Content showCloseButton={false} class="flex max-h-[90dvh] w-full flex-col gap-0 p-0 sm:max-w-2xl">
        <Dialog.Header class="flex shrink-0 flex-row items-center justify-between border-b border-border px-5 py-2">
          <Dialog.Title>{title}</Dialog.Title>
          <Button type="button" variant="ghost" size="icon" class="shrink-0 max-md:size-11" onclick={close} aria-label="Close dialog"><XIcon class="size-4" aria-hidden /></Button>
        </Dialog.Header>
        {@render body()}
      </Dialog.Content>
    </Dialog.Root>
  {/if}
{/if}
