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

//! SST File Compaction for LSM Tree
//!
//! Implements level-based compaction strategy to prevent unbounded growth
//! of SST files. Uses background workers to merge files when thresholds are exceeded.

use super::LsmEntry;
use crate::core::{CollectionId, LsmConfig, VectorId};
use crate::storage::Result;
use chrono::Utc;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

/// Compaction task to be processed by background workers
#[derive(Debug, Clone)]
pub struct CompactionTask {
    pub collection_id: CollectionId,
    pub level: u8,
    pub input_files: Vec<PathBuf>,
    pub output_file: PathBuf,
    pub priority: CompactionPriority,
}

/// Priority levels for compaction tasks
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum CompactionPriority {
    Low = 0,
    Medium = 1,
    High = 2,
    Critical = 3, // When storage is nearly full
}

/// Statistics for compaction operations
#[derive(Debug, Clone, Default)]
pub struct CompactionStats {
    pub total_compactions: u64,
    pub bytes_written: u64,
    pub bytes_read: u64,
    pub files_merged: u64,
    pub avg_compaction_time_ms: u64,
    pub last_compaction_time: Option<chrono::DateTime<chrono::Utc>>,
}

/// Manages background compaction of SST files
#[derive(Debug)]
pub struct CompactionManager {
    config: LsmConfig,
    task_queue: Arc<Mutex<VecDeque<CompactionTask>>>,
    worker_handles: Vec<JoinHandle<()>>,
    shutdown_signal: Arc<AtomicBool>,
    stats: Arc<RwLock<CompactionStats>>,
    active_compactions: Arc<RwLock<HashMap<CollectionId, CompactionTask>>>,
}

