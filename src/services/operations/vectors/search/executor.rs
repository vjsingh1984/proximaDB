//! Search execution utilities for vector operations.
//!
//! Provides helper types and functions for executing vector searches
//! across different storage engines with progressive quantization support.

use std::collections::HashMap;

/// Search result with similarity scores.
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// Vector ID
    pub id: String,
    /// Vector data
    pub vector: Option<Vec<f32>>,
    /// Associated metadata
    pub metadata: HashMap<String, crate::proto::proximadb_v1::SqlValue>,
    /// Similarity score
    pub score: f32,
}

/// Convert proto search results to VectorRecord format.
pub fn proto_results_to_vector_records(
    search_results: Vec<crate::proto::proximadb_v1::SearchResult>,
) -> Vec<crate::proto::proximadb_v1::VectorRecord> {
    let mut vector_records = Vec::new();
    for search_result in search_results {
        for result in search_result.results {
            vector_records.push(crate::proto::proximadb_v1::VectorRecord {
                id: result.id,
                vector: result.vector,
                metadata: result.metadata,
                timestamp: Some(chrono::Utc::now().timestamp_millis()),
                updated_at: None,
                expires_at: None,
                version: None,
                source: Some("search_result".to_string()),
            });
        }
    }
    vector_records
}

/// Calculate candidate counts for progressive search stages.
///
/// Uses the formula: k_stage = k · Π(1/r_i) for all subsequent stages
/// where r_i is the recall target for stage i.
///
/// # Arguments
///
/// * `k` - Final top-k result count
/// * `recalls` - Recall targets for each progressive stage
///
/// # Returns
///
/// Vector of candidate counts for each stage
pub fn calculate_progressive_candidates(k: usize, recalls: &[f32]) -> Vec<usize> {
    let mut candidates = Vec::with_capacity(recalls.len());
    let mut recall_product = 1.0;

    for &recall in recalls {
        recall_product *= recall;
        let stage_k = ((k as f32) / recall_product).ceil() as usize;
        candidates.push(stage_k.max(k));
    }

    candidates
}

/// Build default progressive stages configuration.
pub fn default_progressive_stages() -> Vec<String> {
    vec!["binary".into(), "int8".into(), "pq".into(), "full".into()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_progressive_candidates() {
        let k = 10;
        let recalls = vec![0.5, 0.8]; // Binary→INT8→Full

        let candidates = calculate_progressive_candidates(k, &recalls);

        // First stage: 10 / 0.5 = 20
        assert_eq!(candidates[0], 20);
        // Second stage: 10 / (0.5 * 0.8) = 25
        assert_eq!(candidates[1], 25);
    }

    #[test]
    fn test_default_progressive_stages() {
        let stages = default_progressive_stages();
        assert_eq!(stages.len(), 4);
        assert_eq!(stages[0], "binary");
        assert_eq!(stages[3], "full");
    }

    #[test]
    fn test_proto_results_to_vector_records() {
        use crate::proto::proximadb_v1::{SearchResult as ProtoSearchResult, SearchVectorRecord};

        let proto_result = ProtoSearchResult {
            collection_id: Some("test".to_string()),
            total_found: 1,
            results: vec![SearchVectorRecord {
                id: "vec1".to_string(),
                score: 0.95,
                vector: vec![1.0, 2.0, 3.0],
                metadata: HashMap::new(),
                version: None,
                similarity: None,
                timestamp: None,
                source: None,
                expanded_context: Vec::new(),
                semantic_similarity: None,
                quantization_info: None,
                engine_stats: HashMap::new(),
                index_path: None,
            }],
        };

        let records = proto_results_to_vector_records(vec![proto_result]);

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, "vec1");
        assert_eq!(records[0].source, Some("search_result".to_string()));
    }
}
