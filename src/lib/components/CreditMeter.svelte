<script lang="ts">
  import { Progress } from '#lib/components/ui/progress/index.js';
  import type { Credit_Serialize } from '../drivers/runtime';
  import { privacy } from '../stores/privacy.svelte';
  let { credit }: {credit:Credit_Serialize}=$props();
  const percent=$derived.by(()=>{
    if(!credit.limit || BigInt(credit.limit.minor)<=0n) return 0;
    const value=BigInt(credit.debt.minor)*100n/BigInt(credit.limit.minor);
    return Number(value<0n?0n:value>100n?100n:value);
  });
</script>
<dl class="mt-2 grid grid-cols-2 gap-y-1 text-sm">
  <dt>Debt</dt><dd class="numeric text-right">{privacy.money(credit.debt)}</dd>
  <dt>Available</dt><dd class="numeric text-right">{privacy.money(credit.available)}</dd>
  <dt>After future holds</dt><dd class="numeric text-right">{privacy.money(credit.availableAfterHolds)}</dd>
</dl>
{#if !privacy.masked && credit.limit}
  <Progress class="mt-3 h-2" value={percent} aria-label={'Debt '+privacy.money(credit.debt)+' of '+privacy.money(credit.limit)}/>
{/if}
