//! WAL Behavior Wrapper for BTree
//! 
//! Extends BTreeMemtable with WAL-specific behaviors using composition:
//! - Sequential write optimizations for compression
//! - MVCC support for recovery consistency
//! - Ordered flush operations for RLE/dictionary encodings
//! - Specialized serialization strategies

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::RwLock;
use anyhow::Result;
use async_trait::async_trait;

use crate::storage::memtable::core::{MemtableCore, MemtableConfig};
use crate::storage::persistence::wal::{WalEntry, WalOperation};

/// WAL-specific behavior wrapper around any ordered memtable implementation
/// 
/// This extends the core BTree with WAL-specific functionality without modifying
/// the base implementation. Uses composition over inheritance.
#[derive(Debug)]
pub struct WalBehaviorWrapper<T> {
    /// The wrapped memtable implementation (typically BTreeMemtable)
    inner: T,
    
    /// WAL-specific configuration
    config: MemtableConfig,
    
    /// Sequence number generator for WAL entries
    sequence_generator: AtomicU64,
    
    /// MVCC tracking: vector_id -> [sequences]
    mvcc_versions: Arc<RwLock<std::collections::HashMap<String, Vec<u64>>>>,
    
    /// WAL-specific metrics
    wal_metrics: Arc<RwLock<WalMetrics>>,
    
    /// Flush coordination state
    flush_state: Arc<RwLock<FlushState>>,
}

impl<T> WalBehaviorWrapper<T> {
    /// Create new WAL behavior wrapper around a memtable implementation
    pub fn new(inner: T, config: MemtableConfig) -> Self {
        Self {
            inner,
            config,
            sequence_generator: AtomicU64::new(1),
            mvcc_versions: Arc::new(RwLock::new(std::collections::HashMap::new())),
            wal_metrics: Arc::new(RwLock::new(WalMetrics::default())),
            flush_state: Arc::new(RwLock::new(FlushState::default())),
        }
    }
    
    /// Get the wrapped implementation
    pub fn inner(&self) -> &T {
        &self.inner
    }
    
    /// Get next sequence number for WAL ordering
    pub fn next_sequence(&self) -> u64 {
        self.sequence_generator.fetch_add(1, Ordering::SeqCst)
    }
    
    /// Get current sequence number without incrementing
    pub fn current_sequence(&self) -> u64 {
        self.sequence_generator.load(Ordering::SeqCst)
    }
    
    /// Extract vector ID from WAL entry for MVCC tracking
    fn extract_vector_id(entry: &WalEntry) -> Option<String> {
        match &entry.operation {
            WalOperation::Insert { vector_id, .. } => Some(vector_id.clone()),
            WalOperation::Update { vector_id, .. } => Some(vector_id.clone()),
            WalOperation::Delete { vector_id, .. } => Some(vector_id.clone()),
            _ => None,
        }
    }
}

