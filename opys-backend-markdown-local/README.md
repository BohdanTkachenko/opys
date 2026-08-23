# opys-backend-markdown-local

The default storage backend for [`opys`](https://crates.io/crates/opys): one
markdown file per document on the local filesystem.

It implements the engine's `Backend` trait and owns all corpus filesystem I/O —
walking and parsing documents on load, and executing the store's `FlushPlan`
(create / rename / relocate / delete) on flush — so the core library does no
document filesystem access and the storage medium stays swappable.

Most users depend on this transitively through the [`opys`](https://crates.io/crates/opys)
CLI rather than directly. Part of the
[opys](https://github.com/BohdanTkachenko/opys) workspace, alongside
[`opys-engine`](https://crates.io/crates/opys-engine).

Licensed under Apache-2.0.
