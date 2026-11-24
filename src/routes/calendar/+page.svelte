<script lang="ts">
  import { Label } from '#lib/components/ui/label/index.js';
  import { Input } from '#lib/components/ui/input/index.js';
  import { NativeSelect } from '#lib/components/ui/native-select/index.js';
  import { NativeCheckbox } from '#lib/components/ui/checkbox/index.js';
  import { Badge } from '#lib/components/ui/badge/index.js';
  import { commands, type DateBasis } from '../../lib/drivers/runtime';
  import { session, today } from '../../lib/stores/session.svelte';
  import { privacy } from '../../lib/stores/privacy.svelte';
  import { query } from '../../lib/stores/task.svelte';
  import Heading from '../../lib/components/Heading.svelte';
  import Feedback from '../../lib/components/Feedback.svelte';
  let month=$state(today().slice(0,7));
  let basis=$state<DateBasis>('bank');
  let projection=$state(true);
  let accountId=$state('');
  const accounts=query(()=>session.call(commands.accountsList(session.scope,{})));
  const data=query(()=>{
    const match=/^(\d{4})-(\d{2})$/.exec(month);
    if(!match) return Promise.reject(new Error('Choose a month.'));
    const last=new Date(Date.UTC(+match[1],+match[2],0)).getUTCDate();
    return session.call(commands.calendarGet(session.scope,{basis,includeProjection:projection,accountIds:accountId?[accountId]:[],
      range:{from:month+'-01',to:month+'-'+last}}));
  });
</script>
<Heading title="Calendar">
  <Label class="field"><span class="sr-only">Month</span><Input type="month" bind:value={month}/></Label>
  <Label class="field"><span class="sr-only">Date basis</span><NativeSelect bind:value={basis}><option value="bank">Bank date</option><option value="reporting">Reporting date</option></NativeSelect></Label>
  <Label class="field"><span class="sr-only">Account</span><NativeSelect bind:value={accountId}><option value="">All accounts</option>{#each accounts.value??[] as item (item.id)}<option value={item.id}>{item.account.name}</option>{/each}</NativeSelect></Label>
  <Label class="check"><NativeCheckbox  bind:checked={projection}/> Include expected transactions</Label>
</Heading>
<Feedback error={data.error} loading={data.loading}/>
{#if data.value}
  <div class="overflow-x-auto">
    <div class="grid min-w-[40rem] grid-cols-7 gap-px overflow-hidden rounded-lg border border-line bg-line">
      {#each ['Sun','Mon','Tue','Wed','Thu','Fri','Sat'] as weekday (weekday)}<div class="bg-ground px-2 py-2 text-xs font-semibold text-muted-foreground">{weekday}</div>{/each}
      {#each data.value.days as day (day.date)}
        <section aria-label={privacy.date(day.date)} class:bg-paper={day.inRange} class:bg-ground={!day.inRange} class="min-h-25 min-w-0 p-2">
          <h2 class="mb-1 text-xs text-muted-foreground">{day.date.slice(-2)}</h2>
          {#each day.events as event (event.transaction.id)}
            <div class="mb-1 rounded bg-ground px-1.5 py-1 text-xs">
              {#if event.occurrence}<a class="block truncate underline max-md:min-h-11 max-md:min-w-11 max-md:leading-11" href="/recurring">{event.transaction.description}</a>
              {:else}<a class="block truncate underline max-md:min-h-11 max-md:min-w-11 max-md:leading-11" href={'/transactions/'+event.transaction.id}>{event.transaction.description}</a>{/if}
              <span class="numeric">{privacy.money(event.transaction.amount)}</span>
              {#if event.projected}<Badge variant="secondary" >Expected</Badge>{/if}
            </div>
          {/each}
          {#each day.bookedMovements as amount (amount.currency)}<p class="text-xs">Recorded <span class="numeric">{privacy.money(amount)}</span></p>{/each}
          {#each day.forecastMovements as amount (amount.currency)}<p class="text-xs text-muted-foreground">Forecast <span class="numeric">{privacy.money(amount)}</span></p>{/each}
        </section>
      {/each}
    </div>
  </div>
{/if}
