// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! TD-CACHE-1 S2 — restart cache warming: shutdown warm-set manifests +
//! boot replay, defusing the post-eviction cold herd at its source.
//!
//! Co-design posture (2026-07-24 review): warming is **demand-proven or
//! contract-driven, never speculative**. A tenant gets warmed only when a
//! `warm_cache.json` manifest exists for them (i.e. they were measurably hot
//! before shutdown); idle/free collections stay lazy first-touch, so Spot
//! churn never generates unnecessary object-store traffic. Replay runs inside
//! the tenant's `io_trace::instrument` scope so (a) replayed entries key under
//! the REAL tenant (matching later lookups) and (b) replay GETs are metered
//! to the tenant they benefit.
//!
//! The manifest is advisory and mixed-safe: absent/corrupt/stale ⇒ cold start;
//! entries referencing compacted-away segments fail their ranged read and are
//! skipped.

use std::sync::Arc;

use crate::storage::engines::sst::survivor_range_cache::WarmKey;
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::trait_components::path_resolver::DrPathBuilder;

/// Manifest schema version (bump on incompatible change; readers skip
/// unknown versions — advisory artifact, never load-bearing).
const WARM_MANIFEST_VERSION: u32 = 1;
/// Entries older than this are ignored (the working set has likely moved).
const WARM_MANIFEST_MAX_AGE_SECS: i64 = 24 * 3600;
/// Top-K hot ranges persisted per tenant (~100 B/entry ⇒ ≤ ~200 KB).
const WARM_MANIFEST_TOP_K: usize = 2000;

#[derive(serde::Serialize, serde::Deserialize)]
struct WarmManifest {
    version: u32,
    created_unix: i64,
    entries: Vec<WarmKey>,
}

/// Per-tenant manifest object URL under the DrPath manifests subprefix —
/// fail-closed on an invalid tenant id (same recipe as
/// `metrics::metering_writer::ksu_snapshot_object_url`).
fn manifest_url(base_url: &str, tenant_id: &str) -> anyhow::Result<String> {
    let resolved = DrPathBuilder::build_tenant_system(None, tenant_id)
        .map_err(|e| anyhow::anyhow!("invalid tenant id for warm manifest: {e}"))?;
    let prefix = resolved.manifests_subprefix();
    let base = base_url.trim_end_matches('/');
    Ok(format!("{base}/{prefix}warm_cache.json"))
}

/// Shutdown-side: persist each hot tenant's warm set. Best-effort — errors are
/// logged and skipped (shutdown must not block on the object store). Returns
/// the number of manifests written.
pub async fn emit_warm_manifests(factory: &Arc<FilesystemFactory>, base_url: &str) -> usize {
    let Some((_invariants, survivor)) = crate::storage::engines::sst::core::get_warm_tier_caches()
    else {
        return 0;
    };
    let now = chrono::Utc::now().timestamp();
    let mut written = 0usize;
    for (tenant, entries) in survivor.warm_set(WARM_MANIFEST_TOP_K) {
        if entries.is_empty() {
            continue;
        }
        let manifest = WarmManifest {
            version: WARM_MANIFEST_VERSION,
            created_unix: now,
            entries,
        };
        let (Ok(url), Ok(bytes)) = (
            manifest_url(base_url, &tenant),
            serde_json::to_vec(&manifest),
        ) else {
            continue;
        };
        let options = proximadb_storage_filesystem_types::FileOptions {
            overwrite: true,
            create_dirs: true,
            ..Default::default()
        };
        match factory.write(&url, &bytes, Some(options)).await {
            Ok(()) => {
                written += 1;
                tracing::info!(
                    tenant,
                    entries = manifest.entries.len(),
                    "🔥 warm manifest persisted"
                );
            }
            Err(e) => tracing::warn!(tenant, "warm manifest write failed (skipped): {e}"),
        }
    }
    written
}

