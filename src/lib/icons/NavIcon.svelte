<script lang="ts">
  import { actionIcons, navIcons } from './registry';

  let { name, active = false, class: classes = 'size-[18px]' }: {
    name: keyof typeof navIcons | keyof typeof actionIcons;
    active?: boolean;
    class?: string;
  } = $props();
  const Icon = $derived(name in navIcons
    ? navIcons[name as keyof typeof navIcons][active ? 1 : 0]
    : actionIcons[name as keyof typeof actionIcons]);
</script>

<span class="inline-flex shrink-0" aria-hidden="true" data-icon-weight={active && name in navIcons ? 'fill' : 'regular'}>
  <Icon class={classes}/>
</span>
