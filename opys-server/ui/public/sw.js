// The service worker: what makes the dashboard an installable app, and what
// lets the installed app open when the node is down — onto the shell, which
// then says the node is not answering, instead of onto a browser error page.
//
// It caches the UI and nothing else. `/api/` is never touched: the node is the
// only truth for documents (ADR-0051), and a cached answer would show a board
// that no longer exists. Navigations go to the network first, so an upgraded
// node's shell wins whenever the node is up; the hashed assets under `/ui/` are
// immutable by URL, so they are served from the cache once fetched. A fresh
// shell prunes every cached asset it no longer references, so upgrades do not
// accumulate old bundles.
//
// Plain JS, served at the root rather than under /ui/: a worker's scope is
// capped at its own directory, and this one has to control `/`.

const CACHE = 'opys-ui-v1';
const SHELL = './';

self.addEventListener('install', (event) => {
  event.waitUntil(
    caches
      .open(CACHE)
      .then((cache) => cache.addAll([SHELL, './manifest.webmanifest', './icon.svg']))
      .then(() => self.skipWaiting()),
  );
});

self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) => Promise.all(keys.filter((key) => key !== CACHE).map((key) => caches.delete(key))))
      .then(() => self.clients.claim()),
  );
});

/** Drop cached `/ui/` assets the given shell no longer references. */
async function prune(cache, html) {
  const keep = new Set(
    [...html.matchAll(/["']\.?\/(ui\/[^"']+)["']/g)].map((m) => new URL(m[1], self.registration.scope).href),
  );
  for (const request of await cache.keys()) {
    if (new URL(request.url).pathname.startsWith('/ui/') && !keep.has(request.url)) {
      await cache.delete(request);
    }
  }
}

async function shell(request) {
  const cache = await caches.open(CACHE);
  try {
    const response = await fetch(request);
    if (response.ok) {
      await cache.put(SHELL, response.clone());
      await prune(cache, await response.clone().text());
    }
    return response;
  } catch (error) {
    const cached = await cache.match(SHELL);
    if (cached) return cached;
    throw error;
  }
}

async function asset(request) {
  const cache = await caches.open(CACHE);
  const cached = await cache.match(request);
  if (cached) return cached;
  const response = await fetch(request);
  if (response.ok) await cache.put(request, response.clone());
  return response;
}

self.addEventListener('fetch', (event) => {
  const { request } = event;
  if (request.method !== 'GET') return;
  const url = new URL(request.url);
  if (url.origin !== self.location.origin || url.pathname.startsWith('/api/')) return;
  if (request.mode === 'navigate') {
    // Only the document at `/`: the UI is hash-routed, so any other navigation
    // is something the node answers itself (a JSON 404) and must see.
    if (url.pathname === '/') event.respondWith(shell(request));
    return;
  }
  if (url.pathname.startsWith('/ui/')) event.respondWith(asset(request));
});
