# opys-tui

The interactive terminal UI for [`opys`](https://crates.io/crates/opys) — a live,
read-mostly board over the inventory that updates as documents change on disk.

It is a thin frontend over the library: reads go through the engine, and the
writes it offers (a status change and `close`) route through the same command
cores as the CLI, so on-disk invariants hold identically. Body edits are delegated
to `$EDITOR`, after which the board runs the auto-sync + `verify` pass.

This crate backs the `opys tui` subcommand. It is pulled in only when the `opys`
binary is built with its `tui` feature:

```sh
cargo install opys --features tui
opys tui
```

Part of the [opys](https://github.com/BohdanTkachenko/opys) workspace, alongside
[`opys-engine`](https://crates.io/crates/opys-engine) and
[`opys-backend-markdown-local`](https://crates.io/crates/opys-backend-markdown-local).

Licensed under Apache-2.0.
