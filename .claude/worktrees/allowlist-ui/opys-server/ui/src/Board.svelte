<script>
  // View 2: one corpus as a board — every document, in a column per status.
  //
  // The whole document set is fetched once and filtered in the browser, rather
  // than pushing the filters into `/docs?type=&tag=`. Two reasons, both about
  // being correct rather than clever:
  //
  //  - The filter dropdowns are built from the documents themselves (no endpoint
  //    enumerates a corpus's types or tags). Fetching filtered would rebuild the
  //    options from the filtered set, so choosing a type would erase every other
  //    type from the menu and leave no way back.
  //  - The text filter has no server-side equivalent at all, so a mixed scheme
  //    would filter in two places at once and have to agree with itself.
  //
  // A corpus is a hand-maintained inventory — hundreds of documents, not
  // millions — so this costs one array and no paging.

  import { api } from './lib/api.js';
  import { corpora } from './lib/corpora.svelte.js';
  import { affects, events } from './lib/events.svelte.js';
  import { corpusLabel, shortDate } from './lib/format.js';
  import { createResource } from './lib/resource.svelte.js';
  import { boardPath, docPath, go, href, queryPath } from './lib/router.svelte.js';

  let { cid, filters = {} } = $props();

  const docs = createResource();

  $effect(() => {
    const wanted = cid;
    docs.run(() => api.docs(wanted));
  });

  // A background refresh on anything the node says about this corpus. Quiet, so
  // a write does not blink the board away and back.
  $effect(() => {
    const wanted = cid;
    return events.subscribe((batch) => {
      if (affects(batch, wanted)) docs.run(() => api.docs(wanted), { quiet: true });
    });
  });

  const corpus = $derived(corpora.find(cid));
  const all = $derived(docs.data ?? []);

  // Facets from the unfiltered set, so the menus list what the corpus contains
  // rather than what the current filter left behind.
  const types = $derived([...new Set(all.map((d) => d.type).filter(Boolean))].sort());
  const tags = $derived([...new Set(all.flatMap((d) => d.tags))].sort());

  // The text box is local, not part of the route: it is a find-as-you-type
  // scratch filter, and putting every keystroke in the hash would fill the back
  // button with them. Type and tag *are* in the route, so a filtered board
  // survives a click into a document and back.
  let text = $state('');
  const needle = $derived(text.trim().toLowerCase());

  function matches(doc) {
    if (filters.type && doc.type !== filters.type) return false;
    if (filters.tag && !doc.tags.includes(filters.tag)) return false;
    if (needle.length === 0) return true;
    const haystack = [doc.id, doc.title, doc.status, doc.path, ...doc.tags].join(' ').toLowerCase();
    return haystack.includes(needle);
  }

  const shown = $derived(all.filter(matches));

  /**
   * The columns, one per status present.
   *
   * Ordered alphabetically, with the unset status last. Not a lifecycle order:
   * each type declares its own statuses in `opys.toml` and the document list
   * does not carry that order, so any "sensible" sequence here would be this
   * UI inventing a vocabulary the project may not use. Alphabetical is at least
   * stable and does not lie. (A corpus-level schema endpoint would fix this
   * properly; nothing exposes one today.)
   */
  const columns = $derived.by(() => {
    const groups = new Map();
    for (const doc of shown) {
      const key = doc.status ?? '';
      if (!groups.has(key)) groups.set(key, []);
      groups.get(key).push(doc);
    }
    return [...groups.entries()]
      .sort(([a], [b]) => {
        if (a === '') return 1;
        if (b === '') return -1;
        return a.localeCompare(b);
      })
      .map(([status, items]) => ({
        status,
        label: status === '' ? '(no status)' : status,
        items: items.sort((x, y) => x.id.localeCompare(y.id, undefined, { numeric: true })),
      }));
  });

  const filtered = $derived(
    Boolean(filters.type) || Boolean(filters.tag) || needle.length > 0,
  );

  /** Filters live in the route, so changing one is a navigation. */
  function setFilter(key, value) {
    go(boardPath(cid, { ...filters, [key]: value }));
  }

  function clearFilters() {
    text = '';
    go(boardPath(cid));
  }
</script>

