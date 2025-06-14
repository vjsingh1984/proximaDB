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

//! Write-Ahead Log (WAL) implementation for durability

use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncWriteExt, AsyncReadExt, AsyncSeekExt};
use chrono::{DateTime, Utc};
use uuid::Uuid;
use serde::{Serialize, Deserialize};

use crate::core::VectorRecord;
use crate::storage::{Result, StorageError};
use crate::storage::viper::{
    ClusterPrediction, VectorStorageFormat, ClusterQualityMetrics, 
    ClusterId, PartitionId, TierLevel
};

/// MVCC operation types for versioning and soft deletes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MvccOperationType {
    /// Insert new version of vector
    Insert,
    /// Update existing vector (creates new version)
    Update,
    /// Soft delete (sets expires_at to current timestamp)
    SoftDelete,
    /// Set TTL for automatic expiration
    SetTtl,
    /// Remove TTL (set expires_at to None)
    RemoveTtl,
}

/// WAL entry types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WalEntry {
    /// Insert or update a vector
    Put {
        collection_id: String,
        record: VectorRecord,
        timestamp: DateTime<Utc>,
    },
    /// Delete a vector (soft delete using expires_at)
    Delete {
        collection_id: String,
        vector_id: Uuid,
        timestamp: DateTime<Utc>,
    },
    /// MVCC-aware vector operation with expires_at for versioning
    MvccVectorOperation {
        collection_id: String,
        vector_id: Uuid,
        vector_data: Vec<f32>,
        metadata: serde_json::Value,
        expires_at: Option<DateTime<Utc>>,
        operation_type: MvccOperationType,
        timestamp: DateTime<Utc>,
    },
    /// Create a new collection
    CreateCollection {
        collection_id: String,
        timestamp: DateTime<Utc>,
    },
    /// Delete a collection
    DeleteCollection {
        collection_id: String,
        timestamp: DateTime<Utc>,
    },
    /// Checkpoint marker
    Checkpoint {
        sequence: u64,
        timestamp: DateTime<Utc>,
    },
    
    // VIPER-specific entries
    /// VIPER vector insertion with cluster prediction
    ViperVectorInsert {
        collection_id: String,
        vector_id: Uuid,
        vector_data: Vec<f32>,
        metadata: serde_json::Value,
        cluster_prediction: ClusterPrediction,
        storage_format: VectorStorageFormat,
        tier_level: TierLevel,
        expires_at: Option<DateTime<Utc>>,
        timestamp: DateTime<Utc>,
    },
    /// VIPER vector update with cluster reassignment
    ViperVectorUpdate {
        collection_id: String,
        vector_id: Uuid,
        old_cluster_id: ClusterId,
        new_cluster_id: ClusterId,
        updated_metadata: Option<serde_json::Value>,
        tier_level: TierLevel,
        expires_at: Option<DateTime<Utc>>,
        timestamp: DateTime<Utc>,
    },
    /// VIPER vector deletion (soft delete with expires_at)
    ViperVectorDelete {
        collection_id: String,
        vector_id: Uuid,
        cluster_id: ClusterId,
        tier_level: TierLevel,
        expires_at: DateTime<Utc>, // Set to current timestamp for soft delete
        timestamp: DateTime<Utc>,
    },
    /// VIPER cluster metadata update
    ViperClusterUpdate {
        collection_id: String,
        cluster_id: ClusterId,
        new_centroid: Vec<f32>,
        quality_metrics: ClusterQualityMetrics,
        timestamp: DateTime<Utc>,
    },
    /// VIPER partition creation
    ViperPartitionCreate {
        collection_id: String,
        partition_id: PartitionId,
        cluster_ids: Vec<ClusterId>,
        tier_level: TierLevel,
        timestamp: DateTime<Utc>,
    },
    /// VIPER compaction operation
    ViperCompactionOperation {
        collection_id: String,
        operation_id: String,
        input_partitions: Vec<PartitionId>,
        output_partitions: Vec<PartitionId>,
        tier_level: TierLevel,
        timestamp: DateTime<Utc>,
    },
    /// VIPER tier migration
    ViperTierMigration {
        collection_id: String,
        partition_id: PartitionId,
        from_tier: TierLevel,
        to_tier: TierLevel,
        migration_reason: String,
        timestamp: DateTime<Utc>,
    },
    /// VIPER ML model update
    ViperModelUpdate {
        collection_id: String,
        model_type: String,
        model_version: String,
        accuracy_metrics: serde_json::Value,
        timestamp: DateTime<Utc>,
    },
}

