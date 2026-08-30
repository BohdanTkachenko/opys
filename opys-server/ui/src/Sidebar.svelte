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
  //
  // On a narrow screen the shell stacks and this becomes a one-line bar —
  // wordmark, live dot, a projects toggle — that opens into the list on
  // demand. A phone that gave a third of every screen to a corpus list would
  // put the first card of every board below the fold.

  import Icon from './lib/Icon.svelte';
  import { corpora } from './lib/corpora.svelte.js';
  import { events } from './lib/events.svelte.js';
  import { corpusLabel, shortTime } from './lib/format.js';
  import { boardPath, href, unionPath } from './lib/router.svelte.js';

  let { activeCid = null, activeKey = null } = $props();

  /** The narrow-screen list, opened by the toggle and closed by navigating. */
  let open = $state(false);

  $effect(() => {
    // Reading the props subscribes to them: any navigation folds the list
    // back into the bar, so a tap on a corpus lands on its board, not on the
    // list that led there.
    void activeCid;
    void activeKey;
    open = false;
  });

  const served = $derived(corpora.groups.reduce((n, group) => n + group.corpora.length, 0));

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

<aside class="sidebar" class:open>
  <header>
    <a class="wordmark mono" href="#/">
      <span class="prompt" aria-hidden="true">❯</span><span class="grad-text">opys</span><span
        class="cursor"
        aria-hidden="true">▌</span
      >
    </a>
    <span
      class="live mono"
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
    <!-- Narrow screens only (hidden by CSS elsewhere): the list behind it. -->
    <button
      class="menu"
      type="button"
      aria-expanded={open}
      aria-controls="projects"
      title={open ? 'hide the project list' : 'show the project list'}
      onclick={() => (open = !open)}
    >
      <Icon name={open ? 'x' : 'menu'} size={15} />
      {#if !open && served > 0}
        <span class="mono">{served}</span>
      {/if}
    </button>
  </header>

  <div class="list" id="projects">
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
                <Icon name="compare" size={13} /> compare {group.corpora.length} worktrees
              </a>
            {/if}
          </section>
        {/each}
      </nav>
    {/if}
  </div>

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
    /* Glass: translucent over the page's corner glows, so the sidebar reads as
       a pane in front of the room rather than a column painted beside it. The
       solid `background` first is the fallback where backdrop-filter is not
       supported — translucency without blur is mud. */
    background: var(--panel);
    padding: 0.95rem 0.8rem 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.8rem;
    /* The list stays put while the main column scrolls; on a narrow screen the
       shell stacks and this becomes a normal block. */
    position: sticky;
    top: 0;
    align-self: start;
    /* min as well as max: without a floor the sidebar shrinks to its content
       and the footer floats mid-air instead of pinning to the bottom. */
    min-height: 100vh;
    max-height: 100vh;
    overflow-y: auto;
  }

  @supports (backdrop-filter: blur(1px)) {
    .sidebar {
      background: var(--overlay);
      backdrop-filter: blur(14px);
      -webkit-backdrop-filter: blur(14px);
    }
  }

  .list {
    display: contents;
  }

  .menu {
    display: none;
  }

  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
  }

  .wordmark {
    display: inline-flex;
    align-items: baseline;
    font-weight: 700;
    font-size: 1.05rem;
    letter-spacing: 0.04em;
    text-decoration: none;
    color: inherit;
  }

  .wordmark .prompt {
    color: var(--accent);
    margin-right: 0.35rem;
    font-weight: 400;
  }

  /* The block cursor after the wordmark. It blinks — slowly, and not at all
     under reduced motion, where it stays lit. */
  .wordmark .cursor {
    color: var(--accent-2);
    margin-left: 0.1rem;
    animation: blink 1.4s step-end infinite;
  }

  .live {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
    font-size: 0.72em;
    color: var(--muted);
  }

  /* Streaming: the dot breathes. Down: it holds still and the label warns. */
  .live:not(.down) .dot.good {
    animation: pulse 2.6s ease-in-out infinite;
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
    font-size: 0.68rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.09em;
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
    gap: 0.45rem;
    padding: 0.28rem 0.5rem;
    border-radius: 6px;
    font-size: 0.9rem;
    text-decoration: none;
    color: inherit;
    transition: background-color 120ms ease;
  }

  .corpus:hover {
    background: var(--raised);
  }

  .corpus.active {
    background: color-mix(in srgb, var(--accent) 9%, var(--raised));
    box-shadow: inset 2px 0 0 var(--accent);
  }

  .corpus.active .name {
    font-weight: 600;
  }

  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .primary {
    font-size: 0.62em;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    border: 1px solid color-mix(in srgb, var(--accent-2) 45%, var(--border));
    border-radius: 3px;
    padding: 0 0.25rem;
    color: var(--accent-2);
  }

  .count {
    margin-left: auto;
    font-family: var(--font-mono);
    font-size: 0.75em;
    font-variant-numeric: tabular-nums;
  }

  .union {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
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
    font-size: 0.9em;
  }

  /* The bar. Everything but the header folds away until the toggle opens it;
     the footer (version, last load) is desktop furniture and stays hidden.
     Last in the sheet on purpose: these override rules of equal specificity
     above, and the cascade settles ties by order. */
  @media (max-width: 46rem) {
    .sidebar {
      position: static;
      min-height: 0;
      max-height: none;
      padding: 0.5rem 0.8rem;
      gap: 0.6rem;
      border-right: none;
      border-bottom: 1px solid var(--border);
    }

    .sidebar:not(.open) .list,
    footer {
      display: none;
    }

    .sidebar.open .list {
      display: block;
      padding-bottom: 0.4rem;
    }

    .menu {
      display: inline-flex;
      align-items: center;
      gap: 0.35rem;
      min-height: 1.9rem;
      padding: 0.15rem 0.5rem;
      font-size: 0.8rem;
      color: var(--muted);
      cursor: pointer;
    }

    .menu .mono {
      font-size: 0.75rem;
      font-variant-numeric: tabular-nums;
    }

    .live {
      margin-left: auto;
    }
  }
</style>
