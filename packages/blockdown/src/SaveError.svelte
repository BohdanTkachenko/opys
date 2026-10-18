<script>
  let { message, diagnosed = false, saving = false, onretry, ondiscard, onedit = null, id = undefined } = $props();
  function act(event, callback) {
    event.stopPropagation();
    callback?.();
  }
</script>

<div class="bd-save-error" role="alert" {id}>
  <div>{diagnosed ? 'Change was not saved.' : `Could not save this change: ${message}`}</div>
  <div class="bd-save-actions">
    {#if onedit}<button type="button" disabled={saving} onclick={(event) => act(event, onedit)}>Edit source</button>{/if}
    <button type="button" disabled={saving} onclick={(event) => act(event, onretry)}>Retry</button>
    <button type="button" disabled={saving} onclick={(event) => act(event, ondiscard)}>Discard unsaved changes</button>
  </div>
</div>

<style>
  .bd-save-error { margin-top:0.5em; padding:0.6em 0.75em; border-left:3px solid var(--bd-error,Crimson); background:color-mix(in srgb,var(--bd-error,Crimson) 8%,transparent); color:var(--bd-error,Crimson); font-size:0.8em; line-height:1.5; white-space:pre-wrap; overflow-wrap:anywhere; }
  .bd-save-actions { display:flex; flex-wrap:wrap; gap:0.5em; margin-top:0.5em; }
</style>
