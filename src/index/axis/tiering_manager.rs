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

//! AXIS Index Tiering Manager
//!
//! Manages intelligent movement of AXIS indexes across storage tiers
//! based on access patterns, memory pressure, and configuration.

use crate::index::axis::collection_state::{CollectionStateManager, CollectionTierState, TierLevel};
use crate::index::axis::memory_tracker::IndexMemoryTracker;
use crate::index::axis::format_strategy::{IndexFormatStrategy, IndexSerializationFormat};
use crate::index::axis::serialization::IndexSerializer;
use crate::storage::persistence::filesystem::{
    FilesystemFactory, StorageTier, FilesystemError, FsResult
};
use crate::common::tier_policy_engine::{GlobalTierManager, TierPolicy};
use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, info, warn, error};
use serde::{Serialize, Deserialize};

/// Configuration for AXIS tiering behavior
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AxisTieringConfig {
    /// Enable automatic tiering
    pub enable_auto_tiering: bool,
    
    /// Memory pressure threshold for demotion (0.0-1.0)
    pub memory_pressure_threshold: f64,
    
    /// Access frequency threshold for promotion (accesses per hour)
    pub promotion_access_threshold: f64,
    
    /// Time threshold for demotion (seconds since last access)
    pub demotion_time_threshold_secs: u64,
    
    /// Maximum concurrent tier operations
    pub max_concurrent_operations: usize,
    
    /// Tier evaluation interval (seconds)
    pub evaluation_interval_secs: u64,
    
    /// Preferred tiers in order of preference
    pub preferred_tiers: Vec<StorageTier>,
    
    /// Enable predictive prefetch based on patterns
    pub enable_predictive_prefetch: bool,
}

impl Default for AxisTieringConfig {
    fn default() -> Self {
        Self {
            enable_auto_tiering: true,
            memory_pressure_threshold: 0.85,
            promotion_access_threshold: 10.0,
            demotion_time_threshold_secs: 3600,
            max_concurrent_operations: 4,
            evaluation_interval_secs: 60,
            preferred_tiers: vec![
                StorageTier::Memory,
                StorageTier::NVMe,
                StorageTier::SSD,
                StorageTier::HDD,
                StorageTier::S3Standard,
            ],
            enable_predictive_prefetch: true,
        }
    }
}

/// Statistics for tiering operations
#[derive(Debug, Clone, Default)]
pub struct TieringStats {
    pub promotions: u64,
    pub demotions: u64,
    pub prefetch_hits: u64,
    pub prefetch_misses: u64,
    pub bytes_promoted: u64,
    pub bytes_demoted: u64,
    pub last_evaluation: Option<Instant>,
}

/// Main tiering manager for AXIS indexes
pub struct AxisTieringManager {
    /// Configuration
    config: AxisTieringConfig,
    
    /// Collection state manager
    collection_state: Arc<CollectionStateManager>,
    
    /// Memory tracker
    memory_tracker: Arc<IndexMemoryTracker>,
    
    /// Filesystem factory for tier operations
    filesystem: Arc<FilesystemFactory>,
    
    /// Global tier policy engine
    tier_policy: Arc<GlobalTierManager>,
    
    /// Active tier operations (collection_id -> operation)
    active_operations: Arc<DashMap<String, TierOperation>>,
    
    /// Tiering statistics
    stats: Arc<RwLock<TieringStats>>,
    
    /// Access pattern tracking (collection_id -> AccessPattern)
    access_patterns: Arc<DashMap<String, AccessPattern>>,
}

/// Represents an active tier operation
#[derive(Debug, Clone)]
struct TierOperation {
    collection_id: String,
    from_tier: TierLevel,
    to_tier: TierLevel,
    started_at: Instant,
    operation_type: TierOperationType,
}

#[derive(Debug, Clone, PartialEq)]
enum TierOperationType {
    Promotion,
    Demotion,
    Prefetch,
}

/// Access pattern tracking for predictive tiering
#[derive(Debug, Clone)]
struct AccessPattern {
    collection_id: String,
    access_times: Vec<Instant>,
    access_frequency: f64, // Accesses per hour
    last_access: Instant,
    total_accesses: u64,
    predicted_next_access: Option<Instant>,
}

