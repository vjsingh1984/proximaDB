//! Typed schema for rank profiles. Serde-friendly, TOML-driven.
//!
//! Mirrors spec §4.5.2 with R-4 tweaks: `version`/`created_at_ms` are
//! filled in by the repository at create/update time, not by the user.

use serde::{Deserialize, Serialize};

/// A rank profile — declarative scoring pipeline.
///
/// Validated by [`crate::validator::validate`]; compiled into a
/// [`crate::compiled::CompiledRankProfile`] which `materialize`s a
/// [`proximadb_rank_core::RankPipeline`] per query.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RankProfileSpec {
    /// Globally unique profile name. Defaults to empty during deserialization
    /// because both [`crate::parse_single`] (caller supplies the name) and
    /// [`crate::parse_document`] (name comes from the table key) override it.
    #[serde(default)]
    pub name: String,
    /// Name of a parent profile this one inherits from. Single inheritance;
    /// resolved by [`crate::validator::resolve_inheritance`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inherits: Option<String>,
    /// Free-form human description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_phase: Option<PhaseSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub second_phase: Option<PhaseSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_phase: Option<GlobalPhaseSpec>,

    /// Features whose values are emitted per-hit alongside the score.
    /// Each is a feature-name string; can be a bare ident (`bm25(title)`)
    /// or any expression-compatible reference.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub match_features: Vec<String>,
    /// Features emitted only in the summary phase.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub summary_features: Vec<String>,

    /// Per-phase wall-clock budgets in microseconds.
    #[serde(default)]
    pub budget: PhaseBudgetSpec,

    /// User-defined intermediate functions (R-9 will wire these into the
    /// expression VM as callable forms; R-4 just persists them).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub functions: Vec<FunctionSpec>,

    /// Numeric constants referenced by expressions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constants: Vec<ConstantSpec>,

    /// Repository-assigned: monotonically increasing version per (name).
    #[serde(default)]
    pub version: u32,
    /// Repository-assigned: ms-since-Unix-epoch of the latest update.
    #[serde(default)]
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhaseSpec {
    /// Rank expression (parsed via `proximadb_rank_expr`).
    pub expression: String,
    /// First-phase: how many hits the heap should keep.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heap_size: Option<u32>,
    /// Second-phase: how many top-K hits to rescore.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rerank_count: Option<u32>,
    /// Optional cross-encoder batch size; ignored if the expression has
    /// no ONNX model features.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_size: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GlobalPhaseSpec {
    /// Strategy id: `"cross_modal"` routes to existing `CrossModalReranker`
    /// (R-6); `"expression"` evaluates an expression over post-merge hits
    /// (R-7); `"llm_listwise"` calls a hosted reranker (R-6 extension).
    pub strategy: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rerank_count: Option<u32>,
    /// Strategy-specific config bag (free-form JSON).
    #[serde(default)]
    pub config: serde_json::Value,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PhaseBudgetSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_max_us: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub second_max_us: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_max_us: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionSpec {
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    pub expression: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConstantSpec {
    pub name: String,
    pub value: f64,
}

impl RankProfileSpec {
    /// Convenience constructor for tests / programmatic profile creation.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            inherits: None,
            description: None,
            first_phase: None,
            second_phase: None,
            global_phase: None,
            match_features: Vec::new(),
            summary_features: Vec::new(),
            budget: PhaseBudgetSpec::default(),
            functions: Vec::new(),
            constants: Vec::new(),
            version: 0,
            created_at_ms: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profile_round_trips_through_json() {
        let p = RankProfileSpec::new("test");
        let j = serde_json::to_string(&p).unwrap();
        let back: RankProfileSpec = serde_json::from_str(&j).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn empty_optional_fields_omitted_from_json() {
        let p = RankProfileSpec::new("test");
        let j = serde_json::to_string(&p).unwrap();
        // No first_phase, no inherits, etc.
        assert!(!j.contains("first_phase"));
        assert!(!j.contains("inherits"));
        assert!(!j.contains("match_features"));
    }

    #[test]
    fn full_profile_round_trips_through_json() {
        let p = RankProfileSpec {
            name: "x".into(),
            inherits: Some("default".into()),
            description: Some("test".into()),
            first_phase: Some(PhaseSpec {
                expression: "bm25(\"t\")".into(),
                heap_size: Some(1000),
                rerank_count: None,
                batch_size: None,
            }),
            second_phase: Some(PhaseSpec {
                expression: "attr(\"x\") * 2".into(),
                heap_size: None,
                rerank_count: Some(100),
                batch_size: Some(32),
            }),
            global_phase: Some(GlobalPhaseSpec {
                strategy: "cross_modal".into(),
                rerank_count: Some(50),
                config: serde_json::json!({"mmr_lambda": 0.7}),
            }),
            match_features: vec!["bm25(\"t\")".into()],
            summary_features: vec!["attr(\"x\")".into()],
            budget: PhaseBudgetSpec {
                first_max_us: Some(5000),
                second_max_us: Some(50_000),
                global_max_us: Some(100_000),
            },
            functions: vec![FunctionSpec {
                name: "personalized".into(),
                args: vec!["user_id".into()],
                expression: "attr(\"affinity\")".into(),
            }],
            constants: vec![ConstantSpec {
                name: "w_bm25".into(),
                value: 0.4,
            }],
            version: 7,
            created_at_ms: 1_700_000_000_000,
        };
        let j = serde_json::to_string(&p).unwrap();
        let back: RankProfileSpec = serde_json::from_str(&j).unwrap();
        assert_eq!(p, back);
    }
}
