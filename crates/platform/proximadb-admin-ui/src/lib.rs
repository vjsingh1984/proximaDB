//! ProximaDB embedded admin dashboard — a self-contained, **read-only** web UI
//! served by the REST listener of standalone `proximadb-server` (or any embedded
//! mode that boots the HTTP server).
//!
//! ## Design
//! A single static HTML page (vanilla JS, no CDN, no build step) is embedded
//! into the binary via [`include_str!`] and served at `GET /admin` (with
//! `/dashboard` kept as a back-compat alias). The page calls existing read-only
//! REST endpoints — `/health`, `/api/v2/_meta/capabilities`,
//! `/api/v2/collections`, `/metrics/prometheus`, `/metrics/json` — with `fetch`,
//! and renders **no mutation controls**, so it is read-only by construction.
//!
//! ## Auto-refresh
//! The dashboard's client-side auto-refresh is driven by the operator's
//! `[server.admin_ui]` config (`auto_refresh`, `refresh_interval_seconds`).
//! [`render_admin_html`] injects that config into the page at serve time by
//! replacing the [`ADMIN_CONFIG_SENTINEL`] token with a JSON literal — no
//! external URLs or write verbs are introduced, so the read-only / air-gapped
//! contract is preserved. When `auto_refresh = false` the page renders the
//! toggle **disabled** (greyed out); polling then only happens on an explicit
//! Refresh click.
//!
//! It lives in its own crate (apps = bins, root = lib, functionality in
//! `crates/`) so the root server only mounts the router; the inline dashboard
//! HTML no longer bloats `src/network/rest/server.rs`.

use std::sync::Arc;

use axum::{Router, response::Html, routing::get};

/// The embedded dashboard page (static template). Self-contained: no external
/// scripts, styles, or links, so it is safe for offline / air-gapped / embedded
/// deployments. The [`ADMIN_CONFIG_SENTINEL`] token is replaced at serve time
/// with the runtime config JSON (see [`render_admin_html`]).
pub const ADMIN_HTML: &str = include_str!("../assets/admin.html");

/// Sentinel HTML comment in [`ADMIN_HTML`] replaced at serve time with a
/// `<script>window.__ADMIN_CONFIG__ = {...};</script>` block carrying the
/// operator's `[server.admin_ui]` auto-refresh defaults. Kept as a plain HTML
/// comment so the static asset stays well-formed and the read-only contract (the
/// `http://` / write-verb scan in the `html_is_embedded_and_self_contained`
/// test) holds even when served without substitution.
const ADMIN_CONFIG_SENTINEL: &str = "<!--__ADMIN_CONFIG__-->";

/// Minimum auto-refresh poll interval enforced at render time (seconds). Guards
/// against a misconfigured tiny interval DoS-ing the diagnostic endpoints.
const MIN_REFRESH_INTERVAL_SECS: u32 = 5;

/// Dashboard runtime options, derived from `[server.admin_ui]` (`AdminUiConfig`
/// in the `proximadb-config` crate). Kept as plain primitives so this crate
/// stays decoupled from the config crate (it depends only on `axum`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AdminUiOptions {
    /// Mount the `/admin` (+ `/dashboard`) route. When `false` the router is
    /// empty.
    pub enabled: bool,
    /// Enable client-side auto-refresh. When `false` the UI toggle is disabled.
    pub auto_refresh: bool,
    /// Auto-refresh poll interval in seconds; clamped to
    /// [`MIN_REFRESH_INTERVAL_SECS`] at render time.
    pub refresh_interval_seconds: u32,
}

/// Render the dashboard HTML with the operator config injected.
///
/// The injected payload is a JSON literal of two scalars (`bool`/`u32`, so it
/// can never carry untrusted string content) consumed by the page's inline
/// script. The [`ADMIN_CONFIG_SENTINEL`] token is replaced exactly once.
pub fn render_admin_html(opts: AdminUiOptions) -> String {
    // Treat an unset/zero interval as the documented default (30s), then clamp
    // to a sane floor so a tiny value cannot hammer the diagnostic endpoints.
    let interval = if opts.refresh_interval_seconds == 0 {
        30
    } else {
        opts.refresh_interval_seconds.max(MIN_REFRESH_INTERVAL_SECS)
    };
    let inject = format!(
        r#"<script>window.__ADMIN_CONFIG__={{"autoRefresh":{},"intervalSec":{}}};</script>"#,
        opts.auto_refresh, interval
    );
    ADMIN_HTML.replacen(ADMIN_CONFIG_SENTINEL, &inject, 1)
}

/// Axum handler returning the read-only admin dashboard HTML **without** config
/// injection (auto-refresh falls back to the page's own disabled default). Used
/// by callers that embed the page without the `[server.admin_ui]` extras; the
/// configured path goes through [`admin_router`] + [`render_admin_html`].
pub async fn admin_page() -> Html<&'static str> {
    Html(ADMIN_HTML)
}

