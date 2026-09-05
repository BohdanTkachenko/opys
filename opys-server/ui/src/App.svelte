<script>
  // The app shell: a sidebar, a routed main column, and the two things that are
  // global to every view — the event stream and the project list.
  //
  // Both are started here and only here. The stream is one WebSocket for the
  // whole page (the node offers no per-corpus subscription), and the project
  // list is the most expensive read the node serves; starting either inside a
  // view would open a second one every time someone navigated.

  import Board from './Board.svelte';
  import Doc from './Doc.svelte';
  import Omni from './Omni.svelte';
  import Query from './Query.svelte';
  import Setup from './Setup.svelte';
  import Sidebar from './Sidebar.svelte';
  import Union from './Union.svelte';
  import { corpora } from './lib/corpora.svelte.js';
  import { events } from './lib/events.svelte.js';
  import { corpusLabel, middlePath } from './lib/format.js';
  import { notice } from './lib/notice.svelte.js';
  import { omni } from './lib/omni.svelte.js';
  import { boardPath, href, nav } from './lib/router.svelte.js';

  const route = $derived(nav.route);

  /**
   * The global shortcuts: Ctrl/⌘+P and `/` open the omnibox from any view,
   * scoped to the corpus on screen. Ctrl+P is the browser's print key, so it
   * is always claimed — even while the box is open, where it doubles as
   * "up" (the fzf habit) — or a slip would print the dashboard.
   */
  function onkeydown(event) {
    const mod = event.ctrlKey || event.metaKey;
    if (mod && !event.altKey && !event.shiftKey && event.key.toLowerCase() === 'p') {
      event.preventDefault();
      if (!omni.open) omni.show(route.cid ?? null);
      return;
    }
    if (omni.open || event.key !== '/' || mod || event.altKey) return;
    const target = event.target;
    if (target instanceof HTMLElement && target.closest('input, textarea, select, [contenteditable]')) return;
    event.preventDefault();
    omni.show(route.cid ?? null);
  }

  // Effects return their teardown, so the socket closes and the subscription
  // drops if the app is ever unmounted (a dev-server hot reload, mostly).
  $effect(() => events.start());
  $effect(() => corpora.start());

  /** Every corpus the node serves, flattened — the landing page's shortcut. */
  const served = $derived(corpora.groups.flatMap((group) => group.corpora));
</script>

<svelte:window {onkeydown} />

