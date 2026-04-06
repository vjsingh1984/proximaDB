//! Late Interaction Scoring for EDR
//!
//! This module implements late interaction scoring, which is the core of EDR.
//! Late interaction means scoring happens at query time between expanded queries
//! and multi-vector documents, rather than using pre-computed scores.

use anyhow::Result;
use std::sync::Arc;

use crate::compute::distance_computation::{DistanceMetric, UnifiedDistanceCompute};

/// Scoring result from late interaction
#[derive(Debug, Clone)]
pub struct ScoringResult {
    /// Document ID
    pub doc_id: String,
    /// Final score
    pub score: f32,
    /// Individual interaction scores for debugging
    pub interaction_scores: Vec<f32>,
}

/// Late interaction scorer for EDR
pub struct LateInteractionScorer {
    distance_compute: Arc<UnifiedDistanceCompute>,
    distance_metric: DistanceMetric,
}

impl LateInteractionScorer {
    /// Create a new late interaction scorer
    pub fn new(distance_metric: DistanceMetric) -> Self {
        let distance_compute = Arc::new(UnifiedDistanceCompute::new(distance_metric));

        Self {
            distance_compute,
            distance_metric,
        }
    }

    /// Compute late interaction score between expanded queries and document vectors
    pub async fn compute_late_interaction_score(
        &self,
        expanded_queries: &[Vec<f32>],
        document_vectors: &[Vec<f32>],
        top_k: usize,
    ) -> Result<f32> {
        let mut all_scores = Vec::new();

        // Compute scores for all query-document pairs
        for query_vec in expanded_queries {
            for doc_vec in document_vectors {
                let score = self.compute_single_pair_score(query_vec, doc_vec)?;
                all_scores.push(score);
            }
        }

        // Late interaction: aggregate scores (e.g., max pooling)
        // For simplicity, we'll use the maximum score
        let final_score = if all_scores.is_empty() {
            0.0
        } else {
            *all_scores
                .iter()
                .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                .unwrap_or(&0.0)
        };

        Ok(final_score)
    }

    /// Compute score for a single query-document pair
    fn compute_single_pair_score(&self, query: &[f32], document: &[f32]) -> Result<f32> {
        // Convert distance to similarity score
        let distance = self
            .distance_compute
            .calculate_distance(query, document, &self.distance_metric)
            .rank_value;

        // Convert distance to similarity (higher = more similar)
        let similarity = 1.0 / (1.0 + distance);

        Ok(similarity)
    }

    /// Compute detailed scoring result with all intermediate scores
    pub async fn compute_detailed_score(
        &self,
        expanded_queries: &[Vec<f32>],
        document_vectors: &[Vec<f32>],
        top_k: usize,
        doc_id: String,
    ) -> Result<ScoringResult> {
        let mut interaction_scores = Vec::new();

        // Compute scores for all query-document pairs
        for query_vec in expanded_queries {
            for doc_vec in document_vectors {
                let score = self.compute_single_pair_score(query_vec, doc_vec)?;
                interaction_scores.push(score);
            }
        }

        // Aggregate scores
        let final_score = if interaction_scores.is_empty() {
            0.0
        } else {
            *interaction_scores
                .iter()
                .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                .unwrap_or(&0.0)
        };

        Ok(ScoringResult {
            doc_id,
            score: final_score,
            interaction_scores,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::hardware_capabilities::initialize_hardware_capabilities_default;

    #[tokio::test]
    async fn test_late_interaction_scoring() {
        let _ = initialize_hardware_capabilities_default();
        let scorer = LateInteractionScorer::new(DistanceMetric::Cosine);

        let query = vec![1.0, 0.0, 0.0];
        let document = vec![0.9, 0.1, 0.0];

        let score = scorer
            .compute_single_pair_score(&query, &document)
            .unwrap();

        assert!(score > 0.8, "Similar vectors should have high score");
    }

    #[tokio::test]
    async fn test_detailed_scoring() {
        let _ = initialize_hardware_capabilities_default();
        let scorer = LateInteractionScorer::new(DistanceMetric::Cosine);

        let expanded_queries = vec![
            vec![1.0, 0.0, 0.0],
            vec![0.9, 0.1, 0.0],
        ];

        let document_vectors = vec![
            vec![0.8, 0.2, 0.0],
            vec![0.1, 0.9, 0.0],
        ];

        let result = scorer
            .compute_detailed_score(&expanded_queries, &document_vectors, 5, "doc1".to_string())
            .await
            .unwrap();

        assert_eq!(result.doc_id, "doc1");
        assert!(result.score > 0.0);
        assert!(!result.interaction_scores.is_empty());
    }
}
