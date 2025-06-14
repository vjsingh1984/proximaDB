use crate::core::{VectorRecord, VectorId, LsmConfig, CollectionId};
use crate::storage::{Result, WalManager, WalEntry};
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
    wal: Arc<WalManager>,
    data_dir: PathBuf,
    compaction_manager: Option<Arc<CompactionManager>>,
}

impl LsmTree {
    pub fn new(
        config: &LsmConfig, 
        collection_id: CollectionId, 
        wal: Arc<WalManager>, 
        data_dir: PathBuf,
        compaction_manager: Option<Arc<CompactionManager>>
    ) -> Self {
        Self {
            config: config.clone(),
            collection_id,
            memtable: RwLock::new(BTreeMap::new()),
            wal,
            data_dir,
            compaction_manager,
        }
    }

    pub async fn put(&self, id: VectorId, record: VectorRecord) -> Result<()> {
        // Write to WAL first for durability
        let wal_entry = WalEntry::Put {
            collection_id: self.collection_id.clone(),
            record: record.clone(),
            timestamp: Utc::now(),
        };
        self.wal.append(wal_entry).await?;
        
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
        // Write to WAL first for durability
        let wal_entry = WalEntry::Delete {
            collection_id: self.collection_id.clone(),
            vector_id: id,
            timestamp: Utc::now(),
        };
        self.wal.append(wal_entry).await?;
        
        // Check if the record currently exists
        let exists = {
            let memtable = self.memtable.read().await;
            matches!(memtable.get(&id), Some(LsmEntry::Record(_)))
        };
        
        // Insert tombstone in memtable
        let mut memtable = self.memtable.write().await;
        let tombstone = LsmEntry::Tombstone {
            id,
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

    pub async fn flush(&self) -> Result<()> {
        let memtable = self.memtable.read().await;
        if memtable.is_empty() {
            return Ok(());
        }
        
        // Create SST file name based on timestamp
        let sst_name = format!("sst_{}.db", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0));
        let sst_path = self.data_dir.join(&self.collection_id).join(&sst_name);
        
        // Ensure directory exists
        if let Some(parent) = sst_path.parent() {
            tokio::fs::create_dir_all(parent).await
                .map_err(|e| crate::core::StorageError::DiskIO(e))?;
        }
        
        // Write memtable to SST file
        let mut sst_data = Vec::new();
        for (id, lsm_entry) in memtable.iter() {
            let entry = bincode::serialize(&(*id, lsm_entry))
                .map_err(|e| crate::core::StorageError::Serialization(e.to_string()))?;
            sst_data.extend_from_slice(&(entry.len() as u32).to_le_bytes());
            sst_data.extend_from_slice(&entry);
        }
        
        tokio::fs::write(&sst_path, sst_data).await
            .map_err(|e| crate::core::StorageError::DiskIO(e))?;
        
        // Clear the memtable after successful flush
        drop(memtable);
        let mut memtable = self.memtable.write().await;
        memtable.clear();
        
        // Write checkpoint to WAL
        let checkpoint_entry = WalEntry::Checkpoint {
            sequence: memtable.len() as u64,
            timestamp: Utc::now(),
        };
        self.wal.append(checkpoint_entry).await?;
        
        // Check if compaction is needed after successful flush
        self.maybe_schedule_compaction().await?;
        
        Ok(())
    }
    
    pub fn size_estimate(&self) -> usize {
        // Estimate memtable size
        std::mem::size_of::<VectorRecord>() * 100 // rough estimate
    }
    
    /// Check if compaction is needed and schedule if necessary
    async fn maybe_schedule_compaction(&self) -> Result<()> {
        if let Some(compaction_manager) = &self.compaction_manager {
            let collection_dir = self.data_dir.join(&self.collection_id);
            
            if let Some(task) = compaction_manager
                .check_compaction_needed(&collection_dir, &self.collection_id)
                .await? 
            {
                compaction_manager.schedule_compaction(task).await?;
            }
        }
        
        Ok(())
    }
}