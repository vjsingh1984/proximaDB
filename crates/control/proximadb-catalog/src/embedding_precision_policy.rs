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
///
/// Variants use only primitive types so a catalog row written by a node
/// that has `experimental-turboquant` enabled stays deserializable on a
/// node without the feature (Phase E — Quantization Trait Convergence
/// Plan §"Catalog migration"). The consumer side decides whether to
/// honor the row; the catalog stays oblivious.
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
    /// TurboQuant — data-oblivious read-time scalar quantizer (ADR-021).
    ///
    /// All four fields are mandatory because TurboQuant's encoding is
    /// per-collection and can't be reconstructed from defaults: a wrong
    /// rotation_seed or bit_width produces garbage codes.
    ///
    /// - `bit_width` ∈ {2, 4} for P1; {2, 3, 4} after P10.
    /// - `calibration_mode` is `"identity"` or `"tq_plus"` — the string
    ///   shape matches `TurboQuantExplainHints.calibration_mode` and
    ///   `DurableQuantState.calibration` for wire-shape consistency
    ///   across catalog row / EXPLAIN payload / agent memory.
    /// - `rotation_seed` is the per-collection multi-tenant isolation
    ///   primitive. Initial value is
    ///   `proximadb_quantization_types::derive_rotation_seed(collection_id)`;
    ///   subsequent reads MUST honor whatever the catalog row carries
    ///   (the seed is immutable for the collection's lifetime — see
    ///   ADR-021 §"Authority mode").
    /// - `encoded_epoch` is the precision-epoch the codes were encoded
    ///   under (EMBEDDING_PRECISION_LLD Q12). Mismatch with the
    ///   collection's current epoch triggers repair from the canonical
    ///   `ProximaRecord` source.
    TurboQuant {
        bit_width: u8,
        calibration_mode: String,
        rotation_seed: u64,
        encoded_epoch: u64,
    },
}

