<script lang="ts">
  import type { AccountKind } from '../drivers/runtime';
  import { accountIcons } from './registry';

  let { kind, wallet = false }: { kind: AccountKind; wallet?: boolean } = $props();
  const Icon = $derived(accountIcons[wallet ? 'wallet' : kind]);
</script>

<span class="inline-grid size-8 shrink-0 place-items-center rounded-lg border border-border bg-muted text-muted-foreground"
  aria-hidden="true" data-account-icon={wallet ? 'wallet' : kind}>
  <Icon class="size-[18px]"/>
</span>
