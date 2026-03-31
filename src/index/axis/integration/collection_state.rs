// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! AXIS Collection State Management - Tracks tier placement and movement
//!
//! This module manages the collection-level tier states for AXIS indexes,
//! enabling bidirectional movement between memory, disk, and cloud storage.

use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info};

/// Collection-level tier state
#[derive(Debug, Clone)]
pub enum CollectionTierState {
    /// Fully loaded in memory, ready for queries.
    Memory {
        /// When the index was loaded into memory.
        loaded_at: Instant,
        /// Memory footprint of the loaded index in bytes.
        memory_bytes: usize,
        /// Total number of accesses since loading.
        access_count: u64,
        /// Timestamp of the most recent access.
        last_access: Instant,
        /// Index version generation for staleness detection.
        generation: u64,
    },

    /// On disk (local or persistent volume).
    Disk {
        /// When the index was stored to disk.
        stored_at: Instant,
        /// Filesystem path of the stored index.
        disk_location: PathBuf,
        /// Size of the on-disk index in bytes.
        disk_bytes: usize,
        /// Timestamp of the most recent access, if any.
        last_access: Option<Instant>,
        /// Whether this index is eligible for promotion to memory.
        promotion_eligible: bool,
    },

    /// In cloud storage (S3/GCS/Azure).
    Cloud {
        /// Cloud storage class and provider.
        storage_type: CloudStorageType,
        /// Cloud storage URL (e.g., S3 URI, GCS path).
        location: String,
        /// Size of the compressed index data in bytes.
        compressed_bytes: usize,
        /// Timestamp of the last modification in cloud storage.
        last_modified: DateTime<Utc>,
        /// Entity tag for cache validation.
        etag: String,
        /// Estimated cost in dollars to retrieve this data.
        retrieval_cost: f64,
    },

    /// Transitioning between tiers.
    Transitioning {
        /// Source tier state being migrated from.
        from: Box<CollectionTierState>,
        /// Target tier level being migrated to.
        to: TierLevel,
        /// When the transition began.
        started_at: Instant,
        /// Progress ratio from 0.0 to 1.0.
        progress: f32,
    },

    /// Not yet built (new collection)
    Unbuilt,
}

/// Tier levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TierLevel {
    /// In-memory tier for lowest latency access.
    Memory,
    /// Local or persistent disk tier for warm data.
    Disk,
    /// Cloud object storage tier for cold data.
    Cloud,
}

/// Cloud storage types
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CloudStorageType {
    /// AWS S3 Standard storage class.
    S3Standard,
    /// AWS S3 Express One Zone for single-digit-ms access.
    S3Express,
    /// AWS S3 Glacier for archival storage.
    S3Glacier,
    /// Google Cloud Storage Standard class.
    GCSStandard,
    /// Google Cloud Storage Nearline for infrequent access.
    GCSNearline,
    /// Google Cloud Storage Archive for long-term retention.
    GCSArchive,
    /// Azure Blob Storage hot tier.
    AzureHot,
    /// Azure Blob Storage cool tier.
    AzureCool,
    /// Azure Blob Storage archive tier.
    AzureArchive,
}

/// Collection state manager
pub struct CollectionStateManager {
    /// Collection states
    states: Arc<DashMap<String, CollectionTierState>>,

    /// Access history for heat scoring
    access_history: Arc<DashMap<String, AccessHistory>>,

    /// Tier transition history
    transition_history: Arc<DashMap<String, Vec<TierTransition>>>,
}

/// Access history for a collection
#[derive(Debug, Clone)]
pub struct AccessHistory {
    /// Timestamps of recent accesses for windowed analysis.
    pub recent_accesses: Vec<Instant>,
    /// Number of accesses in the last hour.
    pub access_count_1h: u64,
    /// Number of accesses in the last 24 hours.
    pub access_count_24h: u64,
    /// Number of accesses in the last 7 days.
    pub access_count_7d: u64,
    /// Average latency in milliseconds when falling back to lower tiers.
    pub avg_fallback_latency_ms: f64,
    /// Computed importance score used for tier placement decisions.
    pub importance_score: f32,
}

/// Tier transition record
#[derive(Debug, Clone)]
pub struct TierTransition {
    /// When the transition occurred.
    pub timestamp: DateTime<Utc>,
    /// Tier the data was migrated from.
    pub from_tier: TierLevel,
    /// Tier the data was migrated to.
    pub to_tier: TierLevel,
    /// Reason that triggered this transition.
    pub reason: TransitionReason,
    /// Duration of the migration in milliseconds.
    pub duration_ms: u64,
    /// Amount of data transferred in bytes.
    pub data_size_bytes: usize,
}

