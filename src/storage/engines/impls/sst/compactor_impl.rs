// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Zero-copy SST Compactor using modular block reader architecture
//!
//! OPTIMIZED: This compactor works directly with VectorRecord to eliminate all conversions
//! during compaction operations, significantly improving performance for
//! large-scale compactions.

use super::SstableWriter; // OPTIMIZED: Removed SstRecord import
use super::block_format::BlockFormatReader;
use super::readers::sst_query_engine::{BlockIterator, SstDirectReader};
use crate::core::search::mvcc_resolution::MvccResolver;
use crate::proto::proximadb_v1::VectorRecord; // OPTIMIZED: Direct VectorRecord usage
use crate::storage::engines::core::formats::arrow_block::{ArrowBlockConfig, ArrowBlockWriter};
use crate::storage::persistence::filesystem::FilesystemFactory;
// Quantization now handled by unified compute module
use anyhow::Result;
use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashMap};
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Statistics for zero-copy compaction with AXIS integration
#[derive(Debug, Clone, Default)]
pub struct ZeroCopyCompactionStats {
    pub records_read: u64,
    pub records_written: u64,
    pub records_deleted: u64,
    pub records_merged: u64,
    pub bytes_read: u64,
    pub bytes_written: u64,
    pub files_compacted: usize,
    pub compaction_time_ms: u64,

    // AXIS integration: track changes for index updates
    pub deleted_vector_ids: Vec<String>, // IDs of deleted/expired vectors
    pub updated_vector_ids: Vec<String>, // IDs of vectors that were updated (version change)
    pub tombstoned_ids: Vec<String>,     // IDs marked as tombstones
    pub recommend_index_rebuild: bool,   // True if significant changes warrant index rebuild
}

/// Entry in the k-way merge heap
#[derive(Debug, Clone)]
struct MergeEntry {
    record: VectorRecord, // OPTIMIZED: Direct VectorRecord usage
    file_index: usize,
    timestamp: u32,
}

impl Eq for MergeEntry {}

impl PartialEq for MergeEntry {
    fn eq(&self, other: &Self) -> bool {
        self.record.id == other.record.id && self.timestamp == other.timestamp
    }
}

impl Ord for MergeEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        // First compare by ID (ascending)
        match self.record.id.cmp(&other.record.id) {
            Ordering::Equal => {
                // Then by timestamp (descending - newer first)
                other.timestamp.cmp(&self.timestamp)
            }
            other => other.reverse(), // Reverse for min-heap behavior
        }
    }
}

impl PartialOrd for MergeEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Sorting strategy for compacted records
#[derive(Clone)]
pub enum CompactionSortStrategy {
    /// Sort by record ID (default for SSTable binary search)
    ById,
    /// Sort by metadata fields for better compression
    ByMetadata(Vec<String>),
    /// Sort by timestamp (newest first)
    ByTimestamp,
    /// Sort by PQ similarity for better compression and progressive search
    ByPQSimilarity(Arc<crate::compute::quantization::storage_engine::StorageQuantizationEngine>),
    /// Custom comparator function
    Custom(Arc<dyn Fn(&VectorRecord, &VectorRecord) -> Ordering + Send + Sync>),
}

impl std::fmt::Debug for CompactionSortStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ById => write!(f, "ById"),
            Self::ByMetadata(keys) => f.debug_tuple("ByMetadata").field(keys).finish(),
            Self::ByTimestamp => write!(f, "ByTimestamp"),
            Self::ByPQSimilarity(_) => write!(f, "ByPQSimilarity"),
            Self::Custom(_) => write!(f, "Custom(<function>)"),
        }
    }
}

impl Default for CompactionSortStrategy {
    fn default() -> Self {
        CompactionSortStrategy::ById
    }
}

/// Zero-copy SST compactor that works directly with VectorRecord
pub struct SstCompactor {
    filesystem_factory: Arc<FilesystemFactory>,
    mvcc_resolver: Option<Arc<MvccResolver>>,
    block_size: usize,
    compression_threshold: usize,
    sort_strategy: CompactionSortStrategy,
}

impl SstCompactor {
    /// Notify AXIS indexes of changes during compaction
    async fn notify_axis_of_changes(
        &self,
        stats: &ZeroCopyCompactionStats,
        collection_id: &str,
        output_file: &str,
    ) -> Result<()> {
        // This would integrate with AXIS similar to how EnhancedCompactionStats does it
        // For now, just log the notification
        if stats.recommend_index_rebuild {
            warn!("🔄 AXIS notification: Index rebuild recommended due to significant changes");
        } else if !stats.deleted_vector_ids.is_empty() {
            info!(
                "📢 AXIS notification: {} vectors deleted",
                stats.deleted_vector_ids.len()
            );
        }

        if !stats.updated_vector_ids.is_empty() {
            info!(
                "📢 AXIS notification: {} vectors updated",
                stats.updated_vector_ids.len()
            );
        }

        // Actual AXIS integration for index updates
        // TODO: Add axis_manager field to SstCompactor or use event log service
        debug!(
            "AXIS integration placeholder - compaction completed for collection {}",
            collection_id
        );
        Ok(())
    }

    /// Check if a record is append-only (no meaningful ID)
    #[inline]
    fn is_append_only(id: &str) -> bool {
        id.is_empty() || id == "null" || id == "none" || id.trim().is_empty()
    }

