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

  import Icon from './lib/Icon.svelte';
  import { api } from './lib/api.js';
  import { corpora } from './lib/corpora.svelte.js';
  import { affects, events } from './lib/events.svelte.js';
  import {
    corpusLabel,
    middlePath,
    relativeTime,
    shortTime,
    statusSettled,
    statusTone,
    typeTone,
  } from './lib/format.js';
  import { MOD, omni } from './lib/omni.svelte.js';
  import { createResource } from './lib/resource.svelte.js';
  import { boardPath, docPath, go, href, queryPath } from './lib/router.svelte.js';

  let { cid, filters = {} } = $props();

  // Keyboard (FEAT-0097). The cursor is real focus on a card: arrow keys move
  // it between columns and cards, Home/End to a column's ends, PageUp/PageDown
  // to the previous/next project, and Enter is the link's own. Focus rather
  // than a selection this component keeps for itself, so Tab, screen readers
  // and Enter all agree about what is selected, and a reload that keeps the
  // card keeps the cursor. (Ctrl/⌘+P and `/` are the shell's: they open the
  // omnibox from every view.)
  let boardEl = $state(null);
  /** Where the cursor was last, so the keys resume there after focus left. */
  let cursor = { col: 0, row: 0 };

  function remember(event) {
    const card = event.target?.closest?.('.card');
    const column = card?.closest('.column');
    if (!card || !column || !boardEl) return;
    cursor = {
      col: [...boardEl.querySelectorAll('.column')].indexOf(column),
      row: [...column.querySelectorAll('.card')].indexOf(card),
    };
  }

  function typing(target) {
    return target instanceof HTMLElement && target.closest('input, textarea, select, [contenteditable]');
  }

  function onwindowkeydown(event) {
    if (omni.open || event.metaKey || event.ctrlKey || event.altKey || typing(event.target)) return;
    switch (event.key) {
      case 'ArrowLeft':
        step(event, -1, 0);
        break;
      case 'ArrowRight':
        step(event, 1, 0);
        break;
      case 'ArrowUp':
        step(event, 0, -1);
        break;
      case 'ArrowDown':
        step(event, 0, 1);
        break;
      case 'Home':
        step(event, 0, -Infinity);
        break;
      case 'End':
        step(event, 0, Infinity);
        break;
      case 'PageUp':
        page(event, -1);
        break;
      case 'PageDown':
        page(event, 1);
        break;
      default:
    }
  }

  const clamp = (n, lo, hi) => Math.max(lo, Math.min(hi, n));

  /** Move the cursor by `dc` columns and `dr` rows (±Infinity: a column's ends). */
  function step(event, dc, dr) {
    const cols = boardEl ? [...boardEl.querySelectorAll('.column')] : [];
    if (cols.length === 0) return;
    event.preventDefault();
    // A card has focus: move from it. None does: the first press only *shows*
    // the cursor, where it last was, rather than moving away from a card
    // nobody could see was selected.
    const held = document.activeElement?.closest?.('.board .card');
    if (held) remember({ target: held });
    let { col, row } = cursor;
    col = clamp(col + (held ? dc : 0), 0, cols.length - 1);
    const cards = [...cols[col].querySelectorAll('.card')];
    if (cards.length === 0) return;
    if (dr === -Infinity) row = 0;
    else if (dr === Infinity) row = cards.length - 1;
    else row = clamp(row + (held ? dr : 0), 0, cards.length - 1);
    cursor = { col, row };
    cards[row].focus({ preventScroll: true });
    cards[row].scrollIntoView({ block: 'nearest', inline: 'nearest' });
  }

  /** The previous/next project, in the sidebar's order. */
  function page(event, dir) {
    event.preventDefault();
    const served = corpora.groups.flatMap((group) => group.corpora);
    const next = served[served.findIndex((c) => c.cid === cid) + dir];
    if (next) go(boardPath(next.cid));
  }

  const docs = createResource();

  /**
   * The clock the board renders against. It ticks once a minute so the
   * relative times age and the heat ring cools between reloads, instead of
   * every card freezing at whatever the clock said when it was drawn.
   */
  let now = $state(Date.now());

  $effect(() => {
    const timer = setInterval(() => {
      now = Date.now();
    }, 60_000);
    return () => clearInterval(timer);
  });

  /**
   * How recently a document was written, as 0..1 "heat".
   *
   * The signal is `updated` (mtime-backed), which every write through the
   * engine stamps — deliberately not file *access* times, which relatime and
   * noatime mounts make a lie on most machines. Combined with the live event
   * stream reloading the board, a working agent leaves a visible warm trail:
   * the cards it writes glow, then cool over the day.
   */
  function heat(updated) {
    if (!updated) return 0;
    const then = new Date(updated).getTime();
    if (Number.isNaN(then)) return 0;
    // Steep, so the board points at what just happened rather than washing
    // amber over everything written today.
    const minutes = (now - then) / 60000;
    if (minutes < 10) return 1;
    if (minutes < 45) return 0.55;
    if (minutes < 240) return 0.16;
    if (minutes < 1440) return 0.06;
    return 0;
  }

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

  // The text filter lives in the route with type and tag (`?q=`), so a
  // narrowed board survives a click into a document and back. It is set from
  // the omnibox's last row — the box replaced the toolbar's search field, and
  // this is that field's other job.
  const needle = $derived(String(filters.q ?? '').trim().toLowerCase());

  function matches(doc) {
    if (filters.type && doc.type !== filters.type) return false;
    if (filters.tag && !doc.tags.includes(filters.tag)) return false;
    if (needle.length === 0) return true;
    const haystack = [doc.id, doc.title, doc.status, doc.path, ...doc.tags].join(' ').toLowerCase();
    return haystack.includes(needle);
  }

  const shown = $derived(all.filter(matches));

  // Focus mode: columns whose status names *settled* work (the done family,
  // the retired family — see `statusSettled`) fold into a one-line strip.
  // Opt-in since the columns became height-bounded: every column scrolls
  // internally, so showing everything no longer costs a wall of page — the
  // fold is for wanting settled work *out of sight*, not out of the way.
  // Persisted per corpus; a text search suspends it, because the grep reflex
  // must always search everything.
  function readFocus(corpusId) {
    try {
      return localStorage.getItem(`opys:board:focus:${corpusId}`) === '1';
    } catch {
      return false;
    }
  }

  let focus = $state(false);

  // The effect is the one place the persisted choice is read: it follows the
  // corpus, which a `$state(readFocus(cid))` seed would capture only once.
  $effect(() => {
    focus = readFocus(cid);
  });

  function setFocus(on) {
    focus = on;
    try {
      localStorage.setItem(`opys:board:focus:${cid}`, on ? '1' : '0');
    } catch {
      // Private browsing; the toggle still works for this visit.
    }
  }

  const focusActive = $derived(focus && needle.length === 0);

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
  const allColumns = $derived.by(() => {
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
        items: items.sort(byPriority),
      }));
  });

  /**
   * Column order: priority first (lower = higher, ADR-0095 — the top card is
   * the one to take next), unprioritised last, ids as the stable tiebreak.
   */
  function byPriority(a, b) {
    const pa = a.priority ?? Infinity;
    const pb = b.priority ?? Infinity;
    if (pa !== pb) return pa - pb;
    return a.id.localeCompare(b.id, undefined, { numeric: true });
  }

  // Drag and drop. Two gestures on the same drag: dropping on *another*
  // column is a status change (the `set-status` action, with every write-time
  // rule that implies), dropping within the *same* column rewrites the card's
  // `priority` field (the verify-gated `set-field` action). Both go through
  // the node's closed vocabulary — a refusal is shown verbatim, and on a
  // corpus whose type never declared `priority`, the first reorder's refusal
  // is the message that says exactly what to add to opys.toml.
  /** The card in the air: `{ id, from }`, or null. */
  let drag = $state(null);
  /** The hovered column's status while dragging. */
  let dropCol = $state(null);
  /** Insertion index within the source column (reorders only, else -1). */
  let dropIndex = $state(-1);
  /** The doc id whose write is in flight — the one-at-a-time latch. */
  let movePending = $state(null);
  /** The last refusal, shown verbatim above the board. */
  let moveProblem = $state(null);

  function dragStart(event, doc, column) {
    drag = { id: doc.id, from: column.status };
    event.dataTransfer.effectAllowed = 'move';
    event.dataTransfer.setData('text/plain', doc.id);
  }

  function dragEnd() {
    drag = null;
    dropCol = null;
    dropIndex = -1;
  }

  function dragOver(event, column) {
    if (!drag || movePending) return;
    const reorder = column.status === drag.from;
    // The unset-status column is not a drop target: there is no action that
    // writes an empty status.
    if (!reorder && column.status === '') return;
    event.preventDefault();
    event.dataTransfer.dropEffect = 'move';
    dropCol = column.status;
    dropIndex = reorder ? insertionIndex(event, event.currentTarget) : -1;
  }

  function dragLeave(event, column) {
    if (event.currentTarget.contains(event.relatedTarget)) return;
    if (dropCol === column.status) {
      dropCol = null;
      dropIndex = -1;
    }
  }

  /** Where between the column's cards the pointer currently sits. */
  function insertionIndex(event, section) {
    const cards = [...section.querySelectorAll('ul > li')];
    for (let i = 0; i < cards.length; i += 1) {
      const rect = cards[i].getBoundingClientRect();
      if (event.clientY < rect.top + rect.height / 2) return i;
    }
    return cards.length;
  }

  async function dropOn(event, column) {
    event.preventDefault();
    if (!drag || movePending) {
      dragEnd();
      return;
    }
    const { id, from } = drag;
    const at = dropIndex;
    dragEnd();
    if (column.status === from) {
      await reprioritize(column, id, at);
    } else if (column.status !== '') {
      await act({ action: 'set-status', id, status: column.status });
    }
  }

  /** One write, then a quiet reload; a refusal lands in the notice, verbatim. */
  async function act(body) {
    movePending = body.id;
    moveProblem = null;
    try {
      await api.action(cid, body);
      await docs.run(() => api.docs(cid), { quiet: true });
      return true;
    } catch (cause) {
      moveProblem = { message: cause.message, busy: cause.busy ?? false };
      return false;
    } finally {
      movePending = null;
    }
  }

  /**
   * Give `id` the priority its new slot implies.
   *
   * One write in the common case: a midpoint between its new neighbours'
   * priorities, or a step past the end. When the numbers leave no room — or
   * the column has never been prioritised at all — the whole column is
   * renumbered with a gap of 10, one verify-gated write per card, so the next
   * hundred reorders are one write again.
   */
  async function reprioritize(column, id, at) {
    const items = column.items;
    const fromIdx = items.findIndex((d) => d.id === id);
    if (fromIdx < 0 || at < 0) return;
    const rest = items.filter((d) => d.id !== id);
    const to = fromIdx < at ? at - 1 : at;
    if (to === fromIdx) return;
    const above = rest[to - 1]?.priority;
    const below = rest[to]?.priority;
    let value = null;
    if (above != null && below != null && below - above > 1) {
      value = Math.floor((above + below) / 2);
    } else if (above != null && below == null) {
      // Past the last prioritised card — the end of the column, or the
      // boundary into the unprioritised tail (any number sorts before it).
      value = above + 10;
    } else if (above == null && to === 0 && below != null) {
      value = below - 10;
    }
    if (value != null) {
      await act({ action: 'set-field', id, key: 'priority', value: String(value) });
      return;
    }
    const order = [...rest.slice(0, to), items[fromIdx], ...rest.slice(to)];
    movePending = id;
    moveProblem = null;
    try {
      for (let i = 0; i < order.length; i += 1) {
        await api.action(cid, {
          action: 'set-field',
          id: order[i].id,
          key: 'priority',
          value: String((i + 1) * 10),
        });
      }
    } catch (cause) {
      moveProblem = { message: cause.message, busy: cause.busy ?? false };
    } finally {
      movePending = null;
      // Reload either way: some of the renumber may have landed before a
      // refusal, and the board must show what is actually on disk.
      docs.run(() => api.docs(cid), { quiet: true });
    }
  }

  /**
   * Which edges of the sideways scroll still have columns beyond them. The
   * fades they drive are the only thing telling a reader that the cut-off
   * column at the window's edge is not the last one.
   */
  let edges = $state({ left: false, right: false });

  function scrollEdges(node) {
    function update() {
      const beyond = node.scrollWidth - node.clientWidth - node.scrollLeft;
      edges = { left: node.scrollLeft > 2, right: beyond > 2 };
    }
    node.addEventListener('scroll', update, { passive: true });
    // The node's own box (a window resize) and its content (a column added
    // by a status change, or a filter) both move the edges.
    const sizes = new ResizeObserver(update);
    sizes.observe(node);
    const children = new MutationObserver(update);
    children.observe(node, { childList: true });
    update();
    return {
      destroy() {
        node.removeEventListener('scroll', update);
        sizes.disconnect();
        children.disconnect();
      },
    };
  }

  const settledColumns = $derived(allColumns.filter((c) => statusSettled(c.status)));
  const columns = $derived(
    focusActive ? allColumns.filter((c) => !statusSettled(c.status)) : allColumns,
  );
  const hiddenDocs = $derived(
    focusActive ? settledColumns.reduce((n, c) => n + c.items.length, 0) : 0,
  );

  const filtered = $derived(
    Boolean(filters.type) || Boolean(filters.tag) || needle.length > 0,
  );

  /** Filters live in the route, so changing one is a navigation. */
  function setFilter(key, value) {
    go(boardPath(cid, { ...filters, [key]: value }));
  }

  function clearFilters() {
    go(boardPath(cid));
  }
