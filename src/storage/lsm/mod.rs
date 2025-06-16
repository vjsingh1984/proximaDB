use crate::core::{VectorRecord, VectorId, LsmConfig, CollectionId};
use crate::storage::{Result, WalManager};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use std::path::PathBuf;
use chrono::Utc;
use serde::{Serialize, Deserialize};

pub mod compaction;
pub use compaction::{CompactionManager, CompactionTask, CompactionPriority, CompactionStats};

/// Entry in the LSM tree that can be either a vector record or a tombstone
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LsmEntry {
    /// An active vector record
    Record(VectorRecord),
    /// A tombstone marking a deleted vector
    Tombstone {
        id: VectorId,
        collection_id: CollectionId,
        timestamp: chrono::DateTime<chrono::Utc>,
    },
}

#[derive(Debug)]
pub struct LsmTree {
    config: LsmConfig,
    collection_id: CollectionId,
    memtable: RwLock<BTreeMap<VectorId, LsmEntry>>,
    wal_manager: Arc<WalManager>,
    data_dir: PathBuf,
    compaction_manager: Option<Arc<CompactionManager>>,
}

impl LsmTree {
    pub fn new(
        config: &LsmConfig, 
        collection_id: CollectionId, 
        wal_manager: Arc<WalManager>, 
        data_dir: PathBuf,
        compaction_manager: Option<Arc<CompactionManager>>
    ) -> Self {
        Self {
            config: config.clone(),
            collection_id,
            memtable: RwLock::new(BTreeMap::new()),
            wal_manager,
            data_dir,
            compaction_manager,
        }
    }

    pub async fn put(&self, id: VectorId, record: VectorRecord) -> Result<()> {
        // Write to WAL first for durability using new WAL system
        let _sequence = self.wal_manager.insert(
            self.collection_id.clone(),
            id.clone(),
            record.clone()
        ).await.map_err(|e| crate::core::StorageError::WalError(e.to_string()))?;
        
        // Then write to memtable as a record entry
        let mut memtable = self.memtable.write().await;
        memtable.insert(id, LsmEntry::Record(record));
        
        // Check if memtable size exceeds threshold and flush to SST
        if memtable.len() * std::mem::size_of::<LsmEntry>() > (self.config.memtable_size_mb as usize * 1024 * 1024) {
            drop(memtable);
            self.flush().await?;
        }
        
        Ok(())
    }

    pub async fn get(&self, id: &VectorId) -> Result<Option<VectorRecord>> {
        let memtable = self.memtable.read().await;
        match memtable.get(id) {
            Some(LsmEntry::Record(record)) => Ok(Some(record.clone())),
            Some(LsmEntry::Tombstone { .. }) => Ok(None), // Deleted record
            None => Ok(None), // Record not found
        }
    }
    
    /// Mark a vector as deleted by inserting a tombstone
    pub async fn delete(&self, id: VectorId) -> Result<bool> {
        // Write to WAL first for durability using new WAL system
        let _sequence = self.wal_manager.delete(
            self.collection_id.clone(),
            id.clone()
        ).await.map_err(|e| crate::core::StorageError::WalError(e.to_string()))?;
        
        // Check if the record currently exists
        let exists = {
            let memtable = self.memtable.read().await;
            matches!(memtable.get(&id), Some(LsmEntry::Record(_)))
        };
        
        // Insert tombstone in memtable
        let mut memtable = self.memtable.write().await;
        let tombstone = LsmEntry::Tombstone {
            id: id.clone(),
            collection_id: self.collection_id.clone(),
            timestamp: Utc::now(),
        };
        memtable.insert(id, tombstone);
        
        // Check if memtable size exceeds threshold and flush to SST
        if memtable.len() * std::mem::size_of::<LsmEntry>() > (self.config.memtable_size_mb as usize * 1024 * 1024) {
            drop(memtable);
            self.flush().await?;
        }
        
        Ok(exists)
    }

    /// Check if a vector exists (including checking for tombstones)
    pub async fn exists(&self, id: &VectorId) -> Result<bool> {
        let memtable = self.memtable.read().await;
        Ok(matches!(memtable.get(id), Some(LsmEntry::Record(_))))
    }

    /// Force flush memtable to SST files
    pub async fn flush(&self) -> Result<()> {
        let mut memtable = self.memtable.write().await;
        
        if memtable.is_empty() {
            return Ok(());
        }
        
        // Create SST file path
        let sst_filename = format!("sst_{}_{}.sst", self.collection_id, Utc::now().timestamp());
        let sst_path = self.data_dir.join(&self.collection_id).join(sst_filename);
        
        // Ensure directory exists
        if let Some(parent) = sst_path.parent() {
            tokio::fs::create_dir_all(parent).await
                .map_err(|e| crate::core::StorageError::DiskIO(e))?;
        }
        
        // Serialize memtable to file
        let data = bincode::serialize(&*memtable)
            .map_err(|e| crate::core::StorageError::SerializationError(format!("Failed to serialize memtable: {}", e)))?;
        
        tokio::fs::write(&sst_path, data).await
            .map_err(|e| crate::core::StorageError::DiskIO(e))?;
        
        // Clear memtable
        memtable.clear();
        
        // Force flush WAL to ensure durability
        let _flush_result = self.wal_manager.flush(Some(&self.collection_id)).await
            .map_err(|e| crate::core::StorageError::WalError(e.to_string()))?;
        
        // Trigger compaction if manager is available
        if let Some(compaction_manager) = &self.compaction_manager {
            let task = CompactionTask {
                collection_id: self.collection_id.clone(),
                level: 0, // Start at level 0
                input_files: vec![sst_path.clone()],
                output_file: sst_path.with_extension("compacted.sst"),
                priority: CompactionPriority::Medium,
            };
            // For now, just log that we would trigger compaction
            tracing::debug!("Would trigger compaction for collection: {}", self.collection_id);
            // compaction_manager.add_task(task).await?;
        }
        
        Ok(())
    }

    /// Get approximate size of the memtable in bytes
    pub async fn memtable_size(&self) -> usize {
        let memtable = self.memtable.read().await;
        memtable.len() * std::mem::size_of::<LsmEntry>()
    }

    /// Get number of entries in memtable
    pub async fn memtable_len(&self) -> usize {
        let memtable = self.memtable.read().await;
        memtable.len()
    }
}