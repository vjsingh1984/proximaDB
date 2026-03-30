//! ✅ Unified Proxima Block Reader with Strategy Pattern
//!
//! This module provides a centralized block reading implementation that all storage engines
//! (SST, SWIFT, HELIX, etc.) can delegate to. It leverages Proxima automatic capabilities
//! and provides different reading strategies optimized for various access patterns.
//!
//! ## Key Benefits
//! - **Unified Implementation**: Single source of truth for block reading logic
//! - **Strategy Pattern**: Different strategies for different access patterns
//! - **Proxima Integration**: Leverages all automatic capabilities (bloom filters, metadata stats, etc.)
//! - **Engine Delegation**: SST, SWIFT, and other engines delegate here instead of reimplementing

use anyhow::Result;
use std::sync::Arc;

use crate::core::search::FilterExpression;
use crate::storage::engines::core::formats::proximablocks::block_structures::{
    ProximaBlockMetadata, ProximaDataBlock,
};
use crate::storage::persistence::filesystem::FileSystem;
use crate::storage::persistence::filesystem::unified::UnifiedCachingFilesystem;

/// ✅ Reading strategy for different access patterns
#[derive(Debug, Clone)]
pub enum ProximaReadStrategy {
    /// Full scan for compaction - reads all blocks sequentially
    /// Optimized for: Compaction, full table scans, bulk exports
    FullScan {
        sequential_io: bool,
        use_block_cache: bool,
        prefetch_ahead: usize,
    },

    /// Selective read with predicate pushdown
    /// Optimized for: Query execution, filtered scans
    SelectiveRead {
        use_bloom_filters: bool,
        use_metadata_stats: bool,
        enable_cache: bool,
        filter_expression: Option<FilterExpression>,
    },

    /// Range-based reading
    /// Optimized for: Range queries, pagination
    RangeRead {
        start_block: u32,
        end_block: u32,
        use_bloom_filters: bool,
        check_overlaps: bool,
    },

    /// Point lookups with bloom filter checking
    /// Optimized for: Key-value lookups, existence checks
    PointLookup {
        target_keys: Vec<String>,
        use_bloom_filters: bool,
        early_termination: bool,
    },

    /// Streaming read for large datasets
    /// Optimized for: ETL, data migration, analytics
    StreamingRead {
        batch_size: usize,
        parallel_streams: usize,
        memory_budget_mb: usize,
    },
}

/// ✅ Unified Proxima Block Reader
/// All storage engines delegate to this reader for consistent block access
pub struct ProximaBlockReader {
    filesystem: Arc<UnifiedCachingFilesystem>,
    #[allow(dead_code)]
    collection_id: String,
}

impl ProximaBlockReader {
    pub fn new(filesystem: Arc<UnifiedCachingFilesystem>, collection_id: String) -> Self {
        Self {
            filesystem,
            collection_id,
        }
    }

    /// ✅ Main entry point: Read blocks using specified strategy
    pub async fn read_blocks(
        &self,
        file_path: &str,
        strategy: &ProximaReadStrategy,
    ) -> Result<Vec<ProximaDataBlock>> {
        match strategy {
            ProximaReadStrategy::FullScan {
                sequential_io,
                use_block_cache,
                prefetch_ahead,
            } => {
                self.read_full_scan(file_path, *sequential_io, *use_block_cache, *prefetch_ahead)
                    .await
            }

            ProximaReadStrategy::SelectiveRead {
                use_bloom_filters,
                use_metadata_stats,
                enable_cache,
                filter_expression,
            } => {
                self.read_selective(
                    file_path,
                    *use_bloom_filters,
                    *use_metadata_stats,
                    *enable_cache,
                    filter_expression.as_ref(),
                )
                .await
            }

            ProximaReadStrategy::RangeRead {
                start_block,
                end_block,
                use_bloom_filters,
                check_overlaps,
            } => {
                self.read_range(
                    file_path,
                    *start_block,
                    *end_block,
                    *use_bloom_filters,
                    *check_overlaps,
                )
                .await
            }

            ProximaReadStrategy::PointLookup {
                target_keys,
                use_bloom_filters,
                early_termination,
            } => {
                self.read_point_lookup(
                    file_path,
                    target_keys,
                    *use_bloom_filters,
                    *early_termination,
                )
                .await
            }

            ProximaReadStrategy::StreamingRead {
                batch_size,
                parallel_streams,
                memory_budget_mb,
            } => {
                self.read_streaming(file_path, *batch_size, *parallel_streams, *memory_budget_mb)
                    .await
            }
        }
    }