</script>

<svelte:window onkeydown={onwindowkeydown} />

<header class="head topbar">
  <div class="title">
    <h1>{corpus ? corpusLabel(corpus) : cid}</h1>
    {#if corpus}
      <div class="path mono" title={corpus.base}>{middlePath(corpus.base, 52)}</div>
    {/if}
  </div>
  <div class="headside">
    <!-- Looks like a search field, is the omnibox's doorbell: typing happens
         in the box, not here. -->
    <button class="jump" type="button" onclick={() => omni.show(cid)} title={`jump to a ticket (${MOD}+P)`}>
      <Icon name="search" size={14} />
      <span class="jumptext">Jump to a ticket…</span>
      <kbd>{MOD}</kbd><kbd>P</kbd>
    </button>
    {#if docs.data}
      <span class="mono small muted">
        {shown.length === all.length
          ? `${all.length} ${all.length === 1 ? 'doc' : 'docs'}`
          : `${shown.length}/${all.length} docs`}
        {#if docs.loading}· refreshing…{/if}
      </span>
    {/if}
    <!-- The board answers "what is in this corpus"; anything sharper than that
         is a question for SQL, which is one click away rather than a CLI. -->
    <a class="btn small" href={href(queryPath(cid))}>
      <Icon name="terminal" size={13} /> query
    </a>
  </div>
</header>

{#if corpus && (corpus.verify_problems ?? 0) > 0}
  <div class="notice warn">
    <p>
      <strong>{corpus.verify_problems}</strong> verify
      {corpus.verify_problems === 1 ? 'problem' : 'problems'} in this corpus.
    </p>
    <p class="why">
      The board still shows every document that parses. Run
      <code>opys verify</code> in the project for the full list.
    </p>
  </div>
{/if}

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
  <div class="toolbar">
    <select
      aria-label="Filter by type"
      value={filters.type ?? ''}
      onchange={(e) => setFilter('type', e.currentTarget.value)}
    >
      <option value="">all types</option>
      {#each types as type (type)}
        <option value={type}>{type}</option>
      {/each}
    </select>

    <select
      aria-label="Filter by tag"
      value={filters.tag ?? ''}
      onchange={(e) => setFilter('tag', e.currentTarget.value)}
    >
      <option value="">all tags</option>
      {#each tags as tag (tag)}
        <option value={tag}>{tag}</option>
      {/each}
    </select>

    {#if settledColumns.length > 0 || !focus}
      <!-- Pressed = work-in-progress view. The classification is the tone
           engine's own done/retired families, so an unfamiliar status never
           silently disappears; a text search suspends the hiding entirely. -->
      <button
        class="btn small"
        class:on={focus}
        aria-pressed={focus}
        title={focus
          ? 'settled columns (done, archived, …) are folded away — click to show everything'
          : 'fold away settled columns (done, archived, …)'}
        onclick={() => setFocus(!focus)}
      >
        <Icon name="zap" size={12} /> focus
        {#if hiddenDocs > 0}
          <span class="mono foldcount" title={`${hiddenDocs} settled ${hiddenDocs === 1 ? 'document' : 'documents'} folded away`}>
            {hiddenDocs}
          </span>
        {/if}
      </button>
    {/if}

    {#if needle.length > 0}
      <span class="chip matching">
        matching “{filters.q}”
        <button class="x" title="drop the text filter" aria-label="drop the text filter" onclick={() => setFilter('q', '')}>×</button>
      </span>
    {/if}

    {#if filtered}
      <button class="btn small" onclick={clearFilters}>
        <Icon name="x" size={12} /> Clear
      </button>
    {/if}

    <span class="keys mono muted" aria-hidden="true">
      <kbd>←</kbd><kbd>→</kbd><kbd>↑</kbd><kbd>↓</kbd> move · <kbd>↵</kbd> open · <kbd>PgUp</kbd><kbd>PgDn</kbd> project
    </span>
  </div>

  {#if all.length === 0}
    <div class="empty">
      <span class="halo"><Icon name="board" size={22} /></span>
      <h3>No documents yet</h3>
      <p>
        Create one with <code>opys new --type &lt;type&gt; --title …</code> in the
        project, and it appears here on its own.
      </p>
    </div>
  {:else if shown.length === 0}
    <div class="empty">
      <span class="halo"><Icon name="search" size={22} /></span>
      <h3>Nothing matches</h3>
      <p>No document matches these filters.</p>
      <p><button class="btn small" onclick={clearFilters}>Clear filters</button></p>
    </div>
  {:else if columns.length === 0}
    <!-- Everything that matched is settled, and focus folded all of it away.
         An empty grid here would read as "no documents", which is a lie. -->
    <div class="empty">
      <span class="halo"><Icon name="check" size={22} /></span>
      <h3>All settled</h3>
      <p>
        {shown.length === 1
          ? 'The one document here is'
          : `All ${shown.length} documents here are`} in settled statuses, folded away by
        focus mode.
      </p>
      <p><button class="btn small" onclick={() => setFocus(false)}>Show them</button></p>
    </div>
  {:else}
    {#if moveProblem}
      <div class="notice bad">
        <!-- Verbatim: the node's refusals name the rule that was broken — a
             terminal status says "use close", an undeclared priority names the
             exact opys.toml block to add. -->
        <p>{moveProblem.message}</p>
        <p class="why">
          {#if moveProblem.busy}
            Nothing was written. Another <code>opys</code> invocation was holding
            the inventory lock — the same drag will work once it lets go.
          {:else}
            Nothing was written. The corpus refused this change.
          {/if}
        </p>
        <p><button class="btn small" onclick={() => (moveProblem = null)}>Dismiss</button></p>
      </div>
    {/if}

    <div class="boardwrap" class:more-left={edges.left} class:more-right={edges.right}>
    <div class="board" use:scrollEdges bind:this={boardEl} onfocusin={remember}>
      {#each columns as column (column.status)}
        {@const tone = statusTone(column.status)}
        <!-- A drop target, not a control: the cards inside are the links, and
             the document view's status menu is the keyboard path to the same
             write, so the container itself needs no role. -->
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <section
          class="column"
          style:--tone={tone}
          class:neutral={tone === null}
          class:droptarget={dropCol === column.status}
          ondragover={(e) => dragOver(e, column)}
          ondragleave={(e) => dragLeave(e, column)}
          ondrop={(e) => dropOn(e, column)}
        >
          <h2>
            <span class="rail" aria-hidden="true"></span>
            <span class="status mono">{column.label}</span>
            <span class="count mono">{column.items.length}</span>
          </h2>
          <ul class:insertend={dropCol === column.status && dropIndex === column.items.length}>
            {#each column.items as doc, index (doc.id)}
              {@const cardTone = typeTone(doc.type)}
              {@const warmth = heat(doc.updated)}
              <li class:insert={dropCol === column.status && dropIndex === index}>
                <!-- The card wears its *type*: a rail on the left edge and the
                     id's colour. The column already says the status, so within
                     one column type is the only thing telling thirty cards
                     apart — and the id text carries the same fact for anyone
                     who does not read colour. No type chip: it would repeat
                     what the id's prefix already spells out. -->
                <a
                  class="card panel"
                  class:neutral={cardTone === null}
                  class:dragsource={drag?.id === doc.id}
                  class:inflight={movePending === doc.id}
                  style:--tone={cardTone}
                  style:--heat={warmth}
                  href={href(docPath(cid, doc.id))}
                  title={warmth > 0 ? 'recently written' : undefined}
                  draggable="true"
                  ondragstart={(e) => dragStart(e, doc, column)}
                  ondragend={dragEnd}
                >
                  <span class="idrow">
                    <span class="id mono small">{doc.id}</span>
                    {#if doc.priority !== null && doc.priority !== undefined}
                      <!-- The rank that put the card here (ADR-0095): lower is
                           higher, so the number reads with its neighbours,
                           not on its own. -->
                      <span
                        class="prio mono"
                        title={`priority ${doc.priority} — lower is higher; the top card is the one to take next`}
                      >
                        <Icon name="flag" size={9} />{doc.priority}
                      </span>
                    {/if}
                    {#if doc.blocked_by > 0}
                      <span
                        class="badge blocked mono"
                        title={`blocked by ${doc.blocked_by} live ${doc.blocked_by === 1 ? 'document' : 'documents'}`}
                      >
                        <Icon name="lock" size={10} />
                        {doc.blocked_by}
                      </span>
                    {/if}
                    {#if doc.blocks > 0}
                      <span
                        class="badge mono"
                        title={`blocking ${doc.blocks} ${doc.blocks === 1 ? 'document' : 'documents'}`}
                      >
                        blocks {doc.blocks}
                      </span>
                    {/if}
                  </span>
                  <span class="doc-title">{doc.title}</span>
                  <span class="meta">
                    <!-- Unkeyed on purpose: a tag list is not guaranteed unique
                         (nothing in `verify` says so, and a hand edit or a bad
                         merge can repeat one), and a duplicate key throws out of
                         the render — in production builds too, which would blank
                         the whole board over one repeated word. Capped at four:
                         on a busy board a six-tag document would be all chips,
                         and the full list is one click away. -->
                    {#each doc.tags.slice(0, 4) as tag}
                      <span class="chip">{tag}</span>
                    {/each}
                    {#if doc.tags.length > 4}
                      <span class="chip muted">+{doc.tags.length - 4}</span>
                    {/if}
                    {#if doc.updated}
                      <span class="when small muted" title={`updated ${shortTime(doc.updated)}`}>
                        <Icon name="clock" size={11} />
                        {relativeTime(doc.updated, now)}
                      </span>
                    {/if}
                  </span>
                </a>
              </li>
            {/each}
          </ul>
        </section>
      {/each}
    </div>
    </div>

    {#if focusActive && settledColumns.length > 0}
      <!-- What focus folded away, by name and count — hidden work must stay
           one glance (and one click) from visible. -->
      <div class="settled small">
        <span class="muted">settled, folded away:</span>
        {#each settledColumns as column (column.status)}
          {@const tone = statusTone(column.status)}
          <span class="chip status" style:--tone={tone} class:neutral={tone === null}>
            {column.label} <span class="mono">{column.items.length}</span>
          </span>
        {/each}
        <button class="btn small" onclick={() => setFocus(false)}>show</button>
      </div>
    {/if}
  {/if}
{/if}

<style>
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    flex-wrap: wrap;
    margin-bottom: 0.9rem;
  }

  .title {
    display: grid;
    gap: 0.1rem;
    min-width: 0;
  }

  h1 {
    margin: 0;
    font-size: 1.4rem;
    font-weight: 650;
    letter-spacing: -0.015em;
  }

  .path {
    font-size: 0.72rem;
    color: var(--muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .headside {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    flex-wrap: wrap;
  }

  .headside a {
    text-decoration: none;
    color: inherit;
  }

  .toolbar .matching {
    gap: 0.35rem;
    font-size: 0.8rem;
  }

  .toolbar .x {
    border: none;
    background: none;
    padding: 0;
    margin: 0;
    min-height: 0;
    line-height: 1;
    cursor: pointer;
    color: var(--muted);
  }

  .toolbar .x:hover {
    color: var(--bad);
  }

  /* The key legend: at the toolbar's far end, and only where there is a
     keyboard worth the space. */
  .keys {
    margin-left: auto;
    font-size: 0.7rem;
    white-space: nowrap;
  }

  .keys kbd {
    font-size: 0.85em;
    margin-right: 0.1rem;
  }

  @media (max-width: 62rem) {
    .keys {
      display: none;
    }
  }

  /* The focus toggle, pressed: lit in the accent, like an active mode. */
  .toolbar button.on {
    border-color: color-mix(in srgb, var(--accent) 55%, var(--border));
    color: var(--accent);
    background: color-mix(in srgb, var(--accent) 10%, transparent);
  }

  /* How many settled documents the fold is holding — on the toggle itself,
     because the strip that itemises them sits below a possibly-tall board. */
  .foldcount {
    font-size: 0.7rem;
    font-variant-numeric: tabular-nums;
    background: var(--raised);
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 0 0.35rem;
    line-height: 1.5;
  }

  /* The folded-away strip: a footnote under the board, not a second board. */
  .settled {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.4rem;
    padding: 0.55rem 0.25rem 0;
    margin-top: 0.35rem;
    border-top: 1px dashed color-mix(in srgb, var(--border) 70%, transparent);
  }

  .settled .chip .mono {
    color: var(--muted);
    font-variant-numeric: tabular-nums;
  }

  /* The fades at the sideways edges: shown only on an edge with columns
     beyond it (`edges`, kept by the scroll action), so a board that fits says
     nothing and a board that does not says "more this way". Kept clear of the
     scrollbar at the bottom. */
  .boardwrap {
    position: relative;
  }

  .boardwrap::before,
  .boardwrap::after {
    content: '';
    position: absolute;
    top: 0;
    bottom: 1.1rem;
    width: 3.5rem;
    pointer-events: none;
    opacity: 0;
    transition: opacity 160ms ease;
    z-index: 2;
  }

  .boardwrap::before {
    left: 0;
    background: linear-gradient(90deg, var(--bg), transparent);
  }

  .boardwrap::after {
    right: 0;
    background: linear-gradient(270deg, var(--bg), transparent);
  }

  .boardwrap.more-left::before,
  .boardwrap.more-right::after {
    opacity: 1;
  }

  /* Columns scroll sideways rather than squeezing: a board with eight statuses
     should stay readable instead of becoming eight slivers. */
  .board {
    display: grid;
    grid-auto-flow: column;
    /* Capped, not fluid: on an ultrawide monitor 1fr columns become reading-
       hostile planks. Narrow screens still scroll sideways. */
    grid-auto-columns: minmax(16rem, 21rem);
    gap: 0.75rem;
    align-items: start;
    overflow-x: auto;
    padding-bottom: 0.5rem;
  }

  /* Each column is a shallow well sunk into the page — a surface a shade off
     the background, so the cards sitting on it read as raised material. */
  .column {
    --well: color-mix(in srgb, var(--panel) 45%, var(--bg));
    background: var(--well);
    border: 1px solid var(--border);
    border-radius: 12px;
    padding: 0.5rem;
  }

  /* While a card is over it: lit like a socket waiting for a plug. */
  .column.droptarget {
    border-color: color-mix(in srgb, var(--accent) 60%, var(--border));
    box-shadow: 0 0 0 1px color-mix(in srgb, var(--accent) 35%, transparent);
  }

  /* Every column carries its status's tone (`--tone`, a hue): a glowing rail
     square in the header, a toned label, a counted corner. A neutral column —
     no status, or a retired word like `archived` — keeps the furniture grey. */
  .column h2 {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    margin: 0;
    font-size: 0.7rem;
    font-weight: 650;
    text-transform: uppercase;
    letter-spacing: 0.09em;
    padding: 0.3rem 0.35rem 0.5rem;
    border-bottom: 1px solid color-mix(in srgb, var(--border) 55%, transparent);
    margin-bottom: 0.5rem;
  }

  .column .rail {
    flex: none;
    width: 0.5rem;
    height: 0.5rem;
    border-radius: 2px;
    background: hsl(var(--tone) 70% 55%);
    box-shadow: 0 0 7px hsl(var(--tone) 70% 55% / calc(0.65 * var(--glow)));
  }

  .column.neutral .rail {
    background: var(--unknown);
    box-shadow: none;
  }

  .column .status {
    overflow: hidden;
    text-overflow: ellipsis;
    color: hsl(var(--tone) 45% var(--tone-text));
  }

  .column.neutral .status {
    color: var(--muted);
  }

  .column .count {
    margin-left: auto;
    font-size: 0.7rem;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
    background: var(--raised);
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 0 0.4rem;
    line-height: 1.5;
  }

  .column ul {
    list-style: none;
    margin: 0;
    padding: 0.2rem 0.1rem 0.2rem 0;
    display: grid;
    gap: 0.45rem;
    /* Each column scrolls itself: one forty-card column must not stretch the
       whole page (the board scrolls sideways, columns scroll down). Capped to
       roughly the viewport minus the chrome above; the floor keeps short
       windows usable. */
    overflow-y: auto;
    max-height: max(16rem, calc(100vh - 15rem));
    max-height: max(16rem, calc(100dvh - 15rem));
    /* Scroll shadows (the local/scroll attachment trick): a soft edge appears
       only on the side where cards are actually cut off. */
    background:
      linear-gradient(var(--well) 30%, transparent),
      linear-gradient(transparent, var(--well) 70%) 0 100%,
      radial-gradient(farthest-side at 50% 0, rgb(0 0 0 / 0.3), transparent),
      radial-gradient(farthest-side at 50% 100%, rgb(0 0 0 / 0.3), transparent) 0 100%;
    background-repeat: no-repeat;
    background-size:
      100% 28px,
      100% 28px,
      100% 9px,
      100% 9px;
    background-attachment: local, local, scroll, scroll;
  }

  .column li {
    position: relative;
  }

  /* The reorder cursor: an accent line where the card will land. */
  .column li.insert::before {
    content: '';
    position: absolute;
    top: -0.29rem;
    left: 0.1rem;
    right: 0.1rem;
    height: 2px;
    border-radius: 2px;
    background: var(--accent);
    box-shadow: 0 0 6px color-mix(in srgb, var(--accent) calc(70% * var(--glow)), transparent);
  }

  .column ul.insertend::after {
    content: '';
    display: block;
    height: 2px;
    margin: 0 0.1rem;
    border-radius: 2px;
    background: var(--accent);
    box-shadow: 0 0 6px color-mix(in srgb, var(--accent) calc(70% * var(--glow)), transparent);
  }

  /* The card's `--tone` is its *type's* hue (`typeTone`): a rail on the left
     edge, the id in the same colour. Neutral — a chore, or a prefix matching
     no type — gets grey furniture, which on a busy board is itself a signal. */
  .card {
    display: grid;
    gap: 0.25rem;
    padding: 0.5rem 0.6rem 0.5rem 0.7rem;
    font-size: 0.9rem;
    line-height: 1.35;
    /* Raised a shade above the well it sits on, so the board reads as cards
       on a surface instead of outlines on a void. */
    background: color-mix(in srgb, var(--raised) 45%, var(--panel));
    border-left: 3px solid hsl(var(--tone) 60% 50% / 0.6);
    text-decoration: none;
    color: inherit;
    transition:
      border-color 120ms ease,
      transform 120ms ease,
      box-shadow 120ms ease;
  }

  .card.neutral {
    border-left-color: var(--border-strong);
  }

  /* The heat signature: a faint amber ring on recently-written cards that
     cools over the day (`--heat`, 1 → 0). Watching a board while agents work,
     the warm trail is where they are. */
  .card {
    outline: 1px solid hsl(30 95% 58% / calc(var(--heat, 0) * 0.5));
    outline-offset: -1px;
  }

  .card:hover {
    border-color: color-mix(in srgb, var(--accent) 60%, var(--border));
    border-left-color: hsl(var(--tone) 65% 55%);
    transform: translateY(-1px);
    box-shadow:
      var(--shadow-2),
      0 4px 18px color-mix(in srgb, var(--accent) calc(12% * var(--glow)), transparent);
  }

  .card.neutral:hover {
    border-left-color: var(--border-strong);
  }

  /* The keyboard cursor. The ring *is* the selection: the arrow keys move
     real focus, so Enter opens and Tab agrees. The radius restates the
     panel's, which the shared link-focus rule would otherwise shrink. */
  .card:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
    border-radius: 10px;
    box-shadow:
      0 0 0 6px color-mix(in srgb, var(--accent) 18%, transparent),
      var(--shadow-2);
  }

  /* The card being dragged stays as a ghost in its slot; the one whose write
     is in flight dims until the reload answers. */
  .card.dragsource {
    opacity: 0.4;
  }

  .card.inflight {
    opacity: 0.55;
    filter: saturate(0.5);
  }

  .idrow {
    display: flex;
    align-items: center;
    gap: 0.35rem;
  }

  .id {
    color: hsl(var(--tone) 60% var(--tone-text));
    letter-spacing: 0.02em;
  }

  .prio {
    display: inline-flex;
    align-items: center;
    gap: 0.1rem;
    font-size: 0.66rem;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
  }

  /* Relation badges. Blockers are the corpus's load-bearing relation, so the
     blocked badge is the loudest thing on a card; "blocks n" is bookkeeping
     and whispers. */
  .badge {
    display: inline-flex;
    align-items: center;
    gap: 0.2rem;
    font-size: 0.68rem;
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 0 0.3rem;
    color: var(--muted);
    margin-left: auto;
  }

  .badge + .badge {
    margin-left: 0;
  }

  .badge.blocked {
    color: var(--bad);
    border-color: color-mix(in srgb, var(--bad) 55%, var(--border));
    background: color-mix(in srgb, var(--bad) 10%, transparent);
  }

  .card.neutral .id {
    color: var(--muted);
  }

  .doc-title {
    overflow-wrap: anywhere;
    font-weight: 550;
    /* Two lines, then an ellipsis: a five-line title would make its card the
       loudest thing on the board for being the least edited. The full title
       is one click away. */
    display: -webkit-box;
    -webkit-line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }

  .meta {
    display: flex;
    flex-wrap: wrap;
    gap: 0.25rem;
    align-items: center;
    margin-top: 0.1rem;
  }

  .meta .when {
    margin-left: auto;
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    white-space: nowrap;
  }
</style>