/// WAL segment file
#[derive(Debug)]
struct WalSegment {
    path: PathBuf,
    file: Mutex<File>,
    size: Arc<Mutex<u64>>,
    sequence_start: u64,
}

impl WalSegment {
    async fn new(path: PathBuf, sequence_start: u64) -> Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(&path)
            .await
            .map_err(StorageError::DiskIO)?;
        
        let metadata = file.metadata().await.map_err(StorageError::DiskIO)?;
        let size = Arc::new(Mutex::new(metadata.len()));
        
        Ok(Self {
            path,
            file: Mutex::new(file),
            size,
            sequence_start,
        })
    }
    
    async fn append(&self, entry: &WalEntry) -> Result<u64> {
        let serialized = bincode::serialize(entry)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;
        
        let mut file = self.file.lock().await;
        
        // Write entry length followed by entry data
        let entry_len = serialized.len() as u32;
        file.write_all(&entry_len.to_le_bytes()).await.map_err(StorageError::DiskIO)?;
        file.write_all(&serialized).await.map_err(StorageError::DiskIO)?;
        
        // Calculate CRC32 checksum
        let checksum = crc32fast::hash(&serialized);
        file.write_all(&checksum.to_le_bytes()).await.map_err(StorageError::DiskIO)?;
        
        file.flush().await.map_err(StorageError::DiskIO)?;
        
        let mut size = self.size.lock().await;
        *size += 4 + serialized.len() as u64 + 4; // length + data + checksum
        
        Ok(*size)
    }
    
    async fn read_all(&self) -> Result<Vec<WalEntry>> {
        let mut entries = Vec::new();
        let mut file = self.file.lock().await;
        file.seek(std::io::SeekFrom::Start(0)).await.map_err(StorageError::DiskIO)?;
        
        loop {
            // Read entry length
            let mut len_bytes = [0u8; 4];
            match file.read_exact(&mut len_bytes).await {
                Ok(_) => {},
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(StorageError::DiskIO(e)),
            }
            let entry_len = u32::from_le_bytes(len_bytes) as usize;
            
            // Read entry data
            let mut entry_data = vec![0u8; entry_len];
            file.read_exact(&mut entry_data).await.map_err(StorageError::DiskIO)?;
            
            // Read and verify checksum
            let mut checksum_bytes = [0u8; 4];
            file.read_exact(&mut checksum_bytes).await.map_err(StorageError::DiskIO)?;
            let stored_checksum = u32::from_le_bytes(checksum_bytes);
            let computed_checksum = crc32fast::hash(&entry_data);
            
            if stored_checksum != computed_checksum {
                return Err(StorageError::Corruption("WAL checksum mismatch".to_string()));
            }
            
            let entry: WalEntry = bincode::deserialize(&entry_data)
                .map_err(|e| StorageError::Serialization(e.to_string()))?;
            entries.push(entry);
        }
        
        Ok(entries)
    }
}

/// WAL configuration
#[derive(Debug, Clone)]
pub struct WalConfig {
    /// Directory to store WAL files
    pub wal_dir: PathBuf,
    /// Maximum size of a single WAL segment in bytes
    pub segment_size: u64,
    /// Sync mode: true for immediate fsync, false for batch
    pub sync_mode: bool,
    /// Number of segments to keep after checkpoint
    pub retention_segments: u32,
}

