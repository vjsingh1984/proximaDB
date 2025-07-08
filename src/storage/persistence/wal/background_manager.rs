// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Background Maintenance Manager for WAL
//!
//! Manages async flush and compaction operations triggered by write operations.
//! Ensures only one background task per collection to prevent race conditions.

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, warn};

use super::WalConfig;
use crate::core::CollectionId;

/// Background task status for a collection
#[derive(Debug, Clone, PartialEq)]
pub enum BackgroundTaskStatus {
    /// No background task running
    Idle,
    /// Flush operation in progress
    Flushing,
    /// Compaction operation in progress  
    Compacting,
    /// Both flush and compaction queued
    FlushAndCompact,
}

/// Background maintenance manager
pub struct BackgroundMaintenanceManager {
    /// Per-collection task status tracking
    collection_status: Arc<RwLock<HashMap<CollectionId, BackgroundTaskStatus>>>,

    /// Configuration
    config: Arc<WalConfig>,

    /// Statistics
    stats: Arc<Mutex<BackgroundMaintenanceStats>>,

    /// AXIS manager for IndexConfig-based indexing after operations
    axis_manager: Option<Arc<crate::index::axis::manager::AxisManager>>,

    /// WAL flush coordinator for atomic operations
    flush_coordinator: Option<Arc<super::flush_coordinator::WalFlushCoordinator>>,
}

/// Statistics for background maintenance operations
#[derive(Debug, Clone, Default)]
pub struct BackgroundMaintenanceStats {
    pub total_flush_operations: u64,
    pub total_compaction_operations: u64,
    pub flush_operations_skipped: u64,
    pub compaction_operations_skipped: u64,
    pub average_flush_duration_ms: f64,
    pub average_compaction_duration_ms: f64,
    pub concurrent_operations_prevented: u64,
}

impl BackgroundMaintenanceManager {
    /// Create new background maintenance manager
    pub fn new(config: Arc<WalConfig>) -> Self {
        Self {
            collection_status: Arc::new(RwLock::new(HashMap::new())),
            config,
            stats: Arc::new(Mutex::new(BackgroundMaintenanceStats::default())),
            axis_manager: None,
            flush_coordinator: None,
        }
    }

    /// Set AXIS manager for IndexConfig-based indexing
    pub fn set_axis_manager(&mut self, axis_manager: Arc<crate::index::axis::manager::AxisManager>) {
        self.axis_manager = Some(axis_manager);
        info!("🔗 BackgroundManager: AXIS manager registered for IndexConfig-based indexing");
    }

    /// Set flush coordinator for atomic operations
    pub fn set_flush_coordinator(&mut self, flush_coordinator: Arc<super::flush_coordinator::WalFlushCoordinator>) {
        self.flush_coordinator = Some(flush_coordinator);
        info!("🔗 BackgroundManager: Flush coordinator registered for atomic operations");
    }

