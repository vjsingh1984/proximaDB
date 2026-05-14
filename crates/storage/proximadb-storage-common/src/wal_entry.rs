//! Canonical WAL entry types for durability, recovery, and CDC consolidation.
//!
//! Every modality (document, graph node/edge, vector, observability) writes
//! WAL entries as `CanonicalOperation::RecordUpsert` carrying a `ProximaRecord`.
//! Recovery replays these entries and calls `ProjectionRebuilder` to reconstruct
//! modality-specific projections without those projections being an independent
//! durable source of truth.
//!
//! Spec reference:
//! `docs/12-design/RELATIONAL_DOCUMENT_GRAPH_CONVERGENCE_2026_05_14.adoc` Phase 5.

use proximadb_records::ProximaRecord;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Canonical WAL entry
// ---------------------------------------------------------------------------

/// The canonical WAL entry written by every modality.
///
/// Document writes, graph node/edge writes, vector writes, and observability
/// writes all funnel through this shape. Modality-specific projection state
/// (adjacency tables, JSON path indexes, HNSW, CSR) is rebuilt from the
/// `projections` field during recovery rather than being persisted as a
/// separate durable truth.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicalWalEntry {
    /// Monotonically increasing WAL sequence number (log sequence number / LSN).
    pub sequence_number: u64,
    /// Wall-clock time at which the entry was written (ms since Unix epoch).
    pub timestamp_ms: u64,
    /// The canonical operation to apply.
    pub operation: CanonicalOperation,
    /// Optional tenant scope; `None` for single-tenant deployments.
    pub tenant_id: Option<String>,
}

impl CanonicalWalEntry {
    pub fn new(
        sequence_number: u64,
        operation: CanonicalOperation,
        tenant_id: Option<String>,
    ) -> Self {
        let timestamp_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        Self {
            sequence_number,
            timestamp_ms,
            operation,
            tenant_id,
        }
    }

    pub fn is_checkpoint(&self) -> bool {
        matches!(self.operation, CanonicalOperation::Checkpoint(_))
    }
}

// ---------------------------------------------------------------------------
// Canonical operation enum
// ---------------------------------------------------------------------------

/// Operation carried by a `CanonicalWalEntry`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CanonicalOperation {
    /// Insert or update any record (document, node, edge, vector, observation).
    ///
    /// The `projections` list tells recovery which physical projections to
    /// rebuild or update when replaying this entry.
    RecordUpsert {
        collection_id: String,
        record: ProximaRecord,
        projections: Vec<ProjectionDirective>,
    },
    /// Delete a record by canonical OID.
    ///
    /// The `projections` list tells recovery which projections to invalidate.
    RecordDelete {
        collection_id: String,
        /// Canonical object ID (`ProximaRecord::oid`).
        oid: String,
        projections: Vec<ProjectionDirective>,
    },
    /// Durable checkpoint. Recovery can start from the latest `Checkpoint`
    /// and replay only subsequent entries rather than the full log.
    Checkpoint(SnapshotManifest),
    /// CDC publish barrier. Marks the highest sequence number up to which
    /// CDC consumers have been notified. Replaying past this entry re-emits
    /// CDC events to catch up late subscribers.
    CdcBarrier {
        /// Sequence number of the last CDC-published entry.
        barrier_sequence: u64,
        /// Events that were published up to `barrier_sequence`.
        events: Vec<CdcRecordEvent>,
    },
}

// ---------------------------------------------------------------------------
// Projection directives
// ---------------------------------------------------------------------------

