// Fuzzy matching for the omnibox — fzf's idea at a fraction of its size.
//
// A query matches a text when its characters appear in order. A contiguous
// hit outranks any scattered one; among scattered ones, characters landing on
// word starts ("dpp" → Design the Presence Protocol) and next to each other
// score higher, and gaps cost a little. No dependency: the corpus is hundreds
// of documents, and a scorer this small runs over all of them per keystroke
// without noticing.

const BOUNDARY = /[^a-z0-9]/;

function isStart(text, i) {
  return i === 0 || BOUNDARY.test(text[i - 1]);
}

/**
 * Score `query` against `text`, or `null` when the query is not a
 * subsequence of it. Positions are indices into `text`, for highlighting.
 */
export function match(query, text) {
  const q = String(query).toLowerCase();
  const t = String(text).toLowerCase();
  if (q.length === 0) return { score: 0, positions: [] };
  const at = t.indexOf(q);
  if (at >= 0) {
    return {
      score: 1000 + (isStart(t, at) ? 200 : 0) - at * 2 - (t.length - q.length),
      positions: Array.from({ length: q.length }, (_, i) => at + i),
    };
  }
  // Preferring word starts can skip past the only viable path ("ab" in
  // "cab a"), so the plain greedy scan is the fallback.
  return scatter(q, t, true) ?? scatter(q, t, false);
}

function scatter(q, t, preferStarts) {
  const positions = [];
  let score = 0;
  let from = 0;
  for (const ch of q) {
    let i = t.indexOf(ch, from);
    if (i < 0) return null;
    if (preferStarts && !isStart(t, i)) {
      for (let j = i + 1; j < Math.min(t.length, i + 12); j += 1) {
        if (t[j] === ch && isStart(t, j)) {
          i = j;
          break;
        }
      }
    }
    const last = positions[positions.length - 1];
    score += 10;
    if (isStart(t, i)) score += 20;
    if (last !== undefined) score += i === last + 1 ? 15 : -Math.min(10, i - last - 1);
    positions.push(i);
    from = i + 1;
  }
  return { score: score - t.length * 0.1, positions };
}

function byUpdated(a, b) {
  return String(b.doc.updated ?? '').localeCompare(String(a.doc.updated ?? ''));
}

/**
 * Rank `{cid, doc}` entries for a query: the id counts most, then the title,
 * then tags and status. An empty query is "what moved lately" — the entries
 * by `updated`, newest first — because that is what a jump usually wants
 * before a single character is typed.
 */
export function rankDocs(query, entries) {
  const q = String(query).trim();
  if (q.length === 0) {
    return entries
      .slice()
      .sort(byUpdated)
      .map((entry) => ({ ...entry, score: 0, idPos: [], titlePos: [] }));
  }
  const out = [];
  for (const entry of entries) {
    const { doc } = entry;
    const id = match(q, doc.id);
    const title = match(q, doc.title ?? '');
    const tags = doc.tags?.length ? match(q, doc.tags.join(' ')) : null;
    const status = doc.status ? match(q, doc.status) : null;
    const score = Math.max(
      id ? id.score * 1.2 : -Infinity,
      title ? title.score : -Infinity,
      tags ? tags.score * 0.7 : -Infinity,
      status ? status.score * 0.5 : -Infinity,
    );
    if (score === -Infinity) continue;
    out.push({ ...entry, score, idPos: id?.positions ?? [], titlePos: title?.positions ?? [] });
  }
  return out.sort((a, b) => b.score - a.score || byUpdated(a, b));
}

/** `text` cut into runs, each marked whether it is one of `positions`. */
export function segments(text, positions) {
  const hits = new Set(positions);
  const out = [];
  let run = '';
  let hit = false;
  for (let i = 0; i < text.length; i += 1) {
    const here = hits.has(i);
    if (here !== hit && run.length > 0) {
      out.push({ text: run, hit });
      run = '';
    }
    hit = here;
    run += text[i];
  }
  if (run.length > 0) out.push({ text: run, hit });
  return out;
}
