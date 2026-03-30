//! # Global Partitioned Memtable - High-Performance In-Memory Vector Storage
//!
//! This module implements ProximaDB's global partitioned memtable, a critical component that
//! serves as the primary in-memory buffer between the WAL and storage engines. It provides
//! high-speed vector access with collection-level isolation and efficient batch management.
//!
//! ## Architecture Overview
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                   Global Partitioned Memtable                │
//! ├─────────────────────────────────────────────────────────────┤
//! │  Global Sequence Counter (Atomic)                           │
//! │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐     │
//! │  │ Collection A │  │ Collection B │  │ Collection C │ ... │
//! │  │  Partition   │  │  Partition   │  │  Partition   │     │
//! │  └──────────────┘  └──────────────┘  └──────────────┘     │
//! │        ↓                 ↓                 ↓               │
//! │  ┌──────────────────────────────────────────────┐         │
//! │  │ WAL Batches (HashMap<BatchId, WALVectorBatch>)│         │
//! │  │ Vector ID Index (HashMap<VectorId, BatchId>)  │         │
//! │  │ Bloom Filters (Per-batch metadata filtering)  │         │
//! │  └──────────────────────────────────────────────┘         │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Key Design Decisions
//!
//! ### 1. Collection Partitioning
//! - **Why**: Isolates collections for independent flush/compaction operations
//! - **How**: Each collection gets its own partition with dedicated data structures
//! - **Benefit**: No cross-collection interference, parallel operations possible
//!
//! ### 2. WAL Batch Storage
//! - **Why**: Preserves batch boundaries for efficient group operations
//! - **How**: Stores complete WALVectorBatch objects instead of individual vectors
//! - **Benefit**: Atomic batch operations, better memory locality, efficient flush
//!
//! ### 3. Dual Indexing Strategy
//! - **Primary**: WAL batches stored by batch ID for sequential access
//! - **Secondary**: Vector ID to batch ID mapping for O(1) lookups
//! - **Benefit**: Fast both for batch operations and individual vector retrieval
//!
//! ### 4. Global Sequence Counter
//! - **Why**: Ensures total ordering across all collections for consistency
//! - **How**: Atomic counter incremented for each operation
//! - **Benefit**: MVCC support, consistent snapshots, ordered recovery
//!
//! ## Performance Characteristics
//!
//! - **Insert**: O(1) amortized for batch operations
//! - **Get by ID**: O(1) via vector ID index
//! - **Search**: O(n) but optimized with bloom filters and parallel search
//! - **Flush**: O(n) streaming with minimal memory overhead
//! - **Memory**: ~100-200 bytes overhead per vector
//!
//! ## Concurrency Model
//!
//! - **Read-Write Lock**: Collection-level RwLock for concurrent reads
//! - **Atomic Sequence**: Lock-free sequence generation
//! - **Copy-on-Write**: Vector records are cloned on read (immutable)
//! - **Batch Atomicity**: Entire batches succeed or fail together
//!
//! ## Integration Points
//!
//! - **WAL System**: Primary consumer, stores WAL batches directly
//! - **Storage Engines**: Provides vectors during flush operations
//! - **Query Layer**: Serves vectors for search before flush
//! - **AXIS Indexing**: Supplies vectors for index building

use anyhow::Result;
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::RwLock;
use tracing::debug;

use super::super::core::MemtableMetrics;
use crate::compute::distance_computation::DistanceMetric as CoreDistanceMetric;
use crate::compute::distance_computation::engine::{
    DistanceComputeProvider, SimilarityResult, UnifiedDistanceCompute,
};
use crate::proto::proximadb_v1::VectorRecord;

/// Collection partition within the global memtable
///
/// Each collection gets its own isolated partition to enable:
/// - Independent flush operations without blocking other collections
/// - Collection-specific memory limits and eviction policies
/// - Efficient collection deletion (drop entire partition)
/// - Parallel search within collection boundaries
#[derive(Debug)]
struct CollectionPartition {
    /// WAL Batches stored as native deserialized batches (PRIMARY STORAGE)
    ///
    /// Key design: We store complete WALVectorBatch objects rather than individual
    /// vectors to preserve batch atomicity and enable efficient group operations.
    /// The batch ID is globally unique (CompactBatchId) ensuring no collisions.
    wal_batches:
        HashMap<String, crate::storage::memtable::specialized::wal_behavior::WALVectorBatch>,

    /// Vector ID to batch lookup index for fast get operations
    ///
    /// Secondary index mapping vector IDs to their containing batch.
    /// This enables O(1) retrieval of individual vectors by ID.
    /// Note: Only vectors with client-provided IDs are indexed.
    vector_id_index: HashMap<String, String>, // vector_id -> batch_id

    /// Collection statistics for monitoring and management
    total_size: usize, // Total bytes consumed by all batches
    vector_count: usize, // Total number of vectors across all batches
    batch_count: usize,  // Number of batches in this partition
    #[allow(dead_code)]
    last_flush_sequence: u64, // Sequence number of last successful flush
    #[allow(dead_code)]
    timestamp: std::time::SystemTime, // Last modification time
    #[allow(dead_code)]
    created_at: std::time::SystemTime, // Partition creation time
}

