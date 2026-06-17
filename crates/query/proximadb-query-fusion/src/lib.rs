use proximadb_data_model::DataModel;
use std::collections::HashMap;

/// Strategy for fusing results from multiple query components.
#[derive(Debug, Clone, Default)]
pub enum FusionStrategy {
    /// Only return records that appear in all component results.
    #[default]
    Intersection,
    /// Return records that appear in any component result.
    Union,
    /// Return records from the first component, filtered by later components.
    FirstWithFilter,
    /// Weighted ranking combining scores from all components.
    RankedFusion {
        /// Weights per data model (default 1.0 if not specified).
        weights: HashMap<DataModel, f64>,
        /// Whether to normalize scores before fusion.
        normalize: bool,
    },
    /// Reciprocal Rank Fusion (RRF).
    ReciprocalRankFusion {
        /// RRF constant (typically 60).
        k: u32,
    },
    /// Projection-aware late fusion for hybrid sparse/dense/vector/graph results.
    ///
    /// This is a planner-visible contract for the B5-style trajectory: candidate
    /// lists are projected into a shared latent/ranking space, then blended with
    /// an exploration/diversity term. Runtime support must prove benchmark parity
    /// before this becomes a default strategy.
    ProjectionFusion {
        /// Relative weight of projected semantic similarity.
        semantic_weight: f64,
        /// Relative weight of structural/document/telemetry context.
        context_weight: f64,
        /// Diversity pressure applied during top-k selection.
        diversity_weight: f64,
        /// Whether to normalize component scores before projection.
        normalize: bool,
    },
    /// Custom fusion using a provided function name.
    Custom(String),
}

impl FusionStrategy {
    /// Validate strategy parameters before planning or execution.
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::Intersection | Self::Union | Self::FirstWithFilter => Ok(()),
            Self::RankedFusion { weights, .. } => {
                if weights
                    .values()
                    .any(|weight| !weight.is_finite() || *weight < 0.0)
                {
                    return Err("ranked fusion weights must be finite and non-negative");
                }
                Ok(())
            }
            Self::ReciprocalRankFusion { k } => {
                if *k == 0 {
                    return Err("rrf k must be > 0");
                }
                Ok(())
            }
            Self::ProjectionFusion {
                semantic_weight,
                context_weight,
                diversity_weight,
                ..
            } => {
                let weights = [*semantic_weight, *context_weight, *diversity_weight];
                if weights
                    .iter()
                    .any(|weight| !weight.is_finite() || *weight < 0.0)
                {
                    return Err("projection fusion weights must be finite and non-negative");
                }
                if weights.iter().sum::<f64>() <= 0.0 {
                    return Err("projection fusion requires at least one positive weight");
                }
                Ok(())
            }
            Self::Custom(name) => {
                if name.is_empty() {
                    return Err("custom fusion name must not be empty");
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_strategy_is_intersection() {
        assert!(matches!(
            FusionStrategy::default(),
            FusionStrategy::Intersection
        ));
    }

    #[test]
    fn ranked_fusion_carries_model_weights() {
        let mut weights = HashMap::new();
        weights.insert(DataModel::Vector, 1.0);
        weights.insert(DataModel::Graph, 0.75);

        let strategy = FusionStrategy::RankedFusion {
            weights,
            normalize: true,
        };

        match strategy {
            FusionStrategy::RankedFusion { weights, normalize } => {
                assert_eq!(weights.len(), 2);
                assert_eq!(weights.get(&DataModel::Graph), Some(&0.75));
                assert!(normalize);
            }
            _ => panic!("expected ranked fusion"),
        }
    }

    #[test]
    fn projection_fusion_carries_projection_parameters() {
        let strategy = FusionStrategy::ProjectionFusion {
            semantic_weight: 0.6,
            context_weight: 0.3,
            diversity_weight: 0.1,
            normalize: true,
        };

        match strategy {
            FusionStrategy::ProjectionFusion {
                semantic_weight,
                context_weight,
                diversity_weight,
                normalize,
            } => {
                assert_eq!(semantic_weight, 0.6);
                assert_eq!(context_weight, 0.3);
                assert_eq!(diversity_weight, 0.1);
                assert!(normalize);
            }
            _ => panic!("expected projection fusion"),
        }
    }

    #[test]
    fn projection_fusion_rejects_invalid_weights() {
        let strategy = FusionStrategy::ProjectionFusion {
            semantic_weight: 0.0,
            context_weight: 0.0,
            diversity_weight: 0.0,
            normalize: true,
        };

        assert_eq!(
            strategy.validate(),
            Err("projection fusion requires at least one positive weight")
        );
    }

    #[test]
    fn rrf_rejects_zero_k() {
        assert_eq!(
            FusionStrategy::ReciprocalRankFusion { k: 0 }.validate(),
            Err("rrf k must be > 0")
        );
    }
}
