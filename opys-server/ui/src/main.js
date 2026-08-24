import { mount } from 'svelte';

import './app.css';
import App from './App.svelte';

// Svelte 5 mounts imperatively; there is no SvelteKit here and no SSR, so the
// whole app is one client-side mount into the shell in index.html.
export default mount(App, { target: document.getElementById('app') });
