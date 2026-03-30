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

//! Search Operations
//!
//! Low-level search operations and utilities for the SST engine.
//! Provides atomic search operations and file-level search coordination.

use anyhow::{Context, Result};
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::compute::distance_computation::DistanceMetric;
use crate::core::search::FilterExpression;
use crate::core::search::bounded_queue::BoundedPriorityQueue;
use crate::core::search::results::OptimizedSearchRecord;
use crate::storage::engines::impls::sst::SstEngine;

/// Low-level search operations
pub struct SearchOperations {
    engine: Arc<SstEngine>,
}

impl SearchOperations {
    /// Create new search operations handler
    pub fn new(engine: Arc<SstEngine>) -> Self {
        Self { engine }
    }

    /// Search a single SSTable file
    pub async fn search_sstable_file(
        &self,
        file_path: &str,
        query_vector: &[f32],
        k: usize,
        distance_metric: DistanceMetric,
        filter_expression: Option<&FilterExpression>,
    ) -> Result<Vec<OptimizedSearchRecord>> {
        debug!("🔍 SearchOps: Searching SSTable file: {}", file_path);

        let results = self
            .engine
            .sstable_reader()
            .search_with_filter(
                file_path,
                query_vector,
                filter_expression.cloned(),
                k,
                distance_metric,
                None, // No collection config available at this level
            )
            .await
            .context("Failed to search SSTable file")?;

        debug!(
            "📊 SearchOps: Found {} results in {}",
            results.len(),
            file_path
        );
        Ok(results)
    }

    /// Search multiple SSTable files in parallel
    pub async fn search_sstable_files_parallel(
        &self,
        file_paths: &[String],
        query_vector: &[f32],
        k: usize,
        distance_metric: DistanceMetric,
        filter_expression: Option<&FilterExpression>,
    ) -> Result<Vec<OptimizedSearchRecord>> {
        debug!(
            "🔍 SearchOps: Searching {} SSTable files in parallel",
            file_paths.len()
        );

        // Use bounded priority queue to maintain only top-k results
        let mut priority_queue = BoundedPriorityQueue::new(k);

        let search_futures: Vec<_> = file_paths
            .iter()
            .map(|path| {
                self.search_sstable_file(
                    path,
                    query_vector,
                    k * 2, // Get more candidates per file for better quality
                    distance_metric,
                    filter_expression,
                )
            })
            .collect();

        // Execute searches in parallel
        let results = futures::future::join_all(search_futures).await;

        // Collect successful results into bounded queue
        let mut total_candidates = 0;
        for (i, result) in results.into_iter().enumerate() {
            match result {
                Ok(file_results) => {
                    total_candidates += file_results.len();
                    for record in file_results {
                        priority_queue.try_insert(record);
                    }
                }
                Err(e) => {
                    warn!(
                        "⚠️ SearchOps: Failed to search file {}: {}",
                        file_paths[i], e
                    );
                    // Continue with other files
                }
            }
        }

        // Extract results from bounded queue
        let results = priority_queue.into_sorted_vec();

        info!(
            "📊 SearchOps: Parallel search completed, {} results from {} candidates",
            results.len(),
            total_candidates
        );
        Ok(results)
    }

    /// Perform point lookup for a specific vector ID
    pub async fn point_lookup(
        &self,
        file_paths: &[String],
        vector_id: &str,
    ) -> Result<Option<OptimizedSearchRecord>> {
        debug!("🎯 SearchOps: Point lookup for vector ID: {}", vector_id);

        for file_path in file_paths {
            // Check if vector exists in this file using bloom filter
            if let Ok(exists) = self.check_vector_exists_in_file(file_path, vector_id).await
                && exists {
                    // Perform detailed lookup in this file
                    if let Ok(result) = self.lookup_vector_in_file(file_path, vector_id).await {
                        debug!("✅ SearchOps: Found vector {} in {}", vector_id, file_path);
                        return Ok(Some(result));
                    }
                }
        }

        debug!("❌ SearchOps: Vector {} not found in any file", vector_id);
        Ok(None)
    }

    /// Check if a vector exists in a specific file using bloom filter
    async fn check_vector_exists_in_file(&self, file_path: &str, vector_id: &str) -> Result<bool> {
        // This would use the bloom filter to quickly check existence
        // For now, we'll implement a simple check
        debug!(
            "🔍 Checking if vector {} exists in {}",
            vector_id, file_path
        );

        // In a real implementation, this would:
        // 1. Read the bloom filter from the SSTable footer
        // 2. Check if the vector ID is in the bloom filter
        // 3. Return false for definite non-existence, true for possible existence

        Ok(true) // Conservative approach - assume it might exist
    }

