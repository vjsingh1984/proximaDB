// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Zero-copy SST Compactor using modular block reader architecture
//! 
//! This compactor works directly with SstRecord to avoid double conversion
//! during compaction operations, significantly improving performance for
//! large-scale compactions.

use super::{SstRecord, SstableWriter};
use super::readers::unified_sstable_reader::{
    ModularBlockReader, SstDirectReader, BlockIterator, ReadMode
};
use crate::storage::persistence::filesystem::{FilesystemFactory, FileSystem};
use crate::storage::transaction_coordinator::TransactionCoordinator;
use crate::core::search::mvcc_resolution::MvccResolver;
use anyhow::{Result, Context};
use std::collections::{BinaryHeap, HashMap};
use std::cmp::{Ordering, Reverse};
use std::sync::Arc;
use tracing::{debug, info, warn, error};
use futures::stream::{Stream, StreamExt};
use async_trait::async_trait;

/// Statistics for zero-copy compaction
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
}

/// Entry in the k-way merge heap
#[derive(Debug, Clone)]
struct MergeEntry {
    record: SstRecord,
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
    /// Custom comparator function
    Custom(Arc<dyn Fn(&SstRecord, &SstRecord) -> Ordering + Send + Sync>),
}

impl std::fmt::Debug for CompactionSortStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ById => write!(f, "ById"),
            Self::ByMetadata(keys) => f.debug_tuple("ByMetadata").field(keys).finish(),
            Self::ByTimestamp => write!(f, "ByTimestamp"),
            Self::Custom(_) => write!(f, "Custom(<function>)"),
        }
    }
}

impl Default for CompactionSortStrategy {
    fn default() -> Self {
        CompactionSortStrategy::ById
    }
}

/// Zero-copy SST compactor that works directly with SstRecord
pub struct SstCompactor {
    filesystem_factory: Arc<FilesystemFactory>,
    mvcc_resolver: Option<Arc<MvccResolver>>,
    block_size: usize,
    compression_threshold: usize,
    sort_strategy: CompactionSortStrategy,
}

impl SstCompactor {
    pub fn new(
        filesystem_factory: Arc<FilesystemFactory>,
        mvcc_resolver: Option<Arc<MvccResolver>>,
    ) -> Self {
        Self {
            filesystem_factory,
            mvcc_resolver,
            block_size: 4096, // Default 4KB blocks
            compression_threshold: 1024, // Compress blocks > 1KB
            sort_strategy: CompactionSortStrategy::default(),
        }
    }
    
    /// Set the sorting strategy for compacted records
    pub fn with_sort_strategy(mut self, strategy: CompactionSortStrategy) -> Self {
        self.sort_strategy = strategy;
        self
    }

    /// Perform zero-copy k-way merge compaction of multiple SST files
    /// This method reads SstRecords directly without converting to VectorRecord
    pub async fn compact_files(
        &self,
        input_files: Vec<String>,
        output_file: String,
        target_level: u8,
    ) -> Result<ZeroCopyCompactionStats> {
        let start = std::time::Instant::now();
        let mut stats = ZeroCopyCompactionStats::default();
        stats.files_compacted = input_files.len();

        info!(
            "🚀 Starting zero-copy compaction of {} files to level {}",
            input_files.len(), target_level
        );

        // Create iterators for each input file
        let mut iterators = Vec::new();
        for (idx, file_path) in input_files.iter().enumerate() {
            debug!("Opening SST file {} for direct reading", file_path);
            
            let mut direct_reader = SstDirectReader::new(self.filesystem_factory.clone())?;
            let iterator = direct_reader.stream_sst_records(file_path.clone()).await?;
            iterators.push((idx, iterator));
        }

        // Perform k-way merge
        let merged_records = self.k_way_merge(iterators).await?;
        
        // Write merged records to output file
        let writer_stats = self.write_merged_records(
            merged_records,
            &output_file,
            target_level,
        ).await?;

        stats.records_written = writer_stats.records_written;
        stats.bytes_written = writer_stats.bytes_written;
        stats.compaction_time_ms = start.elapsed().as_millis() as u64;

        info!(
            "✅ Zero-copy compaction completed in {}ms: {} records written, {} bytes",
            stats.compaction_time_ms, stats.records_written, stats.bytes_written
        );

        Ok(stats)
    }

