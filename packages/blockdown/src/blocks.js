// Markdown → top-level blocks, with exact source ranges.
//
// The editor never re-serializes markdown. It keeps the source string as the
// only truth, shows each top-level block rendered, and when a block is edited
// it splices the new text over that block's range. So everything here is
// about *where* blocks are, never about what they mean: a line scanner that
// follows CommonMark's block-start rules closely enough to agree with a real
// renderer on where one block ends and the next begins.
//
// A block's range is `[start, end)`: its text, without the newline that ends
// its last line. The blank lines between blocks belong to no block, so
// `source.slice(start, end)` is exactly what the reader sees as that block and
// a splice leaves every other byte of the document alone.
//
// A YAML frontmatter fence at the very top (`---` … `---` or `...`) is a
// block of its own, kind `frontmatter`: markdown renderers do not know it
// (they would draw a rule and a paragraph), so the editor renders it itself.
//
// Top-level only: a list item is one block with everything nested in it, a
// blockquote is one block. That is the editing granularity, and it is also
// what keeps each block renderable on its own.

/** @typedef {'frontmatter'|'paragraph'|'heading'|'code'|'list'|'quote'|'table'|'html'|'rule'} BlockKind */
/** @typedef {{ start: number, end: number, kind: BlockKind }} Block */

