<script lang="ts">
  import { Badge } from '#lib/components/ui/badge/index.js';
  import { Button } from '#lib/components/ui/button/index.js';
  import { CheckCircleFill } from '#lib/icons/registry.js';
  import { theme, themeFlavors, themeOptions, type ThemeFlavor } from '../stores/theme.svelte';

  const swatches = [
    ['base', 'Base'], ['surface0', 'Surface'], ['text', 'Text'], ['blue', 'Blue'],
    ['mauve', 'Mauve'], ['green', 'Green'], ['peach', 'Peach'], ['red', 'Red']
  ] as const;
  const id = $props.id();
</script>

{#snippet preview(flavor: ThemeFlavor, compact = false)}
  <span data-theme={flavor} class="block min-w-0 overflow-hidden rounded-lg border border-border bg-background text-foreground transition-colors duration-150 motion-reduce:transition-none">
    <span class="flex flex-wrap items-center gap-x-2 gap-y-1 border-b border-border bg-card px-2 py-1.5">
      <span aria-hidden="true" class="inline-flex gap-1">
        <span class="size-1.5 rounded-full bg-(--ctp-red)"></span>
        <span class="size-1.5 rounded-full bg-(--ctp-yellow)"></span>
        <span class="size-1.5 rounded-full bg-(--ctp-green)"></span>
      </span>
      <span class="text-[10px] font-semibold text-muted-foreground">{themeFlavors[flavor].name}</span>
    </span>
    <span class="block p-1.5">
      <span class="block rounded-md border border-border bg-background p-1.5">
        <span class="block text-[10px] leading-4 text-muted-foreground">Net balance</span>
        <span class="numeric block text-xs font-semibold leading-5">Rp 12.450.000</span>
        <span class="mt-1 flex flex-wrap gap-1 text-[10px] leading-4">
          <span class="numeric rounded-sm bg-money-in-soft px-1 text-money-in" title="Money in">+Rp 4.500.000</span>
          <span class="numeric rounded-sm bg-money-out-soft px-1 text-money-out" title="Money out">-Rp 850.000</span>
        </span>
      </span>
      <span class="mt-1.5 inline-block rounded-sm bg-primary px-1.5 py-0.5 text-[10px] font-semibold text-primary-foreground">Add transaction</span>
      <span role="img" aria-label={themeFlavors[flavor].name + ' palette: ' + swatches.map(([, label]) => label).join(', ')} class="mt-1.5 grid grid-cols-8 gap-0.5">
        {#each swatches as [color, label] (color)}
          <span title={label} class={['h-2 rounded-xs border border-foreground/20', compact && 'h-1.5']} data-swatch={color}></span>
        {/each}
      </span>
    </span>
  </span>
{/snippet}

<div class="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
  {#each themeOptions as option (option.value)}
    {@const selected = theme.preference === option.value}
    <div class="relative min-w-0">
      <Button variant="outline" type="button" aria-label={option.name} aria-pressed={selected}
        aria-describedby={id + '-' + option.value}
        onclick={()=>theme.select(option.value)}
        class={['h-full w-full min-w-0 flex-col items-stretch justify-start gap-2 whitespace-normal rounded-xl bg-background p-2.5 text-left transition-colors duration-150 motion-reduce:transition-none',
          selected && 'border-primary ring-1 ring-primary']}>
        <span class="flex min-w-0 items-center gap-1.5">
          <span class="mr-auto leading-5">{option.name}</span>
          {#if option.value !== 'system'}<Badge variant="secondary">{option.mode}</Badge>{/if}
          {#if selected}<span aria-hidden="true" class="shrink-0 text-primary-ink"><CheckCircleFill class="size-4"/></span>{/if}
        </span>
        {#if option.value === 'system'}
          <span class="text-xs text-muted-foreground">{option.mode}</span>
          <span class="grid grid-cols-2 items-start gap-1.5">
            {@render preview('latte', true)}
            {@render preview('mocha', true)}
          </span>
        {:else}
          {@render preview(option.value)}
        {/if}
      </Button>
      <span class="sr-only" id={id + '-' + option.value}>
        {option.value === 'system' ? 'Follows your device with Latte and Mocha previews.' : option.mode + ' theme.'}
        Preview palette swatches {swatches.map(([, label]) => label).join(', ')}.
      </span>
    </div>
  {/each}
</div>

<style>
  [data-swatch="base"] { background: var(--ctp-base); }
  [data-swatch="surface0"] { background: var(--ctp-surface0); }
  [data-swatch="text"] { background: var(--ctp-text); }
  [data-swatch="blue"] { background: var(--ctp-blue); }
  [data-swatch="mauve"] { background: var(--ctp-mauve); }
  [data-swatch="green"] { background: var(--ctp-green); }
  [data-swatch="peach"] { background: var(--ctp-peach); }
  [data-swatch="red"] { background: var(--ctp-red); }
</style>