impl CompactionManager {
    /// Create a new compaction manager
    pub fn new(config: LsmConfig) -> Self {
        Self {
            config,
            task_queue: Arc::new(Mutex::new(VecDeque::new())),
            worker_handles: Vec::new(),
            shutdown_signal: Arc::new(AtomicBool::new(false)),
            stats: Arc::new(RwLock::new(CompactionStats::default())),
            active_compactions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Start background compaction workers
    pub async fn start_workers(&mut self, worker_count: usize) -> Result<()> {
        info!("Starting {} compaction workers", worker_count);

        for worker_id in 0..worker_count {
            let task_queue = self.task_queue.clone();
            let shutdown_signal = self.shutdown_signal.clone();
            let stats = self.stats.clone();
            let active_compactions = self.active_compactions.clone();
            let config = self.config.clone();

            let handle = tokio::spawn(async move {
                Self::worker_loop(
                    worker_id,
                    task_queue,
                    shutdown_signal,
                    stats,
                    active_compactions,
                    config,
                )
                .await;
            });

            self.worker_handles.push(handle);
        }

        Ok(())
    }

    /// Stop all compaction workers gracefully
    pub async fn stop(&mut self) -> Result<()> {
        info!("Stopping compaction manager");

        self.shutdown_signal.store(true, Ordering::SeqCst);

        // Wait for all workers to finish
        for handle in self.worker_handles.drain(..) {
            if let Err(e) = handle.await {
                warn!("Compaction worker failed to shutdown cleanly: {}", e);
            }
        }

        // Complete any remaining compactions
        let remaining_tasks = {
            let queue = self.task_queue.lock().await;
            queue.len()
        };

        if remaining_tasks > 0 {
            warn!(
                "Compaction manager stopped with {} pending tasks",
                remaining_tasks
            );
        }

        info!("Compaction manager stopped successfully");
        Ok(())
    }

    /// Schedule a compaction task
    pub async fn schedule_compaction(&self, task: CompactionTask) -> Result<()> {
        debug!(
            "Scheduling compaction for collection {} level {}",
            task.collection_id, task.level
        );

        // Check if there's already an active compaction for this collection
        {
            let active = self.active_compactions.read().await;
            if active.contains_key(&task.collection_id) {
                debug!(
                    "Skipping compaction - already active for collection {}",
                    task.collection_id
                );
                return Ok(());
            }
        }

        let mut queue = self.task_queue.lock().await;

        // Insert task in priority order
        let insert_pos = queue
            .iter()
            .position(|existing_task| existing_task.priority < task.priority)
            .unwrap_or(queue.len());

        queue.insert(insert_pos, task);

        debug!(
            "Compaction task queued (position: {}, queue size: {})",
            insert_pos,
            queue.len()
        );

        Ok(())
    }

    /// Check if compaction is needed for the given collection and level
    pub async fn check_compaction_needed(
        &self,
        collection_dir: &Path,
        collection_id: &CollectionId,
    ) -> Result<Option<CompactionTask>> {
        let sst_files = self.get_sst_files_by_level(collection_dir).await?;

        for level in 0..self.config.level_count {
            let files_at_level = sst_files.get(&level).map(|v| v.len()).unwrap_or(0);

            if files_at_level >= self.config.compaction_threshold as usize {
                info!(
                    "Compaction needed for collection {} level {} ({} files >= {})",
                    collection_id, level, files_at_level, self.config.compaction_threshold
                );

                let input_files = sst_files.get(&level).cloned().unwrap_or_default();
                let output_file = self.generate_output_file_path(collection_dir, level + 1);

                let priority = if files_at_level >= (self.config.compaction_threshold * 2) as usize
                {
                    CompactionPriority::High
                } else {
                    CompactionPriority::Medium
                };

                return Ok(Some(CompactionTask {
                    collection_id: collection_id.clone(),
                    level,
                    input_files,
                    output_file,
                    priority,
                }));
            }
        }

        Ok(None)
    }

    /// Get compaction statistics
    pub async fn get_stats(&self) -> CompactionStats {
        self.stats.read().await.clone()
    }

    /// Worker loop for processing compaction tasks
    async fn worker_loop(
        worker_id: usize,
        task_queue: Arc<Mutex<VecDeque<CompactionTask>>>,
        shutdown_signal: Arc<AtomicBool>,
        stats: Arc<RwLock<CompactionStats>>,
        active_compactions: Arc<RwLock<HashMap<CollectionId, CompactionTask>>>,
        config: LsmConfig,
    ) {
        debug!("Compaction worker {} started", worker_id);

        loop {
            if shutdown_signal.load(Ordering::SeqCst) {
                break;
            }

            // Get next task from queue
            let task = {
                let mut queue = task_queue.lock().await;
                queue.pop_front()
            };

            if let Some(task) = task {
                debug!(
                    "Worker {} processing compaction for collection {} level {}",
                    worker_id, task.collection_id, task.level
                );

                // Mark as active
                {
                    let mut active = active_compactions.write().await;
                    active.insert(task.collection_id.clone(), task.clone());
                }

                let start_time = std::time::Instant::now();

                // Perform compaction
                match Self::perform_compaction(&task, &config).await {
                    Ok(compaction_stats) => {
                        info!(
                            "Compaction completed for collection {} level {} in {}ms",
                            task.collection_id,
                            task.level,
                            start_time.elapsed().as_millis()
                        );

                        // Update statistics
                        {
                            let mut stats_guard = stats.write().await;
                            stats_guard.total_compactions += 1;
                            stats_guard.bytes_written += compaction_stats.bytes_written;
                            stats_guard.bytes_read += compaction_stats.bytes_read;
                            stats_guard.files_merged += compaction_stats.files_merged;
                            stats_guard.last_compaction_time = Some(Utc::now());

                            // Update average compaction time
                            let elapsed_ms = start_time.elapsed().as_millis() as u64;
                            if stats_guard.total_compactions == 1 {
                                stats_guard.avg_compaction_time_ms = elapsed_ms;
                            } else {
                                stats_guard.avg_compaction_time_ms =
                                    (stats_guard.avg_compaction_time_ms + elapsed_ms) / 2;
                            }
                        }
                    }
                    Err(e) => {
                        error!(
                            "Compaction failed for collection {} level {}: {}",
                            task.collection_id, task.level, e
                        );
                    }
                }

                // Remove from active compactions
                {
                    let mut active = active_compactions.write().await;
                    active.remove(&task.collection_id);
                }
            } else {
                // No tasks available, wait a bit
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }
        }

        debug!("Compaction worker {} stopped", worker_id);
    }

    /// Perform the actual compaction operation
    async fn perform_compaction(
        task: &CompactionTask,
        _config: &LsmConfig,
    ) -> Result<CompactionStats> {
        let start_time = std::time::Instant::now();
        let mut merged_data = BTreeMap::<VectorId, LsmEntry>::new();
        let mut bytes_read = 0u64;

        debug!(
            "Merging {} input files for level {}",
            task.input_files.len(),
            task.level
        );

        // Read and merge all input files
        for input_file in &task.input_files {
            let file_data = tokio::fs::read(input_file)
                .await
                .map_err(|e| crate::core::StorageError::DiskIO(e))?;

            bytes_read += file_data.len() as u64;

            // Parse SST file format: [len:4][data][len:4][data]...
            let mut offset = 0;
            while offset < file_data.len() {
                if offset + 4 > file_data.len() {
                    break;
                }

                let entry_len = u32::from_le_bytes([
                    file_data[offset],
                    file_data[offset + 1],
                    file_data[offset + 2],
                    file_data[offset + 3],
                ]) as usize;

                offset += 4;

                if offset + entry_len > file_data.len() {
                    break;
                }

                let entry_data = &file_data[offset..offset + entry_len];

                match bincode::deserialize::<(VectorId, LsmEntry)>(entry_data) {
                    Ok((id, entry)) => {
                        // Handle merge logic for LSM entries
                        match (&entry, merged_data.get(&id)) {
                            // If we have a newer entry, use it
                            (new_entry, Some(existing_entry)) => {
                                if should_replace_entry(existing_entry, new_entry) {
                                    merged_data.insert(id, entry);
                                }
                            }
                            // If no existing entry, insert the new one
                            (_, None) => {
                                merged_data.insert(id, entry);
                            }
                        }
                    }
                    Err(e) => {
                        warn!(
                            "Failed to deserialize entry in {}: {}",
                            input_file.display(),
                            e
                        );
                    }
                }

                offset += entry_len;
            }
        }

        debug!("Merged {} unique records", merged_data.len());

        // Write merged data to output file, filtering out old tombstones
        let mut output_data = Vec::new();
        for (id, lsm_entry) in merged_data.iter() {
            // Skip old tombstones (they can be garbage collected during compaction)
            // Keep only records and recent tombstones (within a certain time window)
            let should_keep = match lsm_entry {
                LsmEntry::Record(_) => true,
                LsmEntry::Tombstone { timestamp, .. } => {
                    // Keep tombstones that are less than 1 hour old
                    let age = chrono::Utc::now().signed_duration_since(*timestamp);
                    age.num_hours() < 1
                }
            };

            if should_keep {
                let entry = bincode::serialize(&(id.clone(), lsm_entry))
                    .map_err(|e| crate::core::StorageError::Serialization(e.to_string()))?;
                output_data.extend_from_slice(&(entry.len() as u32).to_le_bytes());
                output_data.extend_from_slice(&entry);
            }
        }

        // Ensure output directory exists
        if let Some(parent) = task.output_file.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| crate::core::StorageError::DiskIO(e))?;
        }

        // Write output file atomically (write to temp file, then rename)
        let temp_file = task.output_file.with_extension("tmp");
        tokio::fs::write(&temp_file, &output_data)
            .await
            .map_err(|e| crate::core::StorageError::DiskIO(e))?;

        tokio::fs::rename(&temp_file, &task.output_file)
            .await
            .map_err(|e| crate::core::StorageError::DiskIO(e))?;

        let bytes_written = output_data.len() as u64;

        debug!(
            "Wrote {} bytes to output file {}",
            bytes_written,
            task.output_file.display()
        );

        // Remove input files after successful compaction
        for input_file in &task.input_files {
            if let Err(e) = tokio::fs::remove_file(input_file).await {
                warn!(
                    "Failed to remove input file {}: {}",
                    input_file.display(),
                    e
                );
            }
        }

        debug!(
            "Compaction completed in {}ms",
            start_time.elapsed().as_millis()
        );

        Ok(CompactionStats {
            total_compactions: 1,
            bytes_written,
            bytes_read,
            files_merged: task.input_files.len() as u64,
            avg_compaction_time_ms: start_time.elapsed().as_millis() as u64,
            last_compaction_time: Some(Utc::now()),
        })
    }

