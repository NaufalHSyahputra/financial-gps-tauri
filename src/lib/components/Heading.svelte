<script lang="ts">
  import type { Snippet } from 'svelte';
  let { title, subtitle, children }: {title:string;subtitle?:string;children?:Snippet}=$props();
</script>
<header class="mb-6 flex flex-wrap items-end justify-between gap-4">
  <div><h1 class="page-title">{title}</h1>{#if subtitle}<p class="mt-2 text-sm text-muted-foreground">{subtitle}</p>{/if}</div>
  {#if children}<div class="flex min-w-0 flex-wrap items-center gap-2 max-md:grid max-md:w-full max-md:grid-cols-2 max-md:[&>*:only-child]:col-span-2 max-md:[&>*]:min-w-0 max-md:[&>[data-slot=button]]:whitespace-normal">{@render children()}</div>{/if}
</header>