impl CollectionPartition {
    fn new() -> Self {
        Self {
            wal_batches: HashMap::new(),
            vector_id_index: HashMap::new(),
            total_size: 0,
            vector_count: 0,
            batch_count: 0,
            last_flush_sequence: 0,
            timestamp: std::time::SystemTime::now(),
            created_at: std::time::SystemTime::now(),
        }
    }

    /// Add WAL batch to this collection partition - CRITICAL HOT PATH
    ///
    /// This is one of the most performance-critical functions in the system as it's
    /// called for every batch insert. Optimizations:
    /// - Inline always for hot path optimization
    /// - Lazy bloom filter creation (only when needed)
    /// - Batch-level operations to amortize costs
    /// - Pre-allocated index updates
    ///
    /// # Arguments
    /// * `batch` - Complete WAL batch containing vectors and metadata
    ///
    /// # Returns
    /// * `Ok(())` - Batch successfully added
    /// * `Err` - Failed to create bloom filter (non-fatal, logged as warning)
    #[inline(always)]
    fn add_batch(
        &mut self,
        mut batch: crate::storage::memtable::specialized::wal_behavior::WALVectorBatch,
    ) -> Result<()> {
        let batch_id = batch.batch_id.to_base62();
        let batch_size = batch.total_size_bytes;
        let vector_count = batch.vector_records.len();

        // Create bloom filter for this batch if not already present
        // Bloom filters enable fast metadata filtering without scanning all vectors.
        // We use a 1% false positive rate as a good balance between memory and accuracy.
        // The filter is created lazily here rather than during batch creation to
        // avoid overhead when metadata filtering isn't used.
        if batch.metadata_bloom_filter.is_none() {
            match batch.create_bloom_filter() {
                Ok(_) => {
                    tracing::debug!(
                        "✅ Created bloom filter for batch {} with {} vectors",
                        batch_id,
                        vector_count
                    );
                }
                Err(e) => {
                    // Non-fatal: System works without bloom filters, just slower filtering
                    tracing::warn!(
                        "⚠️ Failed to create bloom filter for batch {}: {}",
                        batch_id,
                        e
                    );
                }
            }
        }

        // Update vector ID index for fast lookups
        // Only index vectors with client-provided IDs (non-empty).
        // This secondary index enables O(1) retrieval by vector ID.
        // Trade-off: Extra memory for index vs fast lookups.
        for vector_record in batch.vector_records.iter() {
            if !vector_record.id.is_empty() {
                // Clone is necessary as we need owned strings in the index
                self.vector_id_index
                    .insert(vector_record.id.clone(), batch_id.clone());
            }
        }

        // Store the batch
        self.wal_batches.insert(batch_id, batch);

        // Update statistics
        self.total_size += batch_size;
        self.vector_count += vector_count;
        self.batch_count += 1;

        Ok(())
    }

    /// Get vector by ID within this collection with MVCC + logical delete support
    ///
    /// Implements Multi-Version Concurrency Control (MVCC) by:
    /// 1. Finding all versions of a vector across batches
    /// 2. Selecting the latest version based on version number and timestamp
    /// 3. Checking TTL expiration for temporal validity
    /// 4. Verifying the vector isn't logically deleted
    ///
    /// # MVCC Resolution Strategy
    /// - Primary: Higher version number wins
    /// - Secondary: If versions equal, newer timestamp wins
    /// - Tertiary: Check TTL expiration
    ///
    /// # Returns
    /// - `Some(VectorRecord)` - Latest valid version of the vector
    /// - `None` - Vector not found, expired, or deleted
    fn vector_by_id(&self, vector_id: &str) -> Option<VectorRecord> {
        // Skip if no ID provided (immutable vectors don't have IDs)
        if vector_id.is_empty() {
            return None;
        }

        let current_time = chrono::Utc::now().timestamp_micros();
        let mut latest_record: Option<(VectorRecord, u64, Option<u32>)> = None; // (record, sequence, version)

        // Search through all batches to find the latest version
        for batch in self.wal_batches.values() {
            for vector_record in batch.vector_records.iter() {
                if !vector_record.id.is_empty() && vector_record.id == vector_id {
                    let sequence = batch
                        .timestamp
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|duration| duration.as_millis() as u64)
                        .unwrap_or(0);
                    let version = vector_record.version;

                    // Check if this is a newer version (prioritize version number over timestamp)
                    let is_newer = match &latest_record {
                        Some((_, existing_seq, existing_version)) => {
                            // Primary: Compare by version number (higher version wins)
                            match (version, existing_version) {
                                (Some(v), Some(ev)) => {
                                    v > *ev || (v == *ev && sequence > *existing_seq)
                                }
                                (Some(_), None) => true, // Some version beats None
                                (None, Some(_)) => false, // None loses to Some version
                                (None, None) => sequence > *existing_seq, // Both None, use sequence
                            }
                        }
                        None => true, // First occurrence
                    };

                    if is_newer {
                        latest_record = Some((vector_record.clone(), sequence, version));
                    }
                }
            }
        }

        // Check the latest record we found
        if let Some((record, _, _)) = latest_record {
            // Check if it's expired (logical delete) - convert current_time to seconds
            let current_time_secs = current_time / 1_000_000; // Convert microseconds to seconds
            let is_expired = record.expires_at.map(|expires| expires < current_time_secs);

            if is_expired.unwrap_or(false) {
                tracing::debug!("🗑️ Vector {} found but expired (tombstone)", vector_id);
                return None; // Logically deleted
            }

            return Some(record);
        }

