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
output, `ui/dist`, is **generated and gitignored**, and compiled into the binary
by `build.rs`. Nothing in the crate build ever runs Node or touches the network —
`cargo install opys`, docs.rs and the nix sandbox cannot — and the page makes
zero external requests: no CDN, no webfont, no source map (ADR-0037).

So the bundle has to be built before the crate is:

```sh
# from the devShell, which is the only place Node lives; any cwd in the checkout
ui-build            # npm ci && vite build, then audit the output
```

Building from a git checkout therefore needs Node. If you are here to change the
Rust and not the UI, you can skip it entirely:

```sh
cargo build --workspace --no-default-features   # no bundle, no Node
```

That drops the `web-ui` feature. The node still serves its whole API; the two UI
routes answer 501 saying why. Everything published gets the UI regardless —
`cargo install opys` and docs.rs read the bundle out of the crate tarball
(`include` in `Cargo.toml`), and the nix package builds it in its own derivation
(`nix build .#opys-ui`).

A failed build (or a failed audit) leaves whatever bundle was already there
untouched — `ui/dist` is what `build.rs` embeds, so a typo in a Svelte file must
not be able to make the whole workspace stop compiling.

`npm run dev` in `ui/` gives a hot-reloading dev server that proxies `/api` to a
node on the default port.

Verification warnings include issue details and a **Fix** button. The corpus
button applies `opys verify --fix`: seed missing lifecycle history from Git,
then apply configured time/visit-limit transitions. A document view shows only
that document's findings, and its **Fix** changes only that document (without a
corpus-wide sync). Existing history is preserved; issues without an automatic
fix remain visible. The closed action API exposes this as
`{"action":"verify-fix"}` or `{"action":"verify-fix","id":"ADR-0010"}`.
The verification response includes `fixable_problems`, computed by previewing
the real engine fix pass in a disposable store without flushing or syncing.
**Fix** is enabled only when that list includes a displayed finding. Unknown
fields and other manual-only findings do not enable it; after the fixable
findings are cleared, the button is disabled even if other problems remain.

The lifecycle view is one live project-wide graph containing every document
type, status and permitted transition. Shared status names merge into common
nodes with combined counts; shared edges retain all owning types and whether
each type requires `close`. Unrestricted workflows draw every allowed status
pair, rather than a wildcard rule. Type controls highlight without filtering.
ELK routes the complete graph, including cycles. The inspector opens on
selection and lists documents across types, blocked counts, next picks and a
search field. Live count changes preserve node positions. Fit includes the
whole diagram, with zoom available for detail. The layout engine is bundled
locally and loaded only for this view.
Reciprocal transitions share one connection with arrowheads at both ends. The
underlying directions retain their own owning types, restrictions and closing
semantics; the inspector and transition count still use the directed model.

## License

Apache-2.0, like the rest of the workspace, permanently (ADR-0033).

See the [project README](../README.md) for what opys is and how to use it.