/// Reasons for tier transitions
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TransitionReason {
    /// Promoted because a user query needed this data.
    UserQuery,
    /// Promoted due to high access frequency pattern.
    HighFrequency,
    /// Demoted to free memory under pressure.
    MemoryPressure,
    /// Demoted due to prolonged inactivity.
    LowAccess,
    /// Triggered by a time-based policy schedule.
    Scheduled,
    /// Explicitly triggered by an administrator.
    Manual,
    /// Proactively promoted based on predicted access.
    Preload,
}

impl CollectionStateManager {
    /// Create a new state manager
    pub fn new() -> Self {
        Self {
            states: Arc::new(DashMap::new()),
            access_history: Arc::new(DashMap::new()),
            transition_history: Arc::new(DashMap::new()),
        }
    }

    /// Get current state for a collection
    pub fn get_state(&self, collection_id: &str) -> Option<CollectionTierState> {
        self.states.get(collection_id).map(|s| s.clone())
    }

    /// Set state for a collection
    pub fn set_state(&self, collection_id: &str, state: CollectionTierState) {
        self.states.insert(collection_id.to_string(), state);
    }

    /// Check if collection is in memory
    pub fn is_in_memory(&self, collection_id: &str) -> bool {
        matches!(
            self.get_state(collection_id),
            Some(CollectionTierState::Memory { .. })
        )
    }

    /// Check if collection is on disk
    pub fn is_on_disk(&self, collection_id: &str) -> bool {
        matches!(
            self.get_state(collection_id),
            Some(CollectionTierState::Disk { .. })
        )
    }

    /// Check if collection is in cloud
    pub fn is_in_cloud(&self, collection_id: &str) -> bool {
        matches!(
            self.get_state(collection_id),
            Some(CollectionTierState::Cloud { .. })
        )
    }

    /// Start a tier transition
    pub fn start_transition(
        &self,
        collection_id: &str,
        from: CollectionTierState,
        to: TierLevel,
    ) -> Result<()> {
        let transitioning = CollectionTierState::Transitioning {
            from: Box::new(from),
            to,
            started_at: Instant::now(),
            progress: 0.0,
        };

        self.set_state(collection_id, transitioning);

        info!("🔄 Starting transition for {} to {:?}", collection_id, to);
        Ok(())
    }

    /// Update transition progress
    pub fn update_transition_progress(&self, collection_id: &str, progress: f32) -> Result<()> {
        if let Some(mut state) = self.states.get_mut(collection_id)
            && let CollectionTierState::Transitioning {
                progress: ref mut p,
                ..
            } = *state
            {
                *p = progress;
                debug!(
                    "📊 Transition progress for {}: {:.1}%",
                    collection_id,
                    progress * 100.0
                );
            }
        Ok(())
    }

    /// Complete a tier transition
    pub fn complete_transition(
        &self,
        collection_id: &str,
        new_state: CollectionTierState,
        reason: TransitionReason,
    ) -> Result<()> {
        // Get the old state for history
        let (from_tier, to_tier, started_at) = if let Some(state) = self.get_state(collection_id) {
            match state {
                CollectionTierState::Transitioning {
                    from,
                    to,
                    started_at,
                    ..
                } => {
                    let from_tier = Self::state_to_tier(&*from);
                    (from_tier, to, started_at)
                }
                _ => {
                    return Err(anyhow!(
                        "Collection {} not in transitioning state",
                        collection_id
                    ));
                }
            }
        } else {
            return Err(anyhow!("Collection {} not found", collection_id));
        };

        // Calculate transition duration
        let duration_ms = started_at.elapsed().as_millis() as u64;

        // Record transition in history
        let transition = TierTransition {
            timestamp: Utc::now(),
            from_tier,
            to_tier,
            reason,
            duration_ms,
            data_size_bytes: Self::get_state_size(&new_state),
        };

        self.transition_history
            .entry(collection_id.to_string())
            .or_default()
            .push(transition);

        // Update state
        self.set_state(collection_id, new_state);

        info!(
            "✅ Completed transition for {} to {:?} ({} ms)",
            collection_id, to_tier, duration_ms
        );

        Ok(())
    }

    /// Record an access for heat scoring
    pub fn record_access(&self, collection_id: &str) {
        let mut history = self
            .access_history
            .entry(collection_id.to_string())
            .or_insert_with(|| AccessHistory {
                recent_accesses: Vec::new(),
                access_count_1h: 0,
                access_count_24h: 0,
                access_count_7d: 0,
                avg_fallback_latency_ms: 0.0,
                importance_score: 50.0,
            });

        let now = Instant::now();
        history.recent_accesses.push(now);

        // Keep only recent accesses (last 7 days)
        let cutoff = now - Duration::from_secs(7 * 24 * 3600);
        history.recent_accesses.retain(|&t| t > cutoff);

        // Update counters
        let hour_ago = now - Duration::from_secs(3600);
        let day_ago = now - Duration::from_secs(24 * 3600);

        history.access_count_1h = history
            .recent_accesses
            .iter()
            .filter(|&&t| t > hour_ago)
            .count() as u64;

        history.access_count_24h = history
            .recent_accesses
            .iter()
            .filter(|&&t| t > day_ago)
            .count() as u64;

        history.access_count_7d = history.recent_accesses.len() as u64;
    }

