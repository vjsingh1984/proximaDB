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

//! # Background Compaction for Disk-Based Graph Storage
//!
//! This module implements background compaction to optimize disk space usage
//! and improve performance by defragmenting the memory-mapped graph files.
//!
//! ## Compaction Goals
//!
//! - **Defragmentation**: Reduce fragmentation in mmap files
//! - **Space Reclamation**: Reclaim space from deleted nodes/edges
//! - **Performance**: Optimize file layout for better cache utilization
//! - **Non-Blocking**: Run compaction without blocking graph operations
//!
//! ## Compaction Strategy
//!
//! 1. **Mark-Sweep**: Identify deleted/unused entries
//! 2. **Copy-Compact**: Rewrite mmap files with only live data
//! 3. **Atomic Swap**: Replace old files with compacted versions
//! 4. **Cleanup**: Remove old files after successful swap

use crate::core::error::ProximaDBError;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::info;

type Result<T> = std::result::Result<T, ProximaDBError>;

/// Configuration for background compaction
#[derive(Debug, Clone)]
pub struct CompactionConfig {
    /// Minimum fragmentation percentage before triggering compaction
    pub min_fragmentation_percent: u8,

    /// Minimum free space percentage before triggering compaction
    pub min_free_space_percent: u8,

    /// Compaction interval in seconds
    pub compaction_interval_secs: u64,

    /// Maximum compaction duration in seconds
    pub max_compaction_duration_secs: u64,

    /// Enable automatic compaction
    pub auto_compaction: bool,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            min_fragmentation_percent: 30,     // Trigger when >30% fragmented
            min_free_space_percent: 20,        // Trigger when <20% free space
            compaction_interval_secs: 3600,    // Run every hour
            max_compaction_duration_secs: 300, // Max 5 minutes per run
            auto_compaction: true,
        }
    }
}

/// Statistics from a compaction run
#[derive(Debug, Clone)]
pub struct CompactionStats {
    /// Bytes before compaction
    pub bytes_before: u64,

    /// Bytes after compaction
    pub bytes_after: u64,

    /// Space saved in bytes
    pub space_saved: u64,

    /// Number of nodes compacted
    pub nodes_compacted: u64,

    /// Number of edges compacted
    pub edges_compacted: u64,

    /// Duration of compaction in milliseconds
    pub duration_ms: u64,

    /// Fragmentation percentage before compaction
    pub fragmentation_before: f64,

    /// Fragmentation percentage after compaction
    pub fragmentation_after: f64,
}

impl CompactionStats {
    /// Calculate space savings percentage
    pub fn savings_percent(&self) -> f64 {
        if self.bytes_before == 0 {
            0.0
        } else {
            ((self.bytes_before - self.bytes_after) as f64 / self.bytes_before as f64) * 100.0
        }
    }

    /// Check if target fragmentation reduction was achieved
    pub fn target_achieved(&self, target_fragmentation: f64) -> bool {
        self.fragmentation_after < target_fragmentation
    }
}

/// Background compaction manager for disk-based graph storage
pub struct CompactionManager {
    /// Compaction configuration
    config: CompactionConfig,

    /// Storage directory
    storage_dir: PathBuf,

    /// Compaction task handle (for cancellation)
    compaction_task: Option<tokio::task::JoinHandle<()>>,

    /// Running flag
    running: Arc<std::sync::atomic::AtomicBool>,
}

impl CompactionManager {
    /// Create a new compaction manager
    pub fn new(storage_dir: PathBuf, config: CompactionConfig) -> Self {
        Self {
            config,
            storage_dir,
            compaction_task: None,
            running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Start background compaction task
    pub async fn start(&mut self) -> Result<()> {
        if self.running.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(()); // Already running
        }

        self.running
            .store(true, std::sync::atomic::Ordering::SeqCst);

        let storage_dir = self.storage_dir.clone();
        let interval = std::time::Duration::from_secs(self.config.compaction_interval_secs);
        let running = self.running.clone();

        let handle = tokio::spawn(async move {
            info!("Compaction task started");

            while running.load(std::sync::atomic::Ordering::SeqCst) {
                tokio::time::sleep(interval).await;

                if !running.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }

                // Check if compaction is needed
                if let Ok(should_compact) = Self::should_compact(&storage_dir).await
                    && should_compact
                {
                    info!("Triggering compaction cycle");
                    // Deferred: Run actual compaction
                    // For now, this is a placeholder
                }
            }

            info!("Compaction task stopped");
        });

        self.compaction_task = Some(handle);
        Ok(())
    }

