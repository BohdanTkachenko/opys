<script>
  // The omnibox: a fuzzy finder over tickets (FEAT-0098), opened from the
  // header's placeholder, Ctrl/⌘+P, or `/`.
  //
  // Scope is the corpus on screen, or every served corpus when there is none —
  // the home page and the union view have no single corpus, and a jump from
  // there is still a jump. Documents are fetched when the box opens rather
  // than kept warm: a corpus is hundreds of documents, one request, and a box
  // that opens on a keystroke must show the corpus as it is now.
  //
  // Enter opens the selected ticket. On a board the last row applies the text
  // as the board's filter (`?q=`) instead — what the toolbar's search field
  // used to do. The box replaced that field, so it keeps the ability.

  import { tick } from 'svelte';
  import Icon from './lib/Icon.svelte';
  import { api } from './lib/api.js';
  import { corpora } from './lib/corpora.svelte.js';
  import { corpusLabel, relativeTime, statusTone, typeTone } from './lib/format.js';
  import { rankDocs, segments } from './lib/fuzzy.js';
  import { omni } from './lib/omni.svelte.js';
  import { boardPath, docPath, go, nav } from './lib/router.svelte.js';

  let query = $state('');
  let index = $state(0);
  /** `{cid, doc}` for every document in scope. */
  let entries = $state([]);
  let loading = $state(false);
  let error = $state(null);
  let inputEl = $state(null);
  let listEl = $state(null);

  const served = $derived(corpora.groups.flatMap((group) => group.corpora));
  const multi = $derived(omni.cid === null);
  const labels = $derived(new Map(served.map((c) => [c.cid, corpusLabel(c)])));
  const scopeText = $derived(
    multi
      ? `${served.length} ${served.length === 1 ? 'corpus' : 'corpora'}`
      : (labels.get(omni.cid) ?? omni.cid),
  );

  $effect(() => {
    if (!omni.open) return;
    query = '';
    index = 0;
    load(omni.cid);
    tick().then(() => inputEl?.focus());
  });

  async function load(cid) {
    const cids = cid ? [cid] : served.map((c) => c.cid);
    loading = true;
    error = null;
    try {
      const lists = await Promise.all(
        cids.map(async (c) => (await api.docs(c)).map((doc) => ({ cid: c, doc }))),
      );
      entries = lists.flat();
    } catch (cause) {
      error = cause;
      entries = [];
    } finally {
      loading = false;
    }
  }

  const hits = $derived(rankDocs(query, entries).slice(0, 12));
  const onBoard = $derived(nav.route.view === 'board' && nav.route.cid === omni.cid);
  const rows = $derived.by(() => {
    const list = hits.map((hit) => ({ kind: 'doc', key: `${hit.cid}/${hit.doc.id}`, ...hit }));
    if (onBoard && query.trim().length > 0) list.push({ kind: 'filter', key: 'filter' });
    return list;
  });
  /** The selected row, kept inside the list as it shrinks under typing. */
  const at = $derived(Math.min(index, Math.max(0, rows.length - 1)));

  $effect(() => {
    void query;
    index = 0;
  });

  $effect(() => {
    listEl?.children[at]?.scrollIntoView?.({ block: 'nearest' });
  });

  function keys(event) {
    const down = event.key === 'ArrowDown' || (event.ctrlKey && event.key === 'n');
    const up = event.key === 'ArrowUp' || (event.ctrlKey && event.key === 'p');
    if (down) {
      event.preventDefault();
      index = Math.min(rows.length - 1, at + 1);
    } else if (up) {
      event.preventDefault();
      index = Math.max(0, at - 1);
    } else if (event.key === 'Enter') {
      event.preventDefault();
      pick(rows[at]);
    } else if (event.key === 'Escape') {
      event.preventDefault();
      omni.hide();
    }
  }

  function pick(row) {
    if (!row) return;
    if (row.kind === 'filter') {
      go(boardPath(omni.cid, { ...nav.route.query, q: query.trim() }));
    } else {
      go(docPath(row.cid, row.doc.id));
    }
    omni.hide();
  }
</script>

