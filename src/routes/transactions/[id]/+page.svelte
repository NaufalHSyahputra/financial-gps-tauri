<script lang="ts">
  import { page } from '$app/state';
  import NewTransaction from '../../../lib/views/NewTransaction.svelte';
  import TransactionEditor from '../../../lib/views/TransactionEditor.svelte';
</script>

{#key page.params.id}
  {#if page.params.id === 'new'}<NewTransaction />
  {:else if page.params.id}<TransactionEditor id={page.params.id} />{/if}
{/key}
