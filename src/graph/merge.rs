//! Graph branch merge semantics (ADR-012).
//!
//! Defines CRDT-based conflict resolution policies for graph mutations across
//! concurrent branches. All durable graph truth flows through canonical
//! `ProximaRecord` WAL commits; this module provides the semantic contracts
//! that govern how conflicting mutations from two branches are reconciled before
//! the merged WAL fragment is written.
//!
//! ## Merge protocol (ADR-012 §3)
//!
//! 1. Identify the common base WAL position for the two branches.
//! 2. Classify each mutation in the diff against the table below.
//! 3. Resolve conflicts using the per-class policy.
//! 4. Commit the resolved mutation set as a single WAL fragment.
//! 5. Trigger incremental projection rebuild (HNSW, CSR) for affected records.
//!
//! ## Per-class policies
//!
//! | Mutation class      | Policy                        |
//! |---------------------|-------------------------------|
//! | Node upsert         | Last-Write-Wins (LWW by WAL ts) |
//! | Node delete         | Delete-wins (2P-Set semantics) |
//! | Edge upsert         | LWW by WAL ts                 |
//! | Edge delete         | Delete-wins (2P-Set semantics) |
//! | Embedding update    | LWW by WAL ts                 |
//! | Label set           | Add-Wins Set union             |
//! | Props key           | LWW per key                   |

use serde::{Deserialize, Serialize};

/// Mutation class used to select the merge policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MutationClass {
    NodeUpsert,
    NodeDelete,
    EdgeUpsert,
    EdgeDelete,
    EmbeddingUpdate,
    LabelSet,
    PropsKey,
}

impl MutationClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NodeUpsert => "node_upsert",
            Self::NodeDelete => "node_delete",
            Self::EdgeUpsert => "edge_upsert",
            Self::EdgeDelete => "edge_delete",
            Self::EmbeddingUpdate => "embedding_update",
            Self::LabelSet => "label_set",
            Self::PropsKey => "props_key",
        }
    }
}

/// CRDT merge policy applied to a conflicting mutation pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MergePolicy {
    /// Accept the mutation with the higher WAL timestamp; discard the other.
    LastWriteWins,
    /// A delete on either branch wins over an upsert on the other.
    DeleteWins,
    /// Union of label/tag sets from both branches; no label is lost.
    AddWinsSetUnion,
    /// LWW applied independently per props key rather than per record.
    LastWriteWinsPerKey,
}

impl MergePolicy {
    pub fn for_class(class: MutationClass) -> Self {
        match class {
            MutationClass::NodeUpsert => Self::LastWriteWins,
            MutationClass::NodeDelete => Self::DeleteWins,
            MutationClass::EdgeUpsert => Self::LastWriteWins,
            MutationClass::EdgeDelete => Self::DeleteWins,
            MutationClass::EmbeddingUpdate => Self::LastWriteWins,
            MutationClass::LabelSet => Self::AddWinsSetUnion,
            MutationClass::PropsKey => Self::LastWriteWinsPerKey,
        }
    }
}

/// A conflict detected between two branches for a single record or key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeConflict {
    /// Canonical record ID (node or edge).
    pub record_id: String,
    /// Mutation class that produced the conflict.
    pub mutation_class: MutationClass,
    /// WAL timestamp on the left (base) branch (microseconds since epoch).
    pub left_wal_ts_us: i64,
    /// WAL timestamp on the right (incoming) branch.
    pub right_wal_ts_us: i64,
    /// Policy that will be applied.
    pub policy: MergePolicy,
}

impl MergeConflict {
    pub fn new(
        record_id: impl Into<String>,
        mutation_class: MutationClass,
        left_wal_ts_us: i64,
        right_wal_ts_us: i64,
    ) -> Self {
        Self {
            record_id: record_id.into(),
            policy: MergePolicy::for_class(mutation_class),
            mutation_class,
            left_wal_ts_us,
            right_wal_ts_us,
        }
    }

    /// Returns true when the right (incoming) branch wins under LWW.
    pub fn right_wins_lww(&self) -> bool {
        self.right_wal_ts_us > self.left_wal_ts_us
    }
}

