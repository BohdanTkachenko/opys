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

Apache-2.0, like the rest of the workspace. Everything you run on your own
machine over your own files is permissive; the copyleft boundary sits at the
layer that turns local nodes into a fleet or a service — the relay and the
hosted plane (ADR-0076).

See the [project README](../README.md) for what opys is and how to use it.
