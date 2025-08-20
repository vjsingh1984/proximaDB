/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Common utilities for compaction across storage engines
//! Provides unified file discovery and filtering logic

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info};

use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::common::compaction_orchestrator::{GenericFileMetadata, TieredFileRegistry};
use crate::storage::engines::sst::flush_eventlog_integration::SstFlushHandler;
// use crate::storage::engines::viper::ViperFlushHandler;  // TODO: Fix import issue
use crate::core::config::CompactionConfig;

/// Storage engine type for EventLog filtering
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StorageEngineType {
    SST,
    VIPER,
}

/// Result of file discovery with EventLog filtering
#[derive(Debug, Clone)]
pub struct FilteredCompactionFiles {
    /// Files ready for compaction (not pending in EventLog)
    pub compactable_files: HashMap<u32, Vec<GenericFileMetadata>>,
    /// Files pending AXIS processing (excluded from compaction)
    pub pending_files: HashMap<u32, Vec<String>>,
    /// Total file count across all levels
    pub total_files: usize,
    /// Count of compactable files
    pub compactable_count: usize,
    /// Count of pending files
    pub pending_count: usize,
}

/// Unified file discovery with EventLog filtering
/// This eliminates duplicate logic between SST and VIPER engines
pub struct CompactionFileDiscovery {
    registry: TieredFileRegistry,
    filesystem: Arc<FilesystemFactory>,
}

impl CompactionFileDiscovery {
    /// Create new file discovery instance
    pub fn new(filesystem: Arc<FilesystemFactory>) -> Self {
        Self {
            registry: TieredFileRegistry::new(),
            filesystem,
        }
    }
    
    /// Discover and filter compactable files for a collection
    /// Files pending in EventLog (AXIS processing) are excluded
    pub async fn discover_compactable_files(
        &self,
        collection_id: &str,
        data_directory: &str,
        extension: &str,
        engine_type: StorageEngineType,
    ) -> Result<FilteredCompactionFiles> {
        debug!(
            "📁 COMPACTION: Discovering {} files in {} for collection {}",
            extension, data_directory, collection_id
        );
        
        // Discover all files using the unified registry
        let all_files = self.registry.discover_files(
            &self.filesystem,
            data_directory,
            extension,
        ).await?;
        
        // Get appropriate EventLog handler based on engine type
        let mut compactable_files = HashMap::new();
        let mut pending_files = HashMap::new();
        let mut total_files = 0;
        let mut compactable_count = 0;
        let mut pending_count = 0;
        
        // Process each level
        for (level, files) in all_files {
            let mut level_compactable = Vec::new();
            let mut level_pending = Vec::new();
            
            for file_meta in files {
                total_files += 1;
                let file_path = file_meta.path.clone();
                
                // Check if file can be compacted
                let can_compact_result = match engine_type {
                    StorageEngineType::SST => {
                        let handler = SstFlushHandler::new();
                        handler.can_compact_files(collection_id, &[file_path.clone()]).await
                    }
                    StorageEngineType::VIPER => {
                        // TODO: Fix ViperFlushHandler import
                        // let handler = ViperFlushHandler::new();
                        // handler.can_compact_files(collection_id, &[file_path.clone()]).await
                        Ok(true) // Temporary stub
                    }
                };
                
                let can_compact = can_compact_result;
                
                if can_compact {
                    level_compactable.push(file_meta);
                    compactable_count += 1;
                    debug!(
                        "  ✅ Level {} file {} is ready for compaction_info",
                        level, file_path
                    );
                } else {
                    level_pending.push(file_path.clone());
                    pending_count += 1;
                    debug!(
                        "  ⏸️ Level {} file {} is pending AXIS processing",
                        level, file_path
                    );
                }
            }
            
            if !level_compactable.is_empty() {
                compactable_files.insert(level, level_compactable);
            }
            if !level_pending.is_empty() {
                pending_files.insert(level, level_pending);
            }
        }
        
        info!(
            "🔍 COMPACTION: Discovery complete for collection {}:\n  Total files: {}\n  Compactable: {}\n  Pending AXIS: {}",
            collection_id, total_files, compactable_count, pending_count
        );
        
        // Log details by level if there are pending files
        if pending_count > 0 {
            for (level, files) in &pending_files {
                debug!(
                    "  Level {} has {} pending files: {:?}",
                    level, files.len(), files
                );
            }
        }
        
        Ok(FilteredCompactionFiles {
            compactable_files,
            pending_files,
            total_files,
            compactable_count,
            pending_count,
        })
    }
    
