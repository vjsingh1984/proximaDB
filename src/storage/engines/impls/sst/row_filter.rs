//! SST Row-Oriented Filtering Optimization
//!
//! Implements fast in-memory filtering on deserialized SstRecord/VectorRecord data.
//! Optimized for SST's row-based storage where entire blocks are read into memory.

use anyhow::Result;
use std::collections::HashMap;
use tracing::{debug, info};

use crate::core::VectorRecord; // OPTIMIZED: Direct VectorRecord usage
use crate::core::search::FilterExpression;
use crate::storage::engines::core::formats::fastlanes_blocks::FastLanesDataBlock;

/// Fast in-memory filter evaluator for row-oriented SST data
pub struct SSTRowFilterEvaluator {
    /// Cache for converted metadata to avoid repeated conversions
    metadata_cache: HashMap<String, HashMap<String, serde_json::Value>>,
}

impl SSTRowFilterEvaluator {
    pub fn new() -> Self {
        Self {
            metadata_cache: HashMap::new(),
        }
    }

    /// Fast filter evaluation on already-loaded VectorRecords
    ///
    /// This is optimal for SST because:
    /// 1. Data is already in memory (row-oriented blocks read in full)
    /// 2. No I/O savings from predicate pushdown
    /// 3. Fast metadata field access from VectorRecord
    /// 4. Can filter entire blocks in memory efficiently
    pub fn filter_records_fast(
        &mut self,
        records: &[VectorRecord],
        filter_expr: &FilterExpression,
    ) -> Result<Vec<usize>> {
        let mut qualifying_indices = Vec::new();

        info!(
            "SST Row Filter: Evaluating {} VectorRecords in mem",
            records.len()
        );

        for (index, record) in records.iter().enumerate() {
            // Use record ID or index as cache key
            let formatted_key = format!("idx_{}", index);
            let cache_key = record.id.as_str();
            let metadata_map = self.get_or_convert_vector_metadata(cache_key, &record.metadata)?;

            // Fast filter evaluation using centralized logic
            if self.evaluate_filter_fast(filter_expr, &metadata_map) {
                qualifying_indices.push(index);
            }
        }

        debug!(
            "SST Row Filter: {} out of {} VectorRecords qualify",
            qualifying_indices.len(),
            records.len()
        );

        Ok(qualifying_indices)
    }

    /// Filter VectorRecord array (common case for SST unified search)
    pub fn filter_vector_records_fast(
        &mut self,
        records: &[VectorRecord],
        filter_expr: &FilterExpression,
    ) -> Result<Vec<usize>> {
        let mut qualifying_indices = Vec::new();

        info!(
            "SST Row Filter: Evaluating {} VectorRecords in mem",
            records.len()
        );

        for (index, record) in records.iter().enumerate() {
            // Use record ID or index as cache key
            let formatted_key = format!("idx_{}", index);
            let cache_key = record.id.as_str();
            let metadata_map = self.get_or_convert_vector_metadata(cache_key, &record.metadata)?;

            if self.evaluate_filter_fast(filter_expr, &metadata_map) {
                qualifying_indices.push(index);
            }
        }

        debug!(
            "SST Row Filter: {} out of {} VectorRecords qualify",
            qualifying_indices.len(),
            records.len()
        );

        Ok(qualifying_indices)
    }

    /// Filter entire FastLanesDataBlock efficiently
    pub fn filter_data_block(
        &mut self,
        block: &FastLanesDataBlock,
        filter_expr: &FilterExpression,
    ) -> Result<Vec<usize>> {
        info!(
            "SST Row Filter: Filtering FastLanesDataBlock {} with {} records",
            block.block_id,
            block.records.len()
        );

        self.filter_records_fast(&block.records, filter_expr)
    }

    /// Parallel block filtering for high-performance workloads
    pub async fn filter_blocks_parallel(
        &mut self,
        blocks: &[FastLanesDataBlock],
        filter_expr: &FilterExpression,
    ) -> Result<HashMap<u32, Vec<usize>>> {
        let mut block_results = HashMap::new();

        info!("SST Row Filter: Parallel filtering {} blocks", blocks.len());

        // For now, sequential - can be parallelized with rayon
        for block in blocks {
            let indices = self.filter_data_block(block, filter_expr)?;
            if !indices.is_empty() {
                block_results.insert(block.block_id, indices);
            }
        }

        let total_qualifying = block_results.values().map(|v| v.len()).sum::<usize>();
        info!(
            "SST Row Filter: {} blocks have qualifying records, {} total matches",
            block_results.len(),
            total_qualifying
        );

        Ok(block_results)
    }