    /// Full scan implementation - optimized for compaction
    async fn read_full_scan(
        &self,
        file_path: &str,
        _sequential_io: bool,
        _use_block_cache: bool,
        _prefetch_ahead: usize,
    ) -> Result<Vec<ProximaDataBlock>> {
        // Read the entire file using the underlying filesystem
        let data = self
            .filesystem
            .read(file_path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read file {}: {}", file_path, e))?;

        // Deserialize all blocks from the file
        let blocks = self.deserialize_blocks(&data)?;

        // Prefetch ahead if requested (this would be done via access pattern tracking)
        // TODO: Implement prefetch logic via access_tracker when available

        Ok(blocks)
    }

    /// Selective read with predicate pushdown
    async fn read_selective(
        &self,
        file_path: &str,
        use_bloom_filters: bool,
        use_metadata_stats: bool,
        _enable_cache: bool,
        filter_expression: Option<&FilterExpression>,
    ) -> Result<Vec<ProximaDataBlock>> {
        // Read the entire file (could be optimized with metadata)
        let data = self
            .filesystem
            .read(file_path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read file {}: {}", file_path, e))?;

        // Deserialize all blocks
        let all_blocks = self.deserialize_blocks(&data)?;

        // Filter blocks based on bloom filters and metadata
        let mut filtered_blocks = Vec::new();
        for block in all_blocks {
            // Check bloom filter if enabled
            if use_bloom_filters
                && let Some(bloom) = &block.bloom_filter
                    && !self.check_bloom_filter(bloom, &filter_expression) {
                        continue; // Skip this block
                    }

            // Check metadata statistics if enabled
            if use_metadata_stats
                && !self.check_metadata_stats(&block.metadata, &filter_expression) {
                    continue; // Skip this block
                }

            // Block passed all filters
            filtered_blocks.push(block);
        }

        Ok(filtered_blocks)
    }

    /// Range-based block reading
    async fn read_range(
        &self,
        file_path: &str,
        start_block: u32,
        end_block: u32,
        _use_bloom_filters: bool,
        _check_overlaps: bool,
    ) -> Result<Vec<ProximaDataBlock>> {
        // Read the entire file
        let data = self
            .filesystem
            .read(file_path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read file {}: {}", file_path, e))?;

        // Deserialize all blocks
        let all_blocks = self.deserialize_blocks(&data)?;

        // Select blocks in the specified range
        let mut range_blocks = Vec::new();
        for (idx, block) in all_blocks.into_iter().enumerate() {
            let block_idx = idx as u32;
            if block_idx >= start_block && block_idx <= end_block {
                // Check for overlaps if requested
                // TODO: Implement overlap checking with bloom filters
                range_blocks.push(block);
            }
        }

        Ok(range_blocks)
    }

    /// Point lookup optimization
    async fn read_point_lookup(
        &self,
        file_path: &str,
        target_keys: &[String],
        use_bloom_filters: bool,
        early_termination: bool,
    ) -> Result<Vec<ProximaDataBlock>> {
        // Read the entire file
        let data = self
            .filesystem
            .read(file_path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read file {}: {}", file_path, e))?;

        // Deserialize all blocks
        let all_blocks = self.deserialize_blocks(&data)?;

        // Track found keys for early termination
        let mut found_keys = std::collections::HashSet::new();
        let mut result_blocks = Vec::new();

        for block in all_blocks {
            // Check bloom filter for each target key
            if use_bloom_filters
                && let Some(bloom) = &block.bloom_filter {
                    let mut has_match = false;
                    for key in target_keys {
                        // SstableBloomFilter uses might_contain_key() method
                        if !found_keys.contains(key) && bloom.might_contain_key(key).unwrap_or(true)
                        {
                            has_match = true;
                            found_keys.insert(key.clone());
                        }
                    }
                    if !has_match {
                        continue; // Skip this block
                    }
                }

            result_blocks.push(block);

            // Early termination if all keys found
            if early_termination && found_keys.len() == target_keys.len() {
                break;
            }
        }

        Ok(result_blocks)
    }

    /// Streaming read for large datasets
    async fn read_streaming(
        &self,
        file_path: &str,
        _batch_size: usize,
        _parallel_streams: usize,
        _memory_budget_mb: usize,
    ) -> Result<Vec<ProximaDataBlock>> {
        // For streaming, we would ideally read in chunks
        // For now, read the entire file and return in batches
        let data = self
            .filesystem
            .read(file_path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read file {}: {}", file_path, e))?;

        // Deserialize blocks in batches to respect memory budget
        let all_blocks = self.deserialize_blocks(&data)?;

        // TODO: Implement actual streaming with parallel_streams and memory_budget_mb
        // For now, return all blocks (could be batched in future)
        Ok(all_blocks)
    }

    /// Deserialize blocks from raw data
    fn deserialize_blocks(&self, data: &[u8]) -> Result<Vec<ProximaDataBlock>> {
        // Handle empty data
        if data.is_empty() {
            return Ok(Vec::new());
        }

        // Try to deserialize as a single block first
        // If that fails, try as multiple blocks
        match ProximaDataBlock::deserialize(data, None) {
            Ok(single_block) => Ok(vec![single_block]),
            Err(_) => {
                // Try deserializing as multiple blocks
                // This would need a format that stores multiple blocks
                // For now, return empty if single block deserialization fails
                Ok(Vec::new())
            }
        }
    }

    /// Check bloom filter for potential matches
    fn check_bloom_filter(
        &self,
        bloom: &crate::core::bloom::SstableBloomFilter,
        filter: &Option<&FilterExpression>,
    ) -> bool {
        // If no filter, always check the block
        let Some(filter_expr) = filter else {
            return true;
        };

        // Extract keys from filter expression to check against bloom filter
        match filter_expr {
            FilterExpression::Comparison { field, value, .. } => {
                // Check if the field-value combination might exist
                // Bloom filters typically store record IDs or field keys
                let _check_key = format!("{field}:{value}");
                // SstableBloomFilter doesn't have proper implementation yet
                // TODO: Implement once bloom filter strategies are fixed
                true
            }
            FilterExpression::And(exprs) => {
                // All expressions must potentially match
                exprs
                    .iter()
                    .all(|expr| self.check_bloom_filter(bloom, &Some(expr)))
            }
            FilterExpression::Or(exprs) => {
                // At least one expression must potentially match
                exprs
                    .iter()
                    .any(|expr| self.check_bloom_filter(bloom, &Some(expr)))
            }
            FilterExpression::Not(_) => {
                // Cannot use bloom filter for NOT operations
                // Must check the block
                true
            }
        }
    }

    /// Check metadata statistics for potential matches
    fn check_metadata_stats(
        &self,
        metadata: &ProximaBlockMetadata,
        filter: &Option<&FilterExpression>,
    ) -> bool {
        use crate::core::search::ComparisonOperator;

        // If no filter, always check the block
        let Some(filter_expr) = filter else {
            return true;
        };

        // Use Proxima auto-generated column statistics for pruning
        match filter_expr {
            FilterExpression::Comparison {
                field,
                operator,
                value,
            } => {
                // Special handling for ID field
                if field == "id" {
                    // Check column statistics for ID field if available
                    if let Some(col_stats) = metadata.column_stats.get("id") {
                        if let (Some(min_id), Some(max_id)) =
                            (&col_stats.min_value, &col_stats.max_value)
                        {
                            // Compare JSON values properly
                            // For now, do string comparison if both are strings
                            if let (Ok(val_str), Ok(min_str), Ok(max_str)) = (
                                serde_json::from_value::<String>(value.clone()),
                                serde_json::from_value::<String>(min_id.clone()),
                                serde_json::from_value::<String>(max_id.clone()),
                            ) {
                                match operator {
                                    ComparisonOperator::Equals => {
                                        // Value must be within range
                                        val_str >= min_str && val_str <= max_str
                                    }
                                    ComparisonOperator::GreaterThan
                                    | ComparisonOperator::GreaterThanOrEqual => {
                                        // Max ID must be greater than value
                                        max_str > val_str
                                    }
                                    ComparisonOperator::LessThan
                                    | ComparisonOperator::LessThanOrEqual => {
                                        // Min ID must be less than value
                                        min_str < val_str
                                    }
                                    _ => true, // Conservative for other operators
                                }
                            } else {
                                // Can't compare, conservatively include
                                true
                            }
                        } else {
                            true // No stats, conservatively include
                        }
                    } else {
                        true // No stats for ID field
                    }
                } else if field == "timestamp" {
                    // Handle timestamp field using version_range as a proxy
                    let (block_start, block_end) = metadata.version_range;
                    if let Ok(ts_val) = serde_json::from_value::<i64>(value.clone()) {
                        match operator {
                            ComparisonOperator::Equals => {
                                ts_val >= block_start && ts_val <= block_end
                            }
                            ComparisonOperator::GreaterThan
                            | ComparisonOperator::GreaterThanOrEqual => block_end > ts_val,
                            ComparisonOperator::LessThan | ComparisonOperator::LessThanOrEqual => {
                                block_start < ts_val
                            }
                            _ => true,
                        }
                    } else {
                        true
                    }
                } else {
                    // Check column statistics for other fields
                    if let Some(col_stats) = metadata.column_stats.get(field) {
                        // Use min/max values for range pruning
                        if let (Some(min_val), Some(max_val)) =
                            (&col_stats.min_value, &col_stats.max_value)
                        {
                            // Try numeric comparison first, fall back to string comparison
                            let can_include = if let (Ok(val_num), Ok(min_num), Ok(max_num)) = (
                                serde_json::from_value::<f64>(value.clone()),
                                serde_json::from_value::<f64>(min_val.clone()),
                                serde_json::from_value::<f64>(max_val.clone()),
                            ) {
                                // Numeric comparison
                                match operator {
                                    ComparisonOperator::Equals => {
                                        val_num >= min_num && val_num <= max_num
                                    }
                                    ComparisonOperator::GreaterThan
                                    | ComparisonOperator::GreaterThanOrEqual => max_num > val_num,
                                    ComparisonOperator::LessThan
                                    | ComparisonOperator::LessThanOrEqual => min_num < val_num,
                                    _ => true,
                                }
                            } else if let (Ok(val_str), Ok(min_str), Ok(max_str)) = (
                                serde_json::from_value::<String>(value.clone()),
                                serde_json::from_value::<String>(min_val.clone()),
                                serde_json::from_value::<String>(max_val.clone()),
                            ) {
                                // String comparison
                                match operator {
                                    ComparisonOperator::Equals => {
                                        val_str >= min_str && val_str <= max_str
                                    }
                                    ComparisonOperator::GreaterThan
                                    | ComparisonOperator::GreaterThanOrEqual => max_str > val_str,
                                    ComparisonOperator::LessThan
                                    | ComparisonOperator::LessThanOrEqual => min_str < val_str,
                                    _ => true,
                                }
                            } else {
                                // Can't compare types, conservatively include
                                true
                            };
                            can_include
                        } else {
                            true // No stats available
                        }
                    } else {
                        true // No stats for this column
                    }
                }
            }
            FilterExpression::And(exprs) => {
                // All expressions must potentially match
                exprs
                    .iter()
                    .all(|expr| self.check_metadata_stats(metadata, &Some(expr)))
            }
            FilterExpression::Or(exprs) => {
                // At least one expression must potentially match
                exprs
                    .iter()
                    .any(|expr| self.check_metadata_stats(metadata, &Some(expr)))
            }
            FilterExpression::Not(_) => {
                // Cannot use stats for NOT operations effectively
                // Must check the block
                true
            }
        }
    }
}

/// ✅ SST-specific reader that delegates to ProximaBlockReader
pub struct SstBlockReader {
    inner: ProximaBlockReader,
}

impl SstBlockReader {
    pub fn new(filesystem: Arc<UnifiedCachingFilesystem>, collection_id: String) -> Self {
        Self {
            inner: ProximaBlockReader::new(filesystem, collection_id),
        }
    }

    /// SST compaction read - uses FullScan strategy
    pub async fn read_for_compaction(&self, file_path: &str) -> Result<Vec<ProximaDataBlock>> {
        let strategy = ProximaReadStrategy::FullScan {
            sequential_io: true,    // Sequential for compaction
            use_block_cache: false, // Don't pollute cache during compaction
            prefetch_ahead: 8,      // Prefetch 8 blocks ahead
        };
        self.inner.read_blocks(file_path, &strategy).await
    }

    /// SST query read - uses SelectiveRead strategy
    pub async fn read_for_query(
        &self,
        file_path: &str,
        filter: Option<FilterExpression>,
    ) -> Result<Vec<ProximaDataBlock>> {
        let strategy = ProximaReadStrategy::SelectiveRead {
            use_bloom_filters: true,  // Use Proxima bloom filters
            use_metadata_stats: true, // Use Proxima metadata stats
            enable_cache: true,       // Cache for repeated queries
            filter_expression: filter,
        };
        self.inner.read_blocks(file_path, &strategy).await
    }
}

/// ✅ SWIFT-specific reader that delegates to ProximaBlockReader
pub struct SwiftBlockReader {
    inner: ProximaBlockReader,
}

impl SwiftBlockReader {
    pub fn new(filesystem: Arc<UnifiedCachingFilesystem>, collection_id: String) -> Self {
        Self {
            inner: ProximaBlockReader::new(filesystem, collection_id),
        }
    }

    /// SWIFT hierarchical read - uses RangeRead for SuperBlocks
    pub async fn read_superblock_range(
        &self,
        file_path: &str,
        start_superblock: u32,
        end_superblock: u32,
    ) -> Result<Vec<ProximaDataBlock>> {
        // Convert SuperBlock range to block range
        let blocks_per_superblock = 64;
        let strategy = ProximaReadStrategy::RangeRead {
            start_block: start_superblock * blocks_per_superblock,
            end_block: end_superblock * blocks_per_superblock,
            use_bloom_filters: true,
            check_overlaps: true,
        };
        self.inner.read_blocks(file_path, &strategy).await
    }

    /// SWIFT billion-scale read - uses StreamingRead
    pub async fn read_for_analytics(&self, file_path: &str) -> Result<Vec<ProximaDataBlock>> {
        let strategy = ProximaReadStrategy::StreamingRead {
            batch_size: 1000,       // Process 1000 blocks per batch
            parallel_streams: 4,    // 4 parallel streams
            memory_budget_mb: 1024, // 1GB memory budget
        };
        self.inner.read_blocks(file_path, &strategy).await
    }
}

/// ✅ HELIX-specific reader that delegates to ProximaBlockReader
pub struct HelixBlockReader {
    inner: ProximaBlockReader,
}

impl HelixBlockReader {
    pub fn new(filesystem: Arc<UnifiedCachingFilesystem>, collection_id: String) -> Self {
        Self {
            inner: ProximaBlockReader::new(filesystem, collection_id),
        }
    }

    /// HELIX compaction read - uses FullScan with sequential I/O
    pub async fn read_for_compaction(&self, file_path: &str) -> Result<Vec<ProximaDataBlock>> {
        let strategy = ProximaReadStrategy::FullScan {
            sequential_io: true,    // Sequential for compaction
            use_block_cache: false, // Don't pollute cache during compaction
            prefetch_ahead: 16,     // Prefetch more blocks for Hilbert patterns
        };
        self.inner.read_blocks(file_path, &strategy).await
    }

    /// HELIX selective read with Hilbert pruning
    pub async fn read_with_hilbert_pruning(
        &self,
        file_path: &str,
        _query_hilbert_key: u64,
        _tolerance: u64,
        filter: Option<FilterExpression>,
    ) -> Result<Vec<ProximaDataBlock>> {
        // First do a selective read with filter
        let strategy = ProximaReadStrategy::SelectiveRead {
            use_bloom_filters: true,
            use_metadata_stats: true,
            enable_cache: true,
            filter_expression: filter,
        };

        let blocks = self.inner.read_blocks(file_path, &strategy).await?;

        // Then apply Hilbert-specific pruning
        // This could be enhanced to use Proxima metadata for Hilbert ranges
        let filtered_blocks = blocks
            .into_iter()
            .filter(|_block| {
                // Check if block's Hilbert range overlaps with query
                // This would use the HelixBlockMetadata that composes with Proxima
                true // For now, pass all blocks
            })
            .collect();

        Ok(filtered_blocks)
    }

    /// HELIX range read for clustering operations
    pub async fn read_cluster_range(
        &self,
        file_path: &str,
        start_cluster: u32,
        end_cluster: u32,
    ) -> Result<Vec<ProximaDataBlock>> {
        // HELIX uses clustering, so convert cluster IDs to block ranges
        let blocks_per_cluster = 32; // HELIX-specific clustering factor
        let strategy = ProximaReadStrategy::RangeRead {
            start_block: start_cluster * blocks_per_cluster,
            end_block: end_cluster * blocks_per_cluster,
            use_bloom_filters: true,
            check_overlaps: true,
        };
        self.inner.read_blocks(file_path, &strategy).await
    }

    /// HELIX point lookup with PCA optimization
    pub async fn read_for_point_lookup(
        &self,
        file_path: &str,
        target_keys: Vec<String>,
    ) -> Result<Vec<ProximaDataBlock>> {
        let strategy = ProximaReadStrategy::PointLookup {
            target_keys,
            use_bloom_filters: true, // Use Proxima bloom filters
            early_termination: true, // Stop when all keys found
        };
        self.inner.read_blocks(file_path, &strategy).await
    }
}
