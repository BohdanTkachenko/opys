<script>
  // Two versions of a document, compared block by block (see `diff.js`).
  //
  // Unchanged blocks are drawn rendered, as in the editor; added and removed
  // blocks rendered too, marked. A changed block shows its *source* with a
  // word-level diff: what changed in a paragraph is usually a few words, and
  // two renderings side by side make the reader hunt for them. Long runs of
  // unchanged blocks fold to a line that unfolds on click, keeping `context`
  // blocks around each change.
  //
  // `mode` is 'split' (before | after) or 'inline' (one column). The diff
  // carries its own switch between them (`toggle`, on by default); `mode` is
  // bindable, so a host can remember the reader's choice.

  import { diffBlocks, diffWords } from './diff.js';
  import { renderCache } from './render.svelte.js';
  import { segment } from './blocks.js';

  /**
   * @type {{
   *   before?: string,
   *   after?: string,
   *   render: (sources: string[]) => string[] | Promise<string[]>,
   *   renderFrontmatter?: (block: string) => string,
   *   mode?: 'split' | 'inline',
   *   toggle?: boolean,
   *   context?: number,
   *   class?: string,
   * }}
   */
  let {
    before = '',
    after = '',
    render,
    renderFrontmatter,
    mode = $bindable('split'),
    toggle = true,
    context = 2,
    class: className = '',
  } = $props();

  const ops = $derived(diffBlocks(before, after));

  /** Each block text's kind, so frontmatter is drawn by its own renderer. */
  function kinds(source) {
    const out = {};
    for (const b of segment(source)) out[source.slice(b.start, b.end)] = b.kind;
    return out;
  }
  const kindOf = $derived({ ...kinds(before), ...kinds(after) });

  const cache = renderCache(
    () => render,
    () => renderFrontmatter,
  );
  $effect(() => {
    const wanted = [];
    for (const op of ops) {
      if (op.type === 'same' || op.type === 'added') wanted.push(op.after);
      if (op.type === 'removed') wanted.push(op.before);
    }
    cache.want(wanted.map((text) => ({ text, kind: kindOf[text] ?? 'paragraph' })));
  });

  /** Unchanged runs longer than the context on both sides, by op index. */
  let unfolded = $state(new Set());
  const rows = $derived.by(() => {
    const out = [];
    let k = 0;
    while (k < ops.length) {
      if (ops[k].type !== 'same') {
        out.push({ op: ops[k], index: k });
        k += 1;
        continue;
      }
      let end = k;
      while (end < ops.length && ops[end].type === 'same') end += 1;
      const keepHead = k === 0 ? 0 : context;
      const keepTail = end === ops.length ? 0 : context;
      if (end - k > keepHead + keepTail + 1 && !unfolded.has(k)) {
        for (let n = k; n < k + keepHead; n += 1) out.push({ op: ops[n], index: n });
        out.push({ fold: end - k - keepHead - keepTail, at: k });
        for (let n = end - keepTail; n < end; n += 1) out.push({ op: ops[n], index: n });
      } else {
        for (let n = k; n < end; n += 1) out.push({ op: ops[n], index: n });
      }
      k = end;
    }
    return out;
  });

  const changes = $derived(ops.filter((o) => o.type !== 'same').length);

  function unfold(at) {
    unfolded = new Set([...unfolded, at]);
  }
</script>