    /// Get SST files organized by level
    async fn get_sst_files_by_level(
        &self,
        collection_dir: &Path,
    ) -> Result<HashMap<u8, Vec<PathBuf>>> {
        let mut files_by_level = HashMap::new();

        if !collection_dir.exists() {
            return Ok(files_by_level);
        }

        let mut dir = tokio::fs::read_dir(collection_dir)
            .await
            .map_err(|e| crate::core::StorageError::DiskIO(e))?;

        while let Some(entry) = dir
            .next_entry()
            .await
            .map_err(|e| crate::core::StorageError::DiskIO(e))?
        {
            let path = entry.path();
            if let Some(filename) = path.file_name().and_then(|f| f.to_str()) {
                if filename.starts_with("sst_") && filename.ends_with(".db") {
                    // For now, assign all SST files to level 0
                    // TODO: Parse level from filename or metadata
                    files_by_level.entry(0).or_insert_with(Vec::new).push(path);
                }
            }
        }

        Ok(files_by_level)
    }

    /// Generate output file path for compacted SST
    fn generate_output_file_path(&self, collection_dir: &Path, level: u8) -> PathBuf {
        let timestamp = Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let filename = format!("sst_l{}_t{}.db", level, timestamp);
        collection_dir.join(filename)
    }
}

/// Determine if a new entry should replace an existing entry during compaction
fn should_replace_entry(existing: &LsmEntry, new: &LsmEntry) -> bool {
    match (existing, new) {
        // Always prefer newer timestamps
        (LsmEntry::Record(existing_record), LsmEntry::Record(new_record)) => {
            new_record.timestamp > existing_record.timestamp
        }
        (LsmEntry::Record(record), LsmEntry::Tombstone { timestamp, .. }) => {
            timestamp.timestamp_millis() > record.timestamp
        }
        (
            LsmEntry::Tombstone {
                timestamp: existing_ts,
                ..
            },
            LsmEntry::Record(record),
        ) => record.timestamp > existing_ts.timestamp_millis(),
        (
            LsmEntry::Tombstone {
                timestamp: existing_ts,
                ..
            },
            LsmEntry::Tombstone {
                timestamp: new_ts, ..
            },
        ) => *new_ts > *existing_ts,
    }
}

impl Drop for CompactionManager {
    fn drop(&mut self) {
        self.shutdown_signal.store(true, Ordering::SeqCst);

        // Abort remaining worker handles
        for handle in &self.worker_handles {
            handle.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_compaction_manager_basic() {
        let config = LsmConfig {
            memtable_size_mb: 1,
            level_count: 3,
            compaction_threshold: 2,
            block_size_kb: 4,
        };

        let mut manager = CompactionManager::new(config);
        assert!(manager.start_workers(1).await.is_ok());
        assert!(manager.stop().await.is_ok());
    }

    #[tokio::test]
    async fn test_compaction_task_scheduling() {
        let config = LsmConfig {
            memtable_size_mb: 1,
            level_count: 3,
            compaction_threshold: 2,
            block_size_kb: 4,
        };

        let manager = CompactionManager::new(config);

        let task = CompactionTask {
            collection_id: "test_collection".to_string(),
            level: 0,
            input_files: vec![],
            output_file: PathBuf::from("/tmp/output.db"),
            priority: CompactionPriority::Medium,
        };

        assert!(manager.schedule_compaction(task).await.is_ok());
    }
}
