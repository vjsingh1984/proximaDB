// MARKED FOR REMOVAL: This file uses assignment_service which is being removed
// Recovery should be handled through collection metadata
/*
// Copyright 2024 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

use anyhow::{anyhow, Context, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{RwLock, Semaphore};
use tracing::{debug, error, info, warn};

use crate::core::VectorRecord;
use crate::storage::persistence::filesystem::{Filesystem, FilesystemFactory};
use crate::storage::persistence::write_ahead_log::avro_serialization_strategy::AvroSerializationStrategy;
use crate::storage::persistence::write_ahead_log::bincode_serialization_strategy::BincodeSerializationStrategy;
use crate::storage::persistence::write_ahead_log::{OptimizedFormat, SerializationStrategy};

/// Parallel WAL recovery system using assignment service for multi-disk coordination
pub struct ParallelRecoverySystem {
    assignment_service: Arc<dyn AssignmentService>,
    filesystem_factory: Arc<FilesystemFactory>,
    recovery_stats: Arc<RwLock<RecoveryStats>>,
}

/// Recovery statistics for monitoring and diagnostics
#[derive(Debug, Default, Clone)]
pub struct RecoveryStats {
    pub total_collections: usize,
    pub successful_collections: usize,
    pub failed_collections: usize,
    pub total_vectors_recovered: usize,
    pub total_sequences_recovered: usize,
    pub total_files_processed: usize,
    pub total_bytes_processed: u64,
    pub recovery_duration_ms: u64,
    pub disk_stats: HashMap<String, DiskRecoveryStats>,
}

/// Per-disk recovery statistics
#[derive(Debug, Default, Clone)]
pub struct DiskRecoveryStats {
    pub disk_id: String,
    pub collections_recovered: usize,
    pub vectors_recovered: usize,
    pub files_processed: usize,
    pub bytes_processed: u64,
    pub errors: Vec<String>,
}

/// Recovered WAL data for a collection
#[derive(Debug, Clone)]
pub struct RecoveredWalData {
    pub collection_id: String,
    pub vectors: Vec<VectorRecord>,
    pub sequences: Vec<u64>,
    pub last_sequence: u64,
    pub file_count: usize,
    pub total_bytes: u64,
}

/// Recovery task for parallel execution
struct RecoveryTask {
    collection_id: String,
    wal_directory: String,
    disk_id: String,
    filesystem: Arc<dyn Filesystem>,
}

impl ParallelRecoverySystem {
    /// Create new parallel recovery system
    pub fn new(
        assignment_service: Arc<dyn AssignmentService>,
        filesystem_factory: Arc<FilesystemFactory>,
    ) -> Self {
        Self {
            assignment_service,
            filesystem_factory,
            recovery_stats: Arc::new(RwLock::new(RecoveryStats::default())),
        }
    }

    /// Recover all collections in parallel using assignment service discovery
    pub async fn recover_all_collections(&self) -> Result<RecoveryStats> {
        let start_time = std::time::Instant::now();
        info!("🔄 Starting parallel WAL recovery using assignment service");

        // Phase 1: Discover collections using assignment service
        let collections_by_disk = self.discover_collections_by_assignment().await
            .context("Failed to discover collections using assignment service")?;

        info!("📦 Discovered {} disks with collections", collections_by_disk.len());
        for (disk_id, collections) in &collections_by_disk {
            info!("   Disk {}: {} collections", disk_id, collections.len());
        }

        // Phase 2: Create recovery tasks
        let mut recovery_tasks = Vec::new();
        for (disk_id, collections) in collections_by_disk {
            for collection_id in collections {
                // Get WAL directory from assignment service
                let wal_assignment = self.assignment_service
                    .get_assignment(&collection_id)
                    .await;
                
                if let Some(assignment) = wal_assignment {
                    let filesystem = self.filesystem_factory
                        .get_filesystem(&assignment.wal_location)?;
                    
                    recovery_tasks.push(RecoveryTask {
                        collection_id,
                        wal_directory: assignment.wal_location,
                        disk_id: disk_id.clone(),
                        filesystem,
                    });
                } else {
                    warn!("⚠️ No assignment found for collection: {}", collection_id);
                }
            }
        }

        let total_tasks = recovery_tasks.len();
        info!("📋 Created {} recovery tasks", total_tasks);

        // Phase 3: Execute recovery in parallel (limit concurrency per disk)
        let max_concurrent_per_disk = 4;
        let semaphore = Arc::new(Semaphore::new(max_concurrent_per_disk));
        
        let mut recovery_handles = Vec::new();
        for task in recovery_tasks {
            let permit = semaphore.clone().acquire_owned().await?;
            let stats = self.recovery_stats.clone();
            
            let handle = tokio::spawn(async move {
                let result = Self::recover_collection_task(task).await;
                drop(permit); // Release semaphore
                
                // Update stats
                let mut stats = stats.write().await;
                match result {
                    Ok(recovered_data) => {
                        stats.successful_collections += 1;
                        stats.total_vectors_recovered += recovered_data.vectors.len();
                        stats.total_sequences_recovered += recovered_data.sequences.len();
                        stats.total_files_processed += recovered_data.file_count;
                        stats.total_bytes_processed += recovered_data.total_bytes;
                        
                        // Update per-disk stats
                        let disk_stats = stats.disk_stats
                            .entry(recovered_data.collection_id.clone())
                            .or_insert_with(DiskRecoveryStats::default);
                        disk_stats.collections_recovered += 1;
                        disk_stats.vectors_recovered += recovered_data.vectors.len();
                        disk_stats.files_processed += recovered_data.file_count;
                        disk_stats.bytes_processed += recovered_data.total_bytes;
                        
                        Ok(recovered_data)
                    }
                    Err(e) => {
                        stats.failed_collections += 1;
                        error!("❌ Recovery failed: {}", e);
                        Err(e)
                    }
                }
            });
            
            recovery_handles.push(handle);
        }

        // Wait for all recovery tasks to complete
        let results = futures::future::join_all(recovery_handles).await;
        
        // Collect successful recoveries
        let mut all_recovered_data = Vec::new();
        for result in results {
            if let Ok(Ok(recovered_data)) = result {
                all_recovered_data.push(recovered_data);
            }
        }

        // Final stats update
        let mut final_stats = self.recovery_stats.write().await;
        final_stats.total_collections = total_tasks;
        final_stats.recovery_duration_ms = start_time.elapsed().as_millis() as u64;

        info!("✅ Parallel recovery complete:");
        info!("   Total collections: {}", final_stats.total_collections);
        info!("   Successful: {}", final_stats.successful_collections);
        info!("   Failed: {}", final_stats.failed_collections);
        info!("   Vectors recovered: {}", final_stats.total_vectors_recovered);
        info!("   Duration: {}ms", final_stats.recovery_duration_ms);

        Ok(final_stats.clone())
    }

    /// Discover collections using assignment service
    async fn discover_collections_by_assignment(&self) -> Result<HashMap<String, Vec<String>>> {
        let mut collections_by_disk: HashMap<String, Vec<String>> = HashMap::new();
        
        // Get all storage locations from assignment service
        let storage_locations = self.assignment_service
            .get_storage_locations()
            .await?;
        
        for location in storage_locations {
            let disk_id = Self::extract_disk_id(&location);
            
            // Get collections assigned to this location
            let collections = self.assignment_service
                .get_collections_at_location(&location)
                .await?;
            
            if !collections.is_empty() {
                collections_by_disk.insert(disk_id, collections);
            }
        }
        
        Ok(collections_by_disk)
    }

    /// Recover a single collection (executed in parallel)
    async fn recover_collection_task(task: RecoveryTask) -> Result<RecoveredWalData> {
        debug!("🔄 Recovering collection {} from {}", task.collection_id, task.wal_directory);
        
        let mut all_vectors = Vec::new();
        let mut all_sequences = Vec::new();
        let mut file_count = 0;
        let mut total_bytes = 0u64;
        let mut last_sequence = 0u64;

        // List WAL files in the directory
        let wal_files = task.filesystem
            .list_files(&format!("{}/logs", task.wal_directory))
            .await?;
        
        debug!("📁 Found {} WAL files for collection {}", wal_files.len(), task.collection_id);

        // Process each WAL file
        for wal_file in wal_files {
            if !wal_file.ends_with(".wal") {
                continue;
            }

            // Read file
            let file_data = task.filesystem
                .read_file(&wal_file)
                .await
                .context(format!("Failed to read WAL file: {}", wal_file))?;
            
            total_bytes += file_data.len() as u64;
            file_count += 1;

            // Detect format and deserialize
            let format = Self::detect_format(&file_data);
            let (vectors, sequences) = Self::deserialize_wal_data(&file_data, format)
                .context(format!("Failed to deserialize WAL file: {}", wal_file))?;

            // Track highest sequence number
            if let Some(&max_seq) = sequences.iter().max() {
                last_sequence = last_sequence.max(max_seq);
            }

            all_vectors.extend(vectors);
            all_sequences.extend(sequences);
        }

        info!("✅ Recovered collection {}: {} vectors, {} files", 
            task.collection_id, all_vectors.len(), file_count);

        Ok(RecoveredWalData {
            collection_id: task.collection_id,
            vectors: all_vectors,
            sequences: all_sequences,
            last_sequence,
            file_count,
            total_bytes,
        })
    }

    /// Extract disk ID from storage path
    fn extract_disk_id(path: &str) -> String {
        // Extract disk identifier from path (e.g., "/mnt/disk1" -> "disk1")
        if let Some(disk_part) = path.split('/').filter(|s| s.contains_hash("disk")).next() {
            disk_part.to_string()
        } else {
            "default".to_string()
        }
    }

    /// Detect WAL file format from content
    fn detect_format(data: &[u8]) -> OptimizedFormat {
        if data.starts_with(b"OBJ\x01") {
            OptimizedFormat::Avro
        } else if data.starts_with(&[0xDE, 0xAD, 0xBE, 0xEF]) {
            OptimizedFormat::Bincode
        } else {
            OptimizedFormat::Json // Default fallback
        }
    }

    /// Deserialize WAL data based on format
    fn deserialize_wal_data(
        data: &[u8],
        format: OptimizedFormat,
    ) -> Result<(Vec<VectorRecord>, Vec<u64>)> {
        match format {
            OptimizedFormat::Avro => {
                let strategy = AvroSerializationStrategy;
                strategy.deserialize(data)
            }
            OptimizedFormat::Bincode => {
                let strategy = BincodeSerializationStrategy;
                strategy.deserialize(data)
            }
            OptimizedFormat::Json => {
                // JSON deserialization
                let json_str = std::str::from_utf8(data)?;
                let parsed: serde_json::Value = serde_json::from_str(json_str)?;
                
                // Extract vectors and sequences from JSON
                let vectors = parsed["vectors"]
                    .as_array()
                    .ok_or_else(|| anyhow!("Missing vectors field"))?
                    .iter()
                    .map(|v| serde_json::from_value(v.clone()))
                    .collect::<Result<Vec<VectorRecord>, _>>()?;
                
                let sequences = parsed["sequences"]
                    .as_array()
                    .ok_or_else(|| anyhow!("Missing sequences field"))?
                    .iter()
                    .map(|s| s.as_u64().unwrap_or(0))
                    .collect();
                
                Ok((vectors, sequences))
            }
        }
    }

    /// Get recovery progress (for monitoring)
    pub async fn get_progress(&self) -> RecoveryStats {
        self.recovery_stats.read().await.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_parallel_recovery() {
        // Test will be implemented when assignment service is refactored
    }

    #[tokio::test]
    async fn test_format_detection() {
        let avro_data = b"OBJ\x01test";
        assert!(matches!(
            ParallelRecoverySystem::detect_format(avro_data),
            OptimizedFormat::Avro
        ));

        let bincode_data = &[0xDE, 0xAD, 0xBE, 0xEF, 0x00];
        assert!(matches!(
            ParallelRecoverySystem::detect_format(bincode_data),
            OptimizedFormat::Bincode
        ));

        let json_data = b"{}";
        assert!(matches!(
            ParallelRecoverySystem::detect_format(json_data),
            OptimizedFormat::Json
        ));
    }
}
*/