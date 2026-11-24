<script lang="ts">
  const modalForm = $props.id();
  import { Button } from '#lib/components/ui/button/index.js';
  import { Card } from '#lib/components/ui/card/index.js';
  import { Label } from '#lib/components/ui/label/index.js';
  import { Input } from '#lib/components/ui/input/index.js';
  import { NativeSelect } from '#lib/components/ui/native-select/index.js';
  import { NativeCheckbox } from '#lib/components/ui/checkbox/index.js';
  import { commands, type AccountingMode, type DateFormat, type NumberFormat, type Protection } from '../../lib/drivers/runtime';
  import { session } from '../../lib/stores/session.svelte';
  import { privacy } from '../../lib/stores/privacy.svelte';
  import { jobs } from '../../lib/stores/jobs.svelte';
  import { query, createTask } from '../../lib/stores/task.svelte';
  import { useLauncher } from '../../lib/launcher.svelte';
  import Heading from '../../lib/components/Heading.svelte';
  import Feedback from '../../lib/components/Feedback.svelte';
  import Modal from '../../lib/components/Modal.svelte';
  import FinancialCalendarSettings from '#lib/views/FinancialCalendarSettings.svelte';
  import StorageSync from '../../lib/views/StorageSync.svelte';
  import ThemeSelector from '../../lib/components/ThemeSelector.svelte';
  import CurrencySelect from '../../lib/components/CurrencySelect.svelte';
  const launcher=useLauncher();
  const settings=query(()=>session.call(commands.ledgerSettingsGet(session.scope)));
  const ledgerTask=createTask(), appTask=createTask(), securityTask=createTask();
  const quick=$derived(launcher.ledgers.find(row=>row.ledgerInstanceId===session.active?.scope.ledgerInstanceId)?.quickUnlock??false);
  let name=$state(''),currency=$state('IDR'),zone=$state('');
  let mode=$state<AccountingMode>('cash');
  let dateFormat=$state<DateFormat>('iso');
  let numbers=$state<NumberFormat>('international');
  let enabled=$state<string[]>([]);
  let section=$state<'protection'|'keychain'|null>(null);
  let protection=$state<Protection>('encrypted');
  let oldCredential=$state(''), nextCredential=$state(''), repeated=$state(''), safety=$state('');
  let savedLedger=$state(false), savedApp=$state(false);
  const modules=[{id:'budgets',label:'Budgets'},{id:'goals',label:'Goals'},{id:'assets',label:'Assets'},{id:'recurring',label:'Recurring'},{id:'groups',key:'shared-groups',label:'Shared groups'}];
  const featureIds=['budgets','goals','assets','recurring','groups','shared','sharedgroups'];
  $effect(()=>{
    const result=settings.value;
    if(result){name=result.name;currency=result.displayCurrency;zone=result.timezoneOverride??'';mode=result.accountingMode;}
  });
  $effect(()=>{
    const prefs=privacy.prefs;
    if(prefs){dateFormat=prefs.dateFormat;numbers=prefs.numberFormat;enabled=modules.filter(row=>privacy.visible(row.id)).map(row=>row.id);}
  });
  function ledgerSave(event:SubmitEvent){
    event.preventDefault();savedLedger=false;void ledgerTask.run(async()=>{
      const result=await session.call(commands.ledgerSettingsSet(session.mutation(),{patch:{name:{change:'set',value:name},displayCurrency:{change:'set',value:currency},timezoneOverride:zone.trim()?{change:'set',value:zone.trim()}:{change:'clear'},accountingMode:{change:'set',value:mode}}}));
      if(session.active)session.set({...session.active,name:result.value.name,baseCurrency:result.value.displayCurrency,revision:result.revision});
      session.timezone=result.value.timezoneOverride;
      settings.refresh();savedLedger=true;
    });
  }
  function appSave(event:SubmitEvent){
    event.preventDefault();savedApp=false;void appTask.run(async()=>{
      const other=(privacy.prefs?.hiddenFeatures??[]).filter(flag=>!featureIds.includes(flag.toLowerCase().replace(/[-_]/g,'')));
      const hiddenFeatures=[...other,...modules.filter(row=>!enabled.includes(row.id)).map(row=>row.key??row.id)];
      privacy.apply(await session.call(commands.appPreferencesSet({dateFormat,numberFormat:numbers,hiddenFeatures,displayCurrency:{change:'clear'}}),false));
      savedApp=true;
    });
  }
  function open(value:'protection'|'keychain'){
    oldCredential='';nextCredential='';repeated='';safety='';protection='encrypted';section=value;
  }
  function securitySave(event:SubmitEvent){
    event.preventDefault();void securityTask.run(async()=>{
      if(section==='keychain'){
        await session.call(commands.ledgerKeychainSet(session.mutation(),{enabled:!quick,credential:quick?null:oldCredential}));
        section=null;oldCredential='';await launcher.refreshLedgers();return;
      }
      if(protection==='encrypted' && nextCredential!==repeated)throw new Error('The new credentials must match.');
      const ticket=await session.call(commands.ledgerChangeProtection(session.mutation(),{protection,oldCredential:oldCredential||null,newCredential:protection==='encrypted'?nextCredential:null,backupPassphrase:safety||null}));
      oldCredential='';nextCredential='';repeated='';safety='';
      const updated=await jobs.result(ticket,'ledgerChangeProtection',null);
      section=null;session.set(updated);await launcher.refreshLedgers();
    });
  }
