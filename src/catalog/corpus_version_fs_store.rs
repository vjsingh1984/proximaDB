// File-backed CorpusVersionStore — first concrete durable backend.
//
// Persists the `(tenant_id, collection) → version` map as a single
// JSON file at a configured path. Suitable for single-node
// deployments and as a reference impl that other backends (catalog
// columns, KV stores) can model their durability after.
//
// Durability shape:
//   - One file per registry. Atomic writes via temp-file + rename so a
//     crash mid-write can never produce a partial file the next
//     `load_all` would parse incorrectly.
//   - JSON format: `[{"tenant_id": "...", "collection": "...",
//     "version": 42}, ...]`. Easy to inspect, easy to back up, easy
//     to migrate to a catalog-row backend later (the shape maps
//     directly to a table).
//   - Each `persist` call writes the FULL map. For the expected
//     scale (≤10k (tenant, collection) pairs) this is cheap; for
//     larger fleets a backend that supports per-row writes is
//     preferable. The CorpusVersionStore trait doesn't force the
//     full-map shape — backends can stream individual rows.
//
// Failure semantics match the trait contract: `load_all` returns
// empty on a fresh install (no file), `persist` errors propagate
// up where the registry logs them. A corrupted file is treated as
// missing — the registry starts empty and bumps re-populate the
// file on the next successful persist.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::catalog::corpus_version::CorpusVersionStore;

/// One row in the JSON-on-disk shape. `serde` handles the
/// round-trip; the shape is intentionally flat so a future migration
/// to a catalog-row backend is a column-for-field mapping.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedRow {
    tenant_id: String,
    collection: String,
    version: u64,
}

/// File-backed implementation of `CorpusVersionStore`.
///
/// Cheap to clone — internal state is wrapped in `Arc<Mutex<...>>`
/// so concurrent persists serialize at the file write rather than
/// the trait surface.
#[derive(Debug, Clone)]
pub struct FileSystemCorpusVersionStore {
    path: PathBuf,
    /// Cached in-memory map. Persisted as a full snapshot on every
    /// `persist`. Held under a Mutex so concurrent persists can't
    /// race a partial update — the write lock covers both the
    /// in-memory update and the file write.
    map: Arc<Mutex<HashMap<(String, String), u64>>>,
}

impl FileSystemCorpusVersionStore {
    /// Build a store rooted at `path`. The file isn't read until
    /// `load_all` is called — construction is cheap and infallible
    /// so the server bootstrap can build the store before deciding
    /// whether to use it.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            map: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// The path the store reads + writes.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Atomic write: serialize the map to a sibling temp file, then
    /// rename over the target. The rename is the only atomic primitive
    /// POSIX guarantees, so this is the smallest reliable durability
    /// pattern. On Windows the rename is also atomic since Rust 1.5.
    async fn write_snapshot(&self, snapshot: &HashMap<(String, String), u64>) -> anyhow::Result<()> {
        let rows: Vec<PersistedRow> = snapshot
            .iter()
            .map(|((t, c), v)| PersistedRow {
                tenant_id: t.clone(),
                collection: c.clone(),
                version: *v,
            })
            .collect();
        let bytes = serde_json::to_vec_pretty(&rows)
            .context("serialize corpus_version snapshot")?;

        // Ensure parent dir exists. The store may be constructed
        // before any directory layout is set up.
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| {
                    format!("create parent directory {}", parent.display())
                })?;
        }

        // Unique temp-file name per persist call so concurrent writes
        // can't collide on a shared `.tmp` name. Each writer renames
        // its own private temp file over the target atomically; the
        // last rename wins, but no intermediate writer ever sees a
        // missing temp file because of another writer's rename.
        let tmp_path = {
            let mut p = self.path.clone();
            let file_name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "corpus_versions.json".to_string());
            // Nanos + thread id + an unlikely-to-collide counter
            // shard. Cheap and good enough for inter-process safety
            // on the millisecond scale.
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let tid = format!("{:?}", std::thread::current().id());
            p.set_file_name(format!(".{file_name}.{stamp}.{tid}.tmp"));
            p
        };

        tokio::fs::write(&tmp_path, &bytes)
            .await
            .with_context(|| format!("write tmp file {}", tmp_path.display()))?;
        tokio::fs::rename(&tmp_path, &self.path)
            .await
            .with_context(|| {
                format!(
                    "atomic rename {} → {}",
                    tmp_path.display(),
                    self.path.display()
                )
            })?;
        Ok(())
    }
}

