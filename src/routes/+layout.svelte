<script lang="ts">
  import { Button } from '#lib/components/ui/button/index.js';
  import { Card } from '#lib/components/ui/card/index.js';
  import '../app.css';
  import { page } from '$app/state';
  import { goto } from '$app/navigation';
  import { onMount, untrack } from 'svelte';
  import { provideLauncher } from '../lib/launcher.svelte';
  import { commands, events } from '../lib/drivers/runtime';
  import { session, message } from '../lib/stores/session.svelte';
  import { privacy } from '../lib/stores/privacy.svelte';
  import { storageSync } from '../lib/stores/storage-sync.svelte';
  import { jobs } from '../lib/stores/jobs.svelte';
  import WorkspaceNav from '#lib/components/WorkspaceNav.svelte';
  import { Label } from '#lib/components/ui/label/index.js';
  import { Switch } from '#lib/components/ui/switch/index.js';
  import { Badge } from '#lib/components/ui/badge/index.js';
  import { navigation, reports } from '../lib/navigation';
  import MobileWorkspaceNav from '../lib/components/MobileWorkspaceNav.svelte';
  import Palette from '../lib/components/Palette.svelte';
  import Modal from '../lib/components/Modal.svelte';
  import ThemeSelector from '../lib/components/ThemeSelector.svelte';
  import { theme } from '../lib/stores/theme.svelte';

  let { children } = $props();
  const launcher = provideLauncher();
  let palette=$state(false);
  let themePicker=$state(false);
  let uiError=$state('');
  let integrityError=$state(false);
  let readyEpoch=$state(-1);
  const standalone=$derived(page.url.pathname === '/' || page.url.pathname === '/unlock');
  const feature=$derived(page.url.pathname.split('/')[1]);
  const wizard=$derived(page.url.pathname === '/transactions/new');
  const title=$derived(feature==='reports'
    ? reports.find(item=>page.url.pathname==='/reports/'+item.path)?.label??'Reports'
    : navigation.flatMap(section=>section.items).find(([,href])=>href===page.url.pathname)?.[0]
      ?? navigation.flatMap(section=>section.items).find(([,href])=>href==='/'+feature)?.[0] ?? 'Workspace');
  const tabs=$derived(page.url.pathname.split('/').filter(Boolean).length === 1);
  onMount(() => {
    const stopTheme = theme.start();
    void launcher.initialize().then(()=>{
      if (launcher.session && page.url.pathname==='/') void goto('/dashboard');
    });
    const stops: (()=>void)[]=[stopTheme, storageSync.watch()];
    const financialDates = ['Asia/Jakarta', 'Asia/Makassar', 'Asia/Jayapura'].map(timeZone =>
      new Intl.DateTimeFormat('en-CA', { timeZone, dateStyle: 'short' }));
    const dayKey = () => financialDates.map(format => format.format(Date.now())).join('/');
    let prior = dayKey();
    const refreshFinancialDate = () => {
      const next = dayKey();
      if (prior !== next) { prior = next; session.financialDateTick++; }
    };
    const financialDateTimer = setInterval(refreshFinancialDate, 30_000);
    window.addEventListener('focus', refreshFinancialDate);
    stops.push(() => { clearInterval(financialDateTimer); window.removeEventListener('focus', refreshFinancialDate); });
    let alive=true;
    void jobs.listen().then(unlisten=>alive ? stops.push(unlisten) : unlisten()).catch(()=>{});
    void events.ledgerIntegrityAlert(([scope,error])=>{
      const active=session.active?.scope;
      if(error.code==='Integrity' && active?.sessionId===scope.sessionId && active.ledgerInstanceId===scope.ledgerInstanceId) {
        integrityError=true;
      }
    }).then(unlisten=>alive ? stops.push(unlisten) : unlisten()).catch(()=>{});
    return ()=>{alive=false;stops.forEach(stop=>stop());};
  });
  $effect(()=>{
    const epoch=session.epoch;
    const scope=untrack(()=>session.active?.scope);
    jobs.clear();
    if (scope) void untrack(()=>storageSync.refresh()).catch(()=>{});
    privacy.currencies=[];
    uiError='';
    palette=false;
    themePicker=false;
    if (scope) void Promise.all([session.call(commands.currenciesList(scope)),session.call(commands.ledgerSettingsGet(scope))]).then(([items,settings])=>{
      if (session.epoch===epoch) { privacy.currencies=items;session.timezone=settings.timezoneOverride; }
    }).catch(error=>{ if(session.epoch===epoch) uiError=message(error); }).finally(()=>{if(session.epoch===epoch)readyEpoch=epoch;});
  });
  $effect(()=>{
    if (launcher.loaded && !session.active && !standalone) void goto(launcher.challenge ? '/unlock' : '/');
  });
  $effect(()=>{
    const scope=session.active?.scope;
    integrityError=false;
    if(!scope)return;
    let active=true;
    let timer:ReturnType<typeof setTimeout>;
    async function verify(){
      try {
        const result=await commands.ledgerIntegrityStatus(scope!);
        if(!active)return;
        if(result.status==='ok' && result.data)return;
        if(result.status==='error'){
          if(result.error.code==='Integrity'){integrityError=true;return;}
          if(result.error.code==='Locked' || result.error.code==='StaleSession')return;
        }
      } catch {
        // The next poll can recover from a transient transport error.
      }
      if(active)timer=setTimeout(()=>void verify(),1000);
    }
    void verify();
    return ()=>{active=false;clearTimeout(timer);};
  });
  function shortcuts(event:KeyboardEvent) {
    if (!event.repeat && !event.isComposing && (event.metaKey || event.ctrlKey) && !event.altKey && !event.shiftKey && event.key.toLowerCase()==='k' && session.active) {
      event.preventDefault();
      palette=!palette;
    }
  }
  async function toggleMask() {
    uiError='';
    try { await privacy.toggle(); } catch (error) { uiError=message(error); }
  }