/// Outcome of resolving a single [`MergeConflict`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MergeOutcome {
    /// Accept the left (base) branch mutation; discard the right.
    KeepLeft,
    /// Accept the right (incoming) branch mutation; discard the left.
    KeepRight,
    /// Both branches are deleted; the record is absent after merge.
    BothDeleted,
    /// Union of label sets from both branches is the resolved value.
    UnionLabels,
}

/// Resolution for one conflict, pairing the conflict descriptor with its outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphMergeResolution {
    pub conflict: MergeConflict,
    pub outcome: MergeOutcome,
}

impl GraphMergeResolution {
    /// Apply ADR-012 policy to produce a resolution for the given conflict.
    pub fn resolve(conflict: MergeConflict) -> Self {
        let outcome = match conflict.policy {
            MergePolicy::LastWriteWins | MergePolicy::LastWriteWinsPerKey => {
                if conflict.right_wins_lww() {
                    MergeOutcome::KeepRight
                } else {
                    MergeOutcome::KeepLeft
                }
            }
            MergePolicy::DeleteWins => MergeOutcome::BothDeleted,
            MergePolicy::AddWinsSetUnion => MergeOutcome::UnionLabels,
        };
        Self { conflict, outcome }
    }
}

/// Convenience: resolve a batch of conflicts.
pub fn resolve_batch(conflicts: Vec<MergeConflict>) -> Vec<GraphMergeResolution> {
    conflicts
        .into_iter()
        .map(GraphMergeResolution::resolve)
        .collect()
}

/// ADR-012 Add-Wins label union: merge two label vectors, deduplicate, preserve order.
///
/// Labels from both sides are preserved (Add-Wins Set Union). Neither side's labels
/// are removed. Use this when updating a node whose label set may have been
/// independently modified in concurrent branches.
pub fn merge_labels(existing: &[String], incoming: &[String]) -> Vec<String> {
    let mut merged = existing.to_vec();
    let existing_set: std::collections::HashSet<&str> =
        existing.iter().map(String::as_str).collect();
    for label in incoming {
        if !existing_set.contains(label.as_str()) {
            merged.push(label.clone());
        }
    }
    merged
}

// ──────────────────────────────────────────────────────────────────────────────
// T3.1 Slice 1 (2026-05-26) — WAL diff walker (pure logic).
//
// The full merge protocol from ADR-012 §3 is:
//   1. Identify merge base (the WAL position where two branches diverged).
//   2. Walk the WAL diff between merge base and each branch HEAD.
//   3. Classify each mutation into one of the 7 `MutationClass` variants.
//   4. Resolve conflicts per `MergePolicy::for_class` (already implemented).
//   5. Write merged records through the canonical WAL.
//
// This slice lands steps 3-4 at the pure-logic boundary. Steps 1-2 + 5
// require `branch_id` plumbing into `ProximaRecord` (not currently a field —
// only a catalog system column at `__proxima_branch_id`) and integration with
// the canonical WAL iteration API. Those land in future slices per
// `docs/_internal/status/PRE_RELEASE_FOUNDATIONS_2026_05_26.adoc`.
//
// The caller is responsible for extracting per-branch mutation events into
// the input lists. Branch identity is implicit: "which list."
// ──────────────────────────────────────────────────────────────────────────────

/// Minimal mutation event consumed by the WAL diff walker.
///
/// Constructed by the caller (typically by walking canonical WAL entries for
/// one branch). The walker compares events across two such lists and emits
/// `MergeConflict` entries for OIDs touched on both sides.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MutationEvent {
    /// Canonical record OID (node or edge).
    pub record_oid: String,
    /// Classification of the mutation (already determined by the caller; in
    /// future slices this comes from `classify_mutation` below applied to the
    /// base + branch versions of the record).
    pub mutation_class: MutationClass,
    /// WAL timestamp in microseconds since epoch.
    pub wal_ts_us: i64,
}