/// Describes one physical projection that must be rebuilt when replaying a
/// canonical WAL entry.
///
/// Directives travel alongside the authoritative record so that recovery
/// knows exactly which projections to touch without hard-coded dispatch on
/// `source_model` or collection naming conventions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ProjectionDirective {
    /// Rebuild the JSON path index for the given collection.
    DocumentJsonPathIndex {
        collection_id: String,
        /// JSON path expressions that are indexed (e.g. `"$.tags[*]"`).
        indexed_paths: Vec<String>,
    },
    /// Rebuild the full-text search index for specific fields.
    FullTextIndex {
        collection_id: String,
        indexed_fields: Vec<String>,
    },
    /// Update one row in the graph adjacency table.
    ///
    /// Each upsert of a node or edge generates one directive per affected
    /// adjacency row so that recovery can apply fine-grained updates without
    /// scanning the whole table.
    AdjacencyTableRow {
        graph_id: String,
        node_oid: String,
        /// Edge references originating from or terminating at `node_oid`.
        edge_refs: Vec<EdgeRef>,
    },
    /// Schedule a CSR re-materialisation for the graph.
    ///
    /// This is a coarse directive: recovery marks the CSR stale and a
    /// background task performs the actual rebuild. Write-heavy workloads
    /// accumulate many individual `AdjacencyTableRow` directives and
    /// coalesce them into a single `CsrRebuild` when the write rate drops
    /// below the CSR-staleness threshold.
    CsrRebuild { graph_id: String },
    /// Update the HNSW index for the given embedding field.
    HnswIndex {
        collection_id: String,
        embedding_field: String,
    },
    /// Update the columnar variation projection for a set of fields.
    ColumnarVariation {
        collection_id: String,
        fields: Vec<String>,
    },
}

/// A lightweight edge reference stored inside `AdjacencyTableRow`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EdgeRef {
    pub src_oid: String,
    pub dst_oid: String,
    pub edge_type: String,
    pub weight: Option<f64>,
}

// ---------------------------------------------------------------------------
// Snapshot manifest (checkpoint)
// ---------------------------------------------------------------------------

/// Snapshot manifest captured inside a `Checkpoint` WAL entry.
///
/// Recovery starts from the latest checkpoint, checks `projection_freshness`
/// to skip projections already up-to-date, then replays only the WAL entries
/// after `sequence_number`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotManifest {
    /// WAL LSN at which the snapshot was taken.
    pub sequence_number: u64,
    /// Wall-clock time of the snapshot (ms since Unix epoch).
    pub timestamp_ms: u64,
    /// Collections present in this snapshot.
    pub collection_ids: Vec<String>,
    /// Per-projection freshness metadata so recovery can skip projections
    /// whose `last_rebuilt_lsn >= checkpoint.sequence_number`.
    pub projection_freshness: Vec<ProjectionFreshness>,
}

/// Records the last WAL LSN at which a named projection was successfully rebuilt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectionFreshness {
    /// Human-readable projection name (e.g. `"doc_json_path:my_collection"`).
    pub projection_name: String,
    /// WAL sequence number of the last successful rebuild.
    pub last_rebuilt_lsn: u64,
    /// Whether the projection is currently stale and requires a rebuild.
    pub stale: bool,
}

// ---------------------------------------------------------------------------
// CDC canonical event
// ---------------------------------------------------------------------------

/// Canonical CDC event published when a record changes.
///
/// CDC consumers receive `CdcRecordEvent` and derive modality-specific logical
/// views from it; they do not receive raw `DocumentRecord` or graph-specific
/// change structs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CdcRecordEvent {
    /// Unique event identifier (UUID recommended).
    pub event_id: String,
    /// WAL sequence number that produced this event.
    pub sequence_number: u64,
    /// Wall-clock time of the event (ms since Unix epoch).
    pub timestamp_ms: u64,
    pub collection_id: String,
    pub operation: CdcOperation,
    /// Canonical record state after the change (`None` for `Delete`).
    pub record_after: Option<ProximaRecord>,
    /// Modality-specific logical views derived from the canonical record.
    ///
    /// These are pre-computed at publish time so consumers can subscribe
    /// to just the view they care about without re-implementing projection logic.
    pub logical_views: Vec<CdcLogicalView>,
}

/// Type of CDC operation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CdcOperation {
    Insert,
    Update,
    Delete,
}

/// A derived logical view attached to a `CdcRecordEvent`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CdcLogicalView {
    /// Document facade view of the record change.
    DocumentChange {
        collection_id: String,
        document_json: serde_json::Value,
    },
    /// Graph node facade view.
    GraphNodeChange {
        graph_id: String,
        node_json: serde_json::Value,
    },
    /// Graph edge facade view.
    GraphEdgeChange {
        graph_id: String,
        edge_json: serde_json::Value,
    },
}

// ---------------------------------------------------------------------------
// Projection rebuilder trait
// ---------------------------------------------------------------------------

