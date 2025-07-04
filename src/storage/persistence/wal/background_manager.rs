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
        }
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
                collection_id_clone,
                start_time,
                current_memory_size
            );

            // TODO: Implement actual flush logic here
            // This would call the WAL manager's flush method

            // Simulate flush operation with detailed logging
            let flush_start = std::time::Instant::now();
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            let flush_duration = flush_start.elapsed();

            debug!(
                "🚿 [FLUSH] Collection: {}, Flush operation completed in: {:?}",
                collection_id_clone,
                flush_duration
            );

            let duration = start_time.elapsed();

            // Check if compaction is also needed after flush
            let needs_compaction =
                Self::should_trigger_compaction_after_flush(&collection_id_clone).await;

            if needs_compaction {
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
                    collection_id_clone,
                    compaction_start
                );

                // TODO: Implement actual compaction logic here
                tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

                let compaction_duration = compaction_start.elapsed();
                debug!(
                    "🔄 [COMPACTION] Collection: {}, Compaction operation completed in: {:?}",
                    collection_id_clone,
                    compaction_duration
                );

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