        None
    }

    /// Clear batches up to sequence number within this collection
    fn clear_flushed(&mut self) -> usize {
        let mut cleared_count = 0;
        let mut removed_size = 0;

        // Find batches to remove that are marked as flushed
        let batch_ids_to_remove: Vec<String> = self
            .wal_batches
            .iter()
            .filter(|(_, batch)| batch.is_flushed)
            .map(|(id, _)| id.clone())
            .collect();

        for batch_id in batch_ids_to_remove {
            if let Some(batch) = self.wal_batches.remove(&batch_id) {
                // Remove vector IDs from index
                for vector_record in batch.vector_records.iter() {
                    if !vector_record.id.is_empty() {
                        self.vector_id_index.remove(&vector_record.id);
                    }
                }

                cleared_count += batch.vector_records.len();
                removed_size += batch.total_size_bytes;
                self.batch_count = self.batch_count.saturating_sub(1);
            }
        }

        self.vector_count = self.vector_count.saturating_sub(cleared_count);
        self.total_size = self.total_size.saturating_sub(removed_size);
        // No longer tracking sequences

        cleared_count
    }

    /// Check if this collection needs flushing
    fn needs_flush(&self, size_threshold: usize, count_threshold: usize) -> bool {
        self.total_size >= size_threshold || self.vector_count >= count_threshold
    }

    /// Get all vectors for iteration or flush operations with MVCC + logical delete support
    fn get_all_vectors(&self) -> Vec<VectorRecord> {
        use std::collections::HashMap;

        let mut id_to_latest: HashMap<String, (VectorRecord, u64, Option<u32>)> = HashMap::new(); // (record, sequence, version)
        let mut vectors_without_id = Vec::new();
        let current_time = chrono::Utc::now().timestamp_micros();

        // Collect latest versions for each ID
        for batch in self.wal_batches.values() {
            for vector_record in batch.vector_records.iter() {
                let sequence = batch
                    .timestamp
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_millis() as u64)
                    .unwrap_or(0);
                let version = vector_record.version;

                if !vector_record.id.is_empty() {
                    let vector_id = &vector_record.id;
                    // Check if this is the latest version (prioritize version number over timestamp)
                    let is_newer = match id_to_latest.get(vector_id) {
                        Some((_, existing_seq, existing_version)) => {
                            // Primary: Compare by version number (higher version wins)
                            match (version, existing_version) {
                                (Some(v), Some(ev)) => {
                                    v > *ev || (v == *ev && sequence > *existing_seq)
                                }
                                (Some(_), None) => true, // Some version beats None
                                (None, Some(_)) => false, // None loses to Some version
                                (None, None) => sequence > *existing_seq, // Both None, use sequence
                            }
                        }
                        None => true,
                    };

                    if is_newer {
                        id_to_latest.insert(
                            vector_record.id.clone(),
                            (vector_record.clone(), sequence, version),
                        );
                    }
                } else {
                    // No ID - include directly if not expired
                    let current_time_secs = current_time / 1_000_000; // Convert microseconds to seconds
                    let is_expired = vector_record
                        .expires_at
                        .map(|expires| expires < current_time_secs);

                    if !is_expired.unwrap_or(false) {
                        vectors_without_id.push(vector_record.clone());
                    }
                }
            }
        }

        // Collect final results, filtering out expired records
        let mut vectors = Vec::new();

        for (_, (record, _, _)) in id_to_latest {
            let current_time_secs = current_time / 1_000_000; // Convert microseconds to seconds
            let is_expired = record.expires_at.map(|expires| expires < current_time_secs);

            if !is_expired.unwrap_or(false) {
                vectors.push(record);
            }
        }

        vectors.extend(vectors_without_id);
        vectors
    }

    /// Search for similar vectors using native batch processing with MVCC + logical deletes
    fn search_vectors(
        &self,
        query_vector: &[f32],
        distance_metric: &CoreDistanceMetric,
        distance_compute: &UnifiedDistanceCompute,
    ) -> Vec<(SimilarityResult, VectorRecord)> {
        self.search_vectors_with_filter(query_vector, distance_metric, distance_compute, None)
    }

    /// Search for similar vectors with optional metadata filter
    fn search_vectors_with_filter(
        &self,
        query_vector: &[f32],
        distance_metric: &CoreDistanceMetric,
        distance_compute: &UnifiedDistanceCompute,
        metadata_filter: Option<&HashMap<String, String>>,
    ) -> Vec<(SimilarityResult, VectorRecord)> {
        use std::collections::HashMap;

        let mut id_to_latest: HashMap<String, (SimilarityResult, VectorRecord, u64, Option<u32>)> =
            HashMap::new(); // (score, record, sequence, version)
        let mut results_without_id: Vec<(SimilarityResult, VectorRecord)> = Vec::new();
        let current_time = chrono::Utc::now().timestamp_micros();

        let mut batches_checked = 0;
        let mut batches_skipped = 0;

        // First pass: Find latest version of each ID by sequence and version (MVCC)
        for (batch_id, wal_batch) in &self.wal_batches {
            batches_checked += 1;

            // Use bloom filter to quickly skip irrelevant batches
            if let Some(filter) = metadata_filter {
                let mut might_contain = false;

                for (key, value) in filter {
                    if wal_batch.might_contain_metadata_value(key, value) {
                        might_contain = true;
                        break;
                    }
                }

                if !might_contain {
                    batches_skipped += 1;
                    tracing::debug!(
                        "⚡ Bloom filter: Skipping batch {} (no matching metadata)",
                        batch_id
                    );
                    continue;
                }
            }

            tracing::debug!(
                "🔍 Processing WAL batch {} with {} vectors",
                batch_id,
                wal_batch.vector_records.len()
            );

            for vector_record in wal_batch.vector_records.iter() {
                let sequence = wal_batch
                    .timestamp
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_millis() as u64)
                    .unwrap_or(0);
                let version = vector_record.version;

                if !vector_record.id.is_empty() {
                    let vector_id = &vector_record.id;
                    // Check if this is the latest version (prioritize version number over timestamp)
                    let is_newer = match id_to_latest.get(vector_id) {
                        Some((_, _, existing_seq, existing_version)) => {
                            // Primary: Compare by version number (higher version wins)
                            match (version, existing_version) {
                                (Some(v), Some(ev)) => {
                                    v > *ev || (v == *ev && sequence > *existing_seq)
                                }
                                (Some(_), None) => true, // Some version beats None
                                (None, Some(_)) => false, // None loses to Some version
                                (None, None) => sequence > *existing_seq, // Both None, use sequence
                            }
                        }
                        None => true,
                    };

                    if is_newer {
                        // Skip tombstones (empty vector + expires_at in past or 0) - they mark deletions
                        // and should not be included in search results
                        let current_time_secs = current_time / 1_000_000;
                        let is_tombstone = vector_record.vector.is_empty()
                            && vector_record
                                .expires_at
                                .map_or(false, |e| e <= current_time_secs);
                        if is_tombstone {
                            // Remove any previous version from results (tombstone shadows it)
                            id_to_latest.remove(vector_id);
                            tracing::debug!(
                                "🗑️ Tombstone found for ID {}: removing from results",
                                vector_id
                            );
                        } else {
                            let score = distance_compute.calculate_distance(
                                query_vector,
                                &vector_record.vector,
                                distance_metric,
                            );
                            id_to_latest.insert(
                                vector_record.id.clone(),
                                (score, vector_record.clone(), sequence, version),
                            );

                            tracing::debug!(
                                "📝 Updated latest version for ID {}: seq={}, version={:?}",
                                &vector_record.id,
                                sequence,
                                version
                            );
                        }
                    }
                } else {
                    // No ID - include directly (no MVCC possible), but check expiry
                    // Also skip empty vectors (should not happen for valid data, but safety check)
                    if vector_record.vector.is_empty() {
                        continue;
                    }

                    let current_time_secs = current_time / 1_000_000; // Convert microseconds to seconds
                    let is_expired = vector_record
                        .expires_at
                        .map(|expires| expires < current_time_secs);

                    if !is_expired.unwrap_or(false) {
                        let score = distance_compute.calculate_distance(
                            query_vector,
                            &vector_record.vector,
                            distance_metric,
                        );
                        results_without_id.push((score, vector_record.clone()));
                    }
                }
            }
        }

        // Second pass: Filter out expired records (tombstones) from latest versions
        let mut final_results: Vec<(SimilarityResult, VectorRecord)> = Vec::new();
        let mut filtered_count = 0;
        let latest_versions_count = id_to_latest.len();

        for (id, (score, vector_record, _, _)) in id_to_latest {
            let current_time_secs = current_time / 1_000_000; // Convert microseconds to seconds
            let is_expired = vector_record
                .expires_at
                .map(|expires| expires < current_time_secs);

            if is_expired.unwrap_or(false) {
                tracing::debug!("🗑️ Filtering out expired latest version for ID {}", id);
                filtered_count += 1;
            } else {
                final_results.push((score, vector_record));
            }
        }

        // Add non-ID results
        let results_without_id_count = results_without_id.len();
        final_results.extend(results_without_id);

        if metadata_filter.is_some() {
            let skip_percentage = (batches_skipped as f64 / batches_checked as f64) * 100.0;
            tracing::info!(
                "⚡ Bloom filter efficiency: Skipped {}/{} batches ({:.1}%) using bloom filters",
                batches_skipped,
                batches_checked,
                skip_percentage
            );
        }

        tracing::debug!(
            "🔍 Search results: {} batches searched, {} latest versions found, {} expired filtered, {} without ID, {} final results",
            batches_checked - batches_skipped,
            latest_versions_count,
            filtered_count,
            results_without_id_count,
            final_results.len()
        );

        final_results
    }
}