<div class="shell">
  <Sidebar activeCid={route.cid ?? null} activeKey={route.key ?? null} />

  <main>
    {#if notice.message}
      <!-- Raised by a view that has since navigated away (a `close` whose sync
           pass was skipped), so it lives here rather than in any one view. -->
      <div class="notice warn">
        <p>{notice.message}</p>
        <p class="why">
          The write landed, but the sync pass did not run — relation maps, prose
          links and file locations are not being maintained until that is fixed.
          Run <code>opys verify</code> in the project.
        </p>
        <p><button class="btn small" onclick={() => notice.clear()}>Dismiss</button></p>
      </div>
    {/if}

    {#if route.view === 'board'}
      <!-- Keyed like the views below, and for the same reason: the resource
           keeps its last answer while the next one is in flight, so an unkeyed
           board would render one corpus's documents under another's name —
           with links built from the new cid — for as long as `/docs` takes. -->
      {#key route.cid}
        <Board cid={route.cid} filters={route.query} />
      {/key}
    {:else if route.view === 'doc'}
      <!-- Keyed, so moving between documents resets the view's local state:
           a half-typed tag, an open close-confirmation, an inline refusal from
           the last write. Without this they would follow the reader around. -->
      {#key `${route.cid}/${route.id}`}
        <Doc cid={route.cid} id={route.id} />
      {/key}
    {:else if route.view === 'query'}
      <!-- Keyed for the same reason the document view is: the console's text is
           a statement about *this* corpus's tables, and carrying it — or the
           result table it produced — to another corpus would be showing one
           corpus's rows under another's name. -->
      {#key route.cid}
        <Query cid={route.cid} />
      {/key}
    {:else if route.view === 'union'}
      {#key route.key}
        <Union key={route.key} filters={route.query} />
      {/key}
    {:else if route.view === 'setup'}
      <Setup />
    {:else if route.view === 'home'}
      {#if corpora.empty}
        <!-- A node serving nothing has exactly one thing worth showing, so the
             setup screen *is* the landing page rather than a link to one. It
             introduces itself when no allowlist file exists yet, and is the
             plain management panel once one does. -->
        <Setup />
      {:else if served.length > 0}
        <div class="home">
        <div class="hero">
          <h1 class="mono">
            <span class="prompt" aria-hidden="true">❯</span><span class="grad-text">opys</span>
          </h1>
          <p class="lede muted">
            {served.length}
            {served.length === 1 ? 'corpus' : 'corpora'} served — every document a
            markdown file, every write through the engine.
          </p>
        </div>
        <ul class="jump">
          {#each served as corpus (corpus.cid)}
            <li>
              <a class="panel jumpcard" href={href(boardPath(corpus.cid))}>
                <span class="jumphead">
                  <span
                    class="dot"
                    class:good={!('error' in corpus) && corpus.verify_problems === 0}
                    class:bad={'error' in corpus ||
                      (corpus.verify_problems ?? 0) > 0}
                  ></span>
                  <span class="jumpname">{corpusLabel(corpus)}</span>
                  {#if corpus.doc_count !== null && corpus.doc_count !== undefined}
                    <span class="muted small mono jumpcount">
                      {corpus.doc_count}
                      {corpus.doc_count === 1 ? 'doc' : 'docs'}
                    </span>
                  {/if}
                </span>
                <span class="muted small mono jumppath" title={corpus.root}>{middlePath(corpus.root, 40)}</span>
              </a>
            </li>
          {/each}
        </ul>
        </div>
      {:else if corpora.error}
        <h1>opys</h1>
        <!-- Without this the column below claims a load is in progress that
             will never finish: `empty` requires a successful read, and a failed
             one leaves `served` empty forever. -->
        <div class="notice bad">
          <p>{corpora.error.message}</p>
          <p class="why">
            {#if corpora.error.offline}
              The node is not answering. It may have stopped, or this page may
              have been left open past a restart.
            {:else}
              The node answered, but not with the project list.
            {/if}
          </p>
          <p><button class="btn small" onclick={() => corpora.reload()}>Try again</button></p>
        </div>
      {:else}
        <p class="lede muted">Loading projects…</p>
      {/if}
    {:else}
      <h1>No such view</h1>
      <div class="notice bad">
        <p>Nothing is routed at <code>{route.path}</code>.</p>
        <p class="why">
          Links in this UI are fragments, so a stale bookmark lands here rather
          than at the node's JSON 404. <a href="#/">Start again</a>.
        </p>
      </div>
    {/if}
  </main>
</div>

<Omni />

<style>
  .home {
    max-width: 58rem;
    margin-inline: auto;
  }

  .hero {
    padding: 1.5rem 0 0.5rem;
  }

  h1 {
    margin: 0 0 0.5rem;
    font-size: 2.8rem;
    letter-spacing: 0.01em;
    line-height: 1.1;
  }

  h1 .prompt {
    color: var(--accent);
    font-weight: 400;
    margin-right: 0.4rem;
    font-size: 0.7em;
    vertical-align: 0.12em;
  }

  .lede {
    margin: 0 0 1.5rem;
    max-width: 34rem;
  }

  .jumphead {
    display: flex;
    align-items: center;
    gap: 0.45rem;
  }

  .jumpcount {
    margin-left: auto;
  }

  .jump {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(17rem, 1fr));
    gap: 0.6rem;
    max-width: 56rem;
  }

  .jumpcard {
    display: grid;
    gap: 0.2rem;
    padding: 0.7rem 0.8rem;
    text-decoration: none;
    color: inherit;
    transition:
      border-color 120ms ease,
      transform 120ms ease,
      box-shadow 120ms ease;
  }

  .jumpcard:hover {
    border-color: color-mix(in srgb, var(--accent) 60%, var(--border));
    transform: translateY(-1px);
    box-shadow: 0 4px 16px color-mix(in srgb, var(--accent) calc(10% * var(--glow)), transparent);
  }

  .jumpname {
    font-weight: 600;
  }

  .jumppath {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
