//! Canonical ranking score types shared across the search and query crates.
//!
//! Lives in `proximadb-kernel` so the root crate (`proximadb`) and the
//! workspace `proximadb-query` crate can both reference the same canonical
//! definitions without a circular dependency.
//!
//! See `roadmap/RANKING_FRAMEWORK_SPEC_2026_05_23.md` for the multi-phase
//! ranking framework these types underpin (Phase R-0).
//!
//! Design invariants:
//! - `OptimizedSearchRecord::score_vector` is `Option<ScoreVector>`; `None`
//!   when no rank profile is attached. This keeps the no-profile path
//!   zero-cost (NFR-9 in the spec).
//! - `ScoreVector::primary` always equals the value driving sort order.
//! - `ScoreComponent` uses `f64` for value/weight/contribution to match the
//!   existing reranker math, even though `primary` is `f32`. The promotion
//!   happens at component aggregation time.

use std::sync::Arc;

/// Stable identifier for a ranking phase.
///
/// `0 = first`, `1 = second`, `2 = global`. Encoded as `u8` for compact
/// wire representation. Higher values are reserved for future phases.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct PhaseId(pub u8);

impl PhaseId {
    pub const FIRST: PhaseId = PhaseId(0);
    pub const SECOND: PhaseId = PhaseId(1);
    pub const GLOBAL: PhaseId = PhaseId(2);
}

impl Default for PhaseId {
    fn default() -> Self {
        PhaseId::FIRST
    }
}

/// A single named contribution to a final ranking score.
///
/// The semantic invariant is: `contribution == value * weight` (within
/// floating-point tolerance). Producers that compose components via a
/// different rule (e.g. logistic mixing) must still report a sensible
/// `contribution` that callers can sum for explainability.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ScoreComponent {
    /// Component name (e.g. `bm25(title)`, `closeness(embedding)`, `model(rerank-v3)`).
    pub name: String,
    /// Raw component value.
    pub value: f64,
    /// Weight applied at composition time.
    pub weight: f64,
    /// Contribution to the final score. Typically `value * weight`.
    pub contribution: f64,
}

/// Multi-component score produced by a rank pipeline.
///
/// Populated only when a rank profile is attached to the collection or query.
/// `primary` is what drives sort order; `components` carries per-feature
/// attribution for explainability and offline training-data export.
///
/// Components are held as `Arc<[ScoreComponent]>` so result clones during
/// merge/sort do not duplicate the allocation.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ScoreVector {
    /// The value driving sort order. Mirrors `OptimizedSearchRecord::score`.
    pub primary: f32,
    /// Which phase produced `primary`.
    #[serde(default)]
    pub phase: PhaseId,
    /// Per-feature attribution. Empty when match-features are not requested.
    #[serde(
        default = "empty_components",
        skip_serializing_if = "components_is_empty",
        with = "arc_components_serde"
    )]
    pub components: Arc<[ScoreComponent]>,
}

fn empty_components() -> Arc<[ScoreComponent]> {
    Arc::<[ScoreComponent]>::from(Vec::new())
}

/// Serde adapter for `Arc<[ScoreComponent]>` — serde does not provide a
/// derive for `Arc<[T]>` out of the box. Mirrors the `arc_slice_serde`
/// pattern in `src/core/search/results.rs`.
mod arc_components_serde {
    use super::ScoreComponent;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::sync::Arc;

    pub fn serialize<S>(c: &Arc<[ScoreComponent]>, ser: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        c.as_ref().serialize(ser)
    }

    pub fn deserialize<'de, D>(de: D) -> Result<Arc<[ScoreComponent]>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let v = Vec::<ScoreComponent>::deserialize(de)?;
        Ok(Arc::from(v.into_boxed_slice()))
    }
}

impl Default for ScoreVector {
    fn default() -> Self {
        Self {
            primary: 0.0,
            phase: PhaseId::FIRST,
            components: Arc::<[ScoreComponent]>::from(Vec::new()),
        }
    }
}

fn components_is_empty(c: &Arc<[ScoreComponent]>) -> bool {
    c.is_empty()
}

