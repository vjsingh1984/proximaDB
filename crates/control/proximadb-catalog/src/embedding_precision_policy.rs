//! xCatalog precision policy schema — embedding-precision rollout PR 6.
//!
//! Mirrors the SQL DDL locked in
//! `docs/12-design/EMBEDDING_PRECISION_LLD_2026_05_22.adoc` §"xCatalog table
//! for precision policy". The catalog persists one row per (policy_id,
//! policy_version) tuple; collections reference a (policy_id, version) pair
//! and may inherit from the global default seeded at startup.
//!
//! Layering: this module is contract-only. It owns the types and the
//! `GLOBAL_DEFAULT_POLICY_ID` constant; the catalog backend (sqlx or
//! filestore) translates rows to/from these structs. WAL segment headers
//! (PR 4) carry the `(policy_id, policy_version)` pair so readers can
//! rejoin a record with the exact policy that wrote it without consulting
//! the live catalog row (which may have been bumped since).

use proximadb_records::EmbeddingScalarType;
use serde::{Deserialize, Serialize};

/// Policy id seeded at server startup so every cluster has a known-good
/// fp32-only baseline. New collections that omit a `precision_policy_id`
/// inherit from this row.
pub const GLOBAL_DEFAULT_POLICY_ID: &str = "global_default_fp32";

/// DDL string locked by the LLD. Catalog backends (Postgres, MySQL, SQLite,
/// filestore-emulated) all materialize the same shape so the
/// `(policy_id, policy_version)` reference from a WAL segment header is
/// resolvable across backends.
pub const EMBEDDING_PRECISION_POLICY_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS embedding_precision_policy (
    policy_id            TEXT NOT NULL,
    policy_version       BIGINT NOT NULL,
    source               TEXT NOT NULL,
    mode                 TEXT NOT NULL,
    canonical_default    TEXT NOT NULL,
    canonical_allowed    JSONB NOT NULL,
    ingest_mismatch      TEXT NOT NULL,
    retain_fp32_shadow   BOOLEAN NOT NULL,
    derived_levels       JSONB NOT NULL,
    derived_material     TEXT NOT NULL,
    max_overhead_ratio   FLOAT NOT NULL,
    rebuildable          BOOLEAN NOT NULL,
    default_search_level TEXT NOT NULL,
    allowed_search_hints JSONB NOT NULL,
    rerank_with          TEXT NOT NULL,
    reject_unknown_recall BOOLEAN NOT NULL,
    min_recall_at_10     JSONB NOT NULL,
    min_recall_at_100    JSONB NOT NULL,
    max_latency_regression_pct FLOAT NOT NULL,
    require_hw_capability BOOLEAN NOT NULL,
    require_explain_visibility BOOLEAN NOT NULL,
    created_at_ns        BIGINT NOT NULL,
    PRIMARY KEY (policy_id, policy_version)
);
"#;

/// Where the precision policy originated. Catalog stores this so multi-
/// source overrides (`tenant_tier` can override `global`, `collection`
/// overrides `tenant_tier`, etc.) are auditable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicySource {
    Global,
    TenantTier,
    Collection,
    QueryHint,
}

/// Adaptive policies bump `policy_version` when the canonical precision is
/// auto-promoted by the catalog (e.g. after a recall regression); fixed
/// policies require an operator DDL to change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyMode {
    Fixed,
    Adaptive,
}

/// How the API ingress reacts when a request's declared precision differs
/// from the collection's `canonical_default`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IngestMismatchPolicy {
    /// Reject the write with `unsupported_precision_*` (PR 3 behaviour for
    /// non-Fp32 records while schema_v2 is off).
    Reject,
    /// Convert once at the API boundary to the canonical default. Lossy
    /// when the conversion narrows precision.
    ConvertOnce,
    /// Persist the record at the declared precision and let mixed-precision
    /// segments coexist (requires PR 5 PAX per-column alignment).
    AcceptMixed,
}

/// What additional derived (quantized) representations the writer should
/// materialize alongside the canonical embedding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DerivedQuantizationLevel {
    Binary,
    Int8,
    Int4,
    /// Product-quantization with `m` sub-vectors and `nbits` per code.
    Pq {
        m: u32,
        nbits: u8,
    },
}

/// Where derived quantizations are materialized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuantizationMaterialization {
    /// Don't materialize — derived levels are recomputed on the fly.
    None,
    /// Index keeps a quantized copy; storage carries only the canonical.
    IndexOnly,
    /// Storage carries an aux column; index uses canonical.
    StorageAux,
    /// Storage AND index carry the quantized form.
    StorageAndIndex,
}

/// Re-ranking strategy applied after a quantized candidate-set search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RerankPrecision {
    None,
    /// Re-rank in the collection's canonical precision (fp32 today).
    Canonical,
    /// Re-rank after promoting candidates to fp32 (matters when canonical is
    /// fp16/bf16 and the re-rank kernel is more accurate at fp32).
    Fp32Promoted,
}

