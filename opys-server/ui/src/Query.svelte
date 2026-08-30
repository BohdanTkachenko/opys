<script>
  // View 4: the query console — user SQL against one corpus.
  //
  // The node decomposes each corpus into an in-memory SQL database per request,
  // and `POST /query` is that database's read-only door: the plan is inspected
  // and anything that is not a `SELECT` is refused before it runs. So this
  // console never has to guard anything itself, and deliberately does not try —
  // a client-side "is this safe" check would be a second, weaker opinion about
  // the same question, and the first one to disagree with the engine would be
  // this one.
  //
  // Two consequences shape what is rendered. Every cell comes back a *string*
  // already (the node stringifies, so `NULL` and `'NULL'` look alike here — the
  // engine's choice, not something to reinvent), and every failure is a message
  // written for a person: a syntax error, a table that does not exist, or the
  // plan guard naming the statement it rejected. All of them are shown verbatim.

  import { api } from './lib/api.js';
  import { corpora } from './lib/corpora.svelte.js';
  import { affects, events } from './lib/events.svelte.js';
  import { corpusLabel } from './lib/format.js';
  import { createResource } from './lib/resource.svelte.js';
  import { boardPath, href } from './lib/router.svelte.js';

  let { cid } = $props();

  /**
   * Starting points, not a menu of everything worth asking.
   *
   * They cover the tables a reader is most likely to want and, between them,
   * demonstrate the shape of the schema: `docs` is one row per document,
   * `tags`/`relations`/`sections` are one row per *thing inside* a document and
   * join back on `doc_id`. Running one is a click, because a query console whose
   * examples have to be retyped teaches nothing.
   */
  const EXAMPLES = [
    {
      label: 'Every document',
      sql: 'SELECT id, type, status, title FROM docs ORDER BY num',
    },
    {
      label: 'Count by status',
      sql: 'SELECT status, COUNT(*) AS n FROM docs GROUP BY status',
    },
    {
      label: 'Live blockers',
      sql: "SELECT doc_id, ref_id, title FROM relations\n  WHERE field = 'blocked_by' AND struck = false",
    },
    {
      label: 'Unfinished checklists',
      sql: 'SELECT doc_id, heading, items, unchecked FROM sections\n  WHERE unchecked > 0',
    },
    {
      label: 'Tag counts',
      sql: 'SELECT tag, COUNT(*) AS n FROM tags GROUP BY tag',
    },
  ];

  const result = createResource();

  let sql = $state('');
  /**
   * The statement the results on screen came from.
   *
   * Kept separately from the textarea so that editing the query does not
   * relabel a table that still shows the previous answer, and so a background
   * refresh re-runs what was actually asked rather than a half-typed edit.
   */
  let ran = $state(null);

  const corpus = $derived(corpora.find(cid));
  const data = $derived(result.data);
  const runnable = $derived(sql.trim().length > 0 && !result.loading);

  function run() {
    const statement = sql.trim();
    if (statement.length === 0) return;
    ran = statement;
    result.run(() => api.query(cid, statement));
  }

  function use(example) {
    sql = example.sql;
    ran = example.sql;
    result.run(() => api.query(cid, example.sql));
  }

  // The corpus reloads when anything writes to it, so a result set on screen can
  // be about a document that has since changed. Re-running the same statement is
  // the only honest refresh — there is no way to patch a table of arbitrary SQL —
  // and it is quiet, so the previous answer stays put while it happens.
  $effect(() => {
    const wanted = cid;
    return events.subscribe((batch) => {
      if (ran === null || !affects(batch, wanted)) return;
      const statement = ran;
      result.run(() => api.query(wanted, statement), { quiet: true });
    });
  });

  function onkeydown(event) {
    // The usual console chord. Enter alone inserts a newline: these are
    // multi-line statements and submitting on Enter would fight the reader.
    if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
      event.preventDefault();
      run();
    }
  }
</script>

<div class="queryview">
<p class="crumb small">
  <a href={href(boardPath(cid))}>← board</a>
  <span class="muted">{corpus ? corpusLabel(corpus) : cid}</span>
</p>

<header class="head">
  <h1>Query console</h1>
  <p class="small muted">
    Read-only SQL over this corpus. The node refuses anything that is not a
    <code>SELECT</code>; writing SQL is <code>opys query --write</code>, which
    re-verifies the whole corpus before it keeps a change.
  </p>
</header>

