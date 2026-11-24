<script lang="ts">
  const modalForm = $props.id();
  import RowActions from '#lib/components/RowActions.svelte';
  import { Button } from '#lib/components/ui/button/index.js';
  import { Card } from '#lib/components/ui/card/index.js';
  import { Table, TableHeader, TableRow, TableHead, TableBody, TableCell } from '#lib/components/ui/table/index.js';
  import { Label } from '#lib/components/ui/label/index.js';
  import { Input } from '#lib/components/ui/input/index.js';
  import { NativeCheckbox } from '#lib/components/ui/checkbox/index.js';
  import { NativeSelect } from '#lib/components/ui/native-select/index.js';
  import { commands, type BackupFrequency, type BackupSchedule_Serialize, type BackupSummary_Serialize, type PathGrant } from '../../../lib/drivers/runtime';
  import { session, pageRequest } from '../../../lib/stores/session.svelte';
  import { privacy } from '../../../lib/stores/privacy.svelte';
  import { jobs } from '../../../lib/stores/jobs.svelte';
  import { query, createTask } from '../../../lib/stores/task.svelte';
  import Heading from '../../../lib/components/Heading.svelte';
  import Feedback from '../../../lib/components/Feedback.svelte';
  import Modal from '../../../lib/components/Modal.svelte';
  import RestoreFlow from '../../../lib/views/RestoreFlow.svelte';
  let cursor=$state<string|null>(null);
  const backups=query(()=>session.call(commands.backupList(session.scope,{page:pageRequest(cursor,40)})));
  const schedules=query(()=>session.call(commands.backupScheduleGet(session.scope)));
  const manualTask=createTask(),scheduleTask=createTask(),removeTask=createTask();
  let mode=$state<'manual'|'schedule'|'restore'|'remove'|null>(null);
  let selected=$state<BackupSummary_Serialize>(), file=$state<PathGrant>(), directory=$state<PathGrant>();
  let password=$state(''),scheduleSecret=$state(''),note=$state('');
  let enabled=$state(false),frequency=$state<BackupFrequency>('daily'),time=$state('19:00'),weekday=$state(1),retainDays=$state<number|undefined>();
  let willRemember=$state<'keep'|'store'|'remove'>('keep'), useDefault=$state(true);
  const weekdays=['Monday','Tuesday','Wednesday','Thursday','Friday','Saturday','Sunday'];
  function manual(){password='';file=undefined;note='';mode='manual';}
  function schedule(row?:BackupSchedule_Serialize){
    enabled=!!row?.schedule;frequency=row?.schedule?.frequency??'daily';time=row?.schedule?.localTime??'19:00';weekday=row?.schedule?.weekday??1;retainDays=row?.schedule?.retainDays??undefined;
    directory=undefined;useDefault=!row?.destinationName;willRemember='keep';scheduleSecret='';mode='schedule';
  }
  function pick(purpose:'backupWrite'|'backupDirectory'){
    void (purpose==='backupWrite'?manualTask:scheduleTask).run(async()=>{
      const [result]=await session.call(commands.fileChoose({purpose}));
      if(!result)return;
      if(purpose==='backupWrite')file=result;else{directory=result;useDefault=false;}
    });
  }
  function create(event:SubmitEvent){
    event.preventDefault();void manualTask.run(async()=>{
      const ticket=await session.call(commands.backupCreateStart(session.scope,{requestId:crypto.randomUUID(),destination:file?.id??null,backupPassphrase:password}));
      password='';file=undefined;
      const result=await jobs.result(ticket,'backupCreate');
      mode=null;note='Backup created · '+result.fileName;cursor=null;backups.refresh();
    });
  }
  function setSchedule(event:SubmitEvent){
    event.preventDefault();void scheduleTask.run(async()=>{
      if(enabled&&!useDefault&&!directory)throw new Error('Choose the custom directory again to authorize this change.');
      const input={
        schedule:enabled?{frequency,localTime:time,weekday:frequency==='weekly'?weekday:null,retainDays:retainDays??null}:null,
        destinationGrant:enabled&&!useDefault?directory?.id??null:null,
        rememberSecret:willRemember==='keep'?null:willRemember==='store',
        backupPassphrase:willRemember==='store'?scheduleSecret:null,
      };
      await session.call(commands.backupScheduleSet(session.mutation(),input));
      scheduleSecret='';mode=null;schedules.refresh();backups.refresh();
    });
  }
  function close(){mode=null;password='';scheduleSecret='';selected=undefined;file=undefined;directory=undefined;}
