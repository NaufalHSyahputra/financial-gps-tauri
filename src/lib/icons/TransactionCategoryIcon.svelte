<script lang="ts">
  import type { TxRow_Serialize } from '../drivers/runtime';
  import CategoryIcon from './CategoryIcon.svelte';

  let { row, size = 'sm' }: { row: TxRow_Serialize; size?: 'sm' | 'md' } = $props();
</script>

{#if row.transferId}
  <CategoryIcon kind="transfer" {size}/>
{:else}
  <CategoryIcon name={row.category?.name} kind={row.category?.kind ?? (row.direction === 'credit' ? 'income' : 'expense')}
    icon={row.category?.icon} {size}/>
{/if}
