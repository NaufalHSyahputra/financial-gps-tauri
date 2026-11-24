<script lang="ts">
  import type { HTMLInputAttributes } from 'svelte/elements';
  let { checked = false, ...props }: Omit<HTMLInputAttributes, 'type'> = $props();
</script>

<input {...props} {checked} type="radio" data-slot="radio"
  class="size-4 shrink-0 accent-primary" />
