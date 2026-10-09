import { mount } from 'svelte';

import './app.css';
import App from './App.svelte';

// Svelte 5 mounts imperatively; there is no SvelteKit here and no SSR, so the
// whole app is one client-side mount into the shell in index.html.
export default mount(App, { target: document.getElementById('app') });

// The service worker makes the dashboard installable and lets an installed app
// open onto the shell while the node is down. Browsers only allow one in a
// secure context — `localhost`/`127.0.0.1` count, a LAN address over plain
// HTTP does not — so elsewhere this is skipped and the page works as before.
// Not in `npm run dev`, where a cached shell would hide the edit being made.
if (!import.meta.env.DEV && 'serviceWorker' in navigator && window.isSecureContext) {
  navigator.serviceWorker.register('./sw.js').catch(() => {
    // Installability is a nicety; the dashboard does not depend on it.
  });
}