    /// Normalize version: None or 0 becomes 1
    #[inline]
    fn normalize_version(version: Option<u32>) -> u32 {
        match version {
            None | Some(0) => 1,
            Some(v) => v,
        }
    }

    pub fn new(
        filesystem_factory: Arc<FilesystemFactory>,
        mvcc_resolver: Option<Arc<MvccResolver>>,
    ) -> Self {
        Self {
            filesystem_factory,
            mvcc_resolver,
            block_size: 2048 * 1024, // Default 2MB blocks (same as SST default)
            compression_threshold: 1024, // Compress blocks > 1KB
            sort_strategy: CompactionSortStrategy::default(),
        }
    }

    /// Create compactor with custom block size
    pub fn with_block_size(
        filesystem_factory: Arc<FilesystemFactory>,
        mvcc_resolver: Option<Arc<MvccResolver>>,
        block_size_kb: u32,
    ) -> Self {
        Self {
            filesystem_factory,
            mvcc_resolver,
            block_size: (block_size_kb * 1024) as usize,
            compression_threshold: 1024,
            sort_strategy: CompactionSortStrategy::default(),
        }
    }

    /// Set the sorting strategy for compacted records
    pub fn with_sort_strategy(mut self, strategy: CompactionSortStrategy) -> Self {
        self.sort_strategy = strategy;
        self
    }

    /// Create compactor with PQ-based sorting for better compression
    pub fn with_pq_sorting(
        mut self,
        quantization_engine: Arc<
            crate::compute::quantization::storage_engine::StorageQuantizationEngine,
        >,
    ) -> Self {
        self.sort_strategy = CompactionSortStrategy::ByPQSimilarity(quantization_engine);
        self
    }

    /// Get the current compaction sort strategy
    pub fn sort_strategy(&self) -> &CompactionSortStrategy {
        &self.sort_strategy
    }

    /// Perform zero-copy k-way merge compaction of multiple SST files
    /// This method reads VectorRecords directly without any conversion
    pub async fn compact_files(
        &self,
        input_files: Vec<String>,
        output_file: String,
        target_level: u8,
        compression_config: Option<crate::proto::proximadb_v1::CompressionConfig>,
    ) -> Result<ZeroCopyCompactionStats> {
        debug!("🔍 SST_COMPACTOR: compact_files called");
        debug!("   Input files: {:?}", input_files);
        debug!("   Output file: {}", output_file);
        debug!("   Target level: {}", target_level);
        debug!("   Block size: {} KB", self.block_size / 1024);
        debug!(
            "   Compression config: {:?}",
            compression_config
                .as_ref()
                .map(|c| format!("algorithm={}, level={:?}", c.algorithm, c.level))
        );
        let start = std::time::Instant::now();
        let mut stats = ZeroCopyCompactionStats::default();
        stats.files_compacted = input_files.len();

        info!(
            "🚀 Starting zero-copy compaction of {} files to level {}",
            input_files.len(),
            target_level
        );

        // Use streaming approach for memory-efficient compaction
        info!("🔄 Using streaming approach for zero-copy compaction_info");
        let mut streaming_iterators = Vec::new();
        let total_records_estimate = 0;
        for (idx, file_path) in input_files.iter().enumerate() {
            debug!("   📂 Opening file {}: {}", idx, file_path);
            let mut direct_reader =
                SstDirectReader::open(self.filesystem_factory.clone(), file_path).await?;
            // Try to get record count from header or estimate
            let iterator = direct_reader
                .stream_vector_records(file_path.clone())
                .await?;
            debug!("   ✅ Created streaming iterator for file {}", file_path);
            streaming_iterators.push((idx, iterator));
        }
        debug!(
            "   📊 Starting k-way merge of {} iterators",
            streaming_iterators.len()
        );

        // Perform streaming k-way merge
        let (merged_records, merge_stats) = self.k_way_merge(streaming_iterators).await?;

        // Retrain PCA model on merged data for better Z-Order spatial encoding
        // Extract collection_id from output file path
        let collection_id = output_file.split('/').nth(1).unwrap_or("unknown");
        if merged_records.len() >= 100 {
            self.retrain_pca_model_for_compaction(collection_id, &output_file, &merged_records)
                .await;
        }

        // Merge the stats from k-way merge
        stats.records_read = merge_stats.records_read;
        stats.records_deleted = merge_stats.records_deleted;
        stats.records_merged = merge_stats.records_merged;
        stats.deleted_vector_ids = merge_stats.deleted_vector_ids;
        stats.updated_vector_ids = merge_stats.updated_vector_ids;
        stats.tombstoned_ids = merge_stats.tombstoned_ids;
        stats.recommend_index_rebuild = merge_stats.recommend_index_rebuild;

        // Write merged records to output file
        debug!(
            "   📝 Writing {} merged records to output",
            merged_records.len()
        );
        let writer_stats = self
            .write_merged_records(
                merged_records,
                &output_file,
                target_level,
                compression_config,
            )
            .await?;

        stats.records_written = writer_stats.records_written;
        stats.bytes_written = writer_stats.bytes_written;
        stats.compaction_time_ms = start.elapsed().as_millis() as u64;

        info!(
            "✅ Zero-copy compaction completed in {}ms: {} records written, {} bytes, {} deleted, {} updated",
            stats.compaction_time_ms,
            stats.records_written,
            stats.bytes_written,
            stats.deleted_vector_ids.len(),
            stats.updated_vector_ids.len()
        );

        // Delete input files after successful compaction
        let fs = self.filesystem_factory.get_filesystem("file:///")?;
        for input_file in &input_files {
            if let Err(e) = fs.delete(input_file).await {
                warn!("Failed to delete input file {}: {}", input_file, e);
            } else {
                debug!("🗑️ Deleted input file: {}", input_file);
            }
        }

        // Send AXIS update notifications if there are changes
        if !stats.deleted_vector_ids.is_empty() || !stats.updated_vector_ids.is_empty() {
            // Extract collection_id from output file path or use placeholder
            let collection_id = output_file.split('/').nth(1).unwrap_or("unknown");
            // Best-effort notification - don't fail compaction if AXIS notification fails
            let _ = self
                .notify_axis_of_changes(&stats, collection_id, &output_file)
                .await;
        }

        Ok(stats)
    }

