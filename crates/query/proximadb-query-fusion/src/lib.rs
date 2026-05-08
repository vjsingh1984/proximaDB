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
    /// Custom fusion using a provided function name.
    Custom(String),
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
}
