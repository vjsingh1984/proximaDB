//! Three-Stage SST Filtering Pipeline with Predicate Pushdown
//!
//! Integrated with UnifiedSstableReader's ReadStrategy pattern for:
//! - WAL search: Skip stages, direct memory access
//! - Flush: Moderate filtering, preserve write throughput  
//! - Compaction: Skip all filtering for maximum throughput
//! - Search: Full three-stage filtering with predicate pushdown
//!
//! Stage 1: Bloom Filter Pre-filtering (Skip entire files)
//! Stage 2: Index Entry Block Filtering (Skip data blocks)  
//! Stage 3: Data Block Row Filtering (Fast in-memory metadata filtering)

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::proto::proximadb_v1::VectorRecord;
use crate::core::bloom::SstableBloomFilter;
use crate::core::search::{ComparisonOperator, FilterExpression};
use crate::storage::engines::core::formats::proximablocks::ProximaDataBlock;
use crate::storage::engines::impls::sst::IndexEntry;
use crate::storage::engines::impls::sst::readers::sst_query_engine::ReadStrategy;
use crate::storage::engines::impls::sst::row_filter::{
    SSTBatchFilterEvaluator, SSTRowFilterEvaluator,
};

/// Complete SST filtering pipeline with all three stages
/// Uses immutable Arc types for optimal read performance
/// Integrates with UnifiedSstableReader's ReadStrategy for operation-aware filtering
pub struct ThreeStageFilterPipeline {
    /// Stage 3: Row-level filtering (immutable for read operations)
    row_filter: Arc<SSTRowFilterEvaluator>,
    /// Stage 3: Batch filtering for AND/OR operations (immutable for read operations)
    batch_filter: Arc<SSTBatchFilterEvaluator>,
    /// Read strategy for operation-aware filtering behavior
    read_strategy: Option<ReadStrategy>,
}

impl ThreeStageFilterPipeline {
    pub fn new() -> Self {
        Self {
            row_filter: Arc::new(SSTRowFilterEvaluator::new()),
            batch_filter: Arc::new(SSTBatchFilterEvaluator::new()),
            read_strategy: None,
        }
    }

    /// Create pipeline with specific read strategy for operation-aware filtering
    pub fn with_strategy(strategy: ReadStrategy) -> Self {
        Self {
            row_filter: Arc::new(SSTRowFilterEvaluator::new()),
            batch_filter: Arc::new(SSTBatchFilterEvaluator::new()),
            read_strategy: Some(strategy),
        }
    }

    /// Check if filtering should be skipped based on strategy
    fn should_skip_filtering(&self) -> bool {
        matches!(self.read_strategy, Some(ReadStrategy::CompactionDirect))
    }

    /// Complete 3-stage filtering pipeline
    /// Returns qualifying indices from records that passed all stages
    pub async fn filter_with_three_stages(
        &mut self,
        file_path: &str,
        filter_expr: &FilterExpression,
        bloom_filter: Option<&SstableBloomFilter>,
        index_entries: &[IndexEntry],
        data_blocks: &[ProximaDataBlock],
    ) -> Result<FilterResult> {
        let mut stats = FilterStageStats::new();

        info!(
            "🔍 SST 3-Stage Filter: Processing file {} with {} blocks",
            file_path,
            data_blocks.len()
        );

        // STAGE 1: Bloom Filter Pre-filtering
        if !self.stage1_bloom_filter_check(filter_expr, bloom_filter, &mut stats)? {
            info!(
                "🌸 Stage 1: Bloom filter rejected file {} - no potential matches",
                file_path
            );
            return Ok(FilterResult::empty_with_stats(stats));
        }

        // STAGE 2: Index Entry Block Filtering
        let qualifying_blocks = self
            .stage2_index_entry_filtering(filter_expr, index_entries, data_blocks, &mut stats)
            .await?;

        if qualifying_blocks.is_empty() {
            info!(
                "📊 Stage 2: Index entries rejected all blocks in {}",
                file_path
            );
            return Ok(FilterResult::empty_with_stats(stats));
        }

        // STAGE 3: Data Block Row Filtering
        let qualifying_indices = self
            .stage3_row_filtering(filter_expr, &qualifying_blocks, &mut stats)
            .await?;

        info!(
            "✅ SST 3-Stage Filter: {} total qualifying indices from {}",
            qualifying_indices.len(),
            file_path
        );

        Ok(FilterResult {
            qualifying_indices,
            stats,
            blocks_processed: qualifying_blocks.len(),
            blocks_skipped: data_blocks.len() - qualifying_blocks.len(),
        })
    }

