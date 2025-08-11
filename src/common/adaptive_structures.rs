/*
 * Copyright 2025 ProximaDB
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

//! Adaptive data structures foundation for ProximaDB
//!
//! This module provides the unified foundation for adaptive data structures that
//! automatically adjust their behavior based on workload characteristics and 
//! integrate with the GlobalTierManager for multi-tier storage.
//!
//! ## Architecture Overview
//!
//! ```
//! AdaptiveStore
//!     ↓
//! ┌─────────────┬─────────────┬─────────────┐
//! │ IndexBackend│ CacheBackend│ HybridBackend│
//! │ (DashMap)   │ (Moka)      │ (Adaptive)  │
//! └─────────────┴─────────────┴─────────────┘
//!     ↓            ↓            ↓
//! ┌──────────────────────────────────────────┐
//! │      UniversalTierManager                │
//! │   (Memory→NVMe→HDD→Cloud hierarchy)      │
//! └──────────────────────────────────────────┘
//! ```
//!
//! ## Key Design Principles
//!
//! 1. **Workload-Aware Selection**: Automatically choose optimal data structures
//! 2. **Shared Tiering Infrastructure**: Single GlobalTierManager per server
//! 3. **Never-Evict vs Evictable**: Indexes retain data, caches can evict
//! 4. **Collection-Aware Policies**: Per-collection constraints from base_location
//! 5. **Unified Metrics**: Comprehensive observability across all backends

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use dashmap::DashMap;
use moka::future::Cache as MokaCache;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, info};

use crate::common::concurrent_structures::{AtomicMetrics, MetricsSnapshot};
use crate::common::tier_policy_engine::{
    GlobalTierManager, SmartTierPolicy, StorageTier, WorkloadPattern, WorkloadMetrics,
    CollectionStorageConfig, CollectionStorageLimits,
};

/// Adaptive storage interface that chooses optimal backend based on workload
#[async_trait]
pub trait AdaptiveStore<K, V>: Send + Sync
where
    K: Hash + Eq + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    /// Insert a key-value pair
    async fn insert(&self, key: K, value: V) -> Result<Option<V>>;

    /// Get a value by key
    async fn get(&self, key: &K) -> Option<V>;

    /// Remove a key-value pair
    async fn remove(&self, key: &K) -> Option<V>;

    /// Check if key exists
    async fn contains(&self, key: &K) -> bool;

    /// Get current size
    async fn len(&self) -> usize;

    /// Check if empty
    async fn is_empty(&self) -> bool;

    /// Get all keys
    async fn keys(&self) -> Vec<K>;

    /// Clear all entries
    async fn clear(&self);

    /// Get performance metrics
    async fn metrics(&self) -> MetricsSnapshot;

    /// Get workload metrics for analysis
    async fn workload_metrics(&self) -> WorkloadMetrics;

    /// Trigger tier management operations (promotion/demotion)
    async fn rebalance_tiers(&self) -> Result<TierRebalanceResult>;
}

/// Backend type classification for workload optimization
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BackendType {
    /// Index backend: Can evict (durability provided by AXIS storage)
    /// AXIS maintains index data at {baseurl}/{collectionid}/indexes/
    Index {
        /// Primary data structure (DashMap for concurrent bulk operations)
        structure: IndexStructure,
        /// Tier policy (unified eviction - same as cache!)
        tier_policy: UnifiedTierPolicy,
    },
    /// Cache backend: Can evict, uses memory pressure for promotion
    Cache {
        /// Primary data structure (Moka for automatic eviction)
        structure: CacheStructure,
        /// Tier policy (unified eviction - same policy type as index!)
        tier_policy: UnifiedTierPolicy,
    },
    /// Hybrid backend: Adaptive behavior based on workload detection
    Hybrid {
        /// Currently active structure
        active_structure: HybridStructure,
        /// Workload detection settings
        detection_config: WorkloadDetectionConfig,
    },
}

/// Index-specific data structures
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexStructure {
    /// DashMap for lock-free concurrent operations
    DashMap {
        initial_capacity: usize,
        memory_limit_mb: Option<usize>,
    },
    /// HashMap with RwLock for simple cases
    RwLockHashMap {
        initial_capacity: usize,
    },
}

/// Cache-specific data structures  
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CacheStructure {
    /// Moka cache with automatic eviction
    Moka {
        max_capacity: u64,
        time_to_live: Option<Duration>,
        time_to_idle: Option<Duration>,
    },
    /// LRU cache for simple eviction
    Lru {
        max_capacity: usize,
    },
}

/// Hybrid structure that can switch between backends
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HybridStructure {
    /// Currently using index-optimized structure
    IndexMode(IndexStructure),
    /// Currently using cache-optimized structure
    CacheMode(CacheStructure),
}

/// Unified tier policy for both index and cache backends
/// KEY INSIGHT: Both can evict because durability is provided by AXIS storage
/// at {baseurl}/{collectionid}/indexes/ which is the source of truth
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnifiedTierPolicy {
    /// Eviction policy (applies to BOTH index and cache backends!)
    pub eviction_policy: EvictionPolicy,
    /// Promotion criteria for moving to faster tiers
    pub promotion_criteria: PromotionCriteria,
    /// Demotion criteria for moving to slower tiers
    pub demotion_criteria: DemotionCriteria,
    /// Reload strategy for restartability
    pub reload_strategy: ReloadStrategy,
}

/// Reload strategy for restartability
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReloadStrategy {
    /// Load data from AXIS storage on startup
    pub load_on_startup: bool,
    /// Prefetch hot data based on historical access patterns
    pub prefetch_hot_data: bool,
    /// Maximum items to load initially
    pub max_initial_load: usize,
    /// AXIS storage path pattern: {baseurl}/{collection_id}/indexes/
    pub axis_storage_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EvictionPolicy {
    /// Never evict, promote to next tier
    NeverEvict,
    /// LRU-based eviction
    Lru { max_entries: usize },
    /// Size-based eviction
    SizeBased { max_memory_mb: usize },
    /// Time-based eviction
    TimeBased { max_age: Duration },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotionCriteria {
    /// Minimum access frequency for promotion
    pub min_access_frequency: u64,
    /// Time window for frequency calculation
    pub frequency_window: Duration,
    /// Minimum tier for promotion consideration
    pub min_promotion_tier: StorageTier,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DemotionCriteria {
    /// Maximum idle time before demotion
    pub max_idle_time: Duration,
    /// Memory pressure threshold (0.0-1.0)
    pub memory_pressure_threshold: f64,
    /// Minimum tier (won't demote below this)
    pub min_tier: StorageTier,
}

/// Workload detection configuration for hybrid backends
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkloadDetectionConfig {
    /// Sample size for workload analysis
    sample_size: usize,
    /// Analysis window duration
    analysis_window: Duration,
    /// Threshold for switching to index mode (write ratio)
    index_mode_write_threshold: f64,
    /// Threshold for switching to cache mode (read ratio)
    cache_mode_read_threshold: f64,
    /// Minimum confidence for mode switching
    switch_confidence_threshold: f64,
}

/// Result of tier rebalancing operation
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TierRebalanceResult {
    /// Number of items promoted
    pub promoted_count: usize,
    /// Number of items demoted
    pub demoted_count: usize,
    /// Number of items evicted
    pub evicted_count: usize,
    /// Total time taken
    pub duration: Duration,
    /// Memory freed (bytes)
    pub memory_freed_bytes: usize,
    /// Memory allocated (bytes)
    pub memory_allocated_bytes: usize,
}

/// Configuration for adaptive store creation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdaptiveStoreConfig {
    /// Collection ID this store belongs to
    pub collection_id: String,
    /// Backend type configuration
    pub backend_type: BackendType,
    /// Tier management settings
    pub tier_config: TierConfig,
    /// Metrics collection settings
    pub metrics_config: MetricsConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierConfig {
    /// Enable tier management
    pub enable_tiering: bool,
    /// Rebalancing interval
    pub rebalance_interval: Duration,
    /// Memory pressure threshold for tier operations
    pub memory_pressure_threshold: f64,
    /// Maximum concurrent tier operations
    pub max_concurrent_operations: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsConfig {
    /// Enable detailed workload metrics
    pub enable_workload_metrics: bool,
    /// Metrics collection interval
    pub collection_interval: Duration,
    /// History retention duration
    pub history_retention: Duration,
}

/// Universal tier manager integration
pub struct UniversalTierManager {
    /// Reference to global tier manager
    global_manager: Arc<GlobalTierManager>,
    /// Collection-specific configurations
    collection_configs: DashMap<String, SmartTierPolicy>,
    /// Active tier operations
    active_operations: AtomicMetrics,
    /// Overall performance metrics
    performance_metrics: AtomicMetrics,
}

impl UniversalTierManager {
    /// Create new universal tier manager
    pub async fn new(global_manager: Arc<GlobalTierManager>) -> Result<Self> {
        Ok(Self {
            global_manager,
            collection_configs: DashMap::new(),
            active_operations: AtomicMetrics::new(),
            performance_metrics: AtomicMetrics::new(),
        })
    }

    /// Register a collection with tier management
    pub async fn register_collection(
        &self,
        collection_id: String,
        tier_policy: SmartTierPolicy,
    ) -> Result<()> {
        info!(
            "Registering collection '{}' with tier policy",
            collection_id
        );

        self.collection_configs.insert(collection_id.clone(), tier_policy);

        // Initialize collection in global manager
        self.global_manager
            .initialize_collection(&collection_id)
            .await?;

        Ok(())
    }

    /// Get tier policy for a collection
    pub fn get_tier_policy(&self, collection_id: &str) -> Option<SmartTierPolicy> {
        self.collection_configs
            .get(collection_id)
            .map(|entry| entry.clone())
    }

    /// Perform tier rebalancing for a collection
    pub async fn rebalance_collection(&self, collection_id: &str) -> Result<TierRebalanceResult> {
        let start = Instant::now();

        let tier_policy = self
            .get_tier_policy(collection_id)
            .ok_or_else(|| anyhow!("No tier policy found for collection: {}", collection_id))?;

        // Delegate to global manager for actual rebalancing
        let result = self
            .global_manager
            .rebalance_collection_tiers(collection_id, &tier_policy)
            .await?;

        let duration = start.elapsed();
        self.performance_metrics.record_success(duration);

        debug!(
            "Rebalanced tiers for collection '{}': promoted={}, demoted={}, evicted={} in {:?}",
            collection_id,
            result.promoted_count,
            result.demoted_count,
            result.evicted_count,
            duration
        );

        Ok(result)
    }

    /// Get global tier manager reference
    pub fn global_manager(&self) -> &Arc<GlobalTierManager> {
        &self.global_manager
    }

    /// Get performance metrics
    pub fn metrics(&self) -> MetricsSnapshot {
        self.performance_metrics.snapshot()
    }
}

/// Factory for creating adaptive stores
pub struct AdaptiveStoreFactory {
    /// Universal tier manager
    tier_manager: Arc<UniversalTierManager>,
    /// Default configurations
    default_configs: HashMap<String, AdaptiveStoreConfig>,
}

impl AdaptiveStoreFactory {
    /// Create new factory
    pub fn new(tier_manager: Arc<UniversalTierManager>) -> Self {
        Self {
            tier_manager,
            default_configs: Self::create_default_configs(),
        }
    }

    /// Create adaptive store for a collection
    pub async fn create_store<K, V>(
        &self,
        collection_id: String,
        config: Option<AdaptiveStoreConfig>,
    ) -> Result<Box<dyn AdaptiveStore<K, V>>>
    where
        K: Hash + Eq + Clone + Send + Sync + 'static,
        V: Clone + Send + Sync + 'static,
    {
        let config = config.unwrap_or_else(|| {
            self.get_default_config(&collection_id)
        });

        // Register collection with tier manager
        let tier_policy = self.create_tier_policy(&config)?;
        self.tier_manager
            .register_collection(collection_id.clone(), tier_policy)
            .await?;

        // Create appropriate backend
        match &config.backend_type {
            BackendType::Index { structure, tier_policy } => {
                self.create_index_backend(collection_id, structure.clone(), tier_policy.clone(), config).await
            }
            BackendType::Cache { structure, tier_policy } => {
                self.create_cache_backend(collection_id, structure.clone(), tier_policy.clone(), config).await
            }
            BackendType::Hybrid { active_structure, detection_config } => {
                self.create_hybrid_backend(collection_id, active_structure.clone(), detection_config.clone(), config).await
            }
        }
    }

    /// Create index backend
    async fn create_index_backend<K, V>(
        &self,
        collection_id: String,
        structure: IndexStructure,
        tier_policy: UnifiedTierPolicy,
        config: AdaptiveStoreConfig,
    ) -> Result<Box<dyn AdaptiveStore<K, V>>>
    where
        K: Hash + Eq + Clone + Send + Sync + 'static,
        V: Clone + Send + Sync + 'static,
    {
        match structure {
            IndexStructure::DashMap { initial_capacity, memory_limit_mb } => {
                Ok(Box::new(
                    IndexBackend::new_dashmap(
                        collection_id,
                        initial_capacity,
                        memory_limit_mb,
                        tier_policy,
                        config,
                        self.tier_manager.clone(),
                    ).await?
                ))
            }
            IndexStructure::RwLockHashMap { initial_capacity } => {
                Ok(Box::new(
                    IndexBackend::new_rwlock_hashmap(
                        collection_id,
                        initial_capacity,
                        tier_policy,
                        config,
                        self.tier_manager.clone(),
                    ).await?
                ))
            }
        }
    }

    /// Create cache backend
    async fn create_cache_backend<K, V>(
        &self,
        collection_id: String,
        structure: CacheStructure,
        tier_policy: UnifiedTierPolicy,
        config: AdaptiveStoreConfig,
    ) -> Result<Box<dyn AdaptiveStore<K, V>>>
    where
        K: Hash + Eq + Clone + Send + Sync + 'static,
        V: Clone + Send + Sync + 'static,
    {
        match structure {
            CacheStructure::Moka { max_capacity, time_to_live, time_to_idle } => {
                Ok(Box::new(
                    CacheBackend::new_moka(
                        collection_id,
                        max_capacity,
                        time_to_live,
                        time_to_idle,
                        tier_policy,
                        config,
                        self.tier_manager.clone(),
                    ).await?
                ))
            }
            CacheStructure::Lru { max_capacity } => {
                Ok(Box::new(
                    CacheBackend::new_lru(
                        collection_id,
                        max_capacity,
                        tier_policy,
                        config,
                        self.tier_manager.clone(),
                    ).await?
                ))
            }
        }
    }

    /// Create hybrid backend
    async fn create_hybrid_backend<K, V>(
        &self,
        collection_id: String,
        active_structure: HybridStructure,
        detection_config: WorkloadDetectionConfig,
        config: AdaptiveStoreConfig,
    ) -> Result<Box<dyn AdaptiveStore<K, V>>>
    where
        K: Hash + Eq + Clone + Send + Sync + 'static,
        V: Clone + Send + Sync + 'static,
    {
        Ok(Box::new(
            HybridBackend::new(
                collection_id,
                active_structure,
                detection_config,
                config,
                self.tier_manager.clone(),
            ).await?
        ))
    }

    /// Create tier policy from config
    fn create_tier_policy(&self, config: &AdaptiveStoreConfig) -> Result<SmartTierPolicy> {
        // This would integrate with the actual GlobalTierManager's policy creation
        // For now, return a default policy based on backend type
        
        // Create collection config
        let collection_config = CollectionStorageConfig {
            collection_id: config.collection_id.clone(),
            base_location: "/tmp".to_string(),
            durable_baseline: StorageTier::HardDisk { mount_path: "/mnt/hdd".to_string() },
            max_acceleration_tier: Some(StorageTier::Memory),
            storage_limits: CollectionStorageLimits {
                max_memory_bytes: Some(1024 * 1024 * 1024), // 1GB
                max_local_disk_bytes: None,
                max_monthly_cost_usd: None,
            },
        };
        
        // Create default available tiers
        let available_tiers = vec![
            StorageTier::Memory,
            StorageTier::NvmeSsd { mount_path: "/mnt/nvme".to_string() },
            StorageTier::HardDisk { mount_path: "/mnt/hdd".to_string() },
        ];
        
        // Create tier configs
        let tier_configs = HashMap::new();
        
        // Use the appropriate constructor based on backend type
        let policy = match &config.backend_type {
            BackendType::Index { .. } => {
                SmartTierPolicy::for_index_workload_constrained(
                    collection_config,
                    &available_tiers,
                    &tier_configs,
                )
            },
            BackendType::Cache { .. } => {
                SmartTierPolicy::for_cache_workload_constrained(
                    collection_config,
                    &available_tiers,
                    &tier_configs,
                )
            },
            BackendType::Hybrid { .. } => {
                SmartTierPolicy::for_hybrid_workload_constrained(
                    collection_config,
                    &available_tiers,
                    &tier_configs,
                )
            },
        };
        
        Ok(policy)
    }

    /// Get default configuration for collection
    fn get_default_config(&self, collection_id: &str) -> AdaptiveStoreConfig {
        // Check if we have a specific default for this collection
        if let Some(config) = self.default_configs.get(collection_id) {
            return config.clone();
        }

        // Return generic default
        self.create_generic_default(collection_id.to_string())
    }

    /// Create default configurations
    fn create_default_configs() -> HashMap<String, AdaptiveStoreConfig> {
        let mut configs = HashMap::new();

        // Default index configuration
        configs.insert("index_default".to_string(), AdaptiveStoreConfig {
            collection_id: "index_default".to_string(),
            backend_type: BackendType::Index {
                structure: IndexStructure::DashMap {
                    initial_capacity: 1024,
                    memory_limit_mb: Some(512),
                },
                tier_policy: UnifiedTierPolicy {
                    eviction_policy: EvictionPolicy::SizeBased { max_memory_mb: 512 },
                    promotion_criteria: PromotionCriteria {
                        min_access_frequency: 100,
                        frequency_window: Duration::from_secs(3600),
                        min_promotion_tier: StorageTier::Memory,
                    },
                    demotion_criteria: DemotionCriteria {
                        max_idle_time: Duration::from_secs(7200),
                        memory_pressure_threshold: 0.85,
                        min_tier: StorageTier::HardDisk { mount_path: "/mnt/hdd".to_string() },
                    },
                    reload_strategy: ReloadStrategy {
                        load_on_startup: true,
                        prefetch_hot_data: true,
                        max_initial_load: 10000,
                        axis_storage_path: "{baseurl}/{collection_id}/indexes/".to_string(),
                    },
                },
            },
            tier_config: TierConfig {
                enable_tiering: true,
                rebalance_interval: Duration::from_secs(300),
                memory_pressure_threshold: 0.8,
                max_concurrent_operations: 4,
            },
            metrics_config: MetricsConfig {
                enable_workload_metrics: true,
                collection_interval: Duration::from_secs(60),
                history_retention: Duration::from_secs(3600),
            },
        });

        // Default cache configuration
        configs.insert("cache_default".to_string(), AdaptiveStoreConfig {
            collection_id: "cache_default".to_string(),
            backend_type: BackendType::Cache {
                structure: CacheStructure::Moka {
                    max_capacity: 10000,
                    time_to_live: Some(Duration::from_secs(3600)),
                    time_to_idle: Some(Duration::from_secs(1800)),
                },
                tier_policy: UnifiedTierPolicy {
                    eviction_policy: EvictionPolicy::Lru { max_entries: 10000 },
                    promotion_criteria: PromotionCriteria {
                        min_access_frequency: 10,
                        frequency_window: Duration::from_secs(300),
                        min_promotion_tier: StorageTier::Memory,
                    },
                    demotion_criteria: DemotionCriteria {
                        max_idle_time: Duration::from_secs(1800),
                        memory_pressure_threshold: 0.9,
                        min_tier: StorageTier::HardDisk { mount_path: "/mnt/hdd".to_string() },
                    },
                    reload_strategy: ReloadStrategy {
                        load_on_startup: false,
                        prefetch_hot_data: false,
                        max_initial_load: 0,
                        axis_storage_path: "{baseurl}/{collection_id}/cache/".to_string(),
                    },
                },
            },
            tier_config: TierConfig {
                enable_tiering: true,
                rebalance_interval: Duration::from_secs(120),
                memory_pressure_threshold: 0.9,
                max_concurrent_operations: 2,
            },
            metrics_config: MetricsConfig {
                enable_workload_metrics: true,
                collection_interval: Duration::from_secs(30),
                history_retention: Duration::from_secs(1800),
            },
        });

        configs
    }

    /// Create generic default configuration
    fn create_generic_default(&self, collection_id: String) -> AdaptiveStoreConfig {
        AdaptiveStoreConfig {
            collection_id,
            backend_type: BackendType::Hybrid {
                active_structure: HybridStructure::IndexMode(IndexStructure::DashMap {
                    initial_capacity: 512,
                    memory_limit_mb: Some(256),
                }),
                detection_config: WorkloadDetectionConfig {
                    sample_size: 1000,
                    analysis_window: Duration::from_secs(300),
                    index_mode_write_threshold: 0.3,
                    cache_mode_read_threshold: 0.8,
                    switch_confidence_threshold: 0.95,
                },
            },
            tier_config: TierConfig {
                enable_tiering: true,
                rebalance_interval: Duration::from_secs(180),
                memory_pressure_threshold: 0.85,
                max_concurrent_operations: 3,
            },
            metrics_config: MetricsConfig {
                enable_workload_metrics: true,
                collection_interval: Duration::from_secs(45),
                history_retention: Duration::from_secs(2700),
            },
        }
    }
}

// Forward declarations for the backend implementations
// These will be implemented in separate files

/// Index backend using DashMap with tier management
pub struct IndexBackend<K, V> {
    collection_id: String,
    storage: DashMap<K, V>,
    tier_policy: UnifiedTierPolicy,
    config: AdaptiveStoreConfig,
    tier_manager: Arc<UniversalTierManager>,
    metrics: AtomicMetrics,
    workload_metrics: RwLock<WorkloadMetrics>,
}

/// Cache backend using Moka with tier management  
pub struct CacheBackend<K, V> {
    collection_id: String,
    storage: MokaCache<K, V>,
    tier_policy: UnifiedTierPolicy,
    config: AdaptiveStoreConfig,
    tier_manager: Arc<UniversalTierManager>,
    metrics: AtomicMetrics,
    workload_metrics: RwLock<WorkloadMetrics>,
}

/// Hybrid backend that can switch between index and cache modes
pub struct HybridBackend<K, V> {
    collection_id: String,
    active_structure: RwLock<HybridStructure>,
    detection_config: WorkloadDetectionConfig,
    config: AdaptiveStoreConfig,
    tier_manager: Arc<UniversalTierManager>,
    metrics: AtomicMetrics,
    workload_metrics: RwLock<WorkloadMetrics>,
    // Storage will be dynamically allocated based on active structure
    index_storage: Option<DashMap<K, V>>,
    cache_storage: Option<MokaCache<K, V>>,
}

// Implementation stubs - these will be implemented in subsequent phases
impl<K, V> IndexBackend<K, V>
where
    K: Hash + Eq + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    async fn new_dashmap(
        collection_id: String,
        initial_capacity: usize,
        _memory_limit_mb: Option<usize>,
        tier_policy: UnifiedTierPolicy,
        config: AdaptiveStoreConfig,
        tier_manager: Arc<UniversalTierManager>,
    ) -> Result<Self> {
        Ok(Self {
            collection_id,
            storage: DashMap::with_capacity(initial_capacity),
            tier_policy,
            config,
            tier_manager,
            metrics: AtomicMetrics::new(),
            workload_metrics: RwLock::new(WorkloadMetrics::new(WorkloadPattern::Mixed)),
        })
    }

    async fn new_rwlock_hashmap(
        collection_id: String,
        initial_capacity: usize,
        tier_policy: UnifiedTierPolicy,
        config: AdaptiveStoreConfig,
        tier_manager: Arc<UniversalTierManager>,
    ) -> Result<Self> {
        Ok(Self {
            collection_id,
            storage: DashMap::with_capacity(initial_capacity),
            tier_policy,
            config,
            tier_manager,
            metrics: AtomicMetrics::new(),
            workload_metrics: RwLock::new(WorkloadMetrics::new(WorkloadPattern::WriteHeavy)),
        })
    }
}

impl<K, V> CacheBackend<K, V>
where
    K: Hash + Eq + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    async fn new_moka(
        collection_id: String,
        max_capacity: u64,
        time_to_live: Option<Duration>,
        time_to_idle: Option<Duration>,
        tier_policy: UnifiedTierPolicy,
        config: AdaptiveStoreConfig,
        tier_manager: Arc<UniversalTierManager>,
    ) -> Result<Self> {
        let mut builder = MokaCache::builder()
            .max_capacity(max_capacity);

        if let Some(ttl) = time_to_live {
            builder = builder.time_to_live(ttl);
        }
        if let Some(tti) = time_to_idle {
            builder = builder.time_to_idle(tti);
        }

        Ok(Self {
            collection_id,
            storage: builder.build(),
            tier_policy,
            config,
            tier_manager,
            metrics: AtomicMetrics::new(),
            workload_metrics: RwLock::new(WorkloadMetrics::new(WorkloadPattern::ReadHeavy)),
        })
    }

    async fn new_lru(
        collection_id: String,
        max_capacity: usize,
        tier_policy: UnifiedTierPolicy,
        config: AdaptiveStoreConfig,
        tier_manager: Arc<UniversalTierManager>,
    ) -> Result<Self> {
        // For now, use Moka as LRU implementation
        Ok(Self {
            collection_id,
            storage: MokaCache::builder()
                .max_capacity(max_capacity as u64)
                .build(),
            tier_policy,
            config,
            tier_manager,
            metrics: AtomicMetrics::new(),
            workload_metrics: RwLock::new(WorkloadMetrics::new(WorkloadPattern::ReadHeavy)),
        })
    }
}

impl<K, V> HybridBackend<K, V>
where
    K: Hash + Eq + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    async fn new(
        collection_id: String,
        active_structure: HybridStructure,
        detection_config: WorkloadDetectionConfig,
        config: AdaptiveStoreConfig,
        tier_manager: Arc<UniversalTierManager>,
    ) -> Result<Self> {
        Ok(Self {
            collection_id,
            active_structure: RwLock::new(active_structure),
            detection_config,
            config,
            tier_manager,
            metrics: AtomicMetrics::new(),
            workload_metrics: RwLock::new(WorkloadMetrics::new(WorkloadPattern::Mixed)),
            index_storage: None,
            cache_storage: None,
        })
    }
}

// Trait implementations will be provided in subsequent phases
// For now, provide empty implementations to avoid compilation errors

#[async_trait]
impl<K, V> AdaptiveStore<K, V> for IndexBackend<K, V>
where
    K: Hash + Eq + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    async fn insert(&self, _key: K, _value: V) -> Result<Option<V>> {
        unimplemented!("IndexBackend implementation pending")
    }

    async fn get(&self, _key: &K) -> Option<V> {
        unimplemented!("IndexBackend implementation pending")
    }

    async fn remove(&self, _key: &K) -> Option<V> {
        unimplemented!("IndexBackend implementation pending")
    }

    async fn contains(&self, _key: &K) -> bool {
        unimplemented!("IndexBackend implementation pending")
    }

    async fn len(&self) -> usize {
        unimplemented!("IndexBackend implementation pending")
    }

    async fn is_empty(&self) -> bool {
        unimplemented!("IndexBackend implementation pending")
    }

    async fn keys(&self) -> Vec<K> {
        unimplemented!("IndexBackend implementation pending")
    }

    async fn clear(&self) {
        unimplemented!("IndexBackend implementation pending")
    }

    async fn metrics(&self) -> MetricsSnapshot {
        self.metrics.snapshot()
    }

    async fn workload_metrics(&self) -> WorkloadMetrics {
        self.workload_metrics.read().await.clone()
    }

    async fn rebalance_tiers(&self) -> Result<TierRebalanceResult> {
        self.tier_manager.rebalance_collection(&self.collection_id).await
    }
}

#[async_trait]
impl<K, V> AdaptiveStore<K, V> for CacheBackend<K, V>
where
    K: Hash + Eq + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    async fn insert(&self, _key: K, _value: V) -> Result<Option<V>> {
        unimplemented!("CacheBackend implementation pending")
    }

    async fn get(&self, _key: &K) -> Option<V> {
        unimplemented!("CacheBackend implementation pending")
    }

    async fn remove(&self, _key: &K) -> Option<V> {
        unimplemented!("CacheBackend implementation pending")
    }

    async fn contains(&self, _key: &K) -> bool {
        unimplemented!("CacheBackend implementation pending")
    }

    async fn len(&self) -> usize {
        unimplemented!("CacheBackend implementation pending")
    }

    async fn is_empty(&self) -> bool {
        unimplemented!("CacheBackend implementation pending")
    }

    async fn keys(&self) -> Vec<K> {
        unimplemented!("CacheBackend implementation pending")
    }

    async fn clear(&self) {
        unimplemented!("CacheBackend implementation pending")
    }

    async fn metrics(&self) -> MetricsSnapshot {
        self.metrics.snapshot()
    }

    async fn workload_metrics(&self) -> WorkloadMetrics {
        self.workload_metrics.read().await.clone()
    }

    async fn rebalance_tiers(&self) -> Result<TierRebalanceResult> {
        self.tier_manager.rebalance_collection(&self.collection_id).await
    }
}

#[async_trait]
impl<K, V> AdaptiveStore<K, V> for HybridBackend<K, V>
where
    K: Hash + Eq + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    async fn insert(&self, _key: K, _value: V) -> Result<Option<V>> {
        unimplemented!("HybridBackend implementation pending")
    }

    async fn get(&self, _key: &K) -> Option<V> {
        unimplemented!("HybridBackend implementation pending")
    }

    async fn remove(&self, _key: &K) -> Option<V> {
        unimplemented!("HybridBackend implementation pending")
    }

    async fn contains(&self, _key: &K) -> bool {
        unimplemented!("HybridBackend implementation pending")
    }

    async fn len(&self) -> usize {
        unimplemented!("HybridBackend implementation pending")
    }

    async fn is_empty(&self) -> bool {
        unimplemented!("HybridBackend implementation pending")
    }

    async fn keys(&self) -> Vec<K> {
        unimplemented!("HybridBackend implementation pending")
    }

    async fn clear(&self) {
        unimplemented!("CacheBackend implementation pending")
    }

    async fn metrics(&self) -> MetricsSnapshot {
        self.metrics.snapshot()
    }

    async fn workload_metrics(&self) -> WorkloadMetrics {
        self.workload_metrics.read().await.clone()
    }

    async fn rebalance_tiers(&self) -> Result<TierRebalanceResult> {
        self.tier_manager.rebalance_collection(&self.collection_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_adaptive_store_factory_creation() {
        // This test will be implemented when the GlobalTierManager is available
        // For now, just test that the structures can be created
        
        let config = AdaptiveStoreConfig {
            collection_id: "test".to_string(),
            backend_type: BackendType::Index {
                structure: IndexStructure::DashMap {
                    initial_capacity: 100,
                    memory_limit_mb: None,
                },
                tier_policy: UnifiedTierPolicy {
                    eviction_policy: EvictionPolicy::SizeBased { max_memory_mb: 100 },
                    promotion_criteria: PromotionCriteria {
                        min_access_frequency: 10,
                        frequency_window: Duration::from_secs(60),
                        min_promotion_tier: StorageTier::Memory,
                    },
                    demotion_criteria: DemotionCriteria {
                        max_idle_time: Duration::from_secs(300),
                        memory_pressure_threshold: 0.8,
                        min_tier: StorageTier::NvmeSsd { mount_path: "/mnt/nvme".to_string() },
                    },
                    reload_strategy: ReloadStrategy {
                        load_on_startup: false,
                        prefetch_hot_data: false,
                        max_initial_load: 0,
                        axis_storage_path: "/tmp/test/indexes/".to_string(),
                    },
                },
            },
            tier_config: TierConfig {
                enable_tiering: true,
                rebalance_interval: Duration::from_secs(60),
                memory_pressure_threshold: 0.8,
                max_concurrent_operations: 2,
            },
            metrics_config: MetricsConfig {
                enable_workload_metrics: true,
                collection_interval: Duration::from_secs(30),
                history_retention: Duration::from_secs(300),
            },
        };

        // Ensure configuration can be serialized/deserialized
        let serialized = serde_json::to_string(&config).unwrap();
        let deserialized: AdaptiveStoreConfig = serde_json::from_str(&serialized).unwrap();
        assert_eq!(config.collection_id, deserialized.collection_id);
    }
}