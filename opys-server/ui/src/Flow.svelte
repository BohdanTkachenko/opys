<script>
  // View 6: one project's lifecycle, drawn as the graph its config declares.
  //
  // The board draws a column per status because that is the shape the corpus
  // has on disk. This draws the shape `opys.toml` describes — a node per
  // distinct status name, an edge wherever some type declares the move — and
  // puts the documents standing on each node on it. Nothing here is about
  // agents; it is the board rearranged into the picture the transitions make.
  //
  // Four things about the payload are worth knowing before reading the markup:
  //
  //  1. **A node is a status name, merged across types.** `task`, `bug` and
  //     `chore` all declare `todo` and share one node. `node.types` says who,
  //     and the guards stay per type — nothing is evaluated here.
  //  2. **The items are `opys next`'s answer, verbatim.** Same function, same
  //     order, so this view and `opys next --status <s>` cannot disagree; the
  //     order is `order::Corpus::ranked`'s, which groups by type declaration
  //     order first and then by each type's `order_by` chain. So a merged node
  //     reads task-block, bug-block, chore-block rather than one interleaved
  //     priority run — exactly as the CLI prints it.
  //  3. **Blocked documents are counted, never listed.** They are not pickable,
  //     and a card offering one would be offering work nobody can take. The
  //     chip names them on hover.
  //  4. **A back edge is a real edge.** The layout ({@link layout}) breaks
  //     cycles so the drawing can be left-to-right at all, and hands back which
  //     edges it had to turn around. They are drawn as returns, dashed and
  //     below, rather than dropped — a lifecycle without its returns is a lie.
  //
  // The edge geometry is measured from the rendered cards rather than computed
  // from the grid: node cards have wildly different heights (one lists five
  // items, its neighbour none), so anything that assumed a cell size would draw
  // lines into the gaps between them.

  import Icon from './lib/Icon.svelte';
  import { api } from './lib/api.js';
  import { corpora } from './lib/corpora.svelte.js';
  import { affects, affectsProject, events } from './lib/events.svelte.js';
  import { layout } from './lib/flow.js';
  import { priorityChip, statusTone, typeTone } from './lib/format.js';
  import { createResource } from './lib/resource.svelte.js';
  import { docPath, href } from './lib/router.svelte.js';

  let { key } = $props();

  const graph = createResource();

  $effect(() => {
    // Read `key` into a local first: that is what subscribes the effect to it.
    // Reading it inside the async callback instead would subscribe to nothing
    // and leave the graph showing the previous project.
    const wanted = key;
    graph.run(() => api.flow(wanted));
  });

  $effect(() => {
    const wanted = key;
    return events.subscribe((batch) => {
      const cid = graph.data?.cid;
      // Two frames matter and they are matched by different functions.
      // `corpus-reloaded` carries the cid this answer came from; `refs-changed`
      // carries only a project key and is invisible to `affects` by design. The
      // second cannot change *this* payload today — occupancy is not drawn here
      // — but it is the frame that says the project's git moved, and a view
      // that ignored it would be the one place in the app that does.
      if (affectsProject(batch, wanted) || !cid || affects(batch, cid)) {
        graph.run(() => api.flow(wanted), { quiet: true });
      }
    });
  });

  const group = $derived(corpora.groups.find((candidate) => candidate.key === key));
  const view = $derived(graph.data);

  const placed = $derived(layout(view?.nodes ?? [], view?.edges ?? []));
  /** status → the node payload, so the grid can render in placement order. */
  const byStatus = $derived(new Map((view?.nodes ?? []).map((node) => [node.status, node])));
  const documents = $derived((view?.nodes ?? []).reduce((n, node) => n + node.total, 0));

  // --- edge geometry -------------------------------------------------------

  /** The positioned container, and one element per node card. */
  let canvas = $state(null);
  let cards = $state({});
  /** `{d, back}` per drawable edge, in container coordinates. */
  let wires = $state([]);
  let size = $state({ width: 0, height: 0 });

  function boxes() {
    if (!canvas) return null;
    const base = canvas.getBoundingClientRect();
    const found = new Map();
    for (const [status, element] of Object.entries(cards)) {
      if (!element) continue;
      const box = element.getBoundingClientRect();
      found.set(status, {
        x: box.left - base.left,
        y: box.top - base.top,
        w: box.width,
        h: box.height,
      });
    }
    return { base, found };
  }

  /**
   * A forward edge leaves the right of its source and enters the left of its
   * target; a return leaves the left and comes back to the right, bowing below
   * both cards so it never crosses the row it belongs to.
   */
  function wire(a, b, back) {
    if (a === b) {
      const x = a.x + a.w / 2;
      return `M ${x - 16} ${a.y} C ${x - 26} ${a.y - 30}, ${x + 26} ${a.y - 30}, ${x + 16} ${a.y}`;
    }
    if (!back) {
      const x1 = a.x + a.w;
      const y1 = a.y + a.h / 2;
      const x2 = b.x;
      const y2 = b.y + b.h / 2;
      const bend = Math.max(26, (x2 - x1) / 2);
      return `M ${x1} ${y1} C ${x1 + bend} ${y1}, ${x2 - bend} ${y2}, ${x2} ${y2}`;
    }
    const x1 = a.x;
    const y1 = a.y + a.h / 2;
    const x2 = b.x + b.w;
    const y2 = b.y + b.h / 2;
    const dip = Math.max(a.y + a.h, b.y + b.h) + 26;
    return `M ${x1} ${y1} C ${x1 - 34} ${dip}, ${x2 + 34} ${dip}, ${x2} ${y2}`;
  }

  function measure() {
    const measured = boxes();
    if (!measured) return;
    const { base, found } = measured;
    size = { width: base.width, height: base.height };
    wires = placed.edges
      .map((edge) => {
        const a = found.get(edge.from);
        const b = found.get(edge.to);
        if (!a || !b) return null;
        return { d: wire(a, b, edge.back), back: edge.back };
      })
      .filter(Boolean);
  }

  $effect(() => {
    // Depend on the placement *and* on the bound elements: a refresh that brings
    // different items re-renders every card, and a line drawn to where a card
    // used to be is worse than no line at all. `Object.values` is what
    // subscribes to the card set — reading the state object alone would not.
    void placed;
    const elements = Object.values(cards).filter(Boolean);
    if (!canvas) return;
    measure();
    // Anything that moves a card without changing the data above: a window
    // resize, the sidebar folding away on a narrow screen, a title rewrapping.
    // Observing the container alone would miss the last of those.
    const observer = new ResizeObserver(() => measure());
    observer.observe(canvas);
    for (const element of elements) observer.observe(element);
    return () => observer.disconnect();
  });

  /** What a node's tooltip says about who declares it. */
  function declaredBy(node) {
    if (!node.declared) return 'no type declares this status — `opys verify` reports it';
    return `declared by ${node.types.join(', ')}`;
  }

  /**
   * A node's heading. The one status that has no name of its own is the blank
   * one — `status: ""`, which `opys verify` reports as `invalid status ''` —
   * and the node sends it as an ordinary undeclared node rather than dropping
   * the documents on it. Rendering the empty string would draw a heading that
   * is just a count, so it gets a label here, where labels live.
   */
  function statusLabel(status) {
    return status.length > 0 ? status : '(no status)';
  }