/// WAL-specific trait implementation
impl<T> WalBehaviorWrapper<T>
where
    T: MemtableCore<u64, WalEntry> + Send + Sync,
{
    /// Insert WAL entry with automatic sequence number
    pub async fn insert_wal_entry(&self, mut entry: WalEntry) -> Result<u64> {
        let sequence = self.next_sequence();
        entry.sequence = sequence;
        
        // Track MVCC versions if enabled
        if self.config.enable_mvcc {
            if let Some(vector_id) = Self::extract_vector_id(&entry) {
                let mut versions = self.mvcc_versions.write().await;
                versions.entry(vector_id).or_insert_with(Vec::new).push(sequence);
            }
        }
        
        // Insert with sequence as key
        let result = self.inner.insert(sequence, entry).await?;
        
        // Update WAL metrics
        let mut metrics = self.wal_metrics.write().await;
        metrics.entries_written += 1;
        metrics.bytes_written += result;
        
        Ok(sequence)
    }
    
    /// Get entries from sequence number (for recovery)
    pub async fn get_from_sequence(&self, from_seq: u64, limit: Option<usize>) -> Result<Vec<WalEntry>> {
        let entries = self.inner.range_scan(from_seq, limit).await?;
        Ok(entries.into_iter().map(|(_, entry)| entry).collect())
    }
    
    /// Check if flush is needed based on WAL-specific thresholds
    pub async fn should_flush(&self) -> bool {
        let size = self.inner.size_bytes().await;
        let count = self.inner.len().await;
        
        size >= self.config.flush_threshold_bytes || 
        count >= 10000 // WAL-specific entry count threshold
    }
    
    /// Create ordered flush data optimized for compression
    pub async fn create_flush_data(&self) -> Result<Vec<u8>> {
        let entries = self.inner.get_all_ordered().await?;
        
        // WAL entries are already ordered by sequence number (BTree key)
        // This enables optimal compression through RLE and dictionary encoding
        let mut flush_data = Vec::new();
        
        for (sequence, entry) in entries {
            // Serialize entry with sequence for ordered storage
            let serialized_entry = self.serialize_wal_entry(sequence, &entry).await?;
            flush_data.extend_from_slice(&serialized_entry);
        }
        
        // Update flush metrics
        let mut metrics = self.wal_metrics.write().await;
        metrics.flushes_performed += 1;
        metrics.total_flushed_bytes += flush_data.len() as u64;
        
        Ok(flush_data)
    }
    
    /// Serialize WAL entry with compression-friendly ordering
    async fn serialize_wal_entry(&self, sequence: u64, entry: &WalEntry) -> Result<Vec<u8>> {
        // Create ordered structure for optimal compression
        let ordered_entry = OrderedWalEntry {
            sequence,
            timestamp: entry.timestamp.timestamp_millis() as u64,
            operation_type: self.get_operation_type(&entry.operation),
            vector_id: Self::extract_vector_id(entry).unwrap_or_default(),
            operation_data: self.serialize_operation(&entry.operation).await?,
        };
        
        // Use Avro for compression-friendly serialization
        Ok(bincode::serialize(&ordered_entry)?)
    }
    
    fn get_operation_type(&self, operation: &WalOperation) -> u8 {
        match operation {
            WalOperation::Insert { .. } => 1,
            WalOperation::Update { .. } => 2,
            WalOperation::Delete { .. } => 3,
            WalOperation::Flush => 4,
            WalOperation::Checkpoint => 5,
            WalOperation::AvroPayload { .. } => 6,
        }
    }
    
    async fn serialize_operation(&self, operation: &WalOperation) -> Result<Vec<u8>> {
        Ok(bincode::serialize(operation)?)
    }
    
    /// Flush entries up to sequence number
    pub async fn flush_up_to_sequence(&self, seq: u64) -> Result<Vec<WalEntry>> {
        let entries_to_flush = self.get_from_sequence(0, None).await?
            .into_iter()
            .filter(|entry| entry.sequence <= seq)
            .collect::<Vec<_>>();
        
        // Remove flushed entries
        self.inner.clear_up_to(seq).await?;
        
        // Update MVCC tracking
        if self.config.enable_mvcc {
            let mut versions = self.mvcc_versions.write().await;
            for vector_versions in versions.values_mut() {
                vector_versions.retain(|&s| s > seq);
            }
            versions.retain(|_, v| !v.is_empty());
        }
        
        Ok(entries_to_flush)
    }
    
    /// Get WAL-specific metrics
    pub async fn get_wal_metrics(&self) -> WalMetrics {
        self.wal_metrics.read().await.clone()
    }
    
    /// Get MVCC versions for a vector ID
    pub async fn get_versions(&self, vector_id: &str) -> Result<Vec<WalEntry>> {
        if !self.config.enable_mvcc {
            return Ok(vec![]);
        }
        
        let versions = self.mvcc_versions.read().await;
        let sequences = match versions.get(vector_id) {
            Some(seqs) => seqs.clone(),
            None => return Ok(vec![]),
        };
        drop(versions);
        
        let mut entries = Vec::new();
        for seq in sequences {
            if let Ok(Some(entry)) = self.inner.get(&seq).await {
                entries.push(entry);
            }
        }
        
        // Sort by sequence number
        entries.sort_by_key(|e| e.sequence);
        
        Ok(entries)
    }
    
    /// Get latest version of a vector
    pub async fn get_latest_version(&self, vector_id: &str) -> Result<Option<WalEntry>> {
        let versions = self.get_versions(vector_id).await?;
        Ok(versions.into_iter().last())
    }
    
    /// Cleanup old versions (keep only N latest)
    pub async fn cleanup_versions(&self, vector_id: &str, keep_count: usize) -> Result<usize> {
        if !self.config.enable_mvcc {
            return Ok(0);
        }
        
        let mut versions = self.mvcc_versions.write().await;
        let sequences = match versions.get_mut(vector_id) {
            Some(seqs) => seqs,
            None => return Ok(0),
        };
        
        if sequences.len() <= keep_count {
            return Ok(0);
        }
        
        // Sort and keep only latest versions
        sequences.sort();
        let old_sequences = sequences.drain(0..sequences.len() - keep_count).collect::<Vec<_>>();
        let removed_count = old_sequences.len();
        
        drop(versions);
        
        // Remove old entries from underlying memtable
        for seq in old_sequences {
            self.inner.clear_up_to(seq).await?;
        }
        
        Ok(removed_count)
    }
}

