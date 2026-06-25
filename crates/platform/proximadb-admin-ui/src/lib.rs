//! ProximaDB embedded admin dashboard — a self-contained, **read-only** web UI
//! served by the REST listener of standalone `proximadb-server` (or any embedded
//! mode that boots the HTTP server).
//!
//! ## Design
//! A single static HTML page (vanilla JS, no CDN, no build step) is embedded
//! into the binary via [`include_str!`] and served at `GET /admin` (with
//! `/dashboard` kept as a back-compat alias). The page calls existing read-only
//! REST endpoints — `/health`, `/api/v2/_meta/capabilities`,
//! `/api/v2/collections`, `/metrics/json` — with `fetch`, and renders **no
//! mutation controls**, so it is read-only by construction.
//!
//! It lives in its own crate (apps = bins, root = lib, functionality in
//! `crates/`) so the root server only mounts the router; the ~1k-line inline
//! dashboard HTML no longer bloats `src/network/rest/server.rs`.

use axum::{Router, response::Html, routing::get};

/// The embedded dashboard page. Self-contained: no external scripts, styles, or
/// links, so it is safe for offline / air-gapped / embedded deployments.
pub const ADMIN_HTML: &str = include_str!("../assets/admin.html");

/// Axum handler returning the read-only admin dashboard HTML.
pub async fn admin_page() -> Html<&'static str> {
    Html(ADMIN_HTML)
}

/// Router exposing the read-only admin dashboard at `/admin` and the back-compat
/// `/dashboard` alias.
///
/// Generic over the caller's router state `S` so it merges cleanly into the main
/// `Router<AppState>` — the handler takes no [`axum::extract::State`], so it is
/// state-agnostic.
pub fn admin_router<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/admin", get(admin_page))
        .route("/dashboard", get(admin_page))
}

/// Like [`admin_router`], but returns an empty router when `enabled` is `false`.
///
/// The dashboard is **disabled by default** and opt-in via TOML
/// (`[server.admin_ui] enabled = true`). Keep it off for Kubernetes pods and for
/// embedded / UDS deployments (which serve over a local Unix-domain socket, not a
/// browser-reachable TCP port) unless an operator explicitly enables it for a
/// standalone instance. The router-build path always merges this; the toggle
/// decides whether the `/admin` route actually exists.
pub fn admin_router_if<S>(enabled: bool) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    if enabled {
        admin_router()
    } else {
        Router::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_is_embedded_and_self_contained() {
        // The asset is baked into the binary.
        assert!(ADMIN_HTML.contains("<!DOCTYPE html>"));
        assert!(ADMIN_HTML.contains("ProximaDB"));
        // Offline / read-only contract: no external script/style/link CDNs.
        assert!(
            !ADMIN_HTML.contains("http://") && !ADMIN_HTML.contains("https://"),
            "admin dashboard must be self-contained (no external URLs)"
        );
        // Read-only contract: only GET fetches, no write verbs wired in the page.
        for verb in [
            "method: 'POST'",
            "method: 'PUT'",
            "method: 'DELETE'",
            "method: 'PATCH'",
        ] {
            assert!(
                !ADMIN_HTML.contains(verb),
                "admin dashboard must be read-only ({verb})"
            );
        }
    }
}
