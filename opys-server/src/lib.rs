//! The always-on opys node (FEAT-0058).
//!
//! A long-lived process that serves an allowlist of local projects over a typed
//! HTTP/WebSocket API and an embedded web UI. The node is a *view* over files
//! and git, never a second write authority (ADR-0051), and it serves only what
//! the user allowlisted (ADR-0077).
//!
//! This is the library half; `src/main.rs` is the `opys-server` binary, and the
//! `opys` CLI links this crate for its `web` subcommand.

pub mod actor;
pub mod discover;
pub mod manager;
pub mod registry;
