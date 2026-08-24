# opys-server

The always-on [`opys`](https://github.com/BohdanTkachenko/opys) node: a
long-lived process that watches configured project roots, holds a warm store per
corpus, and serves the typed opys API over HTTP + WebSocket plus an embedded web
UI on localhost.

Files and git stay the only truth — the server is a view over them. Writes go
through the same engine command cores and the same advisory inventory lock the
CLI uses, so a running server and a concurrent `opys` invocation never become two
write authorities.

## The web UI

`ui/` is a Svelte 5 + Vite single-page app — no SvelteKit, no SSR. Its build
output, `ui/dist`, is **committed** and compiled into the binary by `build.rs`,
because `cargo install opys`, docs.rs and the nix sandbox all build with no Node
and no network (ADR-0078). Nothing in the crate build ever runs Node, and the
page makes zero external requests: no CDN, no webfont, no source map.

To change it:

```sh
# from the devShell, which is the only place Node lives; any cwd in the checkout
ui-build            # npm ci && vite build, then audit the output
```

A failed build (or a failed audit) leaves the bundle that was already in the
tree untouched — `ui/dist` is what `build.rs` embeds, so a typo in a Svelte file
must not be able to make the whole workspace stop compiling.

Then commit `ui/dist` alongside your source change; CI runs `ui-build --check`
and fails if the committed bundle no longer matches its sources. `npm run dev` in
`ui/` gives a hot-reloading dev server that proxies `/api` to a node on the
default port.

## License

Apache-2.0, like the rest of the workspace. Everything you run on your own
machine over your own files is permissive; the copyleft boundary sits at the
layer that turns local nodes into a fleet or a service — the relay and the
hosted plane (ADR-0076).

See the [project README](../README.md) for what opys is and how to use it.