impl ScoreVector {
    /// Construct a `ScoreVector` with no per-feature attribution.
    pub fn from_primary(primary: f32, phase: PhaseId) -> Self {
        Self {
            primary,
            phase,
            components: Arc::<[ScoreComponent]>::from(Vec::new()),
        }
    }

    /// Construct a `ScoreVector` with components.
    pub fn new(primary: f32, phase: PhaseId, components: Vec<ScoreComponent>) -> Self {
        Self {
            primary,
            phase,
            components: Arc::from(components.into_boxed_slice()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_id_constants_are_stable() {
        // Wire-format guarantee: these numeric tags must not change.
        assert_eq!(PhaseId::FIRST.0, 0);
        assert_eq!(PhaseId::SECOND.0, 1);
        assert_eq!(PhaseId::GLOBAL.0, 2);
    }

    #[test]
    fn phase_id_default_is_first() {
        assert_eq!(PhaseId::default(), PhaseId::FIRST);
    }

    #[test]
    fn phase_id_serializes_as_bare_integer() {
        // #[serde(transparent)] ensures the on-wire form is a plain u8,
        // not a tagged struct. Critical for forward-compat with raw integer
        // phase ids in proto payloads.
        let j = serde_json::to_string(&PhaseId::SECOND).unwrap();
        assert_eq!(j, "1");
    }

    #[test]
    fn score_component_round_trips() {
        let c = ScoreComponent {
            name: "bm25(title)".to_string(),
            value: 12.4,
            weight: 0.4,
            contribution: 4.96,
        };
        let j = serde_json::to_string(&c).unwrap();
        let back: ScoreComponent = serde_json::from_str(&j).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn score_vector_default_has_empty_components() {
        let v = ScoreVector::default();
        assert_eq!(v.primary, 0.0);
        assert_eq!(v.phase, PhaseId::FIRST);
        assert!(v.components.is_empty());
    }

    #[test]
    fn score_vector_empty_components_omitted_from_json() {
        // NFR-9: when match-features aren't requested, payload stays compact.
        let v = ScoreVector::from_primary(0.87, PhaseId::GLOBAL);
        let j = serde_json::to_string(&v).unwrap();
        assert!(!j.contains("components"), "got: {j}");
    }

    #[test]
    fn score_vector_with_components_round_trips() {
        let v = ScoreVector::new(
            0.87,
            PhaseId::GLOBAL,
            vec![
                ScoreComponent {
                    name: "bm25(title)".to_string(),
                    value: 12.4,
                    weight: 0.4,
                    contribution: 4.96,
                },
                ScoreComponent {
                    name: "closeness(embedding)".to_string(),
                    value: 0.91,
                    weight: 0.6,
                    contribution: 0.546,
                },
            ],
        );
        let j = serde_json::to_string(&v).unwrap();
        let back: ScoreVector = serde_json::from_str(&j).unwrap();
        assert_eq!(v, back);
        assert_eq!(back.components.len(), 2);
    }

    #[test]
    fn score_vector_components_arc_clones_are_cheap() {
        // Cloning a ScoreVector must not duplicate the components vec.
        let v = ScoreVector::new(
            1.0,
            PhaseId::FIRST,
            vec![ScoreComponent {
                name: "a".into(),
                value: 1.0,
                weight: 1.0,
                contribution: 1.0,
            }],
        );
        let v2 = v.clone();
        assert!(
            Arc::ptr_eq(&v.components, &v2.components),
            "cloning ScoreVector must share the components Arc"
        );
    }

    #[test]
    fn score_vector_from_primary_has_empty_components() {
        let v = ScoreVector::from_primary(0.5, PhaseId::SECOND);
        assert_eq!(v.primary, 0.5);
        assert_eq!(v.phase, PhaseId::SECOND);
        assert!(v.components.is_empty());
    }

    #[test]
    fn score_component_contribution_invariant_documented() {
        // Documenting the convention: producers SHOULD set
        // `contribution == value * weight`.
        // (Not enforced — composition rules vary — but this test pins the doc.)
        let c = ScoreComponent {
            name: "x".into(),
            value: 2.0,
            weight: 3.0,
            contribution: 6.0,
        };
        let expected = c.value * c.weight;
        assert!((c.contribution - expected).abs() < 1e-9);
    }
}
