// Two versions of a markdown document, compared block by block.
//
// The unit is the block (see `blocks.js`): a reader reviewing a change thinks
// "this paragraph changed, that item was added", not in lines. Blocks are
// aligned by their exact text with a longest-common-subsequence pass; a run of
// removed blocks next to a run of added ones pairs up, in order, as *changed*
// blocks, and a changed block carries a word-level diff of its source.

import { segment } from './blocks.js';

/**
 * @typedef {{ type: 'same', before: string, after: string }
 *   | { type: 'added', after: string }
 *   | { type: 'removed', before: string }
 *   | { type: 'changed', before: string, after: string }} BlockOp
 * @typedef {{ type: 'same' | 'add' | 'del', text: string }} WordOp
 */

/** LCS alignment of two sequences; returns [i, j] index pairs that match. */
function lcs(a, b, eq = (x, y) => x === y) {
  const n = a.length;
  const m = b.length;
  // Trim the common head and tail first: most edits touch a small middle,
  // and the table is only built for what is left.
  let head = 0;
  while (head < n && head < m && eq(a[head], b[head])) head += 1;
  let tail = 0;
  while (tail < n - head && tail < m - head && eq(a[n - 1 - tail], b[m - 1 - tail])) tail += 1;
  const A = a.slice(head, n - tail);
  const B = b.slice(head, m - tail);
  const pairs = [];
  for (let k = 0; k < head; k += 1) pairs.push([k, k]);
  if (A.length > 0 && B.length > 0) {
    const w = B.length + 1;
    const table = new Uint32Array((A.length + 1) * w);
    for (let i = A.length - 1; i >= 0; i -= 1) {
      for (let j = B.length - 1; j >= 0; j -= 1) {
        table[i * w + j] = eq(A[i], B[j])
          ? table[(i + 1) * w + j + 1] + 1
          : Math.max(table[(i + 1) * w + j], table[i * w + j + 1]);
      }
    }
    let i = 0;
    let j = 0;
    while (i < A.length && j < B.length) {
      if (eq(A[i], B[j])) {
        pairs.push([head + i, head + j]);
        i += 1;
        j += 1;
      } else if (table[(i + 1) * w + j] >= table[i * w + j + 1]) i += 1;
      else j += 1;
    }
  }
  for (let k = tail; k > 0; k -= 1) pairs.push([n - k, m - k]);
  return pairs;
}

/** Below this share of unchanged text, two blocks are not "the same block, edited". */
const SIMILAR = 0.4;

/** The share of the two texts' characters a word diff keeps unchanged. */
function similarity(a, b) {
  const total = a.length + b.length;
  if (total === 0) return 1;
  const same = diffWords(a, b)
    .filter((w) => w.type === 'same' && w.text.trim() !== '')
    .reduce((n, w) => n + w.text.length, 0);
  return (2 * same) / total;
}

/** The blocks' texts. */
function texts(source) {
  return segment(source).map((b) => source.slice(b.start, b.end));
}

/**
 * Compare two documents block by block.
 *
 * @param {string} before
 * @param {string} after
 * @returns {BlockOp[]}
 */
export function diffBlocks(before, after) {
  const a = texts(before);
  const b = texts(after);
  const ops = [];
  let i = 0;
  let j = 0;
  const flush = (toI, toJ) => {
    const removed = a.slice(i, toI);
    const added = b.slice(j, toJ);
    const paired = Math.min(removed.length, added.length);
    const tail = [];
    for (let k = 0; k < paired; k += 1) {
      // Pair only blocks that are recognisably the same block edited; two
      // unrelated paragraphs at one spot are a removal and an addition.
      if (similarity(removed[k], added[k]) >= SIMILAR) {
        ops.push({ type: 'changed', before: removed[k], after: added[k] });
      } else {
        ops.push({ type: 'removed', before: removed[k] });
        tail.push({ type: 'added', after: added[k] });
      }
    }
    for (const text of removed.slice(paired)) ops.push({ type: 'removed', before: text });
    ops.push(...tail);
    for (const text of added.slice(paired)) ops.push({ type: 'added', after: text });
  };
  for (const [x, y] of lcs(a, b)) {
    flush(x, y);
    ops.push({ type: 'same', before: a[x], after: b[y] });
    i = x + 1;
    j = y + 1;
  }
  flush(a.length, b.length);
  return ops;
}

/** Words, runs of whitespace, and single punctuation marks. */
function tokens(text) {
  return text.match(/\s+|[\p{L}\p{N}_]+|[^\s\p{L}\p{N}_]/gu) ?? [];
}

/** Past this many token-pairs a word diff is not worth its table. */
const WORD_DIFF_LIMIT = 4_000_000;

/**
 * A word-level diff of two texts, with adjacent ops of one type merged.
 *
 * @param {string} before
 * @param {string} after
 * @returns {WordOp[]}
 */
export function diffWords(before, after) {
  const a = tokens(before);
  const b = tokens(after);
  /** @type {WordOp[]} */
  const ops = [];
  const emit = (type, text) => {
    if (text === '') return;
    const last = ops[ops.length - 1];
    if (last && last.type === type) last.text += text;
    else ops.push({ type, text });
  };
  if (a.length * b.length > WORD_DIFF_LIMIT) {
    emit('del', before);
    emit('add', after);
    return ops;
  }
  let i = 0;
  let j = 0;
  for (const [x, y] of lcs(a, b)) {
    emit('del', a.slice(i, x).join(''));
    emit('add', b.slice(j, y).join(''));
    emit('same', a[x]);
    i = x + 1;
    j = y + 1;
  }
  emit('del', a.slice(i).join(''));
  emit('add', b.slice(j).join(''));
  return coalesce(ops);
}

/**
 * Fold whitespace that sits between two changes into them, so "A new
 * closing paragraph" reads as one insertion, not four with gaps.
 */
function coalesce(ops) {
  const changedAt = (k) => ops[k] && ops[k].type !== 'same';
  /** @type {WordOp[]} */
  const out = [];
  const push = (type, text) => {
    const last = out[out.length - 1];
    if (last && last.type === type) last.text += text;
    else out.push({ type, text });
  };
  // Within a run of changes, deletions come before additions.
  let dels = '';
  let adds = '';
  const flush = () => {
    if (dels) push('del', dels);
    if (adds) push('add', adds);
    dels = '';
    adds = '';
  };
  ops.forEach((op, k) => {
    if (op.type === 'same' && /^\s+$/.test(op.text) && changedAt(k - 1) && changedAt(k + 1)) {
      dels += op.text;
      adds += op.text;
    } else if (op.type === 'del') dels += op.text;
    else if (op.type === 'add') adds += op.text;
    else {
      flush();
      push('same', op.text);
    }
  });
  flush();
  return out;
}