/// Callback interface called by `recover_from_canonical_wal` for each
/// `ProjectionDirective` encountered during log replay.
///
/// Production implementations wire into the document index, graph adjacency
/// table, HNSW manager, and CSR materialiser. The in-memory test double
/// records which directives were applied.
pub trait ProjectionRebuilder {
    fn apply_directive(
        &mut self,
        record: Option<&ProximaRecord>,
        directive: &ProjectionDirective,
    ) -> Result<(), String>;
}

// ---------------------------------------------------------------------------
// Recovery
// ---------------------------------------------------------------------------

/// Summary produced by `recover_from_canonical_wal`.
#[derive(Debug, Default)]
pub struct RecoveryResult {
    /// Number of `RecordUpsert` entries replayed.
    pub upserts_replayed: u64,
    /// Number of `RecordDelete` entries replayed.
    pub deletes_replayed: u64,
    /// Number of projection directives applied.
    pub directives_applied: u64,
    /// Number of directives that returned an error (non-fatal; logged).
    pub directive_errors: u64,
    /// Sequence number of the latest entry replayed.
    pub last_sequence: u64,
}

/// Replay a slice of canonical WAL entries, firing each `ProjectionDirective`
/// through `rebuilder`.
///
/// Entries before `start_after_lsn` are skipped so callers can resume from a
/// checkpoint LSN without replaying the full history.
pub fn recover_from_canonical_wal<R: ProjectionRebuilder>(
    entries: &[CanonicalWalEntry],
    rebuilder: &mut R,
    start_after_lsn: u64,
) -> RecoveryResult {
    let mut result = RecoveryResult::default();

    for entry in entries {
        if entry.sequence_number <= start_after_lsn {
            continue;
        }
        result.last_sequence = entry.sequence_number;

        match &entry.operation {
            CanonicalOperation::RecordUpsert {
                record, projections, ..
            } => {
                result.upserts_replayed += 1;
                for directive in projections {
                    match rebuilder.apply_directive(Some(record), directive) {
                        Ok(()) => result.directives_applied += 1,
                        Err(_) => result.directive_errors += 1,
                    }
                }
            }
            CanonicalOperation::RecordDelete {
                oid: _, projections, ..
            } => {
                result.deletes_replayed += 1;
                for directive in projections {
                    match rebuilder.apply_directive(None, directive) {
                        Ok(()) => result.directives_applied += 1,
                        Err(_) => result.directive_errors += 1,
                    }
                }
            }
            // Checkpoints and CDC barriers do not trigger projection rebuilds;
            // the caller uses the checkpoint LSN to scope the replay window.
            CanonicalOperation::Checkpoint(_) | CanonicalOperation::CdcBarrier { .. } => {}
        }
    }

    result
}