{#if omni.open}
  <!-- Clicking outside closes; a button, so it needs no key handler of its
       own — Escape on the input is the keyboard path. -->
  <button type="button" class="scrim" aria-label="close" tabindex="-1" onclick={() => omni.hide()}></button>
  <div class="omni panel" role="dialog" aria-modal="true" aria-label="Jump to a ticket">
    <label class="bar">
      <Icon name="search" size={16} />
      <input
        bind:this={inputEl}
        bind:value={query}
        onkeydown={keys}
        placeholder={`Jump to a ticket in ${scopeText}…`}
        autocomplete="off"
        autocapitalize="off"
        spellcheck="false"
        role="combobox"
        aria-expanded="true"
        aria-controls="omni-results"
        aria-activedescendant={rows[at] ? `omni-${at}` : undefined}
        aria-label="Jump to a ticket"
      />
      <kbd>esc</kbd>
    </label>

    <ul class="results" id="omni-results" role="listbox" bind:this={listEl}>
      {#each rows as row, i (row.key)}
        <li id={`omni-${i}`} role="option" aria-selected={i === at} class:active={i === at}>
          <button type="button" class="row" tabindex="-1" onmousemove={() => (index = i)} onclick={() => pick(row)}>
            {#if row.kind === 'filter'}
              <span class="id mono muted"><Icon name="board" size={13} /></span>
              <span class="title">Filter the board by “{query.trim()}”</span>
              <span class="meta muted small">every card whose id, title, tag or path has it</span>
            {:else}
              {@const tone = typeTone(row.doc.type)}
              {@const stone = statusTone(row.doc.status)}
              <span class="id mono" style:--tone={tone} class:neutral={tone === null}>
                {#each segments(row.doc.id, row.idPos) as s}{#if s.hit}<mark>{s.text}</mark>{:else}{s.text}{/if}{/each}
              </span>
              <span class="title">
                {#each segments(row.doc.title ?? '', row.titlePos) as s}{#if s.hit}<mark>{s.text}</mark>{:else}{s.text}{/if}{/each}
              </span>
              <span class="meta">
                {#if multi}<span class="chip">{labels.get(row.cid) ?? row.cid}</span>{/if}
                {#if row.doc.status}
                  <span class="chip status" style:--tone={stone} class:neutral={stone === null}>{row.doc.status}</span>
                {/if}
                {#if row.doc.updated}<span class="when muted small">{relativeTime(row.doc.updated)}</span>{/if}
              </span>
            {/if}
          </button>
        </li>
      {/each}
    </ul>

    {#if loading}
      <p class="state muted small">loading {scopeText}…</p>
    {:else if error}
      <p class="state small"><span class="bad">{error.message}</span></p>
    {:else if rows.length === 0}
      <p class="state muted small">{query.trim() ? 'no ticket matches' : 'nothing here yet'}</p>
    {/if}

    <footer class="hint small muted">
      <span><kbd>↑</kbd><kbd>↓</kbd> move</span>
      <span><kbd>↵</kbd> open</span>
      <span><kbd>esc</kbd> close</span>
      <span class="scope mono">{scopeText}</span>
    </footer>
  </div>
{/if}

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 49;
    border: none;
    border-radius: 0;
    margin: 0;
    padding: 0;
    background: rgba(4, 8, 14, 0.5);
    cursor: default;
  }

  @supports (backdrop-filter: blur(1px)) {
    .scrim {
      backdrop-filter: blur(2px);
      -webkit-backdrop-filter: blur(2px);
    }
  }

  .omni {
    position: fixed;
    z-index: 50;
    top: 12vh;
    left: 50%;
    transform: translateX(-50%);
    width: min(42rem, 94vw);
    max-height: 72vh;
    display: grid;
    grid-template-rows: auto minmax(0, 1fr) auto auto;
    overflow: hidden;
    box-shadow:
      var(--shadow-2),
      0 0 48px color-mix(in srgb, var(--accent) calc(16% * var(--glow)), transparent);
  }

  .bar {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    padding: 0.55rem 0.85rem;
    border-bottom: 1px solid var(--border);
    color: var(--muted);
  }

  .bar input {
    flex: 1;
    min-width: 0;
    border: none;
    background: none;
    box-shadow: none;
    padding: 0;
    min-height: 0;
    font-size: 1.05rem;
    color: var(--fg);
  }

  .bar input:focus-visible {
    outline: none;
    box-shadow: none;
    border: none;
  }

  .results {
    list-style: none;
    margin: 0;
    padding: 0.3rem;
    overflow-y: auto;
  }

  .row {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    align-items: center;
    gap: 0.6rem;
    width: 100%;
    min-height: 0;
    padding: 0.42rem 0.6rem;
    border: none;
    border-radius: 7px;
    background: none;
    text-align: left;
    cursor: pointer;
    font-size: 0.925rem;
  }

  .active .row {
    background: color-mix(in srgb, var(--accent) 12%, var(--raised));
    box-shadow: inset 2px 0 0 var(--accent);
  }

  .id {
    font-size: 0.8em;
    letter-spacing: 0.02em;
    color: hsl(var(--tone) 60% var(--tone-text));
  }

  .id.neutral {
    color: var(--muted);
  }

  .title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .meta {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    white-space: nowrap;
  }

  mark {
    background: color-mix(in srgb, var(--accent) 28%, transparent);
    color: inherit;
    border-radius: 2px;
  }

  .state {
    margin: 0;
    padding: 0.6rem 0.85rem;
  }

  .bad {
    color: var(--bad);
  }

  .hint {
    display: flex;
    gap: 0.9rem;
    align-items: center;
    padding: 0.45rem 0.85rem;
    border-top: 1px solid var(--border);
    background: var(--raised);
  }

  .hint .scope {
    margin-left: auto;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  @media (max-width: 46rem) {
    .omni {
      top: 3vh;
      width: 96vw;
      max-height: 86vh;
    }

    .when,
    .hint span:not(.scope) {
      display: none;
    }
  }
</style>
