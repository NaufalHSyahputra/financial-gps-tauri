<script lang="ts">
  import { Button } from '#lib/components/ui/button/index.js';
  import { page } from '$app/state';
  import { goto } from '$app/navigation';
  import { commands, type AccountDraft_Deserialize, type Money } from '../../../lib/drivers/runtime';
  import { createTask } from '../../../lib/stores/task.svelte';
  import { session } from '../../../lib/stores/session.svelte';
  import AccountEditor from '../../../lib/views/AccountEditor.svelte';
  import AccountSetup from '../../../lib/views/AccountSetup.svelte';

  const task = createTask();
  function create(draft: AccountDraft_Deserialize, startingBalance: Money) {
    void task.run(async () => {
      const account = await session.call(commands.accountSave(session.mutation(), { draft, startingBalance }));
      await goto('/accounts/' + account.value.id);
    });
  }
</script>

{#key page.params.id}
  {#if page.params.id === 'new'}
    <div class="mb-4"><Button variant="outline"  href="/accounts">All accounts</Button></div>
    <AccountSetup base={session.active?.baseCurrency ?? 'IDR'} busy={task.busy} error={task.error} complete={create} />
  {:else if page.params.id}
    <AccountEditor id={page.params.id} />
  {/if}
{/key}
