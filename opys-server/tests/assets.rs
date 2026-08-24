//! The embedded web UI, served over the real router (TASK-0074).
//!
//! Only the server half is tested here. The bundle's *behaviour* — routing,
//! rendering, the WebSocket reconnect — is verified by hand at M1 closeout;
//! a browser harness would be a second toolchain to keep alive for assertions
//! these three facts already cover: the bytes are embedded, they come back
//! under the right content type, and the shell is reachable at `/`.
//!
//! Note there is no [`AppState`] fixture with corpora: nothing about serving the
//! bundle touches the manager, and building a tempdir project to prove that
//! would test the fixture instead.

use std::sync::{Arc, Mutex};

use axum::body::{Body, Bytes};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use opys_backend_markdown_local::MarkdownLocal;
use opys_engine::backend::Backend;
use opys_server::api::{self, AppState};
use opys_server::assets;
use opys_server::manager::Manager;
use tokio::sync::broadcast;
use tower::ServiceExt;

/// A node serving nothing but the bundle: the allowlist is an empty file, so
/// there are no corpora and no filesystem to keep alive.
fn state(dir: &tempfile::TempDir) -> AppState {
    let config = dir.path().join("server.toml");
    std::fs::write(&config, "").unwrap();
    let (events, _) = broadcast::channel(8);
    let backend = || Box::new(MarkdownLocal) as Box<dyn Backend + Send>;
    let mut manager = Manager::new(config, events.clone(), backend);
    manager.rescan().unwrap();
    AppState::new(Arc::new(Mutex::new(manager)), events)
}

/// Unlike the JSON fixtures, this keeps the raw body: an asset test that decoded
/// JSON would see `null` for every one of these responses.
async fn get(state: &AppState, uri: &str) -> (StatusCode, Option<String>, Option<String>, Bytes) {
    let response = api::router(state.clone())
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let header = |name: axum::http::HeaderName| {
        response
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    let (content_type, cache_control) = (header(CONTENT_TYPE), header(CACHE_CONTROL));
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, content_type, cache_control, bytes)
}

#[tokio::test]
async fn the_root_serves_the_spa_shell() {
    let dir = tempfile::tempdir().unwrap();
    let (status, content_type, cache_control, body) = get(&state(&dir), "/").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type.as_deref(), Some("text/html; charset=utf-8"));
    // The shell's URL never changes, so a cached copy would survive an upgrade.
    assert_eq!(cache_control.as_deref(), Some("no-cache"));
    let html = String::from_utf8(body.to_vec()).expect("the shell is UTF-8");
    assert!(html.contains("<div id=\"app\">"), "{html}");
    assert!(
        html.contains("./ui/"),
        "assets must be requested relatively, so the bundle does not care what \
         path it is served from: {html}"
    );
}

#[tokio::test]
async fn every_bundled_asset_comes_back_intact() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(&dir);

    let mut served = 0;
    for asset in assets::all().filter(|a| a.path != assets::INDEX) {
        let (status, content_type, cache_control, body) =
            get(&state, &format!("/{}", asset.path)).await;
        assert_eq!(status, StatusCode::OK, "GET /{}", asset.path);
        assert_eq!(content_type.as_deref(), Some(asset.content_type));
        // Content-hashed filenames, so the bytes behind a URL never change.
        assert_eq!(
            cache_control.as_deref(),
            Some("public, max-age=31536000, immutable"),
        );
        assert!(!body.is_empty(), "/{} is empty", asset.path);
        assert_eq!(&body[..], asset.bytes, "/{} was altered", asset.path);
        served += 1;
    }
    assert!(served >= 2, "a real bundle is a script and a stylesheet");
}

/// The script and the stylesheet the shell actually asks for, by the content
/// type a browser refuses to execute without.
#[tokio::test]
async fn the_shell_and_its_assets_agree() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(&dir);
    let (_, _, _, shell) = get(&state, "/").await;
    let html = String::from_utf8(shell.to_vec()).unwrap();

    for (marker, expected) in [
        (".js", "text/javascript; charset=utf-8"),
        (".css", "text/css; charset=utf-8"),
    ] {
        let referenced = assets::all()
            .find(|a| a.path.ends_with(marker))
            .unwrap_or_else(|| panic!("the bundle has no {marker}"));
        assert!(
            html.contains(&format!("./{}", referenced.path)),
            "the shell does not reference {}: {html}",
            referenced.path,
        );
        let (status, content_type, _, body) = get(&state, &format!("/{}", referenced.path)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(content_type.as_deref(), Some(expected));
        assert!(!body.is_empty());
    }
}

#[tokio::test]
async fn an_unknown_asset_is_a_json_404() {
    let dir = tempfile::tempdir().unwrap();
    let (status, content_type, _, body) = get(&state(&dir), "/ui/nope-12345678.js").await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(content_type.as_deref(), Some("application/json"));
    let error: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(
        error["error"].as_str().is_some_and(|e| e.contains("nope")),
        "{error}"
    );
}

/// Lookup is an exact match against a fixed table, so `..` is not special — it
/// simply is not in the table. Asserted anyway: the day someone replaces the
/// table with a filesystem read, this is the test that should stop them.
#[tokio::test]
async fn the_asset_route_cannot_escape_the_bundle() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(&dir);
    for path in ["/ui/../Cargo.toml", "/ui/../../etc/passwd", "/ui/"] {
        let (status, _, _, _) = get(&state, path).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "GET {path}");
    }
}

/// The bundle rides inside every copy of the binary, so its size is a number
/// worth failing on rather than noticing years later.
///
/// The ceiling is set near the real figure (~103 kB: the Svelte runtime plus
/// seven eagerly-loaded views) rather than at a round number far above it. A
/// 512 kB ceiling would catch a webfont, but it would also let the bundle
/// quintuple first — and growth is cheapest to reverse while it is small. Raise
/// it deliberately, with the reason, when a view genuinely needs the room.
#[test]
fn the_bundle_stays_small() {
    const CEILING: usize = 160 * 1024;
    let total: usize = assets::all().map(|a| a.bytes.len()).sum();
    assert!(
        total <= CEILING,
        "the embedded web UI is {total} bytes, over the {CEILING} ceiling — check \
         for a font, an image or a source map that should not be there, and if \
         the growth is real, raise the ceiling here with the reason",
    );
}

/// Not a runtime property, but the one packaging rule that is invisible until it
/// bites: the bundle must make no external request, or the node stops working
/// offline. Assets are audited fully by `scripts/ui-build.sh`; the shell is
/// cheap enough to re-assert here, where a hand edit to `ui/index.html` would
/// otherwise sail through.
#[test]
fn the_shell_requests_nothing_from_the_internet() {
    let html = std::str::from_utf8(assets::index().unwrap().bytes).unwrap();
    for offender in ["src=\"http", "href=\"http", "src=\"//", "href=\"//"] {
        assert!(
            !html.contains(offender),
            "the shell fetches something remote ({offender}): {html}",
        );
    }
    assert!(!html.contains("sourceMappingURL"), "{html}");
    // Sanity: the assertions above are only meaningful against the real shell.
    assert!(html.contains("<title>opys</title>"), "{html}");
}