    /// Trigger async flush for collection if not already running
    /// Returns true if flush was triggered, false if already running
    pub async fn trigger_flush_if_needed(
        &self,
        collection_id: &CollectionId,
        current_memory_size: usize,
    ) -> Result<bool> {
        let effective_config = self.config.effective_config_for_collection(collection_id);

        // Check if flush is needed based on size
        if current_memory_size < effective_config.memory_flush_size_bytes {
            return Ok(false);
        }

        // Check if background task is already running
        {
            let status_map = self.collection_status.read().await;
            if let Some(status) = status_map.get(collection_id) {
                match status {
                    BackgroundTaskStatus::Idle => {}
                    BackgroundTaskStatus::Flushing => {
                        debug!(
                            "🔄 Flush already in progress for collection {}, skipping",
                            collection_id
                        );
                        let mut stats = self.stats.lock().await;
                        stats.flush_operations_skipped += 1;
                        return Ok(false);
                    }
                    BackgroundTaskStatus::Compacting => {
                        // Upgrade to flush + compact
                        debug!(
                            "📈 Upgrading compaction to flush+compact for collection {}",
                            collection_id
                        );
                        drop(status_map);
                        let mut status_map = self.collection_status.write().await;
                        status_map
                            .insert(collection_id.clone(), BackgroundTaskStatus::FlushAndCompact);
                        return Ok(false);
                    }
                    BackgroundTaskStatus::FlushAndCompact => {
                        debug!(
                            "⏳ Flush+compact already queued for collection {}, skipping",
                            collection_id
                        );
                        let mut stats = self.stats.lock().await;
                        stats.flush_operations_skipped += 1;
                        return Ok(false);
                    }
                }
            }
        }

        // Set status to flushing
        {
            let mut status_map = self.collection_status.write().await;
            status_map.insert(collection_id.clone(), BackgroundTaskStatus::Flushing);
        }

        // Trigger async flush task
        let collection_id_clone = collection_id.clone();
        let status_map_clone = self.collection_status.clone();
        let stats_clone = self.stats.clone();
        let flush_coordinator = self.flush_coordinator.clone();
        let axis_manager = self.axis_manager.clone();

        tokio::spawn(async move {
            let start_time = std::time::Instant::now();

            info!(
                "🚿 [FLUSH] Starting background flush for collection {} (memory: {}MB, trigger_size: {}MB)",
                collection_id_clone,
                current_memory_size / (1024 * 1024),
                effective_config.memory_flush_size_bytes / (1024 * 1024)
            );

            debug!(
                "🚿 [FLUSH] Collection: {}, Start time: {:?}, Memory size: {} bytes",
                collection_id_clone, start_time, current_memory_size
            );

            // Execute coordinated flush using FlushCoordinator for atomic operations
            let flush_start = std::time::Instant::now();
            let flush_result = if let Some(ref flush_coordinator) = flush_coordinator {
                match flush_coordinator
                    .execute_coordinated_flush(
                        &collection_id_clone,
                        super::flush_coordinator::FlushDataSource::Memory,
                        None, // Use default engine selection
                        None, // WAL manager will be resolved internally
                    )
                    .await
                {
                    Ok(result) => {
                        info!(
                            "✅ [FLUSH] Coordinated flush successful for collection {}: {} entries, {} bytes, {} files",
                            collection_id_clone,
                            result.entries_flushed,
                            result.bytes_written,
                            result.files_created.len()
                        );
                        Some(result)
                    }
                    Err(e) => {
                        warn!(
                            "❌ [FLUSH] Coordinated flush failed for collection {}: {}",
                            collection_id_clone, e
                        );
                        None
                    }
                }
            } else {
                warn!(
                    "⚠️ [FLUSH] No flush coordinator available for collection {}, skipping flush",
                    collection_id_clone
                );
                None
            };

            let flush_duration = flush_start.elapsed();
            debug!(
                "🚿 [FLUSH] Collection: {}, Flush operation completed in: {:?}",
                collection_id_clone, flush_duration
            );

            let duration = start_time.elapsed();

            // Determine if compaction is needed and execute the complete cycle
            let needs_compaction = Self::should_trigger_compaction_after_flush(&collection_id_clone).await;
            let mut final_files_created = if let Some(ref result) = flush_result {
                result.files_created.clone()
            } else {
                Vec::new()
            };

            if needs_compaction && flush_result.is_some() {
                info!(
                    "🔄 [COMPACTION] Triggering compaction after flush for collection {}",
                    collection_id_clone
                );

                // Update status to compacting
                {
                    let mut status_map = status_map_clone.write().await;
                    status_map.insert(
                        collection_id_clone.clone(),
                        BackgroundTaskStatus::Compacting,
                    );
                }

                let compaction_start = std::time::Instant::now();
                debug!(
                    "🔄 [COMPACTION] Collection: {}, Compaction start time: {:?}",
                    collection_id_clone, compaction_start
                );

                // Execute compaction (TODO: integrate with actual storage engine compaction)
                // For now, simulate compaction - in production this would call storage engine compaction
                let compaction_result = Self::execute_compaction(&collection_id_clone).await;
                
                let compaction_duration = compaction_start.elapsed();
                
                match compaction_result {
                    Ok(compacted_files) => {
                        info!(
                            "✅ [COMPACTION] Compaction successful for collection {}: {} files created in {:?}",
                            collection_id_clone, compacted_files.len(), compaction_duration
                        );
                        // Update final files list with compacted files
                        final_files_created = compacted_files;
                    }
                    Err(e) => {
                        warn!(
                            "❌ [COMPACTION] Compaction failed for collection {}: {}",
                            collection_id_clone, e
                        );
                        // Keep original flush files if compaction failed
                    }
                }

                // Update stats
                {
                    let mut stats = stats_clone.lock().await;
                    stats.total_compaction_operations += 1;
                    let total_ops = stats.total_compaction_operations;
                    Self::update_average_duration(
                        &mut stats.average_compaction_duration_ms,
                        compaction_duration.as_millis() as f64,
                        total_ops,
                    );
                }

                info!(
                    "✅ [COMPACTION] Background compaction completed for collection {} in {}ms (files_before: TODO, files_after: TODO, size_reduction: TODO)",
                    collection_id_clone,
                    compaction_duration.as_millis()
                );
            }

            // CRITICAL: IndexConfig-based indexing AFTER complete flush-compaction cycle
            if let (Some(ref axis), Some(ref flush_result)) = (&axis_manager, &flush_result) {
                if flush_result.success && !final_files_created.is_empty() {
                    info!(
                        "🔄 [INDEXING] Starting IndexConfig-based indexing for collection {} after flush-compaction cycle",
                        collection_id_clone
                    );
                    
                    let indexing_start = std::time::Instant::now();
                    
                    // Extract vectors from flush result for indexing
                    // TODO: This should come from the actual flushed data, not simulated
                    let vectors_to_index = Vec::new(); // Placeholder - get from flush result
                    
                    match axis.handle_flushed_vectors(
                        &collection_id_clone,
                        vectors_to_index,
                        final_files_created.clone()
                    ).await {
                        Ok(()) => {
                            let indexing_duration = indexing_start.elapsed();
                            info!(
                                "✅ [INDEXING] IndexConfig-based indexing completed for collection {} in {:?}",
                                collection_id_clone, indexing_duration
                            );
                        }
                        Err(e) => {
                            warn!(
                                "⚠️ [INDEXING] IndexConfig-based indexing failed for collection {}: {}",
                                collection_id_clone, e
                            );
                            // Continue - flush/compaction was successful even if indexing failed
                        }
                    }
                } else {
                    info!(
                        "📋 [INDEXING] Skipping indexing for collection {} (no files created or flush failed)",
                        collection_id_clone
                    );
                }
            } else {
                info!(
                    "📋 [INDEXING] No AXIS manager or flush result available for collection {}, skipping indexing",
                    collection_id_clone
                );
            }

            // Reset status to idle
            {
                let mut status_map = status_map_clone.write().await;
                status_map.insert(collection_id_clone.clone(), BackgroundTaskStatus::Idle);
            }

            // Update stats
            {
                let mut stats = stats_clone.lock().await;
                stats.total_flush_operations += 1;
                let total_ops = stats.total_flush_operations;
                Self::update_average_duration(
                    &mut stats.average_flush_duration_ms,
                    duration.as_millis() as f64,
                    total_ops,
                );
            }

            info!(
                "✅ [FLUSH] Background flush completed for collection {} in {}ms (total_ops: {}, avg_duration: {:.2}ms)",
                collection_id_clone,
                duration.as_millis(),
                {
                    let stats = stats_clone.lock().await;
                    stats.total_flush_operations
                },
                {
                    let stats = stats_clone.lock().await;
                    stats.average_flush_duration_ms
                }
            );

            debug!(
                "🚿 [FLUSH] Collection: {}, End time: {:?}, Total duration: {:?}, Memory freed: {}MB",
                collection_id_clone,
                std::time::Instant::now(),
                duration,
                current_memory_size / (1024 * 1024)
            );
        });

        Ok(true)
    }

