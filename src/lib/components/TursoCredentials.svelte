<script lang="ts">
  import { Label } from '#lib/components/ui/label/index.js';
  import { Input } from '#lib/components/ui/input/index.js';
  let { url = $bindable(''), token = $bindable(''), lockedUrl = false, onEdit }:
    { url: string; token: string; lockedUrl?: boolean; onEdit: () => void } = $props();
</script>

{#if !lockedUrl}
  <Label class="field">
    Turso Database URL
    <Input type="text" inputmode="url" placeholder="libsql://your-database.turso.io" autocomplete="off"
      spellcheck="false" maxlength={260} required bind:value={url} oninput={onEdit} />
  </Label>
{/if}
<Label class="field">
  Turso Auth Token
  <Input type="password" autocomplete="new-password" maxlength={8192} required bind:value={token} oninput={onEdit} />
</Label>
