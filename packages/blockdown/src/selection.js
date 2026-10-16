// Rendered selection → source range.
//
// A selection in a rendered block is a run of *rendered* text; the host wants
// the run of *source* it came from (to anchor a comment, to place a caret).
// Rendering only ever removes characters from the source — markup, escapes —
// so the selected text, if it holds no markup itself, appears verbatim in the
// block's source. Which occurrence is decided by counting: the selection is
// the k-th occurrence in the rendered text, so it is taken to be the k-th in
// the source. Whitespace is compared loosely (a rendered line break is a
// space, a source line break may be either).
//
// When the selected run crosses markup (`**bold** and more`), there is no
// verbatim match and the answer is `null` — the honest result, which a host
// can meet with the block's whole range or a coarser anchor.

/** Escape a string for use inside a RegExp. */
function escape(text) {
  return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/** Every match of `needle` in `hay`, whitespace runs matching loosely. */
function matches(hay, needle) {
  const words = needle.trim().split(/\s+/).filter(Boolean).map(escape);
  if (words.length === 0) return [];
  const re = new RegExp(words.join('\\s+'), 'g');
  const out = [];
  let m;
  while ((m = re.exec(hay)) !== null) {
    out.push({ start: m.index, end: m.index + m[0].length });
    if (m[0].length === 0) re.lastIndex += 1;
  }
  return out;
}

/**
 * Locate a rendered selection in its block's source.
 *
 * @param {string} source    the block's markdown
 * @param {string} rendered  the block's rendered text (`textContent`)
 * @param {string} selected  the selected text
 * @param {number} offset    where the selection starts in `rendered`
 * @returns {{ start: number, end: number } | null} offsets into `source`
 */
export function locate(source, rendered, selected, offset) {
  const inRendered = matches(rendered, selected);
  if (inRendered.length === 0) return null;
  // The occurrence the reader selected: the one starting nearest `offset`.
  let k = 0;
  let best = Infinity;
  inRendered.forEach((m, i) => {
    const d = Math.abs(m.start - offset);
    if (d < best) {
      best = d;
      k = i;
    }
  });
  const inSource = matches(source, selected);
  if (inSource.length === 0) return null;
  // Same count: the k-th maps to the k-th. Otherwise markup hid or added an
  // occurrence, and the nearest by relative position is the best guess.
  if (inSource.length === inRendered.length) return inSource[k];
  const ratio = rendered.length > 0 ? offset / rendered.length : 0;
  const target = ratio * source.length;
  return inSource.reduce((a, b) => (Math.abs(b.start - target) < Math.abs(a.start - target) ? b : a));
}

/** The text offset of a DOM point inside `container`. */
export function textOffset(container, node, offset) {
  const range = document.createRange();
  range.selectNodeContents(container);
  range.setEnd(node, offset);
  return range.toString().length;
}