/// Global partitioned memtable implementation for WAL operations
///
/// This implements a two-tier index structure:
/// 1. Global sequence ordering for flush coordination
/// 2. Per-collection data partitions with batch-based storage
#[derive(Debug)]
pub struct GlobalPartitionedMemtable {
    /// Global sequence generator for cross-collection ordering
    global_sequence: AtomicU64,

    /// Per-collection data partitions (collection_id -> partition)
    collections: Arc<RwLock<HashMap<String, CollectionPartition>>>,

    /// Unified distance computation manager
    distance_compute: UnifiedDistanceCompute,

    /// Global metrics
    metrics: Arc<RwLock<MemtableMetrics>>,
}

impl GlobalPartitionedMemtable {
    /// Create new global partitioned memtable
    pub fn new() -> Self {
        Self {
            global_sequence: AtomicU64::new(1),
            collections: Arc::new(RwLock::new(HashMap::new())),
            distance_compute: UnifiedDistanceCompute::default(),
            metrics: Arc::new(RwLock::new(MemtableMetrics::default())),
        }
    }

    /// Add native WAL batch to the appropriate collection partition - CRITICAL HOT PATH
    /// STREAMLINED ARCHITECTURE with optimized atomic operations
    #[inline(always)]
    pub async fn add_wal_batch(
        &self,
        collection_id: &str,
        wal_batch: crate::storage::memtable::specialized::wal_behavior::WALVectorBatch,
    ) -> Result<Vec<u64>> {
        let batch_id = wal_batch.batch_id.to_base62();
        let vector_count = wal_batch.vector_records.len();
        let batch_size = wal_batch.total_size_bytes;

        tracing::debug!(
            "🚀 NATIVE_BATCH_ADD: Adding WAL batch {} to collection {} ({} vectors, {} bytes)",
            batch_id,
            collection_id,
            vector_count,
            batch_size
        );

        // Generate global sequences for the batch
        let start_seq = self.global_sequence.load(Ordering::SeqCst);
        let sequences: Vec<u64> = (start_seq..start_seq + vector_count as u64).collect();
        self.global_sequence
            .store(start_seq + vector_count as u64, Ordering::SeqCst);

        // Get or create collection partition
        let mut collections = self.collections.write().await;
        let partition_exists = collections.contains_key(collection_id);
        let partition = collections
            .entry(collection_id.to_string())
            .or_insert_with(CollectionPartition::new);

        tracing::debug!(
            "🚀 GLOBAL_PARTITIONED_DEBUG: Adding batch to partition for collection {}, partition existed: {}",
            collection_id,
            partition_exists
        );

        // Store the batch natively in the partition
        partition.add_batch(wal_batch)?;

        tracing::debug!(
            "🚀 GLOBAL_PARTITIONED_DEBUG: Updated partition stats - vector_count: {}, total_size: {}",
            partition.vector_count,
            partition.total_size
        );

        drop(collections);

        // Update global metrics
        let mut metrics = self.metrics.write().await;
        metrics.insert_count += 1; // One batch insert
        metrics.entry_count += vector_count; // Multiple vectors
        metrics.size_bytes += batch_size;

        debug!(
            "Batch added: {} (collection={}, vectors={}, bytes={})",
            batch_id, collection_id, vector_count, batch_size
        );

        Ok(sequences)
    }

