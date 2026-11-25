<script lang="ts">
  import { Card } from '#lib/components/ui/card/index.js';
  import { Label } from '#lib/components/ui/label/index.js';
  import { NativeSelect } from '#lib/components/ui/native-select/index.js';
  import { Input, RadioInput } from '#lib/components/ui/input/index.js';
  import { Button } from '#lib/components/ui/button/index.js';
  import { NativeCheckbox } from '#lib/components/ui/checkbox/index.js';
  import { commands, type AccountDraft_Deserialize, type CalendarInput, type Money } from '../drivers/runtime';
  import { accountOptions, setupCurrencies } from '../account-types';
  import { formatMinor, toMinor } from '../money';
  import { privacy } from '../stores/privacy.svelte';
  import { session, message } from '../stores/session.svelte';
  import AccountTypeIcon from '#lib/icons/AccountTypeIcon.svelte';
  import Feedback from '../components/Feedback.svelte';
  import CategoryPresetSelector from '../components/CategoryPresetSelector.svelte';
  import {
    applyCategoryPreset,
    defaultGroupKeysForPreset,
    type PresetId,
    type PresetLocale,
  } from '../presets/category-presets';

  let { initial = false, base = 'IDR', busy = false, error = '', locked = false, complete }: {
    initial?: boolean; base?: string; busy?: boolean; error?: string; locked?: boolean;
    complete: (draft: AccountDraft_Deserialize, startingBalance: Money, calendar?: CalendarInput) => void;
  } = $props();
  let step = $state(1);
  let currency = $state('');
  const code = $derived(initial ? currency || 'IDR' : base);
  const scale = $derived(setupCurrencies.find(item => item.code === code)?.scale ?? 0);
  let selected = $state(0);
  const option = $derived(accountOptions[selected]);
  let name = $state('');
  let balance = $state('');
  let setupCalendar = $state(false);
  let anchor = $state(25);
  let timezone = $state('Asia/Jakarta');
  let localError = $state('');
  let hasZeroCategories = $state(false);
  let presetExpanded = $state(false);
  let presetId = $state<PresetId>('indonesia-personal');
  let presetLocale = $state<PresetLocale>('en');
  let selectedGroupKeys = $state<string[]>(defaultGroupKeysForPreset('indonesia-personal'));
  let presetBusy = $state(false);
  let presetNotice = $state('');

  $effect(() => {
    const active = session.active;
    if (!active) {
      hasZeroCategories = false;
      return;
    }
    let cancelled = false;
    void commands
      .categoriesList(active.scope, { includeArchived: true, page: { cursor: null, limit: 1 } })
      .then(result => {
        if (!cancelled && result.status === 'ok') {
          hasZeroCategories = result.data.rows.length === 0;
        }
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  });

  async function applyStarterCategories() {
    if (presetBusy || !selectedGroupKeys.length) return;
    presetBusy = true;
    localError = '';
    try {
      const result = await applyCategoryPreset({
        scope: session.scope,
        presetId,
        locale: presetLocale,
        selectedGroupKeys,
      });
      hasZeroCategories = false;
      presetExpanded = false;
      presetNotice = `Added ${result.groupsCreated} category groups and ${result.categoriesCreated} categories.`;
    } catch (cause) {
      localError = message(cause);
    } finally {
      presetBusy = false;
    }
  }
  const preview = $derived.by(() => {
    try { return formatMinor({ currency: code, minor: toMinor(balance.trim().replace(',', '.') || '0', scale) }, scale, privacy.locale(code)); }
    catch { return '—'; }
  });
  function next(event: SubmitEvent) {
    event.preventDefault();
    localError = '';
    if (step === 2 && !name.trim()) { localError = 'Give your account a name.'; return; }
    if (step < 3) { step++; return; }
    try {
      const minor = toMinor(balance.trim().replace(',', '.') || '0', scale);
      if (BigInt(minor) < -9_000_000_000_000n || BigInt(minor) > 9_000_000_000_000n) throw new Error('Enter a smaller starting balance.');
      complete({ name: name.trim(), kind: option.kind, currencyCode: code, paymentMethod: option.wallet ? 'Digital wallet' : null }, { currency: code, minor }, initial && code === 'IDR' && setupCalendar ? { anchor, timezone, weekdayPolicy: 'none' } : undefined);
    } catch (cause) { localError = (cause as Error).message; }
  }
</script>

<Card as="section" class="mx-auto max-w-2xl" aria-labelledby="account-setup-heading">
  <p class="mb-2 text-sm font-semibold text-muted-foreground">Step {step} of 3</p>
  <h2 id="account-setup-heading" class="page-title">
    {step === 1 ? initial ? 'Welcome to Financial GPS' : 'Add an account' : step === 2 ? 'Where do you keep your money?' : 'What’s the starting balance?'}
  </h2>
  <form class="mt-4 space-y-5" onsubmit={next}>
    <fieldset class="space-y-4" disabled={busy}>
      {#if step === 1}
        <p class="text-muted-foreground">{initial ? 'Start with one account and a currency you use every day.' : 'Add a place where you keep or borrow money.'}</p>
        {#if initial}
          <Label class="field">Primary currency
            <NativeSelect value={code} onchange={event => currency = event.currentTarget.value}>
              {#each setupCurrencies as item (item.code)}<option value={item.code}>{item.code} · {item.name}</option>{/each}
            </NativeSelect>
          </Label>
        {:else}<p>Primary currency <strong>{code}</strong></p>{/if}
      {:else if step === 2}
        <fieldset class="grid gap-2 sm:grid-cols-2">
          <legend class="mb-2 font-semibold">Account type</legend>
          {#each accountOptions as item, index (item.label)}
            <Label class="flex cursor-pointer gap-3 rounded-lg border border-line bg-paper p-3 has-checked:border-accent has-checked:bg-ground">
              <RadioInput name="setup-account-kind" value={index} checked={selected===index} onchange={()=>selected=index} />
              <AccountTypeIcon kind={item.kind} wallet={'wallet' in item}/>
              <span><strong>{item.label}</strong><span class="block text-sm text-muted-foreground">{item.description}</span></span>
            </Label>
          {/each}
        </fieldset>
        <Label class="field">Account name<Input required maxlength={256} bind:value={name} placeholder={option.example}/></Label>
      {:else}
        <Label class="field">Starting balance ({code})<Input type={privacy.masked ? 'password' : 'text'} inputmode="decimal" bind:value={balance} placeholder="0" maxlength={23} /></Label>
        <p class="text-sm text-muted-foreground">Leave it blank to start at zero.</p>
        <dl class="grid gap-2 rounded-lg bg-ground p-4 sm:grid-cols-2">
          <div><dt class="text-sm text-muted-foreground">Account</dt><dd>{name.trim()} · {option.label}</dd></div>
          <div><dt class="text-sm text-muted-foreground">Starting balance</dt><dd class="numeric">{privacy.masked ? '••••' : preview}</dd></div>
        </dl>
        {#if initial && code === 'IDR'}
          <Label class="check"><NativeCheckbox bind:checked={setupCalendar} /> Set up a payday cycle</Label>
          {#if setupCalendar}
            <div class="form-grid">
              <Label class="field">Payday<Input type="number" min={1} max={31} bind:value={anchor} required /></Label>
              <Label class="field">Financial timezone<NativeSelect bind:value={timezone}>
                <option value="Asia/Jakarta">Jakarta (WIB)</option>
                <option value="Asia/Makassar">Makassar (WITA)</option>
                <option value="Asia/Jayapura">Jayapura (WIT)</option>
              </NativeSelect></Label>
            </div>
          {/if}
        {/if}
        {#if hasZeroCategories || presetNotice}
          <div class="rounded-lg border border-line bg-ground p-4 space-y-3">
            <div class="flex flex-wrap items-center justify-between gap-2">
              <div>
                <p class="font-medium text-ink">Starter categories</p>
                <p class="text-sm text-muted-foreground">
                  {presetNotice || 'This activity has no categories yet. Apply a starter pack before finishing setup.'}
                </p>
              </div>
              {#if hasZeroCategories}
                <Button
                  type="button"
                  variant="outline"
                  class="min-h-11"
                  onclick={() => (presetExpanded = !presetExpanded)}
                >
                  {presetExpanded ? 'Hide templates' : 'Choose template'}
                </Button>
              {/if}
            </div>
            {#if hasZeroCategories && presetExpanded}
              <CategoryPresetSelector
                allowBlank={false}
                bind:presetId
                bind:locale={presetLocale}
                bind:selectedGroupKeys
              />
              <Button
                type="button"
                variant="secondary"
                class="min-h-11"
                disabled={presetBusy || !selectedGroupKeys.length}
                onclick={applyStarterCategories}
              >
                {presetBusy ? 'Applying categories…' : 'Apply starter categories'}
              </Button>
            {/if}
          </div>
        {/if}
      {/if}
      <Feedback error={localError || error}/>
      <div class="flex flex-wrap gap-2 max-md:grid max-md:w-full max-md:grid-cols-2 max-md:[&>*:only-child]:col-span-2">
        {#if step > 1}<Button variant="outline" type="button" onclick={() => { step--; localError = ''; }} disabled={locked && step === 2}>Back</Button>{/if}
        <Button variant="default"  type="submit">{step < 3 ? 'Continue' : initial ? 'Start Tracking' : 'Save account'}</Button>
      </div>
    </fieldset>
  </form>
</Card>