#[async_trait]
impl CorpusVersionStore for FileSystemCorpusVersionStore {
    async fn load_all(&self) -> anyhow::Result<HashMap<(String, String), u64>> {
        let bytes = match tokio::fs::read(&self.path).await {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Fresh install — no file yet. This isn't a failure.
                return Ok(HashMap::new());
            }
            Err(e) => {
                return Err(anyhow::Error::new(e).context(format!(
                    "read corpus_version file {}",
                    self.path.display()
                )));
            }
        };

        // Corruption treated as missing: log via the returned Result
        // shape (the registry's hydrate path logs and continues with
        // an empty registry). A malformed file gets rewritten on the
        // next successful persist, so corruption is self-healing.
        let rows: Vec<PersistedRow> = match serde_json::from_slice(&bytes) {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!(
                    path = %self.path.display(),
                    error = %e,
                    "corpus_version file unparseable; treating as empty (will rewrite on next persist)"
                );
                return Ok(HashMap::new());
            }
        };

        let map: HashMap<(String, String), u64> = rows
            .into_iter()
            .map(|r| ((r.tenant_id, r.collection), r.version))
            .collect();

        // Seed the in-memory cache so subsequent persists carry the
        // full map (not just the one row being updated).
        *self.map.lock().await = map.clone();
        Ok(map)
    }

    async fn persist(
        &self,
        tenant_id: &str,
        collection: &str,
        version: u64,
    ) -> anyhow::Result<()> {
        // Hold the mutex through both the map update and the file
        // write. This serializes concurrent persists so a slow writer
        // can't overwrite a faster writer's already-completed file.
        // Throughput cost is acceptable for the corpus_version bump
        // rate (low single-digit Hz per collection); correctness
        // dominates.
        let mut guard = self.map.lock().await;
        guard.insert((tenant_id.to_string(), collection.to_string()), version);
        let snapshot = guard.clone();
        self.write_snapshot(&snapshot).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn tmp_store() -> (TempDir, FileSystemCorpusVersionStore) {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("corpus_versions.json");
        let store = FileSystemCorpusVersionStore::new(path);
        (dir, store)
    }

    #[tokio::test]
    async fn fresh_install_load_returns_empty_map() {
        let (_dir, store) = tmp_store();
        let m = store.load_all().await.unwrap();
        assert!(m.is_empty(), "fresh install has no file → empty map");
    }

    #[tokio::test]
    async fn persist_creates_the_file() {
        let (_dir, store) = tmp_store();
        assert!(!store.path().exists());
        store.persist("tenant-a", "kb", 42).await.unwrap();
        assert!(store.path().exists(), "persist must create the file");
    }

    #[tokio::test]
    async fn persist_then_load_round_trips_single_row() {
        let (_dir, store) = tmp_store();
        store.persist("tenant-a", "kb", 42).await.unwrap();
        let m = store.load_all().await.unwrap();
        assert_eq!(m.get(&("tenant-a".into(), "kb".into())), Some(&42));
    }

    #[tokio::test]
    async fn persist_accumulates_multiple_rows() {
        let (_dir, store) = tmp_store();
        store.persist("tenant-a", "kb", 10).await.unwrap();
        store.persist("tenant-a", "logs", 20).await.unwrap();
        store.persist("tenant-b", "kb", 30).await.unwrap();
        let m = store.load_all().await.unwrap();
        assert_eq!(m.len(), 3);
        assert_eq!(m.get(&("tenant-a".into(), "kb".into())), Some(&10));
        assert_eq!(m.get(&("tenant-a".into(), "logs".into())), Some(&20));
        assert_eq!(m.get(&("tenant-b".into(), "kb".into())), Some(&30));
    }

    #[tokio::test]
    async fn persist_updates_existing_row() {
        let (_dir, store) = tmp_store();
        store.persist("tenant-a", "kb", 1).await.unwrap();
        store.persist("tenant-a", "kb", 5).await.unwrap();
        store.persist("tenant-a", "kb", 100).await.unwrap();
        let m = store.load_all().await.unwrap();
        // Only the latest write survives.
        assert_eq!(m.get(&("tenant-a".into(), "kb".into())), Some(&100));
        assert_eq!(m.len(), 1, "single key, not three rows");
    }

    #[tokio::test]
    async fn corrupted_file_returns_empty_and_is_self_healing() {
        let (_dir, store) = tmp_store();
        // Hand-write a malformed JSON file to the store's path.
        tokio::fs::write(store.path(), b"this is not json").await.unwrap();
        // load_all logs + returns empty (no panic, no propagated error).
        let m = store.load_all().await.unwrap();
        assert!(m.is_empty(), "corrupt file → empty load");
        // A subsequent persist overwrites the corrupted file cleanly.
        store.persist("tenant-a", "kb", 7).await.unwrap();
        let m2 = store.load_all().await.unwrap();
        assert_eq!(m2.get(&("tenant-a".into(), "kb".into())), Some(&7));
    }

    #[tokio::test]
    async fn atomic_rename_does_not_leave_tmp_file_in_steady_state() {
        let (dir, store) = tmp_store();
        store.persist("tenant-a", "kb", 1).await.unwrap();
        store.persist("tenant-a", "kb", 2).await.unwrap();
        // After successful persists the temp file is gone (rename
        // moved it to the target). Walk the directory to confirm
        // only the target file exists.
        let mut entries = tokio::fs::read_dir(dir.path()).await.unwrap();
        let mut files = Vec::new();
        while let Some(e) = entries.next_entry().await.unwrap() {
            files.push(e.file_name().to_string_lossy().into_owned());
        }
        assert_eq!(files.len(), 1, "only the target file should remain");
        assert_eq!(files[0], "corpus_versions.json");
    }

    #[tokio::test]
    async fn nested_directory_is_created_on_first_persist() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested/deeper/corpus.json");
        let store = FileSystemCorpusVersionStore::new(path.clone());
        // No `nested/` directory yet. persist() must create it.
        assert!(!path.parent().unwrap().exists());
        store.persist("tenant-a", "kb", 1).await.unwrap();
        assert!(path.exists());
    }

    #[tokio::test]
    async fn load_seeds_in_memory_cache_for_subsequent_persists() {
        // Persist three rows, drop the store, reconstruct, load,
        // persist a NEW row — the load_all must seed the in-memory
        // cache so the next persist carries all four rows, not just
        // the new one (otherwise the previous three would be lost).
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("corpus.json");
        let s1 = FileSystemCorpusVersionStore::new(path.clone());
        s1.persist("tenant-a", "kb", 1).await.unwrap();
        s1.persist("tenant-a", "logs", 2).await.unwrap();
        s1.persist("tenant-b", "kb", 3).await.unwrap();

        // New store handle on the same path — simulates a server
        // restart.
        let s2 = FileSystemCorpusVersionStore::new(path.clone());
        let loaded = s2.load_all().await.unwrap();
        assert_eq!(loaded.len(), 3);
        // Now persist a new row.
        s2.persist("tenant-c", "kb", 4).await.unwrap();

        // A third store reads the file fresh and must see all four.
        let s3 = FileSystemCorpusVersionStore::new(path);
        let final_loaded = s3.load_all().await.unwrap();
        assert_eq!(final_loaded.len(), 4, "all four rows present");
        assert_eq!(
            final_loaded.get(&("tenant-c".into(), "kb".into())),
            Some(&4)
        );
        // The pre-existing rows survived.
        assert_eq!(
            final_loaded.get(&("tenant-a".into(), "kb".into())),
            Some(&1)
        );
    }

    #[tokio::test]
    async fn path_accessor_returns_constructed_path() {
        let (dir, store) = tmp_store();
        assert_eq!(store.path(), &dir.path().join("corpus_versions.json"));
    }

    #[tokio::test]
    async fn json_shape_is_stable_and_inspectable() {
        // Pin the JSON shape so external tooling (operator scripts,
        // migration tools) can parse it without reading Rust source.
        let (_dir, store) = tmp_store();
        store.persist("tenant-a", "kb", 42).await.unwrap();
        let bytes = tokio::fs::read(store.path()).await.unwrap();
        let s = std::str::from_utf8(&bytes).unwrap();
        // Expected shape: a top-level array of objects with
        // {tenant_id, collection, version}.
        assert!(s.contains("\"tenant_id\""));
        assert!(s.contains("\"collection\""));
        assert!(s.contains("\"version\""));
        assert!(s.contains("\"tenant-a\""));
        assert!(s.contains("\"kb\""));
        assert!(s.contains("42"));
    }

    #[tokio::test]
    async fn concurrent_persists_serialize_at_the_file_write() {
        // Multiple concurrent persists must produce a consistent
        // final file — no race where one persist overwrites another's
        // changes. Spawn 20 concurrent persists on distinct keys.
        let (_dir, store) = tmp_store();
        let store = Arc::new(store);
        let mut handles = Vec::new();
        for i in 0..20 {
            let s = store.clone();
            handles.push(tokio::spawn(async move {
                s.persist("tenant-a", &format!("coll-{i}"), i as u64)
                    .await
                    .unwrap();
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        let m = store.load_all().await.unwrap();
        // All 20 distinct keys present.
        assert_eq!(m.len(), 20);
    }
}
