<script lang="ts">
  import RowActions from '#lib/components/RowActions.svelte';
  import { Label } from '#lib/components/ui/label/index.js';
  import { NativeSelect } from '#lib/components/ui/native-select/index.js';
  import { Input } from '#lib/components/ui/input/index.js';
  import { NativeCheckbox } from '#lib/components/ui/checkbox/index.js';
  import { session } from '../stores/session.svelte';
  import type { Predicate, Field, Operator } from '../drivers/runtime';
  import { privacy } from '../stores/privacy.svelte';
  let { predicate=$bindable(), remove }: {predicate:Predicate;remove:()=>void}=$props();
  const fields:[Field,string][]=[['description','Description'],['payee','Payee name'],['notes','Notes'],['signedAmount','Signed amount'],['type','Direction'],['accountId','Account ID'],['payeeId','Payee ID'],['bankDate','Bank date'],['status','Status'],['providerCategory','Provider category'],['ignored','Ignored']];
  const equality:[Operator,string][]=[['equal','Equals'],['notEqual','Does not equal']];
  const comparisons:[Operator,string][]=[['greater','Greater than'],['greaterEqual','At least'],['less','Less than'],['lessEqual','At most']];
  const text:[Operator,string][]=[['contains','Contains'],['notContains','Does not contain'],['prefix','Starts with'],['suffix','Ends with'],['regex','Regular expression']];
  const operators=$derived(predicate.field==='signedAmount'||predicate.field==='bankDate'?[...equality,...comparisons]:
    ['description','payee','notes','providerCategory'].includes(predicate.field)?[...equality,...text]:equality);
  function change(field:Field){
    predicate={field,operator:'equal',value:field==='signedAmount'?{type:'amount',value:{currency:session.active?.baseCurrency ?? 'IDR',decimal:'0'}}:
      field==='ignored'?{type:'boolean',value:false}:{type:'text',value:field==='status'?'posted':field==='type'?'debit':''}};
  }
</script>
<div class="grid items-end gap-2 rounded-lg border border-line p-3 sm:grid-cols-[1fr_1fr_1fr_auto]">
  <Label class="field">Field<NativeSelect value={predicate.field} onchange={event=>change(event.currentTarget.value as Field)}>{#each fields as [field,label] (field)}<option value={field}>{label}</option>{/each}</NativeSelect></Label>
  <Label class="field">Operator<NativeSelect bind:value={predicate.operator}>{#each operators as [operator,label] (operator)}<option value={operator}>{label}</option>{/each}</NativeSelect></Label>
  {#if predicate.value.type==='amount'}
    <div class="grid grid-cols-2 gap-1">
      <Label class="field">Currency<NativeSelect bind:value={predicate.value.value.currency}>{#each privacy.currencies as currency (currency.code)}<option value={currency.code}>{currency.code}</option>{/each}</NativeSelect></Label>
      <Label class="field">Value<Input type={privacy.masked?'password':'text'} inputmode="decimal" bind:value={predicate.value.value.decimal}/></Label>
    </div>
  {:else if predicate.value.type==='boolean'}
    <Label class="check py-2"><NativeCheckbox  bind:checked={predicate.value.value}/> True</Label>
  {:else if predicate.field==='status'}
    <Label class="field">Value<NativeSelect bind:value={predicate.value.value}><option value="posted">Confirmed</option><option value="pending">Pending</option><option value="projected">Expected</option></NativeSelect></Label>
  {:else if predicate.field==='type'}
    <Label class="field">Value<NativeSelect bind:value={predicate.value.value}><option value="debit">Debit</option><option value="credit">Credit</option></NativeSelect></Label>
  {:else}
    <Label class="field">Value<Input required type={predicate.field==='bankDate'?'date':'text'} bind:value={predicate.value.value}/></Label>
  {/if}
  <RowActions deleteLabel="Remove condition" onDelete={remove}/>
</div>
