<script>
  // View 1: the projects a node serves.
  //
  // A *project group* is a repository and its worktrees; a *corpus* is one
  // inventory inside one of them. The two identifiers are different namespaces
  // and only coincide for a single non-git project, so every corpus link uses
  // `corpus.cid` and anything group-shaped uses `group.key` — mixing them
  // produces a 404 that looks like a bug in the node.
  //
  // The verify dot comes from `/api/projects`, which already carries each
  // corpus's cached problem count. Asking `/api/corpus/{cid}/verify` per corpus
  // would be one request per project for a number the list just delivered.

  import { corpora } from './lib/corpora.svelte.js';
  import { events } from './lib/events.svelte.js';
  import { corpusLabel, shortTime } from './lib/format.js';
  import { boardPath, href, unionPath } from './lib/router.svelte.js';

  let { activeCid = null, activeKey = null } = $props();

  /**
   * How healthy a corpus is, in three states.
   *
   * `unknown` is not a rounding of `good`: a corpus that has never loaded
   * reports `null` counts, and showing that as a green tick would claim a clean
   * verify for an inventory nobody has read.
   */
  function health(corpus) {
    // `error` is omitted from the JSON when absent, so this is a presence test —
    // `corpus.error === null` would be false for every corpus, healthy or not.
    if ('error' in corpus) {
      return { kind: 'bad', label: corpus.error };
    }
    if (corpus.verify_problems === null || corpus.verify_problems === undefined) {
      return { kind: 'unknown', label: 'not loaded yet' };
    }
    if (corpus.verify_problems > 0) {
      const count = corpus.verify_problems;
      return { kind: 'bad', label: `${count} verify ${count === 1 ? 'problem' : 'problems'}` };
    }
    return { kind: 'good', label: 'verify is clean' };
  }
</script>

