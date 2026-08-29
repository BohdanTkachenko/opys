// The node's event stream: one WebSocket, shared by every view.
//
// `GET /api/events` is a global firehose — no subscription protocol, no replay,
// one frame per thing that happened anywhere on the node — and three of its
// properties shape this module:
//
//  - **One write produces two or three frames.** The `action-completed`
//    acknowledgement, the explicit post-write reload, and then the filesystem
//    watcher's debounced reload. Delivering each to the views would refetch the
//    active view three times per click, so frames are coalesced into a batch and
//    delivered once the stream goes quiet for [`COALESCE_MS`].
//  - **Payloads are counts, not data.** A frame says *that* a corpus changed;
//    the view refetches what it needs. So a subscriber is a refresh callback,
//    not a reducer.
//  - **There is no catch-up.** A client that lags past the server's broadcast
//    buffer is dropped, and a reconnect starts from now. Every reconnect
//    therefore delivers a batch marked `reconnected`, which means "you missed an
//    unknown amount — refetch everything you are showing".

/** How long the stream must be quiet before a batch is delivered. */
const COALESCE_MS = 300;

/** Backoff bounds for reconnecting. Jittered, so many tabs do not sync up. */
const FIRST_RETRY_MS = 500;
const MAX_RETRY_MS = 10_000;

/**
 * Whether a batch could have changed what `cid` shows.
 *
 * `reconnected` always counts (we do not know what was missed), and so does any
 * frame without a `cid` — a frame the client does not recognise is assumed to
 * matter rather than assumed not to.
 */
export function affects(batch, cid) {
  if (batch.reconnected) return true;
  return batch.events.some((event) => event.cid === undefined || event.cid === cid);
}

function createEvents() {
  /** `idle` before `start()`, then `connecting` → `open` ↔ `reconnecting`. */
  let status = $state('idle');
  /** The node's version, from the stream's `hello` frame. */
  let version = $state(null);

  const listeners = new Set();
  let socket = null;
  let retryMs = FIRST_RETRY_MS;
  let retryTimer = null;
  let coalesceTimer = null;
  let pending = [];
  let pendingReconnect = false;
  /** Whether we have ever been connected — a first connect is not a reconnect. */
  let everOpen = false;
  let running = false;

  function url() {
    const it = new URL('/api/events', window.location.href);
    it.protocol = it.protocol === 'https:' ? 'wss:' : 'ws:';
    return it.toString();
  }

  function deliver() {
    coalesceTimer = null;
    const batch = { events: pending, reconnected: pendingReconnect };
    pending = [];
    pendingReconnect = false;
    // A copy, so a listener that unsubscribes itself while being called (a view
    // navigating away in response to a refresh) cannot skip the next one.
    for (const listener of [...listeners]) listener(batch);
  }

  function schedule() {
    if (coalesceTimer !== null) clearTimeout(coalesceTimer);
    coalesceTimer = setTimeout(deliver, COALESCE_MS);
  }

  function connect() {
    if (!running) return;
    status = everOpen ? 'reconnecting' : 'connecting';
    let next;
    try {
      next = new WebSocket(url());
    } catch {
      // Constructing can throw on a malformed URL or a blocked scheme; treat it
      // as a failed attempt so the backoff still applies.
      retryLater();
      return;
    }
    socket = next;

    next.onopen = () => {
      if (socket !== next) return;
      status = 'open';
      retryMs = FIRST_RETRY_MS;
      if (everOpen) {
        // The stream has no replay, so anything that happened while we were
        // away is simply unknown. Say so and let every view reload.
        pendingReconnect = true;
        schedule();
      }
      everOpen = true;
    };

    next.onmessage = (message) => {
      if (socket !== next) return;
      let frame;
      try {
        frame = JSON.parse(message.data);
      } catch {
        return;
      }
      if (frame?.type === 'hello') {
        version = frame.version ?? null;
        return;
      }
      if (typeof frame?.event !== 'string') return;
      pending.push(frame);
      schedule();
    };

    const failed = () => {
      if (socket !== next) return;
      socket = null;
      retryLater();
    };
    next.onerror = failed;
    next.onclose = failed;
  }

  function retryLater() {
    if (!running) return;
    status = everOpen ? 'reconnecting' : 'connecting';
    // Full jitter: a node restart wakes every open tab at once, and identical
    // backoffs would have them all knock at the same instant.
    const wait = Math.round(retryMs * (0.5 + Math.random() * 0.5));
    retryMs = Math.min(retryMs * 2, MAX_RETRY_MS);
    retryTimer = setTimeout(connect, wait);
  }

  return {
    get status() {
      return status;
    },
    get version() {
      return version;
    },
    /** True once connected and still connected — the live indicator's source. */
    get live() {
      return status === 'open';
    },

    /**
     * Subscribe to coalesced batches. Returns the unsubscribe function, which is
     * what an `$effect` should return so a view stops refreshing when it leaves.
     */
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },

    /**
     * Open the stream. Returns a stop function; safe to call once from the app
     * shell's `$effect`, which is the only place that should.
     */
    start() {
      if (running) return () => {};
      running = true;
      everOpen = false;
      retryMs = FIRST_RETRY_MS;
      connect();
      return () => {
        running = false;
        status = 'idle';
        if (retryTimer !== null) clearTimeout(retryTimer);
        if (coalesceTimer !== null) clearTimeout(coalesceTimer);
        retryTimer = null;
        coalesceTimer = null;
        const closing = socket;
        socket = null;
        if (closing) {
          closing.onopen = closing.onmessage = closing.onerror = closing.onclose = null;
          closing.close();
        }
      };
    },
  };
}

export const events = createEvents();