/// Find the highest-LSN `Checkpoint` entry in the log.
///
/// Recovery should call this first, then pass the returned `sequence_number`
/// as `start_after_lsn` to `recover_from_canonical_wal`.
pub fn latest_checkpoint(entries: &[CanonicalWalEntry]) -> Option<&SnapshotManifest> {
    entries
        .iter()
        .rev()
        .find_map(|e| match &e.operation {
            CanonicalOperation::Checkpoint(m) => Some(m),
            _ => None,
        })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_records::ProximaRecord;
    use std::collections::HashMap;

    // ── test double ──────────────────────────────────────────────────────

    #[derive(Default)]
    struct RecordingRebuilder {
        applied: Vec<(Option<String>, ProjectionDirective)>,
        fail_variant: Option<std::mem::Discriminant<ProjectionDirective>>,
    }

    impl ProjectionRebuilder for RecordingRebuilder {
        fn apply_directive(
            &mut self,
            record: Option<&ProximaRecord>,
            directive: &ProjectionDirective,
        ) -> Result<(), String> {
            if let Some(fail) = self.fail_variant {
                if std::mem::discriminant(directive) == fail {
                    return Err("injected failure".into());
                }
            }
            let oid = record.map(|r| r.oid.clone());
            self.applied.push((oid, directive.clone()));
            Ok(())
        }
    }

    // ── helpers ──────────────────────────────────────────────────────────

    fn make_record(oid: &str) -> ProximaRecord {
        ProximaRecord {
            oid: oid.into(),
            ..Default::default()
        }
    }

    fn upsert_entry(seq: u64, oid: &str, dirs: Vec<ProjectionDirective>) -> CanonicalWalEntry {
        CanonicalWalEntry::new(
            seq,
            CanonicalOperation::RecordUpsert {
                collection_id: "col".into(),
                record: make_record(oid),
                projections: dirs,
            },
            None,
        )
    }

    fn delete_entry(seq: u64, oid: &str, dirs: Vec<ProjectionDirective>) -> CanonicalWalEntry {
        CanonicalWalEntry::new(
            seq,
            CanonicalOperation::RecordDelete {
                collection_id: "col".into(),
                oid: oid.into(),
                projections: dirs,
            },
            None,
        )
    }

    fn checkpoint_entry(seq: u64, collections: Vec<String>) -> CanonicalWalEntry {
        let manifest = SnapshotManifest {
            sequence_number: seq,
            timestamp_ms: 0,
            collection_ids: collections,
            projection_freshness: vec![],
        };
        CanonicalWalEntry::new(seq, CanonicalOperation::Checkpoint(manifest), None)
    }

    // ── CanonicalWalEntry construction ───────────────────────────────────

    #[test]
    fn new_entry_has_correct_sequence_and_tenant() {
        let entry = CanonicalWalEntry::new(
            42,
            CanonicalOperation::RecordUpsert {
                collection_id: "docs".into(),
                record: make_record("r1"),
                projections: vec![],
            },
            Some("acme".into()),
        );
        assert_eq!(entry.sequence_number, 42);
        assert_eq!(entry.tenant_id.as_deref(), Some("acme"));
        assert!(!entry.is_checkpoint());
    }

    #[test]
    fn checkpoint_entry_is_identified() {
        let e = checkpoint_entry(10, vec!["docs".into()]);
        assert!(e.is_checkpoint());
    }

    // ── recovery: basic replay ────────────────────────────────────────────

    #[test]
    fn recovery_replays_upserts_and_fires_directives() {
        let dir = ProjectionDirective::DocumentJsonPathIndex {
            collection_id: "docs".into(),
            indexed_paths: vec!["$.title".into()],
        };
        let entries = vec![upsert_entry(1, "r1", vec![dir.clone()])];
        let mut rb = RecordingRebuilder::default();

        let result = recover_from_canonical_wal(&entries, &mut rb, 0);

        assert_eq!(result.upserts_replayed, 1);
        assert_eq!(result.directives_applied, 1);
        assert_eq!(result.last_sequence, 1);
        assert_eq!(rb.applied.len(), 1);
        assert_eq!(rb.applied[0].1, dir);
        assert_eq!(rb.applied[0].0.as_deref(), Some("r1"));
    }

    #[test]
    fn recovery_replays_deletes_with_no_record() {
        let dir = ProjectionDirective::AdjacencyTableRow {
            graph_id: "g1".into(),
            node_oid: "n1".into(),
            edge_refs: vec![],
        };
        let entries = vec![delete_entry(1, "n1", vec![dir])];
        let mut rb = RecordingRebuilder::default();

        let result = recover_from_canonical_wal(&entries, &mut rb, 0);

        assert_eq!(result.deletes_replayed, 1);
        assert_eq!(result.directives_applied, 1);
        // Delete directives pass None as the record.
        assert_eq!(rb.applied[0].0, None);
    }

    #[test]
    fn recovery_skips_entries_at_or_before_start_lsn() {
        let entries = vec![
            upsert_entry(1, "r1", vec![]),
            upsert_entry(2, "r2", vec![]),
            upsert_entry(3, "r3", vec![]),
        ];
        let mut rb = RecordingRebuilder::default();

        let result = recover_from_canonical_wal(&entries, &mut rb, 2);

        // Only entry 3 replayed.
        assert_eq!(result.upserts_replayed, 1);
        assert_eq!(result.last_sequence, 3);
    }

    #[test]
    fn recovery_skips_checkpoint_and_cdc_barrier_entries() {
        let cdc = CanonicalWalEntry::new(
            2,
            CanonicalOperation::CdcBarrier {
                barrier_sequence: 1,
                events: vec![],
            },
            None,
        );
        let entries = vec![
            upsert_entry(1, "r1", vec![]),
            cdc,
            checkpoint_entry(3, vec![]),
            upsert_entry(4, "r4", vec![]),
        ];
        let mut rb = RecordingRebuilder::default();

        let result = recover_from_canonical_wal(&entries, &mut rb, 0);

        assert_eq!(result.upserts_replayed, 2);
        assert_eq!(result.deletes_replayed, 0);
        assert_eq!(result.last_sequence, 4);
    }

    #[test]
    fn recovery_counts_directive_errors_without_stopping() {
        let good = ProjectionDirective::FullTextIndex {
            collection_id: "c".into(),
            indexed_fields: vec!["body".into()],
        };
        let bad = ProjectionDirective::CsrRebuild {
            graph_id: "g".into(),
        };
        let entries = vec![upsert_entry(1, "r1", vec![good.clone(), bad.clone()])];

        let mut rb = RecordingRebuilder {
            fail_variant: Some(std::mem::discriminant(&bad)),
            ..Default::default()
        };
        let result = recover_from_canonical_wal(&entries, &mut rb, 0);

        assert_eq!(result.directives_applied, 1);
        assert_eq!(result.directive_errors, 1);
        // Recovery did not abort.
        assert_eq!(result.upserts_replayed, 1);
    }

    // ── latest_checkpoint ────────────────────────────────────────────────

    #[test]
    fn latest_checkpoint_finds_highest_lsn_checkpoint() {
        let entries = vec![
            checkpoint_entry(5, vec!["a".into()]),
            upsert_entry(6, "r6", vec![]),
            checkpoint_entry(10, vec!["a".into(), "b".into()]),
            upsert_entry(11, "r11", vec![]),
        ];
        let cp = latest_checkpoint(&entries).expect("should find a checkpoint");
        assert_eq!(cp.sequence_number, 10);
        assert_eq!(cp.collection_ids.len(), 2);
    }

    #[test]
    fn latest_checkpoint_returns_none_when_no_checkpoint() {
        let entries = vec![upsert_entry(1, "r1", vec![])];
        assert!(latest_checkpoint(&entries).is_none());
    }

    // ── ProjectionDirective variants ──────────────────────────────────────

    #[test]
    fn adjacency_table_directive_carries_edge_refs() {
        let directive = ProjectionDirective::AdjacencyTableRow {
            graph_id: "social".into(),
            node_oid: "alice".into(),
            edge_refs: vec![
                EdgeRef {
                    src_oid: "alice".into(),
                    dst_oid: "bob".into(),
                    edge_type: "FOLLOWS".into(),
                    weight: Some(1.0),
                },
                EdgeRef {
                    src_oid: "carol".into(),
                    dst_oid: "alice".into(),
                    edge_type: "FOLLOWS".into(),
                    weight: None,
                },
            ],
        };
        if let ProjectionDirective::AdjacencyTableRow { edge_refs, .. } = &directive {
            assert_eq!(edge_refs.len(), 2);
            assert!(edge_refs[0].weight.is_some());
            assert!(edge_refs[1].weight.is_none());
        } else {
            panic!("wrong variant");
        }
    }

    // ── SnapshotManifest freshness ────────────────────────────────────────

    #[test]
    fn snapshot_manifest_freshness_tracks_stale_projections() {
        let manifest = SnapshotManifest {
            sequence_number: 100,
            timestamp_ms: 0,
            collection_ids: vec!["docs".into()],
            projection_freshness: vec![
                ProjectionFreshness {
                    projection_name: "doc_json_path:docs".into(),
                    last_rebuilt_lsn: 100,
                    stale: false,
                },
                ProjectionFreshness {
                    projection_name: "fts:docs".into(),
                    last_rebuilt_lsn: 80,
                    stale: true,
                },
            ],
        };
        let stale: Vec<_> = manifest
            .projection_freshness
            .iter()
            .filter(|p| p.stale)
            .collect();
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].projection_name, "fts:docs");
    }

    // ── CDC canonical event ───────────────────────────────────────────────

    #[test]
    fn cdc_event_carries_logical_views() {
        let event = CdcRecordEvent {
            event_id: "evt-1".into(),
            sequence_number: 7,
            timestamp_ms: 0,
            collection_id: "docs".into(),
            operation: CdcOperation::Insert,
            record_after: Some(make_record("doc-1")),
            logical_views: vec![
                CdcLogicalView::DocumentChange {
                    collection_id: "docs".into(),
                    document_json: serde_json::json!({"title": "hello"}),
                },
                CdcLogicalView::GraphNodeChange {
                    graph_id: "kg".into(),
                    node_json: serde_json::json!({"id": "doc-1"}),
                },
            ],
        };
        assert_eq!(event.operation, CdcOperation::Insert);
        assert!(event.record_after.is_some());
        assert_eq!(event.logical_views.len(), 2);
    }

    #[test]
    fn cdc_delete_event_has_no_record_after() {
        let event = CdcRecordEvent {
            event_id: "evt-2".into(),
            sequence_number: 8,
            timestamp_ms: 0,
            collection_id: "nodes".into(),
            operation: CdcOperation::Delete,
            record_after: None,
            logical_views: vec![],
        };
        assert_eq!(event.operation, CdcOperation::Delete);
        assert!(event.record_after.is_none());
    }

    // ── crash/recovery: checkpoint-scoped replay ──────────────────────────

    #[test]
    fn crash_recovery_resumes_from_checkpoint_lsn() {
        // Simulate a log with an earlier checkpoint and entries after it.
        let dir = ProjectionDirective::HnswIndex {
            collection_id: "vecs".into(),
            embedding_field: "embedding".into(),
        };
        let entries = vec![
            upsert_entry(1, "pre-cp-1", vec![dir.clone()]),
            upsert_entry(2, "pre-cp-2", vec![dir.clone()]),
            checkpoint_entry(3, vec!["vecs".into()]),
            upsert_entry(4, "post-cp-1", vec![dir.clone()]),
            upsert_entry(5, "post-cp-2", vec![dir.clone()]),
        ];

        // Recovery finds the checkpoint at LSN 3, then replays only after it.
        let checkpoint_lsn = latest_checkpoint(&entries)
            .map(|m| m.sequence_number)
            .unwrap_or(0);
        assert_eq!(checkpoint_lsn, 3);

        let mut rb = RecordingRebuilder::default();
        let result = recover_from_canonical_wal(&entries, &mut rb, checkpoint_lsn);

        // Only entries 4 and 5 replayed.
        assert_eq!(result.upserts_replayed, 2);
        assert_eq!(result.directives_applied, 2);
        let replayed_oids: Vec<_> = rb.applied.iter().filter_map(|(oid, _)| oid.as_deref()).collect();
        assert!(replayed_oids.contains(&"post-cp-1"));
        assert!(replayed_oids.contains(&"post-cp-2"));
        assert!(!replayed_oids.contains(&"pre-cp-1"));
    }

    #[test]
    fn crash_recovery_graph_adjacency_directives_all_fired() {
        let edge_dir = ProjectionDirective::AdjacencyTableRow {
            graph_id: "kg".into(),
            node_oid: "alice".into(),
            edge_refs: vec![EdgeRef {
                src_oid: "alice".into(),
                dst_oid: "bob".into(),
                edge_type: "KNOWS".into(),
                weight: None,
            }],
        };
        let csr_dir = ProjectionDirective::CsrRebuild { graph_id: "kg".into() };

        let entries = vec![
            upsert_entry(1, "alice", vec![edge_dir.clone()]),
            upsert_entry(2, "bob", vec![edge_dir.clone(), csr_dir.clone()]),
        ];
        let mut rb = RecordingRebuilder::default();
        let result = recover_from_canonical_wal(&entries, &mut rb, 0);

        assert_eq!(result.directives_applied, 3);
        let csr_count = rb
            .applied
            .iter()
            .filter(|(_, d)| matches!(d, ProjectionDirective::CsrRebuild { .. }))
            .count();
        assert_eq!(csr_count, 1);
    }

    #[test]
    fn projection_corruption_repaired_by_full_replay_from_zero() {
        // Simulate projection corruption: ignore the checkpoint and replay
        // everything from LSN 0 to get a clean rebuild.
        let dir = ProjectionDirective::DocumentJsonPathIndex {
            collection_id: "docs".into(),
            indexed_paths: vec!["$.tags".into()],
        };
        let entries = vec![
            upsert_entry(1, "d1", vec![dir.clone()]),
            upsert_entry(2, "d2", vec![dir.clone()]),
            checkpoint_entry(3, vec!["docs".into()]),
            upsert_entry(4, "d3", vec![dir.clone()]),
        ];
        let mut rb = RecordingRebuilder::default();
        // Full replay (start_after_lsn = 0) rebuilds even pre-checkpoint entries.
        let result = recover_from_canonical_wal(&entries, &mut rb, 0);

        assert_eq!(result.upserts_replayed, 3);
        assert_eq!(result.directives_applied, 3);
    }
}