    /// K-way merge of pre-loaded SST records with proper MVCC resolution
    /// Implements upsert semantics: keeps highest continuous version for each ID
    /// FALLBACK: This batch loading approach is kept for compatibility/testing
    async fn k_way_merge_records(
        &self,
        file_records: Vec<(usize, Vec<VectorRecord>)>,
    ) -> Result<(Vec<VectorRecord>, ZeroCopyCompactionStats)> {
        let mut heap = BinaryHeap::new();
        let mut merged_records = Vec::new();
        let mut stats = ZeroCopyCompactionStats::default();
        // Track all versions for each ID to apply MVCC rules
        let mut id_versions: HashMap<String, Vec<VectorRecord>> = HashMap::new();

        // Add all records to the heap for k-way merge
        for (file_idx, records) in file_records {
            for record in records {
                stats.records_read += 1;
                heap.push(Reverse(MergeEntry {
                    timestamp: record.timestamp.unwrap_or(0) as u32,
                    record,
                    file_index: file_idx,
                }));
            }
        }

        // Process all records from the heap
        while let Some(Reverse(entry)) = heap.pop() {
            let record_id = entry.record.id.clone().clone();

            // Collect all versions of each ID
            id_versions
                .entry(record_id.clone())
                .or_default()
                .push(entry.record);
        }

        // Separate append-only records (no ID) from versioned records
        let mut append_only_records = Vec::new();
        if let Some(no_id_records) = id_versions.remove("") {
            append_only_records.extend(no_id_records);
        }

        // Now apply MVCC resolution rules for each ID
        let now = chrono::Utc::now().timestamp() as u32;

        for (id, mut versions) in id_versions {
            // Skip append-only IDs
            if Self::is_append_only(&id) {
                append_only_records.extend(versions);
                continue;
            }

            // Track if this ID has any tombstones
            // Tombstone indicators (consistent with search coordinator):
            // 1. Empty vector (primary indicator - marks deleted records)
            // 2. expires_at in the past (secondary - marks expired tombstones ready for cleanup)
            let current_time = chrono::Utc::now().timestamp() as u32;

            // Check for empty vector tombstones (deleted records)
            // Tombstone design: empty vector + expires_at in past (including 0 = epoch = always past)
            let has_empty_vector_tombstone = versions.iter().any(|r| r.vector.is_empty());
            if has_empty_vector_tombstone {
                // Check if any tombstone has expired (past grace period)
                // expires_at = 0 means "epoch time" which is always in the past = tombstone marker
                let tombstone_expired = versions.iter().any(|r| {
                    r.vector.is_empty()
                        && r.expires_at.map_or(false, |exp| exp <= current_time as i64)
                });

                if tombstone_expired {
                    // Tombstone grace period has passed - fully remove
                    debug!("Removing expired tombstone for record: {}", id);
                    stats.tombstoned_ids.push(id.clone());
                    stats.records_deleted += versions.len() as u64;
                    continue;
                } else {
                    // Tombstone still within grace period - keep the tombstone marker
                    // but don't include any actual data versions
                    debug!("Keeping active tombstone for record: {}", id);
                    // Keep only the tombstone record (newest empty vector)
                    if let Some(tombstone) = versions.iter().find(|r| r.vector.is_empty()).cloned()
                    {
                        stats.tombstoned_ids.push(id.clone());
                        merged_records.push(tombstone);
                    }
                    continue;
                }
            }

            // Check for expired records (via expires_at without empty vector)
            let has_expired = versions
                .iter()
                .any(|r| r.expires_at.map_or(false, |exp| exp < current_time as i64));
            if has_expired {
                debug!("Skipping expired record: {}", id);
                stats.deleted_vector_ids.push(id.clone());
                stats.records_deleted += versions.len() as u64;
                continue;
            }

            // Track if this ID has multiple versions (indicates an update)
            let has_multiple_versions = versions.len() > 1;

            // Sort by version (ascending), then by timestamp (ascending for same version)
            versions.sort_by(|a, b| {
                let ver_a = Self::normalize_version(a.version.map(|v| v as u32));
                let ver_b = Self::normalize_version(b.version.map(|v| v as u32));

                ver_a
                    .cmp(&ver_b)
                    .then_with(|| a.timestamp.cmp(&b.timestamp))
            });

            // Find the highest continuous version
            let mut expected_version = 1u32;
            let mut last_valid: Option<VectorRecord> = None;

            for record in versions {
                let version = Self::normalize_version(record.version.map(|v| v as u32));

                if version == expected_version {
                    // This version is continuous
                    last_valid = Some(record);
                    expected_version += 1;
                } else if version > expected_version {
                    // Version gap detected - stop here
                    debug!(
                        "Version gap for ID '{}': expected {}, found {}",
                        id, expected_version, version
                    );
                    break;
                }
                // Skip older versions (version < expected)
            }

            // Add the highest continuous version to output
            if let Some(record) = last_valid {
                let selected_version = Self::normalize_version(record.version.map(|v| v as u32));

                // Track if this was an update (version > 1 OR multiple records with same ID)
                if selected_version > 1 || has_multiple_versions {
                    stats.updated_vector_ids.push(id.clone());
                    stats.records_merged += 1;
                }

                // Update the level to match the target compaction level
                // Use version field instead of deprecated level field

                debug!("Selected version {} for ID '{}'", selected_version, id);
                merged_records.push(record);
            }
        }

        // Add all append-only records (they don't participate in deduplication)
        merged_records.extend(append_only_records);

        // Determine if index rebuild is recommended
        let total_changes = stats.deleted_vector_ids.len()
            + stats.updated_vector_ids.len()
            + stats.tombstoned_ids.len();
        let change_ratio = if stats.records_read > 0 {
            total_changes as f64 / stats.records_read as f64
        } else {
            0.0
        };

        // Recommend rebuild if more than 30% of records changed
        stats.recommend_index_rebuild = change_ratio > 0.3;

        if stats.recommend_index_rebuild {
            warn!(
                "🔄 Index rebuild recommended: {:.1}% of records changed during compaction_info",
                change_ratio * 100.0
            );
        }

        info!(
            "K-way merge completed: {} records after MVCC resolution (deleted: {}, updated: {}, tombstoned: {})",
            merged_records.len(),
            stats.deleted_vector_ids.len(),
            stats.updated_vector_ids.len(),
            stats.tombstoned_ids.len()
        );

        Ok((merged_records, stats))
    }