impl Default for WalConfig {
    fn default() -> Self {
        Self {
            wal_dir: PathBuf::from("./data/wal"),
            segment_size: 64 * 1024 * 1024, // 64MB
            sync_mode: true,
            retention_segments: 3,
        }
    }
}

/// Write-Ahead Log manager
#[derive(Debug)]
pub struct WalManager {
    config: WalConfig,
    current_segment: Arc<RwLock<WalSegment>>,
    segments: Arc<RwLock<Vec<PathBuf>>>,
    sequence: Arc<Mutex<u64>>,
}

impl WalManager {
    pub async fn new(config: WalConfig) -> Result<Self> {
        // Create WAL directory if it doesn't exist
        if !config.wal_dir.exists() {
            tokio::fs::create_dir_all(&config.wal_dir)
                .await
                .map_err(StorageError::DiskIO)?;
        }
        
        // Find existing segments
        let mut segments = Vec::new();
        let mut max_sequence = 0u64;
        
        let mut entries = tokio::fs::read_dir(&config.wal_dir)
            .await
            .map_err(StorageError::DiskIO)?;
        
        while let Some(entry) = entries.next_entry().await.map_err(StorageError::DiskIO)? {
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with("wal_") && name.ends_with(".log") {
                    if let Ok(seq) = name[4..name.len()-4].parse::<u64>() {
                        max_sequence = max_sequence.max(seq);
                        segments.push(path);
                    }
                }
            }
        }
        
        segments.sort();
        
        // Create or open current segment
        let sequence = max_sequence + 1;
        let current_path = config.wal_dir.join(format!("wal_{:010}.log", sequence));
        let current_segment = WalSegment::new(current_path, sequence).await?;
        
