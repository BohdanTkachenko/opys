<script>
  // View 5: one project group's worktrees, side by side.
  //
  // A group is a repository's main worktree plus the siblings that carry the
  // same inventory. Each has its own copy of every document and they drift — a
  // task is `doing` on main and `done` on a branch, a document exists only where
  // it was written, two branches hand out the same id number. The table makes
  // that visible and does nothing else: nothing here merges or writes, because
  // git is the only merger (ADR-0051).
  //
  // Two things in the payload are easy to render into a lie, and most of the
  // care in this file is about them:
  //
  //  1. **A blank cell has two meanings.** `status === null` is "this worktree
  //     does not have this document"; `unknown` is "this worktree did not
  //     answer". They must not look alike — the first justifies "new on a
  //     branch", the second justifies nothing at all.
  //  2. **A filter is applied per corpus *before* the merge.** So a filtered
  //     union answers "where does this match", not "where does this exist", and
  //     filtering by status hides one side of every status drift. That is said
  //     next to the filter itself, below, because a reader who does not know it
  //     is being shown the opposite of what they came for.

  import { api } from './lib/api.js';
  import { corpora } from './lib/corpora.svelte.js';
  import { affects, events } from './lib/events.svelte.js';
  import { shortDate, statusTone } from './lib/format.js';
  import { createResource } from './lib/resource.svelte.js';
  import { docPath, go, href, unionPath } from './lib/router.svelte.js';

  let { key, filters = {} } = $props();

  const table = createResource();

  // The filter values are pulled out here, in a `$derived`, so the effects below
  // depend on the *values*: reading `filters.status` inside an async callback
  // instead would subscribe to nothing and leave the table showing the answer to
  // the previous filter.
  const query = $derived({
    type: filters.type ?? '',
    status: filters.status ?? '',
    tag: filters.tag ?? '',
  });

  $effect(() => {
    const [wanted, asked] = [key, query];
    table.run(() => api.union(wanted, asked));
  });

  $effect(() => {
    const [wanted, asked] = [key, query];
    return events.subscribe((batch) => {
      // Which corpora this view is watching is the column list itself, which is
      // always right — the group's membership is discovery's business and can
      // change under us. Before the first answer there is no list, so anything
      // counts.
      const cids = (table.data?.columns ?? []).map((column) => column.cid);
      if (cids.length > 0 && !cids.some((cid) => affects(batch, cid))) return;
      table.run(() => api.union(wanted, asked), { quiet: true });
    });
  });

  const group = $derived(corpora.groups.find((candidate) => candidate.key === key));
  const view = $derived(table.data);

  /** cid → column label, for naming the corpora a row is `only_in`. */
  const labels = $derived(
    new Map((view?.columns ?? []).map((column) => [column.cid, column.label])),
  );

  const silent = $derived((view?.columns ?? []).filter((column) => 'error' in column));

  const drifted = $derived((view?.rows ?? []).filter((row) => row.differs).length);
  const collisions = $derived((view?.rows ?? []).filter((row) => row.collision).length);

  // The filter inputs follow the route until the reader types, and go back to
  // following it the moment it changes — an overridable `$derived`, the same
  // shape the document view's status menu uses. They are free text rather than
  // menus because nothing enumerates a group's types, statuses or tags, and
  // deriving the options from the *filtered* answer would delete every value the
  // current filter excluded.
  let type = $derived(filters.type ?? '');
  let status = $derived(filters.status ?? '');
  let tag = $derived(filters.tag ?? '');

  const filtered = $derived(
    Boolean(filters.type) || Boolean(filters.status) || Boolean(filters.tag),
  );

  function apply(event) {
    event?.preventDefault();
    go(unionPath(key, { type: type.trim(), status: status.trim(), tag: tag.trim() }));
  }

  function clear() {
    go(unionPath(key));
  }

  /** What a present cell's status should read as; `''` is an answer, not a gap. */
  function statusText(cell) {
    return cell.status === '' ? '(no status)' : cell.status;
  }
</script>

