<script lang="ts">
  import RowActions from '#lib/components/RowActions.svelte';
  import { Button } from '#lib/components/ui/button/index.js';
  import { Card } from '#lib/components/ui/card/index.js';
  import { Label } from '#lib/components/ui/label/index.js';
  import { NativeSelect } from '#lib/components/ui/native-select/index.js';
  import { Input } from '#lib/components/ui/input/index.js';
  import { Textarea } from '#lib/components/ui/textarea/index.js';
  import { goto } from '$app/navigation';
  import { page } from '$app/state';
  import { commands, type Money } from '../drivers/runtime';
  import * as Tabs from '#lib/components/ui/tabs/index.js';
  import { calculateMinor } from '../calculator';
  import { session, today } from '../stores/session.svelte';
  import { privacy } from '../stores/privacy.svelte';
  import { createTask, query } from '../stores/task.svelte';
  import Heading from '../components/Heading.svelte';
  import Feedback from '../components/Feedback.svelte';
  import CategoryIcon from '#lib/icons/CategoryIcon.svelte';
  import AccountTypeIcon from '#lib/icons/AccountTypeIcon.svelte';
  import NavIcon from '#lib/icons/NavIcon.svelte';
  import AmountKeypad from '../components/AmountKeypad.svelte';

  const task = createTask();
  const accounts = query(() => session.call(commands.accountsList(session.scope, {})));
  const categories = query(() => session.call(commands.categoriesList(session.scope, { page: { limit: 200 } })));
  const groups = query(() => privacy.visible('groups') ? session.call(commands.groupsList(session.scope, { page: { limit: 200 } })) : Promise.resolve(null));
  const kinds = ['Expense', 'Income', 'Transfer'] as const;
  type Kind = typeof kinds[number];
  let kind = $state<Kind>('Expense');
  let step = $state(1);
  let body: HTMLDivElement;
  const detailsForm = $props.id();
  $effect(() => {
    step;
    if (body) body.scrollTop = 0;
  });
  let expression = $state('');
  let math = $state(false);
  let chosenCurrency = $state('');
  const currency = $derived(chosenCurrency || session.active?.baseCurrency || 'IDR');
  const scale = $derived(privacy.scale(currency) ?? 0);
  let minor = $state('0');
  let sourceId = $state('');
  let destinationId = $state('');
  let categoryId = $state('');
  let categorySearch = $state('');
  let description = $state('');
  let notes = $state('');
  let feeAmount = $state('');
  let feeCategory = $state('');
  let date = $state(today());
  let groupId = $state('');
  let localError = $state('');
  let splits = $state<{ id: string; categoryId: string; amount: string }[]>([]);
  let receivedRoute = $state(false);
  $effect(() => {
    if (receivedRoute || !accounts.value) return;
    receivedRoute = true;
    if (page.url.searchParams.get('transfer') === '1') kind = 'Transfer';
    const account = accounts.value.find(item => item.id === page.url.searchParams.get('account'));
    if (account) {
      sourceId = account.id;
      chosenCurrency = account.account.currencyCode;
    }
  });
  const available = $derived(accounts.value ?? []);
  const source = $derived(available.find(item => item.id === sourceId));
  const destination = $derived(available.find(item => item.id === destinationId));
  const matchingCategories = $derived((categories.value?.rows ?? []).filter(item =>
    item.draft.kind === (kind === 'Income' ? 'income' : 'expense') && !item.draft.hidden && !item.draft.archived));
  const shownCategories = $derived(matchingCategories.filter(item => item.draft.name.toLocaleLowerCase().includes(categorySearch.trim().toLocaleLowerCase())));
  const formatted = $derived(privacy.money({ currency, minor }));
  const categoryName = $derived(splits.length ? 'Split between categories'
    : matchingCategories.find(item => item.id === categoryId)?.draft.name ?? 'No category');

  function chooseKind(value: Kind) {
    if (kind === value) return;
    kind = value;
    categoryId = categorySearch = groupId = '';
    splits = [];
    localError = '';
  }
  function chooseAccount(id: string) {
    const selected = available.find(item => item.id === id);
    sourceId = id;
    if (selected && selected.account.currencyCode !== currency) {
      chosenCurrency = selected.account.currencyCode;
      expression = '';
      minor = '0';
      destinationId = '';
      splits = [];
      step = 1;
      localError = `Enter the amount in ${currency} for this account.`;
    }
    if (sourceId === destinationId) destinationId = '';
  }
  function amountNext() {
    localError = '';
    try {
      const value = calculateMinor(expression, scale, math);
      if (BigInt(value) <= 0n) throw new Error('Enter an amount greater than zero.');
      minor = value;
      step = 2;
      if (!sourceId && available.length === 1 && available[0].account.currencyCode === currency) sourceId = available[0].id;
    } catch (error) { localError = (error as Error).message; }
  }
  function validAccounts() {
    localError = '';
    if (!source || source.account.currencyCode !== currency) {
      localError = 'Choose an account for this amount.';
      return false;
    }
    if (kind === 'Transfer' && (!destination || sourceId === destinationId || destination.account.currencyCode !== currency)) {
      localError = 'Choose two different accounts that use the same currency.';
      return false;
    }
    return true;
  }
  function save() {
    if (!validAccounts()) return;
    void task.run(async () => {
      if (!date) throw new Error('Choose a date.');
      const amount: Money = { currency, minor };
      const label = description.trim() || (kind === 'Transfer' ? 'Transfer' : categoryName === 'No category' || splits.length ? kind : categoryName);
      const shared = { bankDate: date, description: label, notes: notes.trim(), status: 'posted' as const };
      let id: string;
      if (kind === 'Transfer') {
        const saved = await session.call(commands.transferSave(session.mutation(), {
          source: { kind: 'draft', draft: { ...shared, accountId: sourceId, amount, direction: 'debit' } },
          destination: { kind: 'draft', draft: { ...shared, accountId: destinationId, amount, direction: 'credit' } },
          fee: feeAmount.trim() ? { ...shared, description: 'Transfer fee', accountId: sourceId,
            amount: privacy.amount(currency, feeAmount.replace(',', '.')), categoryId: feeCategory || null, direction: 'debit' } : null
        }));
        id = saved.value.pair.source.row.id;
      } else {
        const parts = splits.map(split => {
          if (!split.categoryId) throw new Error('Choose a category for each part.');
          const value = privacy.amount(currency, split.amount.replace(',', '.')).minor;
          if (BigInt(value) <= 0n) throw new Error('Give each part an amount greater than zero.');
          return { categoryId: split.categoryId, minor: value };
        });
        if (parts.length && (parts.length < 2 || parts.reduce((total, split) => total + BigInt(split.minor), 0n) !== BigInt(minor))) {
          throw new Error('Use at least two parts that add up to the full amount.');
        }
        const saved = await session.call(commands.transactionSave(session.mutation(), {
          groupId: kind === 'Expense' && groupId ? groupId : null,
          draft: { ...shared, accountId: sourceId, amount, direction: kind === 'Expense' ? 'debit' : 'credit',
            categoryId: parts.length ? null : categoryId || null, splits: parts }
        }));
        id = saved.value.row.id;
      }
      await goto('/transactions/' + id);
    });
  }
  function back() {
    step--;
    localError = '';
  }