const BLANK = /^[ \t]*$/;
const ATX = /^ {0,3}#{1,6}(?:[ \t]|$)/;
const FENCE = /^( {0,3})(`{3,}|~{3,})(.*)$/;
const RULE = /^ {0,3}([-*_])(?:[ \t]*\1){2,}[ \t]*$/;
const SETEXT = /^ {0,3}(?:=+|-+)[ \t]*$/;
const QUOTE = /^ {0,3}>/;
const ITEM = /^( {0,3})([-*+]|\d{1,9}[.)])([ \t]+|$)/;
const COMMENT = /^ {0,3}<!--/;
const HTML = /^ {0,3}<\/?[A-Za-z][\w-]*(?:[\s/>]|$)/;
const TABLE_DELIM = /^[ \t]*\|?[ \t]*:?-+:?[ \t]*(?:\|[ \t]*:?-+:?[ \t]*)*\|?[ \t]*$/;

/** The source as lines, each with its offsets (`end` excludes the newline). */
function lines(source) {
  const out = [];
  let start = 0;
  while (start <= source.length) {
    const nl = source.indexOf('\n', start);
    const end = nl === -1 ? source.length : nl;
    out.push({ start, end, text: source.slice(start, end) });
    if (nl === -1) break;
    start = nl + 1;
  }
  // A trailing newline yields one empty last "line" that is not a line.
  if (out.length > 1 && out[out.length - 1].text === '' && source.endsWith('\n')) out.pop();
  return out;
}

/** Leading indentation width, tabs to the next multiple of four. */
function indent(text) {
  let width = 0;
  for (const ch of text) {
    if (ch === ' ') width += 1;
    else if (ch === '\t') width += 4 - (width % 4);
    else break;
  }
  return width;
}

/** A list item marker, with the column its content starts at. */
function item(text) {
  const m = ITEM.exec(text);
  if (!m) return null;
  const rest = text.slice(m[0].length);
  // An empty item's content column is one past the marker.
  const content = rest.length === 0 && m[3] === '' ? m[0].length + 1 : m[0].length;
  return { indent: m[1].length, content, ordered: /\d/.test(m[2]), number: parseInt(m[2], 10), empty: BLANK.test(rest) };
}

/** Does this line begin a block that ends an open paragraph? (CommonMark §4.8) */
function interrupts(text) {
  if (ATX.test(text) || FENCE.test(text) || RULE.test(text) || QUOTE.test(text) || COMMENT.test(text)) return true;
  const it = item(text);
  // Only a non-empty item, and of ordered ones only one numbered 1.
  return Boolean(it && !it.empty && (!it.ordered || it.number === 1));
}

/**
 * Split markdown into its top-level blocks.
 *
 * @param {string} source
 * @returns {Block[]}
 */
export function segment(source) {
  const ls = lines(source);
  /** @type {Block[]} */
  const blocks = [];
  let i = 0;
  const push = (from, to, kind) => blocks.push({ start: ls[from].start, end: ls[to].end, kind });

  if (ls.length > 1 && /^---[ \t]*$/.test(ls[0].text)) {
    const close = ls.findIndex((l, n) => n > 0 && /^(?:---|\.\.\.)[ \t]*$/.test(l.text));
    if (close !== -1) {
      push(0, close, 'frontmatter');
      i = close + 1;
    }
  }

  while (i < ls.length) {
    const text = ls[i].text;
    if (BLANK.test(text)) {
      i += 1;
      continue;
    }
    const from = i;

    const fence = FENCE.exec(text);
    if (fence && !(fence[2][0] === '`' && fence[3].includes('`'))) {
      const ch = fence[2][0];
      const len = fence[2].length;
      const close = new RegExp(`^ {0,3}${ch === '`' ? '`' : '~'}{${len},}[ \\t]*$`);
      i += 1;
      while (i < ls.length && !close.test(ls[i].text)) i += 1;
      const to = Math.min(i, ls.length - 1);
      push(from, to, 'code');
      i = to + 1;
      continue;
    }

    if (ATX.test(text)) {
      push(from, from, 'heading');
      i += 1;
      continue;
    }

    if (RULE.test(text) && !item(text)) {
      push(from, from, 'rule');
      i += 1;
      continue;
    }

    if (COMMENT.test(text)) {
      while (i < ls.length && !ls[i].text.includes('-->')) i += 1;
      const to = Math.min(i, ls.length - 1);
      push(from, to, 'html');
      i = to + 1;
      continue;
    }

    if (HTML.test(text)) {
      while (i + 1 < ls.length && !BLANK.test(ls[i + 1].text)) i += 1;
      push(from, i, 'html');
      i += 1;
      continue;
    }

    if (QUOTE.test(text)) {
      // `>` lines, plus lazy continuation of a paragraph inside the quote.
      while (i + 1 < ls.length) {
        const next = ls[i + 1].text;
        if (QUOTE.test(next) || (!BLANK.test(next) && !interrupts(next))) i += 1;
        else break;
      }
      push(from, i, 'quote');
      i += 1;
      continue;
    }

    const it = item(text);
    if (it) {
      // The item owns every following line indented to its content column,
      // blank lines between such lines, and lazy paragraph continuations.
      let last = i;
      let j = i + 1;
      let prevBlank = false;
      while (j < ls.length) {
        const next = ls[j].text;
        if (BLANK.test(next)) {
          prevBlank = true;
          j += 1;
          continue;
        }
        if (indent(next) >= it.content) {
          last = j;
          prevBlank = false;
          j += 1;
          continue;
        }
        if (!prevBlank && !interrupts(next) && !item(next)) {
          last = j;
          j += 1;
          continue;
        }
        break;
      }
      push(from, last, 'list');
      i = last + 1;
      continue;
    }

    if (text.includes('|') && i + 1 < ls.length && TABLE_DELIM.test(ls[i + 1].text) && ls[i + 1].text.includes('-')) {
      i += 1;
      while (i + 1 < ls.length && !BLANK.test(ls[i + 1].text) && !interrupts(ls[i + 1].text)) i += 1;
      push(from, i, 'table');
      i += 1;
      continue;
    }

    // A paragraph: until a blank line or a block that interrupts it. A setext
    // underline ends it as a heading, underline included.
    let kind = 'paragraph';
    while (i + 1 < ls.length) {
      const next = ls[i + 1].text;
      if (BLANK.test(next)) break;
      if (SETEXT.test(next)) {
        i += 1;
        kind = 'heading';
        break;
      }
      if (interrupts(next)) break;
      i += 1;
    }
    push(from, i, /** @type {BlockKind} */ (kind));
    i += 1;
  }
  return blocks;
}

/**
 * Replace `[start, end)` with `text`, keeping the document's blank-line
 * structure sane: replacing a block with nothing removes it and the gap after
 * it, rather than leaving a run of blank lines behind.
 */
export function splice(source, start, end, text) {
  if (text.trim() !== '') return source.slice(0, start) + text + source.slice(end);
  let before = source.slice(0, start);
  let after = source.slice(end).replace(/^\n+/, '');
  if (after === '') {
    const trailing = source.endsWith('\n');
    before = before.replace(/\n+$/, '');
    return before === '' ? '' : before + (trailing ? '\n' : '');
  }
  return before + after;
}

/**
 * Insert a new block after offset `at` (a block's `end`, or 0 for the top),
 * separated from its neighbours by blank lines.
 */
export function insertBlock(source, at, text) {
  if (text.trim() === '') return source;
  if (source.trim() === '') return text + '\n';
  if (at === 0) return text + '\n\n' + source.replace(/^\n+/, '');
  return source.slice(0, at) + '\n\n' + text + source.slice(at);
}

const TASK = /^([ \t]*(?:>[ \t]*)*(?:[-*+]|\d{1,9}[.)])[ \t]+\[)([ xX])(\])/gm;

/**
 * Toggle the `n`th task-list checkbox (`- [ ]` / `- [x]`) in `text`, in
 * document order. Returns `text` unchanged when there is no such checkbox.
 */
export function toggleTask(text, n) {
  let seen = -1;
  return text.replace(TASK, (whole, open, mark, close) => {
    seen += 1;
    if (seen !== n) return whole;
    return open + (mark === ' ' ? 'x' : ' ') + close;
  });
}

const ESCAPES = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' };

/** Escape text for HTML. */
export function escapeHtml(text) {
  return text.replace(/[&<>"']/g, (c) => ESCAPES[c]);
}

/**
 * The default rendering of a frontmatter block: a key/value table of its
 * top-level keys (a value's indented continuation lines stay with it), or the
 * YAML as preformatted text when it is not that simple shape. No YAML parser:
 * this is display, and the source is what gets edited.
 *
 * @param {string} block the whole block, fences included
 */
export function frontmatterHtml(block) {
  const lines = block.split('\n').slice(1, -1);
  const rows = [];
  for (const line of lines) {
    const m = /^([A-Za-z0-9_][\w.-]*)[ \t]*:(?:[ \t]+(.*)|[ \t]*)$/.exec(line);
    if (m) rows.push([m[1], m[2] ?? '']);
    else if (rows.length > 0 && (/^[ \t]/.test(line) || line.startsWith('-') || line.trim() === '')) {
      const row = rows[rows.length - 1];
      row[1] = row[1] === '' ? line : `${row[1]}\n${line}`;
    } else {
      return `<pre class="bd-frontmatter"><code>${escapeHtml(lines.join('\n'))}</code></pre>`;
    }
  }
  const body = rows
    .map(([k, v]) => `<tr><th>${escapeHtml(k)}</th><td>${escapeHtml(v.replace(/\s+$/, ''))}</td></tr>`)
    .join('');
  return `<table class="bd-frontmatter"><tbody>${body}</tbody></table>`;
}