    /// Check if compaction should be triggered based on filtered files
    pub fn should_trigger_compaction(
        &self,
        filtered_files: &FilteredCompactionFiles,
        level: u32,
        threshold: usize,
    ) -> bool {
        if let Some(level_files) = filtered_files.compactable_files.get(&0) {
            let should_compact = level_files.len() >= threshold;
            
            if should_compact {
                info!(
                    "✅ COMPACTION: Level {} has {} compactable files (>= threshold {})",
                    level, level_files.len(), threshold
                );
            } else if filtered_files.pending_files.get(&0).is_some() {
                debug!(
                    "⏸️ COMPACTION: Level {} has only {} compactable files (< threshold {}), some files pending",
                    level, level_files.len(), threshold
                );
            }
            
            should_compact
        } else {
            false
        }
    }
    
    /// Get compaction task files for a specific level
    pub fn get_compaction_files(
        &self,
        filtered_files: &FilteredCompactionFiles,
        level: u32,
    ) -> Vec<String> {
        filtered_files.compactable_files
            .get(&level)
            .map(|files| files.iter().map(|f| f.path.clone()).collect())
            .unwrap_or_default()
    }
}

/// Unified compaction task builder
/// Creates compaction tasks based on discovered files and thresholds
pub struct CompactionTaskBuilder;

impl CompactionTaskBuilder {
    /// Check if compaction is needed and build task if necessary
    /// This unifies logic from SST's check_compaction_needed and VIPER's discover_compactable_files
    pub async fn check_and_build_compaction_task(
        collection_id: &str,
        data_directory: &str,
        extension: &str,
        engine_type: StorageEngineType,
        config: &CompactionConfig,
        filesystem: Arc<FilesystemFactory>,
    ) -> Result<Option<CompactionTaskInfo>> {
        debug!(
            "🔍 UNIFIED COMPACTION: Checking compaction for {} collection {} in {}",
            engine_type.as_str(), collection_id, data_directory
        );
        
        // Use unified file discovery with EventLog filtering
        let file_discovery = CompactionFileDiscovery::new(filesystem);
        let filtered_files = file_discovery.discover_compactable_files(
            collection_id,
            data_directory,
            extension,
            engine_type,
        ).await?;
        
        // Determine thresholds based on strategy
        let should_compact_l0 = match config.strategy.as_str() {
            "count" => file_discovery.should_trigger_compaction(&filtered_files, 0, config.l0_file_threshold),
            "size" => {
                // Calculate total size at L0
                let l0_total_size_mb = filtered_files.compactable_files.get(&0)
                    .map(|files| files.iter().map(|f| f.size_bytes / (1024 * 1024)).sum::<u64>() as usize)
                    ;
                l0_total_size_mb >= config.l0_size_threshold_mb
            }
            "hybrid" | _ => {
                // Use both count and size thresholds
                let count_triggered = file_discovery.should_trigger_compaction(&filtered_files, 0, config.l0_file_threshold);
                let l0_total_size_mb = filtered_files.compactable_files.get(&0)
                    .map(|files| files.iter().map(|f| f.size_bytes / (1024 * 1024)).sum::<u64>() as usize)
                    ;
                let size_triggered = l0_total_size_mb >= config.l0_size_threshold_mb;
                count_triggered || size_triggered
            }
        };
        
        // Check if Level 0 compaction is needed
        if should_compact_l0 {
            let compactable_files = file_discovery.get_compaction_files(&filtered_files, 0);
            
            info!(
                "✅ {} COMPACTION: Triggering with {} compactable files for collection {} (excluded {} pending files)",
                engine_type.as_str(),
                compactable_files.len(),
                collection_id,
                filtered_files.pending_count
            );
            
            return Ok(Some(CompactionTaskInfo {
                collection_id: collection_id.to_string(),
                source_level: 0,
                target_level: 1,
                input_files: compactable_files,
                extension: extension.to_string(),
                pending_files_count: filtered_files.pending_count,
                total_files_count: filtered_files.total_files,
            }));
        }
        
        // Check higher levels if needed (using configured max_levels)
        for level in 1..config.max_levels as u32 {
            // Apply level multiplier to thresholds
            let level_file_threshold = (config.l0_file_threshold as f64 * config.level_multiplier.powi(level as i32)) as usize;
            let level_size_threshold_mb = (config.l0_size_threshold_mb as f64 * config.level_multiplier.powi(level as i32)) as usize;
            
            let should_compact = match config.strategy.as_str() {
                "count" => file_discovery.should_trigger_compaction(&filtered_files, level, level_file_threshold),
                "size" => {
                    let level_total_size_mb = filtered_files.compactable_files.get(&level)
                        .map(|files| files.iter().map(|f| f.size_bytes / (1024 * 1024)).sum::<u64>() as usize)
                        ;
                    level_total_size_mb >= level_size_threshold_mb
                }
                "hybrid" | _ => {
                    let count_triggered = file_discovery.should_trigger_compaction(&filtered_files, level, level_file_threshold);
                    let level_total_size_mb = filtered_files.compactable_files.get(&level)
                        .map(|files| files.iter().map(|f| f.size_bytes / (1024 * 1024)).sum::<u64>() as usize)
                        ;
                    let size_triggered = level_total_size_mb >= level_size_threshold_mb;
                    count_triggered || size_triggered
                }
            };
            
            if should_compact {
                let compactable_files = file_discovery.get_compaction_files(&filtered_files, level);
                
                info!(
                    "✅ {} COMPACTION: Level {} triggering with {} files for collection {}",
                    engine_type.as_str(), level, compactable_files.len(), collection_id
                );
                
                return Ok(Some(CompactionTaskInfo {
                    collection_id: collection_id.to_string(),
                    source_level: level,
                    target_level: level + 1,
                    input_files: compactable_files,
                    extension: extension.to_string(),
                    pending_files_count: filtered_files.pending_count,
                    total_files_count: filtered_files.total_files,
                }));
            }
        }
        
        if filtered_files.pending_count > 0 {
            debug!(
                "⏸️ {} COMPACTION: Not enough compactable files for collection {} ({} ready, {} pending AXIS)",
                engine_type.as_str(),
                collection_id,
                filtered_files.compactable_count,
                filtered_files.pending_count
            );
        } else {
            debug!(
                "📋 {} COMPACTION: No compaction needed for collection {} ({} total files)",
                engine_type.as_str(),
                collection_id,
                filtered_files.total_files
            );
        }
        
        Ok(None)
    }
    