</script>
<Heading title="Backups" subtitle="Protect your activity and attachments with encrypted local archives.">
  <Button variant="outline"  href="/settings">Settings</Button><Button variant="outline" onclick={()=>{selected=undefined;mode='restore';}}>Restore from file</Button><Button variant="default"  onclick={manual}>Create backup</Button>
</Heading>
{#if note}<Card as="p" class="mb-4 text-status" role="status">{note}</Card>{/if}
<Card as="section" class="mb-5">
  <div class="flex flex-wrap items-center justify-between gap-3"><h2 class="section-title">Scheduled backups</h2><Button variant="outline" disabled={schedules.loading} onclick={()=>schedule(schedules.value)}>Edit schedule</Button></div>
  <Feedback error={schedules.error} loading={schedules.loading}/>
  {#if schedules.value}
    {@const schedule=schedules.value.schedule}
    <dl class="mt-3 grid grid-cols-2 gap-3 text-sm md:grid-cols-4">
      <div><dt class="text-muted-foreground">Frequency</dt><dd>{schedule? (schedule.frequency==='weekly'?weekdays[(schedule.weekday??1)-1]+' at ': 'Daily at ')+schedule.localTime : 'Off'}</dd></div>
      <div><dt class="text-muted-foreground">Destination</dt><dd>{schedules.value.destinationName??'App storage'}</dd></div>
      <div><dt class="text-muted-foreground">Last successful backup</dt><dd>{privacy.date(schedules.value.lastSuccess)}</dd></div>
      <div><dt class="text-muted-foreground">Archive credential</dt><dd>{schedules.value.remembered?'Stored in system keychain':'No credential stored'}</dd></div>
    </dl>
    {#if schedules.value.error}<p class="error mt-3" role="alert">{schedules.value.error.replaceAll('_',' ')}</p>{/if}
  {/if}
</Card>
<Card as="section" >
  <div class="flex items-center justify-between gap-2"><h2 class="section-title">Managed archives</h2><Button variant="outline" onclick={()=>{backups.refresh();schedules.refresh();}}>Refresh</Button></div>
  <Feedback error={backups.error} loading={backups.loading}/>
  <div class="table-scroll mt-3"><Table contained={false}><TableHeader><TableRow><TableHead>Archive</TableHead><TableHead>Created</TableHead><TableHead class="w-px whitespace-nowrap text-right">Actions</TableHead></TableRow></TableHeader><TableBody>
    {#each backups.value?.rows??[] as row (row.id)}
      <TableRow><TableHead class="whitespace-normal break-all" scope="row">{row.fileName}</TableHead><TableCell>{privacy.date(row.createdAt)}</TableCell><TableCell class="text-right"><RowActions itemName={row.fileName} onDelete={()=>{selected=row;mode='remove';}}
        extraActions={[{id:'restore',label:'Restore',onclick:()=>{selected=row;mode='restore';}}]}/></TableCell></TableRow>
    {:else}<TableRow><TableCell colspan={3}>No managed backups found.</TableCell></TableRow>{/each}
  </TableBody></Table></div>
  {#if cursor}<Button variant="outline" class="mt-3" onclick={()=>cursor=null}>First backups</Button>{/if}{#if backups.value?.nextCursor}<Button variant="outline" class="mt-3" onclick={()=>cursor=backups.value?.nextCursor??null}>Next backups</Button>{/if}
</Card>
{#if mode==='restore'}<RestoreFlow backup={selected} {close}/>
{:else if mode==='manual'}<Modal title="Create encrypted backup" {close}><form id={modalForm + '-backup'} class="space-y-3" autocomplete="off" onsubmit={create}>
  <Label class="field">Archive passphrase<Input type="password" autocomplete="off" bind:value={password} required/></Label>
  <p class="text-sm text-muted-foreground">You’ll need this passphrase to verify and restore the archive.</p>
  <div class="flex flex-wrap items-center gap-2"><span class="break-all text-sm text-muted-foreground">{file?.displayName??'App storage'}</span><Button variant="outline" type="button" disabled={manualTask.busy} onclick={()=>pick('backupWrite')}>Choose file…</Button>{#if file}<Button variant="outline" type="button" onclick={()=>file=undefined}>Use app storage</Button>{/if}</div>
  <Feedback error={manualTask.error}/>
</form>
  {#snippet footer()}
    <Button variant="default" type="submit" form={modalForm + '-backup'}  disabled={manualTask.busy}>Create encrypted backup</Button>
  {/snippet}
</Modal>
{:else if mode==='schedule'}<Modal title="Backup schedule" {close}><form id={modalForm + '-backup-schedule'} class="space-y-3" autocomplete="off" onsubmit={setSchedule}>
  <Label class="check"><NativeCheckbox  bind:checked={enabled}/> Enable scheduled backups</Label>
  {#if enabled}
    <div class="form-grid">
      <Label class="field">Frequency<NativeSelect bind:value={frequency}><option value="daily">Daily</option><option value="weekly">Weekly</option></NativeSelect></Label>
      <Label class="field">Local time<Input type="time" bind:value={time} required/></Label>
      {#if frequency==='weekly'}<Label class="field">Day of week<NativeSelect bind:value={weekday}>{#each weekdays as day,i (day)}<option value={i+1}>{day}</option>{/each}</NativeSelect></Label>{/if}
      <Label class="field">Remove archives older than (days)<Input type="number" min={1} max={3660} bind:value={retainDays} placeholder="Keep up to ten generations"/></Label>
    </div>
    <div class="rounded border border-line p-3"><Label class="check"><NativeCheckbox  bind:checked={useDefault} onchange={()=>{if(useDefault)directory=undefined;}}/> Use app storage</Label>
      {#if !useDefault}<div class="mt-3 flex flex-wrap items-center gap-2"><Button variant="outline" type="button" disabled={scheduleTask.busy} onclick={()=>pick('backupDirectory')}>Choose destination directory…</Button><span class="text-sm text-muted-foreground">{directory?.displayName??schedules.value?.destinationName??'Choose a directory'}</span></div>{/if}
    </div>
  {/if}
  <Label class="field">Backup passphrase in secure vault<NativeSelect bind:value={willRemember}><option value="keep">{schedules.value?.remembered?'Keep stored passphrase':'Leave unconfigured'}</option><option value="store">Store or replace passphrase</option>{#if schedules.value?.remembered}<option value="remove">Remove stored passphrase</option>{/if}</NativeSelect></Label>
  {#if willRemember==='store'}<Label class="field">Archive passphrase<Input type="password" autocomplete="off" bind:value={scheduleSecret} required/></Label>{/if}
  {#if enabled && (willRemember==='remove' || willRemember==='keep' && !schedules.value?.remembered)}<p class="text-sm text-muted-foreground">Automatic backups need an archive passphrase in the system keychain.</p>{/if}
  <Feedback error={scheduleTask.error}/>
</form>
  {#snippet footer()}
    <Button variant="default" type="submit" form={modalForm + '-backup-schedule'}  disabled={scheduleTask.busy}>Save backup schedule</Button>
  {/snippet}
</Modal>
{:else if mode==='remove' && selected}<Modal confirm title="Delete backup" {close}><p>Permanently delete {selected.fileName}?</p><Feedback error={removeTask.error}/>

  {#snippet footer()}
    <Button variant="destructive" disabled={removeTask.busy} onclick={()=>void removeTask.run(async()=>{if(!selected)return;await session.call(commands.backupRemove(session.mutation(),{id:selected.id,confirm:true}));close();backups.refresh();})}>Delete archive</Button>
  {/snippet}
</Modal>
{/if}
