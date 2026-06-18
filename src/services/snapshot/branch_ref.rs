//! Branch refs — object-store-metadata copy-on-write branching (TD-117).
//!
//! A [`BranchRef`] is one small immutable object that pins a point in history:
//! a per-collection WAL fork LSN and a per-table manifest version, plus an
//! optional catalog snapshot id. Creating a branch writes *only* this ref —
//! immutable WAL layers and manifest data files are shared with the parent by
//! reference until the branch diverges, so a branch is an O(1), size-independent
//! metadata operation (the Neon/Iceberg copy-on-write model; see design doc §6).
//!
//! This module is the foundation: the ref type, its on-store location, and the
//! assembly logic. The live read-path *ancestor fall-through* and the
//! generation-fenced commit wiring are tracked as TD-117 follow-ups; the
//! generation carried here is what feeds
//! [`ManifestCommitter::commit_fenced`](../../../../proximadb_iceberg_engine/manifest/struct.ManifestCommitter.html).

use std::collections::BTreeMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::storage::trait_components::path_resolver::DrResolvedPath;

/// An immutable, object-store-persisted pointer to a point in a collection's
/// (and its tables') history, forming the root of a copy-on-write branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchRef {
    /// Unique branch identifier (also the object file stem: `<id>.json`).
    pub branch_id: String,
    /// Parent branch id, or `None` for a root branch.
    pub parent: Option<String>,
    /// Per-collection WAL fork point: reads of unmodified records at or below
    /// this LSN fall through to the parent (fall-through is a TD-117 follow-up).
    pub collections: BTreeMap<String, u64>,
    /// Per-table manifest version shared (copy-on-write) from the parent.
    pub tables: BTreeMap<String, u64>,
    /// Optional catalog snapshot id pinning the schema consistent with the data.
    pub catalog_snapshot_id: Option<String>,
    /// Monotonic writer generation — strictly greater than the parent's. Fed to
    /// the manifest committer's generation fence to reject stale writers.
    pub generation: u64,
    /// Wall-clock creation time, nanoseconds since the Unix epoch.
    pub created_at_ns: i64,
}

impl BranchRef {
    /// Assemble a branch ref forking from `parent` (or a root branch when `None`).
    ///
    /// The generation is `parent.generation + 1` (or `0` for a root), giving a
    /// strictly-increasing single-writer fence per branch lineage.
    pub fn create_branch(
        branch_id: impl Into<String>,
        parent: Option<&BranchRef>,
        collections: BTreeMap<String, u64>,
        tables: BTreeMap<String, u64>,
        catalog_snapshot_id: Option<String>,
        created_at_ns: i64,
    ) -> Self {
        let generation = parent.map_or(0, |p| p.generation.saturating_add(1));
        Self {
            branch_id: branch_id.into(),
            parent: parent.map(|p| p.branch_id.clone()),
            collections,
            tables,
            catalog_snapshot_id,
            generation,
            created_at_ns,
        }
    }

    /// Serialize to the canonical JSON bytes persisted in object storage.
    pub fn to_json_bytes(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec_pretty(self)?)
    }

    /// Deserialize a branch ref from its JSON bytes.
    pub fn from_json_bytes(bytes: &[u8]) -> Result<Self> {
        Ok(serde_json::from_slice(bytes)?)
    }

    /// Object-store key for this ref under a resolved collection path,
    /// `data/<tenant>/<ns>/<collection>/_branches/<branch_id>.json`.
    pub fn object_key(&self, resolved: &DrResolvedPath) -> String {
        format!("{}{}.json", resolved.branches_subprefix(), self.branch_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::trait_components::path_resolver::DrPathBuilder;

    fn coll(pairs: &[(&str, u64)]) -> BTreeMap<String, u64> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn create_branch_root_then_child_increments_generation() {
        let root = BranchRef::create_branch(
            "main",
            None,
            coll(&[("docs", 100), ("vectors", 100)]),
            coll(&[("events", 5)]),
            Some("cat:v1".to_string()),
            1_000,
        );
        assert_eq!(root.generation, 0);
        assert_eq!(root.parent, None);
        assert_eq!(root.collections["docs"], 100);
        assert_eq!(root.tables["events"], 5);

        let child = BranchRef::create_branch(
            "agent-42",
            Some(&root),
            root.collections.clone(),
            root.tables.clone(),
            root.catalog_snapshot_id.clone(),
            2_000,
        );
        assert_eq!(child.generation, 1, "child generation > parent");
        assert_eq!(child.parent.as_deref(), Some("main"));
        // Fork points are inherited by reference (shared until divergence).
        assert_eq!(child.collections["vectors"], 100);
    }

    #[test]
    fn branch_ref_json_round_trips() {
        let branch = BranchRef::create_branch(
            "exp-7",
            None,
            coll(&[("docs", 42)]),
            coll(&[("users", 9)]),
            None,
            123,
        );
        let bytes = branch.to_json_bytes().expect("serialize");
        let decoded = BranchRef::from_json_bytes(&bytes).expect("deserialize");
        assert_eq!(branch, decoded);
    }

    #[test]
    fn object_key_lives_under_branches_subprefix() {
        // build_from_parts is request-tenant-authoritative and validates each segment.
        let resolved =
            DrPathBuilder::build_from_parts("tenant-a", "ns-1", "coll-1", Default::default())
                .expect("resolved path");

        let branch =
            BranchRef::create_branch("br-xyz", None, BTreeMap::new(), BTreeMap::new(), None, 0);
        let key = branch.object_key(&resolved);
        assert!(key.ends_with("/_branches/br-xyz.json"), "key = {key}");
        assert!(key.contains("/tenant-a/ns-1/coll-1/"));
    }
}