/// Boot-side: replay one tenant's manifest, if present and fresh. Runs the
/// replays inside the tenant's `io_trace` scope (tenant-correct cache keys +
/// metering). Also prefetches the referenced segments' CONTROL invariants
/// (header/footer/A0) so first queries skip their 3 control GETs. Returns
/// (ranges_loaded, segments_prefilled).
pub async fn replay_tenant(
    factory: &Arc<FilesystemFactory>,
    base_url: &str,
    tenant_id: &str,
) -> (usize, usize) {
    let Some((invariants, survivor)) = crate::storage::engines::sst::core::get_warm_tier_caches()
    else {
        return (0, 0);
    };
    let Ok(url) = manifest_url(base_url, tenant_id) else {
        return (0, 0);
    };
    let Ok(bytes) = factory.read(&url).await else {
        return (0, 0); // no manifest = tenant wasn't hot = nothing to warm
    };
    let Ok(manifest) = serde_json::from_slice::<WarmManifest>(&bytes) else {
        tracing::warn!(tenant_id, "warm manifest unreadable — cold start");
        return (0, 0);
    };
    if manifest.version != WARM_MANIFEST_VERSION
        || chrono::Utc::now().timestamp() - manifest.created_unix > WARM_MANIFEST_MAX_AGE_SECS
    {
        return (0, 0); // stale/unknown — the working set has moved on
    }

    let tenant_owned = tenant_id.to_string();
    crate::observability::io_trace::instrument(Some(tenant_owned), "warm.replay", async {
        let mut loaded = 0usize;
        let mut prefilled = 0usize;
        let mut seen_segments = std::collections::HashSet::new();
        for entry in &manifest.entries {
            let Ok(fs) = factory.get_filesystem(&entry.p) else {
                continue;
            };
            // Control-invariants prefill once per referenced segment.
            // `Ok(false)` = already warm (recovery/compaction scans can win
            // the race) or a legacy segment — both benign.
            if seen_segments.insert(entry.p.clone()) {
                match crate::storage::engines::sst::segment_format::prefetch_segment_invariants(
                    fs.as_ref(),
                    &entry.p,
                    &invariants,
                )
                .await
                {
                    Ok(true) => prefilled += 1,
                    Ok(false) => {
                        tracing::debug!(segment = %entry.p, "invariants prefill skipped (warm/legacy)")
                    }
                    Err(e) => {
                        tracing::warn!(segment = %entry.p, "invariants prefill failed: {e}")
                    }
                }
            }
            let path = entry.p.clone();
            let (off, len) = (entry.o, entry.l);
            let fs_for_load = fs.clone();
            if survivor
                .replay_entry(entry, || async move {
                    fs_for_load.read_range(&path, off, len).await
                })
                .await
            {
                loaded += 1;
            }
        }
        (loaded, prefilled)
    })
    .await
}

/// Boot warm task body: replay every tenant that has a manifest. Tenants are
/// derived from the catalog's collections; tenants without manifests cost one
/// cheap negative read each. Gated by `PROXIMADB_CACHE_PREFILL` (unset/1 = on).
pub async fn boot_warm(factory: Arc<FilesystemFactory>, base_url: String, tenants: Vec<String>) {
    if std::env::var("PROXIMADB_CACHE_PREFILL").ok().as_deref() == Some("0") {
        tracing::info!("cache warming disabled (PROXIMADB_CACHE_PREFILL=0)");
        return;
    }
    let mut total_ranges = 0usize;
    let mut total_segments = 0usize;
    for tenant in &tenants {
        let (r, s) = replay_tenant(&factory, &base_url, tenant).await;
        total_ranges += r;
        total_segments += s;
    }
    if total_ranges > 0 || total_segments > 0 {
        tracing::info!(
            tenants = tenants.len(),
            ranges = total_ranges,
            segments = total_segments,
            "🔥 restart cache warming complete"
        );
    }
}