/// Where a collection is in its precision-migration lifecycle. A migration
/// from fp32 → fp16 typically goes `Stable → ShadowingTarget →
/// CutoverPending → Stable` over multiple compactions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrecisionMigrationState {
    Stable,
    ShadowingTarget,
    CutoverPending,
    RollingBack,
}

/// Per-metric recall@K target.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RecallTargets {
    pub at_10: f32,
    pub at_100: f32,
}

/// Per-distance-metric recall SLO. LLD §Q13 locks the defaults that ship
/// with the global default policy — operators can override per policy.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RecallSlo {
    pub cosine: RecallTargets,
    pub l2: RecallTargets,
    pub dot: RecallTargets,
}

impl Default for RecallSlo {
    fn default() -> Self {
        Self::lld_defaults()
    }
}

impl RecallSlo {
    /// LLD §Q13 per-metric recall defaults. Cosine + L2 share fp16-noise
    /// tolerance (normalized magnitudes); dot product needs tighter recall
    /// because raw magnitude affects ranking.
    pub const fn lld_defaults() -> Self {
        Self {
            cosine: RecallTargets {
                at_10: 0.99,
                at_100: 0.995,
            },
            l2: RecallTargets {
                at_10: 0.99,
                at_100: 0.995,
            },
            dot: RecallTargets {
                at_10: 0.995,
                at_100: 0.998,
            },
        }
    }
}

/// Full precision policy row as it lives in the catalog.
///
/// The primary key is `(policy_id, policy_version)`; the catalog backend
/// appends a new version (rather than updating in place) so historical
/// segments can resolve the exact policy under which they were written.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmbeddingPrecisionPolicy {
    pub policy_id: String,
    pub policy_version: u64,
    pub source: PolicySource,
    pub mode: PolicyMode,
    pub canonical_default: EmbeddingScalarType,
    pub canonical_allowed: Vec<EmbeddingScalarType>,
    pub ingest_mismatch: IngestMismatchPolicy,
    pub retain_fp32_shadow: bool,
    pub derived_levels: Vec<DerivedQuantizationLevel>,
    pub derived_material: QuantizationMaterialization,
    pub max_overhead_ratio: f32,
    pub rebuildable: bool,
    pub default_search_level: String,
    pub allowed_search_hints: Vec<String>,
    pub rerank_with: RerankPrecision,
    pub reject_unknown_recall: bool,
    pub min_recall_at_10: RecallSlo,
    pub min_recall_at_100: RecallSlo,
    pub max_latency_regression_pct: f32,
    pub require_hw_capability: bool,
    pub require_explain_visibility: bool,
    pub created_at_ns: i64,
}

