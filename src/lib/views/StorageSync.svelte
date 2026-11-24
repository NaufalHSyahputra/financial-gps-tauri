<script lang="ts">
  import { Card } from '#lib/components/ui/card/index.js';
  import { Button } from '#lib/components/ui/button/index.js';
  import { Label } from '#lib/components/ui/label/index.js';
  import { Input } from '#lib/components/ui/input/index.js';
  import { NativeCheckbox } from '#lib/components/ui/checkbox/index.js';
  import { commands, type LedgerStorageMode, type SyncStatus_Serialize } from '../drivers/runtime';
  import { saveStorageMode, storageMessage, testConnection, testSavedConnection } from '../drivers/storage-credentials';
  import { session } from '../stores/session.svelte';
  import { storageSync } from '../stores/storage-sync.svelte';
  import { jobs } from '../stores/jobs.svelte';
  import Modal from '../components/Modal.svelte';
  import TursoCredentials from '../components/TursoCredentials.svelte';

  let { onLedgerChanged }: { onLedgerChanged: () => Promise<void> } = $props();
  const status = $derived(storageSync.status);
  const cloud = $derived(status?.mode === 'turso-sync');
  const conflict = $derived(storageSync.conflict);
  type Conflict = NonNullable<typeof conflict>;
  let dialog = $state<'enroll' | 'rotate' | 'local' | 'conflict'>();
  let selection = $state<Conflict>();
  let url = $state(''), token = $state(''), passphrase = $state('');
  let error = $state(''), note = $state('');
  let busy = $state(false), replacementConfirmed = $state(false);
  const phaseLabels = { idle: 'Idle', syncing: 'Syncing', offline: 'Offline', error: 'Error' };

  function masked(endpoint: string) {
    const host = new URL(endpoint).hostname;
    const label = host.split('.')[0];
    return `${label.slice(0, 2)}•••.turso.io`;
  }

  function open(next: NonNullable<typeof dialog>) {
    url = next === 'rotate' ? status?.url ?? '' : '';
    token = passphrase = error = note = '';
    replacementConfirmed = false;
    selection = next === 'conflict' && conflict ? { ...conflict } : undefined;
    dialog = next;
  }

  function close() {
    if (busy) return;
    dialog = undefined;
    token = passphrase = error = note = '';
  }

  async function run(work: () => Promise<void>) {
    if (busy) return;
    busy = true;
    error = note = '';
    try { await work(); }
    catch (cause) { error = storageMessage(cause); }
    finally { busy = false; }
  }

  const credentials = () => ({ url: url.trim(), authToken: token });

  function verify() {
    void run(async () => {
      await testConnection(credentials());
      note = 'Connection verified.';
    });
  }

  function switchMode(mode: LedgerStorageMode) {
    void run(async () => {
      const scope = session.scope;
      const next = await saveStorageMode(scope, mode === 'turso-sync'
        ? { mode, ...credentials() } : { mode });
      storageSync.receive(scope, next);
      note = dialog === 'rotate' ? 'Credentials updated.' : 'Storage mode updated.';
      dialog = undefined;
      token = '';
    });
  }

  function syncNow() {
    void run(async () => {
      const scope = session.scope;
      const next = await session.call(commands.ledgerSyncNow(scope));
      storageSync.receive(scope, next);
      if (next.phase === 'idle') note = 'Sync complete.';
    });
  }

  function exportBackup(event: SubmitEvent) {
    event.preventDefault();
    void run(async () => {
      const scope = session.scope;
      const [grant] = await session.call(commands.fileChoose({ purpose: 'backupWrite' }));
      if (!grant) return;
      const ticket = await session.call(commands.backupCreateStart(scope, {
        requestId: crypto.randomUUID(), destination: grant.id, backupPassphrase: passphrase
      }));
      passphrase = '';
      const backup = await jobs.result(ticket, 'backupCreate', scope);
      note = `Encrypted safety backup exported as ${backup.fileName}.`;
    });
  }

  function resolve(choice: 'local' | 'remote') {
    void run(async () => {
      if (!selection) return;
      const scope = session.scope;
      const next = await session.call(choice === 'local'
        ? commands.ledgerSyncPublishLocal(scope, selection.generation)
        : commands.ledgerSyncAcceptRemote(scope, {
          expectedRemoteGeneration: selection.generation,
          expectedLocalVersion: selection.localVersion
        }));
      storageSync.receive(scope, next);
      if (next.error) {
        if (storageSync.conflict) selection = { ...storageSync.conflict };
        replacementConfirmed = false;
        throw next.error;
      }
      dialog = undefined;
      passphrase = '';
      note = choice === 'local' ? 'Local version published.' : 'Remote version applied.';
      await onLedgerChanged();
    });
  }