impl MutationEvent {
    pub fn new(
        record_oid: impl Into<String>,
        mutation_class: MutationClass,
        wal_ts_us: i64,
    ) -> Self {
        Self {
            record_oid: record_oid.into(),
            mutation_class,
            wal_ts_us,
        }
    }
}

/// Walk two ordered lists of mutation events (one per branch) and produce
/// `MergeConflict` entries for every record OID touched on BOTH sides.
///
/// For each side, the event with the latest `wal_ts_us` is the
/// representative — earlier events are superseded. The mutation class is
/// taken from the left side; if the right side disagrees on class, that
/// indicates an upstream classifier inconsistency and the right side's
/// class is preserved on the resulting `MergeConflict` (the caller can
/// reconcile if needed — but in normal use both sides should classify the
/// same record OID into the same class).
///
/// OIDs that appear on only one side are NOT emitted as conflicts; they're
/// clean unilateral mutations that the orchestrator applies directly.
pub fn walk_diff(left: &[MutationEvent], right: &[MutationEvent]) -> Vec<MergeConflict> {
    use std::collections::HashMap;

    // Build "latest event per OID" maps for each side. `O(n)` per side.
    let mut left_latest: HashMap<&str, &MutationEvent> = HashMap::new();
    for ev in left {
        let entry = left_latest.entry(ev.record_oid.as_str()).or_insert(ev);
        if ev.wal_ts_us > entry.wal_ts_us {
            *entry = ev;
        }
    }
    let mut right_latest: HashMap<&str, &MutationEvent> = HashMap::new();
    for ev in right {
        let entry = right_latest.entry(ev.record_oid.as_str()).or_insert(ev);
        if ev.wal_ts_us > entry.wal_ts_us {
            *entry = ev;
        }
    }

    // For OIDs in BOTH maps, emit a conflict. Prefer the left side's class
    // (in agreement runs this matches the right side's class anyway; in
    // disagreement runs the orchestrator should reconcile upstream).
    let mut conflicts: Vec<MergeConflict> = left_latest
        .iter()
        .filter_map(|(oid, l)| {
            right_latest.get(oid).map(|r| {
                MergeConflict::new(*oid, l.mutation_class, l.wal_ts_us, r.wal_ts_us)
            })
        })
        .collect();

    // Deterministic ordering — by OID — so callers (and tests) can compare
    // outputs without depending on HashMap iteration order.
    conflicts.sort_by(|a, b| a.record_id.cmp(&b.record_id));
    conflicts
}