    /// K-way merge of multiple SST record streams with proper MVCC resolution
    /// Implements upsert semantics: keeps highest continuous version for each ID
    /// Uses streaming iterators for memory-efficient compaction
    async fn k_way_merge(
        &self,
        iterators: Vec<(usize, BlockIterator<VectorRecord>)>,
    ) -> Result<(Vec<VectorRecord>, ZeroCopyCompactionStats)> {
        let mut heap = BinaryHeap::new();
        let mut merged_records = Vec::new();
        let mut stats = ZeroCopyCompactionStats::default();
        // Track all versions for each ID to apply MVCC rules
        let mut id_versions: HashMap<String, Vec<VectorRecord>> = HashMap::new();
        let mut active_iterators: Vec<(usize, BlockIterator<VectorRecord>)> = Vec::new();

        // Initialize heap with first record from each iterator
        for (file_idx, mut iter) in iterators.into_iter() {
            if let Some(record) = iter.next() {
                let rec = record?;
                heap.push(Reverse(MergeEntry {
                    timestamp: rec.timestamp.unwrap_or(0) as u32,
                    record: rec,
                    file_index: file_idx,
                }));
                active_iterators.push((file_idx, iter));
            }
        }

        // Collect all records grouped by ID
        while let Some(Reverse(entry)) = heap.pop() {
            let record_id = entry.record.id.clone().clone();
            stats.records_read += 1;

            // Collect all versions of each ID
            id_versions
                .entry(record_id.clone())
                .or_default()
                .push(entry.record);

            // Get next record from the same file
            if let Some((_, iter)) = active_iterators
                .iter_mut()
                .find(|(idx, _)| *idx == entry.file_index)
            {
                if let Some(next_record) = iter.next() {
                    let rec = next_record?;
                    heap.push(Reverse(MergeEntry {
                        timestamp: rec.timestamp.unwrap_or(0) as u32,
                        record: rec,
                        file_index: entry.file_index,
                    }));
                }
            }
        }

        // Separate append-only records (no ID) from versioned records
        let mut append_only_records = Vec::new();
        if let Some(no_id_records) = id_versions.remove("") {
            append_only_records.extend(no_id_records);
        }

        // Now apply MVCC resolution rules for each ID
        let now = chrono::Utc::now().timestamp() as u32;

        for (id, mut versions) in id_versions {
            // Skip append-only IDs
            if Self::is_append_only(&id) {
                append_only_records.extend(versions);
                continue;
            }

            // Track if this ID has any tombstones
            // Tombstone indicators (consistent with search coordinator):
            // 1. Empty vector (primary indicator - marks deleted records)
            // 2. expires_at in the past (secondary - marks expired tombstones ready for cleanup)
            let current_time = chrono::Utc::now().timestamp() as u32;

            // Check for empty vector tombstones (deleted records)
            // Tombstone design: empty vector + expires_at in past (including 0 = epoch = always past)
            let has_empty_vector_tombstone = versions.iter().any(|r| r.vector.is_empty());
            if has_empty_vector_tombstone {
                // Check if any tombstone has expired (past grace period)
                // expires_at = 0 means "epoch time" which is always in the past = tombstone marker
                let tombstone_expired = versions.iter().any(|r| {
                    r.vector.is_empty()
                        && r.expires_at.map_or(false, |exp| exp <= current_time as i64)
                });

                if tombstone_expired {
                    // Tombstone grace period has passed - fully remove
                    debug!("Removing expired tombstone for record: {}", id);
                    stats.tombstoned_ids.push(id.clone());
                    stats.records_deleted += versions.len() as u64;
                    continue;
                } else {
                    // Tombstone still within grace period - keep the tombstone marker
                    // but don't include any actual data versions
                    debug!("Keeping active tombstone for record: {}", id);
                    // Keep only the tombstone record (newest empty vector)
                    if let Some(tombstone) = versions.iter().find(|r| r.vector.is_empty()).cloned()
                    {
                        stats.tombstoned_ids.push(id.clone());
                        merged_records.push(tombstone);
                    }
                    continue;
                }
            }

            // Check for expired records (via expires_at without empty vector)
            let has_expired = versions
                .iter()
                .any(|r| r.expires_at.map_or(false, |exp| exp < current_time as i64));
            if has_expired {
                debug!("Skipping expired record: {}", id);
                stats.deleted_vector_ids.push(id.clone());
                stats.records_deleted += versions.len() as u64;
                continue;
            }

            // Track if this ID has multiple versions (indicates an update)
            let has_multiple_versions = versions.len() > 1;

            // Sort by version (ascending), then by timestamp (ascending for same version)
            versions.sort_by(|a, b| {
                let ver_a = Self::normalize_version(a.version.map(|v| v as u32));
                let ver_b = Self::normalize_version(b.version.map(|v| v as u32));

                ver_a
                    .cmp(&ver_b)
                    .then_with(|| a.timestamp.cmp(&b.timestamp))
            });

            // Find the highest continuous version
            let mut expected_version = 1u32;
            let mut last_valid: Option<VectorRecord> = None;

            for record in versions {
                let version = Self::normalize_version(record.version.map(|v| v as u32));

                if version == expected_version {
                    // This version is continuous
                    last_valid = Some(record);
                    expected_version += 1;
                } else if version > expected_version {
                    // Version gap detected - stop here
                    debug!(
                        "Version gap for ID '{}': expected {}, found {}",
                        id, expected_version, version
                    );
                    break;
                }
                // Skip older versions (version < expected)
            }

            // Add the highest continuous version to output
            if let Some(record) = last_valid {
                let selected_version = Self::normalize_version(record.version.map(|v| v as u32));

                // Track if this was an update (version > 1 OR multiple records with same ID)
                if selected_version > 1 || has_multiple_versions {
                    stats.updated_vector_ids.push(id.clone());
                    stats.records_merged += 1;
                }

                // Update the level to match the target compaction level
                // Use version field instead of deprecated level field

                debug!("Selected version {} for ID '{}'", selected_version, id);
                merged_records.push(record);
            }
        }

        // Add all append-only records (they don't participate in deduplication)
        merged_records.extend(append_only_records);

        // Determine if index rebuild is recommended
        let total_changes = stats.deleted_vector_ids.len()
            + stats.updated_vector_ids.len()
            + stats.tombstoned_ids.len();
        let change_ratio = if stats.records_read > 0 {
            total_changes as f64 / stats.records_read as f64
        } else {
            0.0
        };

        // Recommend rebuild if more than 30% of records changed
        stats.recommend_index_rebuild = change_ratio > 0.3;

        if stats.recommend_index_rebuild {
            warn!(
                "🔄 Index rebuild recommended: {:.1}% of records changed during compaction_info",
                change_ratio * 100.0
            );
        }

        info!(
            "K-way merge completed: {} records after MVCC resolution (deleted: {}, updated: {}, tombstoned: {})",
            merged_records.len(),
            stats.deleted_vector_ids.len(),
            stats.updated_vector_ids.len(),
            stats.tombstoned_ids.len()
        );

        Ok((merged_records, stats))
    }