<header class="head topbar">
  <div class="title">
    <h1>{group ? group.name : key}</h1>
    {#if view}
      <span class="small muted">
        {view.columns.length}
        {view.columns.length === 1 ? 'worktree' : 'worktrees'} ·
        {view.rows.length}
        {view.rows.length === 1 ? 'document' : 'documents'}
        {#if table.loading}· refreshing…{/if}
      </span>
    {/if}
  </div>
</header>

{#if table.error && !table.data}
  <div class="notice bad">
    <p>{table.error.message}</p>
    <p class="why">
      {#if table.error.status === 404}
        No project group with that key is served. Group keys are derived from the
        repository, not from a corpus id — a link built from a cid lands here.
      {:else if table.error.offline}
        The node is not answering.
      {:else}
        The node could not build this view.
      {/if}
    </p>
    <p><button class="btn small" onclick={() => table.run(() => api.union(key, filters))}>
      Try again
    </button></p>
  </div>
{:else if !table.settled}
  <p class="muted">Loading the union…</p>
{:else if view}
  {#if table.error}
    <div class="notice warn"><p>Could not refresh: {table.error.message}</p></div>
  {/if}

  <form class="filters" onsubmit={apply}>
    <label>
      <span class="microlabel">Type</span>
      <input bind:value={type} placeholder="task" spellcheck="false" autocomplete="off" />
    </label>
    <label>
      <span class="microlabel">Status</span>
      <input bind:value={status} placeholder="in-progress" spellcheck="false" autocomplete="off" />
    </label>
    <label>
      <span class="microlabel">Tag</span>
      <input bind:value={tag} placeholder="server" spellcheck="false" autocomplete="off" />
    </label>
    <button class="btn" type="submit">Filter</button>
    {#if filtered}
      <button class="btn" type="button" onclick={clear}>Clear</button>
    {/if}

    <!-- Obligation from TASK-0073, and the reason it is here rather than in a
         tooltip: the node filters each worktree's documents *before* merging
         them, so a filter can hide exactly the disagreement this view exists to
         show. Saying it beside the fields is the only placement that reaches
         someone in the act of typing one. -->
    <p class="caution small">
      Filters are matched in each worktree <strong>before</strong> the columns are
      merged, so a filtered union answers <em>where does this match</em> — not
      <em>where does this exist</em>. Filtering by <code>status</code> in
      particular hides one half of every status drift: a task that is
      <code>doing</code> on main and <code>done</code> on a branch appears under
      <code>status=doing</code> as present only on main, with no drift marked.
      Only the id-collision warning survives a filter; it is computed from the
      unfiltered document sets.
    </p>
  </form>

  {#if filtered}
    <div class="notice warn">
      <p>
        This table is filtered
        {#if filters.type}by type <code>{filters.type}</code>{/if}{#if filters.type && (filters.status || filters.tag)},
        {/if}{#if filters.status}by status <code>{filters.status}</code>{/if}{#if filters.status && filters.tag},
        {/if}{#if filters.tag}by tag <code>{filters.tag}</code>{/if}. Absences and
        “no drift” below mean <em>did not match here</em>, which is not the same
        as <em>is not here</em>.
      </p>
      <p><button class="btn small" onclick={clear}>Show everything</button></p>
    </div>
  {/if}

  {#if silent.length > 0}
    <div class="notice bad">
      <p>
        {silent.length}
        {silent.length === 1 ? 'worktree' : 'worktrees'} did not answer, so
        {silent.length === 1 ? 'its column says' : 'their columns say'} nothing at
        all — not that the documents are missing there.
      </p>
      <ul class="small">
        {#each silent as column (column.cid)}
          <li><strong>{column.label}</strong> — {column.error}</li>
        {/each}
      </ul>
    </div>
  {/if}

  {#if view.columns.length === 0}
    <div class="notice">
      <p>This group has no corpora.</p>
      <p class="why">
        It may have been pruned between the sidebar's list and this request.
      </p>
    </div>
  {:else if view.rows.length === 0}
    <div class="notice">
      <p>
        {filtered
          ? 'No document matches these filters in any worktree.'
          : 'No worktree in this group has any documents.'}
      </p>
    </div>
  {:else}
    <p class="tally small muted">
      {#if collisions > 0}
        <span class="mark collision">{collisions} id collision{collisions === 1 ? '' : 's'}</span>
      {/if}
      {#if drifted > 0}
        <span class="mark drift">{drifted} drifted</span>
      {/if}
      {#if collisions === 0 && drifted === 0}
        No drift and no id collisions among the worktrees that answered.
      {/if}
    </p>

    <div class="scroll panel">
      <table>
        <thead>
          <tr>
            <th class="doc">Document</th>
            {#each view.columns as column (column.cid)}
              <!-- `error` is omitted from the JSON when absent, so this is a
                   presence test; `column.error === null` is false for every
                   column, healthy or not. -->
              <th class:silent={'error' in column} title={column.error ?? column.cid}>
                {column.label}
                {#if 'error' in column}<span class="small">did not answer</span>{/if}
              </th>
            {/each}
          </tr>
        </thead>
        <tbody>
          {#each view.rows as row (row.id)}
            <tr class:drift={row.differs} class:collision={row.collision}>
              <th class="doc" scope="row">
                <span class="mono small">{row.id}</span>
                <span class="doc-title">{row.title}</span>
                <span class="marks">
                  {#if row.collision}
                    <span class="mark collision" title="another id claims this number">
                      id collision
                    </span>
                  {/if}
                  {#if row.differs}
                    <span class="mark drift" title="the worktrees that have it disagree">
                      drifted
                    </span>
                  {/if}
                  {#if row.only_in.length > 0}
                    <span class="mark only" title="the worktrees that have it">
                      only in {row.only_in.map((cid) => labels.get(cid) ?? cid).join(', ')}
                    </span>
                  {/if}
                </span>
              </th>

              {#each row.cells as cell (cell.cid)}
                {#if cell.unknown}
                  <!-- The column errored. Nothing is claimed about this document
                       here, and the striping says so: an empty-looking cell that
                       meant "absent" would turn one broken worktree into a
                       branch that deleted the inventory. -->
                  <td class="cell unknown" title="this worktree did not answer">
                    <span class="small">no answer</span>
                  </td>
                {:else if cell.status === null}
                  <td class="cell absent" title="this worktree does not have this document">
                    <span class="small">not here</span>
                  </td>
                {:else}
                  {@const tone = statusTone(cell.status)}
                  <td class="cell">
                    <a
                      class="chip status"
                      style:--tone={tone}
                      class:neutral={tone === null}
                      href={href(docPath(cell.cid, row.id))}
                    >
                      {statusText(cell)}
                    </a>
                    {#if cell.title && cell.title !== row.title}
                      <!-- A title that disagrees with the row's is half of what
                           `differs` is reporting, and it appears nowhere else. -->
                      <span class="retitled small">“{cell.title}”</span>
                    {/if}
                    {#if cell.updated}
                      <span class="small muted">{shortDate(cell.updated)}</span>
                    {/if}
                  </td>
                {/if}
              {/each}
            </tr>
          {/each}
        </tbody>
      </table>
    </div>

    <dl class="legend small muted">
      <div><dt><span class="swatch drift"></span> drifted</dt>
        <dd>the worktrees that have it disagree about its status or title</dd></div>
      <div><dt><span class="swatch collision"></span> id collision</dt>
        <dd>two different ids claim the same number; they will collide when the
          branches meet — <code>opys renumber</code> is the repair</dd></div>
      <div><dt><span class="swatch absent"></span> not here</dt>
        <dd>this worktree answered, and does not have the document</dd></div>
      <div><dt><span class="swatch unknown"></span> no answer</dt>
        <dd>this worktree could not be asked; nothing is known either way</dd></div>
    </dl>
  {/if}
{/if}

<style>


  .title {
    display: flex;
    align-items: baseline;
    gap: 0.6rem;
    flex-wrap: wrap;
  }

  h1 {
    margin: 0;
    font-size: 1.3rem;
    overflow-wrap: anywhere;
  }

  .filters {
    display: flex;
    gap: 0.6rem;
    align-items: end;
    flex-wrap: wrap;
    margin-bottom: 1rem;
  }

  .filters label {
    display: grid;
    gap: 0.15rem;
  }

  .filters input {
    width: 9rem;
  }

  /* Full width, so it sits under the fields rather than beside them: this is the
     warning that keeps the view honest, not a footnote. */
  .caution {
    flex: 1 0 100%;
    margin: 0;
    padding: 0.4rem 0.6rem;
    border-left: 3px solid var(--warn);
    background: var(--panel);
    border-radius: 0 4px 4px 0;
    color: var(--muted);
    max-width: 52rem;
  }

  .notice ul {
    margin: 0.35rem 0 0;
    padding-left: 1.1rem;
  }

  .tally {
    display: flex;
    gap: 0.4rem;
    align-items: center;
    flex-wrap: wrap;
    margin: 0 0 0.4rem;
  }

  .scroll {
    overflow-x: auto;
    max-width: 100%;
  }

  table {
    border-collapse: collapse;
    width: 100%;
  }

  th,
  td {
    border-bottom: 1px solid var(--border);
    padding: 0.35rem 0.6rem;
    text-align: left;
    vertical-align: top;
  }

  thead th {
    background: var(--raised);
    font-size: 0.8rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--muted);
    white-space: nowrap;
  }

  thead th.silent {
    color: var(--bad);
  }

  thead th.silent span {
    display: block;
    text-transform: none;
    letter-spacing: 0;
  }

  th.doc {
    min-width: 16rem;
    font-weight: 400;
  }

  tbody th.doc {
    display: table-cell;
    text-transform: none;
    letter-spacing: 0;
    color: inherit;
    background: none;
  }

  .doc-title {
    display: block;
    overflow-wrap: anywhere;
  }

  .marks {
    display: flex;
    flex-wrap: wrap;
    gap: 0.25rem;
    margin-top: 0.15rem;
  }

  .mark {
    font-size: 0.75rem;
    border: 1px solid var(--border);
    border-radius: 3px;
    padding: 0 0.3rem;
    white-space: nowrap;
    color: var(--muted);
  }

  /* Drift is a warning; a collision is a fault. They are marked with different
     weights on purpose — a document taken in two directions is normal branch
     life, two branches minting the same id number is not. */
  .mark.drift {
    border-color: var(--warn);
    color: var(--warn);
  }

  .mark.collision {
    border-color: var(--bad);
    background: var(--bad);
    color: var(--bg);
    font-weight: 600;
  }

  tbody tr.drift {
    background: color-mix(in srgb, var(--warn) 8%, transparent);
  }

  tbody tr.collision {
    background: color-mix(in srgb, var(--bad) 10%, transparent);
    box-shadow: inset 3px 0 0 var(--bad);
  }

  .cell {
    white-space: nowrap;
  }

  .cell .retitled {
    display: block;
    white-space: normal;
    overflow-wrap: anywhere;
    color: var(--warn);
  }

  /* The two blanks, told apart at a glance: an absence is quiet, a column that
     said nothing is hatched. */
  .absent {
    color: var(--muted);
    opacity: 0.7;
  }

  .unknown {
    color: var(--muted);
    font-style: italic;
    background: repeating-linear-gradient(
      45deg,
      transparent,
      transparent 4px,
      var(--raised) 4px,
      var(--raised) 8px
    );
  }

  .legend {
    display: grid;
    gap: 0.2rem 1rem;
    margin: 0.75rem 0 0;
    max-width: 52rem;
  }

  .legend > div {
    display: flex;
    gap: 0.4rem;
    align-items: baseline;
    flex-wrap: wrap;
  }

  .legend dt {
    display: flex;
    align-items: center;
    gap: 0.3rem;
    flex: none;
    font-weight: 600;
  }

  .legend dd {
    margin: 0;
    flex: 1 1 18rem;
  }

  .swatch {
    display: inline-block;
    width: 0.7rem;
    height: 0.7rem;
    border: 1px solid var(--border);
    border-radius: 2px;
  }

  .swatch.drift {
    background: color-mix(in srgb, var(--warn) 35%, transparent);
    border-color: var(--warn);
  }

  .swatch.collision {
    background: var(--bad);
    border-color: var(--bad);
  }

  .swatch.absent {
    background: transparent;
  }

  .swatch.unknown {
    background: repeating-linear-gradient(
      45deg,
      transparent,
      transparent 2px,
      var(--raised) 2px,
      var(--raised) 4px
    );
  }
</style>
