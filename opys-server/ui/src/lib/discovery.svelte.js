// One suggestion scan shared by the sidebar and setup. Scanning only finds
// project directories; it does not open their inventories or register them.
import { api } from './api.js';
import { events } from './events.svelte.js';
import { createResource } from './resource.svelte.js';

const REFRESH_MS = 60_000;

function createDiscovery() {
  const resource = createResource();
  let inFlight = null;

  function reload({force = false} = {}) {
    if (inFlight && !force) return inFlight;
    const pending = resource.run(() => api.suggestions(), {quiet:resource.settled});
    inFlight = pending;
    pending.finally(() => { if (inFlight === pending) inFlight = null; });
    return pending;
  }

  return {
    get items() { return resource.data ?? []; },
    get count() { return this.items.length; },
    get loading() { return resource.loading; },
    get error() { return resource.error; },
    reload,
    start() {
      reload();
      const visible = () => {
        if (document.visibilityState !== 'hidden') reload();
      };
      const timer = setInterval(visible, REFRESH_MS);
      document.addEventListener('visibilitychange', visible);
      const unsubscribe = events.subscribe((batch) => {
        if (batch.reconnected || batch.events.some((event) =>
          event.event === 'corpus-added' || event.event === 'corpus-removed')) {
          reload({force:true});
        }
      });
      return () => {
        clearInterval(timer);
        document.removeEventListener('visibilitychange', visible);
        unsubscribe();
      };
    },
  };
}

export const discovery = createDiscovery();
