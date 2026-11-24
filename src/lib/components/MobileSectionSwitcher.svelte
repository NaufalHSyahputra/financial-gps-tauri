<script lang="ts" generics="Section extends string">
  import { Button } from '#lib/components/ui/button/index.js';

  let { label, options, selected = $bindable() }: {
    label: string;
    options: readonly (readonly [Section, string])[];
    selected: Section;
  } = $props();
</script>

<div class="mobile-section-nav grid grid-cols-4 md:hidden" role="group" aria-label={label}>
  {#each options as [key, title] (key)}
    <Button type="button" variant={selected === key ? 'default' : 'ghost'} aria-pressed={selected === key}
      class="min-h-11 min-w-11 px-1 text-xs leading-tight whitespace-normal" onclick={() => selected = key}>{title}</Button>
  {/each}
</div>