/// Router exposing the read-only admin dashboard at `/admin` and the back-compat
/// `/dashboard` alias, with the operator's auto-refresh config injected.
///
/// Generic over the caller's router state `S` so it merges cleanly into the main
/// `Router<AppState>` — the handler takes no [`axum::extract::State`], so it is
/// state-agnostic.
pub fn admin_router<S>(opts: AdminUiOptions) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    if !opts.enabled {
        return Router::new();
    }
    // The HTML depends only on boot-time config, so render it once and share the
    // allocation across requests via `Arc`. Each request clones the string
    // (cheap relative to an admin page view) — config never changes at runtime.
    let html: Arc<String> = Arc::new(render_admin_html(opts));
    let a = html.clone();
    let b = html.clone();
    Router::new()
        .route(
            "/admin",
            get(move || std::future::ready(Html::<String>((*a).clone()))),
        )
        .route(
            "/dashboard",
            get(move || std::future::ready(Html::<String>((*b).clone()))),
        )
}

/// Like [`admin_router`], but reads as "mount only when enabled" at the call
/// site. The dashboard is **disabled by default** and opt-in via TOML
/// (`[server.admin_ui] enabled = true`); the enable check happens inside
/// [`admin_router`] regardless.
pub fn admin_router_if<S>(opts: AdminUiOptions) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    admin_router(opts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_is_embedded_and_self_contained() {
        // The asset is baked into the binary.
        assert!(ADMIN_HTML.contains("<!DOCTYPE html>"));
        assert!(ADMIN_HTML.contains("ProximaDB"));
        // The serve-time injection anchor is present exactly once.
        assert_eq!(
            ADMIN_HTML.matches(ADMIN_CONFIG_SENTINEL).count(),
            1,
            "admin HTML must carry exactly one config-injection sentinel"
        );
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

    #[test]
    fn render_injects_config_and_preserves_readonly_contract() {
        let rendered = render_admin_html(AdminUiOptions {
            enabled: true,
            auto_refresh: true,
            refresh_interval_seconds: 30,
        });
        // Sentinel consumed exactly once.
        assert!(!rendered.contains(ADMIN_CONFIG_SENTINEL));
        assert!(
            rendered.contains(r#"window.__ADMIN_CONFIG__={"autoRefresh":true,"intervalSec":30}"#),
            "rendered HTML must carry the injected config JSON"
        );
        // Read-only / air-gapped contract still holds post-injection.
        assert!(
            !rendered.contains("http://") && !rendered.contains("https://"),
            "injected config must not introduce external URLs"
        );
        for verb in [
            "method: 'POST'",
            "method: 'PUT'",
            "method: 'DELETE'",
            "method: 'PATCH'",
        ] {
            assert!(
                !rendered.contains(verb),
                "injection must stay read-only ({verb})"
            );
        }
    }

    #[test]
    fn render_clamps_interval_and_defaults_zero() {
        // Zero resolves to the documented default (30s).
        let r = render_admin_html(AdminUiOptions {
            enabled: true,
            auto_refresh: false,
            refresh_interval_seconds: 0,
        });
        assert!(
            r.contains(r#""intervalSec":30"#),
            "zero interval defaults to 30s"
        );

        // A tiny interval is clamped to the minimum floor.
        let r = render_admin_html(AdminUiOptions {
            enabled: true,
            auto_refresh: true,
            refresh_interval_seconds: 1,
        });
        assert!(
            r.contains(&format!(r#""intervalSec":{}"#, MIN_REFRESH_INTERVAL_SECS)),
            "sub-minimum interval clamped to floor"
        );

        // A sane interval passes through unchanged.
        let r = render_admin_html(AdminUiOptions {
            enabled: true,
            auto_refresh: true,
            refresh_interval_seconds: 45,
        });
        assert!(
            r.contains(r#""intervalSec":45"#),
            "valid interval preserved"
        );
    }

    #[test]
    fn injected_config_payload_is_only_json_scalars() {
        // The injected payload is built only from a bool + u32, so it can never
        // contain quotes/braces that would break the JSON literal or enable an
        // injection. Isolate the literal and assert its exact shape.
        let rendered = render_admin_html(AdminUiOptions {
            enabled: true,
            auto_refresh: false,
            refresh_interval_seconds: 30,
        });
        let prefix = "window.__ADMIN_CONFIG__=";
        let start = rendered.find(prefix).expect("config global present");
        let slice = &rendered[start + prefix.len()..];
        let semi = slice.find(';').expect("statement terminated");
        assert_eq!(&slice[..semi], r#"{"autoRefresh":false,"intervalSec":30}"#);
    }
}
