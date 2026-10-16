# svelte-blockdown

A markdown editor and diff view for Svelte 5 that never rewrites what you
didn't touch.

- **Rendered until you double-click.** The document shows as rendered blocks
  (paragraphs, headings, list items, code blocks, tables, frontmatter).
  Double-clicking one turns *that block* into its markdown source, in place;
  leaving it renders it again.
- **Byte-faithful.** The markdown string is the only state. An edit replaces
  the edited block's exact source range and nothing else: no reformatted
  lists, no escaped punctuation, no dropped HTML comments. A one-word edit is
  a one-word diff.
- **Bring your own renderer.** You pass `render(sources) → html[]`, so blocks
  look exactly as your app renders markdown everywhere else (server-side,
  `marked`, `markdown-it`, …). The package ships no markdown renderer.
- **Frontmatter-aware.** A leading `---` YAML block is a block of its own,
  shown as a key/value table (or by your `renderFrontmatter`), and edited as
  source like any other.
- **Block diffs.** `BlockdownDiff` compares two versions block by block,
  side by side or inline (the reader switches with a built-in control), with word-level highlights inside changed blocks
  and unchanged runs folded.
- **Selection toolbar.** Select text in a rendered block and your `toolbar`
  snippet appears above it, told the selected text *and its range in the
  source* — enough to anchor a comment to the markdown.
- **No dependencies** beyond Svelte 5. About 10 kB minified.

## Install

```sh
npm install svelte-blockdown
```

The package ships Svelte source (`svelte` export condition), compiled by your
own Svelte/Vite setup.

## Editor

```svelte
<script>
  import { Blockdown } from 'svelte-blockdown';
  import { marked } from 'marked';

  let text = $state('# Hello\n\nDouble-click me.\n');

  // Any markdown → HTML function; it may be async (e.g. a server call).
  const render = (sources) => sources.map((md) => marked.parse(md));

  async function save(value) {
    const ok = await persist(value);
    return ok; // `false` tells the editor the save was refused
  }
</script>

<Blockdown bind:value={text} {render} onsave={save} />
```

### Props

| Prop | Type | |
|---|---|---|
| `value` | `string` (bindable) | The markdown. Updated on every committed block edit. A new value from outside replaces the document unless a block is open. |
| `render` | `(sources: string[]) => string[] \| Promise<string[]>` | Renders block sources to HTML, in order. Called only for blocks not rendered yet. **You are responsible for sanitizing** its output: it is inserted with `{@html}`. |
| `renderFrontmatter` | `(block: string) => string` | Renders a frontmatter block (fences included). Default: a key/value table. |
| `onsave` | `(value: string) => unknown` | Called when the reader leaves the editor with changes, after a checkbox click, and when the component is destroyed with unsaved changes. Return `false` to refuse: the change stays visible, is not re-sent until it changes again, and a refused checkbox click is reverted. |
| `onchange` | `(value: string) => void` | Every committed block edit. |
| `toolbar` | `Snippet<[{ text, start, end, close }]>` | Shown above a selection inside one rendered block. `start`/`end` are offsets into `value`, or `null` when the selection crosses markup and has no verbatim source. |
| `readonly` | `boolean` | Rendered only; checkboxes stay disabled. |
| `placeholder` | `string` | Shown for an empty document. |
| `class` | `string` | Added to the root element. |

The component also exports `startEditing()`, which opens the first visible
block: wire it to an "Edit" button as the keyboard way in.

### Keys, in an open block

| Key | |
|---|---|
| ↑ on the first line / ↓ on the last | Move into the previous / next block. |
| Enter on an empty last line (or in a heading) | Finish this block, start a new one below. |
| Backspace at the very start | Join this block onto the one above. |
| Esc, ⌘/Ctrl+Enter, or a click outside | Leave (and save, through `onsave`). |

Switching to another window does not count as leaving.

### Selection toolbar

```svelte
<Blockdown bind:value={text} {render}>
  {#snippet toolbar({ text, start, end, close })}
    <button onclick={() => { addComment(start, end, text); close(); }}>Comment</button>
  {/snippet}
</Blockdown>
```

## Diff

```svelte
<script>
  import { BlockdownDiff } from 'svelte-blockdown';
</script>

<BlockdownDiff before={oldText} after={newText} {render} bind:mode context={2} />
```

| Prop | | |
|---|---|---|
| `before`, `after` | `string` | The two versions. |
| `render`, `renderFrontmatter` | | As for the editor. |
| `mode` | `'split' \| 'inline'` (bindable) | Two columns, or one. Default `split`. The reader switches with the diff's own *Side by side / Inline* control; bind it to remember their choice. |
| `toggle` | `boolean` | Show that control. Default `true`; `false` when the host provides its own. |
| `context` | `number` | Unchanged blocks kept around each change; longer runs fold to a "⋯ N unchanged blocks" button. Default 2. |

Blocks are aligned by exact text (longest common subsequence). Removed blocks
next to added ones pair up as *changed* when they share enough text, and a
changed block shows its source with a word-level diff; unrelated blocks at the
same spot stay a removal and an addition.

## Styling

Rendered blocks get your app's markdown styles: wrap the component in the
element those styles target. The editor's own pieces take CSS variables:

| Variable | Used for |
|---|---|
| `--bd-source-font`, `--bd-source-size` | The source box and word diffs. |
| `--bd-source-bg`, `--bd-source-border`, `--bd-accent` | The source box, and its focus border. |
| `--bd-hover`, `--bd-muted`, `--bd-radius` | Block hover, placeholder and fold text, corners. |
| `--bd-diff-add`, `--bd-diff-del`, `--bd-diff-change` (+ `-bg`, `-word`) | Diff colours. |

## The pieces, without the components

```js
import { segment, splice, insertBlock, toggleTask, diffBlocks, diffWords, locate } from 'svelte-blockdown';
```

- `segment(markdown)` → `[{ start, end, kind }]`: top-level blocks with exact
  ranges (`kind`: `frontmatter`, `paragraph`, `heading`, `code`, `list`,
  `quote`, `table`, `html`, `rule`). Follows CommonMark's block-start rules.
- `splice`, `insertBlock`, `toggleTask`: the edits the editor makes.
- `diffBlocks(a, b)`, `diffWords(a, b)`: the diff's two levels.
- `locate(source, rendered, selected, offset)`: a rendered selection's range
  in its block's source.

## Limits

- Editing is per block: inside an open block you see that block's markdown
  (a paragraph, one list item), not inline-rendered text around the caret.
- Each block is rendered on its own, so document-wide markdown features —
  reference-style link definitions, footnotes — render only within the block
  that holds them.
- A selection that crosses inline markup (`**bold** and more`) has no
  verbatim source range; the toolbar gets `null` offsets for it.

## Tests

```sh
npm test
```

## License

Apache-2.0.
