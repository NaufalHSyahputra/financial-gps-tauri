<script lang="ts">
  import { Button } from '#lib/components/ui/button/index.js';
  import { Label } from '#lib/components/ui/label/index.js';
  import { RadioInput } from '#lib/components/ui/input/index.js';
  import { NativeCheckbox } from '#lib/components/ui/checkbox/index.js';
  import {
    categoryPresets,
    defaultGroupKeysForPreset,
    getCategoryPreset,
    type PresetId,
    type PresetLocale,
  } from '#lib/presets/category-presets';

  let {
    presetId = $bindable<PresetId>('indonesia-personal'),
    locale = $bindable<PresetLocale>('en'),
    selectedGroupKeys = $bindable<string[]>(defaultGroupKeysForPreset('indonesia-personal')),
    allowBlank = true,
    heading = 'Starter categories',
  }: {
    presetId?: PresetId;
    locale?: PresetLocale;
    selectedGroupKeys?: string[];
    allowBlank?: boolean;
    heading?: string;
  } = $props();

  let customizeOpen = $state(false);

  const visiblePresets = $derived(
    allowBlank ? categoryPresets : categoryPresets.filter(preset => preset.id !== 'blank')
  );
  const currentPreset = $derived(getCategoryPreset(presetId));

  function selectPreset(next: PresetId) {
    presetId = next;
    selectedGroupKeys = defaultGroupKeysForPreset(next);
    if (next === 'blank') customizeOpen = false;
  }

  function toggleGroup(key: string, checked: boolean) {
    if (checked) {
      if (!selectedGroupKeys.includes(key)) {
        selectedGroupKeys = [...selectedGroupKeys, key];
      }
    } else {
      selectedGroupKeys = selectedGroupKeys.filter(item => item !== key);
    }
  }
</script>

<fieldset class="grid min-w-0 gap-3 rounded-lg border border-line px-3 pb-3 pt-2">
  <legend class="px-1 font-semibold">{heading}</legend>

  <div class="flex flex-wrap items-center justify-between gap-2">
    <span class="text-xs font-medium text-muted-foreground">Preset language</span>
    <div class="inline-flex rounded-lg border border-line bg-ground p-0.5" role="group" aria-label="Preset language">
      <Button
        type="button"
        size="sm"
        variant={locale === 'en' ? 'default' : 'ghost'}
        class="min-h-11 px-3 text-xs"
        aria-pressed={locale === 'en'}
        onclick={() => (locale = 'en')}
      >
        English
      </Button>
      <Button
        type="button"
        size="sm"
        variant={locale === 'id' ? 'default' : 'ghost'}
        class="min-h-11 px-3 text-xs"
        aria-pressed={locale === 'id'}
        onclick={() => (locale = 'id')}
      >
        Bahasa Indonesia
      </Button>
    </div>
  </div>

  <div class="grid gap-2">
    {#each visiblePresets as preset (preset.id)}
      <Label class="flex min-h-11 cursor-pointer items-start gap-3 rounded-lg border border-line bg-paper p-3 has-checked:border-accent has-checked:bg-ground">
        <RadioInput
          name="category-preset"
          value={preset.id}
          checked={presetId === preset.id}
          onchange={() => selectPreset(preset.id)}
        />
        <span class="min-w-0">
          <span class="block font-medium text-ink">{preset.name[locale]}</span>
          <span class="block text-xs text-muted-foreground">{preset.description[locale]}</span>
        </span>
      </Label>
    {/each}
  </div>

  {#if currentPreset.groups.length}
    <div class="border-t border-line pt-2">
      <Button
        type="button"
        variant="ghost"
        size="sm"
        class="min-h-11 w-full justify-between px-2 text-sm"
        aria-expanded={customizeOpen}
        onclick={() => (customizeOpen = !customizeOpen)}
      >
        <span>Customize groups ({selectedGroupKeys.length} of {currentPreset.groups.length})</span>
        <span class="text-xs text-muted-foreground">{customizeOpen ? 'Hide' : 'Show'}</span>
      </Button>

      {#if customizeOpen}
        <div class="mt-2 grid gap-1.5 pl-1">
          {#each currentPreset.groups as group (group.key)}
            <Label class="check min-h-11 rounded-md px-2 py-1.5 hover:bg-ground">
              <NativeCheckbox
                checked={selectedGroupKeys.includes(group.key)}
                onchange={event => toggleGroup(group.key, event.currentTarget.checked)}
              />
              <span class="min-w-0 flex-1 text-sm">
                <span class="font-medium text-ink">{group.name[locale]}</span>
                <span class="ml-1 text-xs text-muted-foreground">({group.categories.length})</span>
              </span>
            </Label>
          {/each}
        </div>
      {/if}
    </div>
  {/if}
</fieldset>
