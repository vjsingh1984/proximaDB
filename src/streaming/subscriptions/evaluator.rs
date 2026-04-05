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

//! Query Evaluator for Live Queries
//!
//! This module provides incremental evaluation of vectors against
//! subscription queries, detecting score changes and maintaining result sets.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use super::subscription::ScoredResult;

/// Query evaluation result
#[derive(Debug, Clone)]
pub struct EvaluationResult {
    /// Candidate results that passed the threshold
    pub candidates: Vec<ScoredResult>,
    /// Number of vectors evaluated
    pub vectors_evaluated: usize,
    /// Evaluation time in microseconds
    pub evaluation_time_us: u64,
}

/// Score change detection
#[derive(Debug, Clone)]
pub struct ScoreChange {
    /// Vector ID
    pub vector_id: String,
    /// Old score (None if new)
    pub old_score: Option<f32>,
    /// New score
    pub new_score: f32,
    /// Old position (None if new)
    pub old_position: Option<u32>,
    /// New position
    pub new_position: u32,
}

/// Query evaluator for live queries
pub struct QueryEvaluator {
    /// SIMD availability flag (for optimization)
    #[allow(dead_code)]
    simd_available: bool,
}

impl QueryEvaluator {
    /// Create a new query evaluator
    pub fn new() -> Self {
        Self {
            simd_available: Self::detect_simd(),
        }
    }

    /// Detect SIMD capabilities
    fn detect_simd() -> bool {
        #[cfg(target_arch = "x86_64")]
        {
            std::arch::is_x86_feature_detected!("avx2")
        }
        #[cfg(target_arch = "aarch64")]
        {
            true // NEON is always available on aarch64
        }
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        {
            false
        }
    }

    /// Evaluate vectors against a query
    pub fn evaluate_vectors(
        &self,
        query: &[f32],
        vectors: &[(String, Vec<f32>, f32)], // (id, vector, score_hint)
        top_k: usize,
        score_threshold: f32,
    ) -> Vec<ScoredResult> {
        let _start = std::time::Instant::now(); // Deferred: Add timing metrics

        // Use a max-heap with negated scores (to get min behavior for top-k)
        let mut heap: BinaryHeap<HeapEntry> = BinaryHeap::with_capacity(top_k + 1);

        for (id, vector, _hint) in vectors {
            // Compute similarity
            let score = self.compute_similarity(query, vector);

            if score >= score_threshold {
                heap.push(HeapEntry {
                    vector_id: id.clone(),
                    score,
                });

                // Keep only top-k
                if heap.len() > top_k {
                    heap.pop();
                }
            }
        }

        // Convert heap to sorted results
        let mut results: Vec<_> = heap.into_vec();
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal));

        results
            .into_iter()
            .enumerate()
            .map(|(i, e)| ScoredResult {
                vector_id: e.vector_id,
                score: e.score,
                position: i as u32,
            })
            .collect()
    }

    /// Compute cosine similarity between two vectors
    fn compute_similarity(&self, a: &[f32], b: &[f32]) -> f32 {
        if a.len() != b.len() {
            return 0.0;
        }

        // Use SIMD-optimized dot product when available
        let dot = self.dot_product(a, b);
        let norm_a = self.dot_product(a, a).sqrt();
        let norm_b = self.dot_product(b, b).sqrt();

        if norm_a == 0.0 || norm_b == 0.0 {
            return 0.0;
        }

        dot / (norm_a * norm_b)
    }

    /// Compute dot product (with SIMD optimization)
    fn dot_product(&self, a: &[f32], b: &[f32]) -> f32 {
        // Simple scalar implementation for now
        // In production, this would use the compute::distance module
        a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
    }

    /// Batch evaluate multiple queries against vectors
    pub fn batch_evaluate(
        &self,
        queries: &[(&[f32], usize, f32)], // (query, top_k, threshold)
        vectors: &[(String, Vec<f32>, f32)],
    ) -> Vec<Vec<ScoredResult>> {
        queries
            .iter()
            .map(|(query, top_k, threshold)| {
                self.evaluate_vectors(query, vectors, *top_k, *threshold)
            })
            .collect()
    }

    /// Check if a vector would affect any subscription
    pub fn would_affect_subscriptions(
        &self,
        vector: &[f32],
        subscriptions: &[(&[f32], f32, f32)], // (query, threshold, current_min_score)
    ) -> Vec<bool> {
        subscriptions
            .iter()
            .map(|(query, threshold, current_min)| {
                let score = self.compute_similarity(query, vector);
                score >= *threshold && score > *current_min
            })
            .collect()
    }

    /// Incremental update: evaluate new vectors against existing result set
    pub fn incremental_evaluate(
        &self,
        query: &[f32],
        current_results: &[ScoredResult],
        new_vectors: &[(String, Vec<f32>, f32)],
        top_k: usize,
        score_threshold: f32,
    ) -> (Vec<ScoredResult>, Vec<ScoreChange>) {
        // Get the minimum score in current results
        let current_min = current_results
            .last()
            .map_or(score_threshold, |r| r.score);

        // Evaluate new vectors
        let mut candidates = Vec::new();
        for (id, vector, _hint) in new_vectors {
            let score = self.compute_similarity(query, vector);
            if score >= score_threshold && (current_results.len() < top_k || score > current_min) {
                candidates.push(ScoredResult {
                    vector_id: id.clone(),
                    score,
                    position: 0, // Will be set later
                });
            }
        }

        if candidates.is_empty() {
            return (current_results.to_vec(), Vec::new());
        }

        // Merge with current results
        let mut merged: Vec<_> = current_results.to_vec();
        merged.extend(candidates);
        merged.sort();
        merged.truncate(top_k);

        // Update positions and detect changes
        let mut changes = Vec::new();
        for (new_pos, result) in merged.iter_mut().enumerate() {
            let old_entry = current_results
                .iter()
                .find(|r| r.vector_id == result.vector_id);

            match old_entry {
                Some(old) => {
                    if old.position != new_pos as u32 || (old.score - result.score).abs() > 0.0001 {
                        changes.push(ScoreChange {
                            vector_id: result.vector_id.clone(),
                            old_score: Some(old.score),
                            new_score: result.score,
                            old_position: Some(old.position),
                            new_position: new_pos as u32,
                        });
                    }
                }
                None => {
                    changes.push(ScoreChange {
                        vector_id: result.vector_id.clone(),
                        old_score: None,
                        new_score: result.score,
                        old_position: None,
                        new_position: new_pos as u32,
                    });
                }
            }

            result.position = new_pos as u32;
        }

        // Check for removed entries
        for old in current_results {
            if !merged.iter().any(|m| m.vector_id == old.vector_id) {
                changes.push(ScoreChange {
                    vector_id: old.vector_id.clone(),
                    old_score: Some(old.score),
                    new_score: 0.0,
                    old_position: Some(old.position),
                    new_position: u32::MAX, // Indicates removal
                });
            }
        }

        (merged, changes)
    }
}