    /// Stop background compaction task
    pub async fn stop(&mut self) -> Result<()> {
        self.running
            .store(false, std::sync::atomic::Ordering::SeqCst);

        if let Some(handle) = self.compaction_task.take() {
            handle.abort();
        }

        Ok(())
    }

    /// Check if compaction should be triggered
    async fn should_compact(storage_dir: &PathBuf) -> Result<bool> {
        // Check fragmentation level
        let fragmentation = Self::calculate_fragmentation(storage_dir).await?;

        if fragmentation > 30.0 {
            // >30% fragmentation
            return Ok(true);
        }

        // Check free space
        let free_space = Self::calculate_free_space(storage_dir).await?;

        if free_space < 20.0 {
            // <20% free space
            return Ok(true);
        }

        Ok(false)
    }

    /// Calculate current fragmentation percentage
    async fn calculate_fragmentation(_storage_dir: &PathBuf) -> Result<f64> {
        // Deferred: Implement actual fragmentation calculation
        // For now, return 0 (no fragmentation)
        Ok(0.0)
    }

    /// Calculate current free space percentage
    async fn calculate_free_space(_storage_dir: &PathBuf) -> Result<f64> {
        // Deferred: Implement actual free space calculation
        // For now, return 100 (all free)
        Ok(100.0)
    }

    /// Run a manual compaction cycle
    pub async fn compact(&self) -> Result<CompactionStats> {
        let start = std::time::Instant::now();

        info!("Starting manual compaction cycle");

        // Deferred: Implement actual compaction logic
        // 1. Scan for deleted/unused entries
        // 2. Copy live data to new mmap files
        // 3. Atomic swap of files
        // 4. Cleanup old files

        let stats = CompactionStats {
            bytes_before: 0,
            bytes_after: 0,
            space_saved: 0,
            nodes_compacted: 0,
            edges_compacted: 0,
            duration_ms: start.elapsed().as_millis() as u64,
            fragmentation_before: 0.0,
            fragmentation_after: 0.0,
        };

        info!(
            "Compaction complete: saved {} bytes in {}ms",
            stats.space_saved, stats.duration_ms
        );

        Ok(stats)
    }

    /// Get compaction statistics
    pub fn stats(&self) -> CompactionStats {
        // Deferred: Return actual statistics
        CompactionStats {
            bytes_before: 0,
            bytes_after: 0,
            space_saved: 0,
            nodes_compacted: 0,
            edges_compacted: 0,
            duration_ms: 0,
            fragmentation_before: 0.0,
            fragmentation_after: 0.0,
        }
    }

    /// Check if compaction is currently running
    pub fn is_running(&self) -> bool {
        self.running.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_compaction_config_default() {
        let config = CompactionConfig::default();
        assert_eq!(config.min_fragmentation_percent, 30);
        assert_eq!(config.min_free_space_percent, 20);
        assert!(config.auto_compaction);
    }

    #[tokio::test]
    async fn test_compaction_manager_creation() {
        let storage_dir = PathBuf::from("/tmp/test_compaction");
        let config = CompactionConfig::default();
        let manager = CompactionManager::new(storage_dir, config);

        assert!(!manager.is_running());
    }

    #[tokio::test]
    async fn test_compaction_start_stop() {
        let storage_dir = PathBuf::from("/tmp/test_compaction_lifecycle");
        let config = CompactionConfig::default();
        let mut manager = CompactionManager::new(storage_dir, config);

        manager.start().await.unwrap();
        assert!(manager.is_running());

        manager.stop().await.unwrap();
        assert!(!manager.is_running());
    }

    #[tokio::test]
    async fn test_manual_compaction() {
        let storage_dir = PathBuf::from("/tmp/test_manual_compaction");
        let config = CompactionConfig::default();
        let manager = CompactionManager::new(storage_dir, config);

        let stats = manager.compact().await.unwrap();
        assert_eq!(stats.space_saved, 0); // Placeholder implementation
    }

    #[tokio::test]
    async fn test_compaction_stats() {
        let stats = CompactionStats {
            bytes_before: 1000,
            bytes_after: 700,
            space_saved: 300,
            nodes_compacted: 100,
            edges_compacted: 500,
            duration_ms: 100,
            fragmentation_before: 40.0,
            fragmentation_after: 20.0,
        };

        assert_eq!(stats.savings_percent(), 30.0);
        assert!(stats.target_achieved(25.0)); // 20% < 25% target
        assert!(!stats.target_achieved(15.0)); // 20% > 15% target
    }
}