#[async_trait]
impl<T> MemtableCore<u64, WalEntry> for WalBehaviorWrapper<T>
where
    T: MemtableCore<u64, WalEntry> + Send + Sync,
{
    async fn insert(&self, key: u64, value: WalEntry) -> Result<u64> {
        // Delegate to inner implementation
        self.inner.insert(key, value).await
    }
    
    async fn get(&self, key: &u64) -> Result<Option<WalEntry>> {
        self.inner.get(key).await
    }
    
    async fn range_scan(&self, from: u64, limit: Option<usize>) -> Result<Vec<(u64, WalEntry)>> {
        self.inner.range_scan(from, limit).await
    }
    
    async fn size_bytes(&self) -> usize {
        self.inner.size_bytes().await
    }
    
    async fn len(&self) -> usize {
        self.inner.len().await
    }
    
    async fn clear_up_to(&self, threshold: u64) -> Result<usize> {
        self.inner.clear_up_to(threshold).await
    }
    
    async fn clear(&self) -> Result<()> {
        let result = self.inner.clear().await;
        
        // Reset WAL-specific state
        if self.config.enable_mvcc {
            let mut versions = self.mvcc_versions.write().await;
            versions.clear();
        }
        
        let mut metrics = self.wal_metrics.write().await;
        *metrics = WalMetrics::default();
        
        result
    }
    
    async fn get_all_ordered(&self) -> Result<Vec<(u64, WalEntry)>> {
        self.inner.get_all_ordered().await
    }
}

