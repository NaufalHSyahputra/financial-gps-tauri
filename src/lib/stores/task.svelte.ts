import { message, session } from './session.svelte';

export function createTask() {
  let error = $state('');
  let busy = $state(false);
  return {
    get error() { return error; },
    get busy() { return busy; },
    async run(work: () => Promise<void>) {
      if (busy) return;
      const epoch = session.epoch;
      busy = true;
      error = '';
      try { await work(); }
      catch (cause) { if (epoch === session.epoch) error = message(cause); }
      finally { busy = false; }
    }
  };
}

export function query<T>(load: () => Promise<T>) {
  let value = $state.raw<T>();
  let error = $state('');
  let loading = $state(true);
  let refresh = $state(0);
  $effect(() => {
    refresh;
    let current = true;
    loading = true;
    error = '';
    value=undefined;
    void (async () => {
      try {
        const result = await load();
        if (current) value=result;
      } catch (cause) {
        if (current) error=message(cause);
      } finally {
        if (current) loading=false;
      }
    })();
    return ()=>{current=false;};
  });
  return { get value(){return value;}, get error(){return error;}, get loading(){return loading;}, refresh(){refresh++;} };
}
