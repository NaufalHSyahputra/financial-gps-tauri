<script lang="ts">
	import { cn, type WithElementRef } from "#lib/utils.js";
	import type { HTMLAttributes } from "svelte/elements";

	let {
		ref = $bindable(null),
		class: className,
		children,
		size = "default",
		as = "div",
		href,
		...restProps
	}: WithElementRef<HTMLAttributes<HTMLElement>, HTMLElement> & { size?: "default" | "sm"; as?: "div" | "article" | "section" | "p" | "form"; href?: string } = $props();
</script>

<svelte:element this={href ? "a" : as} {href}
	bind:this={ref}
	data-slot="card"
	data-size={size}
	class={cn("min-w-0 rounded-xl border border-border bg-card p-4 text-card-foreground md:p-5 data-[size=sm]:p-3", className)}
	{...restProps}
>
	{@render children?.()}
</svelte:element>
