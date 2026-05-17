/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! # ProximaRecord — Unified Record Envelope (Phase B, TD-054)
//!
//! Authoritative Rust definition of the `ProximaRecord` envelope as specified
//! in MULTIMODAL_OVERHAUL_SPEC_2026_05_08.adoc §3.
//!
//! Every record stored in ProximaDB — vector, graph node/edge, document, relational
//! row, time-series sample, event, or observability span — projects onto this single
//! envelope. Modality-specific fields live in `props` (NF² tree), `edge`, and
//! `embeddings`. Cross-cutting concerns (identity, tenancy, temporal, provenance,
//! labels) are top-level fields so security and query layers can reach them without
//! deserialising opaque blobs.

use std::collections::HashMap;

use proximadb_data_model::{MemoryType, ProximaValue};
use serde::{Deserialize, Serialize};

pub mod conversions;
pub mod proto_v2;
pub mod store;

pub use store::{
    RecordKey, RecordRecoveryOperation, RecordRecoverySummary, RecordScan, RecordScanOptions,
    RecordStorage, RecordStore, RecordStoreResult, RecordWriteResult,
    replay_record_recovery_operations,
};

// ---------------------------------------------------------------------------
// NF² Property Tree
// ---------------------------------------------------------------------------

/// A node in a nested property tree (NF² JSONB analogue).
///
/// Each key maps to either a leaf `ProximaValue` or a nested sub-tree, allowing
/// arbitrary depth without a fixed schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ProximaTreeNode {
    Value(ProximaValue),
    Object(ProximaTree),
}

/// Nested property map — the `props` field of [`ProximaRecord`].
pub type ProximaTree = HashMap<String, ProximaTreeNode>;

/// Look up a dot-separated path in a [`ProximaTree`].
///
/// Returns `None` if any segment is missing or the path traverses a leaf.
pub fn tree_get<'a>(tree: &'a ProximaTree, path: &str) -> Option<&'a ProximaValue> {
    let mut segments = path.splitn(2, '.');
    let head = segments.next()?;
    let tail = segments.next();

    match tree.get(head)? {
        ProximaTreeNode::Value(v) => {
            if tail.is_none() {
                Some(v)
            } else {
                None // path continues but this is a leaf
            }
        }
        ProximaTreeNode::Object(subtree) => match tail {
            Some(rest) => tree_get(subtree, rest),
            None => None, // path ends at an object node
        },
    }
}

// ---------------------------------------------------------------------------
// Supporting record components
// ---------------------------------------------------------------------------

/// Typed inter-record reference (spec §3 — refs field).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TypedRef {
    /// Relational foreign-key reference.
    ForeignKey { table: String, id: String },
    /// Graph directed edge reference.
    GraphEdge {
        edge_id: String,
        direction: EdgeDirection,
    },
    /// Reference to an embedding model + vector index.
    Embedding { model_id: String },
}

/// Direction of a graph edge reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeDirection {
    Outgoing,
    Incoming,
}

/// Graph edge topology fields (present only when the record IS an edge).
///
/// Corresponds to the topology layer of the three-layer storage model (spec §5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EdgeShape {
    pub source_id: String,
    pub target_id: String,
    pub edge_type: String,
    pub weight: Option<f64>,
}

/// A single embedding stored alongside a record (spec §3 — embeddings field).
///
/// One record can carry multiple `EmbeddingCell`s — one per model or modality.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmbeddingCell {
    /// Identifier of the model that produced this embedding.
    pub model_id: String,
    /// Modality tag (e.g. "text", "image", "audio").
    pub modality: String,
    /// Dense float32 vector values.
    pub values: Vec<f32>,
    /// Declared dimensionality (must equal `values.len()` for dense vectors).
    pub dim: u32,
}

/// Token sequence for LLM / event-stream records (spec §3 — sequence field).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenSequence {
    pub tokens: Vec<u32>,
    pub model_id: String,
    pub offset: u64,
}

/// Searchable label set (spec §3 — labels field).
///
/// Labels are stored as a `Vec<String>` and deduplicated on insert.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LabelSet(Vec<String>);

