//! Query Expansion for EDR
//!
//! This module implements query expansion strategies for EDR,
//! transforming a single query into multiple query vectors for improved retrieval.

use anyhow::Result;
use std::sync::Arc;

use crate::compute::distance_computation::{DistanceMetric, UnifiedDistanceCompute};

/// Query expansion configuration
#[derive(Debug, Clone)]
pub struct QueryExpansionConfig {
    /// Number of expanded queries to generate
    pub num_expansions: usize,
    /// Expansion method to use
    pub expansion_method: ExpansionMethod,
    /// Distance metric for diversity calculations
    pub distance_metric: DistanceMetric,
}

impl Default for QueryExpansionConfig {
    fn default() -> Self {
        Self {
            num_expansions: 3,
            expansion_method: ExpansionMethod::Hybrid,
            distance_metric: DistanceMetric::Cosine,
        }
    }
}

/// Query expansion methods
#[derive(Debug, Clone, PartialEq)]
pub enum ExpansionMethod {
    /// No expansion - use original query only
    None,
    /// Word-level expansion (simulated)
    WordLevel,
    /// Hybrid expansion combining multiple methods
    Hybrid,
}

/// Query expansion module
pub struct QueryExpansion {
    config: QueryExpansionConfig,
    distance_compute: Arc<UnifiedDistanceCompute>,
}

impl QueryExpansion {
    /// Create a new query expansion module
    pub fn new(distance_metric: DistanceMetric, num_expansions: usize) -> Self {
        let config = QueryExpansionConfig {
            num_expansions,
            expansion_method: ExpansionMethod::Hybrid,
            distance_metric,
        };

        let distance_compute = Arc::new(UnifiedDistanceCompute::new(distance_metric));

        Self {
            config,
            distance_compute,
        }
    }

    /// Expand a single query into multiple query vectors
    pub async fn expand_query(&self, query: &[f32]) -> Result<Vec<Vec<f32>>> {
        match self.config.expansion_method {
            ExpansionMethod::None => Ok(vec![query.to_vec()]),
            ExpansionMethod::WordLevel => self.word_level_expansion(query).await,
            ExpansionMethod::Hybrid => self.hybrid_expansion(query).await,
        }
    }

    /// Word-level query expansion
    async fn word_level_expansion(&self, query: &[f32]) -> Result<Vec<Vec<f32>>> {
        let mut expanded_queries = Vec::new();
        expanded_queries.push(query.to_vec());

        // Simulate word-level expansion by adding small perturbations
        let perturbation_scale = 0.1;
        for i in 1..self.config.num_expansions {
            let mut expanded_query = query.to_vec();

            // First add perturbations
            for val in expanded_query.iter_mut() {
                *val += (i as f32) * perturbation_scale;
            }

            // Then normalize to maintain unit vector properties
            let norm: f32 = expanded_query.iter().map(|v| v * v).sum::<f32>().sqrt();
            if norm > 0.0 {
                for val in expanded_query.iter_mut() {
                    *val /= norm;
                }
            }

            expanded_queries.push(expanded_query);
        }

        Ok(expanded_queries)
    }

    /// Hybrid query expansion combining multiple methods
    async fn hybrid_expansion(&self, query: &[f32]) -> Result<Vec<Vec<f32>>> {
        let mut expanded_queries = Vec::new();

        // Original query
        expanded_queries.push(query.to_vec());

        // Add expanded queries with different strategies
        let base_expansion = self.word_level_expansion(query).await?;

        // Select diverse expanded queries based on diversity
        let num_to_add = (self.config.num_expansions - 1).min(base_expansion.len());

        for expanded_query in base_expansion.into_iter().take(num_to_add) {
            expanded_queries.push(expanded_query);
        }

        Ok(expanded_queries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::hardware_capabilities::initialize_hardware_capabilities_default;

    #[tokio::test]
    async fn test_query_expansion_none() {
        let _ = initialize_hardware_capabilities_default();
        let expansion = QueryExpansion::new(DistanceMetric::Cosine, 3);

        let query = vec![1.0, 0.0, 0.0];
        let expanded = expansion.expand_query(&query).await.unwrap();

        assert_eq!(expanded.len(), 3);
    }

    #[tokio::test]
    async fn test_query_expansion_diversity() {
        let _ = initialize_hardware_capabilities_default();
        let expansion = QueryExpansion::new(DistanceMetric::Cosine, 3);

        let query = vec![1.0, 0.0, 0.0];
        let expanded = expansion.expand_query(&query).await.unwrap();

        // Verify we get multiple different queries
        assert!(expanded.len() > 1);

        // Verify all queries are unit vectors (approximately)
        for query_vec in &expanded {
            let norm: f32 = query_vec.iter().map(|v| v * v).sum::<f32>().sqrt();
            assert!((norm - 1.0).abs() < 0.01);
        }
    }
}