</script>

<svelte:window onkeydown={shortcuts} />
<svelte:head><title>Financial GPS</title><meta name="description" content="Track your spending, savings, and plans." /></svelte:head>

<div class="min-h-dvh bg-ground text-ink">
  <a href="#content" class="sr-only focus:not-sr-only focus:fixed focus:z-50 focus:bg-paper focus:p-3">Skip to content</a>
  {#if launcher.loaded && session.active && !standalone}
    <div class="grid min-h-dvh grid-cols-[minmax(0,1fr)] md:grid-cols-[4rem_minmax(0,1fr)] lg:grid-cols-[15.5rem_minmax(0,1fr)]">
      <WorkspaceNav/>
      <div class="ambient-canvas min-w-0 bg-background md:m-2 md:rounded-2xl md:border">
        <header class="liquid-glass-bar sticky top-0 z-20 hidden h-14 min-w-0 items-center gap-2 rounded-t-2xl border-x-0 border-t-0 px-4 md:flex lg:px-6">
          <p class="mr-auto truncate text-sm font-medium">{title}</p>
          {#if storageSync.status?.mode==='turso-sync'}
            <Badge variant="secondary" class="capitalize" aria-label={'Sync status: '+storageSync.status.phase}>{storageSync.status.phase}</Badge>
          {/if}
          <Button variant="ghost" class="tactile" type="button" onclick={()=>palette=true}>Search <kbd class="rounded-md border px-1.5 py-0.5 text-xs text-muted-foreground">Ctrl/⌘ K</kbd></Button>
          <Label for="workspace-privacy" class="gap-2 px-2">
            <Switch id="workspace-privacy" checked={privacy.masked} onCheckedChange={()=>void toggleMask()} aria-label="Hide amounts"/>
            <span class="hidden xl:inline">Hide amounts</span>
          </Label>
          <Button variant="ghost" href="/">Your activity</Button>
          <Button variant="outline" size="sm" onclick={()=>launcher.run(()=>launcher.lock())} disabled={launcher.busy}>Lock</Button>
        </header>
        <MobileWorkspaceNav {tabs} {wizard} search={()=>palette=true} {toggleMask}/>
        <main id="content" data-tabs={tabs} data-wizard={wizard} class="mobile-workspace min-w-0 px-4 py-4 md:py-6 lg:px-6">
          {#if launcher.error || uiError}<p class="error mb-4" role="alert">{launcher.error || uiError}</p>{/if}
          {#if theme.error && !themePicker}<p class="error mb-4" role="alert">{theme.error}</p>{/if}
          {#if storageSync.conflict && page.url.pathname !== '/settings'}
            <Card class="mb-4 flex flex-wrap items-center justify-between gap-3" role="status">
              <p>Cloud Sync found edits on both devices.</p>
              <Button variant="outline"  href="/settings#storage-sync">Review sync conflict</Button>
            </Card>
          {/if}
          {#if integrityError}
            <p class="error mb-4" role="alert">We couldn’t verify this activity file. Lock it and restore a verified backup, or contact support.</p>
          {:else if readyEpoch!==session.epoch}
            <p role="status">Loading activity settings…</p>
          {:else if privacy.visible(feature)}
            {#key session.epoch}{@render children()}{/key}
          {:else}
            <Card as="section" ><h1 class="page-title">This feature is turned off</h1><Button variant="outline" class="mt-4" href="/settings">Open Settings</Button></Card>
          {/if}
          {#if Object.keys(jobs.progress).length}
            <section data-tabs={tabs} aria-label="Background operations" class="fixed right-4 bottom-4 data-[tabs=true]:max-md:bottom-(--mobile-tabs-height) z-20 w-[min(22rem,calc(100%-2rem))] space-y-2">
              {#each Object.values(jobs.progress) as job (job.jobId)}
                <Card class="flex items-center justify-between gap-2" role="status">
                  <span class="text-sm capitalize">{job.phase}{job.done !== undefined ? ' · '+job.done+' rows' : ''}</span>
                  {#if job.cancellable}<Button variant="outline" onclick={()=>jobs.cancel(job.jobId).catch(error=>uiError=message(error))}>Cancel</Button>{/if}
                </Card>
              {/each}
            </section>
          {/if}
        </main>
      </div>
    </div>
  {:else}
    <div class="mx-auto w-full max-w-[68rem] px-4 pt-[calc(1rem+var(--mobile-safe-top,0px))] pb-[calc(1rem+var(--mobile-safe-bottom,0px))] md:px-9 md:py-10 lg:px-14">
      <header class="flex items-center gap-3 border-b border-line pb-5">
        <span aria-hidden="true" class="grid size-9 place-items-center rounded-md bg-accent font-mono font-bold text-accent-foreground">F</span>
        <span class="text-base font-semibold tracking-tight">Financial GPS</span>
      </header>
      <main id="content" class="pt-8 sm:pt-10">
        {#if !launcher.loaded}
          {#if launcher.error}<p class="error mb-4" role="alert">{launcher.error}</p><Button variant="outline" onclick={()=>launcher.initialize()}>Try again</Button>
          {:else}<p role="status">Loading local activity…</p>{/if}
        {:else}
          {#if launcher.error}<p class="error mb-5" role="alert">{launcher.error}</p>
          {:else if launcher.busy}<p role="status" class="mb-4 text-muted-foreground">Working…</p>{/if}
          {#if standalone}{@render children()}{/if}
        {/if}
      </main>
    </div>
  {/if}
  {#if themePicker && session.active}
    <Modal title="Theme previews" close={()=>themePicker=false}>
      {#if theme.error}<p class="error mb-3" role="alert">{theme.error}</p>{/if}
      <ThemeSelector/>
    </Modal>
  {/if}
  {#if palette && session.active}<Palette close={()=>palette=false} openThemes={()=>themePicker=true}/>{/if}
</div>
