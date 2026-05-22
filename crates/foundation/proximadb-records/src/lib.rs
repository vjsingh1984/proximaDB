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

/// Scalar storage type for embedding values. See
/// `docs/12-design/EMBEDDING_PRECISION_LLD_2026_05_22.adoc` for the canonical
/// byte tags and serialization shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum EmbeddingScalarType {
    /// IEEE-754 f32, 4 bytes per element. Today's universal default.
    Fp32 = 0x01,
    /// IEEE-754 f16 (half-precision), 2 bytes per element.
    Fp16 = 0x02,
    /// Brain float16, 2 bytes per element. Reserved for Phase 6 hardware paths.
    Bf16 = 0x03,
    /// Signed 8-bit scalar with per-cell scale + zero-point. Lossy.
    Int8Scalar = 0x04,
    /// Unsigned 8-bit scalar with per-cell scale + zero-point. Lossy.
    UInt8Scalar = 0x05,
}

impl EmbeddingScalarType {
    /// Bytes-per-element for this scalar type (excluding per-cell metadata).
    pub fn bytes_per_element(self) -> usize {
        match self {
            Self::Fp32 => 4,
            Self::Fp16 | Self::Bf16 => 2,
            Self::Int8Scalar | Self::UInt8Scalar => 1,
        }
    }

    /// Whether this precision is lossy relative to the f32 source.
    pub fn is_lossy(self) -> bool {
        matches!(self, Self::Int8Scalar | Self::UInt8Scalar)
    }
}

impl Default for EmbeddingScalarType {
    fn default() -> Self {
        Self::Fp32
    }
}

/// Typed embedding payload. PR 1 of the precision rollout adds the enum
/// variants; only `Fp32` is exercised by existing call sites today. PR 3
/// switches durable storage to this enum.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EmbeddingValues {
    Fp32(Vec<f32>),
    Fp16(Vec<half::f16>),
    Bf16(Vec<half::bf16>),
    Int8Scalar {
        values: Vec<i8>,
        scale: f32,
        zero_point: i8,
    },
    UInt8Scalar {
        values: Vec<u8>,
        scale: f32,
        zero_point: u8,
    },
}