</script>

<Card as="section" id="storage-sync" class="lg:col-span-2 scroll-mt-4" aria-labelledby="storage-sync-title">
  <div class="flex flex-wrap items-center justify-between gap-2">
    <h2 id="storage-sync-title" class="section-title">Storage &amp; Sync</h2>
    {#if status}
      <span class="rounded-full border border-line bg-ground px-3 py-1 text-sm font-medium">
        {cloud ? 'Turso Cloud Sync' : 'Local Only'}
      </span>
    {/if}
  </div>
  {#if !status}
    <p class="mt-3 text-sm text-muted-foreground" role="status">Loading storage status…</p>
  {:else}
    <p class="mt-3 text-sm text-muted-foreground">
      {cloud ? 'This activity syncs with your Turso database. Edits are saved on this device first.'
        : 'Data stays on this device or in this browser. Activity storage makes zero network requests and cannot sync across platforms.'}
    </p>
    {#if cloud}
      <dl class="my-4 grid grid-cols-2 gap-3 sm:grid-cols-4">
        <div><dt class="text-sm text-muted-foreground">Turso endpoint</dt><dd class="font-medium">{status.url ? masked(status.url) : 'Unavailable'}</dd></div>
        <div><dt class="text-sm text-muted-foreground">Sync status</dt><dd class="font-medium">{phaseLabels[status.phase]}</dd></div>
        <div><dt class="text-sm text-muted-foreground">Last successful sync</dt><dd class="font-medium">
          {#if status.lastSuccessAt}<time datetime={status.lastSuccessAt}>{new Date(status.lastSuccessAt).toLocaleString()}</time>{:else}Never{/if}
        </dd></div>
        <div><dt class="text-sm text-muted-foreground">Pending outbox changes</dt><dd class="font-mono font-medium">{status.pendingCount}</dd></div>
      </dl>
      {#if conflict}
        <div class="mb-4 flex flex-wrap items-center justify-between gap-3 rounded-lg border border-line bg-ground p-3">
          <p>Edits on both devices need review.</p>
          <Button variant="outline" type="button" onclick={() => open('conflict')}>Review sync conflict</Button>
        </div>
      {:else if status.error}<p class="mb-3 text-sm text-muted-foreground" role="status">{storageMessage(status.error)}</p>{/if}
    {/if}
    <div class="mt-4 flex flex-wrap gap-2">
      {#if cloud}
        <Button variant="outline" type="button" disabled={busy || status.phase === 'syncing'} onclick={() => run(async () => {
          await testSavedConnection(session.scope);
          note = 'Connection verified.';
        })}>Test Connection</Button>
        <Button variant="default" type="button"  disabled={busy || status.phase === 'syncing'} onclick={syncNow}>Sync Now</Button>
        <Button variant="outline" type="button" disabled={busy} onclick={() => open('rotate')}>Update credentials</Button>
      {/if}
      <Button variant="outline" type="button" disabled={busy} onclick={() => open(cloud ? 'local' : 'enroll')}>Switch Mode</Button>
    </div>
    {#if !dialog && note}<p role="status" class="mt-3 text-sm text-status">{note}</p>{/if}
    {#if !dialog && error}<p role="alert" class="error mt-3 text-sm">{error}</p>{/if}
  {/if}
</Card>

{#if dialog}
  <Modal title={dialog === 'enroll' ? 'Switch to Cloud Sync' : dialog === 'local' ? 'Switch to Local Only'
      : dialog === 'rotate' ? 'Update Turso credentials' : 'Resolve sync conflict'} {close}>
    {#if dialog === 'enroll' || dialog === 'rotate'}
      <form autocomplete="off" onsubmit={event => { event.preventDefault(); switchMode('turso-sync'); }}>
        <fieldset disabled={busy} class="grid gap-3">
          <p class="text-sm text-muted-foreground">{dialog === 'enroll'
            ? 'Confirm enrollment to sync this activity through your Turso database on Desktop, Mobile, and Web.'
            : 'Enter a new token for the enrolled database. Saving replaces the token on this device.'}</p>
          <TursoCredentials bind:url bind:token lockedUrl={dialog === 'rotate'} onEdit={() => { error = note = ''; }} />
          <div class="flex flex-wrap justify-end gap-2">
            <Button variant="outline" type="button" onclick={close}>Cancel</Button>
            <Button variant="outline" type="button" disabled={!url.trim() || !token} onclick={verify}>Test Connection</Button>
            <Button variant="default"  type="submit">{dialog === 'enroll' ? 'Confirm Cloud Sync' : 'Save new token'}</Button>
          </div>
        </fieldset>
      </form>
    {:else if dialog === 'local'}
      <p class="mb-4 text-sm text-muted-foreground">This device will retain its current activity and remove its saved Turso token. Existing copies on Turso and other devices will remain there.</p>
      <div class="flex flex-wrap justify-end gap-2">
        <Button variant="outline" type="button" disabled={busy} onclick={close}>Cancel</Button>
        <Button variant="default" type="button"  disabled={busy} onclick={() => switchMode('local-isolated')}>Confirm Local Only</Button>
      </div>
    {:else if selection}
      <dl class="mb-4 grid grid-cols-2 gap-3 text-sm">
        <div><dt class="text-muted-foreground">Local edit version</dt><dd class="font-mono">{selection.localVersion}</dd></div>
        <div><dt class="text-muted-foreground">Remote generation</dt><dd class="font-mono">{selection.generation}</dd></div>
        <div><dt class="text-muted-foreground">Last shared generation on this device</dt><dd class="font-mono">{selection.base ?? '—'}</dd></div>
      </dl>
      <form autocomplete="off" class="mb-4 grid gap-2 border-b border-line pb-4" onsubmit={exportBackup}>
        <Label class="field">Safety backup passphrase<Input type="password" autocomplete="new-password" required bind:value={passphrase} disabled={busy}/></Label>
        <Button variant="outline" type="submit" class="justify-self-start" disabled={busy}>Export encrypted safety backup</Button>
      </form>
      <p class="mb-3 text-sm text-muted-foreground">Keeping local publishes this device’s version as a new generation. Accepting remote replaces the pending edits on this device.</p>
      <Label class="check mb-4"><NativeCheckbox  bind:checked={replacementConfirmed} disabled={busy}/> I understand that accepting remote replaces my pending local edits.</Label>
      <div class="flex flex-wrap justify-end gap-2">
        <Button variant="outline" type="button" disabled={busy} onclick={close}>Cancel</Button>
        <Button variant="outline" type="button" disabled={busy} onclick={() => resolve('local')}>Keep Local Version</Button>
        <Button variant="destructive" type="button"  disabled={busy || !replacementConfirmed} onclick={() => resolve('remote')}>Accept Remote Version</Button>
      </div>
    {/if}
    {#if busy}<p class="mt-3 text-sm text-muted-foreground" role="status">Working…</p>{/if}
    {#if note}<p class="mt-3 text-sm text-status" role="status">{note}</p>{/if}
    {#if error}<p class="error mt-3 text-sm" role="alert">{error}</p>{/if}
  </Modal>
{/if}
