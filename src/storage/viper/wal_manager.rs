// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! VIPER WAL Manager
//! 
//! Write-Ahead Log management for VIPER storage with multi-tier coordination
//! and ML-guided cluster prediction logging for fast recovery.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{RwLock, Mutex, mpsc, oneshot};
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncWriteExt, AsyncReadExt, AsyncSeekExt, BufWriter};
use chrono::{DateTime, Utc};
use anyhow::{Result, Context};
use serde::{Serialize, Deserialize};

use crate::core::{VectorId, CollectionId};
use super::types::*;
use super::ViperConfig;

/// VIPER WAL Manager with multi-tier coordination
pub struct ViperWalManager {
    /// Configuration
    config: ViperConfig,
    
    /// WAL files per storage location
    storage_wal_files: Arc<RwLock<HashMap<String, StorageWalFile>>>,
    
    /// WAL write queue
    write_queue: Arc<Mutex<VecDeque<WalWriteRequest>>>,
    
    /// Background writer task
    writer_handle: Option<tokio::task::JoinHandle<()>>,
    
    /// Shutdown signal
    shutdown_tx: Option<mpsc::Sender<()>>,
    
    /// WAL statistics
    stats: Arc<RwLock<WalStats>>,
    
    /// Checkpoint manager
    checkpoint_manager: Arc<CheckpointManager>,
}

/// WAL file for a specific storage location
#[derive(Debug)]
pub struct StorageWalFile {
    /// Storage URL
    pub storage_url: String,
    
    /// WAL file path
    pub file_path: PathBuf,
    
    /// File handle
    pub file_handle: Arc<Mutex<BufWriter<File>>>,
    
    /// Current file size
    pub current_size: Arc<Mutex<u64>>,
    
    /// Last sequence number
    pub last_sequence: Arc<Mutex<u64>>,
    
    /// File creation time
    pub created_at: DateTime<Utc>,
}

/// WAL write request
#[derive(Debug)]
pub struct WalWriteRequest {
    /// Request ID for tracking
    pub request_id: String,
    
    /// Collection ID
    pub collection_id: CollectionId,
    
    /// Target storage URL
    pub storage_url: String,
    
    /// WAL entry to write
    pub entry: WalEntry,
    
    /// Response channel
    pub response_tx: oneshot::Sender<Result<u64>>,
    
    /// Request timestamp
    pub timestamp: DateTime<Utc>,
}

/// WAL entry types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WalEntry {
    /// Vector insertion with cluster prediction
    VectorInsert {
        vector_id: VectorId,
        vector_data: Vec<f32>,
        metadata: serde_json::Value,
        cluster_prediction: ClusterPrediction,
        storage_format: VectorStorageFormat,
    },
    
    /// Vector update
    VectorUpdate {
        vector_id: VectorId,
        old_cluster_id: ClusterId,
        new_cluster_id: ClusterId,
        updated_metadata: Option<serde_json::Value>,
    },
    
    /// Vector deletion
    VectorDelete {
        vector_id: VectorId,
        cluster_id: ClusterId,
    },
    
    /// Cluster metadata update
    ClusterUpdate {
        cluster_id: ClusterId,
        new_centroid: Vec<f32>,
        quality_metrics: ClusterQualityMetrics,
    },
    
    /// Partition creation
    PartitionCreate {
        partition_id: PartitionId,
        cluster_ids: Vec<ClusterId>,
        storage_layout: super::partitioner::StorageLayout,
    },
    
    /// Compaction operation
    CompactionOperation {
        operation_id: String,
        input_partitions: Vec<PartitionId>,
        output_partitions: Vec<PartitionId>,
        optimization_applied: CompactionOptimizationType,
    },
    
    /// Tier migration
    TierMigration {
        partition_id: PartitionId,
        from_tier: String,
        to_tier: String,
        migration_reason: MigrationReason,
    },
    
    /// ML model update
    ModelUpdate {
        collection_id: CollectionId,
        model_type: ModelType,
        model_version: String,
        accuracy_metrics: ModelAccuracyMetrics,
    },
}

/// Compaction optimization types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompactionOptimizationType {
    FileMerging,
    Reclustering,
    FeatureReorganization,
    CompressionOptimization,
    TierMigration,
}