    /// Get any vector from the memtable (useful for testing/debugging)
    pub async fn get_any_vector(&self) -> Result<Option<VectorRecord>> {
        let collections = self.collections.read().await;

        // Linear search through all collections (could be optimized with sequence->collection mapping)
        for partition in collections.values() {
            // Search through native WAL batches
            for batch in partition.wal_batches.values() {
                // With CompactBatchId, we don't track individual sequences
                // Just return the first vector as a placeholder
                // TODO: Implement proper sequence tracking if needed
                if let Some(vector) = batch.vector_records.first() {
                    return Ok(Some(vector.clone()));
                }
            }
        }

        Ok(None)
    }

    /// Search for similar vectors within a specific collection using configurable distance metric
    pub async fn search_vectors(
        &self,
        query_vector: &[f32],
        k: usize,
        collection_id: &str,
        distance_metric: CoreDistanceMetric,
    ) -> Result<Vec<(SimilarityResult, VectorRecord)>> {
        let collections = self.collections.read().await;

        debug!(
            "🔍 GLOBAL_PARTITIONED_SEARCH: Searching for collection_id '{}' in {} collections",
            collection_id,
            collections.len()
        );
        for (id, partition) in collections.iter() {
            debug!(
                "🔍 Available collection: '{}' with {} vectors",
                id, partition.vector_count
            );
        }

        if let Some(partition) = collections.get(collection_id) {
            let mut results =
                partition.search_vectors(query_vector, &distance_metric, &self.distance_compute);

            // Sort by rank_value (lower = better) and limit to k
            results.sort_by(|a, b| {
                a.0.rank_value
                    .partial_cmp(&b.0.rank_value)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            results.truncate(k);

            tracing::debug!(
                "📊 GLOBAL_PARTITIONED_SEARCH: Found {} results in collection {} (partition has {} vectors) using {:?}",
                results.len(),
                collection_id,
                partition.vector_count,
                distance_metric
            );
            Ok(results)
        } else {
            tracing::debug!(
                "📊 GLOBAL_PARTITIONED_SEARCH: Collection {} not found",
                collection_id
            );
            Ok(Vec::new())
        }
    }

    /// Get vector by ID within a specific collection (MODERN - no deserialization)
    pub async fn vector_by_id(
        &self,
        collection_id: &str,
        vector_id: &str,
    ) -> Result<Option<VectorRecord>> {
        let collections = self.collections.read().await;

        if let Some(partition) = collections.get(collection_id) {
            Ok(partition.vector_by_id(vector_id))
        } else {
            Ok(None)
        }
    }

    /// Get all vectors for a specific collection (MODERN - returns VectorRecord directly)
    pub async fn get_collection_vectors(&self, collection_id: &str) -> Result<Vec<VectorRecord>> {
        let collections = self.collections.read().await;

        if let Some(partition) = collections.get(collection_id) {
            let vectors = partition.get_all_vectors();

            tracing::debug!(
                "🚀 COLLECTION_VECTORS: Returning {} vectors from {} native batches",
                vectors.len(),
                partition.wal_batches.len()
            );

            Ok(vectors)
        } else {
            Ok(Vec::new())
        }
    }

    /// Get collection statistics
    pub async fn get_collection_stats(&self, collection_id: &str) -> (usize, usize) {
        let collections = self.collections.read().await;

        if let Some(partition) = collections.get(collection_id) {
            (partition.vector_count, partition.total_size)
        } else {
            (0, 0)
        }
    }

    /// Clear entries for a specific collection up to sequence number
    pub async fn clear_flushed_batches(&self, collection_id: &str) -> Result<usize> {
        let mut collections = self.collections.write().await;

        if let Some(partition) = collections.get_mut(collection_id) {
            let cleared_count = partition.clear_flushed();

            // Update global metrics
            let mut metrics = self.metrics.write().await;
            metrics.entry_count = metrics.entry_count.saturating_sub(cleared_count);
            // Note: size_bytes will be recalculated in next size_bytes() call

            tracing::debug!(
                "📊 GLOBAL_PARTITIONED: Cleared {} flushed entries from collection {}",
                cleared_count,
                collection_id
            );

            Ok(cleared_count)
        } else {
            Ok(0)
        }
    }

    /// Get collections that need flushing based on thresholds
    pub async fn collections_needing_flush(&self, size_threshold: usize) -> Result<Vec<String>> {
        let collections = self.collections.read().await;
        let count_threshold = 10000; // Could be configurable

        let mut collections_to_flush = Vec::new();
        let mut total_collections = 0;

        for (collection_id, partition) in collections.iter() {
            total_collections += 1;
            if partition.needs_flush(size_threshold, count_threshold) {
                collections_to_flush.push(collection_id.clone());
                tracing::debug!(
                    "📊 GLOBAL_PARTITIONED: Collection {} needs flush - {} entries, {} bytes",
                    collection_id,
                    partition.vector_count,
                    partition.total_size
                );
            }
        }

        tracing::debug!(
            "📊 GLOBAL_PARTITIONED: {} of {} collections need flushing",
            collections_to_flush.len(),
            total_collections
        );

        Ok(collections_to_flush)
    }

    /// Get collections intelligently selected for global flush based on strategy
    pub async fn get_intelligent_flush_collections(
        &self,
        global_threshold: usize,
        shrink_factor: f64,
        max_collections: Option<usize>,
    ) -> Result<Vec<CollectionFlushInfo>> {
        let collections = self.collections.read().await;
        let current_total_size = collections.values().map(|p| p.total_size).sum::<usize>();

        // If we're under global threshold, no flush needed
        if current_total_size <= global_threshold {
            return Ok(Vec::new());
        }

        // Calculate target size after shrinking
        let target_size = (global_threshold as f64 * shrink_factor) as usize;
        let reduction_needed = current_total_size.saturating_sub(target_size);

        tracing::info!(
            "🧠 INTELLIGENT_FLUSH: Current={} bytes, Global threshold={} bytes, Target={} bytes, Reduction needed={} bytes",
            current_total_size,
            global_threshold,
            target_size,
            reduction_needed
        );

        // Create collection info with flush priority
        let mut collection_infos: Vec<CollectionFlushInfo> = collections
            .iter()
            .map(|(collection_id, partition)| {
                let efficiency_score = calculate_flush_efficiency_score(
                    partition.total_size,
                    partition.vector_count,
                    partition.batch_count,
                );

                CollectionFlushInfo {
                    collection_id: collection_id.clone(),
                    total_size: partition.total_size,
                    vector_count: partition.vector_count,
                    batch_count: partition.batch_count,
                    efficiency_score,
                    age_score: calculate_age_score(partition.created_at),
                }
            })
            .collect();

        // Sort by intelligent selection criteria (largest collections first, then by efficiency)
        collection_infos.sort_by(|a, b| {
            // Primary: Size (largest first)
            let size_cmp = b.total_size.cmp(&a.total_size);
            if size_cmp != std::cmp::Ordering::Equal {
                return size_cmp;
            }

            // Secondary: Efficiency score (highest first)
            let efficiency_cmp = b.efficiency_score.partial_cmp(&a.efficiency_score);
            if let Some(cmp) = efficiency_cmp
                && cmp != std::cmp::Ordering::Equal {
                    return cmp;
                }

            // Tertiary: Age score (oldest first)
            a.age_score
                .partial_cmp(&b.age_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Select collections until we meet reduction target or max_collections limit
        let mut selected_collections = Vec::new();
        let mut total_reduction = 0;
        let max_to_select = max_collections;

        for collection_info in collection_infos
            .into_iter()
            .take(max_to_select.unwrap_or(usize::MAX))
        {
            selected_collections.push(collection_info.clone());
            total_reduction += collection_info.total_size;

            tracing::debug!(
                "🎯 INTELLIGENT_FLUSH: Selected collection {} ({} bytes, efficiency={:.2})",
                collection_info.collection_id,
                collection_info.total_size,
                collection_info.efficiency_score
            );

            // Stop if we've achieved sufficient reduction
            if total_reduction >= reduction_needed {
                break;
            }
        }

        tracing::info!(
            "🎯 INTELLIGENT_FLUSH: Selected {} collections for flush, total reduction={} bytes ({:.1}% of target)",
            selected_collections.len(),
            total_reduction,
            (total_reduction as f64 / reduction_needed as f64) * 100.0
        );

        Ok(selected_collections)
    }

    /// Get collections for emergency flush (when many small collections cause global explosion)
    pub async fn get_emergency_flush_collections(
        &self,
        global_threshold: usize,
        small_collection_threshold: usize,
    ) -> Result<Vec<CollectionFlushInfo>> {
        let collections = self.collections.read().await;
        let current_total_size = collections.values().map(|p| p.total_size).sum::<usize>();

        // Only handle emergency case when global threshold is exceeded
        if current_total_size <= global_threshold {
            return Ok(Vec::new());
        }

        // Identify small collections (under threshold but collectively causing issues)
        let mut small_collections: Vec<CollectionFlushInfo> = collections
            .iter()
            .filter(|(_, partition)| {
                partition.total_size < small_collection_threshold && partition.total_size > 0
            })
            .map(|(collection_id, partition)| CollectionFlushInfo {
                collection_id: collection_id.clone(),
                total_size: partition.total_size,
                vector_count: partition.vector_count,
                batch_count: partition.batch_count,
                efficiency_score: calculate_flush_efficiency_score(
                    partition.total_size,
                    partition.vector_count,
                    partition.batch_count,
                ),
                age_score: calculate_age_score(partition.created_at),
            })
            .collect();

        // Sort small collections by age (oldest first) to handle long-lived small collections
        small_collections.sort_by(|a, b| {
            a.age_score
                .partial_cmp(&b.age_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let small_collections_count = small_collections.len();
        let small_collections_total_size: usize =
            small_collections.iter().map(|c| c.total_size).sum();

        tracing::warn!(
            "🚨 EMERGENCY_FLUSH: {} small collections ({} bytes total) contributing to global threshold exceeded",
            small_collections_count,
            small_collections_total_size
        );

        // In emergency case, select up to 25% of small collections for flush
        let max_emergency_flush = (small_collections_count / 4).max(1);
        let selected_emergency: Vec<CollectionFlushInfo> = small_collections
            .into_iter()
            .take(max_emergency_flush)
            .collect();

        tracing::info!(
            "🚨 EMERGENCY_FLUSH: Selected {} small collections for emergency flush",
            selected_emergency.len()
        );

        Ok(selected_emergency)
    }

    /// Get metrics for external access
    pub async fn get_metrics(&self) -> MemtableMetrics {
        self.metrics.read().await.clone()
    }

    /// Update metrics externally (for specialized behavior wrappers)
    pub async fn update_metrics<F>(&self, updater: F) -> Result<()>
    where
        F: FnOnce(&mut MemtableMetrics),
    {
        let mut metrics = self.metrics.write().await;
        updater(&mut *metrics);
        Ok(())
    }

    /// Get vectors from sequence number onwards (for recovery) - MODERN
    pub async fn get_all_vectors(&self, limit: Option<usize>) -> Result<Vec<(u64, VectorRecord)>> {
        let collections = self.collections.read().await;
        let mut all_vectors = Vec::new();

        // Collect all vectors from all collections with their sequences
        for partition in collections.values() {
            for batch in partition.wal_batches.values() {
                for (index, vector_record) in batch.vector_records.iter().enumerate() {
                    // With CompactBatchId, we use index as pseudo-sequence
                    let vector_sequence = index as u64;
                    all_vectors.push((vector_sequence, vector_record.clone()));
                }
            }
        }

        // Sort by global sequence
        all_vectors.sort_by_key(|(seq, _)| *seq);

        // Apply limit if specified
        if let Some(limit) = limit {
            all_vectors.truncate(limit);
        }

        Ok(all_vectors)
    }

    /// Clear entries up to sequence number (global operation)
    pub async fn clear_all_flushed(&self) -> Result<usize> {
        let mut collections = self.collections.write().await;
        let mut total_cleared = 0;

        for partition in collections.values_mut() {
            total_cleared += partition.clear_flushed();
        }

        // Update global metrics
        let mut metrics = self.metrics.write().await;
        metrics.entry_count = metrics.entry_count.saturating_sub(total_cleared);

        Ok(total_cleared)
    }

    /// Mark all batches as flushed (for flush operations)
    pub async fn mark_all_batches_as_flushed(&self) -> Result<()> {
        let mut collections = self.collections.write().await;
        for partition in collections.values_mut() {
            for batch in partition.wal_batches.values_mut() {
                batch.is_flushed = true;
            }
        }
        Ok(())
    }

    /// Remove a specific batch from a collection (for atomic rollback)
    pub async fn remove_batch(&self, collection_id: &str, batch_id: &str) -> Result<()> {
        let mut collections = self.collections.write().await;
        if let Some(partition) = collections.get_mut(collection_id)
            && let Some(removed_batch) = partition.wal_batches.remove(batch_id) {
                // Update partition stats
                partition.vector_count = partition
                    .vector_count
                    .saturating_sub(removed_batch.vector_records.len());
                partition.total_size = partition
                    .total_size
                    .saturating_sub(removed_batch.total_size_bytes);
                partition.batch_count = partition.batch_count.saturating_sub(1);

                // Remove from vector index
                for vector_record in removed_batch.vector_records.iter() {
                    if !vector_record.id.is_empty() {
                        partition.vector_id_index.remove(&vector_record.id);
                    }
                }

                // Update global metrics
                let mut metrics = self.metrics.write().await;
                metrics.entry_count = metrics
                    .entry_count
                    .saturating_sub(removed_batch.vector_records.len());

                tracing::debug!(
                    "🗑️ Removed batch {} from collection {} ({} vectors)",
                    batch_id,
                    collection_id,
                    removed_batch.vector_records.len()
                );

                return Ok(());
            }

        Err(anyhow::anyhow!(
            "Batch {} not found in collection {}",
            batch_id,
            collection_id
        ))
    }

    /// Clear all vectors and batches
    pub async fn clear(&self) -> Result<()> {
        let mut collections = self.collections.write().await;
        collections.clear();
        drop(collections);

        let mut metrics = self.metrics.write().await;
        *metrics = MemtableMetrics::default();

        // Reset global sequence
        self.global_sequence.store(1, Ordering::SeqCst);

        Ok(())
    }

    /// Get current number of entries across all collections
    pub async fn len(&self) -> usize {
        let collections = self.collections.read().await;
        collections.values().map(|p| p.vector_count).sum()
    }

    /// Get current size in bytes across all collections
    pub async fn size_bytes(&self) -> usize {
        let collections = self.collections.read().await;
        collections.values().map(|p| p.total_size).sum()
    }

    /// Check if empty
    pub async fn is_empty(&self) -> bool {
        let collections = self.collections.read().await;
        collections.is_empty() || collections.values().all(|p| p.vector_count == 0)
    }

    /// Get statistics for all collections
    pub async fn get_all_collection_stats(&self) -> HashMap<String, (usize, usize)> {
        let collections = self.collections.read().await;
        collections
            .iter()
            .map(|(id, partition)| (id.clone(), (partition.vector_count, partition.total_size)))
            .collect()
    }

    /// Get stats for a specific collection
    pub async fn stats(&self, collection_id: &str) -> (usize, usize) {
        let collections = self.collections.read().await;
        collections
            .get(collection_id)
            .map_or((0, 0), |partition| (partition.vector_count, partition.total_size))
    }

    /// List all collection IDs
    pub async fn list_collections(&self) -> Result<Vec<String>> {
        let collections = self.collections.read().await;
        Ok(collections.keys().cloned().collect())
    }
}

impl GlobalPartitionedMemtable {
    /// Get all vectors without sequences (for flush operations) - MODERN
    pub async fn get_all_vectors_flat(&self) -> Result<Vec<VectorRecord>> {
        let vectors_with_sequences = self.get_all_vectors(None).await?;
        Ok(vectors_with_sequences
            .into_iter()
            .map(|(_, vector)| vector)
            .collect())
    }
}

impl Default for GlobalPartitionedMemtable {
    fn default() -> Self {
        Self::new()
    }
}

/// Collection flush information for intelligent flush selection
#[derive(Debug, Clone)]
pub struct CollectionFlushInfo {
    pub collection_id: String,
    pub total_size: usize,
    pub vector_count: usize,
    pub batch_count: usize,
    pub efficiency_score: f64,
    pub age_score: f64,
}

/// Calculate flush efficiency score based on collection characteristics
/// Higher score = more efficient to flush (larger size, fewer batches)
fn calculate_flush_efficiency_score(
    size_bytes: usize,
    vector_count: usize,
    batch_count: usize,
) -> f64 {
    // Efficiency factors:
    // 1. Size factor (larger is better for flush efficiency)
    // 2. Batch consolidation factor (fewer batches = more efficient)
    // 3. Vector density factor (more vectors per batch = better)

    let size_factor = (size_bytes as f64) / (1024.0 * 1024.0); // Convert to MB
    let batch_factor = if batch_count > 0 {
        (vector_count as f64) / (batch_count as f64)
    } else {
        0.0
    };

    // Weighted similarity: size matters more than batch consolidation
    let efficiency_score = (size_factor * 0.7) + (batch_factor * 0.3);
    efficiency_score.max(0.1) // Minimum score to avoid division by zero
}

/// Calculate age score based on collection creation time
/// Higher score = older collection (should be flushed sooner)
fn calculate_age_score(timestamp: std::time::SystemTime) -> f64 {
    let now = std::time::SystemTime::now();
    let age_duration = now
        .duration_since(timestamp)
        .unwrap_or_else(|_| std::time::Duration::from_secs(0));

    // Age in minutes (higher = older)
    let age_minutes = age_duration.as_secs() as f64 / 60.0;

    // Score increases with age, but caps at reasonable limit
    age_minutes.min(1440.0) // Cap at 24 hours
}

// Tests moved to src/storage/memtable/implementations/tests/global_partitioned_tests.rs

impl DistanceComputeProvider for GlobalPartitionedMemtable {
    fn distance_compute(&self) -> &UnifiedDistanceCompute {
        &self.distance_compute
    }
}

#[cfg(test)]
impl GlobalPartitionedMemtable {
    /// Test-only method to mark all batches in a collection as flushed
    pub async fn mark_all_batches_flushed(&self, collection_id: &str) -> Result<()> {
        let mut collections = self.collections.write().await;
        if let Some(partition) = collections.get_mut(collection_id) {
            for batch in partition.wal_batches.values_mut() {
                batch.is_flushed = true;
            }
            Ok(())
        } else {
            Err(anyhow::anyhow!("Collection not found"))
        }
    }
}