    /// Lookup a specific vector in a file using B+ tree optimized index
    ///
    /// OPTIMIZED: Uses the SST reader's B+ tree index for O(log n) block lookup
    /// instead of full scan. Critical for RAG use cases.
    async fn lookup_vector_in_file(
        &self,
        file_path: &str,
        vector_id: &str,
    ) -> Result<OptimizedSearchRecord> {
        debug!("🔍 Looking up vector {} in {}", vector_id, file_path);

        // Use the optimized SST reader with B+ tree index
        let reader = self.engine.sstable_reader();

        match reader.vector(file_path, vector_id).await {
            Ok(Some(record)) => {
                debug!("✅ Found vector {} in {}", vector_id, file_path);

                // Convert VectorRecord to OptimizedSearchRecord
                let mut result = OptimizedSearchRecord::default();
                result.id = record.id;
                result.score = 1.0; // Perfect match for point lookup
                result.vector = Some(Arc::new(record.vector));
                result.timestamp = record.timestamp;
                result.updated_at = record.updated_at;
                result.expires_at = record.expires_at;
                result.version = record.version;

                // VectorRecord.metadata is already HashMap<String, SqlValue>, just clone
                result.metadata = record.metadata;

                Ok(result)
            }
            Ok(None) => {
                debug!("❌ Vector {} not found in {}", vector_id, file_path);
                Err(anyhow::anyhow!(
                    "Vector {} not found in {}",
                    vector_id,
                    file_path
                ))
            }
            Err(e) => {
                warn!(
                    "⚠️ Error looking up vector {} in {}: {}",
                    vector_id, file_path, e
                );
                Err(e)
            }
        }
    }

    /// Perform range query within a score threshold
    pub async fn range_query(
        &self,
        file_paths: &[String],
        query_vector: &[f32],
        max_distance: f32,
        distance_metric: DistanceMetric,
        filter_expression: Option<&FilterExpression>,
    ) -> Result<Vec<OptimizedSearchRecord>> {
        debug!(
            "🎯 SearchOps: Range query with max distance: {}",
            max_distance
        );

        let mut all_results = Vec::new();

        for file_path in file_paths {
            match self
                .search_sstable_file(
                    file_path,
                    query_vector,
                    10000, // Large k to get all candidates
                    distance_metric,
                    filter_expression,
                )
                .await
            {
                Ok(results) => {
                    // Filter results by distance threshold
                    let filtered: Vec<_> = results
                        .into_iter()
                        .filter(|r| r.score <= max_distance)
                        .collect();

                    debug!(
                        "📊 Found {} results within range in {}",
                        filtered.len(),
                        file_path
                    );
                    all_results.extend(filtered);
                }
                Err(e) => {
                    warn!("⚠️ SearchOps: Failed range query on {}: {}", file_path, e);
                }
            }
        }

        // Sort by score
        all_results.sort_by(|a, b| {
            a.score
                .partial_cmp(&b.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        info!(
            "📊 SearchOps: Range query completed, {} results within distance {}",
            all_results.len(),
            max_distance
        );
        Ok(all_results)
    }

    /// Get search operation statistics
    pub async fn get_operation_stats(&self) -> Result<SearchOperationStats> {
        Ok(SearchOperationStats {
            total_file_searches: 0, // Would be tracked in real implementation
            avg_file_search_time_ms: 0.0,
            bloom_filter_hits: 0,
            bloom_filter_misses: 0,
            cache_hits: 0,
            cache_misses: 0,
        })
    }

    /// Validate search parameters
    pub fn validate_search_params(
        &self,
        query_vector: &[f32],
        k: usize,
        distance_metric: DistanceMetric,
    ) -> Result<()> {
        if query_vector.is_empty() {
            return Err(anyhow::anyhow!("Query vector cannot be empty"));
        }

        if k == 0 {
            return Err(anyhow::anyhow!("k must be greater than 0"));
        }

        if k > 10000 {
            warn!(
                "⚠️ SearchOps: Large k value ({}), performance may be impacted",
                k
            );
        }

        // Validate distance metric compatibility
        match distance_metric {
            DistanceMetric::Cosine | DistanceMetric::Euclidean | DistanceMetric::Manhattan => {
                // These are supported
            }
            _ => {
                return Err(anyhow::anyhow!(
                    "Unsupported distance metric: {:?}",
                    distance_metric
                ));
            }
        }

        debug!("✅ SearchOps: Search parameters validated");
        Ok(())
    }
}

/// Statistics for search operations
#[derive(Debug, Clone)]
pub struct SearchOperationStats {
    pub total_file_searches: u64,
    pub avg_file_search_time_ms: f64,
    pub bloom_filter_hits: u64,
    pub bloom_filter_misses: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
    use crate::storage::engines::impls::sst::SstConfig;
    use crate::storage::persistence::filesystem::FilesystemFactory;

    #[tokio::test]
    async fn test_validate_search_params() {
        let engine = create_test_engine().await;
        let ops = SearchOperations::new(Arc::new(engine));

        // Test valid parameters
        let query_vector = vec![1.0, 2.0, 3.0];
        assert!(
            ops.validate_search_params(&query_vector, 10, DistanceMetric::Cosine)
                .is_ok()
        );

        // Test empty vector
        let empty_vector = vec![];
        assert!(
            ops.validate_search_params(&empty_vector, 10, DistanceMetric::Cosine)
                .is_err()
        );

        // Test zero k
        assert!(
            ops.validate_search_params(&query_vector, 0, DistanceMetric::Cosine)
                .is_err()
        );
    }

    #[tokio::test]
    async fn test_operation_stats() {
        let engine = create_test_engine().await;
        let ops = SearchOperations::new(Arc::new(engine));

        let stats = ops.get_operation_stats().await.unwrap();
        assert_eq!(stats.total_file_searches, 0);
    }

    async fn create_test_engine() -> SstEngine {
        let config = SstConfig::default();
        let filesystem_config =
            crate::storage::persistence::filesystem::FilesystemConfig::default();
        let filesystem = Arc::new(FilesystemFactory::create(filesystem_config).await.unwrap());
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());

        SstEngine::new_with_config(config, filesystem, distance_compute)
            .await
            .unwrap()
    }
}