    /// Check if collection needs compaction based on file count and sizes
    async fn should_trigger_compaction_after_flush(_collection_id: &CollectionId) -> bool {
        // TODO: Implement compaction criteria check
        // This would check file count and average file sizes
        false
    }

    /// Execute compaction for a collection
    async fn execute_compaction(_collection_id: &CollectionId) -> Result<Vec<String>> {
        // TODO: Implement actual compaction logic by calling storage engine compaction
        // This should:
        // 1. Call storage engine compaction (VIPER or LSM)
        // 2. Return list of new files created after compaction
        // 3. Handle compaction errors gracefully
        
        // For now, simulate compaction
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
        
        // Return simulated compacted files
        Ok(vec![
            format!("compacted_{}_sst_001.parquet", _collection_id),
            format!("compacted_{}_sst_002.parquet", _collection_id),
        ])
    }

    /// Update moving average for duration tracking
    fn update_average_duration(current_avg: &mut f64, new_duration: f64, total_count: u64) {
        if total_count == 1 {
            *current_avg = new_duration;
        } else {
            let alpha = 0.1; // Smoothing factor for exponential moving average
            *current_avg = alpha * new_duration + (1.0 - alpha) * (*current_avg);
        }
    }

    /// Get current status for a collection
    pub async fn get_collection_status(
        &self,
        collection_id: &CollectionId,
    ) -> BackgroundTaskStatus {
        let status_map = self.collection_status.read().await;
        status_map
            .get(collection_id)
            .cloned()
            .unwrap_or(BackgroundTaskStatus::Idle)
    }

    /// Get maintenance statistics
    pub async fn get_stats(&self) -> BackgroundMaintenanceStats {
        let stats = self.stats.lock().await;
        stats.clone()
    }

    /// Check if any background operations are running
    pub async fn has_active_operations(&self) -> bool {
        let status_map = self.collection_status.read().await;
        status_map
            .values()
            .any(|status| *status != BackgroundTaskStatus::Idle)
    }

    /// Wait for all background operations to complete
    pub async fn wait_for_completion(&self) -> Result<()> {
        let mut check_count = 0;
        const MAX_CHECKS: u32 = 600; // 60 seconds with 100ms intervals

        while self.has_active_operations().await && check_count < MAX_CHECKS {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            check_count += 1;
        }

        if check_count >= MAX_CHECKS {
            warn!("Background operations did not complete within timeout");
        }

        Ok(())
    }

    /// Force stop all background operations (for shutdown)
    pub async fn shutdown(&self) -> Result<()> {
        info!("🛑 Shutting down background maintenance manager");

        // Clear all status tracking
        {
            let mut status_map = self.collection_status.write().await;
            status_map.clear();
        }

        Ok(())
    }
}
