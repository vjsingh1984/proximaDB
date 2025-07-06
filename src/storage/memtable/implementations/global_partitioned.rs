//! Global Partitioned Memtable Implementation for WAL
//!
//! Optimized for global WAL with collection partitioning:
//! - Global sequence ordering for flush coordination
//! - Per-collection data partitions for efficient operations
//! - Content-based search within collections
//! - Efficient per-collection flush isolation

use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

use super::super::core::{MemtableCore, MemtableMetrics};
use crate::compute::distance::DistanceMetric as CoreDistanceMetric;
use crate::compute::unified_distance::{
    DistanceComputeProvider, DistanceResultOrdering, UnifiedDistanceCompute,
};
use crate::storage::persistence::wal::WalEntry;

/// Collection partition within the global memtable
#[derive(Debug)]
struct CollectionPartition {
    /// Sequence-based storage within collection (local ordering)
    entries: Vec<WalEntry>,

    /// Content-based search index within collection (content_key -> vec index)
    content_index: HashMap<String, usize>,

    /// Collection statistics
    total_size: usize,
    entry_count: usize,
    last_flush_sequence: u64,
    created_at: std::time::SystemTime,
}

impl CollectionPartition {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
            content_index: HashMap::new(),
            total_size: 0,
            entry_count: 0,
            last_flush_sequence: 0,
            created_at: std::time::SystemTime::now(),
        }
    }

    /// Append entry to this collection partition
    fn append(&mut self, entry: WalEntry) -> Result<usize> {
        let entry_size = entry.actual_size_bytes();
        let local_index = self.entries.len();

        // Generate content key for search index
        let content_key = entry
            .content_key()
            .map_err(|e| anyhow::anyhow!("Failed to generate content key: {}", e))?;

        // Add to sequence-based storage
        self.entries.push(entry);

        // Add to content-based index
        self.content_index.insert(content_key, local_index);

        // Update statistics
        self.total_size += entry_size;
        self.entry_count += 1;

        Ok(local_index)
    }

    /// Get entry by content key within this collection
    fn get_by_content(&self, content_key: &str) -> Option<WalEntry> {
        if let Some(&index) = self.content_index.get(content_key) {
            self.entries.get(index).cloned()
        } else {
            None
        }
    }

    /// Clear entries up to sequence number within this collection
    fn clear_up_to(&mut self, up_to_seq: u64) -> usize {
        let mut cleared_count = 0;
        let mut removed_size = 0;
        let mut new_entries = Vec::new();
        let mut new_content_index = HashMap::new();

        // Rebuild entries and index, excluding cleared entries
        for (index, entry) in self.entries.iter().enumerate() {
            if entry.sequence <= up_to_seq {
                // Clear this entry
                cleared_count += 1;
                removed_size += entry.actual_size_bytes();
            } else {
                // Keep this entry with new index
                let new_index = new_entries.len();
                new_entries.push(entry.clone());

                // Update content index with new index
                if let Ok(content_key) = entry.content_key() {
                    new_content_index.insert(content_key, new_index);
                }
            }
        }

        // Replace data structures
        self.entries = new_entries;
        self.content_index = new_content_index;

        // Update statistics
        self.total_size = self.total_size.saturating_sub(removed_size);
        self.entry_count = self.entry_count.saturating_sub(cleared_count);
        self.last_flush_sequence = up_to_seq;

        cleared_count
    }

    /// Check if this collection needs flushing
    fn needs_flush(&self, size_threshold: usize, count_threshold: usize) -> bool {
        self.total_size >= size_threshold || self.entry_count >= count_threshold
    }

    /// Extract vector data from WAL entry (memtable stores deserialized data)
    fn extract_vector_from_entry(entry: &WalEntry) -> Option<Vec<f32>> {
        use crate::storage::persistence::wal::WalOperation;

        match &entry.operation {
            WalOperation::AvroPayload { avro_data, .. } => {
                // Parse Avro payload to extract vector data - supports both single and batch
                if let Ok(single_record) = crate::core::VectorRecord::from_avro_bytes(avro_data) {
                    Some(single_record.vector)
                } else if let Ok(vectors) = crate::storage::persistence::wal::schema::deserialize_vector_batch(avro_data) {
                    vectors.first().map(|v| v.vector.clone())
                } else {
                    tracing::warn!("Failed to parse Avro payload for vector extraction");
                    None
                }
            }
            _ => None,
        }
    }

    /// Search for similar vectors with support for WAL entries and ID-based deduplication
    fn search_vectors_unified(
        &self,
        query_vector: &[f32],
        distance_metric: &CoreDistanceMetric,
        distance_compute: &UnifiedDistanceCompute,
    ) -> Vec<(f32, WalEntry)> {
        use std::collections::HashMap;
        
        let mut id_to_latest_entry: HashMap<String, (f32, WalEntry, u64)> = HashMap::new();
        let mut results_without_id = Vec::new();

        // Linear search through collection entries with deduplication
        for entry in &self.entries {
            // Handle different WAL operation types
            match &entry.operation {
                crate::storage::persistence::wal::WalOperation::AvroPayload { avro_data, .. } => {
                    // Parse AVRO data for search - supports both single and batch formats
                    if let Ok(single_record) = crate::core::VectorRecord::from_avro_bytes(avro_data) {
                        let score = distance_compute.calculate_distance(query_vector, &single_record.vector, distance_metric);
                        
                        // ID-based deduplication: keep only latest version by sequence number
                        if !single_record.id.is_empty() {
                            match id_to_latest_entry.get(&single_record.id) {
                                Some((_, _, existing_seq)) if entry.sequence <= *existing_seq => {
                                    // Skip older entry
                                    continue;
                                }
                                _ => {
                                    // Keep this entry (newer or first occurrence)
                                    id_to_latest_entry.insert(
                                        single_record.id.clone(),
                                        (score, entry.clone(), entry.sequence)
                                    );
                                }
                            }
                        } else {
                            // No ID - include directly (no deduplication possible)
                            results_without_id.push((score, entry.clone()));
                        }
                    } else if let Ok(vectors) = crate::storage::persistence::wal::schema::deserialize_vector_batch(avro_data) {
                        // Handle batch data - search each vector in batch with deduplication
                        for vector_record in vectors {
                            let score = distance_compute.calculate_distance(query_vector, &vector_record.vector, distance_metric);
                            
                            // ID-based deduplication within batch
                            if !vector_record.id.is_empty() {
                                match id_to_latest_entry.get(&vector_record.id) {
                                    Some((_, _, existing_seq)) if entry.sequence <= *existing_seq => {
                                        // Skip older entry
                                        continue;
                                    }
                                    _ => {
                                        // Keep this entry (newer or first occurrence)
                                        id_to_latest_entry.insert(
                                            vector_record.id.clone(),
                                            (score, entry.clone(), entry.sequence)
                                        );
                                    }
                                }
                            } else {
                                // No ID - include directly (no deduplication possible)
                                results_without_id.push((score, entry.clone()));
                            }
                        }
                    }
                }
                _ => {
                    // Skip non-vector operations
                }
            }
        }

        // Combine deduplicated ID-based results with non-ID results
        let mut final_results = Vec::new();
        
        // Count results before moving
        let unique_ids_count = id_to_latest_entry.len();
        let results_without_id_count = results_without_id.len();
        
        // Add latest version of each ID
        for (_, (score, entry, _)) in id_to_latest_entry {
            final_results.push((score, entry));
        }
        
        // Add non-ID results
        final_results.extend(results_without_id);

        tracing::debug!(
            "🔍 Search deduplication: {} total entries, {} unique IDs, {} without ID, {} final results",
            self.entries.len(),
            unique_ids_count,
            results_without_id_count,
            final_results.len()
        );

        final_results
    }
}

