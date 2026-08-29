// Small pure helpers for rendering what the API returns.
//
// Nothing here knows about the DOM or about Svelte; everything is a total
// function over a value the node might send, including the nulls it sends
// deliberately.

/** An id looks like `PREFIX-0001`; the prefix is uppercase and the number bare. */
const DOC_ID = /^[A-Z][A-Z0-9]*-\d+$/;

/**
 * A timestamp as a short local string, or `null` if there is not one.
 *
 * Several fields are explicitly nullable (`updated`, `loaded_at`) and one of
 * them, `updated`, is simply absent for a document whose frontmatter has no
 * date — so "unknown" has to render as nothing rather than as `Invalid Date`.
 */
export function shortTime(iso) {
  if (typeof iso !== 'string' || iso.length === 0) return null;
  const when = new Date(iso);
  if (Number.isNaN(when.getTime())) return iso;
  return when.toLocaleString(undefined, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
}

/**
 * The same, coarsened to a date.
 *
 * For a card or a table cell, where the day is the useful part and the time is
 * noise. Timestamps in frontmatter are full RFC3339 (`2026-08-24T14:34:06-04:00`),
 * so this *drops* information — use [`shortTime`] anywhere that matters.
 */
export function shortDate(iso) {
  if (typeof iso !== 'string' || iso.length === 0) return null;
  const when = new Date(iso);
  if (Number.isNaN(when.getTime())) return iso;
  return when.toLocaleDateString(undefined, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
  });
}

/**
 * A relation map's title, split into text and whether it is a tombstone.
 *
 * The node keeps the `~~strikethrough~~` markers of a closed document on
 * purpose: that marker is the *only* thing distinguishing "this used to point at
 * a document that has since been closed" from a live link. Stripping it in the
 * client would be discarding the distinction; rendering it as literal tildes
 * would be showing markdown source. So: strip the markers, report the fact.
 */
export function relationTitle(title) {
  const text = typeof title === 'string' ? title : '';
  const match = /^~~(.*)~~$/.exec(text.trim());
  return match ? { text: match[1], struck: true } : { text, struck: false };
}

/** The last segment of a `/`-separated path. */
export function basename(path) {
  if (typeof path !== 'string') return '';
  const parts = path.split('/').filter((part) => part.length > 0);
  return parts.length > 0 ? parts[parts.length - 1] : path;
}

/**
 * What to call a corpus on screen.
 *
 * Its branch, or the directory it lives in — `branch` is `null` for any
 * non-git project, so the fallback is not the rare case. Here rather than in
 * each view because a corpus called two things one click apart reads as two
 * corpora; the `cid` in particular is an internal key nobody should ever see.
 */
export function corpusLabel(corpus) {
  if (!corpus) return '';
  return corpus.branch ?? basename(corpus.root);
}

/**
 * The document id a rendered body's link points at, or `null`.
 *
 * Bodies are linkified by the corpus, so a mention of `ADR-0050` becomes
 * `<a href="ADR-0050.md">` — a *file* path, relative to the document. Followed
 * literally that leaves the SPA entirely and lands on the node's JSON 404, so
 * the doc view intercepts these and routes them instead. Anything that is not a
 * bare `<ID>.md` (an external URL, an image, a link to a source file) is left
 * alone.
 */
export function docIdFromHref(href) {
  if (typeof href !== 'string' || href.length === 0) return null;
  // Not a relative document link: a scheme, a protocol-relative URL, or an
  // in-page anchor.
  if (/^[a-z][a-z0-9+.-]*:/i.test(href) || href.startsWith('//') || href.startsWith('#')) {
    return null;
  }
  const [path] = href.split(/[?#]/);
  if (!path.endsWith('.md')) return null;
  const id = basename(path).slice(0, -'.md'.length);
  return DOC_ID.test(id) ? id : null;
}

/**
 * A frontmatter value as a single readable string.
 *
 * `fields` carries arbitrary YAML — scalars, lists, nested maps — and a chip has
 * to show *something* for each. Scalars and scalar lists read naturally; anything
 * structured falls back to compact JSON, which is at least honest about what is
 * in the file.
 */
export function fieldText(value) {
  if (value === null || value === undefined) return '';
  if (typeof value === 'string') return value;
  if (typeof value === 'number' || typeof value === 'boolean') return String(value);
  if (Array.isArray(value)) {
    return value.every((item) => item === null || typeof item !== 'object')
      ? value.map(fieldText).join(', ')
      : JSON.stringify(value);
  }
  return JSON.stringify(value);
}

/** Split a comma-separated input the way the node's actions expect it. */
export function splitCsv(input) {
  return String(input ?? '')
    .split(',')
    .map((part) => part.trim())
    .filter((part) => part.length > 0);
}