    /// K-way merge of multiple SST record streams with proper MVCC resolution
    /// Implements upsert semantics: keeps highest continuous version for each ID
    async fn k_way_merge(
        &self,
        mut iterators: Vec<(usize, BlockIterator<SstRecord>)>,
    ) -> Result<Vec<SstRecord>> {
        let mut heap = BinaryHeap::new();
        let mut merged_records = Vec::new();
        // Track all versions for each ID to apply MVCC rules
        let mut id_versions: HashMap<String, Vec<SstRecord>> = HashMap::new();
        let mut active_iterators: Vec<(usize, BlockIterator<SstRecord>)> = Vec::new();

        // Initialize heap with first record from each iterator
        for (file_idx, mut iter) in iterators.into_iter() {
            if let Some(record) = iter.next() {
                let rec = record?;
                heap.push(Reverse(MergeEntry {
                    timestamp: rec.timestamp,
                    record: rec,
                    file_index: file_idx,
                }));
                active_iterators.push((file_idx, iter));
            }
        }

        // Collect all records grouped by ID
        while let Some(Reverse(entry)) = heap.pop() {
            let record_id = entry.record.id.clone();
            
            // Collect all versions of each ID
            id_versions.entry(record_id.clone())
                .or_insert_with(Vec::new)
                .push(entry.record);

            // Get next record from the same file
            if let Some((_, ref mut iter)) = active_iterators.iter_mut().find(|(idx, _)| *idx == entry.file_index) {
                if let Some(next_record) = iter.next() {
                    let rec = next_record?;
                    heap.push(Reverse(MergeEntry {
                        timestamp: rec.timestamp,
                        record: rec,
                        file_index: entry.file_index,
                    }));
                }
            }
        }

        // Now apply MVCC resolution rules for each ID
        let now = chrono::Utc::now().timestamp() as u32;
        
        for (id, mut versions) in id_versions {
            // Skip if any version is a tombstone (deletion marker)
            if versions.iter().any(|r| r.is_tombstone) {
                debug!("Skipping tombstoned record: {}", id);
                continue;
            }
            
            // Skip if any version is expired
            if versions.iter().any(|r| {
                r.expires_at.map_or(false, |exp| exp < now)
            }) {
                debug!("Skipping expired record: {}", id);
                continue;
            }
            
            // Sort by version (ascending), then by timestamp (ascending for same version)
            versions.sort_by(|a, b| {
                let ver_a = a.version.unwrap_or(1);
                let ver_b = b.version.unwrap_or(1);
                
                ver_a.cmp(&ver_b)
                    .then_with(|| a.timestamp.cmp(&b.timestamp))
            });
            
            // Find the highest continuous version
            let mut expected_version = 1u32;
            let mut last_valid: Option<SstRecord> = None;
            
            for record in versions {
                let version = record.version.unwrap_or(1);
                
                if version == expected_version {
                    // This version is continuous
                    last_valid = Some(record);
                    expected_version += 1;
                } else if version > expected_version {
                    // Version gap detected - stop here
                    debug!("Version gap for ID '{}': expected {}, found {}", 
                           id, expected_version, version);
                    break;
                }
                // Skip older versions (version < expected)
            }
            
            // Add the highest continuous version to output
            if let Some(mut record) = last_valid {
                // Update the level to match the target compaction level
                record.level = self.block_size as u8; // Will be set properly by caller
                
                debug!("Selected version {} for ID '{}'", 
                       record.version.unwrap_or(1), id);
                merged_records.push(record);
            }
        }

        info!("K-way merge completed: {} records after MVCC resolution", merged_records.len());
        Ok(merged_records)
    }