/// Global partitioned memtable implementation for WAL operations
///
/// This implements a three-tier index structure:
/// 1. Global sequence ordering for flush coordination
/// 2. Per-collection data partitions for efficient operations  
/// 3. Content-based search within collections
#[derive(Debug)]
pub struct GlobalPartitionedMemtable {
    /// Global sequence generator for cross-collection ordering
    global_sequence: AtomicU64,

    /// Per-collection data partitions (collection_id -> partition)
    collections: Arc<RwLock<HashMap<String, CollectionPartition>>>,

    /// Global content index for cross-collection deduplication (content_key -> (collection_id, local_index))
    global_content_index: Arc<RwLock<HashMap<String, (String, usize)>>>,

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
            global_content_index: Arc::new(RwLock::new(HashMap::new())),
            distance_compute: UnifiedDistanceCompute::default(),
            metrics: Arc::new(RwLock::new(MemtableMetrics::default())),
        }
    }

    /// Append entry to the appropriate collection partition
    pub async fn append(&self, mut entry: WalEntry) -> Result<u64> {
        // Assign global sequence
        let global_seq = self.global_sequence.fetch_add(1, Ordering::SeqCst);
        entry.sequence = global_seq;

        let collection_id = entry.collection_id.clone();
        let entry_size = entry.actual_size_bytes();

        // Get or create collection partition
        let mut collections = self.collections.write().await;
        let partition = collections
            .entry(collection_id.clone())
            .or_insert_with(CollectionPartition::new);

        // Append to collection partition
        let local_index = partition.append(entry.clone())?;
        drop(collections);

        // Update global content index
        let content_key = entry
            .content_key()
            .map_err(|e| anyhow::anyhow!("Failed to generate content key: {}", e))?;
        let mut global_index = self.global_content_index.write().await;
        global_index.insert(content_key.clone(), (collection_id.clone(), local_index));
        drop(global_index);

        // Update global metrics
        let mut metrics = self.metrics.write().await;
        metrics.insert_count += 1;
        metrics.entry_count += 1;
        metrics.size_bytes += entry_size;

        tracing::info!("🌍 *** GLOBAL_PARTITIONED_MEMTABLE_INSERT_TRACE *** 🌍: collection={}, global_seq={}, local_index={}, size={}B, THIS_IS_THE_MODERN_IMPLEMENTATION", 
                       collection_id, global_seq, local_index, entry_size);

        Ok(global_seq)
    }

    /// Get entry by global sequence number
    pub async fn get_by_sequence(&self, sequence: u64) -> Result<Option<WalEntry>> {
        let collections = self.collections.read().await;

        // Linear search through all collections (could be optimized with sequence->collection mapping)
        for partition in collections.values() {
            for entry in &partition.entries {
                if entry.sequence == sequence {
                    return Ok(Some(entry.clone()));
                }
            }
        }

        Ok(None)
    }

    /// Get entry by content key (across all collections)
    pub async fn get_by_content(&self, content_key: &str) -> Result<Option<WalEntry>> {
        let global_index = self.global_content_index.read().await;

        if let Some((collection_id, local_index)) = global_index.get(content_key) {
            let collections = self.collections.read().await;
            if let Some(partition) = collections.get(collection_id) {
                if let Some(entry) = partition.entries.get(*local_index) {
                    return Ok(Some(entry.clone()));
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
    ) -> Result<Vec<(f32, WalEntry)>> {
        let collections = self.collections.read().await;

        if let Some(partition) = collections.get(collection_id) {
            let mut results = partition.search_vectors_unified(
                query_vector,
                &distance_metric,
                &self.distance_compute,
            );

            // Sort and limit results using unified distance system
            DistanceResultOrdering::sort_and_limit(
                &mut results,
                &distance_metric,
                &self.distance_compute,
                k,
            );

            tracing::debug!("📊 GLOBAL_PARTITIONED_SEARCH: Found {} results in collection {} (partition has {} entries) using {:?}", 
                           results.len(), collection_id, partition.entry_count, distance_metric);
            Ok(results)
        } else {
            tracing::debug!(
                "📊 GLOBAL_PARTITIONED_SEARCH: Collection {} not found",
                collection_id
            );
            Ok(Vec::new())
        }
    }

    /// Get all entries for a specific collection
    pub async fn get_collection_entries(&self, collection_id: &str) -> Result<Vec<WalEntry>> {
        let collections = self.collections.read().await;

        if let Some(partition) = collections.get(collection_id) {
            Ok(partition.entries.clone())
        } else {
            Ok(Vec::new())
        }
    }

    /// Get collection statistics
    pub async fn get_collection_stats(&self, collection_id: &str) -> (usize, usize) {
        let collections = self.collections.read().await;

        if let Some(partition) = collections.get(collection_id) {
            (partition.entry_count, partition.total_size)
        } else {
            (0, 0)
        }
    }

    /// Clear entries for a specific collection up to sequence number
    pub async fn clear_collection_up_to(
        &self,
        collection_id: &str,
        up_to_seq: u64,
    ) -> Result<usize> {
        let mut collections = self.collections.write().await;

        if let Some(partition) = collections.get_mut(collection_id) {
            let cleared_count = partition.clear_up_to(up_to_seq);

            // Update global metrics
            let mut metrics = self.metrics.write().await;
            metrics.entry_count = metrics.entry_count.saturating_sub(cleared_count);
            // Note: size_bytes will be recalculated in next size_bytes() call

            tracing::debug!(
                "📊 GLOBAL_PARTITIONED: Cleared {} entries from collection {} up to sequence {}",
                cleared_count,
                collection_id,
                up_to_seq
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
                    partition.entry_count,
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

    /// Get entries from sequence number onwards (for recovery)
    pub async fn get_from_sequence(
        &self,
        from_seq: u64,
        limit: Option<usize>,
    ) -> Result<Vec<WalEntry>> {
        let collections = self.collections.read().await;
        let mut all_entries = Vec::new();

        // Collect all entries from all collections
        for partition in collections.values() {
            for entry in &partition.entries {
                if entry.sequence >= from_seq {
                    all_entries.push(entry.clone());
                }
            }
        }

        // Sort by global sequence
        all_entries.sort_by_key(|entry| entry.sequence);

        // Apply limit if specified
        if let Some(limit) = limit {
            all_entries.truncate(limit);
        }

        Ok(all_entries)
    }

    /// Clear entries up to sequence number (global operation)
    pub async fn clear_up_to(&self, up_to_seq: u64) -> Result<usize> {
        let mut collections = self.collections.write().await;
        let mut total_cleared = 0;

        for partition in collections.values_mut() {
            total_cleared += partition.clear_up_to(up_to_seq);
        }

        // Update global metrics
        let mut metrics = self.metrics.write().await;
        metrics.entry_count = metrics.entry_count.saturating_sub(total_cleared);

        Ok(total_cleared)
    }

    /// Clear all entries
    pub async fn clear(&self) -> Result<()> {
        let mut collections = self.collections.write().await;
        collections.clear();
        drop(collections);

        let mut global_index = self.global_content_index.write().await;
        global_index.clear();
        drop(global_index);

        let mut metrics = self.metrics.write().await;
        *metrics = MemtableMetrics::default();

        // Reset global sequence
        self.global_sequence.store(1, Ordering::SeqCst);

        Ok(())
    }

    /// Get current number of entries across all collections
    pub async fn len(&self) -> usize {
        let collections = self.collections.read().await;
        collections.values().map(|p| p.entry_count).sum()
    }

    /// Get current size in bytes across all collections
    pub async fn size_bytes(&self) -> usize {
        let collections = self.collections.read().await;
        collections.values().map(|p| p.total_size).sum()
    }

    /// Check if empty
    pub async fn is_empty(&self) -> bool {
        let collections = self.collections.read().await;
        collections.is_empty() || collections.values().all(|p| p.entry_count == 0)
    }

    /// Get statistics for all collections
    pub async fn get_all_collection_stats(&self) -> HashMap<String, (usize, usize)> {
        let collections = self.collections.read().await;
        collections
            .iter()
            .map(|(id, partition)| (id.clone(), (partition.entry_count, partition.total_size)))
            .collect()
    }
}

// Implement MemtableCore for backwards compatibility
#[async_trait]
impl MemtableCore<u64, WalEntry> for GlobalPartitionedMemtable {
    async fn insert(&self, _key: u64, value: WalEntry) -> Result<u64> {
        // For global partitioned implementation, we ignore the key and use collection-based partitioning
        self.append(value).await
    }

    async fn get(&self, key: &u64) -> Result<Option<WalEntry>> {
        self.get_by_sequence(*key).await
    }

    async fn range_scan(&self, from: u64, limit: Option<usize>) -> Result<Vec<(u64, WalEntry)>> {
        let entries = self.get_from_sequence(from, limit).await?;
        let mut result = Vec::new();

        for entry in entries {
            result.push((entry.sequence, entry));
        }

        Ok(result)
    }

    async fn size_bytes(&self) -> usize {
        self.size_bytes().await
    }

    async fn len(&self) -> usize {
        self.len().await
    }

    async fn clear_up_to(&self, threshold: u64) -> Result<usize> {
        self.clear_up_to(threshold).await
    }

    async fn clear(&self) -> Result<()> {
        self.clear().await
    }

    async fn get_all_ordered(&self) -> Result<Vec<(u64, WalEntry)>> {
        let entries = self.get_from_sequence(0, None).await?;
        let mut result = Vec::new();

        for entry in entries {
            result.push((entry.sequence, entry));
        }

        Ok(result)
    }
}

impl GlobalPartitionedMemtable {
    /// Get all entries (for flush operations)
    pub async fn get_all(&self) -> Result<Vec<WalEntry>> {
        self.get_from_sequence(0, None).await
    }
}

impl Default for GlobalPartitionedMemtable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::distance::DistanceMetric as CoreDistanceMetric;
    use crate::storage::persistence::wal::{WalEntry, WalOperation};

    #[tokio::test]
    async fn test_global_partitioned_basic_operations() {
        let memtable = GlobalPartitionedMemtable::new();

        // Create test WAL entries for different collections
        let now = chrono::Utc::now().timestamp_millis();
        let vector_record1 = crate::core::VectorRecord {
            id: "test_vector_1".to_string(),
            collection_id: "collection_a".to_string(),
            vector: vec![0.1, 0.2, 0.3],
            metadata: std::collections::HashMap::new(),
            timestamp: now,
            created_at: now,
            updated_at: now,
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        };

        let vector_record2 = crate::core::VectorRecord {
            id: "test_vector_2".to_string(),
            collection_id: "collection_b".to_string(),
            vector: vec![0.4, 0.5, 0.6],
            metadata: std::collections::HashMap::new(),
            timestamp: now,
            created_at: now,
            updated_at: now,
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        };

        let avro_data1 = vector_record1.to_avro_bytes().unwrap();
        let wal_entry1 = WalEntry {
            entry_id: "test_vector_1".to_string(),
            collection_id: "collection_a".to_string(),
            sequence: 0, // Will be assigned by append
            global_sequence: 0,
            timestamp: chrono::Utc::now(),
            expires_at: None,
            version: 1,
            operation: WalOperation::AvroPayload {
                operation_type: "upsert".to_string(),
                avro_data: avro_data1,
            },
        };

        let avro_data2 = vector_record2.to_avro_bytes().unwrap();
        let wal_entry2 = WalEntry {
            entry_id: "test_vector_2".to_string(),
            collection_id: "collection_b".to_string(),
            sequence: 0, // Will be assigned by append
            global_sequence: 0,
            timestamp: chrono::Utc::now(),
            expires_at: None,
            version: 1,
            operation: WalOperation::AvroPayload {
                operation_type: "upsert".to_string(),
                avro_data: avro_data2,
            },
        };

        // Test append to different collections
        let seq1 = memtable.append(wal_entry1.clone()).await.unwrap();
        let seq2 = memtable.append(wal_entry2.clone()).await.unwrap();

        assert_eq!(seq1, 1);
        assert_eq!(seq2, 2);
        assert_eq!(memtable.len().await, 2);

        // Test collection-specific operations
        let (entries_a, size_a) = memtable.get_collection_stats("collection_a").await;
        let (entries_b, size_b) = memtable.get_collection_stats("collection_b").await;

        assert_eq!(entries_a, 1);
        assert_eq!(entries_b, 1);
        assert!(size_a > 0);
        assert!(size_b > 0);

        // Test collection-specific search
        let query_vector = vec![0.1, 0.2, 0.3];
        let search_results_a = memtable
            .search_vectors(&query_vector, 2, "collection_a", CoreDistanceMetric::Cosine)
            .await
            .unwrap();
        let search_results_b = memtable
            .search_vectors(&query_vector, 2, "collection_b", CoreDistanceMetric::Cosine)
            .await
            .unwrap();

        assert_eq!(search_results_a.len(), 1);
        assert_eq!(search_results_b.len(), 1);

        // Test sequence-based retrieval
        let retrieved1 = memtable.get_by_sequence(seq1).await.unwrap();
        assert!(retrieved1.is_some());
        assert_eq!(retrieved1.unwrap().entry_id, "test_vector_1");

        // Test collection-specific cleanup
        let cleared = memtable
            .clear_collection_up_to("collection_a", seq1)
            .await
            .unwrap();
        assert_eq!(cleared, 1);
        assert_eq!(memtable.len().await, 1);

        let (entries_a_after, _) = memtable.get_collection_stats("collection_a").await;
        let (entries_b_after, _) = memtable.get_collection_stats("collection_b").await;

        assert_eq!(entries_a_after, 0);
        assert_eq!(entries_b_after, 1);
    }
}

impl DistanceComputeProvider for GlobalPartitionedMemtable {
    fn distance_compute(&self) -> &UnifiedDistanceCompute {
        &self.distance_compute
    }
}
