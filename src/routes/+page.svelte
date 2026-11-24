<script lang="ts">
  const modalForm = $props.id();
  import { Button } from '#lib/components/ui/button/index.js';
  import { Card } from '#lib/components/ui/card/index.js';
  import { Label } from '#lib/components/ui/label/index.js';
  import { Input, RadioInput } from '#lib/components/ui/input/index.js';
  import { NativeSelect } from '#lib/components/ui/native-select/index.js';
  import { NativeCheckbox } from '#lib/components/ui/checkbox/index.js';
  import type { AccountDraft_Deserialize, CalendarInput, LedgerStorageMode, Money, PathGrant } from '../lib/drivers/runtime';
  import { useLauncher } from '../lib/launcher.svelte';
  import { saveStorageMode, storageMessage, testConnection } from '../lib/drivers/storage-credentials';
  import { storageSync } from '../lib/stores/storage-sync.svelte';
  import TursoCredentials from '../lib/components/TursoCredentials.svelte';
  import AccountSetup from '../lib/views/AccountSetup.svelte';
  import { setupCurrencies } from '../lib/account-types';
  import { commands } from '../lib/drivers/runtime';
  import { session } from '../lib/stores/session.svelte';
  import { goto } from '$app/navigation';
  import Modal from '../lib/components/Modal.svelte';
  import RestoreFlow from '../lib/views/RestoreFlow.svelte';

  const launcher = useLauncher();
  const currencies = setupCurrencies.map(item => item.code);
  let advanced = $state(false);
  let setupStarted = $state(false);
  let setupScope = $state<string>();
  let setupRequest = $state<string>();
  const guided = $derived(launcher.loaded && !advanced && (!launcher.ledgers.length || setupStarted));
  function initialSetup(draft: AccountDraft_Deserialize, startingBalance: Money, calendar?: CalendarInput) {
    setupStarted = true;
    void launcher.run(async () => {
      async function saveAccount() {
        const scope = session.scope;
        setupScope = scope.sessionId;
        setupRequest ??= crypto.randomUUID();
        await session.call(commands.accountSave({ scope, requestId: setupRequest }, { draft, startingBalance }));
        if (calendar) await session.call(commands.financialCalendarSave(session.mutation(), calendar));
      }
      if (setupScope && launcher.session?.scope.sessionId === setupScope) {
        await saveAccount();
        await goto('/dashboard');
      } else {
        await launcher.create({ name: 'Personal activity', baseCurrency: draft.currencyCode,
          protection: 'plain', rememberKey: false }, saveAccount);
      }
    });
  }
  let name = $state('');
  let restoring = $state(false);
  let baseCurrency = $state('IDR');
  let encrypted = $state(false);
  let storageMode = $state<LedgerStorageMode>('local-isolated');
  let url = $state(''), token = $state(''), storageError = $state(''), connectionMessage = $state('');
  const connection = () => ({ url: url.trim(), authToken: token });

  function verifyConnection() {
    storageError = connectionMessage = '';
    void launcher.run(async () => {
      try {
        await testConnection(connection());
        connectionMessage = 'Connection verified.';
      } catch (error) { storageError = storageMessage(error); }
    });
  }
  let credentialDialog = $state(false);
  let destination = $state<PathGrant>();
  const active = $derived(launcher.session);

  function prepareCreate(event: SubmitEvent) {
    event.preventDefault();
    if (launcher.busy) return;
    storageError = connectionMessage = '';
    if (encrypted) {
      credentialDialog = true;
    } else {
      void create();
    }
  }

  function confirmCredential(event: SubmitEvent) {
    event.preventDefault();
    const form = event.currentTarget as HTMLFormElement;
    const credential = (form.elements.namedItem('credential') as HTMLInputElement).value;
    const rememberKey = (form.elements.namedItem('rememberKey') as HTMLInputElement).checked;
    form.reset();
    credentialDialog = false;
    void create(credential, rememberKey);
  }

  async function create(credential?: string, rememberKey = false) {
    await launcher.run(async () => {
      const cloud = storageMode === 'turso-sync';
      try {
        if (cloud) {
          try { await testConnection(connection()); }
          catch (error) { storageError = storageMessage(error); return; }
        }
        await launcher.create({
          name,
          baseCurrency,
          protection: encrypted ? 'encrypted' : 'plain',
          rememberKey,
          credential,
          destination: cloud ? undefined : destination?.id
        }, cloud ? async ledger => {
          try {
            const status = await saveStorageMode(ledger.scope, { mode: 'turso-sync', ...connection() });
            storageSync.receive(ledger.scope, status);
          } catch (error) {
            storageError = `Activity created in Local Only. Open Storage & Sync to finish enrollment. ${storageMessage(error)}`;
            throw new Error(storageError);
          }
        } : undefined);
        name = '';
        encrypted = false;
        storageMode = 'local-isolated';
      } catch (error) {
        if (!storageError) throw error;
      } finally {
        token = '';
        destination = undefined;
      }
    });
  }
