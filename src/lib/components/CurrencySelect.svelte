<script lang="ts">
  import { Label } from '#lib/components/ui/label/index.js';
  import { NativeSelect } from '#lib/components/ui/native-select/index.js';
  import { privacy } from '../stores/privacy.svelte';
  let { value=$bindable('USD'), label='Currency', disabled=false }: {value?:string;label?:string;disabled?:boolean}=$props();
</script>
<Label class="field">{label}<NativeSelect bind:value {disabled} required>
  {#each privacy.currencies as row (row.code)}<option value={row.code}>{row.code} · {row.name}</option>{/each}
</NativeSelect></Label>
