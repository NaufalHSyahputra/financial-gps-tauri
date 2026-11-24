<script lang="ts">
  let { error='', loading=false }: {error?:string;loading?:boolean}=$props();
</script>
{#if error}<p role="alert" class="error my-3">{error}</p>
{:else if loading}<p role="status" class="my-3 text-sm text-muted-foreground">Loading…</p>{/if}
