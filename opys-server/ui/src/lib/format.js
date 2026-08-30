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
 * How long ago a timestamp was, compactly: `now`, `5m ago`, `3h ago`, `2d
 * ago`, and past a week the date itself.
 *
 * For a card or a header, where "how fresh" is the question and the absolute
 * time belongs in a tooltip ([`shortTime`]). `now` is a parameter so a view
 * that re-renders on a ticker ages every timestamp together, rather than each
 * one freezing at whatever the clock said when it was rendered.
 */
export function relativeTime(iso, now = Date.now()) {
  if (typeof iso !== 'string' || iso.length === 0) return null;
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return iso;
  const seconds = Math.round((now - then) / 1000);
  if (seconds < 45) return 'now';
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.round(hours / 24);
  if (days < 7) return `${days}d ago`;
  return shortDate(iso);
}

/**
 * Whether a frontmatter value is an RFC3339 timestamp — `created`, or a
 * hand-written date field. Shown formatted, edited raw.
 */
export function isTimestamp(value) {
  return (
    typeof value === 'string' &&
    /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}/.test(value) &&
    !Number.isNaN(new Date(value).getTime())
  );
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

/**
 * A hue for a status name, or `null` for the ones that should recede.
 *
 * Statuses are project vocabulary — `opys.toml` can declare anything — so this
 * deliberately does not rank or order them (the board learned that lesson; see
 * its column comment). Colour is annotation, not lifecycle: a handful of
 * near-universal words get their semantic hue, the "this word is retired"
 * family gets neutral grey, and anything else gets a deterministic hue hashed
 * from its own name — the same status is the same colour on every screen, in
 * every corpus, forever, without this file having opinions about it.
 */
// Finished things: green. Named because [`statusSettled`] reuses it — the
// words that earn the green are exactly the words the board's focus mode may
// hide.
const DONE_TONES =
  /^(done|implemented|accepted|fixed|closed|complete(d)?|merged|resolved|shipped|released)$/;

const SEMANTIC_TONES = [
  [DONE_TONES, 152],
  // Underway: cyan, the accent family.
  [/^(in-progress|doing|active|partial|in-review|review|testing|wip)$/, 195],
  // Stuck: red.
  [/^(blocked|stuck|failed)$/, 3],
  // Queued: blue.
  [/^(todo|open|planned|proposed|new|backlog|triage|draft|idea)$/, 226],
];

const NEUTRAL_TONES =
  /^(wontfix|won't-fix|superseded|archived|retired|rejected|obsolete|deprecated|cancelled|canceled|abandoned|invalid|duplicate)$/;

/**
 * Whether a status names *settled* work — the done family or the retired
 * family, exactly as the tone engine already classifies them. The board's
 * focus mode hides these columns; a status this file has never heard of is
 * never settled, so an unknown vocabulary always stays visible.
 */
export function statusSettled(status) {
  const name = String(status ?? '')
    .trim()
    .toLowerCase();
  return DONE_TONES.test(name) || NEUTRAL_TONES.test(name);
}

/**
 * FNV-1a, folded onto the wheel but kept out of the red band (±18° around 3),
 * so only the words that mean alarm read as alarm.
 */
function hashHue(name) {
  let hash = 0x811c9dc5;
  for (let i = 0; i < name.length; i += 1) {
    hash ^= name.charCodeAt(i);
    hash = Math.imul(hash, 0x01000193);
  }
  return 21 + ((hash >>> 0) % 325);
}

export function statusTone(status) {
  const name = String(status ?? '')
    .trim()
    .toLowerCase();
  if (name.length === 0 || NEUTRAL_TONES.test(name)) return null;
  for (const [pattern, hue] of SEMANTIC_TONES) {
    if (pattern.test(name)) return hue;
  }
  return hashHue(name);
}

/**
 * A hue for a document *type* — the card's colour on the board, where the
 * column already carries the status. Within a column every card shares a
 * status, so type is the axis that tells thirty cards apart.
 *
 * The same rules as [`statusTone`]: a few near-universal names get their
 * semantic hue, `chore` and friends recede to neutral (their whole identity is
 * being unremarkable), and any project-specific type gets a deterministic hue
 * from its own name. Both the type name and its likely id prefix are matched
 * (`feature` and `feat`), so a view that only has the prefix agrees with one
 * that has the type.
 */
const TYPE_TONES = [
  [/^(bug|defect|fix)$/, 355],
  [/^(feat|feature)$/, 262],
  [/^(task)$/, 210],
  [/^(adr|decision|rfc)$/, 40],
  [/^(epic|initiative)$/, 300],
  [/^(risk)$/, 25],
];

const NEUTRAL_TYPES = /^(chore|maintenance|misc)$/;

export function typeTone(type) {
  const name = String(type ?? '')
    .trim()
    .toLowerCase();
  if (name.length === 0 || NEUTRAL_TYPES.test(name)) return null;
  for (const [pattern, hue] of TYPE_TONES) {
    if (pattern.test(name)) return hue;
  }
  return hashHue(name);
}

/**
 * A filesystem path shortened from the middle: first segment, an ellipsis,
 * and the tail that still fits. The *end* of a path is the part that names
 * things, so end-truncation (`/home/dan/projects/ne…`) throws away exactly the
 * information a reader came for; this keeps it (`/home/…/nebula/opys`).
 */
export function middlePath(path, max = 42) {
  const text = typeof path === 'string' ? path : '';
  if (text.length <= max) return text;
  const parts = text.split('/');
  // Take segments from the end until the budget is spent, then prepend the
  // root segment and one ellipsis. Degenerate cases (one huge segment) fall
  // back to a plain end-truncate so something readable always comes back.
  const head = parts.slice(0, 2).join('/');
  let tail = '';
  for (let i = parts.length - 1; i >= 2; i -= 1) {
    const candidate = `/${parts[i]}${tail}`;
    if (head.length + 1 + candidate.length > max) break;
    tail = candidate;
  }
  if (tail.length === 0) return `${text.slice(0, max - 1)}…`;
  return `${head}/…${tail}`;
}

/** Split a comma-separated input the way the node's actions expect it. */
export function splitCsv(input) {
  return String(input ?? '')
    .split(',')
    .map((part) => part.trim())
    .filter((part) => part.length > 0);
}