    /// Write merged records to a new SST file
    async fn write_merged_records(
        &self,
        mut records: Vec<VectorRecord>,
        output_path: &str,
        level: u8,
        compression_config: Option<crate::proto::proximadb_v1::CompressionConfig>,
    ) -> Result<WriterStats> {
        debug!("🔍 SST_COMPACTOR: write_merged_records");
        debug!("   Records to write: {}", records.len());
        debug!("   Output path: {}", output_path);
        debug!("   Target level: {}", level);
        debug!("   Block size for writer: {} KB", self.block_size / 1024);
        debug!(
            "   Compression: {:?}",
            compression_config
                .as_ref()
                .map(|c| format!("algorithm={}, level={:?}", c.algorithm, c.level))
        );
        let mut stats = WriterStats::default();

        // CRITICAL: Sort records based on configured strategy
        // SSTable requires records to be sorted for efficient binary search
        // Handle append-only records (no ID) specially
        match &self.sort_strategy {
            CompactionSortStrategy::ById => {
                records.sort_by(|a, b| {
                    // Check if either record is append-only
                    let a_is_append = Self::is_append_only(&a.id);
                    let b_is_append = Self::is_append_only(&b.id);

                    match (a_is_append, b_is_append) {
                        (true, true) => {
                            // Both are append-only: sort by timestamp, then by metadata
                            a.timestamp.cmp(&b.timestamp)
                        }
                        (true, false) => Ordering::Greater, // Append-only records go to the end
                        (false, true) => Ordering::Less,
                        (false, false) => a.id.cmp(&b.id), // Normal ID comparison
                    }
                });
            }
            CompactionSortStrategy::ByTimestamp => {
                records.sort_by(|a, b| b.timestamp.cmp(&a.timestamp)); // Newest first
            }
            CompactionSortStrategy::ByMetadata(keys) => {
                records.sort_by(|a, b| {
                    // Sort by specified metadata keys in order
                    for key in keys {
                        // Find metadata items with matching key
                        let a_val = a
                            .metadata
                            .get(key)
                            .and_then(|sql_val| match &sql_val.value {
                                Some(
                                    crate::proto::proximadb_v1::sql_value::Value::StringValue(s),
                                ) => Some(s.as_str()),
                                _ => None,
                            });

                        let b_val = b
                            .metadata
                            .get(key)
                            .and_then(|sql_val| match &sql_val.value {
                                Some(
                                    crate::proto::proximadb_v1::sql_value::Value::StringValue(s),
                                ) => Some(s.as_str()),
                                _ => None,
                            });

                        match (a_val, b_val) {
                            (Some(av), Some(bv)) => {
                                let cmp = av.cmp(bv);
                                if cmp != Ordering::Equal {
                                    return cmp;
                                }
                            }
                            (Some(_), None) => return Ordering::Less,
                            (None, Some(_)) => return Ordering::Greater,
                            (None, None) => continue,
                        }
                    }
                    // Fall back to ID comparison if metadata is equal
                    a.id.cmp(&b.id)
                });
            }
            CompactionSortStrategy::ByPQSimilarity(adapter) => {
                info!(
                    "🎯 Applying PQ-based similarity sorting for {} records",
                    records.len()
                );

                // ZERO-COPY APPROACH: Extract vectors directly from VectorRecord without conversion
                let vectors: Vec<Vec<f32>> = records
                    .iter()
                    .map(|sst_record| sst_record.vector.clone())
                    .collect();

                let vector_ids: Vec<String> = records
                    .iter()
                    .map(|sst_record| sst_record.id.clone().clone())
                    .collect();

                // Quantize vectors for similarity clustering
                let runtime = tokio::runtime::Handle::current();
                let sorted_indices = runtime
                    .block_on(async {
                        // Quantize the vectors using the quantization engine directly
                        let quantized_data = adapter
                            .quantize_batch(&vectors, Some(vector_ids.as_slice()))
                            .await
                            .map_err(|e| anyhow::anyhow!("Quantization failed: {}", e))?;

                        // Create similarity clusters based on PQ codes for optimal compression
                        // For now, just return indices in original order
                        // TODO: Implement proper PQ-based clustering when method is available
                        let sorted_indices: Vec<usize> = (0..vectors.len()).collect();

                        Ok::<Vec<usize>, anyhow::Error>(sorted_indices)
                    })
                    .map_err(|e| anyhow::anyhow!("PQ sorting failed: {}", e))?;

                // Reorder records based on PQ similarity clusters
                let mut sorted_records = Vec::with_capacity(records.len());
                for &index in &sorted_indices {
                    if index < records.len() {
                        let record = records[index].clone();
                        // Use version field - level field deprecated
                        sorted_records.push(record);
                    }
                }

                // Use sorted records (any remaining records that weren't clustered)
                if sorted_records.len() < records.len() {
                    let mut used = vec![false; records.len()];
                    for &index in &sorted_indices {
                        if index < used.len() {
                            used[index] = true;
                        }
                    }

                    for (i, record) in records.into_iter().enumerate() {
                        if !used[i] {
                            // Use version field - level field deprecated
                            sorted_records.push(record);
                        }
                    }
                }

                records = sorted_records;
                info!(
                    "✅ PQ-based sorting completed: {} records reordered for better compression",
                    records.len()
                );
            }
            CompactionSortStrategy::Custom(comparator) => {
                records.sort_by(|a, b| comparator(a, b));
            }
        }

        info!(
            "📊 Writing {} sorted records to file at level {}",
            records.len(),
            level
        );

        // Detect format from file extension
        let block_format = BlockFormatReader::detect_format(output_path);
        debug!(
            "🔍 SST_COMPACTOR: Detected block format: {:?} for path: {}",
            block_format, output_path
        );

        match block_format {
            super::block_format::BlockFormat::ArrowBlock => {
                // Use ArrowBlockWriter for Arrow IPC format
                debug!("🔍 SST_COMPACTOR: Using ArrowBlockWriter for Arrow format");

                // Infer dimension from first record
                let dimension = records
                    .first()
                    .map(|r| r.vector.len() as u32)
                    .unwrap_or(128);

                // Ensure parent directory exists
                let path = std::path::Path::new(output_path);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }

                let config = ArrowBlockConfig::new(dimension);
                let mut writer = ArrowBlockWriter::new(path, config)?;

                // Update stats
                for record in &records {
                    stats.records_written += 1;
                    stats.bytes_written += bincode::serialized_size(record).unwrap_or(0);
                }

                writer.write_block(&records)?;
                writer.finalize()?;

                info!(
                    "✅ SST_COMPACTOR: Wrote {} records to Arrow block",
                    records.len()
                );
            }
            super::block_format::BlockFormat::ProximaBlocks => {
                // Use SstableWriter for ProximaBlocks format
                debug!("🔍 SST_COMPACTOR: Creating SstableWriter");
                let writer = if let Some(ref compression) = compression_config {
                    debug!(
                        "   ✅ WITH compression: algorithm={}, level={:?}",
                        compression.algorithm, compression.level
                    );
                    debug!("   Block size passed to writer: {} bytes", self.block_size);
                    SstableWriter::with_compression(
                        output_path,
                        self.block_size,
                        self.filesystem_factory.clone(),
                        Some(compression.clone()),
                    )
                } else {
                    debug!("   ⚠️ NO compression - using default writer");
                    debug!("   Block size passed to writer: {} bytes", self.block_size);
                    SstableWriter::new(
                        output_path,
                        self.block_size,
                        self.filesystem_factory.clone(),
                    )
                };

                // Convert records to sorted format (id, record)
                let sorted_records: Vec<(String, VectorRecord)> = records
                    .into_iter()
                    .map(|r| {
                        let id = r.id.clone().clone();
                        stats.records_written += 1;
                        stats.bytes_written += bincode::serialized_size(&r).unwrap_or(0);
                        (id, r)
                    })
                    .collect();

                let record_count = sorted_records.len();

                // Write using the streaming API
                writer
                    .write_sorted_vector_records(sorted_records.into_iter(), record_count)
                    .await?;
            }
        }

