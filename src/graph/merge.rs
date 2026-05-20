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
}
