<script lang="ts">
  import { page } from '$app/state';
  import { Badge } from '#lib/components/ui/badge/index.js';
  import { Button } from '#lib/components/ui/button/index.js';
  import * as Menu from '#lib/components/ui/dropdown-menu/index.js';
  import * as Tooltip from '#lib/components/ui/tooltip/index.js';
  import { Separator } from '#lib/components/ui/separator/index.js';
  import { routeIcon } from '#lib/icons/registry.js';
  import NavIcon from '#lib/icons/NavIcon.svelte';
  import { navigation } from '../navigation';
  import { useLauncher } from '../launcher.svelte';
  import { session } from '../stores/session.svelte';
  import { privacy } from '../stores/privacy.svelte';
  import { storageSync } from '../stores/storage-sync.svelte';

  const launcher = useLauncher();
  const groups = ['Overview','Money','Planning','System'];
  const mode = $derived(storageSync.status?.mode==='turso-sync' ? 'Turso Sync' : storageSync.status ? 'Local' : '');

  function current(href: string) {
    const path = page.url.pathname;
    return path===href || (href.startsWith('/reports/') && path.startsWith('/reports/')) ||
      (href!=='/settings' && path.startsWith(href+'/'));
  }
</script>

<aside class="sticky top-0 hidden h-dvh flex-col gap-3 overflow-y-auto border-r border-sidebar-border bg-sidebar p-2 md:flex lg:p-4">
  <a href="/dashboard" class="flex min-h-9 items-center justify-center gap-3 rounded-lg font-semibold no-underline lg:justify-start" aria-label="Financial GPS">
    <span aria-hidden="true" class="grid size-8 place-items-center rounded-lg bg-accent font-mono text-accent-foreground">F</span>
    <span class="hidden lg:inline">Financial GPS</span>
  </a>
  <Menu.Root>
    <Menu.Trigger>
      {#snippet child({ props })}
        <Button {...props} variant="outline" aria-label="Open activity" class="tactile min-w-0 lg:w-full lg:justify-start lg:px-2" disabled={launcher.busy}>
          <NavIcon name="ledger"/>
          <span class="hidden min-w-0 flex-1 truncate text-left lg:block">{session.active?.name}</span>
          {#if mode}<Badge variant="secondary" class="hidden lg:inline-flex">{mode}</Badge>{/if}
        </Button>
      {/snippet}
    </Menu.Trigger>
    <Menu.Content align="start" class="max-w-[calc(100vw-2rem)] min-w-52">
      <Menu.Label>Switch activity</Menu.Label>
      {#each launcher.ledgers as item (item.ledgerInstanceId)}
        <Menu.Item disabled={item.ledgerInstanceId===session.active?.scope.ledgerInstanceId}
          onSelect={()=>void launcher.run(()=>launcher.open({ledgerInstanceId:item.ledgerInstanceId}))}>{item.label}</Menu.Item>
      {/each}
      <Menu.Separator/>
      <Menu.Item onSelect={()=>void launcher.run(()=>launcher.lock())}>Lock activity</Menu.Item>
    </Menu.Content>
  </Menu.Root>
  <Button href="/transactions/new" aria-label="Add transaction" class="px-2 lg:justify-start">
    <NavIcon name="add"/><span class="hidden lg:inline">New transaction</span>
  </Button>
  <Separator/>
  <Tooltip.Provider delayDuration={200}>
    <nav aria-label="Main navigation" class="grid gap-3 lg:gap-4">
      {#each navigation as section,index (section.label)}
        {@const items=section.items.filter(([,path])=>privacy.visible(path.split('/')[1]))}
        {#if items.length}
          <div class="grid gap-1">
            <p class="hidden px-2 text-xs font-medium text-muted-foreground lg:block">{groups[index]}</p>
            {#each items as [label,href] (href)}
              <Tooltip.Root>
                <Tooltip.Trigger>
                  {#snippet child({ props })}
                    <a {...props} {href} aria-label={label} aria-current={current(href)?'page':undefined}
                      class="nav-link min-w-0 justify-center lg:justify-start">
                      <NavIcon name={routeIcon(href)} active={current(href)}/><span class="hidden truncate lg:inline">{label}</span>
                    </a>
                  {/snippet}
                </Tooltip.Trigger>
                <Tooltip.Content side="right" class="lg:hidden">{label}</Tooltip.Content>
              </Tooltip.Root>
            {/each}
          </div>
        {/if}
      {/each}
    </nav>
  </Tooltip.Provider>
</aside>
