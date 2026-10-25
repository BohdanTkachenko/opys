<script>
  import { onDestroy } from 'svelte';
  import Icon from './lib/Icon.svelte';

  let { id } = $props();
  let message = $state('');
  let timer;
  onDestroy(() => clearTimeout(timer));

  async function copy() {
    clearTimeout(timer);
    try {
      await navigator.clipboard.writeText(id);
      message = 'Copied';
    } catch {
      message = 'Couldn’t copy';
    }
    timer = setTimeout(() => message = '', 2000);
  }
</script>

<span class="copy-id">
  <button class="chip mono" type="button" aria-label={`Copy ${id}`} title="Copy ticket ID" onclick={copy}>
    {id}<Icon name="copy" size={12} />
  </button>
  <span class="feedback small muted" role="status">{message}</span>
</span>

<style>
  .copy-id { display:inline-flex; align-items:center; gap:0.4rem; }
  button { cursor:pointer; color:inherit; }
  button:hover { border-color:var(--accent); color:var(--accent); }
  button:focus-visible { outline:2px solid var(--accent); outline-offset:2px; }
  .feedback:empty { display:none; }
</style>
