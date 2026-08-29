import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// The bundle is committed (opys-server/ui/dist) and embedded into the crate with
// include_bytes!, so this config optimises for two things the usual web app does
// not care about: byte-for-byte reproducibility (a CI job rebuilds and fails on
// drift) and zero external requests (the node must work with the machine
// offline, so no CDN, no webfonts, no source maps pointing at anything).
//
// Nothing build-time-variable may enter the output — no dates, no git SHA, no
// injected version — or the drift gate goes red without a source change. The UI
// reads the node's version from GET /api/health at runtime instead.
export default defineConfig({
  // Relative asset URLs, so the bundle does not care what path axum serves it
  // from. Safe because the UI is hash-routed: the document URL is always `/`,
  // which is what `./` resolves against. Switch to an absolute base if a
  // history-API route is ever added — do not mix the two.
  base: './',
  plugins: [svelte()],
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    // Emitted URLs become ./ui/… which is exactly the GET /ui/{file} route the
    // server serves; index.html is the only thing at the root.
    assetsDir: 'ui',
    // No .map files and no sourceMappingURL — a source map is an external
    // request waiting to happen and doubles the embedded bytes.
    sourcemap: false,
    target: 'es2022',
    // One stylesheet, so the asset set stays small and predictable.
    cssCodeSplit: false,
    // Suppresses Vite's injected inline module-preload polyfill; every browser
    // that can reach a local node supports it natively.
    modulePreload: { polyfill: false },
    rollupOptions: {
      output: {
        // Content-hashed filenames: an upgraded node serves new URLs, so a
        // stale bundle cannot survive in a browser cache.
        entryFileNames: 'ui/[name]-[hash].js',
        chunkFileNames: 'ui/[name]-[hash].js',
        assetFileNames: 'ui/[name]-[hash][extname]',
      },
    },
  },
  server: {
    // `npm run dev` only: the dev server proxies the API to a node running on
    // the default port, so the browser sees one origin and the node's
    // Origin/Host guard is never in the way.
    proxy: {
      '/api': {
        target: 'http://127.0.0.1:6797',
        ws: true,
      },
    },
  },
});