</script>

<div class="hidden md:block"><Heading title="New transaction"><Button variant="outline"  href="/transactions">All transactions</Button></Heading></div>
<Card as="section" class="max-w-3xl max-md:flex max-md:h-dvh max-md:max-w-none max-md:flex-col max-md:rounded-none max-md:border-0 max-md:p-0" aria-labelledby="transaction-step">
  <header class="shrink-0 border-b border-line px-4 pt-(--mobile-safe-top) md:hidden">
    <div class="flex h-14 items-center gap-2">
      <Button variant="ghost" href="/transactions" aria-label="Close new transaction" class="mobile-icon-button"><NavIcon name="close"/></Button>
      <h1 class="mr-auto text-base font-semibold">New transaction</h1>
      <p aria-label={'Step '+step+' of 3'} class="font-mono text-sm text-muted-foreground">{step}/3</p>
    </div>
  </header>
  <div bind:this={body} class="max-md:flex max-md:min-h-0 max-md:flex-1 max-md:flex-col max-md:overflow-y-auto max-md:overscroll-contain max-md:p-4">
  <p class="mb-2 hidden text-sm font-semibold text-muted-foreground md:block">Step {step} of 3</p>
  <h2 class="section-title mb-4 max-md:mb-3" id="transaction-step">
    {step === 1 ? 'Enter the amount' : step === 2 ? 'Choose an account' : 'Add details'}
  </h2>
  <Feedback error={localError || task.error || accounts.error || categories.error}/>
  {#if step === 1}
    <Tabs.Root value={kind} onValueChange={value=>chooseKind(value as Kind)} class="flex-col max-md:min-h-0 max-md:flex-1">
      <Tabs.List aria-label="Transaction type" class="mb-2 h-auto! max-md:w-full">
        {#each kinds as item (item)}
          <Tabs.Trigger value={item} class="min-h-9 px-3 max-md:min-h-11">{item}</Tabs.Trigger>
        {/each}
      </Tabs.List>
      <Tabs.Content value={kind} class="max-md:flex max-md:flex-1 max-md:flex-col">
        <AmountKeypad bind:value={expression} bind:math {currency} {scale} advance={amountNext}/>
      </Tabs.Content>
    </Tabs.Root>
    <Button variant="default" type="button" class="mt-4 hidden md:inline-block" onclick={amountNext}>Continue</Button>
  {:else if step === 2}
    <p class="mb-4 font-medium">{kind} · <span class="numeric">{formatted}</span></p>
    {#if !accounts.loading && !available.length}<Button variant="outline" class="mb-4" href="/accounts/new">Add an account</Button>{/if}
    {#snippet accountStrip(label: string, target: string, pick: (id: string) => void, choices: typeof available)}
      <fieldset class="mb-3 min-w-0 md:hidden">
        <legend class="mb-2 text-sm font-medium">{label}</legend>
        <div class="mobile-scroll-strip snap-x px-1 py-1">
          {#each choices as item (item.id)}
            <Button variant="outline" type="button" aria-pressed={target===item.id} onclick={()=>pick(item.id)}
              class="flex min-w-32 snap-start items-start gap-2 rounded-xl p-3 text-left aria-pressed:border-status aria-pressed:bg-ground">
              <AccountTypeIcon kind={item.account.kind} wallet={item.account.paymentMethod==='Digital wallet'}/>
              <span class="min-w-0">
                <span class="block max-w-36 truncate">{item.account.name}</span>
                <span class="numeric block text-xs text-muted-foreground">{privacy.money(item.balance)}</span>
              </span>
            </Button>
          {/each}
        </div>
      </fieldset>
    {/snippet}
    {@render accountStrip(kind==='Transfer'?'From account':'Account', sourceId, chooseAccount, available)}
    {#if kind==='Transfer'}{@render accountStrip('To account', destinationId, id=>destinationId=id, available.filter(item=>item.id!==sourceId && item.account.currencyCode===currency))}{/if}
    <div class="form-grid hidden md:grid">
      <Label class="field"><span class="flex items-center gap-2">{#if source}<AccountTypeIcon kind={source.account.kind} wallet={source.account.paymentMethod==='Digital wallet'}/>{/if}{kind === 'Transfer' ? 'From account' : 'Account'}</span>
        <NativeSelect value={sourceId} onchange={event => chooseAccount(event.currentTarget.value)}>
          <option value="">Choose an account</option>
          {#each available as item (item.id)}
            <option value={item.id}>{item.account.name} · {item.account.currencyCode}</option>
          {/each}
        </NativeSelect>
      </Label>
      {#if kind === 'Transfer'}
        <Label class="field"><span class="flex items-center gap-2">{#if destination}<AccountTypeIcon kind={destination.account.kind} wallet={destination.account.paymentMethod==='Digital wallet'}/>{/if}To account</span>
          <NativeSelect bind:value={destinationId}>
            <option value="">Choose an account</option>
            {#each available.filter(item => item.id !== sourceId && item.account.currencyCode === currency) as item (item.id)}
              <option value={item.id}>{item.account.name} · {item.account.currencyCode}</option>
            {/each}
          </NativeSelect>
        </Label>
      {/if}
    </div>
    {#if kind !== 'Transfer'}
      <fieldset class="mt-4 space-y-2">
        <legend class="font-semibold">Category (optional)</legend>
        {#if matchingCategories.length}<Label class="field">Find a category<Input type="search" bind:value={categorySearch}/></Label>{/if}
        <div class="flex flex-wrap gap-2 max-md:grid max-md:grid-cols-3">
          <Button variant={!categoryId ? 'default' : 'outline'} type="button" class="category-tile min-h-11 gap-2" aria-pressed={!categoryId}  onclick={() => categoryId = ''}><CategoryIcon size="sm"/>No category</Button>
          {#each shownCategories as item (item.id)}
            <Button variant={categoryId === item.id ? 'default' : 'outline'} type="button" class="category-tile min-h-11 gap-2" aria-pressed={categoryId === item.id}  onclick={() => { categoryId = item.id; splits = []; }}><CategoryIcon name={item.draft.name} kind={item.draft.kind} icon={item.draft.icon} size="sm"/>{item.draft.name}</Button>
          {/each}
        </div>
      </fieldset>
    {:else}<p class="mt-3 text-sm text-muted-foreground">Transfers use two accounts in the same currency.</p>{/if}
    <div class="mt-5 hidden flex-wrap gap-2 md:flex">
      <Button variant="outline" type="button" disabled={task.busy} onclick={back}>Back</Button>
      <Button variant="outline" type="button" disabled={task.busy} onclick={() => { if (validAccounts()) step = 3; }}>Add details</Button>
      <Button variant="default" type="button"  disabled={task.busy} onclick={save}>Save Transaction</Button>
    </div>
  {:else}
    <form id={detailsForm} class="space-y-4" onsubmit={event => { event.preventDefault(); save(); }}>
      <fieldset disabled={task.busy} class="space-y-4">
        <dl class="grid gap-2 rounded-lg bg-ground p-3 sm:grid-cols-3">
          <div><dt class="text-sm text-muted-foreground">{kind}</dt><dd class="numeric">{formatted}</dd></div>
          <div><dt class="text-sm text-muted-foreground">Account</dt><dd>{source?.account.name}{#if destination && kind === 'Transfer'} → {destination.account.name}{/if}</dd></div>
          {#if kind !== 'Transfer'}<div><dt class="text-sm text-muted-foreground">Category</dt><dd>{categoryName}</dd></div>{/if}
        </dl>
        <div class="form-grid">
          <Label class="field">Description (optional)<Input bind:value={description} maxlength={256}/></Label>
          <Label class="field">Date<Input type="date" bind:value={date} required/></Label>
        </div>
        <Label class="field">Notes (optional)<Textarea bind:value={notes} maxlength={1000} rows={3}></Textarea></Label>
        {#if kind === 'Transfer'}
          <div class="form-grid">
            <Label class="field">Transfer fee (optional)<Input type={privacy.masked ? 'password' : 'text'} inputmode="decimal" bind:value={feeAmount}/></Label>
            {#if feeAmount.trim()}
              <Label class="field">Fee category<NativeSelect bind:value={feeCategory} required>
                <option value="">Choose a category</option>
                {#each matchingCategories.filter(row=>!row.draft.ignored) as row (row.id)}
                  <option value={row.id}>{row.draft.name}</option>
                {/each}
              </NativeSelect></Label>
            {/if}
          </div>
        {/if}
        {#if kind === 'Expense' && groups.value?.rows.some(item => !item.archived)}
          <Label class="field">Shared group (optional)<NativeSelect bind:value={groupId}><option value="">No shared group</option>
            {#each groups.value.rows.filter(item => !item.archived) as group (group.id)}<option value={group.id}>{group.name}</option>{/each}
          </NativeSelect></Label>
        {/if}
        {#if kind !== 'Transfer' && matchingCategories.length}
          <div class="space-y-2">
            <Button variant="outline" type="button" onclick={() => splits = [...splits, { id: crypto.randomUUID(), categoryId: '', amount: '' }]}>Split between categories</Button>
            {#each splits as part, index (part.id)}
              <div class="grid items-end gap-2 sm:grid-cols-[1fr_1fr_auto]">
                <Label class="field">Category for part {index + 1}<NativeSelect bind:value={part.categoryId} required>
                  <option value="">Choose a category</option>
                  {#each matchingCategories as item (item.id)}<option value={item.id}>{item.draft.name}</option>{/each}
                </NativeSelect></Label>
                <Label class="field">Amount for part {index + 1}<Input type={privacy.masked ? 'password' : 'text'} inputmode="decimal" bind:value={part.amount} required/></Label>
                <RowActions deleteLabel={`Remove part ${index + 1}`} onDelete={()=>splits=splits.filter(item=>item.id!==part.id)}/>
              </div>
            {/each}
          </div>
        {/if}
        <div class="hidden flex-wrap gap-2 md:flex">
          <Button variant="outline" type="button" onclick={back}>Back</Button>
          <Button variant="default" type="submit" >Save Transaction</Button>
        </div>
      </fieldset>
    </form>
  {/if}
  </div>
  <footer class={["mobile-sticky-actions shrink-0 md:hidden", step === 2 && "grid-cols-[auto_minmax(0,1fr)_minmax(0,1fr)]!"]}>
    {#if step===1}
      <Button variant="default" type="button" class="col-span-2" onclick={amountNext}>Continue</Button>
    {:else if step===2}
      <Button variant="outline" type="button" class="size-11 p-0" onclick={back} aria-label="Back" disabled={task.busy}><NavIcon name="back"/></Button>
      <Button variant="outline" type="button" disabled={task.busy} onclick={()=>{if(validAccounts()) step=3;}}>Details</Button>
      <Button type="button" onclick={save} disabled={task.busy}>Save</Button>
    {:else}
      <Button variant="outline" type="button" class="size-11 p-0" onclick={back} aria-label="Back" disabled={task.busy}><NavIcon name="back"/></Button>
      <Button variant="default" type="submit" form={detailsForm}  disabled={task.busy}>Save Transaction</Button>
    {/if}
  </footer>
</Card>