</script>

<header class="head topbar">
  <div class="title">
    <h1>{group ? group.name : key}</h1>
    {#if view}
      <span class="small muted">
        {placed.nodes.length}
        {placed.nodes.length === 1 ? 'status' : 'statuses'} · {view.edges.length}
        {view.edges.length === 1 ? 'transition' : 'transitions'} · {documents}
        {documents === 1 ? 'document' : 'documents'}
        {#if graph.loading}· refreshing…{/if}
      </span>
    {/if}
  </div>
</header>

{#if graph.error && !graph.data}
  <div class="notice bad">
    <p>{graph.error.message}</p>
    <p class="why">
      {#if graph.error.status === 404}
        No project group with that key is served. Group keys are derived from the
        repository, not from a corpus id — a link built from a cid lands here.
      {:else if graph.error.offline}
        The node is not answering.
      {:else}
        The node could not build this view.
      {/if}
    </p>
    <p>
      <button class="btn small" onclick={() => graph.run(() => api.flow(key))}>Try again</button>
    </p>
  </div>
{:else if !graph.settled}
  <p class="muted">Loading the lifecycle…</p>
{:else if view}
  {#if graph.error}
    <div class="notice warn"><p>Could not refresh: {graph.error.message}</p></div>
  {/if}

  {#if placed.nodes.length === 0}
    <div class="notice">
      <p>This project declares no statuses.</p>
      <p class="why">
        Every node here comes from a type's <code>statuses</code> list in
        <code>opys.toml</code>; there is nothing to draw until one declares some.
      </p>
    </div>
  {:else}
    <p class="types small muted">
      {#each view.types as type (type.name)}
        {@const tone = typeTone(type.name)}
        <span
          class="chip"
          style:--tone={tone}
          class:neutral={tone === null}
          title={type.has_graph
            ? `${type.prefix} · ${type.statuses.length} statuses, with a declared transition graph`
            : `${type.prefix} · ${type.statuses.length} statuses and no transitions map — any status may follow any other, so it draws no edges`}
        >
          {type.name}{#if !type.has_graph}<span class="loose"> ⋯</span>{/if}
        </span>
      {/each}
      <span class="note">
        listing the top {view.limit} of each status, in <code>opys next</code> order
      </span>
    </p>

    <div class="scroll">
      <div
        class="canvas"
        bind:this={canvas}
        style:--layers={placed.layers}
        style:--rows={placed.rows}
      >
        <!-- Under the cards, and inert: the cards are the links. Drawn from
             measured positions, so it survives cards of different heights. -->
        <svg
          class="wires"
          width={size.width}
          height={size.height}
          viewBox={`0 0 ${Math.max(size.width, 1)} ${Math.max(size.height, 1)}`}
          aria-hidden="true"
        >
          <defs>
            <marker
              id="flow-arrow"
              viewBox="0 0 8 8"
              refX="7"
              refY="4"
              markerWidth="6"
              markerHeight="6"
              orient="auto-start-reverse"
            >
              <path d="M0 0 L8 4 L0 8 z" fill="currentColor" />
            </marker>
          </defs>
          {#each wires as edge, index (index)}
            <path class="wire" class:back={edge.back} d={edge.d} marker-end="url(#flow-arrow)" />
          {/each}
        </svg>

        {#each placed.nodes as spot (spot.status)}
          {@const node = byStatus.get(spot.status)}
          {@const tone = statusTone(spot.status)}
          <section
            class="node panel"
            class:neutral={tone === null}
            class:terminal={node.terminal}
            class:undeclared={!node.declared}
            style:--tone={tone}
            style:grid-column={spot.layer + 1}
            style:grid-row={spot.row + 1}
            bind:this={cards[spot.status]}
          >
            <h2 title={declaredBy(node)}>
              <span class="status mono" class:nameless={spot.status.length === 0}
                >{statusLabel(spot.status)}</span
              >
              {#if node.initial}<span class="pill" title="where a new document starts">start</span>
              {/if}
              {#if node.terminal}
                <!-- Always empty, and marked so it does not read as a stage
                     nobody reached: `close` deletes the file, and `set-status`
                     refuses a terminal target. -->
                <span class="pill" title="reached by `opys close`, which deletes the document">
                  end
                </span>
              {/if}
              {#if !node.declared}
                <span class="pill bad" title="no type declares this status">undeclared</span>
              {/if}
              <span class="count mono" title="documents at this status">{node.total}</span>
            </h2>

            {#if node.types.length > 0}
              <p class="owners small muted">{node.types.join(' · ')}</p>
            {/if}

            {#if node.items.length > 0}
              <ul>
                {#each node.items as item (item.id)}
                  {@const itemTone = typeTone(item.type)}
                  {@const chip = priorityChip(item.priority, item.effective_priority)}
                  <li>
                    <a
                      class="item"
                      class:neutral={itemTone === null}
                      style:--tone={itemTone}
                      href={href(docPath(view.cid, item.id))}
                    >
                      <span class="idrow">
                        <span class="id mono small">{item.id}</span>
                        {#if chip}
                          <!-- The number that *ordered* the item, which is the
                               effective one: `next::pick` ranks by the smallest
                               priority over the document and everything it
                               still blocks, so drawing the document's own here
                               would contradict the order it is drawn in. The
                               qualifier is the CLI's (`30`, `1 (own 30)`,
                               `1 (inherited)`), and the guard is on the chip so
                               a document that inherits its whole rank still
                               shows one. -->
                          <span class="prio mono" title={chip.title}>
                            <Icon name="flag" size={9} />{chip.rank}{#if chip.note}<span
                                class="own">({chip.note})</span
                              >{/if}
                          </span>
                        {/if}
                      </span>
                      <span class="item-title">{item.title}</span>
                    </a>
                  </li>
                {/each}
              </ul>
              {#if node.pickable > node.items.length}
                <p class="more small muted">
                  and {node.pickable - node.items.length} more pickable
                </p>
              {/if}
            {:else if node.pickable === 0 && node.total > 0}
              <p class="more small muted">nothing here is pickable</p>
            {:else if node.total === 0}
              <p class="more small muted">empty</p>
            {/if}

            {#if node.blocked.length > 0 || node.orphaned.length > 0}
              <p class="marks">
                {#if node.blocked.length > 0}
                  <!-- Counted, never listed: a blocked document is not pickable,
                       and a node offering it would be offering work nobody can
                       take. The ids are on the chip so the count is checkable. -->
                  <span
                    class="mark blocked"
                    title={`blocked by a live blocker: ${node.blocked.join(', ')}`}
                  >
                    <Icon name="lock" size={10} />
                    {node.blocked.length} blocked
                  </span>
                {/if}
                {#if node.orphaned.length > 0}
                  <span
                    class="mark orphaned"
                    title={node.orphaned
                      .map((orphan) => `${orphan.id}: ${orphan.reason}`)
                      .join('\n')}
                  >
                    <Icon name="alert" size={10} />
                    {node.orphaned.length} stranded
                  </span>
                {/if}
              </p>
            {/if}
          </section>
        {/each}
      </div>
    </div>

    <dl class="legend small muted">
      <div>
        <dt><span class="swatch forward"></span> transition</dt>
        <dd>
          a move some type's <code>transitions</code> map declares. A type that declares
          none contributes its statuses and no edges — any status may follow any other,
          which is not the same as every edge
        </dd>
      </div>
      <div>
        <dt><span class="swatch return"></span> return</dt>
        <dd>
          the same kind of edge, drawn coming back: a left-to-right picture has to
          break the cycles, and hiding the edges it broke would hide half the lifecycle
        </dd>
      </div>
      <div>
        <dt><span class="mark blocked static">blocked</span></dt>
        <dd>
          at this status with a live blocker, so not pickable — counted and named on
          hover, never listed
        </dd>
      </div>
      <div>
        <dt><span class="mark orphaned static">stranded</span></dt>
        <dd>
          the links its type requires no longer hold (the parent shipped or was
          abandoned), so it will never become pickable on its own
        </dd>
      </div>
      <div>
        <dt>items</dt>
        <dd>
          exactly what <code>opys next --status &lt;status&gt;</code> answers over this
          project's main worktree, in that order
        </dd>
      </div>
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

  .types {
    display: flex;
    gap: 0.3rem;
    align-items: center;
    flex-wrap: wrap;
    margin: 0 0 0.7rem;
  }

  .types .loose {
    opacity: 0.7;
  }

  .types .note {
    margin-left: 0.3rem;
  }

  .scroll {
    overflow-x: auto;
    max-width: 100%;
    padding-bottom: 0.4rem;
  }

  /* One column per layer and one row per row the layout assigned. Columns are
     a fixed minimum so the picture reads as a flow rather than reflowing into
     a list; the container scrolls sideways instead. */
  .canvas {
    position: relative;
    display: grid;
    grid-template-columns: repeat(var(--layers), minmax(14rem, 1fr));
    grid-auto-rows: min-content;
    gap: 2.6rem 3.4rem;
    align-items: start;
    min-width: min-content;
    padding: 0.2rem 0.2rem 1.6rem;
  }

  .wires {
    position: absolute;
    inset: 0;
    pointer-events: none;
    overflow: visible;
    color: var(--border);
  }

  .wire {
    fill: none;
    stroke: currentColor;
    stroke-width: 1.5;
  }

  /* A return is the same relation drawn the other way; dashed and dimmer so the
     forward spine of the graph stays the thing you read first. */
  .wire.back {
    stroke-dasharray: 4 4;
    opacity: 0.55;
  }

  .node {
    position: relative;
    z-index: 1;
    display: grid;
    gap: 0.35rem;
    padding: 0.55rem 0.65rem 0.6rem;
    /* The status's hue on the leading edge, the same annotation the board's
       column rail carries. Neutral — a retired-family word, or one this app has
       never heard of — gets grey furniture rather than an invented colour. */
    border-left: 3px solid hsl(var(--tone) 70% 55%);
  }

  .node.neutral {
    border-left-color: var(--unknown);
  }

  /* A terminus: the last thing on its path, and always empty. */
  .node.terminal {
    border-style: dashed;
    border-left-style: solid;
  }

  .node.undeclared {
    border-left-color: var(--bad);
  }

  h2 {
    display: flex;
    align-items: center;
    gap: 0.3rem;
    margin: 0;
    font-size: 0.85rem;
    font-weight: 600;
    flex-wrap: wrap;
  }

  .status {
    color: hsl(var(--tone) 45% var(--tone-text));
    overflow-wrap: anywhere;
  }

  .node.neutral .status {
    color: var(--muted);
  }

  /* The blank status has no name to set in mono — the label stands in for one. */
  .status.nameless {
    font-style: italic;
    opacity: 0.8;
  }

  .count {
    margin-left: auto;
    font-size: 0.78rem;
    font-variant-numeric: tabular-nums;
    color: var(--muted);
  }

  .pill {
    font-size: 0.62rem;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    border: 1px solid var(--border);
    border-radius: 3px;
    padding: 0 0.25rem;
    color: var(--muted);
    font-weight: 500;
  }

  .pill.bad {
    border-color: var(--bad);
    color: var(--bad);
  }

  .owners {
    margin: -0.2rem 0 0;
    overflow-wrap: anywhere;
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 0.25rem;
  }

  .item {
    display: grid;
    gap: 0.1rem;
    padding: 0.28rem 0.4rem;
    border-radius: 5px;
    border: 1px solid var(--border);
    border-left: 2px solid hsl(var(--tone) 65% 55%);
    background: var(--raised);
    text-decoration: none;
    color: inherit;
    transition:
      border-color 120ms ease,
      transform 120ms ease;
  }

  .item.neutral {
    border-left-color: var(--unknown);
  }

  .item:hover {
    border-color: color-mix(in srgb, var(--accent) 60%, var(--border));
    transform: translateX(1px);
  }

  .idrow {
    display: flex;
    align-items: center;
    gap: 0.3rem;
  }

  .id {
    color: hsl(var(--tone) 60% var(--tone-text));
    letter-spacing: 0.02em;
  }

  .item.neutral .id {
    color: var(--muted);
  }

  .prio {
    display: inline-flex;
    align-items: center;
    gap: 0.1rem;
    margin-left: auto;
    font-size: 0.7rem;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
  }

  /* The qualifier beside an inherited rank — quieter than the number, because
     the number is the one that ordered the item. */
  .prio .own {
    margin-left: 0.15rem;
    opacity: 0.75;
  }

  .item-title {
    font-size: 0.82rem;
    line-height: 1.25;
    overflow-wrap: anywhere;
  }

  .more {
    margin: 0;
  }

  .marks {
    display: flex;
    gap: 0.25rem;
    flex-wrap: wrap;
    margin: 0.1rem 0 0;
  }

  .mark {
    display: inline-flex;
    align-items: center;
    gap: 0.2rem;
    font-size: 0.72rem;
    border: 1px solid var(--border);
    border-radius: 3px;
    padding: 0 0.3rem;
    white-space: nowrap;
    color: var(--muted);
  }

  /* Blocked work is greyed, not coloured: it is normal, temporary and nobody's
     fault. Stranded work is a warning — it will not clear on its own. */
  .mark.orphaned {
    border-color: var(--warn);
    color: var(--warn);
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

  .legend .mark.static {
    font-weight: 400;
  }

  .swatch {
    display: inline-block;
    width: 1.1rem;
    height: 0;
    border-top: 1.5px solid var(--border);
  }

  .swatch.return {
    border-top-style: dashed;
    opacity: 0.6;
  }
</style>