/// Migration reasons for audit trail
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MigrationReason {
    Age,
    AccessFrequency,
    StorageCapacity,
    CostOptimization,
    ManualTrigger,
}

/// ML model types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ModelType {
    ClusterPrediction,
    FeatureImportance,
    CompressionSelection,
    AccessPattern,
}

/// Model accuracy metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelAccuracyMetrics {
    pub overall_accuracy: f32,
    pub precision: f32,
    pub recall: f32,
    pub f1_score: f32,
    pub training_samples: usize,
}

/// WAL record header
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalRecordHeader {
    /// Sequence number
    pub sequence: u64,
    
    /// Record size in bytes
    pub size: u32,
    
    /// Checksum
    pub checksum: u32,
    
    /// Timestamp
    pub timestamp: DateTime<Utc>,
    
    /// Collection ID
    pub collection_id: CollectionId,
    
    /// Tier level
    pub tier_level: String,
}

/// WAL statistics
#[derive(Debug, Default, Clone)]
pub struct WalStats {
    /// Total records written
    pub total_records: u64,
    
    /// Records per tier
    pub tier_records: HashMap<String, u64>,
    
    /// Total bytes written
    pub total_bytes: u64,
    
    /// Write latency statistics
    pub avg_write_latency_ms: f32,
    pub max_write_latency_ms: f32,
    
    /// Queue statistics
    pub queue_depth: usize,
    pub max_queue_depth: usize,
    
    /// Error counts
    pub write_errors: u64,
    pub recovery_errors: u64,
}

/// Checkpoint manager for WAL cleanup
pub struct CheckpointManager {
    /// Checkpoint interval
    checkpoint_interval: std::time::Duration,
    
    /// Last checkpoint per tier
    last_checkpoints: Arc<RwLock<HashMap<String, CheckpointInfo>>>,
    
    /// Checkpoint task handle
    task_handle: Option<tokio::task::JoinHandle<()>>,
}

/// Checkpoint information
#[derive(Debug, Clone)]
pub struct CheckpointInfo {
    pub checkpoint_id: String,
    pub sequence_number: u64,
    pub timestamp: DateTime<Utc>,
    pub file_path: PathBuf,
    pub validated: bool,
}

/// WAL recovery information
#[derive(Debug)]
pub struct WalRecoveryInfo {
    /// Number of records recovered
    pub records_recovered: u64,
    
    /// Recovery duration
    pub recovery_duration: std::time::Duration,
    
    /// Tiers recovered
    pub tiers_recovered: Vec<String>,
    
    /// Any errors encountered
    pub errors: Vec<String>,
    
    /// Recovery strategy used
    pub strategy: RecoveryStrategy,
}

/// Recovery strategies
#[derive(Debug, Clone)]
pub enum RecoveryStrategy {
    /// Full recovery from beginning
    Full,
    
    /// Recovery from last checkpoint
    FromCheckpoint { checkpoint_id: String },
    
    /// Partial recovery with best effort
    BestEffort { start_sequence: u64 },
}

impl ViperWalManager {
    /// Create new VIPER WAL manager
    pub async fn new(config: ViperConfig) -> Result<Self> {
        let wal_dir = config.storage_root.join("wal");
        tokio::fs::create_dir_all(&wal_dir).await
            .context("Failed to create WAL directory")?;
        
        let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
        
        let checkpoint_manager = Arc::new(CheckpointManager::new(
            std::time::Duration::from_secs(300) // 5 minutes
        ));
        
        let mut manager = Self {
            config,
            tier_wal_files: Arc::new(RwLock::new(HashMap::new())),
            write_queue: Arc::new(Mutex::new(VecDeque::new())),
            writer_handle: None,
            shutdown_tx: Some(shutdown_tx),
            stats: Arc::new(RwLock::new(WalStats::default())),
            checkpoint_manager,
        };
        
        // Initialize WAL files for each tier
        manager.initialize_tier_wal_files().await?;
        
        // Start background writer
        manager.start_background_writer(shutdown_rx).await?;
        
        Ok(manager)
    }
    
    /// Write a WAL entry
    pub async fn write_entry(
        &self,
        collection_id: CollectionId,
        tier_level: String,
        entry: WalEntry,
    ) -> Result<u64> {
        let (response_tx, response_rx) = oneshot::channel();
        
        let request = WalWriteRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            collection_id,
            tier_level,
            entry,
            response_tx,
            timestamp: Utc::now(),
        };
        
