<script lang="ts">
  import { Label } from '#lib/components/ui/label/index.js';
  import { Input } from '#lib/components/ui/input/index.js';
  import { NativeSelect } from '#lib/components/ui/native-select/index.js';
  import { Button } from '#lib/components/ui/button/index.js';
  import { commands, type GroupKind, type GroupSummary } from '../drivers/runtime';
  import { session } from '../stores/session.svelte';
  import { createTask } from '../stores/task.svelte';
  import Feedback from '../components/Feedback.svelte';
  import Modal from '../components/Modal.svelte';
  let { group, self='Me', close, saved }: {group?:GroupSummary;self?:string;close:()=>void;saved:(id:string)=>void}=$props();
  let name=$state(''), label=$state('Me'), kind=$state<GroupKind>('trip');
  $effect(()=>{name=group?.name??'';kind=group?.kind??'trip';label=self;});
  const task=createTask();
  function submit(event:SubmitEvent){
    event.preventDefault();void task.run(async()=>{
      const result=await session.call(commands.groupSave(session.mutation(),{target:group?{id:group.id,version:group.version}:null,draft:{name,kind,selfLabel:label}}));
      saved(result.value.group.id);
    });
  }
</script>
<Modal title={group?'Edit group':'New shared group'} {close}>
  <form class="space-y-3" onsubmit={submit}>
    <Label class="field">Name<Input bind:value={name} required/></Label>
    <Label class="field">Type<NativeSelect bind:value={kind}><option value="social">Social</option><option value="trip">Trip</option><option value="costCenter">Cost center</option><option value="project">Project</option><option value="client">Client</option><option value="other">Other</option></NativeSelect></Label>
    <Label class="field">Your name in this group<Input bind:value={label} required/></Label>
    <Feedback error={task.error}/><Button variant="default" type="submit"  disabled={task.busy}>Save group</Button>
  </form>
</Modal>