impl<T> WalBehaviorWrapper<T>
where
    T: MemtableCore<u64, WalEntry> + Send + Sync,
{
    /// Get all entries (compatible with old WAL interface)
    pub async fn get_all_entries(&self, collection_id: &crate::core::CollectionId) -> Result<Vec<WalEntry>> {
        let entries = self.inner.get_all_ordered().await?;
        // Filter entries by collection_id
        let filtered_entries: Vec<WalEntry> = entries
            .into_iter()
            .map(|(_, entry)| entry)
            .filter(|entry| &entry.collection_id == collection_id)
            .collect();
        Ok(filtered_entries)
    }
    
    /// Get collections that need flushing
    pub async fn collections_needing_flush(&self) -> Result<Vec<crate::core::CollectionId>> {
        // For now, simple implementation - could be enhanced with per-collection tracking
        if self.should_flush().await {
            // Extract unique collection IDs from current entries
            let all_entries = self.inner.get_all_ordered().await?;
            let mut collections = std::collections::HashSet::new();
            for (_, entry) in all_entries {
                collections.insert(entry.collection_id.clone());
            }
            Ok(collections.into_iter().collect())
        } else {
            Ok(vec![])
        }
    }
    
    /// Clear flushed entries
    pub async fn clear_flushed(&self, collection_id: &crate::core::CollectionId, up_to_sequence: u64) -> Result<usize> {
        // Get all entries for this collection up to the sequence number
        let entries = self.inner.get_all_ordered().await?;
        let mut cleared_count = 0;
        
        for (seq, entry) in entries {
            if &entry.collection_id == collection_id && seq <= up_to_sequence {
                // Remove this specific entry - for now use a simple approach
                // In a real implementation, we'd need a more efficient removal method
                cleared_count += 1;
            }
        }
        
        // For simplicity, clear all entries up to sequence (this could be optimized)
        let total_cleared = self.inner.clear_up_to(up_to_sequence).await?;
        Ok(cleared_count)
    }
    
    /// Insert batch of entries
    pub async fn insert_batch(&self, entries: Vec<WalEntry>) -> Result<Vec<u64>> {
        let mut sequences = Vec::new();
        for entry in entries {
            let seq = self.insert(entry.sequence, entry).await?;
            sequences.push(seq);
        }
        Ok(sequences)
    }
    
    /// Insert single entry (compatible interface)
    pub async fn insert_entry(&self, entry: WalEntry) -> Result<u64> {
        self.insert(entry.sequence, entry).await
    }
    
    /// Get statistics (compatible interface)
    pub async fn get_stats(&self) -> Result<std::collections::HashMap<crate::core::CollectionId, crate::storage::persistence::wal::WalStats>> {
        // Simple implementation - could be enhanced with detailed per-collection stats
        let mut stats = std::collections::HashMap::new();
        let total_size = self.inner.size_bytes().await;
        let total_entries = self.inner.len().await;
        
        // Get unique collections from entries
        let entries = self.inner.get_all_ordered().await?;
        let mut collection_counts = std::collections::HashMap::new();
        for (_, entry) in entries {
            *collection_counts.entry(entry.collection_id.clone()).or_insert(0) += 1;
        }
        
        // Create stats for each collection
        for (collection_id, count) in collection_counts {
            let wal_stats = crate::storage::persistence::wal::WalStats {
                total_entries: count as u64,
                memory_entries: count as u64,
                disk_segments: 0,
                total_disk_size_bytes: 0,
                memory_size_bytes: if total_entries > 0 { (total_size / total_entries * count) as u64 } else { 0 },
                collections_count: 1,
                last_flush_time: None,
                write_throughput_entries_per_sec: 0.0,
                read_throughput_entries_per_sec: 0.0,
                compression_ratio: 1.0,
            };
            stats.insert(collection_id, wal_stats);
        }
        
        Ok(stats)
    }
    
    /// Check if global flush is needed
    pub async fn needs_global_flush(&self) -> Result<bool> {
        // Use the same logic as should_flush but with a more conservative threshold
        let size = self.inner.size_bytes().await;
        let count = self.inner.len().await;
        
        // Global flush needed if we exceed larger thresholds
        Ok(size >= self.config.flush_threshold_bytes * 2 || count >= 50000)
    }
    
    /// Search for specific vector entry
    pub async fn search_vector(&self, collection_id: &crate::core::CollectionId, vector_id: &str) -> Result<Option<WalEntry>> {
        let entries = self.inner.get_all_ordered().await?;
        
        for (_, entry) in entries {
            if &entry.collection_id == collection_id && entry.entry_id == vector_id {
                return Ok(Some(entry));
            }
        }
        
        Ok(None)
    }
    
    /// Get entries for specific collection
    pub async fn get_entries(&self, collection_id: &crate::core::CollectionId, from_sequence: u64, limit: Option<usize>) -> Result<Vec<WalEntry>> {
        let all_entries = self.get_all_entries(collection_id).await?;
        
        // Filter by sequence number and apply limit
        let mut filtered: Vec<WalEntry> = all_entries
            .into_iter()
            .filter(|entry| entry.sequence >= from_sequence)
            .collect();
        
        // Sort by sequence number
        filtered.sort_by_key(|entry| entry.sequence);
        
        // Apply limit if specified
        if let Some(limit) = limit {
            filtered.truncate(limit);
        }
        
        Ok(filtered)
    }
    
    /// Get collection-specific statistics
    pub async fn get_collection_stats(&self, collection_id: &crate::core::CollectionId) -> Result<crate::storage::persistence::wal::WalStats> {
        let all_stats = self.get_stats().await?;
        
        match all_stats.get(collection_id) {
            Some(stats) => Ok(stats.clone()),
            None => Ok(crate::storage::persistence::wal::WalStats {
                total_entries: 0,
                memory_entries: 0,
                disk_segments: 0,
                total_disk_size_bytes: 0,
                memory_size_bytes: 0,
                collections_count: 0,
                last_flush_time: None,
                write_throughput_entries_per_sec: 0.0,
                read_throughput_entries_per_sec: 0.0,
                compression_ratio: 1.0,
            })
        }
    }
    
    /// Drop collection from memtable
    pub async fn drop_collection(&self, collection_id: &crate::core::CollectionId) -> Result<usize> {
        // Get all entries to find ones for this collection
        let entries = self.inner.get_all_ordered().await?;
        let mut removed_count = 0;
        
        // For simplicity, we'll clear all entries for this collection
        // In a real implementation, this would be more efficient with selective removal
        for (seq, entry) in entries {
            if &entry.collection_id == collection_id {
                // Would remove individual entry - for now just count
                removed_count += 1;
            }
        }
        
        // Note: Actual removal would require a more sophisticated approach
        // For now, this is a placeholder implementation
        Ok(removed_count)
    }
    
    /// Perform maintenance operations
    pub async fn maintenance(&self) -> Result<crate::storage::persistence::wal::WalStats> {
        // Cleanup old MVCC versions if enabled
        if self.config.enable_mvcc {
            let _cleaned = self.cleanup_versions("", 10).await?; // Keep 10 versions
        }
        
        // Return current stats after maintenance
        let all_stats = self.get_stats().await?;
        
        // Return aggregated stats
        let total_entries: u64 = all_stats.values().map(|s| s.total_entries).sum();
        let total_memory: u64 = all_stats.values().map(|s| s.memory_size_bytes).sum();
        
        Ok(crate::storage::persistence::wal::WalStats {
            total_entries,
            memory_entries: total_entries,
            disk_segments: 0,
            total_disk_size_bytes: 0,
            memory_size_bytes: total_memory,
            collections_count: all_stats.len(),
            last_flush_time: None,
            write_throughput_entries_per_sec: 0.0,
            read_throughput_entries_per_sec: 0.0,
            compression_ratio: 1.0,
        })
    }
    
    /// Atomically mark entries for flush
    pub async fn atomic_mark_for_flush(&self, collection_id: &crate::core::CollectionId, up_to_sequence: u64) -> Result<Vec<WalEntry>> {
        // Get entries for this collection up to the sequence
        let entries = self.get_all_entries(collection_id).await?;
        let marked_entries: Vec<WalEntry> = entries
            .into_iter()
            .filter(|entry| entry.sequence <= up_to_sequence)
            .collect();
        
        Ok(marked_entries)
    }
    
    /// Complete flush and remove marked entries
    pub async fn complete_flush_removal(&self, collection_id: &crate::core::CollectionId, up_to_sequence: u64) -> Result<usize> {
        self.clear_flushed(collection_id, up_to_sequence).await
    }
    
    /// Abort flush and restore entries
    pub async fn abort_flush_restore(&self, collection_id: &crate::core::CollectionId, _entries: Vec<WalEntry>) -> Result<()> {
        // In a real implementation, this would restore the entries
        // For now, this is a no-op since we haven't actually removed them
        tracing::warn!("Flush aborted for collection {}, entries preserved in memtable", collection_id);
        Ok(())
    }
}