impl EmbeddingValues {
    pub fn len(&self) -> usize {
        match self {
            Self::Fp32(v) => v.len(),
            Self::Fp16(v) => v.len(),
            Self::Bf16(v) => v.len(),
            Self::Int8Scalar { values, .. } => values.len(),
            Self::UInt8Scalar { values, .. } => values.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn scalar_type(&self) -> EmbeddingScalarType {
        match self {
            Self::Fp32(_) => EmbeddingScalarType::Fp32,
            Self::Fp16(_) => EmbeddingScalarType::Fp16,
            Self::Bf16(_) => EmbeddingScalarType::Bf16,
            Self::Int8Scalar { .. } => EmbeddingScalarType::Int8Scalar,
            Self::UInt8Scalar { .. } => EmbeddingScalarType::UInt8Scalar,
        }
    }

    /// On-disk byte size of the values payload (excluding the surrounding
    /// EmbeddingCell metadata). Drives storage budget enforcement.
    pub fn byte_size(&self) -> usize {
        let bytes = self.scalar_type().bytes_per_element();
        let scale_meta = match self {
            Self::Int8Scalar { .. } | Self::UInt8Scalar { .. } => 5, // 4B scale + 1B zero_point
            _ => 0,
        };
        self.len() * bytes + scale_meta
    }

    pub fn as_fp32_slice(&self) -> Option<&[f32]> {
        match self {
            Self::Fp32(v) => Some(v.as_slice()),
            _ => None,
        }
    }

    pub fn to_fp32_owned(&self) -> Vec<f32> {
        match self {
            Self::Fp32(v) => v.clone(),
            Self::Fp16(v) => v.iter().map(|x| x.to_f32()).collect(),
            Self::Bf16(v) => v.iter().map(|x| x.to_f32()).collect(),
            Self::Int8Scalar {
                values,
                scale,
                zero_point,
            } => values
                .iter()
                .map(|x| (i32::from(*x) - i32::from(*zero_point)) as f32 * scale)
                .collect(),
            Self::UInt8Scalar {
                values,
                scale,
                zero_point,
            } => values
                .iter()
                .map(|x| (i32::from(*x) - i32::from(*zero_point)) as f32 * scale)
                .collect(),
        }
    }

    /// Downconvert an `&[f32]` to the requested scalar type. Symmetric for
    /// Int8Scalar (per-cell scale, zero_point=0), zero-point-aware for UInt8.
    pub fn from_fp32_lossy(src: &[f32], target: EmbeddingScalarType) -> Self {
        match target {
            EmbeddingScalarType::Fp32 => Self::Fp32(src.to_vec()),
            EmbeddingScalarType::Fp16 => {
                Self::Fp16(src.iter().map(|&x| half::f16::from_f32(x)).collect())
            }
            EmbeddingScalarType::Bf16 => {
                Self::Bf16(src.iter().map(|&x| half::bf16::from_f32(x)).collect())
            }
            EmbeddingScalarType::Int8Scalar => {
                let abs_max = src
                    .iter()
                    .fold(0.0_f32, |acc, &x| acc.max(x.abs()))
                    .max(f32::EPSILON);
                let scale = abs_max / 127.0;
                let values: Vec<i8> = src
                    .iter()
                    .map(|&x| (x / scale).round().clamp(-127.0, 127.0) as i8)
                    .collect();
                Self::Int8Scalar {
                    values,
                    scale,
                    zero_point: 0,
                }
            }
            EmbeddingScalarType::UInt8Scalar => {
                let min = src.iter().cloned().fold(f32::INFINITY, f32::min);
                let max = src.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
                let range = (max - min).max(f32::EPSILON);
                let scale = range / 255.0;
                let zero_point: u8 = (-min / scale).round().clamp(0.0, 255.0) as u8;
                let values: Vec<u8> = src
                    .iter()
                    .map(|&x| ((x / scale).round() + zero_point as f32).clamp(0.0, 255.0) as u8)
                    .collect();
                Self::UInt8Scalar {
                    values,
                    scale,
                    zero_point,
                }
            }
        }
    }
}

/// A single embedding stored alongside a record (spec §3 — embeddings field).
///
/// One record can carry multiple `EmbeddingCell`s — one per model or modality.
/// The `values: Vec<f32>` field remains the durable canonical storage for PR 1;
/// `precision` and `precision_epoch` are advisory until PR 3 switches durable
/// storage to the `EmbeddingValues` enum.
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
    /// Declared scalar storage precision (PR 1 advisory; PR 3 authoritative).
    #[serde(default)]
    pub precision: EmbeddingScalarType,
    /// Precision epoch this cell was written under, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub precision_epoch: Option<u64>,
}

impl EmbeddingCell {
    /// Construct an fp32 embedding cell with default precision metadata.
    pub fn new_fp32(
        model_id: impl Into<String>,
        modality: impl Into<String>,
        dim: u32,
        values: Vec<f32>,
    ) -> Self {
        Self {
            model_id: model_id.into(),
            modality: modality.into(),
            values,
            dim,
            precision: EmbeddingScalarType::Fp32,
            precision_epoch: None,
        }
    }

    pub fn as_fp32_slice(&self) -> &[f32] {
        &self.values
    }

    pub fn values_byte_size(&self) -> usize {
        self.values.len() * self.precision.bytes_per_element()
    }

    pub fn as_embedding_values(&self) -> EmbeddingValues {
        EmbeddingValues::from_fp32_lossy(&self.values, self.precision)
    }
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
    /// Returns true when this record is a tombstone marker at `snapshot_time_ns`.
    ///
    /// Legacy vector paths encode delete tombstones as records with no embeddings
    /// and `valid_to_ns <= snapshot_time_ns`. Relational paths should converge on
    /// the same visibility rule while storage compaction eventually closes older
    /// versions and removes obsolete tombstones after retention.
    pub fn is_tombstone_at(&self, snapshot_time_ns: i64) -> bool {
        self.valid_to_ns
            .is_some_and(|valid_to_ns| valid_to_ns <= snapshot_time_ns)
            && self.embeddings.is_empty()
            && self.origin.as_deref() == Some("delete")
    }

    /// Returns true when this record is visible at `snapshot_time_ns`.
    ///
    /// `valid_to_ns` is exclusive: a row with `valid_to_ns == snapshot_time_ns`
    /// has already been closed for that snapshot.
    pub fn is_visible_at(&self, snapshot_time_ns: i64) -> bool {
        if self.is_tombstone_at(snapshot_time_ns) {
            return false;
        }

        let valid_from_ok = self
            .valid_from_ns
            .is_none_or(|valid_from_ns| valid_from_ns <= snapshot_time_ns);
        let valid_to_ok = self
            .valid_to_ns
            .is_none_or(|valid_to_ns| snapshot_time_ns < valid_to_ns);
        valid_from_ok && valid_to_ok
    }