    /// Write merged records to a new SST file
    async fn write_merged_records(
        &self,
        mut records: Vec<SstRecord>,
        output_path: &str,
        level: u8,
    ) -> Result<WriterStats> {
        let mut stats = WriterStats::default();
        
        // CRITICAL: Sort records based on configured strategy
        // SSTable requires records to be sorted for efficient binary search
        match &self.sort_strategy {
            CompactionSortStrategy::ById => {
                records.sort_by(|a, b| a.id.cmp(&b.id));
            }
            CompactionSortStrategy::ByTimestamp => {
                records.sort_by(|a, b| b.timestamp.cmp(&a.timestamp)); // Newest first
            }
            CompactionSortStrategy::ByMetadata(keys) => {
                records.sort_by(|a, b| {
                    // Sort by specified metadata keys in order
                    for key in keys {
                        // Find metadata items with matching key
                        let a_val = a.metadata.iter()
                            .find(|m| m.key == *key)
                            .and_then(|m| m.value.as_ref())
                            .and_then(|v| match v {
                                crate::proto::proximadb::metadata_item::Value::StringValue(s) => Some(s.as_str()),
                                _ => None,
                            });
                        
                        let b_val = b.metadata.iter()
                            .find(|m| m.key == *key)
                            .and_then(|m| m.value.as_ref())
                            .and_then(|v| match v {
                                crate::proto::proximadb::metadata_item::Value::StringValue(s) => Some(s.as_str()),
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
            CompactionSortStrategy::Custom(comparator) => {
                records.sort_by(|a, b| comparator(a, b));
            }
        }
        
        info!(
            "📊 Writing {} sorted records to SST file at level {}",
            records.len(), level
        );
        
        // Create SSTable writer
        let writer = SstableWriter::new(
            output_path,
            self.block_size,
            self.filesystem_factory.clone(),
        );

        // Convert records to sorted format (id, record)
        let sorted_records: Vec<(String, SstRecord)> = records
            .into_iter()
            .map(|r| {
                let id = r.id.clone();
                stats.records_written += 1;
                stats.bytes_written += bincode::serialized_size(&r).unwrap_or(0);
                (id, r)
            })
            .collect();

        let record_count = sorted_records.len();
        
        // Write using the streaming API
        writer.write_sorted_records(
            sorted_records.into_iter(),
            record_count,
        ).await?;

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
                if files.len() >= 4 { // Compact when 4+ files at a level
                    let output_file = format!("level_{}_compacted_{}.sst", 
                        level + 1, chrono::Utc::now().timestamp_millis());
                    
                    let stats = self.compact_files(
                        files.clone(),
                        output_file,
                        level + 1,
                    ).await?;
                    
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
    ) -> Result<Vec<ZeroCopyCompactionStats>> {
        let mut all_stats = Vec::new();
        let mut current_batch = Vec::new();
        let mut current_size = 0u64;

        for (file_path, file_size) in files_by_size {
            current_batch.push(file_path.clone());
            current_size += file_size;

            // Compact when batch reaches target size
            if current_size >= target_size && current_batch.len() >= 2 {
                let output_file = format!("size_tiered_compacted_{}.sst", 
                    chrono::Utc::now().timestamp_millis());
                
                let stats = self.compact_files(
                    current_batch.clone(),
                    output_file,
                    0, // Size-tiered doesn't use levels
                ).await?;
                
                all_stats.push(stats);
                current_batch.clear();
                current_size = 0;
            }
        }

        // Compact remaining files if any
        if current_batch.len() >= 2 {
            let output_file = format!("size_tiered_compacted_{}.sst", 
                chrono::Utc::now().timestamp_millis());
            
            let stats = self.compact_files(
                current_batch,
                output_file,
                0,
            ).await?;
            
            all_stats.push(stats);
        }

        Ok(all_stats)
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
}