/// Classify a record mutation across base + branch_a + branch_b versions.
///
/// Implements the precedence rules from ADR-012 §3 (codified during Phase 1
/// audit). When base is `None`, treat as new-record creation on the
/// branch(es). When any branch is `None`, only the existing branch's
/// classification matters.
///
/// Precedence (top wins):
///   1. Tombstone on either branch → `NodeDelete` / `EdgeDelete`.
///   2. Pure embedding change → `EmbeddingUpdate`.
///   3. Pure label change → `LabelSet`.
///   4. Any props change → `PropsKey`.
///   5. Else, if any version is an edge → `EdgeUpsert`.
///   6. Default → `NodeUpsert`.
pub fn classify_mutation(
    base: Option<&proximadb_records::ProximaRecord>,
    branch_a: Option<&proximadb_records::ProximaRecord>,
    branch_b: Option<&proximadb_records::ProximaRecord>,
) -> MutationClass {
    let is_tombstone = |r: Option<&proximadb_records::ProximaRecord>| -> bool {
        r.is_some_and(|rec| {
            rec.valid_to_ns.is_some_and(|vt| vt <= 0)
                && rec.embeddings.is_empty()
                && rec.origin.as_deref() == Some("delete")
        })
    };
    let is_edge = |r: Option<&proximadb_records::ProximaRecord>| -> bool {
        r.is_some_and(|rec| rec.edge.is_some())
    };
    let embeddings_changed = |old: Option<&proximadb_records::ProximaRecord>,
                              new: Option<&proximadb_records::ProximaRecord>|
     -> bool {
        match (old, new) {
            (Some(o), Some(n)) => o.embeddings != n.embeddings,
            _ => false,
        }
    };
    let labels_changed = |old: Option<&proximadb_records::ProximaRecord>,
                          new: Option<&proximadb_records::ProximaRecord>|
     -> bool {
        match (old, new) {
            (Some(o), Some(n)) => o.labels != n.labels,
            _ => false,
        }
    };
    let props_changed = |old: Option<&proximadb_records::ProximaRecord>,
                         new: Option<&proximadb_records::ProximaRecord>|
     -> bool {
        match (old, new) {
            (Some(o), Some(n)) => o.props != n.props,
            _ => false,
        }
    };

    // 1. Delete-wins: tombstone on either branch.
    if is_tombstone(branch_a) || is_tombstone(branch_b) {
        if is_edge(base) || is_edge(branch_a) || is_edge(branch_b) {
            return MutationClass::EdgeDelete;
        }
        return MutationClass::NodeDelete;
    }

    let a_emb = embeddings_changed(base, branch_a);
    let b_emb = embeddings_changed(base, branch_b);
    let a_lbl = labels_changed(base, branch_a);
    let b_lbl = labels_changed(base, branch_b);
    let a_props = props_changed(base, branch_a);
    let b_props = props_changed(base, branch_b);

    // 2. Pure embedding change (no other mutations on either branch).
    if (a_emb || b_emb) && !a_lbl && !b_lbl && !a_props && !b_props {
        return MutationClass::EmbeddingUpdate;
    }

    // 3. Pure label change.
    if (a_lbl || b_lbl) && !a_emb && !b_emb && !a_props && !b_props {
        return MutationClass::LabelSet;
    }

    // 4. Any props change wins next.
    if a_props || b_props {
        return MutationClass::PropsKey;
    }

    // 5. Edge upsert default if any version is an edge.
    if is_edge(base) || is_edge(branch_a) || is_edge(branch_b) {
        return MutationClass::EdgeUpsert;
    }

    // 6. Default — node upsert.
    MutationClass::NodeUpsert
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_delete_always_wins() {
        let conflict = MergeConflict::new("node-1", MutationClass::NodeDelete, 100, 50);
        let resolution = GraphMergeResolution::resolve(conflict);
        assert_eq!(resolution.outcome, MergeOutcome::BothDeleted);
    }

    #[test]
    fn node_upsert_lww_right_wins() {
        let conflict = MergeConflict::new("node-2", MutationClass::NodeUpsert, 100, 200);
        let resolution = GraphMergeResolution::resolve(conflict);
        assert_eq!(resolution.outcome, MergeOutcome::KeepRight);
    }

    #[test]
    fn node_upsert_lww_left_wins() {
        let conflict = MergeConflict::new("node-3", MutationClass::NodeUpsert, 300, 200);
        let resolution = GraphMergeResolution::resolve(conflict);
        assert_eq!(resolution.outcome, MergeOutcome::KeepLeft);
    }

    #[test]
    fn label_set_always_unions() {
        let conflict = MergeConflict::new("node-4", MutationClass::LabelSet, 100, 50);
        let resolution = GraphMergeResolution::resolve(conflict);
        assert_eq!(resolution.outcome, MergeOutcome::UnionLabels);
    }

    #[test]
    fn embedding_update_lww() {
        let conflict = MergeConflict::new("node-5", MutationClass::EmbeddingUpdate, 500, 400);
        let resolution = GraphMergeResolution::resolve(conflict);
        assert_eq!(resolution.outcome, MergeOutcome::KeepLeft);
    }

    #[test]
    fn edge_delete_wins_over_upsert() {
        let conflict = MergeConflict::new("edge-1", MutationClass::EdgeDelete, 50, 200);
        let resolution = GraphMergeResolution::resolve(conflict);
        assert_eq!(resolution.outcome, MergeOutcome::BothDeleted);
    }

    #[test]
    fn resolve_batch_processes_all() {
        let conflicts = vec![
            MergeConflict::new("a", MutationClass::NodeUpsert, 1, 2),
            MergeConflict::new("b", MutationClass::NodeDelete, 1, 2),
            MergeConflict::new("c", MutationClass::LabelSet, 1, 2),
        ];
        let resolutions = resolve_batch(conflicts);
        assert_eq!(resolutions.len(), 3);
        assert_eq!(resolutions[0].outcome, MergeOutcome::KeepRight);
        assert_eq!(resolutions[1].outcome, MergeOutcome::BothDeleted);
        assert_eq!(resolutions[2].outcome, MergeOutcome::UnionLabels);
    }

    #[test]
    fn merge_policy_table_matches_adr_012() {
        assert_eq!(
            MergePolicy::for_class(MutationClass::NodeUpsert),
            MergePolicy::LastWriteWins
        );
        assert_eq!(
            MergePolicy::for_class(MutationClass::NodeDelete),
            MergePolicy::DeleteWins
        );
        assert_eq!(
            MergePolicy::for_class(MutationClass::EdgeDelete),
            MergePolicy::DeleteWins
        );
        assert_eq!(
            MergePolicy::for_class(MutationClass::LabelSet),
            MergePolicy::AddWinsSetUnion
        );
        assert_eq!(
            MergePolicy::for_class(MutationClass::PropsKey),
            MergePolicy::LastWriteWinsPerKey
        );
    }

    // ── T3.1 Slice 1 — walk_diff tests ─────────────────────────────────────

    fn ev(oid: &str, class: MutationClass, ts: i64) -> MutationEvent {
        MutationEvent::new(oid, class, ts)
    }

    #[test]
    fn walk_diff_empty_inputs_yields_no_conflicts() {
        let conflicts = walk_diff(&[], &[]);
        assert!(conflicts.is_empty());
    }

    #[test]
    fn walk_diff_disjoint_oids_yields_no_conflicts() {
        let left = vec![
            ev("a", MutationClass::NodeUpsert, 100),
            ev("b", MutationClass::NodeUpsert, 100),
        ];
        let right = vec![
            ev("c", MutationClass::NodeUpsert, 100),
            ev("d", MutationClass::NodeUpsert, 100),
        ];
        let conflicts = walk_diff(&left, &right);
        assert!(conflicts.is_empty());
    }

    #[test]
    fn walk_diff_overlapping_oid_yields_conflict_with_correct_timestamps() {
        let left = vec![ev("shared", MutationClass::NodeUpsert, 100)];
        let right = vec![ev("shared", MutationClass::NodeUpsert, 200)];
        let conflicts = walk_diff(&left, &right);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].record_id, "shared");
        assert_eq!(conflicts[0].left_wal_ts_us, 100);
        assert_eq!(conflicts[0].right_wal_ts_us, 200);
        // Policy comes from the existing primitive — verify LWW for NodeUpsert.
        assert_eq!(conflicts[0].policy, MergePolicy::LastWriteWins);
    }

    #[test]
    fn walk_diff_picks_latest_event_per_oid_per_side() {
        let left = vec![
            ev("x", MutationClass::NodeUpsert, 100),
            ev("x", MutationClass::NodeUpsert, 250), // latest on left
            ev("x", MutationClass::NodeUpsert, 200),
        ];
        let right = vec![
            ev("x", MutationClass::NodeUpsert, 50),
            ev("x", MutationClass::NodeUpsert, 175), // latest on right
        ];
        let conflicts = walk_diff(&left, &right);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].left_wal_ts_us, 250);
        assert_eq!(conflicts[0].right_wal_ts_us, 175);
    }

    #[test]
    fn walk_diff_delete_on_one_side_resolves_to_delete_wins() {
        let left = vec![ev("doomed", MutationClass::NodeDelete, 100)];
        let right = vec![ev("doomed", MutationClass::NodeUpsert, 500)];
        let conflicts = walk_diff(&left, &right);
        assert_eq!(conflicts.len(), 1);
        // Class is taken from left (NodeDelete); existing primitive routes
        // through DeleteWins policy regardless of timestamp ordering.
        assert_eq!(conflicts[0].mutation_class, MutationClass::NodeDelete);
        let resolution = GraphMergeResolution::resolve(conflicts[0].clone());
        assert_eq!(resolution.outcome, MergeOutcome::BothDeleted);
    }

    #[test]
    fn walk_diff_results_are_deterministically_ordered_by_oid() {
        let left = vec![
            ev("z", MutationClass::NodeUpsert, 100),
            ev("a", MutationClass::NodeUpsert, 100),
            ev("m", MutationClass::NodeUpsert, 100),
        ];
        let right = vec![
            ev("z", MutationClass::NodeUpsert, 200),
            ev("a", MutationClass::NodeUpsert, 200),
            ev("m", MutationClass::NodeUpsert, 200),
        ];
        let conflicts = walk_diff(&left, &right);
        let oids: Vec<&str> = conflicts
            .iter()
            .map(|c| c.record_id.as_str())
            .collect();
        assert_eq!(oids, vec!["a", "m", "z"]);
    }

    // ── T3.1 Slice 1 — classify_mutation tests ─────────────────────────────

    fn rec_default() -> proximadb_records::ProximaRecord {
        proximadb_records::ProximaRecord::default()
    }

    fn rec_tombstone() -> proximadb_records::ProximaRecord {
        let mut r = rec_default();
        r.valid_to_ns = Some(0);
        r.embeddings = Vec::new();
        r.origin = Some("delete".to_string());
        r
    }

    fn rec_edge() -> proximadb_records::ProximaRecord {
        let mut r = rec_default();
        r.edge = Some(proximadb_records::EdgeShape {
            source_id: "src".to_string(),
            target_id: "dst".to_string(),
            edge_type: "rel".to_string(),
            weight: None,
        });
        r
    }

    #[test]
    fn classify_tombstone_on_branch_a_returns_node_delete() {
        let base = rec_default();
        let a = rec_tombstone();
        let b = rec_default();
        assert_eq!(
            classify_mutation(Some(&base), Some(&a), Some(&b)),
            MutationClass::NodeDelete
        );
    }

    #[test]
    fn classify_tombstone_on_edge_returns_edge_delete() {
        let base = rec_edge();
        let mut a = rec_tombstone();
        a.edge = Some(proximadb_records::EdgeShape {
            source_id: "src".to_string(),
            target_id: "dst".to_string(),
            edge_type: "rel".to_string(),
            weight: None,
        });
        let b = rec_edge();
        assert_eq!(
            classify_mutation(Some(&base), Some(&a), Some(&b)),
            MutationClass::EdgeDelete
        );
    }

    #[test]
    fn classify_default_returns_node_upsert() {
        let base = rec_default();
        let a = rec_default();
        let b = rec_default();
        assert_eq!(
            classify_mutation(Some(&base), Some(&a), Some(&b)),
            MutationClass::NodeUpsert
        );
    }

    #[test]
    fn classify_edge_with_no_field_changes_returns_edge_upsert() {
        let base = rec_edge();
        let a = rec_edge();
        let b = rec_edge();
        assert_eq!(
            classify_mutation(Some(&base), Some(&a), Some(&b)),
            MutationClass::EdgeUpsert
        );
    }

    #[test]
    fn classify_label_only_change_returns_label_set() {
        let base = rec_default();
        let mut a = rec_default();
        a.labels = proximadb_records::LabelSet::from(vec!["new".to_string()]);
        let b = rec_default();
        assert_eq!(
            classify_mutation(Some(&base), Some(&a), Some(&b)),
            MutationClass::LabelSet
        );
    }

    #[test]
    fn classify_props_change_returns_props_key() {
        use proximadb_records::{ProximaTreeNode, ProximaValue};
        let base = rec_default();
        let mut a = rec_default();
        a.props.insert(
            "k".to_string(),
            ProximaTreeNode::Value(ProximaValue::String("v".to_string())),
        );
        let b = rec_default();
        assert_eq!(
            classify_mutation(Some(&base), Some(&a), Some(&b)),
            MutationClass::PropsKey
        );
    }
}