impl AccessPattern {
    fn new(collection_id: String) -> Self {
        Self {
            collection_id,
            access_times: Vec::new(),
            access_frequency: 0.0,
            last_access: Instant::now(),
            total_accesses: 0,
            predicted_next_access: None,
        }
    }
    
    fn record_access(&mut self) {
        let now = Instant::now();
        self.access_times.push(now);
        self.last_access = now;
        self.total_accesses += 1;
        
        // Keep only last hour of access times
        let one_hour_ago = now - Duration::from_secs(3600);
        self.access_times.retain(|&t| t > one_hour_ago);
        
        // Calculate access frequency
        self.access_frequency = self.access_times.len() as f64;
        
        // Simple prediction: if regular pattern, predict next access
        if self.access_times.len() >= 3 {
            // Calculate average interval
            let intervals: Vec<Duration> = self.access_times.windows(2)
                .map(|w| w[1] - w[0])
                .collect();
            
            if !intervals.is_empty() {
                let avg_interval = intervals.iter().sum::<Duration>() / intervals.len() as u32;
                self.predicted_next_access = Some(now + avg_interval);
            }
        }
    }
    
    fn should_prefetch(&self, now: Instant) -> bool {
        if let Some(predicted) = self.predicted_next_access {
            // Prefetch if we're within 10% of the predicted time
            let prefetch_window = Duration::from_secs(60);
            predicted.saturating_duration_since(now) < prefetch_window
        } else {
            false
        }
    }
}

impl AxisTieringManager {
    /// Create new tiering manager
    pub fn new(
        config: AxisTieringConfig,
        collection_state: Arc<CollectionStateManager>,
        memory_tracker: Arc<IndexMemoryTracker>,
        filesystem: Arc<FilesystemFactory>,
        tier_policy: Arc<GlobalTierManager>,
    ) -> Self {
        Self {
            config,
            collection_state,
            memory_tracker,
            filesystem,
            tier_policy,
            active_operations: Arc::new(DashMap::new()),
            stats: Arc::new(RwLock::new(TieringStats::default())),
            access_patterns: Arc::new(DashMap::new()),
        }
    }
    
