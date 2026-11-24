<script lang="ts">
	import { cn, type WithElementRef } from "#lib/utils.js";
	import type { HTMLTableAttributes } from "svelte/elements";

	let {
		ref = $bindable(null),
		class: className,
		children,
		contained = true,
		...restProps
	}: WithElementRef<HTMLTableAttributes, HTMLTableElement> & { contained?: boolean } = $props();
</script>

{#snippet markup()}
	<table bind:this={ref} data-slot="table" class={cn("w-full caption-bottom text-sm", className)} {...restProps}>
		{@render children?.()}
	</table>
{/snippet}
{#if contained}
	<div data-slot="table-container" class="relative w-full overflow-x-auto">{@render markup()}</div>
{:else}
	{@render markup()}
{/if}
