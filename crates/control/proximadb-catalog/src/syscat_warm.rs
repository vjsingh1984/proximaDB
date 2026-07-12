//! TD-SC-2 / ADR-035 D2 (warm tier): on-disk warm cache for the per-tenant system
//! catalog, sitting between the in-memory hot tier ([`super::syscat_cache`]) and
//! the canonical object-store catalog.
//!
//! It is a [`CatalogMetadataSource`] **decorator**: the hot cache's inner source
//! is a [`WarmDiskStore`] whose own inner is the canonical catalog reader. A hot
//! miss (eviction / 5-min TTL expiry / cold process) reads through here, which
//! serves the entry from a small local file — hitting the **OS page cache**
//! instead of paying the catalog's `1 + N + M` object-store round-trips. So a
//! collection that is read again after its hot entry expired, but was not
//! written in between, costs **zero object GETs**.
//!
//! Coherence (no stale reads), with **zero write-path coupling**: each on-disk
//! entry is stamped with the [`CorpusVersionRegistry`] version it was written at.
//! A read recomputes the live version and serves the file only when they match —
//! a write bumps the version (unconditionally, per the cache-coherence fix), so
//! the stamp no longer matches and the warm tier refetches the canonical and
//! rewrites the file. A 5-minute mtime backstop bounds staleness even if a
//! version signal is ever missed.
//!
//! Scope: the **within-process** TTL-expired-but-unchanged read. The version
//! registry is process-local, so a pod restart resets it and the warm files
//! refetch once (correct, just not yet a restart speedup); the durable shared
//! version store (TD-SC-5) unlocks the cross-restart / multi-pod warm benefit.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::Result;
use async_trait::async_trait;
use prost::Message;
use tracing::debug;

use crate::CorpusVersionRegistry;
use crate::proto::proximadb_v1::Collection;
use crate::syscat_cache::CatalogMetadataSource;

/// mtime backstop: a warm file older than this is treated as a miss regardless of
/// version (mirrors the hot tier's 5-minute TTL — a freshness ceiling).
const WARM_TTL: Duration = Duration::from_secs(300);

/// On-disk warm tier decorating an inner [`CatalogMetadataSource`] (the canonical
/// catalog reader). Files live under `root/{tenant}/{name}.bin`, each framed as
/// `[u64 version little-endian][prost-encoded Collection]`.
pub struct WarmDiskStore {
    inner: Arc<dyn CatalogMetadataSource>,
    root: PathBuf,
}

impl WarmDiskStore {
    /// Build a warm store rooted at `root` (created on first write) over `inner`.
    pub fn new(root: PathBuf, inner: Arc<dyn CatalogMetadataSource>) -> Self {
        Self { inner, root }
    }

    /// Filesystem path for a `(tenant, name)` entry, or `None` if either segment
    /// is unsafe as a path component (contains a separator, `..`, or NUL) — such
    /// a read bypasses the disk tier and goes straight to `inner` (fail-safe,
    /// never escapes `root`).
    fn entry_path(&self, tenant: &str, name: &str) -> Option<PathBuf> {
        for seg in [tenant, name] {
            if seg.is_empty()
                || seg.contains('/')
                || seg.contains('\\')
                || seg.contains('\0')
                || seg.contains("..")
            {
                return None;
            }
        }
        Some(self.root.join(tenant).join(format!("{name}.bin")))
    }

    /// Decode a warm file into `(version, collection)`; `None` on any framing or
    /// decode error (treated as a miss — never panics, never serves garbage).
    fn decode_entry(bytes: &[u8]) -> Option<(u64, Collection)> {
        if bytes.len() < 8 {
            return None;
        }
        let (head, body) = bytes.split_at(8);
        let version = u64::from_le_bytes(head.try_into().ok()?);
        let collection = Collection::decode(body).ok()?;
        Some((version, collection))
    }

    fn encode_entry(version: u64, collection: &Collection) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + collection.encoded_len());
        out.extend_from_slice(&version.to_le_bytes());
        collection.encode(&mut out).ok();
        out
    }

    /// Fresh enough by the mtime backstop?
    fn within_ttl(modified: Option<SystemTime>) -> bool {
        modified
            .and_then(|m| m.elapsed().ok())
            .map(|age| age < WARM_TTL)
            .unwrap_or(true) // unknown mtime ⇒ rely on the version stamp alone
    }

    async fn write_through(&self, path: &PathBuf, version: u64, collection: &Collection) {
        if let Some(parent) = path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        // Best-effort: a warm-tier write failure must never fail the read.
        if let Err(e) = tokio::fs::write(path, Self::encode_entry(version, collection)).await {
            debug!("syscat warm write {path:?} failed (non-fatal): {e}");
        }
    }
}

