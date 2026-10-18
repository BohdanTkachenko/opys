<script>
  // Allowlist management, and the first-run screen that introduces it
  // (FEAT-0036, authorised by ADR-0035).
  //
  // One component, two moods. When the node has no allowlist file at all,
  // `configured` is false and this is onboarding: pick a mode, confirm a scan
  // root, see what would be found. Once a file exists it is the ongoing panel —
  // the same controls, without the welcome. Splitting them would duplicate every
  // control to make one sentence differ.
  //
  // The rules the node enforces (under $HOME, no hidden directories) are *not*
  // re-implemented here. A refusal comes back as a 422 whose message is written
  // to be read, and it is shown verbatim: a second copy of the rules in the
  // browser would drift from the ones that actually bind.

  import { api, ApiError } from './lib/api.js';
  import { corpora } from './lib/corpora.svelte.js';
  import { discovery } from './lib/discovery.svelte.js';

  let setup = $state(null);
  const suggestions = $derived(discovery.items);
  let loading = $state(true);
  let error = $state(null);
  /** A refusal from the last add, shown against the input that caused it. */
  let refusal = $state(null);
  let added = $state(null);
  let typed = $state('');
  let busy = $state(null);
  const readOnly = $derived(setup?.writable === false);

  async function load() {
    loading = true;
    error = null;
    try {
      setup = await api.setup();
      await discovery.reload();
    } catch (e) {
      error = e instanceof ApiError ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  async function setMode(mode) {
    if (readOnly || busy !== null) return;
    busy = 'mode';
    try {
      setup = await api.saveSetup({ mode });
      await discovery.reload({force:true});
    } catch (e) {
      error = e instanceof ApiError ? e.message : String(e);
    } finally {
      busy = null;
    }
  }

  async function add(path, {suggested = false} = {}) {
    if (readOnly || busy !== null) return;
    busy = path;
    refusal = null;
    added = null;
    try {
      setup = await api.allowlist({ action: 'add', path });
      added = {path, suggested};
      if (!suggested && typed.trim() === path) typed = '';
      await discovery.reload({force:true});
      // The roster changed, so the sidebar's copy is stale. Quiet, because the
      // list on screen is still correct until the new one arrives.
      corpora.reload(true);
    } catch (e) {
      refusal = {path, suggested, message:e instanceof ApiError ? e.message : String(e)};
    } finally {
      busy = null;
    }
  }

  async function remove(path) {
    if (readOnly || busy !== null) return;
    busy = path;
    try {
      setup = await api.allowlist({ action: 'remove', path });
      await discovery.reload({force:true});
      corpora.reload(true);
    } catch (e) {
      error = e instanceof ApiError ? e.message : String(e);
    } finally {
      busy = null;
    }
  }

  $effect(() => {
    load();
  });

  const onboarding = $derived(setup !== null && setup.configured === false);
</script>

<div class="setup">
  {#if loading}
    <p class="muted">Loading…</p>
  {:else if error}
    <div class="notice warn"><p>{error}</p></div>
  {:else if setup}
    {#if onboarding}
      <h1>Choose what this node watches</h1>
      <p class="lede">
        opys serves an explicit allowlist — nothing is read until you add it.
        Pick how you want to find projects; you can change this later.
      </p>
    {:else}
      <h1>Allowlist</h1>
      <p class="lede">
        The projects this node serves, and where it looks for more.
      </p>
    {/if}

    {#if readOnly}
      <div class="notice warn" role="note" aria-label="Read-only configuration">
        <p><strong>Read-only configuration</strong></p>
        <p>{setup.read_only_reason ?? 'This configuration cannot be modified here.'}</p>
        <p class="why">Configuration: <code>{setup.path}</code></p>
      </div>
    {/if}

    <section>
      <h2>Discovery</h2>
      <div class="modes">
        <button
          class="mode"
          class:on={setup.mode === 'suggest'}
          disabled={readOnly || busy !== null}
          onclick={() => setMode('suggest')}
        >
          <strong>Suggest</strong>
          <span>Look under the scan root and offer what it finds. Nothing is served until you accept it.</span>
        </button>
        <button
          class="mode"
          class:on={setup.mode === 'off'}
          disabled={readOnly || busy !== null}
          onclick={() => setMode('off')}
        >
          <strong>Off</strong>
          <span>Never scan. The allowlist is exactly what you add by hand.</span>
        </button>
      </div>
      <p class="why">
        Scanning from <code>{setup.scan_root}</code>. There is deliberately no
        automatic mode: adding a project is what causes it to be opened and read,
        so a person stays in that loop.
      </p>
    </section>

    {#if setup.mode !== 'off'}
      <section>
        <h2>Found {suggestions.length > 0 ? `(${suggestions.length})` : ''}</h2>
        <p class="why">Discovered project directories not registered yet, including worktrees and copies.</p>
        {#if added?.suggested}<p class="outcome" role="status">Added <code>{added.path}</code> to registered projects.</p>{/if}
        {#if discovery.error}
          <div class="notice warn">
            <p>Could not refresh discovered projects: {discovery.error.message}</p>
            <button class="btn small" onclick={() => discovery.reload()}>Retry discovery</button>
          </div>
        {/if}
        {#if discovery.loading && suggestions.length === 0}
          <p class="muted">Looking for projects…</p>
        {:else if suggestions.length === 0 && !discovery.error}
          <p class="muted">
            Nothing new under <code>{setup.scan_root}</code>.
          </p>
        {:else if suggestions.length > 0}
          <ul class="rows">
            {#each suggestions as s (s.path)}
              <li>
                <div class="what">
                  <strong>{s.name}</strong>
                  <code>{s.path}</code>
                  {#if refusal?.suggested && refusal.path === s.path}
                    <p class="add-error" role="alert">{refusal.message}</p>
                  {/if}
                </div>
                <button class="btn small" disabled={readOnly || busy !== null} onclick={() => add(s.path, {suggested:true})}>
                  {busy === s.path ? 'Adding…' : 'Add'}
                </button>
              </li>
            {/each}
          </ul>
        {/if}
      </section>
    {/if}

    <section>
      <h2>Add a directory</h2>
      <form
        onsubmit={(e) => {
          e.preventDefault();
          if (!readOnly && typed.trim()) add(typed.trim());
        }}
      >
        <input
          type="text"
          bind:value={typed}
          disabled={readOnly}
          placeholder="~/Projects/something"
          aria-label="Project directory"
          spellcheck="false"
        />
        <button class="btn" type="submit" disabled={readOnly || !typed.trim() || busy !== null}>Add</button>
      </form>
      {#if added && !added.suggested}<p class="outcome" role="status">Added <code>{added.path}</code> to registered projects.</p>{/if}
      {#if refusal && !refusal.suggested}
        <!-- The node's own words. It knows the rules; this screen does not
             restate them, so the two cannot disagree. -->
        <div class="notice warn" role="alert"><p>{refusal.message}</p></div>
      {/if}
      {#if !readOnly}<p class="why">
        Must be inside <code>{setup.home}</code>, and not hidden. Anything
        outside it can be added by editing <code>{setup.path}</code> directly.
      </p>{/if}
    </section>

    <section>
      <h2>Serving ({setup.entries.length})</h2>
      {#if setup.entries.length === 0}
        <p class="muted">Nothing yet.</p>
      {:else}
        <ul class="rows">
          {#each setup.entries as e (e.path)}
            <li>
              <div class="what">
                <code>{e.path}</code>
                <span class="kind">{e.kind}</span>
                {#if e.error}<span class="bad">{e.error}</span>{/if}
              </div>
              <button class="btn small ghost" disabled={readOnly || busy !== null} onclick={() => remove(e.path)}>
                {busy === e.path ? 'Removing…' : 'Remove'}
              </button>
            </li>
          {/each}
        </ul>
      {/if}
    </section>
  {/if}
</div>

<style>
  .setup {
    max-width: 46rem;
    margin-inline: auto;
    padding: 1.5rem;
  }
  h1 {
    margin: 0 0 0.25rem;
    font-size: 1.3rem;
  }
  h2 {
    margin: 0 0 0.5rem;
    font-size: 0.95rem;
  }
  .lede {
    margin: 0 0 1.5rem;
    color: var(--muted);
  }
  section {
    margin-bottom: 1.75rem;
  }
  .modes {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 0.5rem;
  }
  .mode {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    padding: 0.75rem;
    text-align: left;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: none;
    color: inherit;
    cursor: pointer;
    font: inherit;
  }
  .mode.on {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 10%, transparent);
  }
  .mode span {
    font-size: 0.82rem;
    color: var(--muted);
  }
  .rows {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
  }
  .rows li {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    padding: 0.5rem 0.65rem;
    border: 1px solid var(--border);
    border-radius: 6px;
  }
  .what {
    display: flex;
    flex-direction: column;
    gap: 0.15rem;
    min-width: 0;
  }
  .what code {
    font-size: 0.8rem;
    color: var(--muted);
    overflow-wrap: anywhere;
  }
  .kind {
    font-size: 0.72rem;
    color: var(--muted);
  }
  .bad {
    font-size: 0.78rem;
    color: var(--bad);
  }
  .add-error { margin:0.35rem 0 0; color:var(--bad); font-size:0.82rem; overflow-wrap:anywhere; }
  .outcome { padding:0.5rem 0.65rem; border-left:3px solid var(--good); background:color-mix(in srgb,var(--good) 8%,var(--panel)); font-size:0.85rem; overflow-wrap:anywhere; }
  form {
    display: flex;
    gap: 0.5rem;
  }
  input {
    flex: 1;
    padding: 0.45rem 0.6rem;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: none;
    color: inherit;
    font: inherit;
  }
  .why {
    margin: 0.6rem 0 0;
    font-size: 0.82rem;
    color: var(--muted);
  }
  .muted {
    color: var(--muted);
  }
</style>