{#snippet rendered(text)}
  {#if text in cache.html}
    {@html cache.html[text]}
  {:else}
    <div class="bd-pending">{text}</div>
  {/if}
{/snippet}

{#snippet words(op, side)}
  <pre class="bd-words">{#each diffWords(op.before, op.after) as w, i (i)}{#if w.type === 'same'}{w.text}{:else if w.type === 'del' && side !== 'after'}<del>{w.text}</del>{:else if w.type === 'add' && side !== 'before'}<ins>{w.text}</ins>{/if}{/each}</pre>
{/snippet}

<div class="blockdown-diff bd-{mode} {className}">
  {#if toggle}
    <div class="bd-modes" role="radiogroup" aria-label="Diff layout">
      {#each [['split', 'Side by side'], ['inline', 'Inline']] as [value, label] (value)}
        <button
          type="button"
          role="radio"
          aria-checked={mode === value}
          class:bd-on={mode === value}
          onclick={() => (mode = value)}>{label}</button
        >
      {/each}
    </div>
  {/if}
  {#if changes === 0}
    <p class="bd-nochange">No changes.</p>
  {/if}
  {#each rows as row (row.fold !== undefined ? `fold-${row.at}` : row.index)}
    {#if row.fold !== undefined}
      <button type="button" class="bd-fold" onclick={() => unfold(row.at)}>
        ⋯ {row.fold} unchanged {row.fold === 1 ? 'block' : 'blocks'}
      </button>
    {:else if mode === 'split'}
      {@const op = row.op}
      <div class="bd-row bd-{op.type}">
        <div class="bd-side bd-before">
          {#if op.type === 'same' || op.type === 'removed'}
            {@render rendered(op.before)}
          {:else if op.type === 'changed'}
            {@render words(op, 'before')}
          {/if}
        </div>
        <div class="bd-side bd-after">
          {#if op.type === 'same' || op.type === 'added'}
            {@render rendered(op.after)}
          {:else if op.type === 'changed'}
            {@render words(op, 'after')}
          {/if}
        </div>
      </div>
    {:else}
      {@const op = row.op}
      <div class="bd-row bd-{op.type}">
        {#if op.type === 'same' || op.type === 'added'}
          {@render rendered(op.after)}
        {:else if op.type === 'removed'}
          {@render rendered(op.before)}
        {:else}
          {@render words(op, 'both')}
        {/if}
      </div>
    {/if}
  {/each}
</div>

<style>
  .blockdown-diff {
    display: grid;
    gap: 0.15em;
  }

  .bd-row {
    border-left: 3px solid transparent;
    padding-left: 0.6em;
    display: flow-root;
  }

  .bd-split .bd-row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
    gap: 1.2em;
  }

  .bd-side {
    min-width: 0;
    display: flow-root;
  }

  .bd-same {
    opacity: var(--bd-diff-same-opacity, 0.75);
  }

  .bd-added {
    border-left-color: var(--bd-diff-add, #2da44e);
    background: var(--bd-diff-add-bg, color-mix(in srgb, #2da44e 9%, transparent));
  }

  .bd-removed {
    border-left-color: var(--bd-diff-del, #cf222e);
    background: var(--bd-diff-del-bg, color-mix(in srgb, #cf222e 9%, transparent));
  }

  .bd-removed :global(*) {
    text-decoration: line-through;
    text-decoration-color: color-mix(in srgb, var(--bd-diff-del, #cf222e) 60%, transparent);
  }

  .bd-changed {
    border-left-color: var(--bd-diff-change, #bf8700);
  }

  /* In split mode a whole-block add or remove leaves the other side empty;
     colour only the side that has the block. */
  .bd-split .bd-added,
  .bd-split .bd-removed {
    background: none;
  }

  .bd-split .bd-added .bd-after,
  .bd-split .bd-removed .bd-before {
    background: var(--bd-diff-add-bg, color-mix(in srgb, #2da44e 9%, transparent));
  }

  .bd-split .bd-removed .bd-before {
    background: var(--bd-diff-del-bg, color-mix(in srgb, #cf222e 9%, transparent));
  }

  .bd-words {
    margin: 0.5em 0;
    padding: 0.4em 0.6em;
    font-family: var(--bd-source-font, ui-monospace, SFMono-Regular, Menlo, Consolas, monospace);
    font-size: var(--bd-source-size, 0.92em);
    line-height: 1.55;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    background: var(--bd-source-bg, transparent);
    border-radius: var(--bd-radius, 4px);
  }

  .bd-words :global(ins) {
    text-decoration: none;
    background: var(--bd-diff-add-word, color-mix(in srgb, #2da44e 28%, transparent));
    border-radius: 2px;
  }

  .bd-words :global(del) {
    background: var(--bd-diff-del-word, color-mix(in srgb, #cf222e 26%, transparent));
    border-radius: 2px;
  }

  .bd-modes {
    justify-self: end;
    display: inline-flex;
    margin-bottom: 0.4em;
    border: 1px solid var(--bd-source-border, currentColor);
    border-radius: var(--bd-radius, 4px);
    overflow: hidden;
  }

  .bd-modes button {
    font: inherit;
    font-size: 0.8em;
    padding: 0.2em 0.7em;
    color: var(--bd-muted, GrayText);
    background: none;
    border: none;
    cursor: pointer;
  }

  .bd-modes button + button {
    border-left: 1px solid var(--bd-source-border, currentColor);
  }

  .bd-modes button.bd-on {
    color: inherit;
    background: var(--bd-source-bg, color-mix(in srgb, currentColor 8%, transparent));
    font-weight: 600;
  }

  .bd-fold {
    justify-self: start;
    margin: 0.2em 0;
    padding: 0.15em 0.6em;
    font: inherit;
    font-size: 0.85em;
    color: var(--bd-muted, GrayText);
    background: none;
    border: 1px dashed currentColor;
    border-radius: var(--bd-radius, 4px);
    cursor: pointer;
  }

  .bd-pending {
    white-space: pre-wrap;
    opacity: 0.7;
  }

  .bd-nochange {
    color: var(--bd-muted, GrayText);
  }
</style>