    /// Stage 1: Bloom Filter Pre-filtering
    /// Returns false if we can definitively skip this entire file
    fn stage1_bloom_filter_check(
        &self,
        filter_expr: &FilterExpression,
        bloom_filter: Option<&SstableBloomFilter>,
        stats: &mut FilterStageStats,
    ) -> Result<bool> {
        let stage_start = std::time::Instant::now();

        // Skip bloom filtering for compaction
        if self.should_skip_filtering() {
            stats.stage1_duration_us = 0;
            return Ok(true);
        }

        let Some(bloom_filter) = bloom_filter else {
            debug!("🌸 Stage 1: No bloom filter - assuming potential matches");
            stats.stage1_duration_us = stage_start.elapsed().as_micros() as u64;
            return Ok(true);
        };

        let might_match = match filter_expr {
            FilterExpression::Comparison {
                field,
                operator,
                value,
            } => {
                match operator {
                    ComparisonOperator::Equals => {
                        // Convert JSON value to MetadataItem for bloom filter check
                        let metadata_item = crate::core::bloom::json_to_metadata_item(field, value);
                        let result = bloom_filter.might_match_metadata(field, &metadata_item);

                        match result {
                            Ok(might_match) => {
                                debug!(
                                    "🌸 Stage 1: Bloom filter check {}={:?} → {}",
                                    field, value, might_match
                                );
                                might_match
                            }
                            Err(e) => {
                                debug!(
                                    "🌸 Stage 1: Bloom filter check error for {}={:?}: {}",
                                    field, value, e
                                );
                                // On error, assume it might match (conservative approach)
                                true
                            }
                        }
                    }
                    _ => {
                        // Non-equality operators can't use bloom filter efficiently
                        debug!(
                            "🌸 Stage 1: Non-equality operator {:?} - assuming potential matches",
                            operator
                        );
                        true
                    }
                }
            }
            FilterExpression::And(sub_exprs) => {
                // For AND: ALL conditions must potentially match
                let mut all_might_match = true;
                for expr in sub_exprs {
                    if !self.stage1_bloom_filter_check(expr, Some(bloom_filter), stats)? {
                        all_might_match = false;
                        break;
                    }
                }
                debug!("🌸 Stage 1: AND condition → {}", all_might_match);
                all_might_match
            }
            FilterExpression::Or(sub_exprs) => {
                // For OR: ANY condition can potentially match
                let mut any_might_match = false;
                for expr in sub_exprs {
                    if self.stage1_bloom_filter_check(expr, Some(bloom_filter), stats)? {
                        any_might_match = true;
                        break;
                    }
                }
                debug!("🌸 Stage 1: OR condition → {}", any_might_match);
                any_might_match
            }
            FilterExpression::Not(_) => {
                // NOT conditions can't use bloom filter efficiently
                debug!("🌸 Stage 1: NOT condition - assuming potential matches");
                true
            }
        };

        stats.stage1_duration_us = stage_start.elapsed().as_micros() as u64;
        stats.bloom_filter_hits = if might_match { 1 } else { 0 };

        Ok(might_match)
    }

    /// Stage 2: Index Entry Block Filtering
    /// Uses IndexEntry metadata statistics to skip entire data blocks
    async fn stage2_index_entry_filtering(
        &self,
        filter_expr: &FilterExpression,
        index_entries: &[IndexEntry],
        data_blocks: &[ProximaDataBlock],
        stats: &mut FilterStageStats,
    ) -> Result<Vec<ProximaDataBlock>> {
        let stage_start = std::time::Instant::now();
        let mut qualifying_blocks = Vec::new();

        info!(
            "📊 Stage 2: Evaluating {} index entries for block pruning",
            index_entries.len()
        );

        // Group index entries by block_id for efficient processing
        let mut block_index_map: HashMap<u32, Vec<&IndexEntry>> = HashMap::new();
        for entry in index_entries {
            block_index_map
                .entry(entry.block_id)
                .or_insert_with(Vec::new)
                .push(entry);
        }

        for block in data_blocks {
            if let Some(block_entries) = block_index_map.get(&block.block_id) {
                if self.block_might_contain_matches(filter_expr, block_entries)? {
                    debug!(
                        "📊 Stage 2: Block {} qualifies - adding to processing list",
                        block.block_id
                    );
                    qualifying_blocks.push(block.clone());
                } else {
                    debug!(
                        "📊 Stage 2: Block {} skipped - index stats rule out matches",
                        block.block_id
                    );
                    stats.blocks_skipped_by_index += 1;
                }
            } else {
                warn!(
                    "📊 Stage 2: No index entries for block {} - including in processing",
                    block.block_id
                );
                qualifying_blocks.push(block.clone());
            }
        }

        stats.stage2_duration_us = stage_start.elapsed().as_micros() as u64;
        stats.blocks_qualifying_after_index = qualifying_blocks.len();

        info!(
            "📊 Stage 2: {} out of {} blocks qualify for row-level filtering",
            qualifying_blocks.len(),
            data_blocks.len()
        );

        Ok(qualifying_blocks)
    }

