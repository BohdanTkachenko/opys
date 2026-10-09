<script>
  // A block-level markdown editor.
  //
  // The document is shown rendered, one top-level block at a time. A double
  // click turns one block into its markdown source, in place; leaving the
  // block puts it back, rendered. The source string is the only state: an
  // edit splices the block's new text over its old range (see `blocks.js`),
  // so nothing the reader did not touch is ever rewritten — no reformatted
  // lists, no escaped punctuation, no dropped HTML comments.
  //
  // Rendering is the host's: `render` turns block sources into HTML, so the
  // editor shows exactly what the host's own renderer shows (and ships none).
  // Persisting is the host's too: `onsave` is called when the reader leaves
  // the editor with changes, and after a checkbox is ticked.

  import { onDestroy, tick, untrack } from 'svelte';
  import { insertBlock, segment, splice, toggleTask } from './blocks.js';
  import { renderCache } from './render.svelte.js';
  import { locate, textOffset } from './selection.js';

  /**
   * @type {{
   *   value?: string,
   *   render: (sources: string[]) => string[] | Promise<string[]>,
   *   renderFrontmatter?: (block: string) => string,
   *   readonly?: boolean,
   *   placeholder?: string,
   *   onsave?: (value: string) => unknown,
   *   onchange?: (value: string) => void,
   *   toolbar?: import('svelte').Snippet<[{ text: string, start: number | null, end: number | null, close: () => void }]>,
   *   class?: string,
   * }}
   */
  let {
    value = $bindable(''),
    render,
    renderFrontmatter,
    readonly = false,
    placeholder = 'Double-click to write…',
    onsave,
    onchange,
    toolbar,
    class: className = '',
  } = $props();

  let source = $state(untrack(() => value));
  /** The last value the host has (handed to `onsave`, or passed in). */
  let saved = untrack(() => value);
  /** A value the host refused, so leaving again does not resend it. */
  let refused = null;

  /**
   * The open edit: the source range being edited and where its box sits.
   * `insert` is a new block at offset `start` (= `end`), drawn before block
   * `slot`; otherwise the box replaces block `slot`.
   * @type {{ start: number, end: number, slot: number, insert: boolean } | null}
   */
  let edit = $state(null);
  let draft = $state('');
  /** @type {HTMLTextAreaElement | null} */
  let area = $state(null);
  /** @type {HTMLDivElement} */
  let root;
  /** Set while one box hands over to the next, so the hand-over is not "leaving". */
  let switching = false;

  const blocks = $derived(segment(source));

  // A new value from the host (a reload after save, a live update elsewhere)
  // replaces the document — unless a block is open: typing is never clobbered.
  $effect(() => {
    const next = value;
    untrack(() => {
      if (edit === null && next !== source) {
        source = next;
        saved = next;
        refused = null;
      }
    });
  });

  // Rendered HTML per block, from the host's `render` (frontmatter from
  // `renderFrontmatter`, or the package's own table).
  const cache = renderCache(
    () => render,
    () => renderFrontmatter,
    (markup) => (readonly ? markup : enableTasks(markup)),
  );
  $effect(() => {
    cache.want(blocks.map((b) => ({ text: source.slice(b.start, b.end), kind: b.kind })));
  });

  /** Renderers emit task checkboxes `disabled`; here a click toggles them. */
  function enableTasks(markup) {
    return markup.replace(/(<input\b[^>]*type="checkbox"[^>]*?)\s+disabled(?:="[^"]*")?/g, '$1');
  }

  /** A committed edit: tell a `bind:value` and `onchange`. */
  function changed() {
    value = source;
    onchange?.(source);
  }

  /** Commit the open box into the source; returns the edited range after. */
  function commit() {
    if (!edit) return null;
    const { start, end, insert } = edit;
    const text = draft.replace(/\s+$/, '');
    const before = source;
    let range;
    if (insert) {
      source = insertBlock(source, start, text);
      const at = start === 0 ? 0 : start + 2;
      range = { start: at, end: at + text.length };
    } else {
      source = splice(source, start, end, text);
      range = { start, end: start + text.length };
    }
    edit = null;
    if (source !== before) changed();
    return range;
  }

  async function focusBox(caret) {
    await tick();
    if (!area) return;
    autosize();
    area.focus();
    const at = caret === 'end' ? area.value.length : Math.max(0, Math.min(caret ?? 0, area.value.length));
    area.setSelectionRange(at, at);
  }

  /** Open block `i` for editing, the caret at `caret` (an offset in the block). */
  async function open(i, caret = 0) {
    if (readonly || i < 0 || i >= blocks.length) return;
    const b = blocks[i];
    // The rendered block being replaced may hold focus; its removal is a
    // hand-over, not the reader leaving.
    switching = true;
    draft = source.slice(b.start, b.end);
    edit = { start: b.start, end: b.end, slot: i, insert: false };
    await focusBox(caret);
    switching = false;
  }

  /** Open an empty box for a new block after source offset `at`, drawn before `slot`. */
  async function openNew(at, slot) {
    if (readonly) return;
    switching = true;
    draft = '';
    edit = { start: at, end: at, slot, insert: true };
    await focusBox(0);
    switching = false;
  }

  /**
   * Open the first visible block — the keyboard way in, for a host's "Edit"
   * button. Visible, because a host may hide a block (a title it shows
   * elsewhere) and opening an invisible box would be a trap.
   */
  export function startEditing() {
    const els = root?.querySelectorAll('[data-block]') ?? [];
    for (const el of els) {
      if (el.getClientRects().length > 0) return open(Number(el.getAttribute('data-block')), 'end');
    }
    return blocks.length === 0 ? openNew(0, 0) : open(0, 'end');
  }

  /** Close the open box and hand the document to the host if it changed. */
  async function leave() {
    commit();
    if (source === saved || source === refused || !onsave) return;
    const value = source;
    const ok = await onsave(value);
    if (ok === false) refused = value;
    else {
      saved = value;
      refused = null;
    }
  }

  function autosize() {
    if (!area) return;
    area.style.height = 'auto';
    area.style.height = `${area.scrollHeight}px`;
  }

  async function keys(event) {
    if (!area || !edit) return;
    const { selectionStart: s, selectionEnd: e, value: v } = area;
    const collapsed = s === e;

    if (event.key === 'Escape' || ((event.metaKey || event.ctrlKey) && event.key === 'Enter')) {
      event.preventDefault();
      area.blur();
      return;
    }

    // Up off the first line, down off the last: into the neighbouring block.
    if (event.key === 'ArrowUp' && collapsed && !v.slice(0, s).includes('\n')) {
      if (before(edit.start) < 0) return;
      event.preventDefault();
      switching = true;
      const range = commit();
      await open(before(range.start), 'end');
      switching = false;
      return;
    }
    if (event.key === 'ArrowDown' && collapsed && !v.slice(e).includes('\n')) {
      if (!blocks.some((b) => b.start >= edit.end)) return;
      event.preventDefault();
      switching = true;
      const range = commit();
      await open(blocks.findIndex((b) => b.start >= range.end), 0);
      switching = false;
      return;
    }

    // Enter on an empty last line (or anywhere in a one-line heading): end
    // this block and start a new one after it.
    const heading = !edit.insert && blocks[edit.slot]?.kind === 'heading' && !v.includes('\n');
    if (event.key === 'Enter' && !event.shiftKey && collapsed && s === v.length && (v.endsWith('\n') || heading)) {
      event.preventDefault();
      switching = true;
      draft = v.replace(/\n+$/, '');
      const range = commit();
      await openNew(range.end, before(range.end) + 1);
      switching = false;
      return;
    }

    // Backspace at the very start: join this block onto the one above.
    if (event.key === 'Backspace' && collapsed && s === 0) {
      const prevIdx = before(edit.start);
      if (prevIdx < 0) return;
      event.preventDefault();
      switching = true;
      const range = commit();
      const prev = blocks[before(range.start)];
      const cur = range.end > range.start ? blocks.find((b) => b.start === range.start) : undefined;
      const head = source.slice(prev.start, prev.end);
      const tail = cur ? source.slice(cur.start, cur.end) : '';
      draft = tail ? `${head}\n${tail}` : head;
      edit = { start: prev.start, end: cur ? cur.end : prev.end, slot: blocks.indexOf(prev), insert: false };
      await focusBox(head.length);
      switching = false;
    }
  }

  /** The index of the last block that ends at or before `offset` (-1: none). */
  function before(offset) {
    let n = -1;
    blocks.forEach((b, i) => {
      if (b.end <= offset) n = i;
    });
    return n;
  }

  function onfocusout(event) {
    if (switching) return;
    const next = event.relatedTarget;
    if (next instanceof Node && root.contains(next)) return;
    // The window lost focus (another app, devtools): not the reader leaving.
    if (!document.hasFocus()) return;
    leave();
  }

  /** A single click elsewhere in the document closes the open box. */
  function onblockclick(event, i) {
    const box = event.target instanceof Element ? event.target.closest('input[type="checkbox"]') : null;
    if (box && !readonly) {
      event.preventDefault();
      toggle(i, box);
      return;
    }
    if (edit) commit();
  }

  /** Tick the clicked checkbox in the source and save straight away. */
  function toggle(i, box) {
    const el = root.querySelector(`[data-block="${i}"]`);
    const n = [...(el?.querySelectorAll('input[type="checkbox"]') ?? [])].indexOf(box);
    const b = blocks[i];
    if (!b || n < 0) return;
    const text = source.slice(b.start, b.end);
    const next = toggleTask(text, n);
    if (next === text) return;
    commit();
    const nb = blocks[i];
    const prior = source;
    source = splice(source, nb.start, nb.end, next);
    changed();
    // A tick is one complete action: if the host refuses it, untick it
    // rather than leave a box showing a state that was never saved.
    leave().then(() => {
      if (refused === source) {
        source = prior;
        refused = null;
        changed();
      }
    });
  }

  async function ondblclick(event, i) {
    if (readonly) return;
    if (event.target instanceof Element && event.target.closest('a, input, button')) return;
    // Put the caret on the word that was double-clicked, then drop the
    // selection the gesture made: it was the gesture, not a selection.
    let caret = 0;
    const sel = window.getSelection?.();
    const el = event.currentTarget;
    const b = blocks[i];
    if (sel && sel.rangeCount > 0 && b) {
      const range = sel.getRangeAt(0);
      const found = locate(
        source.slice(b.start, b.end),
        el.textContent ?? '',
        sel.toString(),
        textOffset(el, range.startContainer, range.startOffset),
      );
      if (found) caret = found.start;
    }
    sel?.removeAllRanges();
    if (edit) commit();
    await open(i, caret);
  }

  // The selection toolbar: over a selection inside one rendered block, the
  // host's `toolbar` snippet, told the selected text and its source range.
  /** @type {{ text: string, start: number | null, end: number | null, top: number, left: number } | null} */
  let picked = $state(null);

  function onmouseup() {
    if (!toolbar || edit) return;
    // After the browser has settled the selection for this mouseup.
    setTimeout(pick, 0);
  }

  function pick() {
    const sel = window.getSelection?.();
    if (!sel || sel.isCollapsed || sel.rangeCount === 0) {
      picked = null;
      return;
    }
    const range = sel.getRangeAt(0);
    const at = (node) => (node instanceof Element ? node : node?.parentElement)?.closest('[data-block]');
    const el = at(range.startContainer);
    if (!el || el !== at(range.endContainer) || !root.contains(el)) {
      picked = null;
      return;
    }
    const i = Number(el.getAttribute('data-block'));
    const b = blocks[i];
    const text = sel.toString();
    const found = b
      ? locate(source.slice(b.start, b.end), el.textContent ?? '', text, textOffset(el, range.startContainer, range.startOffset))
      : null;
    const rect = range.getBoundingClientRect();
    const box = root.getBoundingClientRect();
    picked = {
      text,
      start: found ? b.start + found.start : null,
      end: found ? b.start + found.end : null,
      top: rect.top - box.top,
      left: rect.left - box.left + rect.width / 2,
    };
  }

  function onselectionchange() {
    if (picked && window.getSelection?.()?.isCollapsed) picked = null;
  }

  $effect(() => {
    document.addEventListener('selectionchange', onselectionchange);
    return () => document.removeEventListener('selectionchange', onselectionchange);
  });

  // Navigating away with a block open, or with unsaved changes, still saves:
  // a change made is a change kept.
  onDestroy(() => {
    if (edit || source !== saved) leave();
  });
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="blockdown {className}" bind:this={root} {onfocusout} onmouseup={onmouseup}>
  {#if blocks.length === 0 && !edit}
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <div class="bd-empty" tabindex="-1" ondblclick={() => openNew(0, 0)}>{readonly ? '' : placeholder}</div>
  {/if}

  {#each blocks as block, i (i)}
    {#if edit?.insert && edit.slot === i}
      {@render box()}
    {/if}
    {#if edit && !edit.insert && edit.slot === i}
      {@render box()}
    {:else}
      {@const text = source.slice(block.start, block.end)}
      <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_click_events_have_key_events -->
      <div
        class="bd-block bd-{block.kind}"
        data-block={i}
        tabindex="-1"
        onclick={(e) => onblockclick(e, i)}
        ondblclick={(e) => ondblclick(e, i)}
      >
        {#if text in cache.html}
          {@html cache.html[text]}
        {:else}
          <div class="bd-pending">{text}</div>
        {/if}
      </div>
    {/if}
  {/each}
  {#if edit?.insert && edit.slot >= blocks.length}
    {@render box()}
  {/if}

  {#if picked && toolbar}
    <div class="bd-toolbar" style:top="{picked.top}px" style:left="{picked.left}px">
      {@render toolbar({ text: picked.text, start: picked.start, end: picked.end, close: () => (picked = null) })}
    </div>
  {/if}
</div>

{#snippet box()}
  <textarea
    class="bd-source"
    bind:this={area}
    bind:value={draft}
    oninput={autosize}
    onkeydown={keys}
    rows="1"
    spellcheck="false"
    autocapitalize="off"
    aria-label="Markdown source of this block"
  ></textarea>
{/snippet}

<style>
  .blockdown {
    position: relative;
  }

  .bd-block {
    border-radius: var(--bd-radius, 4px);
    outline: none;
  }

  .bd-block:hover {
    background: var(--bd-hover, transparent);
  }

  /* A rendered block is a <div> around the host's HTML: its children keep
     their own margins, which would otherwise collapse through the wrapper
     differently from the unwrapped document. Flow-root keeps them inside. */
  .bd-block {
    display: flow-root;
  }

  /* Each list item is its own block, so its own <ul>/<ol>: close the gap a
     list's margins would open between items of what reads as one list. */
  .bd-list + .bd-list > :global(:is(ul, ol)) {
    margin-top: 0;
  }

  .bd-list:has(+ .bd-list) > :global(:is(ul, ol)) {
    margin-bottom: 0;
  }

  /* The default frontmatter rendering: a quiet key/value table. */
  .blockdown :global(.bd-frontmatter) {
    font-size: 0.85em;
    border-collapse: collapse;
    margin: 0 0 1em;
  }

  .blockdown :global(table.bd-frontmatter th) {
    text-align: left;
    font-weight: 600;
    padding: 0.1em 1em 0.1em 0;
    vertical-align: top;
    opacity: 0.7;
  }

  .blockdown :global(table.bd-frontmatter td) {
    white-space: pre-wrap;
    font-family: var(--bd-source-font, ui-monospace, monospace);
    padding: 0.1em 0;
  }

  .bd-pending {
    white-space: pre-wrap;
    opacity: 0.7;
  }

  .bd-empty {
    color: var(--bd-muted, GrayText);
    padding: 0.5em 0;
    cursor: text;
    outline: none;
  }

  .bd-source {
    display: block;
    width: 100%;
    box-sizing: border-box;
    margin: 0.4em 0;
    padding: var(--bd-source-padding, 0.35em 0.5em);
    font: inherit;
    font-family: var(--bd-source-font, ui-monospace, SFMono-Regular, Menlo, Consolas, monospace);
    font-size: var(--bd-source-size, 0.92em);
    line-height: 1.55;
    color: inherit;
    background: var(--bd-source-bg, transparent);
    border: 1px solid var(--bd-source-border, currentColor);
    border-radius: var(--bd-radius, 4px);
    resize: none;
    overflow: hidden;
    white-space: pre-wrap;
    tab-size: 4;
  }

  .bd-source:focus {
    outline: none;
    border-color: var(--bd-accent, Highlight);
  }

  .bd-toolbar {
    position: absolute;
    z-index: 10;
    transform: translate(-50%, calc(-100% - 6px));
  }
</style>
