<script lang="ts">
  import { Button } from '#lib/components/ui/button/index.js';
  import { page } from '$app/state';
  import { useLauncher } from '../launcher.svelte';
  import { privacy } from '../stores/privacy.svelte';
  import { session } from '../stores/session.svelte';
  import { navigation, reports } from '../navigation';
  import Modal from './Modal.svelte';
  import NavIcon from '#lib/icons/NavIcon.svelte';

  let { tabs, wizard, search, toggleMask }: { tabs: boolean; wizard: boolean; search: () => void; toggleMask: () => void } = $props();
  const launcher = useLauncher();
  let sheet = $state<'ledgers' | 'more' | null>(null);
  const path = $derived(page.url.pathname);
  const feature = $derived(path.split('/')[1]);
  const plan = [
    { label: 'Budgets', href: '/budgets' },
    { label: 'Cash plan', href: '/planning' },
    { label: 'Goals', href: '/goals' },
    { label: 'Recurring', href: '/recurring' },
    { label: 'Loans', href: '/accounts#loans' },
    { label: 'Assets', href: '/assets' }
  ];
  const onLoans = $derived(path === '/accounts' && page.url.hash === '#loans');
  const planning = $derived(onLoans || plan.some(item => item.href === path));
  const planHome = $derived(plan.find(item => item.href !== '/planning' && privacy.visible(item.href.split(/[\/#]/)[1]))?.href ?? '/accounts#loans');
  const title = $derived(
    path === '/accounts/new' ? 'New account'
    : feature === 'reports' ? reports.find(item => path === '/reports/' + item.path)?.label ?? 'Reports'
    : path === '/settings/backups' ? 'Backups'
    : feature === 'dashboard' ? 'Home'
    : feature === 'transactions' ? tabs ? 'Activity' : 'Transaction'
    : path.startsWith('/accounts/') ? 'Account'
    : navigation.flatMap(section => section.items).find(([, href]) => href === '/' + feature)?.[0] ?? 'Financial GPS'
  );
  const back = $derived(
    feature === 'reports' ? '/dashboard' : path.startsWith('/assets/groups/') ? '/assets' : '/' + feature
  );
  $effect(() => {
    path;
    session.epoch;
    sheet = null;
  });

  async function ledgerAction(action: () => Promise<void>) {
    await launcher.run(action);
    if (!launcher.error) sheet = null;
  }
</script>

{#if !wizard}
  <header class="liquid-glass-bar sticky top-0 z-20 border-x-0 border-t-0 pt-(--mobile-safe-top) select-none md:hidden">
    <div class="flex h-14 min-w-0 items-center gap-1 px-3">
      {#if !tabs}<Button variant="ghost" class="mobile-icon-button no-underline" href={back} aria-label="Back"><NavIcon name="back"/></Button>{/if}
      <p class="mr-auto min-w-0 truncate text-base font-semibold">{title}</p>
      {#if tabs}
        <Button variant="ghost" type="button" class="tactile mobile-icon-button rounded-full border-(--glass-border)" aria-label="Switch ledger" onclick={()=>sheet='ledgers'}><NavIcon name="ledger"/></Button>
        <Button variant="ghost" type="button" class="tactile mobile-icon-button" aria-pressed={privacy.masked} aria-label={privacy.masked ? 'Show amounts' : 'Hide amounts'} onclick={toggleMask}><NavIcon name={privacy.masked ? 'eyeOff' : 'eye'}/></Button>
        <Button variant="ghost" type="button" class="tactile mobile-icon-button" aria-label="Search" onclick={search}><NavIcon name="search"/></Button>
      {/if}
    </div>
  </header>
{/if}

{#if tabs}
  <nav aria-label="Mobile navigation" class="liquid-glass-bar fixed inset-x-0 bottom-[calc(var(--mobile-safe-bottom)+0.5rem)] z-30 mx-3 rounded-full px-2 py-1.5 select-none md:hidden">
    <div class="grid h-14 grid-cols-5 items-center gap-0.5">
      <Button variant="ghost" href="/dashboard" aria-current={path==='/dashboard'?'page':undefined} class="tactile mobile-tab rounded-full"><NavIcon name="home" active={path==='/dashboard'}/>Home</Button>
      <Button variant="ghost" href="/transactions" aria-current={path==='/transactions'?'page':undefined} class="tactile mobile-tab rounded-full"><NavIcon name="activity" active={path==='/transactions'}/>Activity</Button>
      <Button href="/transactions/new" aria-label="New transaction" class="tactile liquid-glass-orb mx-auto size-12 rounded-full p-0"><NavIcon name="add" class="size-6"/></Button>
      <Button variant="ghost" href={planHome} aria-current={planning?'page':undefined} class="tactile mobile-tab rounded-full"><NavIcon name="plan" active={planning}/>Plan</Button>
      <Button variant="ghost" type="button" class="tactile mobile-tab rounded-full" aria-pressed={!planning && path!=='/dashboard' && path!=='/transactions'} onclick={()=>sheet='more'}><NavIcon name="more" active={!planning && path!=='/dashboard' && path!=='/transactions'}/>More</Button>
    </div>
  </nav>
{/if}

{#if planning}
  <nav aria-label="Plan sections" class="mobile-scroll-strip border-b border-line bg-paper px-4 py-2 md:hidden">
    {#each plan as item (item.href)}
      {#if privacy.visible(item.href.split(/[\/#]/)[1])}
        <Button variant="ghost" class="mobile-chip" href={item.href} aria-current={(item.label === 'Loans' ? onLoans : path === item.href) ? 'page' : undefined}>{item.label}</Button>
      {/if}
    {/each}
  </nav>
{/if}

{#if sheet==='ledgers'}
  <Modal title="Switch ledger" close={()=>sheet=null} mobileOnly>
    {#if launcher.error}<p class="error mb-2" role="alert">{launcher.error}</p>{/if}
    <ul class="divide-y divide-line">
      {#each launcher.ledgers as item (item.ledgerInstanceId)}
        {@const active = item.ledgerInstanceId === session.active?.scope.ledgerInstanceId}
        <li><Button variant="outline" type="button" class="flex w-full items-center justify-between gap-2 border-0 px-2 text-left" disabled={launcher.busy || active}
          onclick={()=>void ledgerAction(()=>launcher.open({ ledgerInstanceId:item.ledgerInstanceId }))}>
          <span class="min-w-0 break-words">{item.label}</span>
          {#if active}<span class="text-xs text-muted-foreground">Open</span>{/if}
        </Button></li>
      {/each}
    </ul>
    {#snippet footer()}
      <div class="flex gap-2">
        <Button variant="outline" class="flex-1 justify-center" href="/">Your activity</Button>
        <Button variant="outline" type="button" disabled={launcher.busy} onclick={()=>void ledgerAction(()=>launcher.lock())}>Lock</Button>
      </div>
    {/snippet}
  </Modal>
{:else if sheet==='more'}
  <Modal title="More" close={()=>sheet=null} mobileOnly>
    <nav aria-label="More sections" class="grid gap-1">
      {#each navigation as section (section.label)}
        {#each section.items as [label, href] (href)}
          {@const key=href.split('/')[1]}
          {#if !['dashboard','transactions','budgets','goals','recurring','assets','planning'].includes(key) && privacy.visible(key)}
            <a href={href} onclick={()=>sheet=null} class="flex min-h-11 items-center justify-between rounded-lg px-3 no-underline hover:bg-ground">
              {label}<span aria-hidden="true" class="text-muted-foreground">›</span>
            </a>
          {/if}
        {/each}
      {/each}
    </nav>
  </Modal>
{/if}