/// Ordered WAL entry for compression-friendly serialization
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct OrderedWalEntry {
    sequence: u64,
    timestamp: u64,
    operation_type: u8,
    vector_id: String,
    operation_data: Vec<u8>,
}

/// WAL-specific metrics
#[derive(Debug, Clone, Default)]
pub struct WalMetrics {
    pub entries_written: u64,
    pub bytes_written: u64,
    pub flushes_performed: u64,
    pub total_flushed_bytes: u64,
    pub recovery_operations: u64,
    pub mvcc_versions_active: usize,
}

/// Flush coordination state
#[derive(Debug, Clone, Default)]
struct FlushState {
    last_flush_sequence: u64,
    flush_in_progress: bool,
    flush_start_time: Option<std::time::Instant>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::memtable::implementations::btree::BTreeMemtable;
    use crate::storage::persistence::wal::{WalEntry, WalOperation};
    
    #[tokio::test]
    async fn test_wal_behavior_wrapper() {
        let config = MemtableConfig::default();
        let btree = BTreeMemtable::new(config.enable_mvcc);
        let mut wal_wrapper = WalBehaviorWrapper::new(btree, config);
        
        // Create test WAL entry
        let now = chrono::Utc::now().timestamp_millis();
        let vector_record = crate::core::VectorRecord {
            id: "test_vector_1".to_string(),
            collection_id: "test_collection".to_string(),
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
        
        let wal_entry = WalEntry {
            entry_id: "test_vector_1".to_string(),
            collection_id: crate::core::CollectionId::from("test_collection".to_string()),
            sequence: 0, // Will be auto-assigned
            global_sequence: 0,
            timestamp: chrono::Utc::now(),
            expires_at: None,
            version: 1,
            operation: WalOperation::Insert {
                vector_id: crate::core::VectorId::from("test_vector_1".to_string()),
                record: vector_record,
                expires_at: None,
            },
        };
        
        // Test WAL entry insertion
        let seq1 = wal_wrapper.insert_wal_entry(wal_entry.clone()).await.unwrap();
        let seq2 = wal_wrapper.insert_wal_entry(wal_entry.clone()).await.unwrap();
        
        assert!(seq2 > seq1);
        assert_eq!(wal_wrapper.len().await, 2);
        
        // Test sequence-based retrieval
        let entries = wal_wrapper.get_from_sequence(seq1, None).await.unwrap();
        assert_eq!(entries.len(), 2);
        
        // Test flush threshold
        assert!(!wal_wrapper.should_flush().await); // Small entries shouldn't trigger flush
        
        // Test flush operation
        let flushed = wal_wrapper.flush_up_to_sequence(seq1).await.unwrap();
        assert_eq!(flushed.len(), 1);
        assert_eq!(wal_wrapper.len().await, 1);
        
        // Test metrics
        let metrics = wal_wrapper.get_wal_metrics().await;
        assert_eq!(metrics.entries_written, 2);
        assert_eq!(metrics.flushes_performed, 0); // flush_up_to_sequence doesn't update this metric
    }
    
    #[tokio::test]
    async fn test_wal_mvcc_functionality() {
        let mut config = MemtableConfig::default();
        config.enable_mvcc = true;
        
        let btree = BTreeMemtable::new(config.enable_mvcc);
        let mut wal_wrapper = WalBehaviorWrapper::new(btree, config);
        
        let vector_id = "test_vector_mvcc";
        
        // Insert multiple versions of the same vector
        for i in 0..3 {
            let now = chrono::Utc::now().timestamp_millis();
            let vector_record = crate::core::VectorRecord {
                id: vector_id.to_string(),
                collection_id: "test_collection".to_string(),
                vector: vec![i as f32, (i + 1) as f32],
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
            
            let wal_entry = WalEntry {
                entry_id: vector_id.to_string(),
                collection_id: crate::core::CollectionId::from("test_collection".to_string()),
                sequence: 0,
                global_sequence: 0,
                timestamp: chrono::Utc::now(),
                expires_at: None,
                version: 1,
                operation: WalOperation::Update {
                    vector_id: crate::core::VectorId::from(vector_id.to_string()),
                    record: vector_record,
                    expires_at: None,
                },
            };
            
            wal_wrapper.insert_wal_entry(wal_entry).await.unwrap();
        }
        
        // Test version retrieval
        let versions = wal_wrapper.get_versions(vector_id).await.unwrap();
        assert_eq!(versions.len(), 3);
        
        // Test latest version
        let latest = wal_wrapper.get_latest_version(vector_id).await.unwrap();
        assert!(latest.is_some());
        
        // Test version cleanup
        let removed = wal_wrapper.cleanup_versions(vector_id, 1).await.unwrap();
        assert_eq!(removed, 2);
        
        let remaining_versions = wal_wrapper.get_versions(vector_id).await.unwrap();
        assert_eq!(remaining_versions.len(), 1);
    }
}