impl LabelSet {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    /// Insert a label. No-op if already present.
    pub fn insert(&mut self, label: impl Into<String>) {
        let label = label.into();
        if !self.0.contains(&label) {
            self.0.push(label);
        }
    }

    pub fn contains(&self, label: &str) -> bool {
        self.0.iter().any(|l| l == label)
    }

    pub fn iter(&self) -> impl Iterator<Item = &String> {
        self.0.iter()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<String>> for LabelSet {
    fn from(v: Vec<String>) -> Self {
        let mut set = LabelSet::new();
        for label in v {
            set.insert(label);
        }
        set
    }
}

// ---------------------------------------------------------------------------
// ProximaRecord — the unified envelope
// ---------------------------------------------------------------------------

/// The unified record envelope for all ProximaDB modalities.
///
/// Implements spec §3 of MULTIMODAL_OVERHAUL_SPEC_2026_05_08.adoc. Every stored
/// record — regardless of modality — projects onto this shape. Modality-specific
/// properties live in `props`. The `edge` field populates topology storage only
/// for graph edge records.
///
/// # RLS contract (spec §8)
///
/// `tenant_id` and `permitted_principals` are **record fields**, not context
/// fields. Engine-level scan iterators use them for row-level security without
/// referencing request context beyond the predicate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProximaRecord {
    // === Identity ===
    /// Globally unique object identifier (UUID or ULID string).
    pub oid: String,
    /// Caller-supplied local identifier (e.g. user-visible slug).
    pub local_id: Option<String>,
    /// Storage-layer tuple identifier for fast physical lookups.
    pub tid: Option<u64>,

    // === Catalog / version ===
    /// Schema variation identifier (U-Schema structural variant tag).
    pub variation_id: Option<String>,
    /// Monotonically increasing record version for OCC.
    pub record_version: u64,
    /// Spec/format version for forward compatibility.
    pub spec_version: u16,

    // === Tenancy + RLS (engine-level — spec §8) ===
    /// Owning tenant. Empty string = single-tenant / no isolation.
    pub tenant_id: String,
    /// ACL: principals allowed to read this record. Empty = unrestricted.
    pub permitted_principals: Vec<String>,
    /// ID of the RLS policy to evaluate at scan time, if any.
    pub rls_policy_id: Option<String>,

    // === Temporal ===
    /// Creation time (nanoseconds since Unix epoch).
    pub created_at_ns: i64,
    /// Last-update time (nanoseconds since Unix epoch).
    pub updated_at_ns: i64,
    /// Bi-temporal valid-from (nanoseconds since Unix epoch).
    pub valid_from_ns: Option<i64>,
    /// Bi-temporal valid-to (nanoseconds since Unix epoch).
    pub valid_to_ns: Option<i64>,

    // === Provenance ===
    /// Source system or connector that produced this record.
    pub origin: Option<String>,
    /// Principal that authored this record.
    pub actor: Option<String>,
    /// Ingestion method (e.g. "api", "cdc", "migration").
    pub method: Option<String>,

    // === Agentic Memory (Memanto — TD-055) ===
    /// High-fidelity memory category.
    pub memory_type: Option<MemoryType>,

    // === Properties (NF² tree) ===
    /// Modality-specific and user-defined properties.
    pub props: ProximaTree,

    // === Cross-modal references ===
    pub refs: Vec<TypedRef>,

    // === Graph topology (None for non-edge records) ===
    pub edge: Option<EdgeShape>,

    // === Embeddings (per-model, per-modality) ===
    pub embeddings: Vec<EmbeddingCell>,

    // === Token sequence (LLM / event streams) ===
    pub sequence: Option<TokenSequence>,

    // === Searchable labels ===
    pub labels: LabelSet,
}

impl Default for ProximaRecord {
    fn default() -> Self {
        let now_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as i64;

        Self {
            oid: String::new(),
            local_id: None,
            tid: None,
            variation_id: None,
            record_version: 0,
            spec_version: 1,
            tenant_id: String::new(),
            permitted_principals: Vec::new(),
            rls_policy_id: None,
            created_at_ns: now_ns,
            updated_at_ns: now_ns,
            valid_from_ns: None,
            valid_to_ns: None,
            origin: None,
            actor: None,
            method: None,
            memory_type: None,
            props: HashMap::new(),
            refs: Vec::new(),
            edge: None,
            embeddings: Vec::new(),
            sequence: None,
            labels: LabelSet::new(),
        }
    }
}

impl ProximaRecord {
    /// Check whether a given principal is permitted to access this record.
    ///
    /// Returns `true` if `permitted_principals` is empty (open access) OR if
    /// `principal` appears in the list.
    pub fn is_accessible_by(&self, principal: &str) -> bool {
        self.permitted_principals.is_empty()
            || self.permitted_principals.iter().any(|p| p == principal)
    }

