// Hash routing.
//
// The fragment, not the History API, and deliberately: the node serves exactly
// one document (`GET /`) and answers every other path with a JSON 404, so a
// history route would break the moment someone reloaded the page or shared a
// link. With `#/…` the browser never asks the server for anything but `/`, which
// is also what lets `base: './'` in the Vite config be safe.
//
// The whole route vocabulary is [`parse`]. Adding a view is adding a branch
// there and a branch in `App.svelte`; nothing else in the app inspects the hash.

/** @typedef {{view: string, cid?: string, id?: string, key?: string, path?: string, query: Record<string,string>}} Route */

/**
 * Parse a location hash into a route.
 *
 * Unrecognised paths become `{view: 'unknown'}` rather than throwing or
 * redirecting: a stale bookmark should say so, not silently land somewhere else.
 */
function parse(hash) {
  const raw = hash.replace(/^#/, '');
  const [pathname, search = ''] = raw.split('?');
  const query = Object.fromEntries(new URLSearchParams(search));
  const parts = pathname
    .split('/')
    .filter((part) => part.length > 0)
    .map(decodeSegment);

  if (parts.length === 0) return { view: 'home', query };

  if (parts[0] === 'corpus' && parts.length === 2) {
    return { view: 'board', cid: parts[1], query };
  }
  if (parts[0] === 'corpus' && parts.length === 4 && parts[2] === 'doc') {
    return { view: 'doc', cid: parts[1], id: parts[3], query };
  }
  // Views 4 and 5 (the query console and the union view) route here. They are
  // recognised already so that adding them is one component and one branch in
  // `App.svelte`, with no change to link building or to this parser.
  if (parts[0] === 'corpus' && parts.length === 3 && parts[2] === 'query') {
    return { view: 'query', cid: parts[1], query };
  }
  if (parts[0] === 'group' && parts.length === 3 && parts[2] === 'union') {
    return { view: 'union', key: parts[1], query };
  }

  return { view: 'unknown', path: pathname, query };
}

/** A path segment, tolerating a hash the browser handed back un-decodable. */
function decodeSegment(part) {
  try {
    return decodeURIComponent(part);
  } catch {
    return part;
  }
}

/** Ids and cids are opaque strings; never interpolate one into a path raw. */
const seg = encodeURIComponent;

function search(params) {
  const query = new URLSearchParams();
  for (const [key, value] of Object.entries(params ?? {})) {
    if (value === undefined || value === null || value === '') continue;
    query.set(key, String(value));
  }
  const rendered = query.toString();
  return rendered.length > 0 ? `?${rendered}` : '';
}

/** The board for a corpus, optionally with its filter state. */
export function boardPath(cid, filters) {
  return `/corpus/${seg(cid)}${search(filters)}`;
}

/** One document. */
export function docPath(cid, id) {
  return `/corpus/${seg(cid)}/doc/${seg(id)}`;
}

/** The query console for a corpus (view 4). */
export function queryPath(cid) {
  return `/corpus/${seg(cid)}/query`;
}

/**
 * The union view for a project group (view 5), optionally filtered.
 *
 * Note: the *group* key, not a cid. They are different namespaces and coincide
 * only for a single non-git project, so a cid passed here 404s.
 */
export function unionPath(key, filters) {
  return `/group/${seg(key)}/union${search(filters)}`;
}

/** An `href` for a route path, so links stay real links — middle-click works. */
export function href(path) {
  return `#${path}`;
}

let current = $state(parse(window.location.hash));

window.addEventListener('hashchange', () => {
  current = parse(window.location.hash);
});

/**
 * The current route.
 *
 * An object with a getter rather than the `$state` itself: an imported binding
 * cannot be reassigned, and this keeps the read reactive in every component that
 * touches `nav.route`.
 */
export const nav = {
  get route() {
    return current;
  },
};

/** Navigate. A no-op when it would not change anything, so no history churn. */
export function go(path) {
  const next = `#${path}`;
  if (window.location.hash === next) return;
  window.location.hash = next;
}
