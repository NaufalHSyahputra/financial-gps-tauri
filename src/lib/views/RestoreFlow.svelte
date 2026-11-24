<script lang="ts">
  const modalForm = $props.id();
  import { Button } from '#lib/components/ui/button/index.js';
  import { Label } from '#lib/components/ui/label/index.js';
  import { Input } from '#lib/components/ui/input/index.js';
  import { NativeSelect } from '#lib/components/ui/native-select/index.js';
  import { NativeCheckbox } from '#lib/components/ui/checkbox/index.js';
  import { goto } from '$app/navigation';
  import { commands, type RestorePreview, type Protection, type PathGrant } from '../drivers/runtime';
  import { session } from '../stores/session.svelte';
  import { privacy } from '../stores/privacy.svelte';
  import { jobs } from '../stores/jobs.svelte';
  import { createTask } from '../stores/task.svelte';
  import { useLauncher } from '../launcher.svelte';
  import Modal from '../components/Modal.svelte';
  import Feedback from '../components/Feedback.svelte';
  let { backup, close }: {backup?:{id:string;fileName:string};close:()=>void}=$props();
  const launcher=useLauncher();
  const task=createTask();
  let source=$state<PathGrant>(),destination=$state<PathGrant>();
  let passphrase=$state(''),name=$state(''),credential=$state(''),confirmCredential=$state(''),safety=$state('');
  let preview=$state<RestorePreview>(), phase=$state(''), jobId=$state('');
  let mode=$state<'new'|'replace'>('new'),protection=$state<Protection>('encrypted'),acknowledge=$state(false);
  function pick(purpose:'restoreRead'|'ledgerCreate'){
    void task.run(async()=>{
      const [grant]=await session.call(commands.fileChoose({purpose}),false);
      if(!grant)return;
      if(purpose==='restoreRead')source=grant;else destination=grant;
    });
  }
  function verify(event:SubmitEvent){
    event.preventDefault();void task.run(async()=>{
      const ticket=await session.call(commands.restorePreviewStart({fileGrant:source?.id??null,scopedBackupId:backup?.id??null,backupPassphrase:passphrase}),false);
      passphrase='';source=undefined;phase='Verifying backup…';jobId=ticket.id;
      try {preview=await jobs.result(ticket,'restorePreview',null);}
      finally {phase='';jobId='';}
    });
  }
  function apply(event:SubmitEvent){
    event.preventDefault();void task.run(async()=>{
      if(!preview)return;
      if(protection==='encrypted' && credential!==confirmCredential)throw new Error('The new credentials must match.');
      const previewToken=preview.previewToken;
      let confirm:string|null=null;
      const targetId=session.active?.scope.ledgerInstanceId;
      if(mode==='replace'){
        if(!acknowledge || !targetId)throw new Error('Confirm replacement of the open activity.');
        confirm=(await session.call(commands.restoreConfirmReplacement(session.scope,{previewToken,targetId}))).token;
      }
      const target=mode==='replace'&&targetId ? {kind:'replace' as const,targetId} : {kind:'new' as const,name,destination:destination?.id??null};
      const ticket=await session.call(commands.restoreApplyStart({requestId:crypto.randomUUID(),previewToken,target,protection,outputCredential:protection==='encrypted'?credential:null,confirm,safetyBackupPassphrase:mode==='replace'&&safety?safety:null}),false);
      credential='';confirmCredential='';safety='';phase='Restoring activity…';jobId=ticket.id;
      try {
        const result=await jobs.result(ticket,'restoreApply',null);
        session.set(result);
        close();await goto('/dashboard');await launcher.refresh();
      } finally {phase='';jobId='';}
    });
  }