<header class="head">
  <div class="title">
    <h1>{corpus ? corpusLabel(corpus) : cid}</h1>
    {#if corpus}
      <span class="mono small muted" title="the inventory this corpus serves">{corpus.base}</span>
    {/if}
  </div>
  <p class="counts small">
    {#if docs.data}
      <span class="muted">
        {shown.length === all.length
          ? `${all.length} ${all.length === 1 ? 'document' : 'documents'}`
          : `${shown.length} of ${all.length} documents`}
        {#if docs.loading}· refreshing…{/if}
      </span>
    {/if}
    <!-- The board answers "what is in this corpus"; anything sharper than that
         is a question for SQL, which is one click away rather than a CLI. -->
    <a href={href(queryPath(cid))}>query console</a>
  </p>
</header>

{#if docs.error && docs.data}
  <!-- A refresh failed but the board still has the last good answer, so it stays
       on screen and this says how old it might be. -->
  <div class="notice warn">
    <p>Could not refresh: {docs.error.message}</p>
  </div>
{/if}

{#if docs.error && !docs.data}
  <div class="notice bad">
    <p>{docs.error.message}</p>
    <p class="why">
      {#if docs.error.notLoaded}
        This corpus has never loaded, so there is nothing to show. That is a
        problem with the project, not with this page — check the path in the
        node's allowlist and run <code>opys verify</code> there.
      {:else if docs.error.offline}
        The node is not answering.
      {:else if docs.error.status === 404}
        The node is no longer serving this corpus. It may have been removed from
        the allowlist.
      {:else}
        The node could not answer for this corpus.
      {/if}
    </p>
    <p>
      <button class="btn small" onclick={() => docs.run(() => api.docs(cid))}>Try again</button>
    </p>
  </div>
{:else if !docs.settled}
  <p class="muted">Loading documents…</p>
{:else}
  <div class="filters">
    <label>
      <span class="small muted">Type</span>
      <select value={filters.type ?? ''} onchange={(e) => setFilter('type', e.currentTarget.value)}>
        <option value="">all types</option>
        {#each types as type (type)}
          <option value={type}>{type}</option>
        {/each}
      </select>
    </label>

    <label>
      <span class="small muted">Tag</span>
      <select value={filters.tag ?? ''} onchange={(e) => setFilter('tag', e.currentTarget.value)}>
        <option value="">all tags</option>
        {#each tags as tag (tag)}
          <option value={tag}>{tag}</option>
        {/each}
      </select>
    </label>

    <label class="grow">
      <span class="small muted">Find</span>
      <input
        type="search"
        bind:value={text}
        placeholder="id, title, tag or path"
        autocomplete="off"
      />
    </label>

    {#if filtered}
      <button class="btn small" onclick={clearFilters}>Clear</button>
    {/if}
  </div>

  {#if all.length === 0}
    <div class="notice">
      <p>This corpus has no documents yet.</p>
      <p class="why">
        Create one with <code>opys new --type &lt;type&gt; --title …</code> in the
        project, and it appears here on its own.
      </p>
    </div>
  {:else if shown.length === 0}
    <div class="notice">
      <p>No documents match these filters.</p>
      <p><button class="btn small" onclick={clearFilters}>Clear filters</button></p>
    </div>
  {:else}
    <div class="board">
      {#each columns as column (column.status)}
        <section class="column">
          <h2>
            <span class="status">{column.label}</span>
            <span class="muted small">{column.items.length}</span>
          </h2>
          <ul>
            {#each column.items as doc (doc.id)}
              <li>
                <a class="card panel" href={href(docPath(cid, doc.id))}>
                  <span class="id mono small">{doc.id}</span>
                  <span class="doc-title">{doc.title}</span>
                  <span class="meta">
                    {#if doc.type}<span class="chip">{doc.type}</span>{/if}
                    <!-- Unkeyed on purpose: a tag list is not guaranteed unique
                         (nothing in `verify` says so, and a hand edit or a bad
                         merge can repeat one), and a duplicate key throws out of
                         the render — in production builds too, which would blank
                         the whole board over one repeated word. -->
                    {#each doc.tags as tag}
                      <span class="chip">{tag}</span>
                    {/each}
                    {#if doc.updated}
                      <span class="small muted">{shortDate(doc.updated)}</span>
                    {/if}
                  </span>
                </a>
              </li>
            {/each}
          </ul>
        </section>
      {/each}
    </div>
  {/if}
{/if}

<style>
  .head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 1rem;
    flex-wrap: wrap;
    margin-bottom: 0.75rem;
  }

  .title {
    display: flex;
    align-items: baseline;
    gap: 0.6rem;
    flex-wrap: wrap;
    min-width: 0;
  }

  h1 {
    margin: 0;
    font-size: 1.3rem;
  }

  .counts {
    display: flex;
    align-items: baseline;
    gap: 0.75rem;
    flex-wrap: wrap;
    margin: 0;
  }

  .filters {
    display: flex;
    gap: 0.75rem;
    align-items: end;
    flex-wrap: wrap;
    margin-bottom: 1rem;
  }

  .filters label {
    display: grid;
    gap: 0.15rem;
  }

  .filters .grow {
    flex: 1 1 14rem;
  }

  .filters input {
    width: 100%;
  }

  /* Columns scroll sideways rather than squeezing: a board with eight statuses
     should stay readable instead of becoming eight slivers. */
  .board {
    display: grid;
    grid-auto-flow: column;
    grid-auto-columns: minmax(15rem, 1fr);
    gap: 0.75rem;
    align-items: start;
    overflow-x: auto;
    padding-bottom: 0.5rem;
  }

  .column h2 {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 0.5rem;
    margin: 0 0 0.4rem;
    font-size: 0.8rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    position: sticky;
    top: 0;
    background: var(--bg);
    padding: 0.2rem 0.1rem;
  }

  .column .status {
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .column ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 0.4rem;
  }

  .card {
    display: grid;
    gap: 0.25rem;
    padding: 0.5rem 0.6rem;
    text-decoration: none;
    color: inherit;
  }

  .card:hover {
    border-color: var(--accent);
  }

  .id {
    color: var(--muted);
    letter-spacing: 0.02em;
  }

  .doc-title {
    overflow-wrap: anywhere;
  }

  .meta {
    display: flex;
    flex-wrap: wrap;
    gap: 0.25rem;
    align-items: center;
  }
</style>
