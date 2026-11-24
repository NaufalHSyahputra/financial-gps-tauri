<script lang="ts">
  import type { Snippet } from 'svelte';
  let { title, subtitle, children }: {title:string;subtitle?:string;children?:Snippet}=$props();
</script>
<header class="mb-6 flex flex-wrap items-end justify-between gap-4">
  <div><h1 class="page-title">{title}</h1>{#if subtitle}<p class="mt-2 text-sm text-muted-foreground">{subtitle}</p>{/if}</div>
  {#if children}<div class="flex flex-wrap items-center gap-2">{@render children()}</div>{/if}
</header>