    /// Get or convert metadata with caching for performance
    fn get_or_convert_metadata(
        &mut self,
        record_id: &str,
        metadata: &[crate::proto::proximadb_v1::MetadataItem],
    ) -> Result<HashMap<String, serde_json::Value>> {
        if let Some(cached) = self.metadata_cache.get(record_id) {
            return Ok(cached.clone());
        }

        // Convert proto metadata to JSON for filtering
        let metadata_map = crate::core::proto_metadata_helper::proto_metadata_to_json(metadata);

        // Cache if record has stable ID
        if !record_id.is_empty() && !record_id.starts_with("idx_") {
            self.metadata_cache
                .insert(record_id.to_string(), metadata_map.clone());
        }

        Ok(metadata_map)
    }

    /// Convert VectorRecord metadata with caching
    fn get_or_convert_vector_metadata(
        &mut self,
        cache_key: &str,
        metadata: &std::collections::HashMap<String, crate::proto::proximadb_v1::SqlValue>,
    ) -> Result<HashMap<String, serde_json::Value>> {
        if let Some(cached) = self.metadata_cache.get(cache_key) {
            return Ok(cached.clone());
        }

        let metadata_map = crate::core::proto_metadata_helper::sqlvalue_metadata_to_json(metadata);

        // Cache for stable IDs only
        if !cache_key.starts_with("idx_") && cache_key.len() < 100 {
            self.metadata_cache
                .insert(cache_key.to_string(), metadata_map.clone());
        }

        Ok(metadata_map)
    }

    /// Fast filter evaluation using unified filter evaluator
    fn evaluate_filter_fast(
        &self,
        expr: &FilterExpression,
        metadata: &HashMap<String, serde_json::Value>,
    ) -> bool {
        // Use the unified filter evaluator for consistency across engines
        crate::storage::engines::core::evaluate_filter(expr, metadata)
    }

    /// Clear metadata cache to prevent memory bloat
    pub fn clear_cache(&mut self) {
        self.metadata_cache.clear();
    }

    /// Get cache statistics for monitoring
    pub fn cache_stats(&self) -> (usize, usize) {
        let entries = self.metadata_cache.len();
        let memory_bytes = self
            .metadata_cache
            .iter()
            .map(|(k, v)| k.len() + v.len() * 50) // Rough estimate
            .sum::<usize>();
        (entries, memory_bytes)
    }

    /// Immutable version for Arc-wrapped read operations (no caching)
    pub fn filter_vector_records_immutable(
        &self,
        records: &[VectorRecord],
        filter_expr: &FilterExpression,
    ) -> Result<Vec<usize>> {
        let mut qualifying_indices = Vec::new();

        debug!(
            "SST Row Filter (immutable): Evaluating {} VectorRecords",
            records.len()
        );

        for (index, record) in records.iter().enumerate() {
            // Convert metadata without caching for immutable operation
            let metadata_map =
                crate::core::proto_metadata_helper::sqlvalue_metadata_to_json(&record.metadata);

            if crate::storage::engines::core::evaluate_filter(filter_expr, &metadata_map) {
                qualifying_indices.push(index);
            }
        }

        debug!(
            "SST Row Filter (immutable): {} out of {} VectorRecords qualify",
            qualifying_indices.len(),
            records.len()
        );

        Ok(qualifying_indices)
    }
}

/// Optimized batch filtering for multiple filter expressions (AND/OR support)
pub struct SSTBatchFilterEvaluator {
    base_evaluator: SSTRowFilterEvaluator,
}

impl SSTBatchFilterEvaluator {
    pub fn new() -> Self {
        Self {
            base_evaluator: SSTRowFilterEvaluator::new(),
        }
    }