    /// Construct a canonical tombstone marker for a logical record id.
    pub fn tombstone(oid: impl Into<String>, timestamp_ns: i64) -> Self {
        Self {
            oid: oid.into(),
            created_at_ns: timestamp_ns,
            updated_at_ns: timestamp_ns,
            valid_to_ns: Some(0),
            origin: Some("delete".to_string()),
            method: Some("tombstone".to_string()),
            ..Self::default()
        }
    }

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
// Search result types (spec §1490)
// ---------------------------------------------------------------------------

/// Canonical search result carrier per MULTIMODAL_OVERHAUL_SPEC §1490.
///
/// Replaces the legacy proto v1 `SearchVectorRecord` type everywhere outside
/// of wire-format protocol adapters. The triple `(record, score, rank)` is
/// the authoritative internal shape for search results.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScoredRecord {
    pub record: ProximaRecord,
    /// Similarity/relevance score (higher is more relevant).
    pub score: f32,
    /// 1-based rank in the result set.
    pub rank: u32,
}

/// Top-level search response envelope (v2).
///
/// Returned by query execution, search services, and hybrid search orchestrators.
/// Protocol adapters (gRPC/REST/Arrow) serialize this into wire shapes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchResponse {
    pub records: Vec<ScoredRecord>,
    pub total_found: i64,
    pub collection_id: String,
    /// Query execution time in microseconds.
    pub query_time_us: u64,
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
        let r1 = ProximaRecord {
            updated_at_ns: 100,
            origin: Some("r1".to_string()),
            ..ProximaRecord::default()
        };

        let r2 = ProximaRecord {
            updated_at_ns: 200,
            origin: Some("r2".to_string()),
            ..ProximaRecord::default()
        };

        let resolved = r1.resolve_conflict(&r2);
        assert_eq!(resolved.origin.unwrap(), "r2");

