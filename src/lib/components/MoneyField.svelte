<script lang="ts">
  import { Label } from '#lib/components/ui/label/index.js';
  import { Input } from '#lib/components/ui/input/index.js';
  import { privacy } from '../stores/privacy.svelte';
  let { label = 'Amount', value = $bindable(''), required = true, disabled = false }: {label?:string;value?:string;required?:boolean;disabled?:boolean}=$props();
</script>
<Label class="field">{label}
  <Input type={privacy.masked ? 'password' : 'text'} inputmode="decimal" bind:value {required} {disabled} autocomplete="off" />
</Label>
