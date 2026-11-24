<script lang="ts">
  import { Label } from '#lib/components/ui/label/index.js';
  import { Input } from '#lib/components/ui/input/index.js';
  import { Button } from '#lib/components/ui/button/index.js';
  import { MediaQuery } from 'svelte/reactivity';
  import { calculateMinor } from '../calculator';
  import { formatMinor, minorToInput } from '../money';
  import { privacy } from '../stores/privacy.svelte';

  let { value = $bindable(''), math = $bindable(false), currency, scale, advance }: {
    value: string; math: boolean; currency: string; scale: number; advance: () => void;
  } = $props();
  const mobile = new MediaQuery('(width < 48rem)');
  let input = $state<HTMLInputElement | null>(null);
  let error = $state('');
  const preview = $derived.by(() => {
    try { return formatMinor({ currency, minor: value.trim() ? calculateMinor(value, scale, math) : '0' }, scale, privacy.locale(currency)); }
    catch { return '—'; }
  });
  function insert(key: string) {
    error = '';
    if (!input) return;
    const start = input.selectionStart ?? value.length;
    const end = input.selectionEnd ?? value.length;
    if (input !== document.activeElement) { value = (value + key).slice(0, 100); return; }
    value = (value.slice(0, start) + key + value.slice(end)).slice(0, 100);
    queueMicrotask(() => input?.setSelectionRange(start + key.length, start + key.length));
  }
  function equals() {
    try { value = minorToInput(calculateMinor(value, scale, math), scale); error = ''; }
    catch (cause) { error = (cause as Error).message; }
  }
  function toggle() {
    if (math && /[+*/×÷−-]/.test(value)) {
      try { value = minorToInput(calculateMinor(value, scale, true), scale); }
      catch (cause) { error = (cause as Error).message; return; }
    }
    math = !math;
    error = '';
  }
</script>

<div class="space-y-3 max-md:flex max-md:min-w-0 max-md:flex-1 max-md:flex-col">
  <Label class="field">Amount ({currency})
    <Input class="numeric text-xl" type={privacy.masked ? 'password' : 'text'} inputmode="decimal" maxlength={100} bind:ref={input}
      bind:value oninput={() => error = ''} onkeydown={event => {
        if (event.key === '=' && math) { event.preventDefault(); equals(); }
        if (event.key === 'Enter') { event.preventDefault(); advance(); }
      }} />
  </Label>
  <p class="numeric text-3xl font-semibold max-md:mb-4 max-md:text-4xl max-md:font-medium" aria-label="Formatted amount">{privacy.masked ? '••••' : preview}</p>
  <div class="amount-keys grid max-w-sm grid-cols-3 gap-2 max-md:mt-auto max-md:max-w-none" aria-label="Amount keypad" role="group">
    {#snippet controls()}
    <div class="col-span-3 flex items-center justify-end gap-2 md:contents">
      <Button variant="outline" type="button" onclick={() => { value = ''; error = ''; }}>Clear</Button>
      <Button variant="outline" type="button" class="md:col-span-2" aria-pressed={math} onclick={toggle}>Calculator</Button>
    </div>
    {#if math}
      <div class="col-span-3 grid grid-cols-5 gap-2 md:contents">
        {#each [['+', 'Add'],['−', 'Subtract'],['×', 'Multiply'],['÷','Divide']] as [key,label] (key)}
          <Button variant="outline" type="button" class="max-md:px-0" aria-label={label} onclick={() => insert(key)}>{key}</Button>
        {/each}
        <Button variant="outline" type="button" class="max-md:px-0" aria-label="Equals" onclick={equals}>=</Button>
      </div>
    {/if}
    {/snippet}
    {#if mobile.current}{@render controls()}{/if}
    {#each ['7','8','9','4','5','6','1','2','3',scale ? '.' : '000','0'] as key (key)}
      <Button variant="outline" type="button" class="min-h-10 max-md:text-lg" onclick={() => insert(key)}>{key}</Button>
    {/each}
    <Button variant="outline" type="button" aria-label="Backspace" onclick={() => { value = value.slice(0, -1); error = ''; }}>⌫</Button>
    {#if !mobile.current}{@render controls()}{/if}
  </div>
  {#if error}<p role="alert" class="error">{error}</p>{/if}
</div>