    /// Get all collection states
    pub async fn get_all_states(&self) -> Result<Vec<(String, CollectionTierState)>> {
        let states: Vec<(String, CollectionTierState)> = self
            .states
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect();
        Ok(states)
    }

    /// List all collections
    pub async fn list_collections(&self) -> Result<Vec<String>> {
        let collections: Vec<String> = self
            .states
            .iter()
            .map(|entry| entry.key().clone())
            .collect();
        Ok(collections)
    }

    /// Transition collection to memory
    pub async fn transition_to_memory(&self, collection_id: &str) -> Result<()> {
        let state = CollectionTierState::Memory {
            loaded_at: Instant::now(),
            memory_bytes: 0,
            access_count: 0,
            last_access: Instant::now(),
            generation: 1,
        };
        self.set_state(collection_id, state);
        Ok(())
    }

    /// Transition collection to disk
    pub async fn transition_to_disk(&self, collection_id: &str, path: String) -> Result<()> {
        let state = CollectionTierState::Disk {
            stored_at: Instant::now(),
            disk_location: PathBuf::from(path),
            disk_bytes: 0,
            last_access: Some(Instant::now()),
            promotion_eligible: true,
        };
        self.set_state(collection_id, state);
        Ok(())
    }

    /// Calculate heat score for a collection
    pub fn calculate_heat_score(&self, collection_id: &str) -> f64 {
        if let Some(history) = self.access_history.get(collection_id) {
            let recency_score = if let Some(&last) = history.recent_accesses.last() {
                let age_secs = last.elapsed().as_secs_f64();
                (-age_secs / 86400.0).exp() // Exponential decay over days
            } else {
                0.0
            };

            let frequency_score = (history.access_count_1h as f64 * 1.0
                + history.access_count_24h as f64 * 0.5
                + history.access_count_7d as f64 * 0.1)
                / 100.0;

            let importance = history.importance_score as f64 / 100.0;

            (recency_score * 0.4 + frequency_score * 0.4 + importance * 0.2).min(100.0)
        } else {
            0.0
        }
    }

    /// Get collections sorted by heat score
    pub fn get_collections_by_heat(&self) -> Vec<(String, f64)> {
        let mut collections: Vec<(String, f64)> = self
            .states
            .iter()
            .map(|entry| {
                let collection_id = entry.key().clone();
                let heat_score = self.calculate_heat_score(&collection_id);
                (collection_id, heat_score)
            })
            .collect();

        collections.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        collections
    }