        Ok(stats)
    }

    /// Compact with level-tiered strategy
    pub async fn compact_level_tiered(
        &self,
        level_files: HashMap<u8, Vec<String>>,
        max_level: u8,
    ) -> Result<Vec<ZeroCopyCompactionStats>> {
        let mut all_stats = Vec::new();

        for level in 0..max_level {
            if let Some(files) = level_files.get(&level) {
                if files.len() >= 4 {
                    // Compact when 4+ files at a level
                    let output_file = format!(
                        "level_{}_compacted_{}.sstable",
                        level + 1,
                        chrono::Utc::now().timestamp_millis()
                    );

                    let stats = self
                        .compact_files(
                            files.clone(),
                            output_file,
                            level + 1,
                            None, // TODO: Pass compression config here
                        )
                        .await?;

                    all_stats.push(stats);
                }
            }
        }

        Ok(all_stats)
    }

    /// Compact with size-tiered strategy
    pub async fn compact_size_tiered(
        &self,
        files_by_size: Vec<(String, u64)>,
        target_size: u64,
        compression_config: Option<crate::proto::proximadb_v1::CompressionConfig>,
    ) -> Result<Vec<ZeroCopyCompactionStats>> {
        debug!(
            "🔍 SST_COMPACTOR: compact_size_tiered with compression: {:?}",
            compression_config
                .as_ref()
                .map(|c| format!("algorithm={}, level={:?}", c.algorithm, c.level))
        );
        let mut all_stats = Vec::new();
        let mut current_batch = Vec::new();
        let mut current_size = 0u64;

        for (file_path, file_size) in files_by_size {
            current_batch.push(file_path.clone());
            current_size += file_size;

            // Compact when batch reaches target size
            if current_size >= target_size && current_batch.len() >= 2 {
                let output_file = format!(
                    "size_tiered_compacted_{}.sstable",
                    chrono::Utc::now().timestamp_millis()
                );

                let stats = self
                    .compact_files(
                        current_batch.clone(),
                        output_file,
                        0, // Size-tiered doesn't use levels
                        compression_config.clone(),
                    )
                    .await?;

                all_stats.push(stats);
                current_batch.clear();
                current_size = 0;
            }
        }

        // Compact remaining files if any
        if current_batch.len() >= 2 {
            let output_file = format!(
                "size_tiered_compacted_{}.sstable",
                chrono::Utc::now().timestamp_millis()
            );

            let stats = self
                .compact_files(current_batch, output_file, 0, compression_config.clone())
                .await?;

            all_stats.push(stats);
        }

        Ok(all_stats)
    }

    /// Retrain PCA model on merged data during compaction
    ///
    /// This ensures the collection-level PCA model reflects the full
    /// data distribution after compaction merges multiple files.
    async fn retrain_pca_model_for_compaction(
        &self,
        collection_id: &str,
        output_path: &str,
        merged_records: &[VectorRecord],
    ) {
        use super::pca_manager::{AdaptivePcaConfig, EnhancedPCAModel};

        if merged_records.is_empty() {
            return;
        }

        let vector_dim = merged_records[0].vector.len();
        if vector_dim == 0 {
            return;
        }

        // Use adaptive configuration for optimal PCA dimensions
        let pca_config = AdaptivePcaConfig::for_vector_dim(vector_dim);
        let n_components = pca_config.n_components;

        // Need at least n_components samples for training
        if merged_records.len() < n_components {
            debug!(
                "[SST Compaction] Not enough vectors ({}) for PCA training (need at least {})",
                merged_records.len(),
                n_components
            );
            return;
        }

        info!(
            "[SST Compaction] Retraining PCA model: {} vectors → {} components (from {}-dim)",
            merged_records.len(),
            n_components,
            vector_dim
        );

        // Train PCA model
        match EnhancedPCAModel::train(merged_records, n_components) {
            Ok(model) => {
                // Update global cache for search access
                super::core::set_collection_pca_model(collection_id, model.clone());

                // Also save to disk at collection path
                // Extract collection data directory from output path
                if let Some(collection_dir) = output_path.rsplit_once('/').map(|(dir, _)| dir) {
                    let model_dir = format!("{}/__model", collection_dir);
                    let model_path = format!("{}/pca_model.bin", model_dir);

                    if let Ok(fs) = self.filesystem_factory.get_filesystem(collection_dir) {
                        // Create model directory
                        let _ = futures::executor::block_on(async {
                            let _ = fs.create_dir_all(&model_dir).await;

                            if let Ok(data) = bincode::serialize(&model) {
                                if let Err(e) = fs.write(&model_path, &data, None).await {
                                    warn!(
                                        "[SST Compaction] Failed to save PCA model to disk: {}",
                                        e
                                    );
                                } else {
                                    info!(
                                        "[SST Compaction] Persisted PCA model at {} ({} components)",
                                        model_path, n_components
                                    );
                                }
                            }
                        });
                    }
                }
            }
            Err(e) => {
                warn!(
                    "[SST Compaction] Failed to train PCA model: {}. Z-Order pruning may be less effective.",
                    e
                );
            }
        }
    }
}

