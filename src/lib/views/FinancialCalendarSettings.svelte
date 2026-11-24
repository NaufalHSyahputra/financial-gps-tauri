<script lang="ts">
  import { Button } from '#lib/components/ui/button/index.js';
  import { Card } from '#lib/components/ui/card/index.js';
  import { Input } from '#lib/components/ui/input/index.js';
  import { Label } from '#lib/components/ui/label/index.js';
  import { NativeSelect } from '#lib/components/ui/native-select/index.js';
  import Feedback from '#lib/components/Feedback.svelte';
  import { commands, type CalendarInput } from '#lib/drivers/runtime';
  import { session } from '#lib/stores/session.svelte';
  import { createTask, query } from '#lib/stores/task.svelte';

  const calendar = query(() => { session.financialDateTick; return session.call(commands.financialCalendarGet(session.scope)); });
  const task = createTask();
  let timezone = $state('Asia/Jakarta');
  let anchor = $state(25);
  let weekdayPolicy = $state<CalendarInput['weekdayPolicy']>('none');
  $effect(() => {
    const settings = calendar.value?.pending?.settings ?? calendar.value?.settings;
    if (settings) ({ timezone, anchor, weekdayPolicy } = settings);
  });

  function save(settings: CalendarInput) {
    void task.run(async () => {
      await session.call(commands.financialCalendarSave(session.mutation(), settings));
      calendar.refresh();
    });
  }
</script>

<Card as="section" id="financial-calendar" aria-labelledby="settings-calendar-sync">
  <h2 id="settings-calendar-sync" class="settings-section section-title">Payday calendar</h2>
  <p class="mt-1 text-sm text-muted-foreground">Set the day your spending cycle starts. The date clamps to the end of shorter months before any weekend adjustment.</p>
  <Feedback error={task.error || calendar.error} loading={calendar.loading} />
  {#if calendar.value}
    <p class="mt-3 text-sm">Current cycle: <strong>{calendar.value.current.start} to {calendar.value.current.end}</strong></p>
    {#if calendar.value.pending}
      <div class="mt-2 flex flex-wrap items-center gap-2 text-sm">
        <span>Change scheduled for {calendar.value.pending.effectiveFrom}.</span>
        <Button variant="outline" size="sm" disabled={task.busy} onclick={() => calendar.value && save(calendar.value.settings)}>Cancel scheduled change</Button>
      </div>
    {/if}
  {/if}
  <form class="mt-4 space-y-3" onsubmit={(event) => { event.preventDefault(); save({ timezone, anchor, weekdayPolicy }); }}>
    <div class="form-grid">
      <Label class="field">Financial timezone
        <NativeSelect bind:value={timezone}>
          <option value="Asia/Jakarta">Jakarta (WIB)</option>
          <option value="Asia/Makassar">Makassar (WITA)</option>
          <option value="Asia/Jayapura">Jayapura (WIT)</option>
        </NativeSelect>
      </Label>
      <Label class="field">Payday<Input type="number" min={1} max={31} bind:value={anchor} required /></Label>
    </div>
    <Label class="field">When payday falls on a weekend
      <NativeSelect bind:value={weekdayPolicy}>
        <option value="none">Keep the date</option>
        <option value="previousWorkingDay">Move to Friday</option>
        <option value="nextWorkingDay">Move to Monday</option>
      </NativeSelect>
    </Label>
    <Button type="submit" disabled={task.busy || calendar.loading}>{calendar.value ? 'Save calendar settings' : 'Set up payday calendar'}</Button>
  </form>
</Card>