    /// Check if a data block might contain matches using IndexEntry statistics
    fn block_might_contain_matches(
        &self,
        filter_expr: &FilterExpression,
        block_entries: &[&IndexEntry],
    ) -> Result<bool> {
        match filter_expr {
            FilterExpression::Comparison {
                field,
                operator,
                value,
            } => {
                // Find index entries for this field
                let field_stats: Vec<_> = block_entries
                    .iter()
                    .filter_map(|entry| {
                        // Check if this index entry has statistics for the field
                        if let (Some(min_val), Some(max_val)) = (
                            entry.metadata_min_values.get(field),
                            entry.metadata_max_values.get(field),
                        ) {
                            Some((min_val, max_val, entry.metadata_null_counts.get(field)))
                        } else {
                            None
                        }
                    })
                    .collect();

                if field_stats.is_empty() {
                    // No statistics available - assume potential matches
                    debug!(
                        "📊 Index Check: No stats for field '{}' - assuming matches",
                        field
                    );
                    return Ok(true);
                }

                // Evaluate based on operator and min/max values
                let might_match = match operator {
                    ComparisonOperator::Equals => {
                        field_stats.iter().any(|(min_val, max_val, _null_count)| {
                            // Value must be within [min, max] range
                            let ge_min = crate::core::search::json_comparison::compare_json_values(
                                value, min_val,
                            ) != std::cmp::Ordering::Less;
                            let le_max = crate::core::search::json_comparison::compare_json_values(
                                value, max_val,
                            ) != std::cmp::Ordering::Greater;
                            ge_min && le_max
                        })
                    }
                    ComparisonOperator::GreaterThan => {
                        field_stats.iter().any(|(min_val, _max_val, _null_count)| {
                            // Some value in block must be > filter value
                            // This is true if max > filter_value, but we check min for safety
                            crate::core::search::json_comparison::compare_json_values(
                                min_val, value,
                            ) == std::cmp::Ordering::Greater
                        })
                    }
                    ComparisonOperator::LessThan => {
                        field_stats.iter().any(|(_min_val, max_val, _null_count)| {
                            // Some value in block must be < filter value
                            crate::core::search::json_comparison::compare_json_values(
                                max_val, value,
                            ) == std::cmp::Ordering::Less
                        })
                    }
                    ComparisonOperator::GreaterThanOrEqual => {
                        field_stats.iter().any(|(_min_val, max_val, _null_count)| {
                            // Some value in block must be >= filter value
                            let max_ge_filter =
                                crate::core::search::json_comparison::compare_json_values(
                                    max_val, value,
                                );
                            max_ge_filter == std::cmp::Ordering::Greater
                                || max_ge_filter == std::cmp::Ordering::Equal
                        })
                    }
                    ComparisonOperator::LessThanOrEqual => {
                        field_stats.iter().any(|(min_val, _max_val, _null_count)| {
                            // Some value in block must be <= filter value
                            let min_le_filter =
                                crate::core::search::json_comparison::compare_json_values(
                                    min_val, value,
                                );
                            min_le_filter == std::cmp::Ordering::Less
                                || min_le_filter == std::cmp::Ordering::Equal
                        })
                    }
                    _ => {
                        // Complex operators - assume potential matches
                        true
                    }
                };

                debug!(
                    "📊 Index Check: Field '{}' {:?} {:?} → {}",
                    field, operator, value, might_match
                );
                Ok(might_match)
            }
            FilterExpression::And(sub_exprs) => {
                // ALL conditions must potentially match
                for expr in sub_exprs {
                    if !self.block_might_contain_matches(expr, block_entries)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            FilterExpression::Or(sub_exprs) => {
                // ANY condition can potentially match
                for expr in sub_exprs {
                    if self.block_might_contain_matches(expr, block_entries)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            FilterExpression::Not(_) => {
                // NOT conditions are complex - assume potential matches
                Ok(true)
            }
        }
    }

    /// Stage 3: Data Block Row Filtering
    /// Fast in-memory filtering on records that passed stages 1-2
    async fn stage3_row_filtering(
        &mut self,
        filter_expr: &FilterExpression,
        qualifying_blocks: &[ProximaDataBlock],
        stats: &mut FilterStageStats,
    ) -> Result<Vec<GlobalRowIndex>> {
        let stage_start = std::time::Instant::now();
        let mut all_qualifying_indices = Vec::new();

        info!(
            "🔍 Stage 3: Row-level filtering on {} qualifying blocks",
            qualifying_blocks.len()
        );

        for block in qualifying_blocks {
            // Convert SstRecord to VectorRecord for filtering
            let vector_records: Vec<VectorRecord> = block
                .records
                .iter()
                .map(|sst_record| sst_record.clone().into())
                .collect();

            // Use batch evaluator for AND/OR support
            let block_indices = match filter_expr {
                FilterExpression::And(_) | FilterExpression::Or(_) => {
                    self.batch_filter
                        .as_ref()
                        .evaluate_parallel_filters_immutable(&vector_records, filter_expr)
                        .await?
                }
                _ => self
                    .row_filter
                    .as_ref()
                    .filter_vector_records_immutable(&vector_records, filter_expr)?,
            };

            // Convert block-local indices to global indices
            for local_idx in block_indices {
                all_qualifying_indices.push(GlobalRowIndex {
                    block_id: block.block_id,
                    local_index: local_idx,
                    global_offset: 0, // Would be calculated based on block positions
                });
            }

            debug!(
                "🔍 Stage 3: Block {} contributed {} qualifying records",
                block.block_id,
                all_qualifying_indices.len()
            );
        }

        stats.stage3_duration_us = stage_start.elapsed().as_micros() as u64;
        stats.final_qualifying_count = all_qualifying_indices.len();

        info!(
            "🔍 Stage 3: Found {} total qualifying records",
            all_qualifying_indices.len()
        );

        Ok(all_qualifying_indices)
    }

    // Arc-wrapped filters are immutable for read operations
    // Cache management should be handled at the service level if needed
}

/// Global row index that can reconstruct the original record
#[derive(Debug, Clone)]
pub struct GlobalRowIndex {
    pub block_id: u32,
    pub local_index: usize,
    pub global_offset: usize,
}

/// Complete filter result with statistics
#[derive(Debug)]
pub struct FilterResult {
    pub qualifying_indices: Vec<GlobalRowIndex>,
    pub stats: FilterStageStats,
    pub blocks_processed: usize,
    pub blocks_skipped: usize,
}

impl FilterResult {
    fn empty_with_stats(stats: FilterStageStats) -> Self {
        Self {
            qualifying_indices: Vec::new(),
            stats,
            blocks_processed: 0,
            blocks_skipped: 0,
        }
    }
}

/// Detailed statistics for each filtering stage
#[derive(Debug, Default)]
pub struct FilterStageStats {
    // Stage 1: Bloom Filter
    pub stage1_duration_us: u64,
    pub bloom_filter_hits: usize,

    // Stage 2: Index Entry Filtering
    pub stage2_duration_us: u64,
    pub blocks_skipped_by_index: usize,
    pub blocks_qualifying_after_index: usize,

    // Stage 3: Row Filtering
    pub stage3_duration_us: u64,
    pub final_qualifying_count: usize,
}

impl FilterStageStats {
    fn new() -> Self {
        Self::default()
    }

    pub fn total_duration_us(&self) -> u64 {
        self.stage1_duration_us + self.stage2_duration_us + self.stage3_duration_us
    }

    pub fn efficiency_report(&self) -> String {
        format!(
            "3-Stage Filter Stats: Stage1={}μs, Stage2={}μs, Stage3={}μs, Total={}μs, Blocks_skipped={}, Final_matches={}",
            self.stage1_duration_us,
            self.stage2_duration_us,
            self.stage3_duration_us,
            self.total_duration_us(),
            self.blocks_skipped_by_index,
            self.final_qualifying_count
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::search::{ComparisonOperator, FilterExpression};
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_three_stage_filtering_pipeline() {
        let mut pipeline = ThreeStageFilterPipeline::new();

        // Create test data blocks
        let data_blocks = create_test_data_blocks();
        let index_entries = create_test_index_entries();

        // Simple equality filter
        let filter = FilterExpression::Comparison {
            field: "category".to_string(),
            operator: ComparisonOperator::Equals,
            value: serde_json::json!("electronics"),
        };

        let result = pipeline
            .filter_with_three_stages(
                "test.sstable",
                &filter,
                None, // No bloom filter in test
                &index_entries,
                &data_blocks,
            )
            .await
            .unwrap();

        debug!("Filter result: {}", result.stats.efficiency_report());
        assert!(
            !result.qualifying_indices.is_empty(),
            "Should find some matches"
        );
    }

    fn create_test_data_blocks() -> Vec<ProximaDataBlock> {
        vec![ProximaDataBlock {
            encoding_marker: 0,
            encoding_metadata: None,
            block_id: 0,
            encoded_vectors: None,
            vector_layout: crate::storage::engines::core::formats::proximablocks::VectorEncodingLayout::Auto,
            records: vec![
                crate::proto::proximadb_v1::VectorRecord {
                    id: "vec1".to_string(),
                    vector: vec![0.1; 128],
                    metadata: {
                        let mut metadata = HashMap::new();
                        metadata.insert(
                            "category".to_string(),
                            crate::proto::proximadb_v1::SqlValue {
                                value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                                    "electronics".to_string(),
                                )),
                            },
                        );
                        metadata
                    },
                    timestamp: 1000,
                    updated_at: None,
                    expires_at: None,
                    version: Some(1),
                    source: None,
                },
                crate::proto::proximadb_v1::VectorRecord {
                    id: "vec2".to_string(),
                    vector: vec![0.2; 128],
                    metadata: {
                        let mut metadata = HashMap::new();
                        metadata.insert(
                            "category".to_string(),
                            crate::proto::proximadb_v1::SqlValue {
                                value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                                    "books".to_string(),
                                )),
                            },
                        );
                        metadata
                    },
                    timestamp: 1001,
                    updated_at: None,
                    expires_at: None,
                    version: Some(1),
                    source: None,
                },
                crate::proto::proximadb_v1::VectorRecord {
                    id: "vec3".to_string(),
                    vector: vec![0.3; 128],
                    metadata: {
                        let mut metadata = HashMap::new();
                        metadata.insert(
                            "category".to_string(),
                            crate::proto::proximadb_v1::SqlValue {
                                value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                                    "electronics".to_string(),
                                )),
                            },
                        );
                        metadata
                    },
                    timestamp: 1002,
                    updated_at: None,
                    expires_at: None,
                    version: Some(1),
                    source: None,
                },
            ],
            quantized_vectors: None,
            quantization_level: None,
            quantized_section: None,
            metadata: crate::storage::engines::core::formats::proximablocks::ProximaBlockMetadata {
                record_count: 3,
                size_bytes: 1024,
                compressed_size: 1024,
                timestamp: 1000,
                compaction_level: 0,
                has_deletes: false,
                has_updates: false,
                version_range: (1, 1),
                column_stats: HashMap::new(),
                quantization_stats: crate::storage::engines::core::formats::proximablocks::QuantizationStatistics::default(),
                data_checksum: 0,
                metadata_checksum: 0,
            },
            compression_config: Default::default(),
            compression_algorithm: crate::core::compression::CompressionAlgorithm::None,
            uncompressed_size: 1024,
            bloom_filter: None,
            block_bloom_filter: None,
            id_range: ("vec1".to_string(), "vec3".to_string()),
            timestamp_range: (1000, 1002),
            statistics: Default::default(),
            metadata_stats: Some(crate::storage::engines::core::formats::proximablocks::BlockMetadataStats {
                unique_keys: 1,
                null_values: 0,
                avg_value_size: 10.0,
                compression_ratio: 1.0,
            }),
            has_deletes: false,
        }]
    }

    fn create_test_index_entries() -> Vec<IndexEntry> {
        vec![IndexEntry {
            key: "vec1".to_string(),
            offset: 0,
            size: 1024,
            block_id: 0,
            block_offset: 0,
            compressed: false,
            metadata_min_values: HashMap::from([(
                "category".to_string(),
                serde_json::json!("books"),
            )]),
            metadata_max_values: HashMap::from([(
                "category".to_string(),
                serde_json::json!("electronics"),
            )]),
            metadata_null_counts: HashMap::new(),
            // Hierarchical bloom filter support
            block_key_bloom: None,
            block_metadata_bloom: None,
            // Vector format optimization
            vector_format: crate::storage::engines::impls::sst::VectorFormat::Variable,
        }]
    }
}