#[derive(Debug, Default)]
struct WriterStats {
    records_written: u64,
    bytes_written: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
    use crate::compute::quantization::storage_engine::{
        StorageQuantizationConfig, StorageQuantizationEngine,
    };
    use crate::compute::quantization::unified::{InMemoryCodebookStore, UnifiedQuantizationEngine};
    // Note: Using StorageQuantizationConfig instead of removed SstQuantizationConfig

    #[tokio::test]
    async fn test_k_way_merge_deduplication() {
        // Test that k-way merge properly deduplicates records
        // TODO: Implement test
    }

    #[tokio::test]
    async fn test_zero_copy_compaction() {
        // Test end-to-end zero-copy compaction
        // TODO: Implement test
    }

    #[tokio::test]
    async fn test_expired_record_removal() {
        // Test that expired records are filtered during compaction
        // TODO: Implement test
    }

    #[tokio::test]
    async fn test_pq_based_sorting() {
        // Initialize hardware capabilities
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

        // Create quantization infrastructure
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());
        let codebook_store = Arc::new(InMemoryCodebookStore::new());
        let unified_engine = Arc::new(UnifiedQuantizationEngine::new(
            distance_compute.clone(),
            codebook_store,
        ));

        let base_config = StorageQuantizationConfig::default();
        let _base_engine = Arc::new(StorageQuantizationEngine::new(
            unified_engine,
            distance_compute,
            base_config,
        ));

