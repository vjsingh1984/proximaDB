// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! # `proximadb-cks-objectstore` — the scale-to-zero object-store `ConditionalKeyStore`
//!
//! F1c (ADR-072 D5): the **serverless escape hatch** implementation of
//! [`ConditionalKeyStore`]. It is deliberately *not* a per-key fixed-slot store —
//! that pattern cannot reclaim a key race-free over immutable object storage (the
//! P2 finding). Instead it uses the **successor-slot manifest** pattern (BatchWeave
//! / Iceberg-style): the whole claim set lives in a monotonically-versioned
//! manifest object, and a commit publishes `manifest v+1` with a **create-only
//! conditional put** — exactly one writer wins the successor slot; the loser
//! rebases onto the winner's manifest and retries.
//!
//! Reclaim is append-only (D11/D14): a tombstone is an entry (`oid = None`) in a
//! *new* manifest version; a slot is never overwritten. This is the correctness-
//! carrying pattern for the immutable tier; batching many claims per commit is the
//! throughput optimization (an MVP commits one claim at a time).
//!
//! `atomic_scope()` = [`AtomicScope::PerBatchManifest`].

use anyhow::{Result, anyhow, bail};
use bytes::Bytes;
use object_store::path::Path as ObjPath;
use proximadb_object_store::ProximaObjectStore;
use proximadb_storage_ports::{
    AtomicScope, ConditionalKeyStore, Generation, Identity, Oid, PutOutcome,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One claim in the manifest. `oid = None` is a tombstone.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    oid: Option<String>,
    generation: u64,
}

/// The full claim set for a manifest version. Keys are hex-encoded [`Identity`]
/// bytes (object stores want printable, delimiter-free names).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Manifest {
    version: u64,
    entries: BTreeMap<String, Entry>,
}

/// Object-store-backed [`ConditionalKeyStore`] using successor-slot manifests.
pub struct ObjectStoreKeyStore {
    store: ProximaObjectStore,
    prefix: String,
    max_retries: usize,
}

impl ObjectStoreKeyStore {
    /// Create a store rooted at `prefix` within `store`.
    pub fn new(store: ProximaObjectStore, prefix: impl Into<String>) -> Self {
        Self {
            store,
            prefix: prefix.into(),
            max_retries: 32,
        }
    }

    /// Manifest object name for `version`. Reverse-lexicographic (`u64::MAX - v`)
    /// so a plain lexicographic `list` surfaces the latest version first (the
    /// LanceDB trick).
    fn manifest_path(&self, version: u64) -> ObjPath {
        ObjPath::from(format!(
            "{}/m/{:020}.manifest",
            self.prefix,
            u64::MAX - version
        ))
    }

