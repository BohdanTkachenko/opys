//! The embedded web UI bundle (ADR-0078).
//!
//! `ui/dist` is built by the `ui-build` devShell command, **committed**, and
//! compiled into the binary by `build.rs`. Nothing here reads the filesystem at
//! runtime: a node serves the exact bundle it was built with, so an upgrade
//! cannot leave a half-old UI behind and a deployment is one file.
//!
//! This module is pure data plus one policy decision (caching); the HTTP shape
//! lives in [`crate::api`], which owns the routes and the error envelope.

/// The table `build.rs` writes: `(path below `ui/dist`, content type, bytes)`.
///
/// Wrapped in a module so the generated code is unambiguously generated, and so
/// a lint fired inside it can be silenced in one place.
mod generated {
    include!(concat!(env!("OUT_DIR"), "/assets.rs"));
}

/// One file of the bundle, ready to serve.
#[derive(Debug, Clone, Copy)]
pub struct Asset {
    /// Path below `ui/dist`, `/`-separated — also the URL path below the root.
    pub path: &'static str,
    pub content_type: &'static str,
    pub bytes: &'static [u8],
    /// What to send as `Cache-Control`. See [`cache_control`].
    pub cache_control: &'static str,
}

/// The document every route that is not the API serves: the SPA shell.
pub const INDEX: &str = "index.html";

/// A never-cache directive, for anything whose URL does not change with its
/// content. `no-cache` still allows storage — it requires revalidation, which is
/// a conditional request, not a re-download.
const REVALIDATE: &str = "no-cache";

/// A year, immutable: correct only for a content-hashed URL, where new content
/// means a new URL.
const FOREVER: &str = "public, max-age=31536000, immutable";

/// The SPA shell, if the bundle has one.
///
/// `Option`, not a panic: a bundle without an `index.html` is a broken build
/// product, and answering 500 with a message beats taking the node down.
pub fn index() -> Option<Asset> {
    get(INDEX)
}

/// One bundled file by its path below `ui/dist`.
///
/// Lookup is exact against a fixed table, which is why no path-traversal check
/// is needed here: `../` simply does not match anything.
pub fn get(path: &str) -> Option<Asset> {
    generated::ASSETS
        .iter()
        .find(|(candidate, _, _)| *candidate == path)
        .map(|&(path, content_type, bytes)| Asset {
            path,
            content_type,
            bytes,
            cache_control: cache_control(path),
        })
}

/// Every bundled file. Only the tests need this; serving goes through [`get`].
pub fn all() -> impl Iterator<Item = Asset> {
    generated::ASSETS
        .iter()
        .filter_map(|(path, _, _)| get(path))
}

/// How long a browser may keep a bundled file.
///
/// The shell is served from a stable URL, so it must be revalidated or an
/// upgraded node would keep answering with the previous UI — the stale-bundle
/// support question this exists to prevent. Everything else Vite emits carries a
/// content hash in its filename (see `assetFileNames` in `ui/vite.config.js`), so
/// its URL changes whenever its bytes do and it can be cached forever.
///
/// The hash test is deliberately conservative: a name we cannot recognise as
/// fingerprinted is revalidated. Being wrong that way costs a conditional
/// request; being wrong the other way serves a stale asset until the cache
/// expires.
fn cache_control(path: &str) -> &'static str {
    if is_fingerprinted(path) {
        FOREVER
    } else {
        REVALIDATE
    }
}

/// How many characters of content hash Vite puts in a filename.
///
/// Its `[hash]` is base64url, and this is its width. If a future Vite widens it,
/// the assets test that asserts every non-shell file is fingerprinted fails —
/// which is the right way to find out.
const HASH_LEN: usize = 8;

/// Whether a filename carries a content hash, as `name-<hash>.ext`.
///
/// Matched positionally — the separator, then exactly the hash — rather than by
/// splitting on the last `-`. The alphabet is base64url, so **the hash itself
/// can contain a hyphen** (`index-DNH-r1KB.js` is a real build of this bundle,
/// hash `DNH-r1KB`): about one build in eight produces one, and splitting at the
/// last `-` would read that as a four-character tail, decide the file is not
/// fingerprinted, and quietly serve a content-addressed asset with `no-cache`.
fn is_fingerprinted(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    let stem = name.split_once('.').map(|(s, _)| s).unwrap_or(name);
    // Bytes, so a non-ASCII filename cannot split mid-character. The index of
    // the `-`; `> 0` because the name before it must not be empty.
    let bytes = stem.as_bytes();
    let Some(sep) = bytes.len().checked_sub(HASH_LEN + 1).filter(|&i| i > 0) else {
        return false;
    };
    bytes[sep] == b'-'
        && bytes[sep + 1..]
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundle_is_embedded() {
        let index = index().expect("the build embeds ui/dist/index.html");
        assert!(!index.bytes.is_empty());
        assert_eq!(index.content_type, "text/html; charset=utf-8");
        assert!(
            all().count() >= 2,
            "a real bundle is a shell plus at least a script"
        );
    }

    #[test]
    fn the_shell_is_revalidated_and_hashed_assets_are_not() {
        assert_eq!(index().unwrap().cache_control, REVALIDATE);
        for asset in all().filter(|a| a.path != INDEX) {
            assert_eq!(
                asset.cache_control, FOREVER,
                "{} should be content-hashed — check assetFileNames in ui/vite.config.js",
                asset.path,
            );
        }
    }

    #[test]
    fn fingerprints_are_recognised_conservatively() {
        assert!(is_fingerprinted("ui/index-MLss2IHM.js"));
        assert!(is_fingerprinted("ui/style-CHEaQAcd.css"));
        assert!(is_fingerprinted("ui/some-name-A1b2C3d4.css"));
        // Not hashed: the shell, a short suffix, a suffix that is not a hash.
        assert!(!is_fingerprinted("index.html"));
        assert!(!is_fingerprinted("ui/index-MLss.js"));
        assert!(!is_fingerprinted("ui/index.js"));
        assert!(!is_fingerprinted("ui/-MLss2IHM.js"));
    }

    /// The case the heuristic was originally wrong about, and the reason a real
    /// bundle rebuild turned the suite red: `[hash]` is base64url, so a hyphen
    /// inside the hash is ordinary, not a second name segment.
    #[test]
    fn a_hyphen_inside_the_hash_is_still_a_hash() {
        assert!(is_fingerprinted("ui/index-DNH-r1KB.js"));
        assert!(is_fingerprinted("ui/index--1234567.js"));
        assert!(is_fingerprinted("ui/style-_a1B2c3D.css"));
        // Still not a hash: eight characters that are not the whole tail.
        assert!(!is_fingerprinted("ui/index-DNH-r1KB-extra.js"));
    }

    #[test]
    fn traversal_finds_nothing() {
        assert!(get("../Cargo.toml").is_none());
        assert!(get("/etc/passwd").is_none());
        assert!(get("").is_none());
    }
}