    /// Get demotion candidates (cold collections in memory)
    pub fn get_demotion_candidates(&self, threshold: f64) -> Vec<String> {
        self.states
            .iter()
            .filter_map(|entry| {
                let collection_id = entry.key().clone();
                if matches!(entry.value(), CollectionTierState::Memory { .. }) {
                    let heat_score = self.calculate_heat_score(&collection_id);
                    if heat_score < threshold {
                        Some(collection_id)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect()
    }

    /// Get promotion candidates (hot collections not in memory)
    pub fn get_promotion_candidates(&self, threshold: f64) -> Vec<String> {
        self.states
            .iter()
            .filter_map(|entry| {
                let collection_id = entry.key().clone();
                if !matches!(entry.value(), CollectionTierState::Memory { .. }) {
                    let heat_score = self.calculate_heat_score(&collection_id);
                    if heat_score > threshold {
                        Some(collection_id)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect()
    }

    /// Get memory usage statistics
    pub fn get_memory_stats(&self) -> MemoryUsageStats {
        let mut total_memory_bytes = 0;
        let mut collections_in_memory = 0;
        let mut collections_on_disk = 0;
        let mut collections_in_cloud = 0;
        let mut collections_transitioning = 0;

        for entry in self.states.iter() {
            match entry.value() {
                CollectionTierState::Memory { memory_bytes, .. } => {
                    total_memory_bytes += memory_bytes;
                    collections_in_memory += 1;
                }
                CollectionTierState::Disk { .. } => {
                    collections_on_disk += 1;
                }
                CollectionTierState::Cloud { .. } => {
                    collections_in_cloud += 1;
                }
                CollectionTierState::Transitioning { .. } => {
                    collections_transitioning += 1;
                }
                _ => {}
            }
        }

        MemoryUsageStats {
            total_memory_bytes,
            collections_in_memory,
            collections_on_disk,
            collections_in_cloud,
            collections_transitioning,
        }
    }

    // Helper methods

    fn state_to_tier(state: &CollectionTierState) -> TierLevel {
        match state {
            CollectionTierState::Memory { .. } => TierLevel::Memory,
            CollectionTierState::Disk { .. } => TierLevel::Disk,
            CollectionTierState::Cloud { .. } => TierLevel::Cloud,
            _ => TierLevel::Memory, // Default
        }
    }

    fn get_state_size(state: &CollectionTierState) -> usize {
        match state {
            CollectionTierState::Memory { memory_bytes, .. } => *memory_bytes,
            CollectionTierState::Disk { disk_bytes, .. } => *disk_bytes,
            CollectionTierState::Cloud {
                compressed_bytes, ..
            } => *compressed_bytes,
            _ => 0,
        }
    }
}

impl Default for CollectionStateManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Memory usage statistics
#[derive(Debug, Clone)]
pub struct MemoryUsageStats {
    /// Total memory consumed by all in-memory collections in bytes.
    pub total_memory_bytes: usize,
    /// Number of collections currently loaded in memory.
    pub collections_in_memory: usize,
    /// Number of collections stored on local disk.
    pub collections_on_disk: usize,
    /// Number of collections stored in cloud object storage.
    pub collections_in_cloud: usize,
    /// Number of collections currently transitioning between tiers.
    pub collections_transitioning: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn test_state_transitions() {
        let manager = CollectionStateManager::new();
        let collection_id = "test_collection";

        // Start as unbuilt
        manager.set_state(collection_id, CollectionTierState::Unbuilt);
        assert!(!manager.is_in_memory(collection_id));

        // Transition to memory
        let memory_state = CollectionTierState::Memory {
            loaded_at: Instant::now(),
            memory_bytes: 1_000_000,
            access_count: 0,
            last_access: Instant::now(),
            generation: 1,
        };

        manager
            .start_transition(
                collection_id,
                CollectionTierState::Unbuilt,
                TierLevel::Memory,
            )
            .unwrap();

        manager
            .complete_transition(collection_id, memory_state, TransitionReason::UserQuery)
            .unwrap();

        assert!(manager.is_in_memory(collection_id));
    }

    #[test]
    fn test_heat_scoring() {
        let manager = CollectionStateManager::new();
        let collection_id = "hot_collection";

        // First set the collection state so it appears in states
        manager.set_state(
            collection_id,
            CollectionTierState::Memory {
                loaded_at: Instant::now(),
                memory_bytes: 1_000_000,
                access_count: 0,
                last_access: Instant::now(),
                generation: 1,
            },
        );

        // Record multiple accesses
        for _ in 0..10 {
            manager.record_access(collection_id);
        }

        let heat_score = manager.calculate_heat_score(collection_id);
        assert!(heat_score > 0.0);

        // Check it appears in hot collections
        let hot_collections = manager.get_collections_by_heat();
        assert!(hot_collections.iter().any(|(id, _)| id == collection_id));
    }

    #[test]
    fn test_promotion_demotion_candidates() {
        let manager = CollectionStateManager::new();

        // Add a hot collection on disk
        let hot_id = "hot_on_disk";
        manager.set_state(
            hot_id,
            CollectionTierState::Disk {
                stored_at: Instant::now(),
                disk_location: PathBuf::from("/tmp/hot"),
                disk_bytes: 1_000_000,
                last_access: Some(Instant::now()),
                promotion_eligible: true,
            },
        );

        // Record many accesses to make it hot (this will create access history)
        for _ in 0..50 {
            manager.record_access(hot_id);
        }

        // Add a cold collection in memory
        let cold_id = "cold_in_memory";
        manager.set_state(
            cold_id,
            CollectionTierState::Memory {
                loaded_at: Instant::now() - Duration::from_secs(3600),
                memory_bytes: 1_000_000,
                access_count: 1,
                last_access: Instant::now() - Duration::from_secs(3600),
                generation: 1,
            },
        );

        // Don't record any accesses for cold collection to keep it cold
        // Just initialize its access history with low importance
        manager.access_history.insert(
            cold_id.to_string(),
            AccessHistory {
                recent_accesses: vec![Instant::now() - Duration::from_secs(7200)],
                access_count_1h: 0,
                access_count_24h: 0,
                access_count_7d: 1,
                avg_fallback_latency_ms: 0.0,
                importance_score: 10.0,
            },
        );

        // Check candidates with adjusted thresholds
        let promotion_candidates = manager.get_promotion_candidates(0.1);
        assert!(promotion_candidates.contains(&hot_id.to_string()));

        let demotion_candidates = manager.get_demotion_candidates(0.5);
        assert!(demotion_candidates.contains(&cold_id.to_string()));
    }
}
