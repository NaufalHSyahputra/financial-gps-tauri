<script lang="ts">
  import type { HTMLInputAttributes } from 'svelte/elements';
  import { cn } from '#lib/utils.js';

  let { checked = $bindable(false), class: classes, ...props }: Omit<HTMLInputAttributes, 'type'> = $props();
</script>

<input {...props} type="checkbox" bind:checked data-slot="checkbox"
  class={cn('size-4 shrink-0 accent-primary focus-visible:outline-2 focus-visible:outline-ring', classes)} />