</script>
<Modal title="Restore encrypted backup" close={()=>{if(!task.busy)close();}}>
  {#if !preview}
    <form id={modalForm + '-verify'} class="space-y-3" autocomplete="off" onsubmit={verify}>
      <div class="flex flex-wrap items-center gap-3"><span class="break-all text-sm text-muted-foreground">{backup?.fileName??source?.displayName??'Choose an archive'}</span>{#if !backup}<Button variant="outline" type="button" disabled={task.busy} onclick={()=>pick('restoreRead')}>Choose backup…</Button>{/if}</div>
      <Label class="field">Archive passphrase<Input type="password" autocomplete="off" bind:value={passphrase} required/></Label>
      <Feedback error={task.error}/>
    </form>
  {:else}
    <section class="mb-4 rounded border border-line bg-ground p-3">
      <h3 class="section-title">Verified backup</h3>
      <p class="text-sm text-muted-foreground">Created {privacy.date(preview.manifest.createdAt)} · Schema {preview.manifest.schemaVersion}</p>
      <dl class="mt-2 grid grid-cols-2 gap-2 text-sm">
        {#each Object.entries(preview.manifest.counts) as [key,count] (key)}<div><dt class="text-muted-foreground">{key.replaceAll('_',' ')}</dt><dd>{count}</dd></div>{/each}
        <div><dt class="text-muted-foreground">Attachments</dt><dd>{preview.manifest.attachments.length}</dd></div>
      </dl>
      {#if preview.warnings.includes('localCredentialsExcluded')}<p class="mt-2 text-sm text-muted-foreground">Credentials stored in the secure vault are excluded from the archive.</p>{/if}
      {#if mode==='new' && preview.warnings.includes('newLedgerIdentity')}<p class="mt-2 text-sm text-muted-foreground">Restoring as new gives the activity file a new identity.</p>{/if}
    </section>
    <form id={modalForm + '-restore'} class="space-y-3" autocomplete="off" onsubmit={apply}>
      {#if session.active}<Label class="field">Restore destination<NativeSelect bind:value={mode} onchange={()=>acknowledge=false}><option value="new">Create a new activity file</option><option value="replace">Replace the open activity</option></NativeSelect></Label>{/if}
      {#if mode==='new'}
        <Label class="field">New activity name<Input bind:value={name} maxlength={256} required/></Label>
        <div class="flex flex-wrap items-center gap-2"><span class="text-sm text-muted-foreground">{destination?.displayName??'App storage'}</span><Button variant="outline" type="button" disabled={task.busy} onclick={()=>pick('ledgerCreate')}>Choose location…</Button>{#if destination}<Button variant="outline" type="button" onclick={()=>destination=undefined}>Use app storage</Button>{/if}</div>
      {:else}
        <p class="text-sm">The current contents of <strong>{session.active?.name}</strong> will be replaced after a safety backup is created.</p>
        <Label class="field">Safety backup passphrase<Input type="password" autocomplete="off" bind:value={safety} placeholder="Uses remembered backup credential if blank"/></Label>
        <Label class="check"><NativeCheckbox  bind:checked={acknowledge} required/> Replace the current activity with this backup</Label>
      {/if}
      <Label class="field">Restored activity protection<NativeSelect bind:value={protection}><option value="encrypted">Encrypted</option><option value="plain">Unencrypted</option></NativeSelect></Label>
      {#if protection==='encrypted'}
        <Label class="field">New activity passphrase or PIN<Input type="password" autocomplete="off" bind:value={credential} required/></Label>
        <Label class="field">Repeat new credential<Input type="password" autocomplete="off" bind:value={confirmCredential} required/></Label>
      {/if}
      <Feedback error={task.error}/>
    </form>
  {/if}
  {#if phase}<div class="mt-3 flex items-center gap-3"><p role="status" class="text-sm text-muted-foreground">{phase}</p></div>{/if}
  {#snippet footer()}
    <div class="flex flex-wrap gap-2">
      {#if !preview}<Button variant="default" type="submit" form={modalForm + '-verify'}  disabled={task.busy || !backup && !source}>Verify backup</Button>{:else}<Button variant="outline" type="submit" form={modalForm + '-restore'} class={mode==='replace'?'danger':'primary'} disabled={task.busy}>{mode==='replace'?'Confirm replacement':'Restore as new activity'}</Button>{/if}
      {#if phase}{#if jobs.progress[jobId]?.cancellable}<Button variant="outline" type="button" onclick={()=>void jobs.cancel(jobId).catch(()=>{})}>Cancel operation</Button>{/if}{/if}
    </div>
  {/snippet}
</Modal>