    /// Load the latest manifest, or an empty one at version 0 if none exists.
    async fn load_latest(&self) -> Result<Manifest> {
        let list_prefix = ObjPath::from(format!("{}/m", self.prefix));
        let metas = self
            .store
            .list(Some(&list_prefix))
            .await
            .map_err(|e| anyhow!(e))?;
        let mut best: Option<(u64, ObjPath)> = None;
        for m in metas {
            if let Some(v) = version_of(&m.location)
                && best.as_ref().is_none_or(|(bv, _)| v > *bv)
            {
                best = Some((v, m.location));
            }
        }
        let Some((_, path)) = best else {
            return Ok(Manifest::default());
        };
        let bytes = self.store.get(&path).await.map_err(|e| anyhow!(e))?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// Publish `manifest` at its `version` via a create-only conditional put.
    /// Returns `Ok(true)` on success, `Ok(false)` if the successor slot was already
    /// taken (a concurrent writer won — the caller must rebase and retry).
    async fn commit(&self, manifest: &Manifest) -> Result<bool> {
        let bytes = Bytes::from(serde_json::to_vec(manifest)?);
        match self
            .store
            .put_if_absent(&self.manifest_path(manifest.version), bytes)
            .await
        {
            Ok(()) => Ok(true),
            Err(e) if is_already_exists(&e) => Ok(false),
            Err(e) => Err(anyhow!(e)),
        }
    }
}

fn hexkey(id: &Identity) -> String {
    let mut s = String::with_capacity(id.as_bytes().len() * 2);
    for b in id.as_bytes() {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn version_of(path: &ObjPath) -> Option<u64> {
    let name = path.filename()?.strip_suffix(".manifest")?;
    let encoded: u64 = name.parse().ok()?;
    Some(u64::MAX - encoded)
}

fn is_already_exists(e: &proximadb_kernel::error::StorageError) -> bool {
    matches!(e, proximadb_kernel::error::StorageError::AlreadyExists(_))
}

#[async_trait::async_trait]
impl ConditionalKeyStore for ObjectStoreKeyStore {
    fn atomic_scope(&self) -> AtomicScope {
        AtomicScope::PerBatchManifest
    }

    async fn put_if_absent(
        &self,
        key: &Identity,
        oid: &Oid,
        fence: Generation,
    ) -> Result<PutOutcome> {
        let hk = hexkey(key);
        for _ in 0..self.max_retries {
            let mut m = self.load_latest().await?;
            if let Some(e) = m.entries.get(&hk)
                && let Some(holder) = &e.oid
            {
                return Ok(PutOutcome::Conflict {
                    holder: Oid(holder.clone()),
                    generation: Generation(e.generation),
                });
            }
            m.version += 1;
            m.entries.insert(
                hk.clone(),
                Entry {
                    oid: Some(oid.0.clone()),
                    generation: fence.0,
                },
            );
            if self.commit(&m).await? {
                return Ok(PutOutcome::Committed { generation: fence });
            }
            // Successor slot taken -> rebase onto the new latest and retry.
        }
        bail!(
            "object-store CKS: put_if_absent exceeded {} rebase retries under contention",
            self.max_retries
        )
    }

    async fn get(&self, key: &Identity) -> Result<Option<Oid>> {
        let m = self.load_latest().await?;
        Ok(m.entries
            .get(&hexkey(key))
            .and_then(|e| e.oid.clone())
            .map(Oid))
    }

    async fn tombstone(&self, key: &Identity, fence: Generation) -> Result<()> {
        let hk = hexkey(key);
        for _ in 0..self.max_retries {
            let mut m = self.load_latest().await?;
            let live = m.entries.get(&hk).map(|e| e.oid.is_some()).unwrap_or(false);
            if !live {
                return Ok(());
            }
            m.version += 1;
            m.entries.insert(
                hk.clone(),
                Entry {
                    oid: None,
                    generation: fence.0,
                },
            );
            if self.commit(&m).await? {
                return Ok(());
            }
        }
        bail!(
            "object-store CKS: tombstone exceeded {} rebase retries under contention",
            self.max_retries
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use object_store::memory::InMemory;
    use std::sync::Arc;

    fn store() -> Arc<InMemory> {
        Arc::new(InMemory::new())
    }
    fn cks(backing: Arc<InMemory>) -> ObjectStoreKeyStore {
        ObjectStoreKeyStore::new(ProximaObjectStore::new(backing), "t/users")
    }
    fn id(s: &str) -> Identity {
        Identity::from_bytes(s.as_bytes().to_vec())
    }

    #[tokio::test]
    async fn lifecycle_over_object_store() {
        let backing = store();
        let s = cks(backing.clone());
        let a = Oid("row-a".into());
        let b = Oid("row-b".into());

        assert_eq!(
            s.put_if_absent(&id("k"), &a, Generation(1)).await.unwrap(),
            PutOutcome::Committed {
                generation: Generation(1)
            }
        );
        // Conflict returns the holder.
        assert_eq!(
            s.put_if_absent(&id("k"), &b, Generation(2)).await.unwrap(),
            PutOutcome::Conflict {
                holder: a.clone(),
                generation: Generation(1)
            }
        );
        assert_eq!(s.get(&id("k")).await.unwrap(), Some(a));
        // Append-only reclaim: tombstone then re-insert is a new manifest version.
        s.tombstone(&id("k"), Generation(3)).await.unwrap();
        assert_eq!(s.get(&id("k")).await.unwrap(), None);
        assert_eq!(
            s.put_if_absent(&id("k"), &b, Generation(4)).await.unwrap(),
            PutOutcome::Committed {
                generation: Generation(4)
            }
        );
        assert_eq!(s.get(&id("k")).await.unwrap(), Some(b));
    }

    #[tokio::test]
    async fn state_is_durable_in_the_object_store() {
        let backing = store();
        {
            let s = cks(backing.clone());
            s.put_if_absent(&id("k"), &Oid("v".into()), Generation(1))
                .await
                .unwrap();
        }
        // A fresh store over the same backing object store sees the committed manifest.
        let s2 = cks(backing.clone());
        assert_eq!(s2.get(&id("k")).await.unwrap(), Some(Oid("v".into())));
        match s2
            .put_if_absent(&id("k"), &Oid("other".into()), Generation(9))
            .await
            .unwrap()
        {
            PutOutcome::Conflict { holder, .. } => assert_eq!(holder, Oid("v".into())),
            other => panic!("expected conflict, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn scope_is_per_batch_manifest() {
        assert_eq!(cks(store()).atomic_scope(), AtomicScope::PerBatchManifest);
    }
}