        let resolved_inverse = r2.resolve_conflict(&r1);
        assert_eq!(resolved_inverse.origin.unwrap(), "r2");
    }

    #[test]
    fn test_memory_type_field() {
        let r = ProximaRecord {
            memory_type: Some(MemoryType::Decision),
            ..ProximaRecord::default()
        };
        assert_eq!(r.memory_type, Some(MemoryType::Decision));
    }

    #[test]
    fn test_tenant_id_is_record_field() {
        let r = ProximaRecord {
            tenant_id: "acme".to_string(),
            ..ProximaRecord::default()
        };
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
        let r = ProximaRecord {
            permitted_principals: vec!["alice".to_string()],
            ..ProximaRecord::default()
        };
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
    fn test_mvcc_visibility_uses_exclusive_valid_to() {
        let r = ProximaRecord {
            valid_from_ns: Some(100),
            valid_to_ns: Some(200),
            ..ProximaRecord::default()
        };

        assert!(!r.is_visible_at(99));
        assert!(r.is_visible_at(100));
        assert!(r.is_visible_at(199));
        assert!(!r.is_visible_at(200));
    }

    #[test]
    fn test_tombstone_is_not_visible() {
        let r = ProximaRecord::tombstone("row-1", 500);
        assert_eq!(r.oid, "row-1");
        assert!(r.is_tombstone_at(500));
        assert!(!r.is_visible_at(500));
    }

    #[test]
    fn test_edge_shape_present_for_graph() {
        let r = ProximaRecord {
            edge: Some(EdgeShape {
                source_id: "node_a".to_string(),
                target_id: "node_b".to_string(),
                edge_type: "KNOWS".to_string(),
                weight: Some(0.9),
            }),
            ..ProximaRecord::default()
        };
        let edge = r.edge.as_ref().unwrap();
        assert_eq!(edge.source_id, "node_a");
        assert_eq!(edge.edge_type, "KNOWS");
        assert!((edge.weight.unwrap() - 0.9).abs() < 1e-9);
    }

    #[test]
    fn test_embedding_cell_fields() {
        let cell = EmbeddingCell::new_fp32(
            "text-embedding-3-small",
            "text",
            3,
            vec![0.1, 0.2, 0.3],
        );
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

    #[test]
    fn test_search_response_carries_scored_records() {
        let record = ProximaRecord {
            oid: "rec_search".to_string(),
            ..ProximaRecord::default()
        };
        let response = SearchResponse {
            records: vec![ScoredRecord {
                record,
                score: 0.98,
                rank: 1,
            }],
            total_found: 1,
            collection_id: "docs".to_string(),
            query_time_us: 42,
        };

        assert_eq!(response.records[0].record.oid, "rec_search");
        assert_eq!(response.records[0].rank, 1);
        assert_eq!(response.total_found, 1);
        assert_eq!(response.collection_id, "docs");
    }

    // ---- EmbeddingScalarType + EmbeddingValues (PR 1, precision rollout) ----

    #[test]
    fn scalar_type_bytes_per_element_match_lld_table() {
        assert_eq!(EmbeddingScalarType::Fp32.bytes_per_element(), 4);
        assert_eq!(EmbeddingScalarType::Fp16.bytes_per_element(), 2);
        assert_eq!(EmbeddingScalarType::Bf16.bytes_per_element(), 2);
        assert_eq!(EmbeddingScalarType::Int8Scalar.bytes_per_element(), 1);
        assert_eq!(EmbeddingScalarType::UInt8Scalar.bytes_per_element(), 1);
    }

    #[test]
    fn scalar_type_lossiness_flags_int8_paths() {
        assert!(!EmbeddingScalarType::Fp32.is_lossy());
        assert!(!EmbeddingScalarType::Fp16.is_lossy());
        assert!(!EmbeddingScalarType::Bf16.is_lossy());
        assert!(EmbeddingScalarType::Int8Scalar.is_lossy());
        assert!(EmbeddingScalarType::UInt8Scalar.is_lossy());
    }

    #[test]
    fn scalar_type_default_is_fp32_for_backward_compat() {
        assert_eq!(EmbeddingScalarType::default(), EmbeddingScalarType::Fp32);
    }

    #[test]
    fn embedding_values_byte_size_matches_layout() {
        let dim = 1024usize;
        assert_eq!(EmbeddingValues::Fp32(vec![0.0; dim]).byte_size(), 4 * dim);
        assert_eq!(
            EmbeddingValues::Fp16(vec![half::f16::from_f32(0.0); dim]).byte_size(),
            2 * dim
        );
        assert_eq!(
            EmbeddingValues::Bf16(vec![half::bf16::from_f32(0.0); dim]).byte_size(),
            2 * dim
        );
        assert_eq!(
            EmbeddingValues::Int8Scalar {
                values: vec![0i8; dim],
                scale: 1.0,
                zero_point: 0,
            }
            .byte_size(),
            dim + 5
        );
        assert_eq!(
            EmbeddingValues::UInt8Scalar {
                values: vec![0u8; dim],
                scale: 1.0,
                zero_point: 128,
            }
            .byte_size(),
            dim + 5
        );
    }

    #[test]
    fn embedding_values_len_matches_inner_vec() {
        let v = vec![1.0, 2.0, 3.0, 4.0];
        let ev = EmbeddingValues::Fp32(v.clone());
        assert_eq!(ev.len(), v.len());
        assert!(!ev.is_empty());
        assert!(EmbeddingValues::Fp32(vec![]).is_empty());
    }

    #[test]
    fn embedding_values_scalar_type_round_trip() {
        for st in [
            EmbeddingScalarType::Fp32,
            EmbeddingScalarType::Fp16,
            EmbeddingScalarType::Bf16,
            EmbeddingScalarType::Int8Scalar,
            EmbeddingScalarType::UInt8Scalar,
        ] {
            let ev = EmbeddingValues::from_fp32_lossy(&[1.0, 2.0, 3.0], st);
            assert_eq!(ev.scalar_type(), st);
        }
    }

    #[test]
    fn embedding_values_as_fp32_slice_only_for_fp32() {
        let v = vec![1.0, 2.0, 3.0];
        assert_eq!(
            EmbeddingValues::Fp32(v.clone()).as_fp32_slice(),
            Some(v.as_slice())
        );
        assert!(EmbeddingValues::from_fp32_lossy(&v, EmbeddingScalarType::Fp16)
            .as_fp32_slice()
            .is_none());
    }

    #[test]
    fn embedding_values_to_fp32_owned_round_trips_fp16_within_tolerance() {
        let src: Vec<f32> = (0..16).map(|i| (i as f32) * 0.1).collect();
        let ev = EmbeddingValues::from_fp32_lossy(&src, EmbeddingScalarType::Fp16);
        let back = ev.to_fp32_owned();
        assert_eq!(back.len(), src.len());
        for (a, b) in src.iter().zip(back.iter()) {
            assert!((a - b).abs() < 0.01, "fp16 RT tolerance fail: {a} vs {b}");
        }
    }

    #[test]
    fn embedding_values_int8_lossy_round_trip_within_per_cell_scale() {
        let src: Vec<f32> = (0..16).map(|i| (i as f32 - 8.0) * 0.1).collect();
        let ev = EmbeddingValues::from_fp32_lossy(&src, EmbeddingScalarType::Int8Scalar);
        let back = ev.to_fp32_owned();
        let quantum = match &ev {
            EmbeddingValues::Int8Scalar { scale, .. } => *scale,
            _ => unreachable!(),
        };
        for (a, b) in src.iter().zip(back.iter()) {
            assert!(
                (a - b).abs() <= quantum * 1.05,
                "int8 RT beyond one quantum: {a} vs {b} q={quantum}"
            );
        }
    }

    #[test]
    fn embedding_values_serde_round_trips_fp32() {
        let ev = EmbeddingValues::Fp32(vec![1.0, 2.0, 3.0]);
        let json = serde_json::to_string(&ev).unwrap();
        let back: EmbeddingValues = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn embedding_values_serde_round_trips_fp16() {
        let ev = EmbeddingValues::from_fp32_lossy(&[0.5, 1.0, 1.5], EmbeddingScalarType::Fp16);
        let json = serde_json::to_string(&ev).unwrap();
        let back: EmbeddingValues = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }

    // ---- EmbeddingCell migration to new schema (PR 1) ----

    #[test]
    fn new_fp32_constructor_sets_default_precision_metadata() {
        let cell = EmbeddingCell::new_fp32("bge-small", "text", 4, vec![0.1, 0.2, 0.3, 0.4]);
        assert_eq!(cell.model_id, "bge-small");
        assert_eq!(cell.modality, "text");
        assert_eq!(cell.dim, 4);
        assert_eq!(cell.values, vec![0.1, 0.2, 0.3, 0.4]);
        assert_eq!(cell.precision, EmbeddingScalarType::Fp32);
        assert_eq!(cell.precision_epoch, None);
    }

    #[test]
    fn embedding_cell_as_fp32_slice_borrows_values() {
        let cell = EmbeddingCell::new_fp32("m", "text", 3, vec![1.0, 2.0, 3.0]);
        assert_eq!(cell.as_fp32_slice(), &[1.0, 2.0, 3.0]);
    }

    #[test]
    fn embedding_cell_byte_size_reflects_declared_precision() {
        let cell = EmbeddingCell::new_fp32("m", "text", 1024, vec![0.0; 1024]);
        assert_eq!(cell.values_byte_size(), 4096);
        let cell16 = EmbeddingCell {
            precision: EmbeddingScalarType::Fp16,
            ..cell
        };
        assert_eq!(cell16.values_byte_size(), 2048);
    }

    #[test]
    fn embedding_cell_serde_back_compat_round_trip() {
        // Old-shape JSON without precision/precision_epoch should deserialize
        // into a cell whose precision defaults to Fp32 and epoch is None.
        let old_json = r#"{
            "model_id": "legacy",
            "modality": "text",
            "values": [0.1, 0.2, 0.3],
            "dim": 3
        }"#;
        let cell: EmbeddingCell = serde_json::from_str(old_json).unwrap();
        assert_eq!(cell.precision, EmbeddingScalarType::Fp32);
        assert_eq!(cell.precision_epoch, None);
        assert_eq!(cell.values, vec![0.1, 0.2, 0.3]);
    }

    #[test]
    fn embedding_cell_as_embedding_values_uses_declared_precision() {
        let mut cell = EmbeddingCell::new_fp32("m", "text", 3, vec![0.0, 1.0, 2.0]);
        cell.precision = EmbeddingScalarType::Fp16;
        let ev = cell.as_embedding_values();
        assert_eq!(ev.scalar_type(), EmbeddingScalarType::Fp16);
        assert_eq!(ev.len(), 3);
    }
}
