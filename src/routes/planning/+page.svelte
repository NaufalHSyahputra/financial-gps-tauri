<script lang="ts">
  const modalForm = $props.id();
  import { Button } from '#lib/components/ui/button/index.js';
  import { Card } from '#lib/components/ui/card/index.js';
  import { Input } from '#lib/components/ui/input/index.js';
  import { Label } from '#lib/components/ui/label/index.js';
  import { NativeCheckbox } from '#lib/components/ui/checkbox/index.js';
  import { NativeSelect } from '#lib/components/ui/native-select/index.js';
  import Feedback from '#lib/components/Feedback.svelte';
  import Heading from '#lib/components/Heading.svelte';
  import Modal from '#lib/components/Modal.svelte';
  import MoneyField from '#lib/components/MoneyField.svelte';
  import RowActions from '#lib/components/RowActions.svelte';
  import { commands, type CashPlanEntry, type CashPlanKind } from '#lib/drivers/runtime';
  import { session, pageRequest, today } from '#lib/stores/session.svelte';
  import { privacy } from '#lib/stores/privacy.svelte';
  import { query, createTask } from '#lib/stores/task.svelte';

  let through = $state('');
  let stageEditor = $state(false);
  let stageRule = $state('');
  let stageRows = $state<{ id: string; startsOn: string; endsOn: string; amount: string }[]>([]);
  const recurringRules = query(() => stageEditor && privacy.visible('recurring')
    ? session.call(commands.recurringList(session.scope, { page: pageRequest(null, 200) })) : Promise.resolve(null));
  const eligibleRules = $derived(recurringRules.value?.rows.filter(rule => rule.draft.direction === 'debit'
    && rule.draft.amount.currency === 'IDR' && !rule.draft.splits.length) ?? []);
  const sourceLabels = { manual: '', recurring: 'Recurring', cardStatement: 'Card statement', loan: 'Loan schedule' };
  let uncertainIds = $state<string[]>([]);
  const forecast = query(() => {
    session.financialDateTick;
    return session.call(commands.cashPlanGet(session.scope, { through: through || null, uncertainIds: [...uncertainIds] }));
  });
  const task = createTask();
  let editing = $state(false);
  let deleting = $state(false);
  let selected = $state<CashPlanEntry>();
  let name = $state('');
  let dueOn = $state('');
  let amount = $state('');
  let kind = $state<CashPlanKind>('obligation');
  let transactionId = $state('');
  const candidates = query(() => editing && !deleting ? session.call(commands.transactionsList(session.scope, {
    filter: { status: 'posted', direction: kind === 'obligation' ? 'debit' : 'credit', ignored: false, transfer: false,
      range: { to: today() }, nativeAmount: { currency: 'IDR' } }, page: pageRequest(null, 100)
  })) : Promise.resolve(null));
  const kinds = { obligation: 'Obligation', stableIncome: 'Stable expected income', uncertainIncome: 'Uncertain expected income' };
  const states = { upcoming: 'Upcoming', overdue: 'Overdue', received: 'Received', missed: 'Missed', settled: 'Settled' };
  const showDays = $derived(forecast.value?.days.filter(day => day.date === forecast.value?.today || day.entries.length > 0 || day.date === forecast.value?.through) ?? []);

  function open(row?: CashPlanEntry, remove = false) {
    selected = row;
    deleting = remove;
    name = row?.name ?? '';
    dueOn = row?.dueOn ?? forecast.value?.today ?? '';
    amount = row?.amount.minor ?? '';
    kind = row?.kind ?? 'obligation';
    transactionId = row?.transactionId ?? '';
    editing = true;
  }

  function openStages(ruleId = '') {
    stageRule = ruleId;
    stageRows = forecast.value?.stages.find(row => row.ruleId === ruleId)?.stages.map(row => ({
      id: crypto.randomUUID(), startsOn: row.startsOn, endsOn: row.endsOn ?? '', amount: row.minor
    })) ?? [];
    stageEditor = true;
  }

  function saveStages(event: SubmitEvent) {
    event.preventDefault();
    void task.run(async () => {
      const stages = stageRows.map(row => ({ startsOn: row.startsOn, endsOn: row.endsOn || null,
        minor: privacy.amount('IDR', row.amount).minor }));
      await session.call(commands.cashPlanEdit(session.mutation(), { setStages: { ruleId: stageRule, stages } }));
      stageEditor = false;
      forecast.refresh();
    });
  }
  function submit(event: SubmitEvent) {
    event.preventDefault();
    void task.run(async () => {
      await session.call(commands.cashPlanEdit(session.mutation(), deleting && selected
        ? { delete: { id: selected.id } }
        : { save: { id: selected?.id, name, dueOn, minor: privacy.amount('IDR', amount).minor, kind, transactionId: transactionId || null } }));
      editing = false;
      forecast.refresh();
    });
  }