        // Note: Using StorageQuantizationConfig instead of undefined SstQuantizationConfig
        // Create storage quantization engine
        let distance_compute = Arc::new(
            crate::compute::distance_computation::engine::UnifiedDistanceCompute::default(),
        );
        let codebook_store =
            Arc::new(crate::compute::quantization::unified::InMemoryCodebookStore::new());
        let unified_engine = Arc::new(
            crate::compute::quantization::unified::UnifiedQuantizationEngine::new(
                distance_compute.clone(),
                codebook_store,
            ),
        );

        let storage_config =
            crate::compute::quantization::storage_engine::StorageQuantizationConfig {
                primary_level: Some(
                    crate::compute::quantization::unified::UnifiedQuantizationLevel::pq8(32),
                ),
                filter_level: Some(
                    crate::compute::quantization::unified::UnifiedQuantizationLevel::binary(),
                ),
                fast_level: Some(
                    crate::compute::quantization::unified::UnifiedQuantizationLevel::int8(),
                ),
                distance_metric:
                    crate::compute::distance_computation::engine::DistanceMetric::Cosine,
                enable_progressive: true,
                filter_threshold: 100.0,
                candidate_multiplier: 4,
                training_sample_size: 10000,
                memory_budget_mb: 512,
                enable_hardware_acceleration: true,
            };

        let distance_compute = Arc::new(
            crate::compute::distance_computation::engine::UnifiedDistanceCompute::new(
                crate::compute::distance_computation::engine::DistanceMetric::Cosine,
            ),
        );

        let quantization_engine = Arc::new(
            crate::compute::quantization::storage_engine::StorageQuantizationEngine::new(
                unified_engine,
                distance_compute,
                storage_config,
            ),
        );

        // Create filesystem factory
        let filesystem_factory = Arc::new(
            crate::storage::persistence::filesystem::FilesystemFactory::create(
                crate::storage::persistence::filesystem::FilesystemConfig::default(),
            )
            .await
            .unwrap(),
        );

        // Create compactor with PQ sorting
        let compactor =
            SstCompactor::new(filesystem_factory, None).with_pq_sorting(quantization_engine);

        // Verify that the sorting strategy is set correctly
        assert!(matches!(
            compactor.sort_strategy,
            CompactionSortStrategy::ByPQSimilarity(_)
        ));
    }
}
