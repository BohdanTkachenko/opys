// The node's HTTP API, in one place.
//
// Every request this UI makes is a function here, and no component builds a URL
// or reads `response.status` itself. (Only the requests it makes: the node also
// serves `/api/health` and `/api/corpus/{cid}/verify`, and nothing here calls
// them — the version arrives in the event stream's `hello` frame and the verify
// counts ride along with `/api/projects`. They are documented in
// opys-server/README.md; a client method is one line if a view ever needs one.)
// That is not tidiness: the API has three
// conventions that are easy to get subtly wrong in a component and impossible to
// notice until a corpus misbehaves, and this module is where they are handled
// once.
//
//  1. **One error shape.** Every failure — including axum's own rejections and
//     the 404/405 fallbacks — is `{"error": "…"}`, and the message is written to
//     be shown to a person verbatim. So a failure becomes an [`ApiError`] whose
//     `message` is exactly that string, never a rephrasing.
//  2. **The status code is the instruction.** 422 means the corpus refused the
//     write and the user must change something; 503 means the inventory lock was
//     held and the *identical* request should simply be retried; a 500 saying
//     "not loaded" means the corpus is broken, not the transport. The predicates
//     below name those cases so a component never pattern-matches on numbers.
//  3. **Query strings are strict.** A repeated key is a hard 400 (axum's
//     `serde_urlencoded` rejects duplicates), so [`qs`] uses `set`, never
//     `append`, and drops empty values rather than sending `?status=`.

/**
 * A request that did not succeed.
 *
 * `status` is the HTTP status, or 0 when the request never reached the node at
 * all (the node is down, the machine is asleep, the browser refused the
 * connection). That distinction matters to the UI: 0 means "try again later",
 * everything else means the node answered and had something to say.
 */
export class ApiError extends Error {
  constructor(status, message) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
  }

  /** The node could not be reached — nothing was attempted. */
  get offline() {
    return this.status === 0;
  }

  /** The corpus refused a write: invalid, and the message says why. */
  get invalid() {
    return this.status === 422;
  }

  /**
   * The inventory lock was held past `OPYS_LOCK_TIMEOUT_MS` — a CLI invocation
   * or another write was in the middle of a cycle. Nothing was written and the
   * same request will work once the lock is free, so this is a retry, not a
   * validation failure.
   */
  get busy() {
    return this.status === 503;
  }

  /**
   * The corpus has never loaded, so a read has nothing to answer from. A
   * project-level breakage (a config that will not parse, a directory that
   * moved) rather than a request problem: retrying changes nothing until the
   * project is fixed.
   */
  get notLoaded() {
    return this.status === 500 && this.message.includes('not loaded');
  }
}

/**
 * One request, with the error envelope unwrapped.
 *
 * The body is read as text first and parsed leniently: an error page from
 * something that is not the node (a proxy, a captive portal) still has to
 * produce a sensible message rather than a `SyntaxError` from the JSON parser.
 */
async function request(path, init) {
  let response;
  try {
    response = await fetch(path, init);
  } catch (cause) {
    const detail = cause instanceof Error ? cause.message : String(cause);
    throw new ApiError(0, `cannot reach the node: ${detail}`);
  }

  const text = await response.text();
  let body = null;
  if (text.length > 0) {
    try {
      body = JSON.parse(text);
    } catch {
      body = null;
    }
  }

  if (!response.ok) {
    const message =
      typeof body?.error === 'string' && body.error.length > 0
        ? body.error
        : `the node answered ${response.status} with no message`;
    throw new ApiError(response.status, message);
  }
  return body;
}

const JSON_HEADERS = { accept: 'application/json' };

function get(path) {
  return request(path, { headers: JSON_HEADERS, credentials: 'same-origin' });
}

function post(path, body) {
  return request(path, {
    method: 'POST',
    // The node requires the content type on its POST routes; without it the
    // body is rejected before any handler sees it.
    headers: { ...JSON_HEADERS, 'content-type': 'application/json' },
    credentials: 'same-origin',
    body: JSON.stringify(body),
  });
}

/**
 * A query string from an object, skipping anything empty.
 *
 * `set` rather than `append`: a repeated key is a 400 from the node, so a filter
 * control that somehow produced two values must lose one here rather than break
 * the whole request. An empty value is left out entirely — the node reads
 * `?status=` as "no filter", but not sending it says the same thing plainly.
 */
function qs(params) {
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(params ?? {})) {
    if (value === undefined || value === null || value === '') continue;
    search.set(key, String(value));
  }
  const rendered = search.toString();
  return rendered.length > 0 ? `?${rendered}` : '';
}

const seg = encodeURIComponent;

export const api = {
  /** The allowlisted project groups and their corpora, with cached counts. */
  projects: () => get('/api/projects'),

  /**
   * Document summaries. The filters are exact equality (`tag` is membership),
   * AND-combined; there is no text search and no paging, which is why the board
   * asks for everything and filters in the browser.
   */
  docs: (cid, filters) => get(`/api/corpus/${seg(cid)}/docs${qs(filters)}`),

  /** One document: frontmatter, relations, rendered body, status vocabulary. */
  doc: (cid, id) => get(`/api/corpus/${seg(cid)}/doc/${seg(id)}`),

  /** Read-only SQL. `params` fills `$1..$n`; every cell comes back a string. */
  query: (cid, sql, params = []) => post(`/api/corpus/${seg(cid)}/query`, { sql, params }),

  /**
   * One write, from the node's closed vocabulary of actions. `body` is the
   * request as the node spells it, e.g. `{action: 'set-status', id, status}`;
   * unknown keys are rejected, so pass exactly the fields an action declares.
   */
  action: (cid, body) => post(`/api/corpus/${seg(cid)}/action`, body),

  /** The merged view across one project group's corpora (views 5). */
  union: (key, filters) => get(`/api/group/${seg(key)}/union${qs(filters)}`),

  /**
   * The allowlist as the setup screen needs it: whether the file exists at all
   * (`configured` — false is what triggers onboarding), the scan mode, the
   * resolved scan root, `$HOME`, and the current entries.
   */
  setup: () => get('/api/setup'),

  /** Write the scan mode and root. Returns the new setup state. */
  saveSetup: (body) => post('/api/setup', body),

  /**
   * Projects the scan found that are not allowlisted.
   *
   * Paths and names only, deliberately: a count or a verify dot beside an
   * unaccepted entry would mean opening the project to render it, and opening
   * it reads wherever its `opys.toml` points `base`. Keeping a person between
   * "found" and "opened" is the point.
   */
  suggestions: () => get('/api/suggestions'),

  /**
   * Add or remove one entry: `{action: 'add', path}` or
   * `{action: 'remove', path}`. Returns the new setup state, so a caller never
   * has to re-read to find out what it now looks like.
   *
   * An add is vetted by the node — under `$HOME`, no hidden directories — and a
   * refusal is a 422 whose message is written to be shown verbatim.
   */
  allowlist: (body) => post('/api/allowlist', body),
};