        // Queue the write request
        {
            let mut queue = self.write_queue.lock().await;
            queue.push_back(request);
            
            // Update queue statistics
            let mut stats = self.stats.write().await;
            stats.queue_depth = queue.len();
            if queue.len() > stats.max_queue_depth {
                stats.max_queue_depth = queue.len();
            }
        }
        
        // Wait for completion
        response_rx.await
            .context("WAL write request canceled")?
    }
    
    /// Recover from WAL files
    pub async fn recover(&self, strategy: RecoveryStrategy) -> Result<WalRecoveryInfo> {
        let start_time = std::time::Instant::now();
        let recovery_info = match strategy {
            RecoveryStrategy::Full => {
                self.recover_full().await?
            }
            
            RecoveryStrategy::FromCheckpoint { checkpoint_id } => {
                self.recover_from_checkpoint(&checkpoint_id).await?
            }
            
            RecoveryStrategy::BestEffort { start_sequence } => {
                self.recover_best_effort(start_sequence).await?
            }
        };
        
        // Update recovery duration
        let mut final_info = recovery_info;
        final_info.recovery_duration = start_time.elapsed();
        
        Ok(final_info)
    }
    
    /// Create checkpoint for WAL cleanup
    pub async fn create_checkpoint(&self, tier_level: String) -> Result<CheckpointInfo> {
        self.checkpoint_manager.create_checkpoint(tier_level).await
    }
    
    /// Get WAL statistics
    pub async fn get_stats(&self) -> WalStats {
        (*self.stats.read().await).clone()
    }
    
    /// Initialize WAL files for each configured tier
    async fn initialize_tier_wal_files(&mut self) -> Result<()> {
        let mut tier_files = self.tier_wal_files.write().await;
        
        for (_, tier_def) in &self.config.tier_config.tiers {
            let tier_level = tier_def.level.clone();
            let wal_file_path = self.config.storage_root
                .join("wal")
                .join(format!("{:?}_wal.log", tier_level));
            
            // Create or open WAL file
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&wal_file_path)
                .await
                .context("Failed to open WAL file")?;
            
            let tier_wal = StorageWalFile {
                storage_url: tier_level.clone(),
                file_path: wal_file_path,
                file_handle: Arc::new(Mutex::new(BufWriter::new(file))),
                current_size: Arc::new(Mutex::new(0)),
                last_sequence: Arc::new(Mutex::new(0)),
                created_at: Utc::now(),
            };
            
            tier_files.insert(tier_level, tier_wal);
        }
        
        Ok(())
    }
    
    /// Start background writer task
    async fn start_background_writer(&mut self, mut shutdown_rx: mpsc::Receiver<()>) -> Result<()> {
        let write_queue = self.write_queue.clone();
        let tier_wal_files = self.tier_wal_files.clone();
        let stats = self.stats.clone();
        
        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown_rx.recv() => {
                        break;
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {
                        // Process write queue
                        if let Ok(mut queue) = write_queue.try_lock() {
                            if let Some(request) = queue.pop_front() {
                                drop(queue);
                                
                                let _result = Self::process_write_request(
                                    request,
                                    &tier_wal_files,
                                    &stats,
                                ).await;
                                
                                // Update queue depth
                                if let Ok(queue) = write_queue.try_lock() {
                                    if let Ok(mut stats) = stats.try_write() {
                                        stats.queue_depth = queue.len();
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });
        
        self.writer_handle = Some(handle);
        
        Ok(())
    }
    
    /// Process a single write request
    async fn process_write_request(
        request: WalWriteRequest,
        tier_wal_files: &Arc<RwLock<HashMap<String, StorageWalFile>>>,
        stats: &Arc<RwLock<WalStats>>,
    ) {
        let start_time = std::time::Instant::now();
        
        let result = async {
            let tier_files = tier_wal_files.read().await;
            let tier_wal = tier_files.get(&request.storage_url)
                .ok_or_else(|| anyhow::anyhow!("No WAL file for storage: {:?}", request.storage_url))?;
            
            // Get next sequence number
            let sequence = {
                let mut last_seq = tier_wal.last_sequence.lock().await;
                *last_seq += 1;
                *last_seq
            };
            
            // Serialize the entry
            let entry_data = bincode::serialize(&request.entry)
                .context("Failed to serialize WAL entry")?;
            
            // Create record header
            let header = WalRecordHeader {
                sequence,
                size: entry_data.len() as u32,
                checksum: crc32fast::hash(&entry_data),
                timestamp: request.timestamp,
                collection_id: request.collection_id,
                storage_url: request.storage_url,
            };
            
            // Serialize header
            let header_data = bincode::serialize(&header)
                .context("Failed to serialize WAL header")?;
            
            // Write to file
            {
                let mut file_handle = tier_wal.file_handle.lock().await;
                
                // Write header length (4 bytes)
                file_handle.write_u32(header_data.len() as u32).await
                    .context("Failed to write header length")?;
                
                // Write header
                file_handle.write_all(&header_data).await
                    .context("Failed to write header")?;
                
                // Write entry data
                file_handle.write_all(&entry_data).await
                    .context("Failed to write entry data")?;
                
                // Flush to ensure durability
                file_handle.flush().await
                    .context("Failed to flush WAL file")?;
            }
            
            // Update file size
            {
                let mut size = tier_wal.current_size.lock().await;
                *size += (4 + header_data.len() + entry_data.len()) as u64;
            }
            
            Ok::<u64, anyhow::Error>(sequence)
        }.await;
        
        // Update statistics
        let elapsed_ms = start_time.elapsed().as_millis() as f32;
        if let Ok(mut stats) = stats.try_write() {
            stats.total_records += 1;
            *stats.tier_records.entry(request.storage_url).or_insert(0) += 1;
            
            // Update latency with exponential moving average
            let alpha = 0.1;
            stats.avg_write_latency_ms = stats.avg_write_latency_ms * (1.0 - alpha) + elapsed_ms * alpha;
            if elapsed_ms > stats.max_write_latency_ms {
                stats.max_write_latency_ms = elapsed_ms;
            }
            
            if result.is_err() {
                stats.write_errors += 1;
            }
        }
        
        // Send response
        let _ = request.response_tx.send(result);
    }
    
    /// Full recovery from WAL files
    async fn recover_full(&self) -> Result<WalRecoveryInfo> {
        let mut recovery_info = WalRecoveryInfo {
            records_recovered: 0,
            recovery_duration: std::time::Duration::default(),
            tiers_recovered: Vec::new(),
            errors: Vec::new(),
            strategy: RecoveryStrategy::Full,
        };
        
        let tier_files = self.tier_wal_files.read().await;
        
        for (tier_level, tier_wal) in tier_files.iter() {
            match self.recover_tier_wal(tier_level, &tier_wal.file_path, 0).await {
                Ok(records) => {
                    recovery_info.records_recovered += records;
                    recovery_info.tiers_recovered.push(tier_level.clone());
                }
                Err(e) => {
                    recovery_info.errors.push(format!("Tier {:?}: {}", tier_level, e));
                }
            }
        }
        
        Ok(recovery_info)
    }
    
    /// Recovery from checkpoint
    async fn recover_from_checkpoint(&self, _checkpoint_id: &str) -> Result<WalRecoveryInfo> {
        // Implementation would load checkpoint and recover from that point
        // For now, fall back to full recovery
        self.recover_full().await
    }
    
    /// Best effort recovery
    async fn recover_best_effort(&self, _start_sequence: u64) -> Result<WalRecoveryInfo> {
        // Implementation would start recovery from specific sequence number
        // For now, fall back to full recovery
        self.recover_full().await
    }
    
    /// Recover a single tier WAL file
    async fn recover_tier_wal(
        &self,
        _tier_level: &String,
        file_path: &Path,
        start_sequence: u64,
    ) -> Result<u64> {
        let mut file = File::open(file_path).await
            .context("Failed to open WAL file for recovery")?;
        
        let mut records_recovered = 0u64;
        let mut _position = 0u64;
        
        loop {
            // Read header length
            let mut header_len_buf = [0u8; 4];
            match file.read_exact(&mut header_len_buf).await {
                Ok(_) => {}
                Err(ref e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    break; // End of file
                }
                Err(e) => return Err(e.into()),
            }
            
            let header_len = u32::from_be_bytes(header_len_buf) as usize;
            _position += 4;
            
            // Read header
            let mut header_buf = vec![0u8; header_len];
            file.read_exact(&mut header_buf).await
                .context("Failed to read WAL header")?;
            _position += header_len as u64;
            
            let header: WalRecordHeader = bincode::deserialize(&header_buf)
                .context("Failed to deserialize WAL header")?;
            
            // Skip if before start sequence
            if header.sequence < start_sequence {
                file.seek(std::io::SeekFrom::Current(header.size as i64)).await
                    .context("Failed to seek past WAL entry")?;
                _position += header.size as u64;
                continue;
            }
            
            // Read entry data
            let mut entry_buf = vec![0u8; header.size as usize];
            file.read_exact(&mut entry_buf).await
                .context("Failed to read WAL entry data")?;
            _position += header.size as u64;
            
            // Verify checksum
            let calculated_checksum = crc32fast::hash(&entry_buf);
            if calculated_checksum != header.checksum {
                return Err(anyhow::anyhow!(
                    "WAL checksum mismatch at sequence {}: expected {}, got {}",
                    header.sequence,
                    header.checksum,
                    calculated_checksum
                ));
            }
            
            // Deserialize and apply entry
            let entry: WalEntry = bincode::deserialize(&entry_buf)
                .context("Failed to deserialize WAL entry")?;
            
            self.apply_wal_entry(&header, &entry).await?;
            records_recovered += 1;
        }
        
        Ok(records_recovered)
    }
    
    /// Apply a WAL entry during recovery
    async fn apply_wal_entry(&self, _header: &WalRecordHeader, entry: &WalEntry) -> Result<()> {
        // Implementation would apply the WAL entry to restore state
        // This would involve recreating the exact state that was logged
        
        match entry {
            WalEntry::VectorInsert { vector_id, .. } => {
                // Restore vector insertion
                println!("Recovering vector insert: {}", vector_id);
            }
            
            WalEntry::VectorUpdate { vector_id, .. } => {
                // Restore vector update
                println!("Recovering vector update: {}", vector_id);
            }
            
            WalEntry::VectorDelete { vector_id, .. } => {
                // Restore vector deletion
                println!("Recovering vector delete: {}", vector_id);
            }
            
            WalEntry::ClusterUpdate { cluster_id, .. } => {
                // Restore cluster metadata
                println!("Recovering cluster update: {}", cluster_id);
            }
            
            WalEntry::PartitionCreate { partition_id, .. } => {
                // Restore partition creation
                println!("Recovering partition create: {}", partition_id);
            }
            
            WalEntry::CompactionOperation { operation_id, .. } => {
                // Restore compaction state
                println!("Recovering compaction: {}", operation_id);
            }
            
            WalEntry::TierMigration { partition_id, .. } => {
                // Restore migration state
                println!("Recovering migration: {}", partition_id);
            }
            
            WalEntry::ModelUpdate { collection_id, .. } => {
                // Restore ML model state
                println!("Recovering model update: {}", collection_id);
            }
        }
        
        Ok(())
    }
}

impl CheckpointManager {
    fn new(checkpoint_interval: std::time::Duration) -> Self {
        Self {
            checkpoint_interval,
            last_checkpoints: Arc::new(RwLock::new(HashMap::new())),
            task_handle: None,
        }
    }
    
    async fn create_checkpoint(&self, tier_level: String) -> Result<CheckpointInfo> {
        let checkpoint_id = uuid::Uuid::new_v4().to_string();
        let timestamp = Utc::now();
        let file_path = PathBuf::from(format!("checkpoint_{}_{}.chkpt", tier_level as u8, checkpoint_id));
        
        // Create checkpoint file
        // Implementation would create a consistent snapshot
        
        let checkpoint_info = CheckpointInfo {
            checkpoint_id: checkpoint_id.clone(),
            sequence_number: 0, // Would be actual sequence number
            timestamp,
            file_path,
            validated: true,
        };
        
        // Update checkpoint registry
        let mut checkpoints = self.last_checkpoints.write().await;
        checkpoints.insert(tier_level, checkpoint_info.clone());
        
        Ok(checkpoint_info)
    }
}

impl Drop for ViperWalManager {
    fn drop(&mut self) {
        // Send shutdown signal
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.try_send(());
        }
    }
}