impl EmbeddingPrecisionPolicy {
    /// LLD-locked default policy seeded at server startup. Every cluster
    /// starts with this row so existing collections (which carry no
    /// `policy_id` field on disk before PR 6) can inherit predictable
    /// fp32-only behaviour.
    pub fn global_default_fp32(created_at_ns: i64) -> Self {
        Self {
            policy_id: GLOBAL_DEFAULT_POLICY_ID.to_string(),
            policy_version: 1,
            source: PolicySource::Global,
            mode: PolicyMode::Fixed,
            canonical_default: EmbeddingScalarType::Fp32,
            canonical_allowed: vec![EmbeddingScalarType::Fp32],
            ingest_mismatch: IngestMismatchPolicy::Reject,
            retain_fp32_shadow: true,
            derived_levels: Vec::new(),
            derived_material: QuantizationMaterialization::None,
            max_overhead_ratio: 0.0,
            rebuildable: true,
            default_search_level: "canonical".to_string(),
            allowed_search_hints: vec!["canonical".to_string()],
            rerank_with: RerankPrecision::None,
            reject_unknown_recall: true,
            min_recall_at_10: RecallSlo::lld_defaults(),
            min_recall_at_100: RecallSlo::lld_defaults(),
            max_latency_regression_pct: 10.0,
            require_hw_capability: false,
            require_explain_visibility: true,
            created_at_ns,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ddl_string_matches_lld_columns() {
        // Spot-check the DDL string includes every column listed in the LLD.
        for col in [
            "policy_id",
            "policy_version",
            "source",
            "mode",
            "canonical_default",
            "canonical_allowed",
            "ingest_mismatch",
            "retain_fp32_shadow",
            "derived_levels",
            "derived_material",
            "max_overhead_ratio",
            "rebuildable",
            "default_search_level",
            "allowed_search_hints",
            "rerank_with",
            "reject_unknown_recall",
            "min_recall_at_10",
            "min_recall_at_100",
            "max_latency_regression_pct",
            "require_hw_capability",
            "require_explain_visibility",
            "created_at_ns",
        ] {
            assert!(
                EMBEDDING_PRECISION_POLICY_DDL.contains(col),
                "DDL missing column {col}"
            );
        }
        assert!(EMBEDDING_PRECISION_POLICY_DDL.contains("PRIMARY KEY (policy_id, policy_version)"));
    }

    #[test]
    fn global_default_policy_id_constant_matches_lld() {
        assert_eq!(GLOBAL_DEFAULT_POLICY_ID, "global_default_fp32");
    }

    #[test]
    fn recall_slo_defaults_match_lld_q13_table() {
        let slo = RecallSlo::lld_defaults();
        assert_eq!(slo.cosine.at_10, 0.99);
        assert_eq!(slo.cosine.at_100, 0.995);
        assert_eq!(slo.l2.at_10, 0.99);
        assert_eq!(slo.l2.at_100, 0.995);
        assert_eq!(slo.dot.at_10, 0.995, "dot needs tighter recall (LLD §Q13)");
        assert_eq!(slo.dot.at_100, 0.998);
    }

    #[test]
    fn global_default_policy_is_fp32_reject() {
        let p = EmbeddingPrecisionPolicy::global_default_fp32(0);
        assert_eq!(p.policy_id, GLOBAL_DEFAULT_POLICY_ID);
        assert_eq!(p.policy_version, 1);
        assert_eq!(p.source, PolicySource::Global);
        assert_eq!(p.mode, PolicyMode::Fixed);
        assert_eq!(p.canonical_default, EmbeddingScalarType::Fp32);
        assert_eq!(p.canonical_allowed, vec![EmbeddingScalarType::Fp32]);
        // The default seed must reject non-fp32 ingest until an operator
        // bumps the policy; this is what makes the rolling deploy safe.
        assert_eq!(p.ingest_mismatch, IngestMismatchPolicy::Reject);
        assert!(p.retain_fp32_shadow);
        assert!(p.derived_levels.is_empty());
        assert_eq!(p.derived_material, QuantizationMaterialization::None);
        assert_eq!(p.rerank_with, RerankPrecision::None);
    }

    #[test]
    fn policy_serde_round_trip() {
        let policy = EmbeddingPrecisionPolicy::global_default_fp32(1_700_000_000_000_000_000);
        let json = serde_json::to_string(&policy).unwrap();
        let back: EmbeddingPrecisionPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(back, policy);
    }

    #[test]
    fn derived_quantization_level_serde_round_trip_each_variant() {
        let cases = [
            DerivedQuantizationLevel::Binary,
            DerivedQuantizationLevel::Int8,
            DerivedQuantizationLevel::Int4,
            DerivedQuantizationLevel::Pq { m: 8, nbits: 8 },
        ];
        for level in cases {
            let json = serde_json::to_string(&level).unwrap();
            let back: DerivedQuantizationLevel = serde_json::from_str(&json).unwrap();
            assert_eq!(back, level);
        }
    }

    #[test]
    fn enum_string_tags_match_ddl_text_values() {
        // The DDL columns store these as TEXT; the kebab/snake-case tags
        // must round-trip the exact strings the catalog backend expects.
        for (val, expected) in [
            (PolicySource::Global, "\"global\""),
            (PolicySource::TenantTier, "\"tenant_tier\""),
            (PolicySource::Collection, "\"collection\""),
            (PolicySource::QueryHint, "\"query_hint\""),
        ] {
            assert_eq!(serde_json::to_string(&val).unwrap(), expected);
        }
        for (val, expected) in [
            (PolicyMode::Fixed, "\"fixed\""),
            (PolicyMode::Adaptive, "\"adaptive\""),
        ] {
            assert_eq!(serde_json::to_string(&val).unwrap(), expected);
        }
        for (val, expected) in [
            (IngestMismatchPolicy::Reject, "\"reject\""),
            (IngestMismatchPolicy::ConvertOnce, "\"convert_once\""),
            (IngestMismatchPolicy::AcceptMixed, "\"accept_mixed\""),
        ] {
            assert_eq!(serde_json::to_string(&val).unwrap(), expected);
        }
        for (val, expected) in [
            (QuantizationMaterialization::None, "\"none\""),
            (QuantizationMaterialization::IndexOnly, "\"index_only\""),
            (QuantizationMaterialization::StorageAux, "\"storage_aux\""),
            (
                QuantizationMaterialization::StorageAndIndex,
                "\"storage_and_index\"",
            ),
        ] {
            assert_eq!(serde_json::to_string(&val).unwrap(), expected);
        }
        for (val, expected) in [
            (RerankPrecision::None, "\"none\""),
            (RerankPrecision::Canonical, "\"canonical\""),
            (RerankPrecision::Fp32Promoted, "\"fp32_promoted\""),
        ] {
            assert_eq!(serde_json::to_string(&val).unwrap(), expected);
        }
    }

    #[test]
    fn precision_migration_state_default_path() {
        // Operators expect a fresh collection to start in Stable.
        // We don't impl Default for the enum (it's stage-machine sensitive)
        // but we lock the round-trip and variant list here.
        for state in [
            PrecisionMigrationState::Stable,
            PrecisionMigrationState::ShadowingTarget,
            PrecisionMigrationState::CutoverPending,
            PrecisionMigrationState::RollingBack,
        ] {
            let json = serde_json::to_string(&state).unwrap();
            let back: PrecisionMigrationState = serde_json::from_str(&json).unwrap();
            assert_eq!(back, state);
        }
    }
}