    /// Parallel evaluation of AND/OR expressions for high performance
    ///
    /// This implements the parallel thread strategy you mentioned:
    /// - Thread 1: category = 'electronics' → indices [1, 5, 8, 12]  
    /// - Thread 2: price > 100 → indices [2, 5, 7, 8, 10, 12]
    /// - Thread 3: brand = 'Apple' → indices [1, 3, 9]
    /// - AND_result = Thread1 ∩ Thread2 = [5, 8, 12]
    /// - Final_result = AND_result ∪ Thread3 = [1, 3, 5, 8, 9, 12]
    pub async fn evaluate_parallel_filters(
        &mut self,
        records: &[VectorRecord],
        filter_expr: &FilterExpression,
    ) -> Result<Vec<usize>> {
        match filter_expr {
            FilterExpression::And(sub_exprs) => {
                info!(
                    "SST Parallel Filter: Evaluating {} AND conditions",
                    sub_exprs.len()
                );

                // Evaluate each condition separately (can be parallelized)
                let mut condition_results = Vec::new();
                for expr in sub_exprs {
                    let indices = self
                        .base_evaluator
                        .filter_vector_records_fast(records, expr)?;
                    condition_results.push(indices);
                }

                // Intersection of all results
                let intersection = self.intersect_indices(condition_results);
                info!(
                    "SST Parallel Filter: AND result has {} qualifying indices",
                    intersection.len()
                );
                Ok(intersection)
            }

            FilterExpression::Or(sub_exprs) => {
                info!(
                    "SST Parallel Filter: Evaluating {} OR conditions",
                    sub_exprs.len()
                );

                // Evaluate each condition separately (can be parallelized)
                let mut condition_results = Vec::new();
                for expr in sub_exprs {
                    let indices = self
                        .base_evaluator
                        .filter_vector_records_fast(records, expr)?;
                    condition_results.push(indices);
                }

                // Union of all results
                let union = self.union_indices(condition_results);
                info!(
                    "SST Parallel Filter: OR result has {} qualifying indices",
                    union.len()
                );
                Ok(union)
            }

            FilterExpression::Not(sub_expr) => {
                info!("SST Parallel Filter: Evaluating NOT condition");
                let positive_indices = self
                    .base_evaluator
                    .filter_vector_records_fast(records, sub_expr)?;
                let all_indices: std::collections::HashSet<usize> = (0..records.len()).collect();
                let positive_set: std::collections::HashSet<usize> =
                    positive_indices.into_iter().collect();
                let negated: Vec<usize> = all_indices.difference(&positive_set).cloned().collect();
                Ok(negated)
            }

            _ => {
                // Single condition - use base evaluator
                self.base_evaluator
                    .filter_vector_records_fast(records, filter_expr)
            }
        }
    }

    /// Set intersection for AND operations
    fn intersect_indices(&self, mut index_sets: Vec<Vec<usize>>) -> Vec<usize> {
        if index_sets.is_empty() {
            return Vec::new();
        }

        if index_sets.len() == 1 {
            return index_sets.into_iter().next().unwrap();
        }

        // Sort by size (smallest first for efficiency)
        index_sets.sort_by_key(|set| set.len());

        let mut result: std::collections::HashSet<usize> = index_sets[0].iter().cloned().collect();

        for set in index_sets.iter().skip(1) {
            let set_hash: std::collections::HashSet<usize> = set.iter().cloned().collect();
            result = result.intersection(&set_hash).cloned().collect();

            // Early termination if intersection becomes empty
            if result.is_empty() {
                break;
            }
        }

        let mut final_result: Vec<usize> = result.into_iter().collect();
        final_result.sort_unstable();
        final_result
    }

    /// Set union for OR operations
    fn union_indices(&self, index_sets: Vec<Vec<usize>>) -> Vec<usize> {
        let mut result: std::collections::HashSet<usize> = std::collections::HashSet::new();

        for set in index_sets {
            result.extend(set);
        }

        let mut final_result: Vec<usize> = result.into_iter().collect();
        final_result.sort_unstable();
        final_result
    }