</script>
<Heading title="Settings"><Button variant="outline"  href="/settings/backups">Backups</Button><Button variant="outline"  href="/">Your activity</Button></Heading>
<div class="grid gap-4 lg:grid-cols-2">
  <Card as="section" class="lg:col-span-2" aria-labelledby="appearance-heading">
    <h2 id="appearance-heading" class="section-title mb-3">Appearance</h2>
    <ThemeSelector/>
  </Card>
  <FinancialCalendarSettings />
  <StorageSync onLedgerChanged={async () => { await launcher.refresh(); settings.refresh(); }} />
  <Card as="section" >
    <h2 class="section-title">Activity settings</h2>
    <Feedback error={settings.error || ledgerTask.error} loading={settings.loading}/>
    {#if settings.value}<form class="mt-3 space-y-4" onsubmit={ledgerSave}>
      <Label class="field">Activity name<Input bind:value={name} maxlength={256} required/></Label>
      <CurrencySelect label="Primary currency" bind:value={currency}/>
      <Label class="field">Time zone<Input bind:value={zone} placeholder="System timezone" aria-describedby="timezone-hint"/><span id="timezone-hint" class="text-xs text-muted-foreground">Enter an IANA timezone such as Asia/Jakarta. Leave blank to follow the operating system.</span></Label>
      <Label class="field">When to count card spending<NativeSelect bind:value={mode}><option value="cash">When it is billed</option><option value="accrual">When the purchase happens</option></NativeSelect></Label>
      <p class="text-sm text-muted-foreground">Set this separately for each card in <a class="underline" href="/accounts">Accounts</a>.</p>
      <Button variant="default" type="submit"  disabled={ledgerTask.busy}>Save activity settings</Button>
      {#if savedLedger}<p role="status" class="text-sm text-status">Activity settings saved.</p>{/if}
    </form>{/if}
  </Card>
  <Card as="section" >
    <h2 class="section-title">App preferences</h2>
    <form class="mt-3 space-y-4" onsubmit={appSave}>
      <div class="form-grid">
        <Label class="field">Date format<NativeSelect bind:value={dateFormat}><option value="iso">ISO</option><option value="dayFirst">Day first</option><option value="monthFirst">Month first</option></NativeSelect></Label>
        <Label class="field">Number format<NativeSelect bind:value={numbers}><option value="international">International</option><option value="indonesian">Indonesian</option></NativeSelect></Label>
      </div>
      <fieldset><legend class="section-title mb-3">Optional modules</legend>
        <div class="grid gap-2 sm:grid-cols-2">{#each modules as row (row.id)}
          <Label class="check"><NativeCheckbox  checked={enabled.includes(row.id)} onchange={event=>enabled=event.currentTarget.checked?[...enabled,row.id]:enabled.filter(key=>key!==row.id)}/> {row.label}</Label>
        {/each}</div>
        <p class="mt-3 text-sm text-muted-foreground">You can re-enable a module to access its saved records.</p>
      </fieldset>
      <Feedback error={appTask.error}/>
      <Button variant="default" type="submit"  disabled={appTask.busy}>Save app preferences</Button>
      {#if savedApp}<p role="status" class="text-sm text-status">App preferences saved.</p>{/if}
    </form>
  </Card>
  <Card as="section" class="lg:col-span-2"><h2 class="section-title">Activity protection</h2>
    <p class="my-3 text-sm text-muted-foreground">{session.active?.protection==='encrypted'?'Activity is encrypted':'Activity is unencrypted'}{quick?' · Quick unlock enabled':''}</p>
    <div class="flex flex-wrap gap-2"><Button variant="outline" onclick={()=>open('protection')}>Change protection</Button>{#if session.active?.protection==='encrypted'}<Button variant="outline" onclick={()=>open('keychain')}>{quick?'Disable':'Enable'} quick unlock</Button>{/if}</div>
  </Card>
</div>
{#if section}<Modal title={section==='keychain'?'Secure vault quick unlock':'Change activity protection'} close={()=>{section=null;oldCredential='';nextCredential='';repeated='';safety='';}}>
  <form id={modalForm + '-security'} class="space-y-3" autocomplete="off" onsubmit={securitySave}>
    {#if section==='keychain'}
      {#if quick}<p>Require your credential when opening this activity.</p>
      {:else}<Label class="field">Current passphrase or PIN<Input type="password" autocomplete="off" bind:value={oldCredential} required/></Label>{/if}
    {:else}
      <Label class="field">Protection<NativeSelect bind:value={protection}><option value="encrypted">{session.active?.protection==='encrypted'?'Encrypted with new credential':'Encrypted'}</option>{#if session.active?.protection==='encrypted'}<option value="plain">Unencrypted</option>{/if}</NativeSelect></Label>
      {#if session.active?.protection==='encrypted'}<Label class="field">Current passphrase or PIN<Input type="password" autocomplete="off" bind:value={oldCredential} required/></Label>{/if}
      {#if protection==='encrypted'}<Label class="field">New passphrase or PIN<Input type="password" autocomplete="off" bind:value={nextCredential} required/></Label><Label class="field">Repeat new credential<Input type="password" autocomplete="off" bind:value={repeated} required/></Label>{/if}
      <Label class="field">Separate passphrase for safety backup (optional)<Input type="password" autocomplete="off" bind:value={safety}/></Label>
    {/if}
    <Feedback error={securityTask.error}/>
  </form>
  {#snippet footer()}
    <Button variant="default" type="submit" form={modalForm + '-security'}  disabled={securityTask.busy}>Confirm change</Button>
  {/snippet}
</Modal>{/if}
