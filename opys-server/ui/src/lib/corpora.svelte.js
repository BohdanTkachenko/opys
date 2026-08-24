// The project list, loaded once and shared.
//
// `GET /api/projects` asks every served corpus for its cached counts, so it is
// the most expensive read in the app and also the one most views want a piece
// of: the sidebar renders all of it, the board and the doc view want one
// corpus's label and branch for their heading. Fetching it per component would
// multiply that cost by the number of views on screen and let them disagree
// about what a corpus is called.
//
// So: one store, refreshed when the node says something changed, read by
// everyone. The one thing it deliberately does *not* do is cache across a
// reconnect — an event stream with no replay means the roster may have changed
// while we were away, which is exactly what a `reconnected` batch signals.

import { api } from './api.js';
import { events } from './events.svelte.js';
import { createResource } from './resource.svelte.js';

function createCorpora() {
  const resource = createResource();

  return {
    /** The project groups, or `[]` before the first load finishes. */
    get groups() {
      return resource.data ?? [];
    },
    get error() {
      return resource.error;
    },
    get settled() {
      return resource.settled;
    },
    /**
     * True when the node is healthy and serving nothing. A fresh install
     * allowlists no projects, and that is a state to explain, not an error.
     */
    get empty() {
      return resource.settled && resource.error === null && this.groups.length === 0;
    },

    /** One corpus by cid, across every group, or `undefined`. */
    find(cid) {
      for (const group of this.groups) {
        const corpus = group.corpora.find((c) => c.cid === cid);
        if (corpus) return corpus;
      }
      return undefined;
    },

    /** Reload. `quiet` keeps the current list on screen while it happens. */
    reload(quiet = false) {
      return resource.run(() => api.projects(), { quiet });
    },

    /**
     * Load, and keep the list current. Returns a stop function for the app
     * shell's `$effect`; nothing else should call this.
     */
    start() {
      this.reload();
      // Any event can change this list: a reload changes the counts and the
      // verify dot, added/removed change the roster itself.
      return events.subscribe(() => this.reload(true));
    },
  };
}

export const corpora = createCorpora();