    /// Check whether this record belongs to the given tenant.
    ///
    /// An empty `tenant_id` on the record matches any tenant (single-tenant mode).
    pub fn matches_tenant(&self, tenant_id: &str) -> bool {
        self.tenant_id.is_empty() || self.tenant_id == tenant_id
    }

    /// Resolve a conflict between two records using Last-Write-Wins (LWW).
    ///
    /// If `self.updated_at_ns >= other.updated_at_ns`, `self` is returned.
    /// Otherwise, `other` is returned.
    pub fn resolve_conflict(&self, other: &Self) -> Self {
        if self.updated_at_ns >= other.updated_at_ns {
            self.clone()
        } else {
            other.clone()
        }
    }
}

// ---------------------------------------------------------------------------
// Tests (TDD)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_data_model::{MemoryType, ProximaValue};

    #[test]
    fn test_proxima_record_defaults() {
        let r = ProximaRecord::default();
        assert!(r.created_at_ns > 0, "created_at_ns must be positive");
        assert_eq!(r.tenant_id, "");
        assert!(r.permitted_principals.is_empty());
        assert!(r.props.is_empty());
        assert_eq!(r.record_version, 0);
        assert_eq!(r.spec_version, 1);
        assert!(r.memory_type.is_none());
    }

    #[test]
    fn test_resolve_conflict_lww() {
        let mut r1 = ProximaRecord::default();
        r1.updated_at_ns = 100;
        r1.origin = Some("r1".to_string());

        let mut r2 = ProximaRecord::default();
        r2.updated_at_ns = 200;
        r2.origin = Some("r2".to_string());

        let resolved = r1.resolve_conflict(&r2);
        assert_eq!(resolved.origin.unwrap(), "r2");

        let resolved_inverse = r2.resolve_conflict(&r1);
        assert_eq!(resolved_inverse.origin.unwrap(), "r2");
    }

    #[test]
    fn test_memory_type_field() {
        let mut r = ProximaRecord::default();
        r.memory_type = Some(MemoryType::Decision);
        assert_eq!(r.memory_type, Some(MemoryType::Decision));
    }

    #[test]
    fn test_tenant_id_is_record_field() {
        let mut r = ProximaRecord::default();
        r.tenant_id = "acme".to_string();
        assert_eq!(r.tenant_id, "acme");
        // Accessible without unwrapping an Option<UserContext>
    }

    #[test]
    fn test_permitted_principals_empty_means_open_access() {
        let r = ProximaRecord::default();
        assert!(r.is_accessible_by("alice"));
        assert!(r.is_accessible_by("bob"));
    }

    #[test]
    fn test_permitted_principals_restricts_access() {
        let mut r = ProximaRecord::default();
        r.permitted_principals = vec!["alice".to_string()];
        assert!(r.is_accessible_by("alice"));
        assert!(!r.is_accessible_by("bob"));
    }

    #[test]
    fn test_matches_tenant() {
        let mut r = ProximaRecord::default();
        assert!(r.matches_tenant("acme"), "empty tenant_id matches any");

        r.tenant_id = "acme".to_string();
        assert!(r.matches_tenant("acme"));
        assert!(!r.matches_tenant("other"));
    }

