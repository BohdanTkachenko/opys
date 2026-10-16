// One async read, with the three states a view has to render.
//
// Every fetch in this app can fail, and most of them are re-run in the
// background when the node reports a change. That produces two requirements a
// bare `await` in a component does not meet:
//
//  - **A background refresh must not blank the screen.** `run(…, {quiet: true})`
//    leaves the current data in place while it works and, if it fails, keeps the
//    data and records the error beside it — so a view can show what it has plus
//    a "could not refresh" strip rather than throwing away a good page because
//    one poll failed.
//  - **Responses can arrive out of order.** Navigating between corpora quickly
//    starts a second read before the first answers; without a token the slower
//    one wins and the view shows the wrong corpus. Only the newest run is
//    allowed to write.

/**
 * @template T
 * A single loadable value.
 */
export function createResource() {
  let data = $state(null);
  /** @type {import('./api.js').ApiError | null} */
  let error = $state(null);
  let loading = $state(false);
  let token = 0;

  return {
    get data() {
      return data;
    },
    get error() {
      return error;
    },
    get loading() {
      return loading;
    },
    /** True once a first attempt has finished, however it went. */
    get settled() {
      return data !== null || error !== null;
    },

    /**
     * Run `fetcher`, superseding any read still in flight.
     *
     * @param {() => Promise<any>} fetcher
     * @param {{quiet?: boolean}} [options] `quiet` for a background refresh: no
     *   loading flag, and existing data survives a failure.
     */
    async run(fetcher, options) {
      const quiet = options?.quiet === true;
      const mine = ++token;
      if (!quiet) loading = true;
      try {
        const next = await fetcher();
        if (mine !== token) return;
        data = next;
        error = null;
      } catch (cause) {
        if (mine !== token) return;
        error = cause;
        // A failed first load has nothing to show, so the view falls back to the
        // error panel; a failed refresh keeps the last good data on screen.
        if (!quiet) data = null;
      } finally {
        if (mine === token) loading = false;
      }
    },
  };
}