    /// Get all compactable files for a collection (used by VIPER for size-based compaction)
    pub async fn get_all_compactable_files(
        collection_id: &str,
        data_directory: &str,
        extension: &str,
        engine_type: StorageEngineType,
        filesystem: Arc<FilesystemFactory>,
    ) -> Result<Vec<String>> {
        let file_discovery = CompactionFileDiscovery::new(filesystem);
        let filtered_files = file_discovery.discover_compactable_files(
            collection_id,
            data_directory,
            extension,
            engine_type,
        ).await?;
        
        // Get all compactable files across all levels
        let mut all_files = Vec::new();
        for (_level, files) in filtered_files.compactable_files {
            for file_meta in files {
                all_files.push(file_meta.path);
            }
        }
        
        if all_files.is_empty() && filtered_files.pending_count > 0 {
            info!(
                "⏸️ {} COMPACTION: All {} files are pending AXIS processing for collection {}",
                engine_type.as_str(),
                filtered_files.pending_count,
                collection_id
            );
        }
        
        Ok(all_files)
    }
}

/// Information about a compaction task
#[derive(Debug, Clone)]
pub struct CompactionTaskInfo {
    pub collection_id: String,
    pub source_level: u32,
    pub target_level: u32,
    pub input_files: Vec<String>,
    pub extension: String,
    pub pending_files_count: usize,
    pub total_files_count: usize,
}

impl StorageEngineType {
    fn as_str(&self) -> &str {
        match self {
            StorageEngineType::SST => "SST",
            StorageEngineType::VIPER => "VIPER",
        }
    }
}

/// Self-healing behavior for compaction
/// When old files become available (AXIS completes), they automatically
/// become eligible for the next compaction cycle
pub struct CompactionSelfHealing;

impl CompactionSelfHealing {
    /// Log self-healing behavior when files transition from pending to ready
    pub fn log_file_transition(collection_id: &str, file_path: &str) {
        info!(
            "🔄 SELF-HEALING: File {} for collection {} is now ready for compaction_info",
            file_path, collection_id
        );
    }
    
    /// Check if self-healing occurred (previously pending files now ready)
    pub fn check_self_healing(
        previous_pending: &[String],
        current_compactable: &[String],
    ) -> Vec<String> {
        let mut healed_files = Vec::new();
        
        for file in previous_pending {
            if current_compactable.contains(file) {
                healed_files.push(file.clone());
            }
        }
        
        if !healed_files.is_empty() {
            info!(
                "🔄 SELF-HEALING: {} previously pending files are now compactable: {:?}",
                healed_files.len(), healed_files
            );
        }
        
        healed_files
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_self_healing_detection() {
        let previous_pending = vec![
            "file1.sstable".to_string(),
            "file2.sstable".to_string(),
            "file3.sstable".to_string(),
        ];
        
        let current_compactable = vec![
            "file1.sstable".to_string(),
            "file3.sstable".to_string(),
            "file4.sstable".to_string(),
        ];
        
        let healed = CompactionSelfHealing::check_self_healing(
            &previous_pending,
            &current_compactable,
        );
        
        assert_eq!(healed.len(), 2);
        assert!(healed.contains(&"file1.sstable".to_string()));
        assert!(healed.contains(&"file3.sstable".to_string()));
    }
}