</script>

<header class="mb-6 flex flex-wrap items-center justify-between gap-3"><h1 class="page-title">Your activity</h1><Button variant="outline" disabled={launcher.busy} onclick={()=>restoring=true}>Restore from backup</Button></header>
{#if restoring}<RestoreFlow close={()=>restoring=false}/>{/if}

{#if guided}
  <AccountSetup initial busy={launcher.busy} locked={!!setupScope} complete={initialSetup} />
  <div class="mx-auto mt-4 max-w-2xl">
    <Button variant="outline" type="button" disabled={launcher.busy} onclick={() => advanced = true}>More setup options</Button>
  </div>
{:else}
{#if active}
  <Card as="section" class="mb-5 flex flex-wrap items-center justify-between gap-4">
    <div>
      <div class="flex flex-wrap items-center gap-2">
        <span class="size-2 rounded-full bg-status" aria-hidden="true"></span>
        <h2 class="text-sm font-medium text-muted-foreground">Activity ready</h2>
      </div>
      <p class="mt-1 break-words text-xl font-semibold text-ink">{active.name}</p>
      <p class="mt-1 text-sm text-muted-foreground">{active.baseCurrency} · {active.protection === 'encrypted' ? 'Encrypted' : 'Unencrypted'}</p>
    </div>
    <div class="flex flex-wrap gap-2">
      <Button variant="default"  href="/dashboard">Dashboard</Button>
      <Button variant="outline"  href="/settings#storage-sync">Storage &amp; Sync</Button>
      <Button variant="outline" type="button" disabled={launcher.busy} onclick={() => launcher.run(() => launcher.lock())}>Lock</Button>
      <Button variant="outline" type="button" disabled={launcher.busy} onclick={() => launcher.run(() => launcher.close())}>Close activity</Button>
    </div>
  </Card>
{/if}

<div class="grid gap-5 md:grid-cols-[1.15fr_1fr]">
  <Card as="section" class="self-start" aria-labelledby="saved-ledgers">
    <div class="mb-4 flex flex-wrap items-center justify-between gap-3">
      <h2 id="saved-ledgers" class="text-lg font-semibold text-ink">Saved activity</h2>
      <Button variant="outline" type="button" disabled={launcher.busy} onclick={() => launcher.run(() => launcher.chooseExisting())}>Open file…</Button>
    </div>
    {#if launcher.ledgers.length}
      <ul class="divide-y divide-line">
        {#each launcher.ledgers as ledger (ledger.ledgerInstanceId)}
          {@const selected = active?.scope.ledgerInstanceId === ledger.ledgerInstanceId}
          <li class="flex items-center justify-between gap-3 py-3">
            <div class="min-w-0">
              <p class="break-words font-medium text-ink">{ledger.label}</p>
              <p class="text-sm text-muted-foreground">
                {ledger.protection === 'encrypted' ? 'Encrypted' : 'Unencrypted'} · {ledger.storagePolicy === 'portable' ? 'Portable file' : 'App storage'}
              </p>
            </div>
            {#if selected}
              <span class="text-sm font-medium text-status">Open</span>
            {:else}
              <Button variant="outline" type="button" disabled={launcher.busy}
                aria-label={`Open ${ledger.label}`}
                onclick={() => launcher.run(() => launcher.open({ ledgerInstanceId: ledger.ledgerInstanceId }))}>
                Open
              </Button>
            {/if}
          </li>
        {/each}
      </ul>
    {:else}
      <p class="rounded-lg bg-ground px-4 py-5 text-sm text-muted-foreground">Create activity or open an existing activity file.</p>
    {/if}
  </Card>

  <Card as="section"  aria-labelledby="new-ledger">
    <h2 id="new-ledger" class="mb-4 text-lg font-semibold text-ink">Create activity</h2>
    <form onsubmit={prepareCreate} autocomplete="off">
      <fieldset disabled={launcher.busy} class="grid min-w-0 gap-4">
        <Label class="field">
          <span>Name</span>
          <Input type="text" name="name" maxlength={256} placeholder="Personal finances" required bind:value={name} />
        </Label>
        <Label class="field">
          <span>Display currency</span>
          <NativeSelect name="baseCurrency" bind:value={baseCurrency}>
            {#each currencies as currency (currency)}<option value={currency}>{currency}</option>{/each}
          </NativeSelect>
        </Label>
        <fieldset class="grid gap-3 rounded-lg border border-line px-3 pb-3">
          <legend class="px-1 font-semibold">Storage mode</legend>
          <Label class="check"><RadioInput name="storage-mode" value="local-isolated" checked={storageMode==='local-isolated'}
            onchange={() => { storageMode='local-isolated'; token = storageError = connectionMessage = ''; }} /> Local Only</Label>
          <p class="text-sm text-muted-foreground">Data stays on this device or in this browser. Activity storage makes zero network requests and cannot sync across platforms.</p>
          <Label class="check"><RadioInput name="storage-mode" value="turso-sync" checked={storageMode==='turso-sync'} onchange={()=>storageMode='turso-sync'} /> Cloud Sync</Label>
          <p class="text-sm text-muted-foreground">Desktop, Mobile, and Web sync independently through your own Turso database.</p>
        </fieldset>
        {#if storageMode === 'turso-sync'}
          <TursoCredentials bind:url bind:token onEdit={() => { storageError = connectionMessage = ''; }} />
          <Button variant="outline" type="button" class="justify-self-start" disabled={!url.trim() || !token} onclick={verifyConnection}>Test Connection</Button>
          {#if connectionMessage}<p role="status" class="text-sm text-status">{connectionMessage}</p>{/if}
        {:else}
        <div class="flex flex-wrap items-center justify-between gap-2 text-sm">
          <span class="break-all text-muted-foreground">{destination?.displayName ?? 'App storage'}</span>
          <div class="flex flex-wrap items-center gap-2">
            {#if destination}
              <Button variant="outline" type="button" onclick={() => { destination = undefined; }}>Use app storage</Button>
            {/if}
            <Button variant="outline" type="button" onclick={() => launcher.run(async () => {
              const grant = await launcher.chooseDestination();
              if (grant) destination = grant;
            })}>Choose location…</Button>
          </div>
        </div>
        {/if}
        {#if storageError}<p class="error text-sm" role="alert">{storageError}</p>{/if}
        <Label class="check border-t border-line pt-4">
          <NativeCheckbox  bind:checked={encrypted} />
          <span>Protect this activity with a passphrase</span>
        </Label>
        <Button variant="default" type="submit" class="mt-1">Create activity</Button>
      </fieldset>
    </form>
  </Card>
</div>

{#if credentialDialog}
<Modal title="Protect your activity" close={()=>credentialDialog=false}>
  <form id={modalForm + '-credential'} onsubmit={confirmCredential} autocomplete="off" class="grid gap-4">
    <Label class="field">
      <span>Passphrase or PIN</span>
      <Input type="password" name="credential" autocomplete="off" maxlength={1024}
        aria-describedby="protection-help" required />
      <span id="protection-help" class="text-xs font-normal text-muted-foreground">A longer passphrase offers stronger protection.</span>
    </Label>
    <Label class="check">
      <NativeCheckbox  name="rememberKey" />
      <span>Remember this activity on this device for quick unlock</span>
    </Label>
  </form>
  {#snippet footer()}
    <div class="flex flex-wrap gap-2 justify-end">
      <Button variant="outline" type="button" onclick={() => credentialDialog=false}>Cancel</Button>
      <Button variant="default"  type="submit" form={modalForm + '-credential'}>Create protected activity</Button>
    </div>
  {/snippet}
</Modal>
{/if}

{/if}
