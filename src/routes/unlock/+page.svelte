<script lang="ts">
  import { Card } from '#lib/components/ui/card/index.js';
  import { Label } from '#lib/components/ui/label/index.js';
  import { Input } from '#lib/components/ui/input/index.js';
  import { Button } from '#lib/components/ui/button/index.js';
  import { useLauncher } from '../../lib/launcher.svelte';

  const launcher = useLauncher();
  const challenge = $derived(launcher.challenge);

  function unlock(event: SubmitEvent) {
    event.preventDefault();
    if (launcher.busy) return;
    const form = event.currentTarget as HTMLFormElement;
    const field = form.elements.namedItem('credential') as HTMLInputElement;
    const credential = field.value;
    field.value = '';
    void launcher.run(() => launcher.unlock(credential));
  }
</script>

<Card as="section" class="mx-auto max-w-md" aria-labelledby="unlock-title">
  <h1 id="unlock-title" class="text-2xl font-semibold tracking-tight text-ink">Unlock activity</h1>
  {#if challenge}
    <p class="mb-5 mt-2 break-words text-muted-foreground">{challenge.ledger.label}</p>
    <form onsubmit={unlock} autocomplete="off">
      <fieldset class="grid min-w-0 gap-4" disabled={launcher.busy}>
        <Label class="field">
          <span>Passphrase or PIN</span>
          <Input type="password" name="credential" autocomplete="off" maxlength={1024} required />
        </Label>
        <Button variant="default"  type="submit">Unlock</Button>
        {#if challenge.ledger.quickUnlock}
          <Button variant="outline" type="button" onclick={() => launcher.run(() => launcher.unlock())}>Use secure vault</Button>
        {/if}
      </fieldset>
    </form>
  {:else}
    <p class="mt-3 text-sm text-muted-foreground">Choose protected activity from Your activity.</p>
  {/if}
  <a class="mobile-touch-link mt-5 inline-block text-sm font-medium text-muted-foreground underline underline-offset-4" href="/"
    aria-disabled={launcher.busy}
    onclick={(event) => {
      if (launcher.busy) {
        event.preventDefault();
        return;
      }
      launcher.error = '';
      launcher.challenge = null;
    }}>Back to your activity</a>
</Card>
