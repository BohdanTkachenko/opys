import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

// Present mostly so the build is explicit rather than defaulted (and so
// vite-plugin-svelte stops warning that it found no config). No SvelteKit, no
// adapter: this is a static SPA that axum serves from memory.
export default {
  preprocess: vitePreprocess(),
  compilerOptions: {
    // Runes only. The skeleton uses them; making it explicit means a legacy
    // `export let` slipping into a later view fails the build instead of
    // silently switching a component to the old reactivity model.
    runes: true,
  },
};
