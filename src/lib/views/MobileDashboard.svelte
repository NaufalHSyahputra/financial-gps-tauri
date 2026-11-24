<script lang="ts">
  import CategoryIcon from '#lib/icons/CategoryIcon.svelte';
  import TransactionCategoryIcon from '#lib/icons/TransactionCategoryIcon.svelte';
  import { Button } from '#lib/components/ui/button/index.js';
  import { Card } from '#lib/components/ui/card/index.js';
  import * as Table from '#lib/components/ui/table/index.js';
  import { accountKinds } from '../account-types';
  import { Progress } from '#lib/components/ui/progress/index.js';
  import { commands, type DashboardWidget } from '../drivers/runtime';
  import { session, today, pageRequest } from '../stores/session.svelte';
  import { privacy } from '../stores/privacy.svelte';
  import { query } from '../stores/task.svelte';
  import AccountTypeIcon from '#lib/icons/AccountTypeIcon.svelte';
  import NavIcon from '#lib/icons/NavIcon.svelte';
  import MobileTransactionRow from '../components/MobileTransactionRow.svelte';
  import Feedback from '../components/Feedback.svelte';

  let { toggleMask }: { toggleMask: () => void } = $props();
  const widgets: DashboardWidget[] = ['netWorth','incomeVsExpense','accounts'];
  const date = $derived(today());
  const wallet = query(()=>session.call(commands.dashboardGet(session.scope,{asOf:date,widgets,accountIds:[]})));
  const recent = query(()=>session.call(commands.transactionsList(session.scope,{
    filter:{basis:'reporting',sort:{field:'date',ascending:false},includeOpening:false},page:pageRequest(null,5)
  })));
  const payees = query(()=>recent.value?.rows.some(row=>row.payeeId)
    ? session.call(commands.payeesList(session.scope,{page:pageRequest(null,200)})) : Promise.resolve(null));
  const calendar = query(() => { session.financialDateTick; return session.call(commands.financialCalendarGet(session.scope)); });
  const cycleBudgets = query(() => calendar.value && privacy.visible('budgets')
    ? session.call(commands.budgetCycleGet(session.scope, {})) : Promise.resolve(null));
  const cashPlan = query(() => calendar.value
    ? session.call(commands.cashPlanGet(session.scope, { uncertainIds: [] })) : Promise.resolve(null));
  const nextDue = $derived(cashPlan.value?.days.find(day => day.obligations.minor !== '0'));
  const budgets = query(() => !calendar.loading && !calendar.value && privacy.visible('budgets')
    ? session.call(commands.budgetsGet(session.scope,{month:date.slice(0,7)})) : Promise.resolve(null));
  const goals = query(()=>privacy.visible('goals')
    ? session.call(commands.goalsList(session.scope,{asOf:date,page:pageRequest(null,3)})) : Promise.resolve(null));
  const month = $derived(new Intl.DateTimeFormat(undefined,{month:'long',timeZone:'UTC'}).format(new Date(date+'T00:00:00Z')));
  const balance = $derived(wallet.value?.netWorth?.points.at(-1)?.netWorth?.reliable);
  const totals = $derived(wallet.value?.incomeVsExpense?.points.at(-1));
  const cashFlow = $derived.by(()=>{
    const income=totals?.income?.reliable, expense=totals?.expense?.reliable;
    return income && expense ? {...income,minor:(BigInt(income.minor)-BigInt(expense.minor)).toString()} : null;
  });
  const kinds = ['cash','checking','savings','creditCard','loan'] as const;
</script>