<aside class="sidebar">
  <header>
    <a class="wordmark" href="#/">opys</a>
    <span
      class="live"
      class:down={!events.live}
      title={events.live
        ? 'Live: the node is streaming changes to this page.'
        : 'The event stream is down. This page is retrying, and will refresh itself when it reconnects.'}
    >
      <span class="dot" class:good={events.live}></span>
      {#if events.status === 'open'}
        live
      {:else if events.status === 'connecting'}
        connecting…
      {:else}
        reconnecting…
      {/if}
    </span>
  </header>

  {#if corpora.error && corpora.groups.length === 0}
    <div class="notice bad">
      <p>{corpora.error.message}</p>
      <p class="why">
        {#if corpora.error.offline}
          The node is not answering. It may have stopped, or this page may have
          been left open past a restart.
        {:else}
          The node answered, but not with the project list.
        {/if}
      </p>
      <p><button class="btn small" onclick={() => corpora.reload()}>Try again</button></p>
    </div>
  {:else if !corpora.settled}
    <p class="muted small pad">Loading projects…</p>
  {:else if corpora.empty}
    <div class="notice">
      <p>No projects.</p>
      <p class="why">
        This node serves an allowlist. <a href={href('/setup')}>Add a project</a>
        to get started.
      </p>
    </div>
  {:else}
    <nav>
      {#each corpora.groups as group (group.key)}
        <section>
          <h2 title={group.key}>{group.name}</h2>
          <ul>
            {#each group.corpora as corpus (corpus.cid)}
              {@const state = health(corpus)}
              <li>
                <a
                  class="corpus"
                  class:active={corpus.cid === activeCid}
                  href={href(boardPath(corpus.cid))}
                  title={corpus.root}
                >
                  <span
                    class="dot"
                    class:good={state.kind === 'good'}
                    class:bad={state.kind === 'bad'}
                    title={state.label}
                  ></span>
                  <span class="name">{corpusLabel(corpus)}</span>
                  {#if corpus.is_primary}
                    <!-- The main worktree: the one a branch's changes are
                         eventually for. Worth marking, because every other
                         column in a union view is measured against it. -->
                    <span class="primary" title="the primary worktree">primary</span>
                  {/if}
                  {#if corpus.doc_count !== null && corpus.doc_count !== undefined}
                    <span class="count muted">{corpus.doc_count}</span>
                  {/if}
                </a>
                {#if state.kind === 'bad'}
                  <p class="problem small">{state.label}</p>
                {/if}
              </li>
            {/each}
          </ul>
          {#if group.corpora.length > 1}
            <!-- Offered only for a group with something to compare. One worktree
                 makes a valid one-column union — the node builds it happily —
                 but a link promising a comparison that cannot exist is noise in
                 the one place that has to stay scannable. -->
            <a
              class="union"
              class:active={group.key === activeKey}
              href={href(unionPath(group.key))}
            >
              compare {group.corpora.length} worktrees
            </a>
          {/if}
        </section>
      {/each}
    </nav>
  {/if}

  <footer>
    {#if events.version}
      <span class="mono small">opys-server {events.version}</span>
    {/if}
    {#if corpora.groups.length > 0}
      {@const newest = corpora.groups
        .flatMap((group) => group.corpora)
        .map((corpus) => corpus.loaded_at)
        .filter(Boolean)
        .sort()
        .at(-1)}
      {#if newest}
        <span class="small muted" title="the most recent corpus load">
          loaded {shortTime(newest)}
        </span>
      {/if}
    {/if}
  </footer>
</aside>

<style>
  .sidebar {
    border-right: 1px solid var(--border);
    background: var(--panel);
    padding: 0.9rem 0.75rem 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
    /* The list stays put while the main column scrolls; on a narrow screen the
       shell stacks and this becomes a normal block. */
    position: sticky;
    top: 0;
    align-self: start;
    max-height: 100vh;
    overflow-y: auto;
  }

  @media (max-width: 46rem) {
    .sidebar {
      position: static;
      max-height: none;
      border-right: none;
      border-bottom: 1px solid var(--border);
    }
  }

  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
  }

  .wordmark {
    font-weight: 600;
    letter-spacing: 0.06em;
    text-decoration: none;
    color: inherit;
  }

  .live {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    font-size: 0.75em;
    color: var(--muted);
  }

  .live.down {
    color: var(--warn);
  }

  nav {
    display: grid;
    gap: 0.9rem;
    flex: 1;
  }

  h2 {
    margin: 0 0 0.3rem;
    font-size: 0.75rem;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--muted);
    overflow-wrap: anywhere;
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 0.15rem;
  }

  .corpus {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0.25rem 0.4rem;
    border-radius: 4px;
    text-decoration: none;
    color: inherit;
  }

  .corpus:hover {
    background: var(--raised);
  }

  .corpus.active {
    background: var(--raised);
    box-shadow: inset 2px 0 0 var(--accent);
  }

  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .primary {
    font-size: 0.65em;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    border: 1px solid var(--border);
    border-radius: 3px;
    padding: 0 0.25rem;
    color: var(--muted);
  }

  .count {
    margin-left: auto;
    font-size: 0.8em;
    font-variant-numeric: tabular-nums;
  }

  .union {
    display: block;
    margin: 0.2rem 0 0 0.4rem;
    padding: 0.1rem 0.3rem;
    border-radius: 4px;
    font-size: 0.8em;
    color: var(--muted);
    text-decoration: none;
  }

  .union:hover {
    color: var(--accent);
  }

  .union.active {
    background: var(--raised);
    color: var(--fg);
    box-shadow: inset 2px 0 0 var(--accent);
  }

  .problem {
    margin: 0 0 0.2rem 1.4rem;
    color: var(--bad);
    overflow-wrap: anywhere;
  }

  .pad {
    padding: 0 0.4rem;
  }

  footer {
    display: grid;
    gap: 0.15rem;
    border-top: 1px solid var(--border);
    padding-top: 0.6rem;
    color: var(--muted);
  }
</style>
