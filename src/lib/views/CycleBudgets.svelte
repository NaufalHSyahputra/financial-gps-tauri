<script lang="ts">
  const modalForm = $props.id();
  import { Button } from '#lib/components/ui/button/index.js';
  import { Card } from '#lib/components/ui/card/index.js';
  import { Input } from '#lib/components/ui/input/index.js';
  import { Label } from '#lib/components/ui/label/index.js';
  import { NativeCheckbox } from '#lib/components/ui/checkbox/index.js';
  import Feedback from '#lib/components/Feedback.svelte';
  import Modal from '#lib/components/Modal.svelte';
  import MoneyField from '#lib/components/MoneyField.svelte';
  import RowActions from '#lib/components/RowActions.svelte';
  import { commands, type CycleBudgetChange, type CycleBudgetRow } from '#lib/drivers/runtime';
  import { pageRequest, session } from '#lib/stores/session.svelte';
  import { privacy } from '#lib/stores/privacy.svelte';
  import { createTask, query } from '#lib/stores/task.svelte';

  const task = createTask();
  let date = $state('');
  const active = query(() => { session.financialDateTick; return session.call(commands.financialCalendarGet(session.scope)); });
  const budget = query(() => { session.financialDateTick; return session.call(commands.budgetCycleGet(session.scope, { start: date || null })); });
  const categories = query(() => session.call(commands.categoriesList(session.scope, { page: pageRequest(null, 200) })));
  const editable = $derived(budget.value?.cycle.start === active.value?.current.start);
  let action = $state<'save' | 'delete' | 'copy'>();
  let selected = $state<CycleBudgetRow>();
  let name = $state('');
  let limit = $state('');
  let categoryIds = $state<string[]>([]);
  const statusLabels = { under: 'Under limit', near: 'Near limit', at: 'At limit', over: 'Over limit', zeroUnused: 'No spending', zeroExceeded: 'Spending above zero limit' };

  function open(row?: CycleBudgetRow) {
    selected = row;
    name = row?.name ?? '';
    limit = row?.limit.minor ?? '';
    categoryIds = [...(row?.categoryIds ?? [])];
    action = 'save';
  }

  function submit(event: SubmitEvent) {
    event.preventDefault();
    const start = budget.value?.cycle.start;
    if (!start) return;
    void task.run(async () => {
      let change: CycleBudgetChange;
      if (action === 'save') {
        change = { save: { id: selected?.id, name, limitMinor: privacy.amount('IDR', limit).minor, categoryIds } };
      } else if (action === 'delete' && selected) {
        change = { delete: { id: selected.id } };
      } else {
        change = 'copyPrevious';
      }
      await session.call(commands.budgetCycleEdit(session.mutation(), { start, change }));
      action = undefined;
      budget.refresh();
    });
  }
</script>

<Feedback error={active.error || budget.error || categories.error} loading={budget.loading} />
{#if !active.loading && !active.value && !active.error}
  <Card class="flex flex-wrap items-center gap-3">
    <p class="mr-auto">Set your payday calendar to create budgets that follow your income cycle.</p>
    <Button href="/settings#financial-calendar">Set up payday calendar</Button>
  </Card>
{:else if budget.value && active.value}
  <div class="mb-4 flex flex-wrap items-end gap-3">
    <Label class="field">Find a cycle by date
      <Input type="date" bind:value={date} min={active.value.historyFloor} max={active.value.today} />
    </Label>
    {#if date}<Button variant="outline" onclick={() => date = ''}>Current cycle</Button>{/if}
    {#if editable}
      <Button variant="outline" disabled={budget.value.cycle.start === active.value.historyFloor} onclick={() => action = 'copy'}>Copy previous cycle</Button>
      <Button onclick={() => open()}>Add budget</Button>
    {/if}
  </div>
  <p class="mb-3 text-sm text-muted-foreground">{budget.value.cycle.start} to {budget.value.cycle.end} · Day {budget.value.cycle.elapsed} of {budget.value.cycle.days}{budget.value.cycle.kind === 'regular' ? '' : ' · ' + budget.value.cycle.kind + ' cycle'}</p>
  {#if budget.value.excludedForeign !== '0'}<p class="mb-3 text-sm text-muted-foreground">Excluded {budget.value.excludedForeign} expenses in other currencies.</p>{/if}
  <div class="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
    {#each budget.value.rows as row (row.id)}
      <Card as="section">
        <div class="flex items-start justify-between gap-2">
          <div><h2 class="section-title">{row.name}</h2><p class="text-sm text-muted-foreground">{statusLabels[row.status]}{row.pace === 'aheadOfPlan' ? ' · Ahead of plan' : ''}</p></div>
          {#if editable}<RowActions itemName={row.name} onEdit={() => open(row)} onDelete={() => { selected = row; action = 'delete'; }} />{/if}
        </div>
        <dl class="mt-4 grid grid-cols-2 gap-3 text-sm">
          <div><dt class="text-muted-foreground">Limit</dt><dd class="numeric">{privacy.money(row.limit)}</dd></div>
          <div><dt class="text-muted-foreground">Spent</dt><dd class="numeric">{privacy.money(row.spent)}</dd></div>
          <div><dt class="text-muted-foreground">Remaining</dt><dd class="numeric">{privacy.money(row.remaining)}</dd></div>
          <div><dt class="text-muted-foreground">Projected spending</dt><dd class="numeric">{row.runRate ? privacy.money(row.runRate) : 'Available on day 3'}</dd></div>
        </dl>
      </Card>
    {:else}
      <Card>No budgets for this cycle.</Card>
    {/each}
  </div>
{/if}
{#if action}
  <Modal title={action === 'save' ? selected ? 'Edit cycle budget' : 'Add cycle budget' : action === 'delete' ? 'Remove cycle budget' : 'Copy previous cycle'} close={() => action = undefined} confirm={action === 'delete'}>
    <form id={modalForm + '-cycle-budget'} class="space-y-3" onsubmit={submit}>
      {#if action === 'save'}
        <Label class="field">Budget name<Input bind:value={name} maxlength={100} required /></Label>
        <MoneyField label="Limit in IDR" bind:value={limit} />
        <fieldset class="max-h-52 space-y-1 overflow-y-auto">
          <legend>Expense categories</legend>
          {#each categories.value?.rows.filter(row => row.draft.kind === 'expense' && (!row.draft.ignored || categoryIds.includes(row.id))) ?? [] as row (row.id)}
            <Label class="check"><NativeCheckbox checked={categoryIds.includes(row.id)} onchange={(event) => {
              categoryIds = event.currentTarget.checked ? [...categoryIds, row.id] : categoryIds.filter(id => id !== row.id);
            }} /> {row.draft.name}</Label>
          {/each}
        </fieldset>
      {:else if action === 'copy'}
        <p>Copy names, categories, and limits to the current cycle. Spending is calculated from this cycle’s transactions.</p>
      {:else}
        <p>Remove the “{selected?.name}” budget from this cycle?</p>
      {/if}
      <Feedback error={task.error} />
    </form>
    {#snippet footer()}
      <Button type="submit" form={modalForm + '-cycle-budget'} disabled={task.busy || (action === 'save' && !categoryIds.length)}>{action === 'save' ? 'Save budget' : action === 'copy' ? 'Copy budgets' : 'Remove budget'}</Button>
    {/snippet}
  </Modal>
{/if}
