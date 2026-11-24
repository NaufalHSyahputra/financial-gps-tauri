<script lang="ts">
  import { resolveCategoryIcon } from './category-icons';

  let { name = '', kind = 'expense', icon, size = 'md' }: {
    name?: string;
    kind?: string;
    icon?: string | null;
    size?: 'sm' | 'md' | 'lg';
  } = $props();
  const resolved = $derived(resolveCategoryIcon(name, kind, icon));
  const sizes = { sm: 'size-8', md: 'size-9', lg: 'size-10' };
  const tint = $derived(kind === 'income' ? 'bg-money-in-soft text-money-in'
    : kind === 'transfer' ? 'bg-transfer/8 text-transfer' : 'bg-muted text-muted-foreground');
</script>

<span class={['inline-grid shrink-0 place-items-center rounded-xl border border-border', sizes[size], tint]}
  aria-hidden="true" data-category-icon={resolved.id}>
  <resolved.Icon class={size === 'lg' ? 'size-5' : 'size-[18px]'}/>
</span>
