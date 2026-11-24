<script lang="ts">
  import { Button } from '#lib/components/ui/button/index.js';
  import type { Money } from '../drivers/runtime';
  import { privacy } from '../stores/privacy.svelte';
  import type { ChartPoint } from '../reporting';
  let { series, label }: {series:ChartPoint[];label:string}=$props();
  const ceiling=$derived(series.reduce((max,point)=>point.values.reduce((prior,item)=>{
    const value=BigInt(item.money.minor);
    const magnitude=value<0n?-value:value;
    return magnitude>prior?magnitude:prior;
  },max),1n));
  const legend=$derived([...new Set(series.flatMap(point=>point.values.map(value=>value.label)))]);
  const seriesColors = ['bg-chart-1', 'bg-chart-2', 'bg-chart-3', 'bg-chart-4', 'bg-chart-5'];
  const signs=$derived({
    negative:series.some(point=>point.values.some(item=>BigInt(item.money.minor)<0n)),
    positive:series.some(point=>point.values.some(item=>BigInt(item.money.minor)>0n))
  });
  const zero=$derived(signs.negative?signs.positive?50n:100n:0n);
  const firstMark=$derived(series.find(point=>point.values.length));
  const chartId=$props.id();
  let active=$state<{date:string;label:string;money:Money}|null>(null);
  let focused=$state<string|null>(null);
  $effect(()=>{ if(privacy.masked){active=null;focused=null;} });
  function info(item:{date:string;label:string;money:Money}){
    return privacy.date(item.date)+' · '+item.label+' · '+privacy.money(item.money);
  }
  function navigate(event:KeyboardEvent){
    if(event.key!=='ArrowLeft' && event.key!=='ArrowRight')return;
    if(!(event.currentTarget instanceof HTMLElement))return;
    const bars=[...event.currentTarget.closest('[data-chart]')!.querySelectorAll<HTMLButtonElement>('button[data-mark]')];
    const next=bars[bars.indexOf(event.currentTarget as HTMLButtonElement)+(event.key==='ArrowRight'?1:-1)];
    if(next){event.preventDefault();next.focus();}
  }
</script>
<section aria-label={label} class="min-w-0">
  {#if privacy.masked}
    <p class="grid min-h-36 place-items-center rounded-lg bg-ground text-sm text-muted-foreground">Chart values are hidden</p>
  {:else if series.some(point=>point.values.length)}
    {#if legend.length>1}<ul class="mb-2 flex flex-wrap gap-3 text-sm">{#each legend as item,i (item)}<li><span aria-hidden="true" class={[seriesColors[i % seriesColors.length], "mr-1 inline-block size-2 rounded-sm"]}></span>{item}</li>{/each}</ul>{/if}
    <div data-chart class="flex h-40 items-end gap-1 overflow-x-auto rounded-lg border border-line bg-ground p-2">
      {#each series as point (point.date)}
        <div class="flex min-w-3 flex-1 items-end justify-center gap-0.5 self-stretch">
          {#each point.values as item (item.label)}
            {@const value=BigInt(item.money.minor)}
            {@const height=value<0n?-value*zero/ceiling:value*(100n-zero)/ceiling}
            {@const mark={...item,date:point.date}}
            {@const key=point.date+'-'+item.label}
            <div class="relative min-w-1 max-w-12 flex-1 self-stretch max-md:max-w-10">
              {#if signs.negative&&signs.positive}<span aria-hidden="true" class="absolute inset-x-0 border-t border-muted-foreground/40" style:bottom={zero+'%'}></span>{/if}
              <Button variant="outline" style={`height:${height}%;${value>=0n?`bottom:${zero}%`:`top:${100n-zero}%`}`} type="button" data-mark={key} tabindex={focused===key || focused===null && point===firstMark && item===point.values[0] ? 0 : -1}
                aria-label={info(mark)} title={info(mark)} aria-describedby={active?.date===point.date&&active.label===item.label?chartId:undefined}
                onfocus={()=>{focused=key;active=mark;}} onblur={()=>active=null} onpointerenter={()=>active=mark} onpointerleave={()=>{if(focused!==key || document.activeElement?.getAttribute('data-mark')!==key)active=null;}} onkeydown={navigate}
                onclick={()=>active=mark} 
                class={["chart-bar absolute inset-x-0", seriesColors[legend.indexOf(item.label) % seriesColors.length]]}   ></Button>
            </div>
          {/each}
        </div>
      {/each}
    </div>
    <div class="mt-2 flex justify-between text-xs text-muted-foreground"><span>{privacy.date(series[0].date)}</span><span>{privacy.date(series[series.length-1].date)}</span></div>
    <p id={chartId} class="numeric mt-2 min-h-5 text-sm">{active?info(active):''}</p>
  {:else}<p class="text-sm text-muted-foreground">No activity in this period.</p>{/if}
</section>
<style>
  :global(.chart-bar[data-mark]) {
    min-width: 0;
    min-height: 3px;
    padding: 0;
    border: none;
    border-radius: 3px 3px 0 0;
  }
</style>