<div class="examples">
  <span class="small muted">Start from:</span>
  {#each EXAMPLES as example (example.label)}
    <button class="btn small" disabled={result.loading} onclick={() => use(example)}>
      {example.label}
    </button>
  {/each}
</div>

<div class="editor panel">
  <div class="termbar mono">
    <span class="prompt" aria-hidden="true">❯</span>
    <span class="termtitle">sql · read-only</span>
    <span class="tables muted" title="the tables this corpus decomposes into">
      docs · tags · relations · fm_fields · retired · fields · sections · blocks
    </span>
  </div>
  <textarea
    bind:value={sql}
    {onkeydown}
    rows="5"
    spellcheck="false"
    autocapitalize="off"
    autocorrect="off"
    autocomplete="off"
    placeholder="SELECT id, status, title FROM docs ORDER BY num"
    aria-label="SQL to run against this corpus"
  ></textarea>
  <div class="controls">
    <button class="btn primary" disabled={!runnable} onclick={run}>
      {result.loading ? 'Running…' : 'Run'}
    </button>
    <span class="small muted"><kbd>⌘</kbd>/<kbd>Ctrl</kbd> + <kbd>Enter</kbd></span>
  </div>
</div>

{#if result.error}
  <div class="notice bad">
    <!-- Verbatim: the engine names the table it could not find or the statement
         kind it refused, and a friendlier rewrite would lose exactly that. -->
    <p class="mono">{result.error.message}</p>
    <p class="why">
      {#if result.error.notLoaded}
        This corpus has never loaded, so there is no database to query. The
        statement may be perfectly good — check the project itself.
      {:else if result.error.offline}
        The node is not answering, so the query was never run.
      {:else if result.error.status === 400}
        Nothing ran. Either the statement is not a <code>SELECT</code>, or the
        engine could not plan it.
      {:else if result.error.status === 404}
        The node is no longer serving this corpus.
      {:else}
        <!-- The catch-all the other views have. Without it an error outside the
             cases above (a 500 from the corpus task, say) renders this
             paragraph empty, which looks like a rendering fault. -->
        The node could not run this statement.
      {/if}
    </p>
  </div>
{/if}

{#if data}
  <section class="results">
    <p class="small muted">
      {data.rows.length}
      {data.rows.length === 1 ? 'row' : 'rows'}{#if result.loading}, refreshing…{/if}
      {#if ran && ran !== sql.trim()}
        · from the statement that was run, not the one in the box
      {/if}
    </p>

    {#if data.columns.length === 0}
      <div class="notice"><p>The statement returned no columns.</p></div>
    {:else if data.rows.length === 0}
      <div class="notice">
        <p>No rows matched.</p>
      </div>
    {:else}
      <div class="scroll panel">
        <table>
          <thead>
            <tr>
              {#each data.columns as column, index (index)}
                <th>{column}</th>
              {/each}
            </tr>
          </thead>
          <tbody>
            <!-- Rows have no identity of their own — the statement decides what a
                 row even is — so both loops are keyed by position. -->
            {#each data.rows as row, rowIndex (rowIndex)}
              <tr>
                {#each data.columns as _column, cellIndex (cellIndex)}
                  <td>{row[cellIndex] ?? ''}</td>
                {/each}
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  </section>
{/if}
</div>

<style>
  .queryview {
    max-width: 62rem;
    margin-inline: auto;
  }

  .head {
    margin-bottom: 0.75rem;
  }

  h1 {
    margin: 0 0 0.25rem;
    font-size: 1.3rem;
  }

  .head p {
    margin: 0;
    max-width: 46rem;
  }

  .examples {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.35rem;
    margin-bottom: 0.5rem;
  }

  /* Each example runs on click; the marker says so without a verb. */
  .examples :global(.btn)::before {
    content: '\25B8';
    color: var(--accent);
    font-size: 0.8em;
  }

  /* The console is dressed as what it is: a terminal. A slim title bar, the
     statement area below it, the run button in a footer row. One frame — the
     textarea drops its own border and melts into the panel. */
  .editor {
    display: grid;
    margin-bottom: 1rem;
    overflow: hidden;
  }

  .termbar {
    display: flex;
    align-items: baseline;
    gap: 0.5rem;
    padding: 0.4rem 0.7rem;
    border-bottom: 1px solid var(--border);
    background: var(--raised);
    font-size: 0.78rem;
  }

  .termbar .prompt {
    color: var(--accent);
    font-weight: 700;
  }

  .termbar .termtitle {
    letter-spacing: 0.04em;
    white-space: nowrap;
  }

  @media (max-width: 46rem) {
    .termbar .tables {
      display: none;
    }
  }

  .termbar .tables {
    margin-left: auto;
    font-size: 0.9em;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  textarea {
    width: 100%;
    border: none;
    border-radius: 0;
    background: var(--panel);
    padding: 0.6rem 0.7rem;
    font-family: var(--font-mono);
    font-size: 0.9em;
    resize: vertical;
    /* Tabs and leading spaces are meaningful in a pasted statement. */
    white-space: pre;
    overflow-wrap: normal;
    overflow-x: auto;
  }

  textarea:focus-visible {
    /* The panel is the frame; a ring inside it would double-border. */
    outline: none;
    box-shadow: inset 0 0 0 1px var(--accent);
  }

  .controls {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    flex-wrap: wrap;
    padding: 0.45rem 0.7rem;
    border-top: 1px solid var(--border);
    background: var(--raised);
  }

  .results p {
    margin: 0 0 0.4rem;
  }

  /* A wide result scrolls inside its own frame rather than pushing the page
     sideways. */
  .scroll {
    overflow-x: auto;
    max-width: 100%;
  }

  table {
    border-collapse: collapse;
    width: 100%;
    font-size: 0.9em;
    /* Query results are rows from a database; they read as one. */
    font-family: var(--font-mono);
    font-variant-numeric: tabular-nums;
  }

  th,
  td {
    border-bottom: 1px solid var(--border);
    padding: 0.3rem 0.6rem;
    text-align: left;
    vertical-align: top;
    /* Long prose in a cell (a body, a title) wraps instead of making one column
       as wide as the document. */
    max-width: 32rem;
    overflow-wrap: anywhere;
    white-space: pre-wrap;
  }

  thead th {
    position: sticky;
    top: 0;
    background: var(--raised);
    font-size: 0.85em;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--muted);
    white-space: nowrap;
  }

  tbody tr:last-child td {
    border-bottom: none;
  }

  tbody tr:nth-child(even) {
    background: color-mix(in srgb, var(--raised) 40%, transparent);
  }

  tbody tr:hover {
    background: var(--raised);
  }
</style>
