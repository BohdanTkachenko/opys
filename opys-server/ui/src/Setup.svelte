<script>
  // Allowlist management, and the first-run screen that introduces it
  // (FEAT-0083, authorised by ADR-0082).
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

  let setup = $state(null);
  let suggestions = $state([]);
  let loading = $state(true);
  let error = $state(null);
  /** A refusal from the last add, shown against the input that caused it. */
  let refusal = $state(null);
  let typed = $state('');
  let busy = $state(null);

  async function load() {
    loading = true;
    error = null;
    try {
      setup = await api.setup();
      await loadSuggestions();
    } catch (e) {
      error = e instanceof ApiError ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  async function loadSuggestions() {
    if (setup?.mode === 'off') {
      suggestions = [];
      return;
    }
    try {
      suggestions = await api.suggestions();
    } catch {
      // A failed scan is not a failed screen: the allowlist below is still
      // readable and editable without it.
      suggestions = [];
    }
  }

  async function setMode(mode) {
    busy = 'mode';
    try {
      setup = await api.saveSetup({ mode });
      await loadSuggestions();
    } catch (e) {
      error = e instanceof ApiError ? e.message : String(e);
    } finally {
      busy = null;
    }
  }

  async function add(path) {
    busy = path;
    refusal = null;
    try {
      setup = await api.allowlist({ action: 'add', path });
      typed = '';
      await loadSuggestions();
      // The roster changed, so the sidebar's copy is stale. Quiet, because the
      // list on screen is still correct until the new one arrives.
      corpora.reload(true);
    } catch (e) {
      refusal = e instanceof ApiError ? e.message : String(e);
    } finally {
      busy = null;
    }
  }

  async function remove(path) {
    busy = path;
    try {
      setup = await api.allowlist({ action: 'remove', path });
      await loadSuggestions();
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

    <section>
      <h2>Discovery</h2>
      <div class="modes">
        <button
          class="mode"
          class:on={setup.mode === 'suggest'}
          disabled={busy === 'mode'}
          onclick={() => setMode('suggest')}
        >
          <strong>Suggest</strong>
          <span>Look under the scan root and offer what it finds. Nothing is served until you accept it.</span>
        </button>
        <button
          class="mode"
          class:on={setup.mode === 'off'}
          disabled={busy === 'mode'}
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
        {#if suggestions.length === 0}
          <p class="muted">
            Nothing new under <code>{setup.scan_root}</code>.
          </p>
        {:else}
          <ul class="rows">
            {#each suggestions as s (s.path)}
              <li>
                <div class="what">
                  <strong>{s.name}</strong>
                  <code>{s.path}</code>
                </div>
                <button class="btn small" disabled={busy === s.path} onclick={() => add(s.path)}>
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
          if (typed.trim()) add(typed.trim());
        }}
      >
        <input
          type="text"
          bind:value={typed}
          placeholder="~/Projects/something"
          aria-label="Project directory"
          spellcheck="false"
        />
        <button class="btn" type="submit" disabled={!typed.trim() || busy === typed.trim()}>Add</button>
      </form>
      {#if refusal}
        <!-- The node's own words. It knows the rules; this screen does not
             restate them, so the two cannot disagree. -->
        <div class="notice warn"><p>{refusal}</p></div>
      {/if}
      <p class="why">
        Must be inside <code>{setup.home}</code>, and not hidden. Anything
        outside it can be added by editing <code>{setup.path}</code> directly.
      </p>
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
              <button class="btn small ghost" disabled={busy === e.path} onclick={() => remove(e.path)}>
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