</script>

<Heading title="Cash plan" subtitle="Track upcoming payments and see which ones depend on expected income.">
  {#if forecast.value}
    {#if privacy.visible('recurring')}<Button variant="outline" onclick={() => openStages()}>Recurring amounts</Button>{/if}
    <Button onclick={() => open()}>Add to cash plan</Button>
  {/if}
</Heading>
<Feedback error={forecast.error} loading={forecast.loading} />
{#if !forecast.loading && !forecast.value && !forecast.error}
  <Card class="flex flex-wrap items-center gap-3">
    <p class="mr-auto">Set a payday calendar to forecast through the end of your next cycle.</p>
    <Button href="/settings#financial-calendar">Set up payday calendar</Button>
  </Card>
{:else if forecast.value}
  <div class="grid gap-4 xl:grid-cols-[minmax(0,3fr)_minmax(18rem,2fr)]">
    <div class="min-w-0 space-y-4">
      <Card as="section">
        <div class="flex flex-wrap items-end justify-between gap-4">
          <div>
            <p class="text-sm text-muted-foreground">Liquid IDR cash today</p>
            <p class="numeric mt-1 text-2xl font-semibold">{privacy.money(forecast.value.cashStart)}</p>
          </div>
          <Label class="field">Forecast through<Input type="date" min={forecast.value.today} bind:value={through} /></Label>
        </div>
        <p class="mt-3 text-sm text-muted-foreground">Credit, investments and non-IDR accounts are excluded. Same-day expected income contributes to closing cash after due payments.
          {#if forecast.value.excludedForeignAccounts !== '0'} Liquid accounts in other currencies: {forecast.value.excludedForeignAccounts}.{/if}</p>
      </Card>
      <Card as="section">
        <h2 class="section-title">Cash forecast</h2>
        <ol class="mt-2 divide-line md:divide-y">
          {#each showDays as day (day.date)}
            <li class="py-3 max-md:my-2 max-md:rounded-lg max-md:border max-md:border-line max-md:px-3">
              <div class="flex flex-wrap items-center justify-between gap-2">
                <h3 class="font-semibold">{day.date === forecast.value.today ? 'Today · ' : ''}{day.date}</h3>
                {#if !privacy.masked && day.dependsOnExpectedIncome}<span class="text-warning">Depends on expected income</span>{/if}
              </div>
              <dl class="mt-2 grid grid-cols-2 gap-3 text-sm sm:grid-cols-4">
                <div><dt class="text-muted-foreground">Due</dt><dd class="numeric">{privacy.money(day.obligations)}</dd></div>
                <div><dt class="text-muted-foreground">Cash only</dt><dd class="numeric">{privacy.money(day.cashOnlyEnd)}</dd></div>
                <div><dt class="text-muted-foreground">With expected income</dt><dd class="numeric">{privacy.money(day.expectedEnd)}</dd></div>
                {#if uncertainIds.length}<div><dt class="text-muted-foreground">With selected income</dt><dd class="numeric">{privacy.money(day.scenarioEnd)}</dd></div>{/if}
              </dl>
              {#if BigInt(day.cashOnlyGap.minor) > 0n}
                <p class="mt-2 text-sm">At-due gap · Cash only {privacy.money(day.cashOnlyGap)} · With expected income {privacy.money(day.expectedGap)}{#if uncertainIds.length} · With selected income {privacy.money(day.scenarioGap)}{/if}</p>
              {/if}
              {#if day.entries.length}
                <ul class="mt-2 space-y-1 text-sm text-muted-foreground">
                  {#each day.entries as entry (entry.id)}
                    <li>{entry.name} · {privacy.money(entry.remaining)}{entry.dueOn < day.date ? ' · Overdue since ' + entry.dueOn : ''}</li>
                  {/each}
                </ul>
              {/if}
            </li>
          {/each}
        </ol>
      </Card>
    </div>
    <Card as="section" class="self-start">
      <h2 class="section-title">Obligations and income</h2>
      <ul class="mt-2 divide-y divide-line">
        {#each forecast.value.entries as entry (entry.id)}
          <li class="py-3">
            <div class="flex flex-wrap items-start justify-between gap-2 max-md:grid max-md:grid-cols-[minmax(0,1fr)_auto] max-md:items-center">
              <div class="max-md:col-span-2 max-md:wrap-anywhere"><h3 class="font-semibold">{entry.name}</h3><p class="text-sm text-muted-foreground">{entry.dueOn} · {kinds[entry.kind]} · {states[entry.state]}{sourceLabels[entry.source] ? ' · ' + sourceLabels[entry.source] : ''}</p></div>
              <span class="numeric max-md:last:col-span-2">{privacy.money(entry.amount)}</span>
              {#if entry.source === 'manual'}<RowActions itemName={entry.name} onEdit={() => open(entry)} onDelete={() => open(entry, true)} />
              {:else if entry.ruleId && privacy.visible('recurring')}<Button size="sm" variant="ghost" onclick={() => openStages(entry.ruleId ?? '')}>Change amounts</Button>{/if}
            </div>
            {#if entry.kind === 'uncertainIncome' && entry.state === 'upcoming'}
              <Label class="check mt-2"><NativeCheckbox checked={uncertainIds.includes(entry.id)} onchange={(event) => {
                uncertainIds = event.currentTarget.checked ? [...uncertainIds, entry.id] : uncertainIds.filter(id => id !== entry.id);
              }} /> Include in scenario</Label>
            {/if}
            {#if entry.transactionId && entry.source === 'manual'}<a class="mobile-touch-link mt-1 inline-block text-sm underline" href={'/transactions/' + entry.transactionId}>Linked transaction</a>{/if}
          </li>
        {:else}
          <li class="py-3 text-sm text-muted-foreground">No upcoming entries.</li>
        {/each}
      </ul>
    </Card>
  </div>
{/if}

{#if editing}
  <Modal title={deleting ? 'Remove plan entry' : selected ? 'Edit plan entry' : 'Add to cash plan'} close={() => editing = false} confirm={deleting}>
    <form id={modalForm + '-plan'} class="space-y-3" onsubmit={submit}>
      {#if deleting}
        <p>Remove “{selected?.name}” from the plan?</p>
      {:else}
        <Label class="field">Name<Input maxlength={100} bind:value={name} required /></Label>
        <Label class="field">Type<NativeSelect bind:value={kind} onchange={() => transactionId = ''}>
          {#each Object.entries(kinds) as [value, label] (value)}<option {value}>{label}</option>{/each}
        </NativeSelect></Label>
        <div class="form-grid"><Label class="field">Due date<Input type="date" bind:value={dueOn} required /></Label><MoneyField label="Amount in IDR" bind:value={amount} /></div>
        <Label class="field">Link a posted transaction (optional)
          <NativeSelect bind:value={transactionId}>
            <option value="">No linked transaction</option>
            {#if transactionId && !candidates.value?.rows.some(row => row.id === transactionId)}<option value={transactionId}>Keep current transaction</option>{/if}
            {#each candidates.value?.rows.filter(row => row.entryKind === 'ordinary' && !row.excludeFromPnl) ?? [] as row (row.id)}
              <option value={row.id}>{row.bankDate} · {row.description} · {privacy.money(row.amount)}</option>
            {/each}
          </NativeSelect>
        </Label>
        <Feedback error={candidates.error} />
      {/if}
      <Feedback error={task.error} />
    </form>
    {#snippet footer()}
      <Button type="submit" form={modalForm + '-plan'} disabled={task.busy}>{deleting ? 'Remove entry' : 'Save entry'}</Button>
    {/snippet}
  </Modal>
{/if}

{#if stageEditor}
  <Modal title="Recurring amounts" close={() => stageEditor = false}>
    <form id={modalForm + '-stages'} class="space-y-3" onsubmit={saveStages}>
      <Label class="field">Recurring expense
        <NativeSelect value={stageRule} onchange={(event) => openStages(event.currentTarget.value)} required>
          <option value="">Choose a recurring expense</option>
          {#each eligibleRules as rule (rule.id)}<option value={rule.id}>{rule.draft.description}</option>{/each}
        </NativeSelect>
      </Label>
      <Feedback error={recurringRules.error} loading={recurringRules.loading} />
      {#if stageRule}
        <p class="text-sm text-muted-foreground">Set date ranges for amount changes. Dates without a range use the rule’s usual amount.</p>
        {#each stageRows as row (row.id)}
          <div class="grid items-end gap-2 rounded-lg border border-line p-3 sm:grid-cols-2">
            <Label class="field">Starts on<Input type="date" bind:value={row.startsOn} required /></Label>
            <Label class="field">Ends on (optional)<Input type="date" bind:value={row.endsOn} /></Label>
            <MoneyField label="Amount in IDR" bind:value={row.amount} />
            <RowActions deleteLabel="Remove date range" onDelete={() => stageRows = stageRows.filter(item => item.id !== row.id)} />
          </div>
        {/each}
        <div class="flex flex-wrap gap-2">
          <Button type="button" variant="outline" onclick={() => stageRows = [...stageRows, { id: crypto.randomUUID(), startsOn: '', endsOn: '', amount: '' }]}>Add date range</Button>

        </div>
      {/if}
      <Feedback error={task.error} />
    </form>
    {#snippet footer()}
      {#if stageRule}<Button type="submit" form={modalForm + '-stages'} disabled={task.busy}>Save amounts</Button>{/if}
    {/snippet}
  </Modal>
{/if}