<div class="grid min-w-0 gap-5 md:grid-cols-[minmax(0,2fr)_minmax(0,1fr)]">
  <header class="sr-only md:not-sr-only md:col-span-2">
    <h1 class="page-title">Dashboard</h1>
    <p class="mt-1 text-sm text-muted-foreground">{session.active?.name}</p>
  </header>
  <div class="empty:hidden md:col-span-2"><Feedback error={wallet.error} loading={wallet.loading}/></div>
  <Card as="section" aria-label="Net balance" class="md:col-span-2">
    <div class="flex items-center justify-between gap-2">
      <h2 class="text-sm font-medium text-muted-foreground">Net balance</h2>
      <Button variant="ghost" class="mobile-icon-button md:hidden" type="button" aria-pressed={privacy.masked}
        aria-label={privacy.masked?'Show net balance':'Hide net balance'} onclick={toggleMask}>
        <NavIcon name={privacy.masked?'eyeOff':'eye'}/>
      </Button>
    </div>
    <p class="numeric mb-4 text-3xl font-medium lg:text-4xl" class:amount-mask={privacy.masked}>{privacy.money(balance)}</p>
    <div class="border-t pt-3">
      <p class="mb-3 text-xs font-medium text-muted-foreground">{month} to date</p>
      <dl class="grid grid-cols-2 gap-3 md:grid-cols-3 md:divide-x">
        <div class="min-w-0 md:pr-3">
          <dt class="flex items-center gap-1 text-sm text-muted-foreground"><NavIcon name="in"/>Money in</dt>
          <dd class="numeric mt-1 text-base font-medium text-money-in">{privacy.money(totals?.income?.reliable)}</dd>
        </div>
        <div class="min-w-0 md:pr-3">
          <dt class="flex items-center gap-1 text-sm text-muted-foreground"><NavIcon name="out"/>Money out</dt>
          <dd class="numeric mt-1 text-base font-medium text-money-out">{privacy.money(totals?.expense?.reliable)}</dd>
        </div>
        <div class="col-span-2 min-w-0 md:col-span-1">
          <dt class="text-sm text-muted-foreground">Net cash flow</dt>
          <dd class="numeric mt-1 text-base font-medium">{privacy.money(cashFlow)}</dd>
        </div>
      </dl>
    </div>
  </Card>

  <section aria-labelledby="wallet-accounts" class="min-w-0 md:col-start-2 md:row-start-3">
    <div class="flex items-center justify-between gap-2">
      <h2 id="wallet-accounts" class="section-title">Accounts</h2>
      <a class="inline-flex min-h-11 items-center text-sm font-medium text-status no-underline" href="/accounts">All accounts</a>
    </div>
    <div class="mobile-scroll-strip -mx-4 snap-x snap-mandatory scroll-px-4 px-4 pb-1 md:hidden">
      {#each wallet.value?.accounts??[] as item (item.id)}
        <a href={'/accounts/'+item.id} class="tactile block w-44 shrink-0 snap-start rounded-2xl border border-line bg-paper p-3 no-underline">
          <span class="flex items-start gap-2 text-sm text-muted-foreground"><AccountTypeIcon kind={item.account.kind} wallet={item.account.paymentMethod==='Digital wallet'}/><span class="min-w-0 wrap-anywhere">{item.account.name}</span></span>
          <span class="numeric mt-3 block font-medium">{privacy.money(item.balance)}</span>
        </a>
      {:else}
        {#if !wallet.loading}<Button variant="outline"  href="/accounts/new">Add an account</Button>{/if}
      {/each}
    </div>
    <Card class="hidden md:block" size="sm">
      {#each kinds as kind (kind)}
        {@const group=wallet.value?.accounts.filter(item=>item.account.kind===kind)??[]}
        {#if group.length}
          <div class="border-b py-2 first:pt-0 last:border-b-0 last:pb-0">
            <h3 class="text-xs font-medium text-muted-foreground">{accountKinds[kind].label}</h3>
            {#each group as item (item.id)}
              <a href={'/accounts/'+item.id} class="flex min-h-9 max-md:min-h-11 items-center justify-between gap-2 text-sm no-underline hover:text-primary-ink">
                <span class="flex min-w-0 items-center gap-2"><AccountTypeIcon kind={item.account.kind} wallet={item.account.paymentMethod==='Digital wallet'}/><span class="truncate">{item.account.name}</span></span>
                <span class="numeric min-w-0 text-right">{privacy.money(item.balance)}</span>
              </a>
            {/each}
          </div>
        {/if}
      {/each}
      {#if !wallet.loading && !wallet.value?.accounts.length}<Button variant="ghost" href="/accounts/new">Add an account</Button>{/if}
    </Card>
  </section>

  <section aria-labelledby="wallet-activity" class="min-w-0 md:col-start-1 md:row-start-3">
    <div class="flex items-center justify-between gap-2">
      <h2 id="wallet-activity" class="section-title">Recent activity</h2>
      <a class="inline-flex min-h-11 items-center justify-end text-sm font-medium text-status no-underline max-md:min-w-11" href="/transactions">See all</a>
    </div>
    <Feedback error={recent.error || payees.error} loading={recent.loading}/>
    <ul class="overflow-hidden rounded-xl border border-line bg-paper md:hidden">
      {#each recent.value?.rows??[] as row (row.id)}
        <MobileTransactionRow {row} account={wallet.value?.accounts.find(item=>item.id===row.accountId)?.account.name}
          payee={payees.value?.rows.find(item=>item.id===row.payeeId)?.draft.name}/>
      {:else}
        {#if !recent.loading}<li class="p-3 text-sm text-muted-foreground">No recent transactions</li>{/if}
      {/each}
    </ul>
    <div class="table-wrap hidden md:block">
      <Table.Root contained={false} class="table-fixed">
        <Table.Header><Table.Row><Table.Head>Transaction</Table.Head><Table.Head class="w-28 text-right lg:w-36">Amount</Table.Head></Table.Row></Table.Header>
        <Table.Body>
          {#each recent.value?.rows??[] as row (row.id)}
            {@const account=wallet.value?.accounts.find(item=>item.id===row.accountId)?.account.name}
            {@const category=row.transferId?'Transfer':row.category?.name}
            <Table.Row>
              <Table.Cell><div class="flex min-w-0 items-center gap-2"><TransactionCategoryIcon {row}/><div class="min-w-0"><a class="block truncate font-medium hover:text-primary-ink" href={'/transactions/'+row.id}>{payees.value?.rows.find(item=>item.id===row.payeeId)?.draft.name??row.description}</a>
                <p class="truncate text-xs text-muted-foreground">{[privacy.date(row.reportingDate),account,category].filter(Boolean).join(' · ')}</p>
              </div></div></Table.Cell>
              <Table.Cell class={['numeric text-right', !privacy.masked && (row.direction==='credit'?'text-money-in':'text-money-out')]}>
                {privacy.money(row.direction==='credit'?row.amount:{...row.amount,minor:'-'+row.amount.minor})}
              </Table.Cell>
            </Table.Row>
          {:else}
            {#if !recent.loading}<Table.Row><Table.Cell colspan={2} class="text-muted-foreground">No recent transactions</Table.Cell></Table.Row>{/if}
          {/each}
        </Table.Body>
      </Table.Root>
    </div>
  </section>

  {#if calendar.value}
    <Card as="section" class="flex flex-wrap items-center gap-3 md:col-span-2">
      <div class="mr-auto">
        <h2 class="section-title">Day {calendar.value.current.elapsed} of {calendar.value.current.days}</h2>
        <p class="text-sm text-muted-foreground">Payday cycle · {privacy.date(calendar.value.current.start)} to {privacy.date(calendar.value.current.end)}</p>
        {#if nextDue}
          <p class="mt-2 text-sm">Next due {privacy.date(nextDue.date)} · <span class="numeric">{privacy.money(nextDue.obligations)}</span>{#if !privacy.masked && nextDue.dependsOnExpectedIncome} · Depends on expected income{/if}</p>
        {/if}
      </div>
      <Button variant="outline" href="/planning">View cash plan</Button>
    </Card>
  {/if}
  {#if privacy.visible('budgets') || privacy.visible('goals')}
    <section aria-labelledby="wallet-plan" class="grid min-w-0 gap-2 md:col-span-2 md:grid-cols-2">
      <h2 id="wallet-plan" class="section-title md:col-span-2">Your plan</h2>
      <Feedback error={budgets.error || cycleBudgets.error || goals.error}/>
      {#if privacy.visible('budgets')}
        {#if calendar.value}
          {#each cycleBudgets.value?.rows.slice(0,3) ?? [] as budget (budget.id)}
            <Card href="/budgets" class="block no-underline">
              <span class="mb-2 block text-sm font-medium">{budget.name}</span>
              <span class="numeric text-sm">{privacy.money(budget.remaining)} <span class="font-sans text-muted-foreground">remaining this cycle</span></span>
            </Card>
          {:else}
            {#if !cycleBudgets.loading}<Button variant="outline" href="/budgets">View cycle budgets</Button>{/if}
          {/each}
        {:else}
        {#each budgets.value?.rows.slice(0,3)??[] as budget (budget.categoryId)}
          {@const limit=BigInt(budget.limit.minor)}
          {@const progress=limit>0n ? BigInt(budget.actual.minor)*100n/limit : 0n}
          <Card href="/budgets" class="block no-underline">
            <span class="mb-2 flex items-center gap-2 text-sm font-medium"><CategoryIcon name={budget.name} icon={budget.icon} size="sm"/><span>{budget.name} <span class="font-normal text-muted-foreground">· Budget</span></span></span>
            <span class="numeric text-sm">{privacy.money(budget.remaining)} <span class="font-sans text-muted-foreground">remaining</span></span>
            {#if !privacy.masked}<Progress class="mt-2 block h-2" max={100} value={Number(progress<0n?0n:progress>100n?100n:progress)} aria-label={budget.name+' budget used'}></Progress>{/if}
          </Card>
        {:else}
          {#if !budgets.loading}<Button variant="outline"  href="/budgets">View budgets</Button>{/if}
        {/each}
        {/if}
      {/if}
      {#if privacy.visible('goals')}
        {#each goals.value?.rows??[] as goal (goal.id)}
          <Card href="/goals" class="block no-underline">
            <span class="mb-2 block text-sm font-medium">{goal.draft.name} <span class="font-normal text-muted-foreground">· Goal</span></span>
            <span class="numeric text-sm">{privacy.money(goal.current)} <span class="font-sans text-muted-foreground">of</span> {privacy.money(goal.draft.target)}</span>
            {#if !privacy.masked}<Progress class="mt-2 block h-2" max={100} value={Math.max(0,Math.min(100,Number(goal.progressPercent)))} aria-label={goal.draft.name+' progress'}></Progress>{/if}
          </Card>
        {:else}
          {#if !goals.loading}<Button variant="outline"  href="/goals">View goals</Button>{/if}
        {/each}
      {/if}
    </section>
  {/if}
</div>