    #[test]
    fn test_edge_shape_present_for_graph() {
        let mut r = ProximaRecord::default();
        r.edge = Some(EdgeShape {
            source_id: "node_a".to_string(),
            target_id: "node_b".to_string(),
            edge_type: "KNOWS".to_string(),
            weight: Some(0.9),
        });
        let edge = r.edge.as_ref().unwrap();
        assert_eq!(edge.source_id, "node_a");
        assert_eq!(edge.edge_type, "KNOWS");
        assert!((edge.weight.unwrap() - 0.9).abs() < 1e-9);
    }

    #[test]
    fn test_embedding_cell_fields() {
        let cell = EmbeddingCell {
            model_id: "text-embedding-3-small".to_string(),
            modality: "text".to_string(),
            values: vec![0.1, 0.2, 0.3],
            dim: 3,
        };
        assert_eq!(cell.dim, 3);
        assert_eq!(cell.values.len(), 3);

        let mut r = ProximaRecord::default();
        r.embeddings.push(cell);
        assert_eq!(r.embeddings[0].modality, "text");
    }

    #[test]
    fn test_typed_ref_variants_exist() {
        let fk = TypedRef::ForeignKey {
            table: "orders".to_string(),
            id: "ord_1".to_string(),
        };
        let ge = TypedRef::GraphEdge {
            edge_id: "e_1".to_string(),
            direction: EdgeDirection::Outgoing,
        };
        let emb = TypedRef::Embedding {
            model_id: "ada-002".to_string(),
        };
        assert!(matches!(fk, TypedRef::ForeignKey { .. }));
        assert!(matches!(ge, TypedRef::GraphEdge { .. }));
        assert!(matches!(emb, TypedRef::Embedding { .. }));
    }

    #[test]
    fn test_label_set_contains() {
        let mut labels = LabelSet::new();
        labels.insert("vector");
        labels.insert("text");
        assert!(labels.contains("vector"));
        assert!(!labels.contains("graph"));
    }

    #[test]
    fn test_label_set_deduplication() {
        let mut labels = LabelSet::new();
        labels.insert("rag");
        labels.insert("rag");
        assert_eq!(labels.len(), 1);
    }

    #[test]
    fn test_proxima_tree_flat_lookup() {
        let mut tree = ProximaTree::new();
        tree.insert(
            "category".to_string(),
            ProximaTreeNode::Value(ProximaValue::String("technology".to_string())),
        );
        let val = tree_get(&tree, "category").unwrap();
        assert!(matches!(val, ProximaValue::String(s) if s == "technology"));
    }

    #[test]
    fn test_proxima_tree_nested_lookup() {
        let mut inner = ProximaTree::new();
        inner.insert(
            "city".to_string(),
            ProximaTreeNode::Value(ProximaValue::String("NYC".to_string())),
        );
        let mut tree = ProximaTree::new();
        tree.insert("address".to_string(), ProximaTreeNode::Object(inner));

        let val = tree_get(&tree, "address.city").unwrap();
        assert!(matches!(val, ProximaValue::String(s) if s == "NYC"));
    }

    #[test]
    fn test_proxima_tree_missing_path_returns_none() {
        let tree = ProximaTree::new();
        assert!(tree_get(&tree, "nonexistent").is_none());
        assert!(tree_get(&tree, "a.b.c").is_none());
    }

    #[test]
    fn test_record_serde_roundtrip() {
        let mut r = ProximaRecord {
            oid: "rec_abc".to_string(),
            tenant_id: "acme".to_string(),
            ..ProximaRecord::default()
        };
        r.labels.insert("vector");
        r.props.insert(
            "score".to_string(),
            ProximaTreeNode::Value(ProximaValue::Float64(0.95)),
        );

        let json = serde_json::to_string(&r).expect("serialize");
        let back: ProximaRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.oid, "rec_abc");
        assert_eq!(back.tenant_id, "acme");
        assert!(back.labels.contains("vector"));
    }
}
