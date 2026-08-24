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
  import Query from './Query.svelte';
  import Sidebar from './Sidebar.svelte';
  import Union from './Union.svelte';
  import { corpora } from './lib/corpora.svelte.js';
  import { events } from './lib/events.svelte.js';
  import { corpusLabel } from './lib/format.js';
  import { notice } from './lib/notice.svelte.js';
  import { boardPath, href, nav } from './lib/router.svelte.js';

  const route = $derived(nav.route);

  // Effects return their teardown, so the socket closes and the subscription
  // drops if the app is ever unmounted (a dev-server hot reload, mostly).
  $effect(() => events.start());
  $effect(() => corpora.start());

  /** Every corpus the node serves, flattened — the landing page's shortcut. */
  const served = $derived(corpora.groups.flatMap((group) => group.corpora));
</script>

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
    {:else if route.view === 'home'}
      <h1>opys</h1>
      {#if corpora.empty}
        <div class="notice">
          <p>This node is not serving any projects.</p>
          <p class="why">
            A node serves an explicit allowlist and nothing else — there is no way
            to add a project over HTTP, by design. List the projects you want in
            <code>~/.config/opys/server.toml</code>, or start the node with
            <code>opys-server run --config &lt;path&gt;</code>, and they appear here
            without a restart.
          </p>
        </div>
      {:else if served.length > 0}
        <p class="lede">
          {served.length}
          {served.length === 1 ? 'corpus' : 'corpora'} served. Pick one on the left,
          or start here:
        </p>
        <ul class="jump">
          {#each served as corpus (corpus.cid)}
            <li>
              <a href={href(boardPath(corpus.cid))}>{corpusLabel(corpus)}</a>
              <span class="muted small mono">{corpus.root}</span>
            </li>
          {/each}
        </ul>
      {:else if corpora.error}
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

<style>
  h1 {
    margin: 0 0 0.75rem;
    font-size: 1.4rem;
    letter-spacing: 0.01em;
  }

  .lede {
    margin: 0 0 1rem;
  }

  .jump {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 0.4rem;
  }

  .jump li {
    display: flex;
    gap: 0.6rem;
    align-items: baseline;
    flex-wrap: wrap;
  }
</style>