#[async_trait]
impl CatalogMetadataSource for WarmDiskStore {
    async fn fetch(&self, tenant_id: &str, name: &str) -> Result<Option<Collection>> {
        let want = CorpusVersionRegistry::global()
            .current(tenant_id, name)
            .await;

        let Some(path) = self.entry_path(tenant_id, name) else {
            // Unsafe key ⇒ bypass the disk tier entirely.
            return self.inner.fetch(tenant_id, name).await;
        };

        // Warm hit: file present, version stamp matches the live version, within
        // the mtime backstop.
        if let Ok(bytes) = tokio::fs::read(&path).await
            && let Some((stamped, collection)) = Self::decode_entry(&bytes)
            && stamped == want
        {
            let modified = tokio::fs::metadata(&path)
                .await
                .ok()
                .and_then(|m| m.modified().ok());
            if Self::within_ttl(modified) {
                return Ok(Some(collection));
            }
        }

        // Warm miss / stale / TTL-expired ⇒ read the canonical and refresh the
        // file (stamped with the version we resolved against).
        match self.inner.fetch(tenant_id, name).await? {
            Some(collection) => {
                self.write_through(&path, want, &collection).await;
                Ok(Some(collection))
            }
            None => {
                // The asset is gone; drop any stale warm copy.
                let _ = tokio::fs::remove_file(&path).await;
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Counting inner source so we can prove a warm hit avoids the (object-store)
    /// round-trip the inner stands in for.
    #[derive(Default)]
    struct CountingSource {
        collections: std::collections::HashMap<String, Collection>,
        fetches: AtomicUsize,
    }
    impl CountingSource {
        fn with(name: &str) -> Self {
            let mut collections = std::collections::HashMap::new();
            collections.insert(
                name.to_string(),
                Collection {
                    id: format!("uuid-{name}"),
                    ..Default::default()
                },
            );
            Self {
                collections,
                fetches: AtomicUsize::new(0),
            }
        }
        fn count(&self) -> usize {
            self.fetches.load(Ordering::SeqCst)
        }
    }
    #[async_trait]
    impl CatalogMetadataSource for CountingSource {
        async fn fetch(&self, _t: &str, name: &str) -> Result<Option<Collection>> {
            self.fetches.fetch_add(1, Ordering::SeqCst);
            Ok(self.collections.get(name).cloned())
        }
    }

    #[tokio::test]
    async fn warm_hit_after_first_read_avoids_inner_then_invalidates_on_version_bump() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (t, n) = ("default", "sc2_warm_probe");
        let src = Arc::new(CountingSource::with(n));
        let warm = WarmDiskStore::new(tmp.path().to_path_buf(), src.clone());

        // Miss → inner once, file written.
        assert_eq!(
            warm.fetch(t, n).await.unwrap().unwrap().id,
            "uuid-sc2_warm_probe"
        );
        assert_eq!(src.count(), 1);
        // Hit → served from disk, inner NOT called again.
        assert_eq!(
            warm.fetch(t, n).await.unwrap().unwrap().id,
            "uuid-sc2_warm_probe"
        );
        assert_eq!(src.count(), 1, "warm hit must not refetch the inner");

        // A write bumps the corpus version → stamp mismatches → refetch.
        CorpusVersionRegistry::global().bump(t, n).await;
        assert!(warm.fetch(t, n).await.unwrap().is_some());
        assert_eq!(src.count(), 2, "version bump must force a refetch");
    }

    #[tokio::test]
    async fn warm_file_persists_across_store_instances() {
        // Simulates a fresh process reusing the on-disk warm tier at the same
        // version (the durable-version restart benefit, TD-SC-5, in miniature).
        let tmp = tempfile::TempDir::new().unwrap();
        let (t, n) = ("default", "sc2_persist_probe");
        {
            let src = Arc::new(CountingSource::with(n));
            let warm = WarmDiskStore::new(tmp.path().to_path_buf(), src.clone());
            warm.fetch(t, n).await.unwrap();
            assert_eq!(src.count(), 1);
        }
        // New store, same dir, an inner that would PANIC if consulted (it must be
        // served from disk).
        struct Forbidden;
        #[async_trait]
        impl CatalogMetadataSource for Forbidden {
            async fn fetch(&self, _t: &str, _n: &str) -> Result<Option<Collection>> {
                panic!("inner must not be consulted on a persisted warm hit");
            }
        }
        let warm2 = WarmDiskStore::new(tmp.path().to_path_buf(), Arc::new(Forbidden));
        assert_eq!(
            warm2.fetch(t, n).await.unwrap().unwrap().id,
            "uuid-sc2_persist_probe"
        );
    }

    #[tokio::test]
    async fn unsafe_key_bypasses_disk() {
        let tmp = tempfile::TempDir::new().unwrap();
        let src = Arc::new(CountingSource::with("x"));
        let warm = WarmDiskStore::new(tmp.path().to_path_buf(), src.clone());
        // A name with a path separator must never touch disk — straight to inner.
        assert!(
            warm.fetch("default", "../etc/passwd")
                .await
                .unwrap()
                .is_none()
        );
        assert!(warm.fetch("default", "a/b").await.unwrap().is_none());
        assert_eq!(
            src.count(),
            2,
            "unsafe keys bypass disk and hit inner each time"
        );
    }
}