impl Default for QueryEvaluator {
    fn default() -> Self {
        Self::new()
    }
}

/// Heap entry for top-k selection
#[derive(Debug, Clone)]
struct HeapEntry {
    vector_id: String,
    score: f32,
}

impl PartialEq for HeapEntry {
    fn eq(&self, other: &Self) -> bool {
        self.score == other.score
    }
}

impl Eq for HeapEntry {}

impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse ordering for min-heap behavior (keep highest scores)
        other
            .score
            .partial_cmp(&self.score)
            .unwrap_or(Ordering::Equal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_vectors() -> Vec<(String, Vec<f32>, f32)> {
        vec![
            ("v1".to_string(), vec![1.0, 0.0, 0.0], 0.0),
            ("v2".to_string(), vec![0.9, 0.1, 0.0], 0.0),
            ("v3".to_string(), vec![0.0, 1.0, 0.0], 0.0),
            ("v4".to_string(), vec![0.0, 0.0, 1.0], 0.0),
            ("v5".to_string(), vec![0.5, 0.5, 0.0], 0.0),
        ]
    }

    #[test]
    fn test_evaluate_vectors_basic() {
        let evaluator = QueryEvaluator::new();
        let query = vec![1.0, 0.0, 0.0];
        let vectors = create_test_vectors();

        let results = evaluator.evaluate_vectors(&query, &vectors, 3, 0.0);

        assert_eq!(results.len(), 3);
        // First result should be the most similar (v1)
        assert_eq!(results[0].vector_id, "v1");
        assert!(results[0].score > 0.99);
    }

    #[test]
    fn test_evaluate_vectors_threshold() {
        let evaluator = QueryEvaluator::new();
        let query = vec![1.0, 0.0, 0.0];
        let vectors = create_test_vectors();

        let results = evaluator.evaluate_vectors(&query, &vectors, 10, 0.8);

        // Only v1 and v2 should pass 0.8 threshold
        assert!(results.len() <= 2);
        for result in &results {
            assert!(result.score >= 0.8);
        }
    }

    #[test]
    fn test_compute_similarity() {
        let evaluator = QueryEvaluator::new();

        // Identical vectors
        let a = vec![1.0, 0.0, 0.0];
        let similarity = evaluator.compute_similarity(&a, &a);
        assert!((similarity - 1.0).abs() < 0.0001);

        // Orthogonal vectors
        let b = vec![0.0, 1.0, 0.0];
        let similarity = evaluator.compute_similarity(&a, &b);
        assert!(similarity.abs() < 0.0001);
    }

    #[test]
    fn test_incremental_evaluate() {
        let evaluator = QueryEvaluator::new();
        let query = vec![1.0, 0.0, 0.0];

        // Initial results
        let current = vec![
            ScoredResult {
                vector_id: "v1".to_string(),
                score: 0.9,
                position: 0,
            },
            ScoredResult {
                vector_id: "v2".to_string(),
                score: 0.8,
                position: 1,
            },
        ];

        // New vector that should enter top-k
        let new_vectors = vec![("v3".to_string(), vec![0.95, 0.05, 0.0], 0.0)];

        let (results, changes) =
            evaluator.incremental_evaluate(&query, &current, &new_vectors, 2, 0.0);

        assert_eq!(results.len(), 2);
        // v3 should be in results with high score
        assert!(results.iter().any(|r| r.vector_id == "v3"));
        // There should be changes
        assert!(!changes.is_empty());
    }

    #[test]
    fn test_batch_evaluate() {
        let evaluator = QueryEvaluator::new();
        let vectors = create_test_vectors();

        let queries: Vec<(&[f32], usize, f32)> =
            vec![(&[1.0, 0.0, 0.0], 2, 0.0), (&[0.0, 1.0, 0.0], 2, 0.0)];

        let results = evaluator.batch_evaluate(&queries, &vectors);

        assert_eq!(results.len(), 2);
        // First query should find v1 as top result
        assert_eq!(results[0][0].vector_id, "v1");
        // Second query should find v3 as top result
        assert_eq!(results[1][0].vector_id, "v3");
    }

    #[test]
    fn test_would_affect_subscriptions() {
        let evaluator = QueryEvaluator::new();

        let subscriptions: Vec<(&[f32], f32, f32)> = vec![
            (&[1.0, 0.0, 0.0][..], 0.5, 0.8), // High threshold, moderate min
            (&[0.0, 1.0, 0.0][..], 0.5, 0.9), // Different direction
        ];

        let vector = vec![0.9, 0.1, 0.0]; // Similar to first query

        let affected = evaluator.would_affect_subscriptions(&vector, &subscriptions);

        assert!(affected[0]); // Should affect first subscription
        assert!(!affected[1]); // Should not affect second (wrong direction)
    }

    #[test]
    fn test_positions_update_correctly() {
        let evaluator = QueryEvaluator::new();
        let query = vec![1.0, 0.0, 0.0];
        let vectors = create_test_vectors();

        let results = evaluator.evaluate_vectors(&query, &vectors, 5, 0.0);

        // Check positions are sequential
        for (i, result) in results.iter().enumerate() {
            assert_eq!(result.position, i as u32);
        }
    }

    #[test]
    fn test_empty_vectors() {
        let evaluator = QueryEvaluator::new();
        let query = vec![1.0, 0.0, 0.0];
        let vectors: Vec<(String, Vec<f32>, f32)> = vec![];

        let results = evaluator.evaluate_vectors(&query, &vectors, 10, 0.0);
        assert!(results.is_empty());
    }

    #[test]
    fn test_score_change_detection() {
        let evaluator = QueryEvaluator::new();
        let query = vec![1.0, 0.0, 0.0];

        let current = vec![ScoredResult {
            vector_id: "v1".to_string(),
            score: 0.9,
            position: 0,
        }];

        // New vector with higher score
        let new_vectors = vec![("v2".to_string(), vec![1.0, 0.0, 0.0], 0.0)];

        let (_results, changes) =
            evaluator.incremental_evaluate(&query, &current, &new_vectors, 2, 0.0);

        // Should detect v2 addition and v1 position change
        assert!(
            changes
                .iter()
                .any(|c| c.vector_id == "v2" && c.old_score.is_none())
        );
    }
}