    /// Immutable version for Arc-wrapped read operations
    pub async fn evaluate_parallel_filters_immutable(
        &self,
        records: &[VectorRecord],
        filter_expr: &FilterExpression,
    ) -> Result<Vec<usize>> {
        match filter_expr {
            FilterExpression::And(sub_exprs) => {
                debug!(
                    "SST Parallel Filter (immutable): Evaluating {} AND conditions",
                    sub_exprs.len()
                );

                let mut condition_results = Vec::new();
                for expr in sub_exprs {
                    let indices = self
                        .base_evaluator
                        .filter_vector_records_immutable(records, expr)?;
                    condition_results.push(indices);
                }

                let intersection = self.intersect_indices(condition_results);
                Ok(intersection)
            }

            FilterExpression::Or(sub_exprs) => {
                debug!(
                    "SST Parallel Filter (immutable): Evaluating {} OR conditions",
                    sub_exprs.len()
                );

                let mut condition_results = Vec::new();
                for expr in sub_exprs {
                    let indices = self
                        .base_evaluator
                        .filter_vector_records_immutable(records, expr)?;
                    condition_results.push(indices);
                }

                let union = self.union_indices(condition_results);
                Ok(union)
            }

            _ => {
                // Single condition - use base evaluator
                self.base_evaluator
                    .filter_vector_records_immutable(records, filter_expr)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::search::{ComparisonOperator, FilterExpression};

    #[tokio::test]
    async fn test_sst_row_filter_performance() {
        let mut evaluator = SSTRowFilterEvaluator::new();

        // Create test records with metadata
        let records = create_test_vector_records(1000);

        // Simple equality filter
        let filter = FilterExpression::Comparison {
            field: "category".to_string(),
            operator: ComparisonOperator::Equals,
            value: serde_json::json!("electronics"),
        };

        let indices = evaluator
            .filter_vector_records_fast(&records, &filter)
            .unwrap();

        assert!(!indices.is_empty(), "Should find some matching records");
        debug!(
            "SST Row Filter found {} matches out of {} records",
            indices.len(),
            records.len()
        );
    }

    #[tokio::test]
    async fn test_parallel_and_or_evaluation() {
        let mut evaluator = SSTBatchFilterEvaluator::new();
        let records = create_test_vector_records(1000);

        // Complex AND/OR filter
        let filter = FilterExpression::Or(vec![
            FilterExpression::And(vec![
                FilterExpression::Comparison {
                    field: "category".to_string(),
                    operator: ComparisonOperator::Equals,
                    value: serde_json::json!("electronics"),
                },
                FilterExpression::Comparison {
                    field: "price".to_string(),
                    operator: ComparisonOperator::GreaterThan,
                    value: serde_json::json!(100),
                },
            ]),
            FilterExpression::Comparison {
                field: "brand".to_string(),
                operator: ComparisonOperator::Equals,
                value: serde_json::json!("Apple"),
            },
        ]);

        let indices = evaluator
            .evaluate_parallel_filters(&records, &filter)
            .await
            .unwrap();

        assert!(!indices.is_empty(), "Should find some matching records");
        debug!("Parallel filter found {} matches", indices.len());
    }

    fn create_test_vector_records(count: usize) -> Vec<VectorRecord> {
        let mut records = Vec::new();

        for i in 0..count {
            let mut metadata = Vec::new();

            // Add test metadata
            metadata.push(crate::proto::proximadb_v1::MetadataItem {
                key: "category".to_string(),
                value: Some(
                    crate::proto::proximadb_v1::metadata_item::Value::StringValue(if i % 3 == 0 {
                        "electronics".to_string()
                    } else {
                        "books".to_string()
                    }),
                ),
            });

            metadata.push(crate::proto::proximadb_v1::MetadataItem {
                key: "price".to_string(),
                value: Some(
                    crate::proto::proximadb_v1::metadata_item::Value::NumberValue(
                        (50 + (i * 10) % 200) as f64,
                    ),
                ),
            });

            metadata.push(crate::proto::proximadb_v1::MetadataItem {
                key: "brand".to_string(),
                value: Some(
                    crate::proto::proximadb_v1::metadata_item::Value::StringValue(if i % 7 == 0 {
                        "Apple".to_string()
                    } else {
                        "Samsung".to_string()
                    }),
                ),
            });

            let mut map_metadata = std::collections::HashMap::new();
            for item in metadata {
                if let Some(key) = item.key {
                    map_metadata.insert(key, crate::proto::proximadb_v1::SqlValue {
                        value: Some(item.value.unwrap())
                    });
                }
            }

            records.push(VectorRecord {
                id: format!("vec_{}", i),
                vector: vec![0.1; 128], // Dummy vector
                metadata: map_metadata,
                timestamp: 1000000 + i as i64,
                updated_at: None,
                expires_at: None,
                version: Some(1),
                quantized_vector: vec![],
                source: None,
            });
        }

        records
    }
}