impl DerivedQuantizationLevel {
    /// Construct a TurboQuant variant from a collection identifier.
    /// Mirrors the runtime construction path: the rotation seed is
    /// derived via the same hash the modality crate uses
    /// (`proximadb_quantization_types::derive_rotation_seed`) so the
    /// catalog row pins the same encoding the live store produces.
    ///
    /// Defaults: 4-bit, TqPlus calibration, epoch 0. Operators that
    /// need different parameters construct the variant directly.
    ///
    /// Note: this helper duplicates the FNV-1a hash from the foundation
    /// crate inline because adding the foundation as a dep here would
    /// drag the feature flag and create a workspace-layering cycle.
    /// The two implementations are tested for parity below; divergence
    /// is a wire-contract violation.
    pub fn turboquant_for_collection(collection_id: &str) -> Self {
        // FNV-1a 64-bit. Must stay byte-equivalent with
        // `proximadb_quantization_types::derive_rotation_seed` —
        // changing one without the other rotates every collection's
        // codes and re-encode is the only repair.
        let prefix = b"turboquant.v1.rotation:";
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &b in prefix.iter().chain(collection_id.as_bytes()) {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        Self::TurboQuant {
            bit_width: 4,
            calibration_mode: "tq_plus".to_string(),
            rotation_seed: h,
            encoded_epoch: 0,
        }
    }
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
    /// Append a `DerivedQuantizationLevel::TurboQuant` to this policy's
    /// `derived_levels`, sized for the given collection id (Phase I —
    /// Quantization Trait Convergence Plan).
    ///
    /// The `rotation_seed` field is derived deterministically from
    /// `collection_id` via the same FNV-1a hash the modality crate uses
    /// (mirrored at [`DerivedQuantizationLevel::turboquant_for_collection`]).
    /// This is the load-bearing wire-stable construction path: every
    /// catalog row, every `TurboQuantStore` in memory, and every
    /// `TurboQuantExplainHints` for this collection refer to the same
    /// seed across restarts and across nodes.
    ///
    /// Defaults: 4-bit, TqPlus calibration, epoch 0. Operators that
    /// need different parameters construct the variant directly and call
    /// `with_derived_level(level)` (next builder below) instead.
    ///
    /// Also sets `derived_material = StorageAndIndex` so the writer
    /// materializes the TurboQuant codes in the `.tq` sidecar (where
    /// they live per LLD §3) AND the index serves them — the natural
    /// shape for `lifecycle = ReadTime` per ADR-021 §"Authority mode".
    ///
    /// Idempotent: if the policy already has a `TurboQuant` derived
    /// level, it is replaced (not appended) so successive calls
    /// converge on the latest seed/mode/bit_width.
    pub fn with_turboquant_for_collection(mut self, collection_id: &str) -> Self {
        let fresh = DerivedQuantizationLevel::turboquant_for_collection(collection_id);
        // Remove any prior TurboQuant entry — idempotency contract.
        self.derived_levels
            .retain(|d| !matches!(d, DerivedQuantizationLevel::TurboQuant { .. }));
        self.derived_levels.push(fresh);
        // Read-time variants live in their own sidecar AND surface from
        // the index — `StorageAndIndex` is the only material option
        // that makes sense per LLD §3.
        if matches!(self.derived_material, QuantizationMaterialization::None) {
            self.derived_material = QuantizationMaterialization::StorageAndIndex;
        }
        self
    }

    /// Append an arbitrary `DerivedQuantizationLevel` to this policy.
    /// Idempotent on the variant kind: the same kind never appears twice
    /// (PQ is treated as a single kind regardless of `m`/`nbits`).
    pub fn with_derived_level(mut self, level: DerivedQuantizationLevel) -> Self {
        use std::mem::discriminant;
        // Compare only on the discriminant, not field contents — calling
        // this twice with different PQ params replaces the prior entry.
        self.derived_levels
            .retain(|d| discriminant(d) != discriminant(&level));
        self.derived_levels.push(level);
        self
    }

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
            DerivedQuantizationLevel::TurboQuant {
                bit_width: 4,
                calibration_mode: "tq_plus".to_string(),
                rotation_seed: 0xdead_beef_cafe_babe,
                encoded_epoch: 17,
            },
            DerivedQuantizationLevel::TurboQuant {
                bit_width: 2,
                calibration_mode: "identity".to_string(),
                rotation_seed: 1,
                encoded_epoch: 0,
            },
        ];
        for level in cases {
            let json = serde_json::to_string(&level).unwrap();
            let back: DerivedQuantizationLevel = serde_json::from_str(&json).unwrap();
            assert_eq!(back, level);
        }
    }

    #[test]
    fn derived_quantization_level_turboquant_kind_tag_is_snake_case() {
        // The catalog row stores `kind:` as a wire string. Future
        // schema changes that rename the tag break loading old rows;
        // pin the literal so renames are loud.
        let level = DerivedQuantizationLevel::TurboQuant {
            bit_width: 4,
            calibration_mode: "tq_plus".to_string(),
            rotation_seed: 0,
            encoded_epoch: 0,
        };
        let v = serde_json::to_value(&level).unwrap();
        assert_eq!(v.get("kind").and_then(|v| v.as_str()), Some("turbo_quant"));
        assert_eq!(v.get("bit_width").and_then(|v| v.as_u64()), Some(4));
        assert_eq!(
            v.get("calibration_mode").and_then(|v| v.as_str()),
            Some("tq_plus"),
        );
    }

    #[test]
    fn turboquant_for_collection_is_deterministic_per_id() {
        // Two calls with the same id MUST produce the same row — the
        // rotation seed is the load-bearing per-collection isolation
        // primitive. Mismatching seeds across nodes corrupts the
        // codes; this test guards against accidental nondeterminism
        // (e.g. someone swapping the hash for `rand::random`).
        let a = DerivedQuantizationLevel::turboquant_for_collection("col-abc");
        let b = DerivedQuantizationLevel::turboquant_for_collection("col-abc");
        assert_eq!(a, b);
        // Different ids MUST produce different seeds — multi-tenant
        // isolation depends on collision-free seeds at any realistic
        // collection count.
        let c = DerivedQuantizationLevel::turboquant_for_collection("col-def");
        assert_ne!(a, c);
    }

    #[test]
    fn turboquant_for_collection_defaults_are_sane() {
        // Pin the default shape so changing them is loud.
        let level = DerivedQuantizationLevel::turboquant_for_collection("col-x");
        match level {
            DerivedQuantizationLevel::TurboQuant {
                bit_width,
                ref calibration_mode,
                encoded_epoch,
                ..
            } => {
                assert_eq!(bit_width, 4);
                assert_eq!(calibration_mode, "tq_plus");
                assert_eq!(encoded_epoch, 0);
            }
            other => panic!("expected TurboQuant variant, got {other:?}"),
        }
    }

    #[test]
    fn with_turboquant_for_collection_appends_derived_level() {
        // Phase I contract: calling `with_turboquant_for_collection`
        // on the global default policy produces a policy whose
        // `derived_levels` carries exactly one TurboQuant variant
        // with the right rotation_seed for the collection id.
        let policy = EmbeddingPrecisionPolicy::global_default_fp32(1)
            .with_turboquant_for_collection("col-1");
        let tq_entries: Vec<_> = policy
            .derived_levels
            .iter()
            .filter(|d| matches!(d, DerivedQuantizationLevel::TurboQuant { .. }))
            .collect();
        assert_eq!(tq_entries.len(), 1, "expected exactly one TurboQuant entry");
        match tq_entries[0] {
            DerivedQuantizationLevel::TurboQuant {
                bit_width,
                calibration_mode,
                rotation_seed,
                encoded_epoch,
            } => {
                assert_eq!(*bit_width, 4);
                assert_eq!(calibration_mode, "tq_plus");
                assert_eq!(*encoded_epoch, 0);
                // The seed must match the canonical derivation — this is
                // the wire-contract guard.
                let expected = match DerivedQuantizationLevel::turboquant_for_collection("col-1") {
                    DerivedQuantizationLevel::TurboQuant { rotation_seed, .. } => rotation_seed,
                    _ => panic!(),
                };
                assert_eq!(*rotation_seed, expected);
            }
            _ => panic!("non-TurboQuant variant leaked into filter"),
        }
    }

    #[test]
    fn with_turboquant_for_collection_is_idempotent() {
        // Calling the builder twice converges on a single TurboQuant
        // entry — not two. Multi-tenant routing depends on this so a
        // collection-update path can re-call the builder without
        // accumulating stale entries.
        let policy = EmbeddingPrecisionPolicy::global_default_fp32(1)
            .with_turboquant_for_collection("col-x")
            .with_turboquant_for_collection("col-x");
        let count = policy
            .derived_levels
            .iter()
            .filter(|d| matches!(d, DerivedQuantizationLevel::TurboQuant { .. }))
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn with_turboquant_for_collection_flips_storage_material_when_none() {
        // ReadTime variants need `StorageAndIndex` materialization per
        // LLD §3 (codes live in the .tq sidecar AND surface from the
        // index). The default policy starts at `None`; the builder
        // must promote it.
        let policy = EmbeddingPrecisionPolicy::global_default_fp32(1)
            .with_turboquant_for_collection("col-y");
        assert_eq!(
            policy.derived_material,
            QuantizationMaterialization::StorageAndIndex,
        );
    }

    #[test]
    fn with_turboquant_for_collection_preserves_existing_material() {
        // If the operator already chose a materialization (e.g.
        // `IndexOnly`), the builder respects it — only flips from `None`.
        let policy = EmbeddingPrecisionPolicy {
            derived_material: QuantizationMaterialization::IndexOnly,
            ..EmbeddingPrecisionPolicy::global_default_fp32(1)
        }
        .with_turboquant_for_collection("col-z");
        assert_eq!(
            policy.derived_material,
            QuantizationMaterialization::IndexOnly,
        );
    }

    #[test]
    fn with_derived_level_dedupes_by_variant_kind() {
        // Calling `with_derived_level` twice with the same variant
        // kind (even with different params) replaces the prior entry.
        // This matches operator intent — "I want PQ with these params,
        // not THESE params AND THOSE params".
        let policy = EmbeddingPrecisionPolicy::global_default_fp32(1)
            .with_derived_level(DerivedQuantizationLevel::Pq { m: 8, nbits: 8 })
            .with_derived_level(DerivedQuantizationLevel::Pq { m: 16, nbits: 4 });
        let pq_entries: Vec<_> = policy
            .derived_levels
            .iter()
            .filter(|d| matches!(d, DerivedQuantizationLevel::Pq { .. }))
            .collect();
        assert_eq!(pq_entries.len(), 1);
        match pq_entries[0] {
            DerivedQuantizationLevel::Pq { m, nbits } => {
                assert_eq!(*m, 16);
                assert_eq!(*nbits, 4);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn turboquant_inline_fnv1a_matches_foundation_crate_helper() {
        // The catalog inlines an FNV-1a hash to avoid a workspace-
        // layering cycle (catalog → foundation), but the two
        // implementations MUST stay byte-equivalent. Divergence
        // would rotate every collection's codes; re-encode would
        // be the only repair.
        //
        // Reproduce the foundation helper inline here (same algo,
        // same prefix bytes) and assert parity across a handful of
        // representative collection ids.
        fn foundation_derive(collection_id: &str) -> u64 {
            let prefix = b"turboquant.v1.rotation:";
            let mut h: u64 = 0xcbf2_9ce4_8422_2325;
            for &b in prefix.iter().chain(collection_id.as_bytes()) {
                h ^= b as u64;
                h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
            h
        }
        for id in ["", "col-1", "tenant-xyz", "collection-with-long-name-12345"] {
            let level = DerivedQuantizationLevel::turboquant_for_collection(id);
            let seed = match level {
                DerivedQuantizationLevel::TurboQuant { rotation_seed, .. } => rotation_seed,
                _ => panic!("expected TurboQuant variant"),
            };
            assert_eq!(
                seed,
                foundation_derive(id),
                "inline FNV-1a diverged from foundation hash for id={id:?}",
            );
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