        Ok(Self {
            config,
            current_segment: Arc::new(RwLock::new(current_segment)),
            segments: Arc::new(RwLock::new(segments)),
            sequence: Arc::new(Mutex::new(sequence)),
        })
    }
    
    /// Write an entry to the WAL
    pub async fn append(&self, entry: WalEntry) -> Result<u64> {
        let current = self.current_segment.read().await;
        let size = current.append(&entry).await?;
        
        // Check if we need to rotate segments
        if size >= self.config.segment_size {
            drop(current);
            self.rotate_segment().await?;
        }
        
        let seq = self.sequence.lock().await;
        Ok(*seq)
    }
    
    /// Rotate to a new WAL segment
    async fn rotate_segment(&self) -> Result<()> {
        let mut sequence = self.sequence.lock().await;
        *sequence += 1;
        
        let new_path = self.config.wal_dir.join(format!("wal_{:010}.log", *sequence));
        let new_segment = WalSegment::new(new_path.clone(), *sequence).await?;
        
        let mut current = self.current_segment.write().await;
        let old_path = current.path.clone();
        *current = new_segment;
        
        let mut segments = self.segments.write().await;
        segments.push(old_path);
        
        // Clean up old segments if needed
        if segments.len() > self.config.retention_segments as usize {
            let to_remove = segments.len() - self.config.retention_segments as usize;
            for i in 0..to_remove {
                if let Err(e) = tokio::fs::remove_file(&segments[i]).await {
                    eprintln!("Failed to remove old WAL segment: {}", e);
                }
            }
            segments.drain(0..to_remove);
        }
        
        Ok(())
    }
    
    /// Read all entries from all WAL segments
    pub async fn read_all(&self) -> Result<Vec<WalEntry>> {
        let mut all_entries = Vec::new();
        
        // Read from all historical segments
        let segments = self.segments.read().await;
        for segment_path in segments.iter() {
            let segment = WalSegment::new(segment_path.clone(), 0).await?;
            let entries = segment.read_all().await?;
            all_entries.extend(entries);
        }
        
        // Read from current segment
        let current = self.current_segment.read().await;
        let entries = current.read_all().await?;
        all_entries.extend(entries);
        
        Ok(all_entries)
    }
    
    /// Create a checkpoint and clean up old segments
    pub async fn checkpoint(&self, sequence: u64) -> Result<()> {
        // Add checkpoint marker to current segment
        let checkpoint = WalEntry::Checkpoint {
            sequence,
            timestamp: Utc::now(),
        };
        self.append(checkpoint).await?;
        
        // Force rotation to new segment
        self.rotate_segment().await?;
        
        Ok(())
    }
    
    /// Replay WAL entries since a given sequence number
    pub async fn replay_since(&self, since_sequence: u64) -> Result<Vec<WalEntry>> {
        let all_entries = self.read_all().await?;
        
        // Find the last checkpoint before the requested sequence
        let mut last_checkpoint_idx = None;
        for (idx, entry) in all_entries.iter().enumerate() {
            if let WalEntry::Checkpoint { sequence, .. } = entry {
                if *sequence <= since_sequence {
                    last_checkpoint_idx = Some(idx);
                }
            }
        }
        
        // Return entries after the checkpoint
        let start_idx = last_checkpoint_idx.map(|idx| idx + 1).unwrap_or(0);
        Ok(all_entries[start_idx..].to_vec())
    }
    
    // VIPER-specific convenience methods
    
    /// Log VIPER vector insertion
    pub async fn log_viper_vector_insert(
        &self,
        collection_id: String,
        vector_id: Uuid,
        vector_data: Vec<f32>,
        metadata: serde_json::Value,
        cluster_prediction: ClusterPrediction,
        storage_format: VectorStorageFormat,
        tier_level: TierLevel,
    ) -> Result<u64> {
        let entry = WalEntry::ViperVectorInsert {
            collection_id,
            vector_id,
            vector_data,
            metadata,
            cluster_prediction,
            storage_format,
            tier_level,
            expires_at: None, // Default: no expiration
            timestamp: Utc::now(),
        };
        self.append(entry).await
    }
    
    /// Log VIPER vector update
    pub async fn log_viper_vector_update(
        &self,
        collection_id: String,
        vector_id: Uuid,
        old_cluster_id: ClusterId,
        new_cluster_id: ClusterId,
        updated_metadata: Option<serde_json::Value>,
        tier_level: TierLevel,
    ) -> Result<u64> {
        let entry = WalEntry::ViperVectorUpdate {
            collection_id,
            vector_id,
            old_cluster_id,
            new_cluster_id,
            updated_metadata,
            tier_level,
            expires_at: None, // Default: no expiration
            timestamp: Utc::now(),
        };
        self.append(entry).await
    }
    
    /// Log VIPER vector deletion
    pub async fn log_viper_vector_delete(
        &self,
        collection_id: String,
        vector_id: Uuid,
        cluster_id: ClusterId,
        tier_level: TierLevel,
    ) -> Result<u64> {
        let entry = WalEntry::ViperVectorDelete {
            collection_id,
            vector_id,
            cluster_id,
            tier_level,
            expires_at: Utc::now(), // Soft delete: immediately expired
            timestamp: Utc::now(),
        };
        self.append(entry).await
    }
    
    /// Log VIPER cluster update
    pub async fn log_viper_cluster_update(
        &self,
        collection_id: String,
        cluster_id: ClusterId,
        new_centroid: Vec<f32>,
        quality_metrics: ClusterQualityMetrics,
    ) -> Result<u64> {
        let entry = WalEntry::ViperClusterUpdate {
            collection_id,
            cluster_id,
            new_centroid,
            quality_metrics,
            timestamp: Utc::now(),
        };
        self.append(entry).await
    }
    
    /// Log VIPER partition creation
    pub async fn log_viper_partition_create(
        &self,
        collection_id: String,
        partition_id: PartitionId,
        cluster_ids: Vec<ClusterId>,
        tier_level: TierLevel,
    ) -> Result<u64> {
        let entry = WalEntry::ViperPartitionCreate {
            collection_id,
            partition_id,
            cluster_ids,
            tier_level,
            timestamp: Utc::now(),
        };
        self.append(entry).await
    }
    
    /// Log VIPER compaction operation
    pub async fn log_viper_compaction_operation(
        &self,
        collection_id: String,
        operation_id: String,
        input_partitions: Vec<PartitionId>,
        output_partitions: Vec<PartitionId>,
        tier_level: TierLevel,
    ) -> Result<u64> {
        let entry = WalEntry::ViperCompactionOperation {
            collection_id,
            operation_id,
            input_partitions,
            output_partitions,
            tier_level,
            timestamp: Utc::now(),
        };
        self.append(entry).await
    }
    
    /// Log VIPER tier migration
    pub async fn log_viper_tier_migration(
        &self,
        collection_id: String,
        partition_id: PartitionId,
        from_tier: TierLevel,
        to_tier: TierLevel,
        migration_reason: String,
    ) -> Result<u64> {
        let entry = WalEntry::ViperTierMigration {
            collection_id,
            partition_id,
            from_tier,
            to_tier,
            migration_reason,
            timestamp: Utc::now(),
        };
        self.append(entry).await
    }
    
    /// Log VIPER ML model update
    pub async fn log_viper_model_update(
        &self,
        collection_id: String,
        model_type: String,
        model_version: String,
        accuracy_metrics: serde_json::Value,
    ) -> Result<u64> {
        let entry = WalEntry::ViperModelUpdate {
            collection_id,
            model_type,
            model_version,
            accuracy_metrics,
            timestamp: Utc::now(),
        };
        self.append(entry).await
    }
    
    /// Get VIPER-specific entries for a collection
    pub async fn get_viper_entries_for_collection(&self, collection_id: &str) -> Result<Vec<WalEntry>> {
        let all_entries = self.read_all().await?;
        let viper_entries = all_entries.into_iter()
            .filter(|entry| {
                match entry {
                    WalEntry::ViperVectorInsert { collection_id: cid, .. } |
                    WalEntry::ViperVectorUpdate { collection_id: cid, .. } |
                    WalEntry::ViperVectorDelete { collection_id: cid, .. } |
                    WalEntry::ViperClusterUpdate { collection_id: cid, .. } |
                    WalEntry::ViperPartitionCreate { collection_id: cid, .. } |
                    WalEntry::ViperCompactionOperation { collection_id: cid, .. } |
                    WalEntry::ViperTierMigration { collection_id: cid, .. } |
                    WalEntry::ViperModelUpdate { collection_id: cid, .. } => cid == collection_id,
                    _ => false,
                }
            })
            .collect();
        Ok(viper_entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::collections::HashMap;
    
    #[tokio::test]
    async fn test_wal_basic_operations() {
        let temp_dir = TempDir::new().unwrap();
        let config = WalConfig {
            wal_dir: temp_dir.path().to_path_buf(),
            segment_size: 1024,
            sync_mode: true,
            retention_segments: 2,
        };
        
        let wal = WalManager::new(config).await.unwrap();
        
        // Test append
        let entry = WalEntry::CreateCollection {
            collection_id: "test_collection".to_string(),
            timestamp: Utc::now(),
        };
        let seq = wal.append(entry.clone()).await.unwrap();
        assert_eq!(seq, 1);
        
        // Test read all
        let entries = wal.read_all().await.unwrap();
        assert_eq!(entries.len(), 1);
        
        // Test segment rotation
        for i in 0..10 {
            let entry = WalEntry::Put {
                collection_id: "test_collection".to_string(),
                record: VectorRecord {
                    id: Uuid::new_v4(),
                    collection_id: "test_collection".to_string(),
                    vector: vec![1.0; 128],
                    metadata: HashMap::new(),
                    timestamp: Utc::now(),
                    expires_at: None,
                },
                timestamp: Utc::now(),
            };
            wal.append(entry).await.unwrap();
        }
        
        let segments = wal.segments.read().await;
        assert!(segments.len() > 0);
    }
}