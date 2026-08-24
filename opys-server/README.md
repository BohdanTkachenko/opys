# opys-server

The always-on [`opys`](https://github.com/BohdanTkachenko/opys) node: a
long-lived process that watches configured project roots, holds a warm store per
corpus, and serves the typed opys API over HTTP + WebSocket plus an embedded web
UI on localhost.

Files and git stay the only truth — the server is a view over them. Writes go
through the same engine command cores and the same advisory inventory lock the
CLI uses, so a running server and a concurrent `opys` invocation never become two
write authorities.

## License

**AGPL-3.0-only** — unlike the rest of the workspace. The tool everyone embeds
and scripts (`opys`, `opys-engine`, and the storage backends) stays Apache-2.0;
the server-side components are copyleft. Dependencies flow one way,
`opys-server` → engine/backend, and nothing Apache-side may depend on this
crate.

See the [project README](../README.md) for what opys is and how to use it.