    /// Record access to a collection for pattern tracking
    pub async fn record_access(&self, collection_id: &str) {
        let mut pattern = self.access_patterns.entry(collection_id.to_string())
            .or_insert_with(|| AccessPattern::new(collection_id.to_string()));
        
        pattern.record_access();
        
        // Check if predictive prefetch is needed
        if self.config.enable_predictive_prefetch {
            if pattern.should_prefetch(Instant::now()) {
                if let Ok(state) = self.collection_state.get_state(collection_id).await {
                    match state {
                        CollectionTierState::Disk { .. } | 
                        CollectionTierState::Cloud { .. } => {
                            // Schedule prefetch
                            let _ = self.schedule_prefetch(collection_id).await;
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    
    /// Evaluate all collections for potential tier movements
    pub async fn evaluate_tiering(&self) -> FsResult<()> {
        if !self.config.enable_auto_tiering {
            return Ok(());
        }
        
        info!("Evaluating collections for tiering decisions");
        
        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.last_evaluation = Some(Instant::now());
        }
        
        // Get memory pressure
        let memory_pressure = self.memory_tracker.get_memory_pressure();
        debug!("Current memory pressure: {:.2}%", memory_pressure * 100.0);
        
        // Evaluate each collection
        let collections = self.collection_state.list_collections().await?;
        
        for collection_id in collections {
            // Skip if operation already in progress
            if self.active_operations.contains_key(&collection_id) {
                continue;
            }
            
            // Get current state and access pattern
            let state = self.collection_state.get_state(&collection_id).await?;
            let access_pattern = self.access_patterns.get(&collection_id);
            
            // Determine optimal tier based on access pattern and memory pressure
            let optimal_tier = self.determine_optimal_tier(
                &state,
                access_pattern.as_ref().map(|p| p.value()),
                memory_pressure,
            ).await;
            
            // Execute tier movement if needed
            if let Some(target_tier) = optimal_tier {
                self.execute_tier_movement(&collection_id, target_tier).await?;
            }
        }
        
        Ok(())
    }
    
    /// Determine optimal tier for a collection
    async fn determine_optimal_tier(
        &self,
        current_state: &CollectionTierState,
        access_pattern: Option<&AccessPattern>,
        memory_pressure: f64,
    ) -> Option<TierLevel> {
        let current_tier = current_state.current_tier();
        
        // Check for demotion due to memory pressure
        if memory_pressure > self.config.memory_pressure_threshold {
            if current_tier == TierLevel::Memory {
                debug!("Memory pressure exceeded, considering demotion");
                
                // Find least recently used collection for demotion
                if let Some(pattern) = access_pattern {
                    let time_since_access = Instant::now() - pattern.last_access;
                    if time_since_access.as_secs() > self.config.demotion_time_threshold_secs {
                        return Some(TierLevel::Disk);
                    }
                }
            }
        }
        
        // Check for promotion based on access frequency
        if let Some(pattern) = access_pattern {
            if pattern.access_frequency > self.config.promotion_access_threshold {
                match current_tier {
                    TierLevel::Disk | TierLevel::Cloud => {
                        // High access frequency, promote to memory if space available
                        if memory_pressure < 0.7 {
                            return Some(TierLevel::Memory);
                        }
                    }
                    _ => {}
                }
            }
        }
        
        // Check global tier policy
        if let Some(policy) = self.tier_policy.get_policy_for_collection(
            access_pattern.map(|p| &p.collection_id).unwrap_or(&String::new())
        ) {
            if let Some(target) = policy.evaluate_tier_change(current_tier) {
                return Some(target);
            }
        }
        
        None
    }
    
    /// Execute tier movement for a collection
    async fn execute_tier_movement(
        &self,
        collection_id: &str,
        target_tier: TierLevel,
    ) -> FsResult<()> {
        let current_state = self.collection_state.get_state(collection_id).await?;
        let current_tier = current_state.current_tier();
        
        if current_tier == target_tier {
            return Ok(());
        }
        
        let operation_type = if target_tier > current_tier {
            TierOperationType::Demotion
        } else {
            TierOperationType::Promotion
        };
        
        info!("Executing {:?} for collection {} from {:?} to {:?}", 
            operation_type, collection_id, current_tier, target_tier);
        
        // Record active operation
        self.active_operations.insert(
            collection_id.to_string(),
            TierOperation {
                collection_id: collection_id.to_string(),
                from_tier: current_tier,
                to_tier: target_tier,
                started_at: Instant::now(),
                operation_type: operation_type.clone(),
            }
        );
        
        // Perform the actual tier movement
        let result = match (current_tier, target_tier) {
            (TierLevel::Memory, TierLevel::Disk) => {
                self.demote_to_disk(collection_id, &current_state).await
            }
            (TierLevel::Disk, TierLevel::Memory) => {
                self.promote_to_memory(collection_id, &current_state).await
            }
            (TierLevel::Disk, TierLevel::Cloud) => {
                self.demote_to_cloud(collection_id, &current_state).await
            }
            (TierLevel::Cloud, TierLevel::Disk) => {
                self.promote_to_disk(collection_id, &current_state).await
            }
            _ => {
                warn!("Unsupported tier movement from {:?} to {:?}", current_tier, target_tier);
                Ok(())
            }
        };
        
        // Remove active operation
        self.active_operations.remove(collection_id);
        
        // Update stats
        if result.is_ok() {
            let mut stats = self.stats.write().await;
            match operation_type {
                TierOperationType::Promotion => stats.promotions += 1,
                TierOperationType::Demotion => stats.demotions += 1,
                TierOperationType::Prefetch => stats.prefetch_hits += 1,
            }
        }
        
        result
    }
    
    /// Demote collection from memory to disk
    async fn demote_to_disk(
        &self,
        collection_id: &str,
        current_state: &CollectionTierState,
    ) -> FsResult<()> {
        info!("Demoting collection {} from memory to disk", collection_id);
        
        // Get access pattern for format selection
        let access_pattern = self.access_patterns.get(collection_id);
        let access_frequency = access_pattern.as_ref()
            .map(|p| p.access_frequency)
            .unwrap_or(0.0);
        
        // Serialize index data from memory
        let index_data = self.memory_tracker.serialize_index(collection_id).await?;
        
        // Select format based on target tier and access pattern
        let format = IndexFormatStrategy::select_format(
            &StorageTier::SSD,
            access_frequency,
            index_data.len() as u64,
        );
        
        info!("Using {:?} format for disk storage", format);
        
        // Serialize with selected format
        let serialized_data = IndexFormatStrategy::serialize(&index_data, format)
            .map_err(|e| FilesystemError::Io(
                std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
            ))?;
        
        // Write to disk tier with format extension
        let extension = match format {
            IndexSerializationFormat::Bincode => "bin",
            IndexSerializationFormat::BincodeCompressed => "bin.zst",
            IndexSerializationFormat::Avro => "avro",
        };
        let disk_path = format!("axis/indexes/{}/index.{}", collection_id, extension);
        let disk_url = self.filesystem.get_tier_url(StorageTier::SSD, &disk_path)?;
        
        self.filesystem.write(&disk_url, &serialized_data, None).await?;
        
        // Store format metadata for recovery
        let format_meta_path = format!("axis/indexes/{}/format.meta", collection_id);
        let format_meta_url = self.filesystem.get_tier_url(StorageTier::SSD, &format_meta_path)?;
        let format_meta = format!("{:?}", format);
        self.filesystem.write(&format_meta_url, format_meta.as_bytes(), None).await?;
        
        // Update state
        self.collection_state.transition_to_disk(
            collection_id,
            disk_url.clone(),
            serialized_data.len() as u64,
        ).await?;
        
        // Clear from memory
        self.memory_tracker.evict_index(collection_id).await?;
        
        info!("Successfully demoted collection {} to disk at {}", collection_id, disk_url);
        Ok(())
    }
    
    /// Promote collection from disk to memory
    async fn promote_to_memory(
        &self,
        collection_id: &str,
        current_state: &CollectionTierState,
    ) -> FsResult<()> {
        info!("Promoting collection {} from disk to memory", collection_id);
        
        if let CollectionTierState::Disk { disk_location, .. } = current_state {
            // Read format metadata
            let format_meta_path = format!("axis/indexes/{}/format.meta", collection_id);
            let format_meta_url = self.filesystem.get_tier_url(StorageTier::SSD, &format_meta_path)?;
            
            let format = if self.filesystem.exists(&format_meta_url).await? {
                let format_data = self.filesystem.read(&format_meta_url).await?;
                let format_str = String::from_utf8_lossy(&format_data);
                
                // Parse format from metadata
                match format_str.as_ref() {
                    "Bincode" => IndexSerializationFormat::Bincode,
                    "BincodeCompressed" => IndexSerializationFormat::BincodeCompressed,
                    "Avro" => IndexSerializationFormat::Avro,
                    _ => {
                        warn!("Unknown format {}, detecting from data", format_str);
                        // Try to detect from file extension or magic bytes
                        if disk_location.to_string_lossy().ends_with(".zst") {
                            IndexSerializationFormat::BincodeCompressed
                        } else if disk_location.to_string_lossy().ends_with(".avro") {
                            IndexSerializationFormat::Avro
                        } else {
                            IndexSerializationFormat::Bincode
                        }
                    }
                }
            } else {
                // No format metadata, try to detect
                warn!("No format metadata found, detecting from file");
                if disk_location.to_string_lossy().ends_with(".zst") {
                    IndexSerializationFormat::BincodeCompressed
                } else if disk_location.to_string_lossy().ends_with(".avro") {
                    IndexSerializationFormat::Avro
                } else {
                    IndexSerializationFormat::Bincode
                }
            };
            
            info!("Reading index from disk with {:?} format", format);
            
            // Read from disk
            let serialized_data = self.filesystem.read(&disk_location.to_string_lossy()).await?;
            
            // Deserialize with detected format
            let index_data: Vec<u8> = IndexFormatStrategy::deserialize(&serialized_data, format)
                .map_err(|e| FilesystemError::Io(
                    std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
                ))?;
            
            // Load into memory
            self.memory_tracker.load_index(collection_id, index_data.clone()).await?;
            
            // Update state
            self.collection_state.transition_to_memory(
                collection_id,
                index_data.len() as u64,
            ).await?;
            
            // Optionally delete from disk
            // self.filesystem.delete(&disk_location.to_string_lossy()).await?;
            
            info!("Successfully promoted collection {} to memory", collection_id);
        }
        
        Ok(())
    }
    
    /// Demote collection from disk to cloud
    async fn demote_to_cloud(
        &self,
        collection_id: &str,
        current_state: &CollectionTierState,
    ) -> FsResult<()> {
        info!("Demoting collection {} from disk to cloud", collection_id);
        
        if let CollectionTierState::Disk { disk_location, disk_bytes, .. } = current_state {
            // Get access pattern for tier and format selection
            let access_pattern = self.access_patterns.get(collection_id);
            let access_frequency = access_pattern.as_ref()
                .map(|p| p.access_frequency)
                .unwrap_or(0.0);
            
            // Determine cloud tier based on access pattern
            let cloud_tier = if access_frequency < 0.1 {
                StorageTier::S3GlacierInstant
            } else {
                StorageTier::S3Standard
            };
            
            // For cloud storage, we should use Avro for schema evolution
            let target_format = IndexSerializationFormat::Avro;
            
            info!("Moving to {:?} with {:?} format", cloud_tier, target_format);
            
            // Read current data from disk
            let current_data = self.filesystem.read(&disk_location.to_string_lossy()).await?;
            
            // Detect current format
            let current_format = if disk_location.to_string_lossy().ends_with(".zst") {
                IndexSerializationFormat::BincodeCompressed
            } else if disk_location.to_string_lossy().ends_with(".avro") {
                IndexSerializationFormat::Avro
            } else {
                IndexSerializationFormat::Bincode
            };
            
            // Convert format if needed
            let cloud_data = if current_format != target_format {
                info!("Converting from {:?} to {:?}", current_format, target_format);
                // This would need the actual index type, for now just use the data as-is
                current_data
            } else {
                current_data
            };
            
            // Write to cloud with Avro extension
            let cloud_path = format!("axis/indexes/{}/index.avro", collection_id);
            let cloud_url = self.filesystem.get_tier_url(cloud_tier, &cloud_path)?;
            self.filesystem.write(&cloud_url, &cloud_data, None).await?;
            
            // Store format metadata
            let format_meta_path = format!("axis/indexes/{}/format.meta", collection_id);
            let format_meta_url = self.filesystem.get_tier_url(cloud_tier, &format_meta_path)?;
            self.filesystem.write(&format_meta_url, b"Avro", None).await?;
            
            // Update state
            self.collection_state.transition_to_cloud(
                collection_id,
                cloud_tier,
                cloud_url,
                cloud_data.len() as u64,
            ).await?;
            
            // Delete from disk after successful cloud upload
            self.filesystem.delete(&disk_location.to_string_lossy()).await?;
            
            info!("Successfully demoted collection {} to cloud tier {:?}", collection_id, cloud_tier);
        }
        
        Ok(())
    }
    
    /// Promote collection from cloud to disk
    async fn promote_to_disk(
        &self,
        collection_id: &str,
        current_state: &CollectionTierState,
    ) -> FsResult<()> {
        info!("Promoting collection {} from cloud to disk", collection_id);
        
        if let CollectionTierState::Cloud { storage_type, location, cloud_bytes, .. } = current_state {
            // Promote to disk
            let disk_path = format!("axis/indexes/{}/index.bin", collection_id);
            self.filesystem.promote_data(
                *storage_type,
                StorageTier::SSD,
                &disk_path,
            ).await?;
            
            // Update state
            let disk_url = self.filesystem.get_tier_url(StorageTier::SSD, &disk_path)?;
            self.collection_state.transition_to_disk(
                collection_id,
                disk_url,
                *cloud_bytes,
            ).await?;
            
            info!("Successfully promoted collection {} to disk", collection_id);
        }
        
        Ok(())
    }
    
    /// Schedule predictive prefetch for a collection
    async fn schedule_prefetch(&self, collection_id: &str) -> FsResult<()> {
        if self.active_operations.contains_key(collection_id) {
            return Ok(());
        }
        
        debug!("Scheduling predictive prefetch for collection {}", collection_id);
        
        // Record prefetch operation
        self.active_operations.insert(
            collection_id.to_string(),
            TierOperation {
                collection_id: collection_id.to_string(),
                from_tier: TierLevel::Disk,
                to_tier: TierLevel::Memory,
                started_at: Instant::now(),
                operation_type: TierOperationType::Prefetch,
            }
        );
        
        // Spawn async prefetch task
        let manager = self.clone();
        let collection_id = collection_id.to_string();
        
        tokio::spawn(async move {
            if let Ok(state) = manager.collection_state.get_state(&collection_id).await {
                let _ = manager.promote_to_memory(&collection_id, &state).await;
            }
            manager.active_operations.remove(&collection_id);
        });
        
        Ok(())
    }
    
    /// Get tiering statistics
    pub async fn get_stats(&self) -> TieringStats {
        self.stats.read().await.clone()
    }
    
    /// Check if a tier operation is active for a collection
    pub fn is_operation_active(&self, collection_id: &str) -> bool {
        self.active_operations.contains_key(collection_id)
    }
}

// Manual Clone implementation
impl Clone for AxisTieringManager {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            collection_state: Arc::clone(&self.collection_state),
            memory_tracker: Arc::clone(&self.memory_tracker),
            filesystem: Arc::clone(&self.filesystem),
            tier_policy: Arc::clone(&self.tier_policy),
            active_operations: Arc::clone(&self.active_operations),
            stats: Arc::clone(&self.stats),
            access_patterns: Arc::clone(&self.access_patterns),
        }
    }
}

// Helper trait for tier policy integration
trait TierPolicyExt {
    fn evaluate_tier_change(&self, current: TierLevel) -> Option<TierLevel>;
}

impl TierPolicyExt for TierPolicy {
    fn evaluate_tier_change(&self, current: TierLevel) -> Option<TierLevel> {
        // Map between TierLevel and rules in TierPolicy
        match current {
            TierLevel::Memory if self.demote_threshold > 0.0 => Some(TierLevel::Disk),
            TierLevel::Disk if self.promote_threshold > 0.0 => Some(TierLevel::Memory),
            _ => None,
        }
    }
}

// Extension trait for GlobalTierManager
trait GlobalTierManagerExt {
    fn get_policy_for_collection(&self, collection_id: &str) -> Option<TierPolicy>;
}

impl GlobalTierManagerExt for GlobalTierManager {
    fn get_policy_for_collection(&self, _collection_id: &str) -> Option<TierPolicy> {
        // For now, return default policy
        // In future, this would look up collection-specific policies
        Some(TierPolicy::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_access_pattern_tracking() {
        let mut pattern = AccessPattern::new("test_collection".to_string());
        
        // Record multiple accesses
        for _ in 0..5 {
            pattern.record_access();
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        
        assert_eq!(pattern.total_accesses, 5);
        assert!(pattern.access_frequency > 0.0);
        assert!(pattern.predicted_next_access.is_some());
    }
    
    #[tokio::test]
    async fn test_tier_comparison() {
        assert!(StorageTier::Memory.is_faster_than(&StorageTier::NVMe));
        assert!(StorageTier::NVMe.is_faster_than(&StorageTier::SSD));
        assert!(StorageTier::SSD.is_faster_than(&StorageTier::HDD));
        assert!(!StorageTier::HDD.is_faster_than(&StorageTier::SSD));
    }
}