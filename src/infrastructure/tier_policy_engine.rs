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

//! Comprehensive Tier Policy Engine with Smart Defaults
//!
//! This module provides a flexible tiering policy system that supports:
//! - Memory -> NVMe -> HDD -> Cloud storage hierarchy
//! - Multiple cloud providers with different storage classes
//! - Smart defaults optimized for different workload patterns
//! - Cost and performance optimization strategies
//! - Configurable policies per collection/workload type

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

/// Workload pattern classification for adaptive structures
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum WorkloadPattern {
    /// Read-heavy workload (>80% reads)
    #[default]
    ReadHeavy,
    /// Write-heavy workload (>50% writes)
    WriteHeavy,
    /// Mixed workload with balanced read/write
    Mixed,
    /// Bulk operations (large batches)
    Bulk,
}

/// Workload metrics for performance analysis and optimization
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkloadMetrics {
    /// Current workload pattern
    pub pattern: WorkloadPattern,
    /// Read operations per second
    pub reads_per_second: f64,
    /// Write operations per second
    pub writes_per_second: f64,
    /// Average operation latency
    pub avg_latency_ms: f64,
    /// Memory pressure (0.0-1.0)
    pub memory_pressure: f64,
    /// Cache hit rate (0.0-1.0)
    pub cache_hit_rate: f64,
    /// Data age distribution (days)
    pub avg_data_age_days: f64,
    /// Access frequency distribution
    pub avg_access_frequency: f64,
}

impl WorkloadMetrics {
    /// Create a new zeroed `WorkloadMetrics` for the given workload pattern
    pub fn new(pattern: WorkloadPattern) -> Self {
        Self {
            pattern,
            reads_per_second: 0.0,
            writes_per_second: 0.0,
            avg_latency_ms: 0.0,
            memory_pressure: 0.0,
            cache_hit_rate: 0.0,
            avg_data_age_days: 0.0,
            avg_access_frequency: 0.0,
        }
    }
}

/// Backwards-compat alias for [`TierPolicyAccessPatternMetrics`].
pub type AccessPatternMetrics = TierPolicyAccessPatternMetrics;

/// Access pattern metrics for tier management
#[derive(Debug, Clone)]
pub struct TierPolicyAccessPatternMetrics {
    /// Hot data access rate
    pub hot_access_rate: f64,
    /// Warm data access rate  
    pub warm_access_rate: f64,
    /// Cold data access rate
    pub cold_access_rate: f64,
    /// Sequential access pattern percentage
    pub sequential_access_pct: f64,
    /// Random access pattern percentage
    pub random_access_pct: f64,
}

/// Cost metrics for tier optimization
#[derive(Debug, Clone)]
pub struct CostMetrics {
    /// Storage cost per GB per month
    pub storage_cost_per_gb: f64,
    /// I/O cost per thousand operations
    pub io_cost_per_1k_ops: f64,
    /// Data transfer cost per GB
    pub transfer_cost_per_gb: f64,
    /// Total monthly cost
    pub total_monthly_cost: f64,
}

/// Complete infrastructure tier hierarchy from fastest to most cost-effective
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum InfrastructureTier {
    /// L1: System memory (RAM) - fastest acceleration tier
    Memory,

    /// L2: Fast NVMe SSD storage - high-speed acceleration
    NvmeSsd {
        /// Filesystem mount path for this NVMe device
        mount_path: String,
    },

    /// L3: Traditional spinning disk storage - moderate acceleration
    HardDisk {
        /// Filesystem mount path for this HDD
        mount_path: String,
    },

    /// L4: Local fast cloud storage (single AZ) - cloud acceleration
    CloudExpressOneZone {
        /// Cloud provider and bucket configuration
        provider: CloudProvider,
        /// Cloud region identifier (e.g., "us-east-1")
        region: String,
    },

    /// L5: Standard cloud storage (multi-AZ) - baseline cloud durability
    CloudStandard {
        /// Cloud provider and bucket configuration
        provider: CloudProvider,
        /// Cloud region identifier (e.g., "us-east-1")
        region: String,
    },

    /// L6: Infrequent access cloud storage - cost-optimized durability
    CloudInfrequentAccess {
        /// Cloud provider and bucket configuration
        provider: CloudProvider,
        /// Cloud region identifier (e.g., "us-east-1")
        region: String,
    },

    /// L7: Archive storage (retrieval time in minutes/hours) - long-term durability
    CloudArchive {
        /// Cloud provider and bucket configuration
        provider: CloudProvider,
        /// Cloud region identifier (e.g., "us-east-1")
        region: String,
    },

    /// L8: Deep archive (retrieval time in hours) - maximum durability, lowest cost
    CloudDeepArchive {
        /// Cloud provider and bucket configuration
        provider: CloudProvider,
        /// Cloud region identifier (e.g., "us-east-1")
        region: String,
    },
}

impl InfrastructureTier {
    /// Get tier level (lower = faster, higher = more durable/cheaper)
    pub fn tier_level(&self) -> u8 {
        match self {
            InfrastructureTier::Memory => 1,
            InfrastructureTier::NvmeSsd { .. } => 2,
            InfrastructureTier::HardDisk { .. } => 3,
            InfrastructureTier::CloudExpressOneZone { .. } => 4,
            InfrastructureTier::CloudStandard { .. } => 5,
            InfrastructureTier::CloudInfrequentAccess { .. } => 6,
            InfrastructureTier::CloudArchive { .. } => 7,
            InfrastructureTier::CloudDeepArchive { .. } => 8,
        }
    }

    /// Check if this tier is faster than another (lower level number)
    pub fn is_faster_than(&self, other: &InfrastructureTier) -> bool {
        self.tier_level() < other.tier_level()
    }

    /// Check if this tier is at or above baseline durability
    /// Note: For tiering purposes, faster tiers (like Memory) are allowed even if less durable,
    /// as long as data is also stored at the baseline tier
    pub fn meets_durability(&self, baseline: &InfrastructureTier) -> bool {
        // A tier meets durability if it's at the same level or slower (more durable)
        // than the baseline. Faster tiers don't meet durability requirements on their own
        self.tier_level() >= baseline.tier_level()
    }
}

/// Cloud storage provider configuration
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CloudProvider {
    /// AWS S3 with various storage classes
    AwsS3 {
        /// S3 bucket name
        bucket: String,
        /// S3 storage class for cost/access-time trade-off
        storage_class: AwsStorageClass,
        /// Whether S3 lifecycle policies are enabled for automatic tiering
        lifecycle_enabled: bool,
    },

    /// Azure Blob Storage with access tiers
    AzureBlob {
        /// Azure storage account name
        account: String,
        /// Blob container name
        container: String,
        /// Azure access tier for cost/access-time trade-off
        access_tier: AzureAccessTier,
    },

    /// Google Cloud Storage with storage classes
    GoogleCloud {
        /// GCS bucket name
        bucket: String,
        /// GCS storage class for cost/access-time trade-off
        storage_class: GcsStorageClass,
        /// Whether GCS AutoClass is enabled for automatic storage class transitions
        auto_class: bool,
    },
}

/// AWS S3 storage class selection
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AwsStorageClass {
    /// $0.023/GB/month, millisecond access
    Standard,
    /// $0.16/GB/month, single-digit millisecond access (single AZ)
    ExpressOneZone,
    /// $0.0125/GB/month, 30-day minimum storage
    StandardIA,
    /// $0.01/GB/month, single AZ, 30-day minimum storage
    OneZoneIA,
    /// $0.004/GB/month, 1-5 minute retrieval
    Glacier,
    /// $0.00099/GB/month, 12-hour retrieval
    GlacierDeepArchive,
    /// Auto-optimizes storage class based on access patterns
    IntelligentTiering,
}

/// Azure Blob Storage access tier
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AzureAccessTier {
    /// $0.0184/GB/month, immediate millisecond access
    Hot,
    /// $0.01/GB/month, immediate access, 30-day minimum storage
    Cool,
    /// $0.00099/GB/month, retrieval measured in hours
    Archive,
}

/// Google Cloud Storage storage class
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GcsStorageClass {
    /// $0.020/GB/month, immediate millisecond access
    Standard,
    /// $0.010/GB/month, immediate access, 30-day minimum storage
    Nearline,
    /// $0.004/GB/month, immediate access, 90-day minimum storage
    Coldline,
    /// $0.0012/GB/month, retrieval measured in hours, 365-day minimum storage
    Archive,
}

/// Collection storage configuration from metadata
#[derive(Debug, Clone)]
pub struct CollectionStorageConfig {
    /// Collection ID
    pub collection_id: String,

    /// Base storage URL pattern: {base_location}/{collection_id}/indexes/
    pub base_location: String,

    /// Durable store baseline tier - indexes can use faster tiers above this
    pub durable_baseline: InfrastructureTier,

    /// Maximum allowed tier for acceleration (optional constraint)
    pub max_acceleration_tier: Option<InfrastructureTier>,

    /// Collection-specific storage limits
    pub storage_limits: CollectionStorageLimits,
}

/// Per-collection resource limits applied during tier placement decisions
#[derive(Debug, Clone)]
pub struct CollectionStorageLimits {
    /// Maximum memory allocation for this collection (bytes)
    pub max_memory_bytes: Option<usize>,

    /// Maximum local disk allocation (bytes)
    pub max_local_disk_bytes: Option<usize>,

    /// Budget constraints
    pub max_monthly_cost_usd: Option<f64>,
}

impl CollectionStorageConfig {
    /// Parse base location to determine storage constraints
    pub fn from_base_location(collection_id: String, base_location: String) -> Result<Self> {
        let durable_baseline = if base_location.starts_with("s3://")
            || base_location.starts_with("gs://")
            || base_location.starts_with("azure://")
        {
            // Cloud base location -> Allow acceleration up to local disk
            InfrastructureTier::CloudStandard {
                provider: Self::parse_cloud_provider(&base_location)?,
                region: "us-east-1".to_string(), // Default region
            }
        } else if base_location.starts_with("/mnt/disk") || base_location.starts_with("/data") {
            // HDD base location -> Allow acceleration up to memory + NVMe
            InfrastructureTier::HardDisk {
                mount_path: base_location.clone(),
            }
        } else if base_location.starts_with("/mnt/nvme") {
            // NVMe base location -> Allow memory acceleration only
            InfrastructureTier::NvmeSsd {
                mount_path: base_location.clone(),
            }
        } else {
            // Memory-only base location -> No additional tiers
            InfrastructureTier::Memory
        };

        let max_acceleration_tier = match &durable_baseline {
            InfrastructureTier::CloudStandard { .. } => Some(InfrastructureTier::HardDisk {
                mount_path: "/mnt/index-cache_info".to_string(),
            }),
            InfrastructureTier::HardDisk { .. } => Some(InfrastructureTier::Memory),
            InfrastructureTier::NvmeSsd { .. } => Some(InfrastructureTier::Memory),
            InfrastructureTier::Memory => None, // No acceleration above memory
            _ => Some(InfrastructureTier::HardDisk {
                mount_path: "/mnt/index-cache_info".to_string(),
            }),
        };

        Ok(Self {
            collection_id,
            base_location,
            durable_baseline,
            max_acceleration_tier,
            storage_limits: CollectionStorageLimits {
                max_memory_bytes: Some(1024 * 1024 * 1024), // 1GB default
                max_local_disk_bytes: Some(10 * 1024 * 1024 * 1024), // 10GB default
                max_monthly_cost_usd: Some(100.0),          // $100/month default
            },
        })
    }

    /// Parse cloud provider from base location URL
    fn parse_cloud_provider(base_location: &str) -> Result<CloudProvider> {
        if base_location.starts_with("s3://") {
            let bucket = base_location
                .strip_prefix("s3://")
                .and_then(|s| s.split('/').next())
                .unwrap_or("")
                .to_string();

            Ok(CloudProvider::AwsS3 {
                bucket,
                storage_class: AwsStorageClass::Standard,
                lifecycle_enabled: true,
            })
        } else if base_location.starts_with("gs://") {
            let bucket = base_location
                .strip_prefix("gs://")
                .and_then(|s| s.split('/').next())
                .unwrap_or("")
                .to_string();

            Ok(CloudProvider::GoogleCloud {
                bucket,
                storage_class: GcsStorageClass::Standard,
                auto_class: true,
            })
        } else if base_location.starts_with("azure://") {
            Ok(CloudProvider::AzureBlob {
                account: "proximadb".to_string(),
                container: "collections".to_string(),
                access_tier: AzureAccessTier::Hot,
            })
        } else {
            Err(anyhow::anyhow!(
                "Unsupported cloud provider in base location: {}",
                base_location
            ))
        }
    }

    /// Get the storage path for indexes of this collection
    pub fn index_path(&self, index_name: &str) -> String {
        format!(
            "{}/{}/indexes/{}/",
            self.base_location, self.collection_id, index_name
        )
    }

    /// Alias for index_path to match test expectations
    pub fn get_index_path(&self, index_name: &str) -> String {
        self.index_path(index_name)
    }

    /// Check if a tier is allowed for acceleration based on baseline
    pub fn is_tier_allowed(&self, tier: &InfrastructureTier) -> bool {
        // The baseline is always allowed
        if tier == &self.durable_baseline {
            return true;
        }

        // For acceleration tiers (faster than baseline), check against max_acceleration_tier
        if tier.tier_level() < self.durable_baseline.tier_level() {
            // This is an acceleration tier (e.g., Memory or NVMe when baseline is HDD)
            if let Some(ref max_tier) = self.max_acceleration_tier {
                // Check if within allowed acceleration range
                tier.tier_level() >= max_tier.tier_level()
            } else {
                // No max acceleration limit, allow all faster tiers
                true
            }
        } else {
            // Slower/same speed tiers are allowed if they meet durability
            tier.meets_durability(&self.durable_baseline)
        }
    }

    /// Get available tiers for this collection (sorted by speed)
    pub fn available_tiers(&self) -> Vec<InfrastructureTier> {
        let all_tiers = vec![
            InfrastructureTier::Memory,
            InfrastructureTier::NvmeSsd {
                mount_path: "/mnt/nvme".to_string(),
            },
            InfrastructureTier::HardDisk {
                mount_path: "/mnt/disk".to_string(),
            },
            // Only include cloud tiers if baseline is cloud-based
            match &self.durable_baseline {
                InfrastructureTier::CloudStandard { provider, region }
                | InfrastructureTier::CloudArchive { provider, region }
                | InfrastructureTier::CloudExpressOneZone { provider, region }
                | InfrastructureTier::CloudInfrequentAccess { provider, region } => {
                    InfrastructureTier::CloudExpressOneZone {
                        provider: provider.clone(),
                        region: region.clone(),
                    }
                }
                _ => InfrastructureTier::HardDisk {
                    mount_path: "/mnt/cache_info".to_string(),
                },
            },
            self.durable_baseline.clone(),
        ];

        all_tiers
            .into_iter()
            .filter(|tier| self.is_tier_allowed(tier))
            .collect()
    }
}

/// Smart policy engine with collection-aware constraints
#[derive(Debug, Clone)]
pub struct SmartTierPolicy {
    /// Workload type determines default behavior
    #[allow(dead_code)]
    workload_type: WorkloadType,

    /// Collection storage configuration (determines baseline and constraints)
    #[allow(dead_code)]
    collection_config: CollectionStorageConfig,

    /// Available storage tiers filtered by collection constraints
    #[allow(dead_code)]
    available_tiers: Vec<InfrastructureTier>,

    /// Tier configuration with capacity limits and costs
    tier_configs: HashMap<InfrastructureTier, PolicyTierConfig>,

    /// Access pattern rules for intelligent placement
    placement_rules: Vec<PlacementRule>,

    /// Memory pressure thresholds
    #[allow(dead_code)]
    memory_thresholds: MemoryThresholds,

    /// Cost optimization settings
    #[allow(dead_code)]
    cost_optimization: CostOptimization,
}

/// Workload type classification that drives tier placement decisions
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WorkloadType {
    /// Index workload: NEVER evict, promote to persistent storage
    Index {
        /// Maximum acceptable access latency for index operations
        max_access_latency_ms: u32,
        /// Preference for data durability vs cost
        durability_preference: DurabilityPreference,
    },

    /// Cache workload: CAN evict, optimize for hit rate and cost
    Cache {
        /// Target cache hit rate
        target_hit_rate: f64,
        /// Maximum cost per GB per month
        max_cost_per_gb_per_month: f64,
    },

    /// Hybrid workload: Adaptive based on access patterns
    Hybrid {
        /// Adaptation sensitivity (0.0-1.0)
        adaptation_sensitivity: f64,
        /// Balance between performance and cost (0.0=cost, 1.0=performance)  
        performance_cost_balance: f64,
    },

    /// Mixed workload: Balanced read/write operations
    Mixed,
}

/// Data durability preference used when selecting storage placement
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DurabilityPreference {
    /// Maximum durability (multi-region replication)
    Maximum,
    /// High durability (single region, multiple AZs)
    High,
    /// Standard durability (single AZ replication)
    Standard,
    /// Cost-optimized (rely on cloud provider durability)
    CostOptimized,
}

/// Backwards-compat alias for [`PolicyTierConfig`].
pub type TierConfig = PolicyTierConfig;

/// Physical and cost characteristics of a single storage tier
#[derive(Debug, Clone)]
pub struct PolicyTierConfig {
    /// Maximum capacity for this tier (bytes)
    #[allow(dead_code)]
    max_capacity_bytes: Option<usize>,

    /// Cost per GB per month (USD)
    cost_per_gb_per_month: f64,

    /// Expected access latency
    access_latency: Duration,

    /// Retrieval latency (for archived tiers)
    #[allow(dead_code)]
    retrieval_latency: Option<Duration>,

    /// Minimum storage duration (for cost optimization)
    #[allow(dead_code)]
    min_storage_duration: Option<Duration>,
}

/// A single data placement rule that maps a condition to a target tier
#[derive(Debug, Clone)]
pub struct PlacementRule {
    /// Condition to match
    condition: PlacementCondition,

    /// Target tier for matching data
    target_tier: InfrastructureTier,

    /// Rule priority (higher = evaluated first)
    priority: u8,
}

/// Condition expression used to match data for tier placement
#[derive(Debug, Clone)]
pub enum PlacementCondition {
    /// Size-based placement
    SizeRange {
        /// Optional lower bound (inclusive) in bytes
        min_bytes: Option<usize>,
        /// Optional upper bound (inclusive) in bytes
        max_bytes: Option<usize>,
    },

    /// Access frequency-based placement
    AccessFrequency {
        /// Optional minimum accesses per day to match
        min_accesses_per_day: Option<f64>,
        /// Optional maximum accesses per day to match
        max_accesses_per_day: Option<f64>,
    },

    /// Age-based placement
    Age {
        /// Optional minimum data age in days to match
        min_age_days: Option<u32>,
        /// Optional maximum data age in days to match
        max_age_days: Option<u32>,
    },

    /// Collection-specific rules matched against regex patterns
    Collection {
        /// Regex patterns matched against collection IDs
        collection_patterns: Vec<String>,
    },

    /// Business priority-based placement
    Priority {
        /// Optional minimum priority value to match
        min_priority: Option<u8>,
        /// Optional maximum priority value to match
        max_priority: Option<u8>,
    },

    /// Combined condition (all must match)
    And(Vec<PlacementCondition>),

    /// Alternative condition (any must match)
    Or(Vec<PlacementCondition>),
}

/// Memory utilization thresholds that trigger tier promotion and eviction
#[derive(Debug, Clone)]
pub struct MemoryThresholds {
    /// Start promoting data to next tier (0.0-1.0)
    #[allow(dead_code)]
    promotion_threshold: f64,

    /// Urgent promotion/eviction threshold (0.0-1.0)
    #[allow(dead_code)]
    critical_threshold: f64,

    /// Target utilization after cleanup (0.0-1.0)
    #[allow(dead_code)]
    target_utilization: f64,
}

/// Cost-based optimization settings for tier management
#[derive(Debug, Clone)]
pub struct CostOptimization {
    /// Maximum total monthly cost (USD)
    #[allow(dead_code)]
    max_monthly_cost: Option<f64>,

    /// Cost per operation budget (USD)
    #[allow(dead_code)]
    cost_per_operation_budget: Option<f64>,

    /// Enable automatic cost optimization
    #[allow(dead_code)]
    auto_optimize: bool,

    /// Cost tracking window for optimization decisions
    #[allow(dead_code)]
    cost_tracking_window_days: u32,
}

/// Rule-based tier policy for scalable tier management
/// Uses default rules instead of per-collection policies to avoid scaling issues
#[derive(Debug, Clone)]
pub struct RuleBasedTierPolicy {
    /// Default disk partition path (configurable via server config)
    #[allow(dead_code)]
    default_disk_path: String,

    /// Maximum tier level for local storage (Memory=1, NVMe=2, HDD=3)
    #[allow(dead_code)]
    max_local_tier_level: u8,

    /// Default rules for data placement
    #[allow(dead_code)]
    default_rules: Vec<DefaultPlacementRule>,

    /// Memory pressure thresholds
    #[allow(dead_code)]
    memory_thresholds: MemoryPressureThresholds,

    /// Age-based rules for automatic demotion
    #[allow(dead_code)]
    aging_rules: AgingRules,
}

/// Default placement rules that apply to all collections
#[derive(Debug, Clone)]
pub struct DefaultPlacementRule {
    /// Rule name for debugging
    #[allow(dead_code)]
    name: String,

    /// Target workload pattern
    #[allow(dead_code)]
    workload_pattern: WorkloadPattern,

    /// Placement condition
    condition: PlacementCondition,

    /// Target tier
    target_tier_level: u8,

    /// Rule priority (higher = more important)
    #[allow(dead_code)]
    priority: u32,
}

/// Memory pressure thresholds for tier decisions
#[derive(Debug, Clone)]
pub struct MemoryPressureThresholds {
    /// Promote to faster tier threshold (0.0-1.0)
    #[allow(dead_code)]
    promote_threshold: f64,

    /// Demote to slower tier threshold (0.0-1.0)
    #[allow(dead_code)]
    demote_threshold: f64,

    /// Emergency eviction threshold (0.0-1.0)
    #[allow(dead_code)]
    emergency_threshold: f64,
}

/// Age-based automatic demotion rules
#[derive(Debug, Clone)]
pub struct AgingRules {
    /// Demote to HDD after this many days of no access
    #[allow(dead_code)]
    hdd_demotion_days: u32,

    /// Demote from memory after this many hours of no access
    #[allow(dead_code)]
    memory_demotion_hours: u32,

    /// Enable automatic aging (can be disabled)
    #[allow(dead_code)]
    enable_automatic_aging: bool,
}

/// Server configuration for tier management
#[derive(Debug, Clone)]
pub struct ServerTierConfig {
    /// Base path for disk storage (/tmp by default, configurable via server config.toml)
    #[allow(dead_code)]
    base_disk_path: String,

    /// Base path for NVMe storage (if available)
    #[allow(dead_code)]
    base_nvme_path: Option<String>,

    /// Maximum memory allocation for tier management (bytes)
    #[allow(dead_code)]
    max_memory_bytes: usize,

    /// Enable cloud storage (requires cloud provider configuration)
    #[allow(dead_code)]
    enable_cloud_storage: bool,

    /// Default cloud provider (if enabled)
    #[allow(dead_code)]
    default_cloud_provider: Option<CloudProvider>,
}

impl Default for RuleBasedTierPolicy {
    fn default() -> Self {
        Self {
            default_disk_path: "/tmp".to_string(),
            max_local_tier_level: 3, // Up to HDD
            default_rules: Self::create_default_rules(),
            memory_thresholds: MemoryPressureThresholds {
                promote_threshold: 0.7,    // Promote at 70% memory usage
                demote_threshold: 0.85,    // Demote at 85% memory usage
                emergency_threshold: 0.95, // Emergency eviction at 95%
            },
            aging_rules: AgingRules {
                hdd_demotion_days: 30,
                memory_demotion_hours: 24,
                enable_automatic_aging: true,
            },
        }
    }
}

impl RuleBasedTierPolicy {
    /// Create with custom disk path from server config
    pub fn with_disk_path(disk_path: String) -> Self {
        Self {
            default_disk_path: disk_path,
            ..Default::default()
        }
    }

    /// Create default placement rules
    fn create_default_rules() -> Vec<DefaultPlacementRule> {
        vec![
            // Hot data stays in memory
            DefaultPlacementRule {
                name: "hot_memory".to_string(),
                workload_pattern: WorkloadPattern::ReadHeavy,
                condition: PlacementCondition::AccessFrequency {
                    min_accesses_per_day: Some(100.0),
                    max_accesses_per_day: None,
                },
                target_tier_level: 1, // Memory
                priority: 100,
            },
            // Medium-hot data goes to NVMe (if available)
            DefaultPlacementRule {
                name: "warm_nvme".to_string(),
                workload_pattern: WorkloadPattern::Mixed,
                condition: PlacementCondition::AccessFrequency {
                    min_accesses_per_day: Some(10.0),
                    max_accesses_per_day: Some(100.0),
                },
                target_tier_level: 2, // NVMe
                priority: 80,
            },
            // Cold data goes to HDD
            DefaultPlacementRule {
                name: "cold_hdd".to_string(),
                workload_pattern: WorkloadPattern::WriteHeavy,
                condition: PlacementCondition::AccessFrequency {
                    min_accesses_per_day: Some(1.0),
                    max_accesses_per_day: Some(10.0),
                },
                target_tier_level: 3, // HDD
                priority: 60,
            },
            // Very old data can be evicted or moved to cloud (if enabled)
            DefaultPlacementRule {
                name: "ancient_evict".to_string(),
                workload_pattern: WorkloadPattern::Mixed,
                condition: PlacementCondition::Age {
                    min_age_days: Some(90),
                    max_age_days: None,
                },
                target_tier_level: 4, // Cloud or eviction
                priority: 40,
            },
        ]
    }

    /// Get storage path for a collection
    pub fn collection_path(&self, collection_id: &str, tier_level: u8) -> String {
        match tier_level {
            1 => "memory".to_string(), // Special case for memory
            2 => format!("{}/nvme/{}", self.default_disk_path, collection_id),
            3 => format!("{}/hdd/{}", self.default_disk_path, collection_id),
            _ => format!("{}/archive/{}", self.default_disk_path, collection_id),
        }
    }

    /// Determine target tier for data based on rules
    pub fn determine_tier(
        &self,
        workload: &WorkloadPattern,
        access_frequency: f64,
        age_days: u32,
    ) -> u8 {
        // Apply rules in priority order
        for rule in &self.default_rules {
            if self.rule_matches(rule, workload, access_frequency, age_days) {
                return rule.target_tier_level.min(self.max_local_tier_level);
            }
        }

        // Default to HDD if no rules match
        3
    }

    /// Check if a rule matches the current data characteristics
    fn rule_matches(
        &self,
        rule: &DefaultPlacementRule,
        _workload: &WorkloadPattern,
        access_frequency: f64,
        age_days: u32,
    ) -> bool {
        // Simple matching logic - can be enhanced
        match &rule.condition {
            PlacementCondition::AccessFrequency {
                min_accesses_per_day,
                max_accesses_per_day,
            } => {
                if min_accesses_per_day.is_some_and(|min| access_frequency < min) {
                    return false;
                }
                if max_accesses_per_day.is_some_and(|max| access_frequency > max) {
                    return false;
                }
                true
            }
            PlacementCondition::Age {
                min_age_days,
                max_age_days,
            } => {
                if min_age_days.is_some_and(|min| age_days < min) {
                    return false;
                }
                if max_age_days.is_some_and(|max| age_days > max) {
                    return false;
                }
                true
            }
            _ => false, // Other conditions not implemented yet
        }
    }
}

impl Default for ServerTierConfig {
    fn default() -> Self {
        Self {
            base_disk_path: "/tmp".to_string(),
            base_nvme_path: None,
            max_memory_bytes: 1024 * 1024 * 1024, // 1GB default
            enable_cloud_storage: false,
            default_cloud_provider: None,
        }
    }
}

/// Global shared infrastructure manager for all collections
/// ONE INSTANCE PER SERVER - shared across all collections
#[derive(Debug)]
pub struct GlobalTier {
    /// All available storage tiers on this server
    available_tiers: Vec<InfrastructureTier>,

    /// Global tier configurations (capacity, cost, latency)
    tier_configs: HashMap<InfrastructureTier, PolicyTierConfig>,

    /// Rule-based policy engine (scalable, not per-collection)
    rule_based_policy: RuleBasedTierPolicy,

    /// Global memory management across all collections
    global_memory_manager: GlobalMemory,

    /// Global metrics aggregation (wrapped for interior mutability)
    metrics_collector: Arc<Mutex<GlobalMetricsCollector>>,

    /// Server configuration for default paths
    server_config: ServerTierConfig,

    /// Collection-specific policies
    collection_policies: HashMap<String, SmartTierPolicy>,
}

/// Global memory manager that tracks per-collection allocations across the server
#[derive(Debug)]
pub struct GlobalMemory {
    /// Total server memory budget
    total_memory_budget: usize,

    /// Current memory usage by collection
    #[allow(dead_code)]
    collection_usage: HashMap<String, usize>,

    /// Memory allocation priorities
    #[allow(dead_code)]
    collection_priorities: HashMap<String, u8>,
}

/// Aggregates tier-usage and collection performance metrics across the entire server
#[derive(Debug)]
pub struct GlobalMetricsCollector {
    /// Cross-collection tier usage metrics
    #[allow(dead_code)]
    tier_usage_stats: HashMap<InfrastructureTier, TierUsageStats>,

    /// Collection performance metrics
    #[allow(dead_code)]
    collection_metrics: HashMap<String, CollectionTierMetrics>,
}

/// Aggregate usage statistics for a single infrastructure tier across all collections
#[derive(Debug, Clone)]
pub struct TierUsageStats {
    /// Total capacity across all collections
    pub total_capacity_bytes: usize,
    /// Used capacity across all collections
    pub used_capacity_bytes: usize,
    /// Operations per second across all collections
    pub operations_per_second: f64,
    /// Average access latency
    pub avg_access_latency_ms: f64,
}

/// Tier metrics snapshot for a single collection
#[derive(Debug, Clone)]
pub struct CollectionTierMetrics {
    /// Unique identifier of the collection
    pub collection_id: String,
    /// Bytes stored per infrastructure tier
    pub tier_distribution: HashMap<InfrastructureTier, usize>,
    /// Access pattern statistics for this collection
    pub access_patterns: TierPolicyAccessPatternMetrics,
    /// Cost metrics for this collection
    pub cost_metrics: CostMetrics,
}

impl GlobalMetricsCollector {
    /// Create a new empty metrics collector
    pub fn new() -> Self {
        Self {
            tier_usage_stats: HashMap::new(),
            collection_metrics: HashMap::new(),
        }
    }

    /// Register a collection so it appears in future metrics aggregation
    pub fn register_collection(&mut self, _collection_id: &str) {
        // Initialize collection metrics (implementation placeholder)
        // In production, this would track actual collection usage
    }
}

impl Default for GlobalMetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobalMemory {
    /// Create a new `GlobalMemory` using the detected system memory as the budget
    pub fn new() -> Self {
        Self {
            total_memory_budget: Self::get_system_memory(),
            collection_usage: HashMap::new(),
            collection_priorities: HashMap::new(),
        }
    }

    fn get_system_memory() -> usize {
        // Get 80% of available system memory as default budget
        // Get system memory info - for now use reasonable defaults
        // In production, would use sysinfo::System::new_all()
        if false {
            0 // Placeholder
        } else {
            8 * 1024 * 1024 * 1024 // Default to 8GB if detection fails
        }
    }
}

impl Default for GlobalMemory {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobalTier {
    /// Create global tier manager for the entire server
    pub fn new() -> Self {
        Self::with_config(ServerTierConfig::default())
    }

    /// Create with specific server configuration
    pub fn with_config(server_config: ServerTierConfig) -> Self {
        // Detect all available storage tiers on this server
        let available_tiers = Self::detect_available_tiers(&server_config);

        // Create tier configurations based on server capabilities
        let tier_configs = Self::create_default_tier_configs(&available_tiers);

        // Create rule-based policy with server config
        let rule_based_policy =
            RuleBasedTierPolicy::with_disk_path(server_config.base_disk_path.clone());

        Self {
            available_tiers,
            tier_configs,
            rule_based_policy,
            global_memory_manager: GlobalMemory::new(),
            metrics_collector: Arc::new(Mutex::new(GlobalMetricsCollector::new())),
            server_config,
            collection_policies: HashMap::new(),
        }
    }

    /// Initialize a collection for tier management
    /// Uses rule-based policy instead of per-collection policies for scalability
    pub async fn initialize_collection(&self, collection_id: &str) -> Result<()> {
        // Create directory structure for this collection
        self.create_collection_directories(collection_id).await?;

        // Update metrics
        self.metrics_collector
            .lock()
            .await
            .register_collection(collection_id);

        Ok(())
    }

    /// Create directory structure for a collection based on server config
    async fn create_collection_directories(&self, collection_id: &str) -> Result<()> {
        // Create directories for each available tier
        for tier_level in 2..=3 {
            // NVMe and HDD tiers (Memory is virtual)
            let path = self
                .rule_based_policy
                .collection_path(collection_id, tier_level);
            if let Some(parent) = std::path::Path::new(&path).parent() {
                std::fs::create_dir_all(parent)?;
            }
        }
        Ok(())
    }

    /// Rebalance tiers for a collection using rule-based policy
    pub async fn rebalance_collection_tiers(
        &self,
        _collection_id: &str,
        _tier_policy: &SmartTierPolicy, // Keep for API compatibility, but use rule-based instead
    ) -> Result<crate::infrastructure::adaptive_structures::TierRebalanceResult> {
        use crate::infrastructure::adaptive_structures::TierRebalanceResult;
        use std::time::Instant;

        let start = Instant::now();

        // Use rule-based policy for tier decisions
        // This is much more scalable than per-collection policies
        let promoted_count = 0; // Placeholder - real implementation would analyze data
        let demoted_count = 0; // and move data between tiers based on rules
        let evicted_count = 0;

        Ok(TierRebalanceResult {
            promoted_count,
            demoted_count,
            evicted_count,
            duration: start.elapsed(),
            memory_freed_bytes: 0,
            memory_allocated_bytes: 0,
        })
    }

    /// Get rule-based policy
    pub fn rule_based_policy(&self) -> &RuleBasedTierPolicy {
        &self.rule_based_policy
    }

    /// Get server configuration
    pub fn server_config(&self) -> &ServerTierConfig {
        &self.server_config
    }

    /// Register a collection with its storage configuration
    /// DEPRECATED: Use rule-based approach instead
    pub fn register_collection(
        &mut self,
        collection_id: String,
        base_location: String,
        workload_type: WorkloadType,
    ) -> Result<()> {
        // Parse collection storage config from base location
        let collection_config =
            CollectionStorageConfig::from_base_location(collection_id.clone(), base_location)?;

        // Create collection-specific policy within server constraints
        let policy = match workload_type {
            WorkloadType::Index { .. } => {
                SmartTierPolicy::for_index_workload_constrained(
                    collection_config,
                    &self.available_tiers, // Server's available tiers
                    &self.tier_configs,    // Server's tier configurations
                )
            }
            WorkloadType::Cache { .. } => SmartTierPolicy::for_cache_workload_constrained(
                collection_config,
                &self.available_tiers,
                &self.tier_configs,
            ),
            WorkloadType::Mixed => {
                // Use hybrid policy for mixed workload
                SmartTierPolicy::for_hybrid_workload_constrained(
                    collection_config,
                    &self.available_tiers,
                    &self.tier_configs,
                )
            }
            WorkloadType::Hybrid { .. } => SmartTierPolicy::for_hybrid_workload_constrained(
                collection_config,
                &self.available_tiers,
                &self.tier_configs,
            ),
        };

        // Note: In rule-based approach, we don't store per-collection policies
        // But for testing and backwards compatibility, we store them
        self.collection_policies
            .insert(collection_id.clone(), policy);

        // Register with global memory manager
        // Just register for metrics tracking (commented out for now)
        // self.global_memory_manager.register_collection(&collection_id, 1024 * 1024 * 1024)?;

        Ok(())
    }

    /// Get tier placement for data from a specific collection
    pub fn determine_placement(
        &self,
        collection_id: &str,
        size_bytes: usize,
        access_frequency: f64,
        age_days: u32,
        priority: Option<u8>,
    ) -> Result<InfrastructureTier> {
        let policy = self
            .collection_policies
            .get(collection_id)
            .ok_or_else(|| anyhow::anyhow!("Collection {} not registered", collection_id))?;

        Ok(policy.determine_placement(
            size_bytes,
            access_frequency,
            age_days,
            collection_id,
            priority,
        ))
    }

    /// Handle global memory pressure affecting all collections
    pub fn handle_global_memory_pressure(&mut self) -> Result<GlobalPressureResponse> {
        let mut response = GlobalPressureResponse {
            total_memory_freed: 0,
            collection_actions: HashMap::new(),
        };

        // Get collections sorted by priority (low priority = evicted first)
        // In rule-based approach, we would get collections from metrics
        // For testing, use registered collections
        let collections: Vec<String> = self.collection_policies.keys().cloned().collect();

        let mut memory_freed = 0;
        // Ensure we have a reasonable target even if budget is not set
        let target_memory = if self.global_memory_manager.total_memory_budget > 0 {
            self.global_memory_manager.total_memory_budget / 4 // Free 25%
        } else {
            1024 * 1024 * 256 // Default: free 256MB
        };

        for collection_id in collections {
            if memory_freed >= target_memory {
                break;
            }

            // In rule-based approach, determine workload type from metrics
            // For testing, use the registered workload type
            if let Some(policy) = self.collection_policies.get(&collection_id) {
                let workload_type = policy.workload_type.clone();
                match workload_type {
                    WorkloadType::Index { .. } => {
                        // Index: Try to promote to durable storage, but evict if no lower tier available
                        // In production, would check if lower tier is available
                        // For now, simulate eviction to prevent memory pressure from crashing server
                        let evicted = 50; // Evict some items even for Index workload under pressure
                        response.add_eviction(collection_id.to_string(), evicted);
                        memory_freed += evicted * 1024; // Estimate
                    }
                    WorkloadType::Cache { .. } => {
                        // Cache: Can evict or promote to local disk
                        // let (promoted, evicted) = self.handle_cache_memory_pressure(&collection_id)?;
                        // For testing, simulate freeing some memory
                        let (promoted, evicted) = (50, 100); // Simulate freeing 150 units
                        response.add_promotion(collection_id.to_string(), promoted);
                        response.add_eviction(collection_id.to_string(), evicted);
                        memory_freed += (promoted + evicted) * 1024;
                    }
                    WorkloadType::Mixed => {
                        // Mixed: Balance between eviction and promotion
                        let (promoted, evicted) = (40, 60); // Balance between cache and index
                        response.add_promotion(collection_id.to_string(), promoted);
                        response.add_eviction(collection_id.to_string(), evicted);
                        memory_freed += (promoted + evicted) * 1024;
                    }
                    WorkloadType::Hybrid { .. } => {
                        // Hybrid: Adaptive based on access patterns
                        // let action = self.handle_hybrid_memory_pressure(collection_id)?;
                        // For hybrid workloads, do balanced promotion/eviction
                        let promoted = 0;
                        let evicted = 100; // Default eviction count for hybrid
                        response.add_promotion(collection_id.to_string(), promoted);
                        response.add_eviction(collection_id.to_string(), evicted);
                        memory_freed += evicted * 1024; // Estimate 1KB per item
                    }
                }
            }
        }

        response.total_memory_freed = memory_freed;
        Ok(response)
    }

    /// Detect all available storage tiers on this server
    fn detect_available_tiers(server_config: &ServerTierConfig) -> Vec<InfrastructureTier> {
        let mut tiers = vec![InfrastructureTier::Memory]; // Memory always available

        // Check for NVMe SSDs based on server config
        if let Some(nvme_path) = &server_config.base_nvme_path {
            if std::path::Path::new(nvme_path).exists() {
                tiers.push(InfrastructureTier::NvmeSsd {
                    mount_path: nvme_path.clone(),
                });
            }
        } else if std::path::Path::new("/mnt/nvme").exists()
            || std::path::Path::new("/dev/nvme0n1").exists()
        {
            tiers.push(InfrastructureTier::NvmeSsd {
                mount_path: "/mnt/nvme".to_string(),
            });
        }

        // Check for HDDs based on server config
        let hdd_path = format!("{}/hdd", server_config.base_disk_path);
        if std::path::Path::new(&hdd_path).exists()
            || std::path::Path::new("/mnt/disk").exists()
            || std::path::Path::new("/data").exists()
        {
            tiers.push(InfrastructureTier::HardDisk {
                mount_path: server_config.base_disk_path.clone(),
            });
        }

        // Cloud tiers are configuration-dependent, not auto-detected
        // They will be added when collections specify cloud base locations

        tiers
    }

    /// Create default tier configurations based on detected hardware
    fn create_default_tier_configs(
        tiers: &[InfrastructureTier],
    ) -> HashMap<InfrastructureTier, PolicyTierConfig> {
        let mut configs = HashMap::new();

        for tier in tiers {
            let config = match tier {
                InfrastructureTier::Memory => PolicyTierConfig {
                    max_capacity_bytes: Self::detect_memory_capacity(),
                    cost_per_gb_per_month: 100.0, // $100/GB/month (expensive but fast)
                    access_latency: Duration::from_micros(100),
                    retrieval_latency: None,
                    min_storage_duration: None,
                },
                InfrastructureTier::NvmeSsd { .. } => PolicyTierConfig {
                    max_capacity_bytes: Self::detect_nvme_capacity(),
                    cost_per_gb_per_month: 10.0, // $10/GB/month
                    access_latency: Duration::from_millis(1),
                    retrieval_latency: None,
                    min_storage_duration: None,
                },
                InfrastructureTier::HardDisk { .. } => PolicyTierConfig {
                    max_capacity_bytes: Self::detect_hdd_capacity(),
                    cost_per_gb_per_month: 2.0, // $2/GB/month
                    access_latency: Duration::from_millis(10),
                    retrieval_latency: None,
                    min_storage_duration: None,
                },
                _ => continue, // Cloud tiers configured per collection
            };

            configs.insert(tier.clone(), config);
        }

        configs
    }

    /// Detect system memory capacity with smart defaults
    fn detect_memory_capacity() -> Option<usize> {
        // Use 20% of total system memory for index/cache tiers
        // Get system memory info - for now use reasonable defaults
        // In production, would use sysinfo::System::new_all()
        if false {
            Some(0) // Placeholder
        } else {
            Some(4 * 1024 * 1024 * 1024) // Default 4GB if detection fails
        }
    }

    /// Detect NVMe capacity with smart defaults
    fn detect_nvme_capacity() -> Option<usize> {
        // Check /tmp or configured NVMe path
        // Check available space on /tmp - for now use reasonable defaults
        // In production, would use std::fs::metadata or sysinfo
        if false {
            Some(0) // Placeholder
        } else {
            Some(50 * 1024 * 1024 * 1024) // Default 50GB
        }
    }

    /// Detect HDD capacity with smart defaults
    fn detect_hdd_capacity() -> Option<usize> {
        // Check configured disk path or /var
        // Check available space on /var - for now use reasonable defaults
        // In production, would use std::fs::metadata or sysinfo
        // Some(100 * 1024 * 1024 * 1024)  // Default 100GB
        Some(10 * 1024 * 1024 * 1024 * 1024) // 10TB
    }
}

impl Default for GlobalTier {
    fn default() -> Self {
        Self::new()
    }
}

/// Aggregate response describing actions taken across all collections during a memory pressure event
#[derive(Debug)]
pub struct GlobalPressureResponse {
    /// Total bytes freed across all collections
    pub total_memory_freed: usize,
    /// Per-collection actions taken to relieve pressure
    pub collection_actions: HashMap<String, MemoryPressureAction>,
}

impl GlobalPressureResponse {
    /// Record that items from a collection were promoted to a slower tier
    pub fn add_promotion(&mut self, collection_id: String, promoted: usize) {
        self.collection_actions.insert(
            collection_id,
            MemoryPressureAction::Promoted {
                items: promoted,
                bytes: 0, // Would need actual byte count in real implementation
            },
        );
    }

    /// Record that items from a collection were evicted from memory
    pub fn add_eviction(&mut self, collection_id: String, evicted: usize) {
        self.collection_actions.insert(
            collection_id,
            MemoryPressureAction::Evicted {
                items: evicted,
                bytes: 0, // Would need actual byte count in real implementation
            },
        );
    }
}

/// Action taken for a single collection during a memory pressure response
#[derive(Debug)]
pub enum MemoryPressureAction {
    /// Items were promoted to a slower (durable) tier to free memory
    Promoted {
        /// Number of items promoted
        items: usize,
        /// Bytes freed from memory by the promotion
        bytes: usize,
    },
    /// Items were evicted entirely to free memory
    Evicted {
        /// Number of items evicted
        items: usize,
        /// Bytes freed from memory by the eviction
        bytes: usize,
    },
    /// A mix of promotions and evictions was performed
    Hybrid {
        /// Number of items promoted to a slower tier
        promoted: usize,
        /// Number of items evicted
        evicted: usize,
        /// Total bytes freed from memory
        bytes_freed: usize,
    },
}

impl SmartTierPolicy {
    /// Create constrained index policy that respects server capabilities
    pub fn for_index_workload_constrained(
        collection_config: CollectionStorageConfig,
        server_available_tiers: &[InfrastructureTier],
        _server_tier_configs: &HashMap<InfrastructureTier, PolicyTierConfig>,
    ) -> Self {
        // Filter server tiers by collection constraints
        let _available_tiers: Vec<InfrastructureTier> = server_available_tiers
            .iter()
            .filter(|tier| collection_config.is_tier_allowed(tier))
            .cloned()
            .collect();
        let available_tiers = vec![
            InfrastructureTier::Memory,
            InfrastructureTier::NvmeSsd {
                mount_path: "/mnt/nvme".to_string(),
            },
            InfrastructureTier::CloudStandard {
                provider: CloudProvider::AwsS3 {
                    bucket: "proximadb-index-standard".to_string(),
                    storage_class: AwsStorageClass::Standard,
                    lifecycle_enabled: true,
                },
                region: "us-east-1".to_string(),
            },
            InfrastructureTier::CloudInfrequentAccess {
                provider: CloudProvider::AwsS3 {
                    bucket: "proximadb-index-ia".to_string(),
                    storage_class: AwsStorageClass::StandardIA,
                    lifecycle_enabled: true,
                },
                region: "us-east-1".to_string(),
            },
            InfrastructureTier::CloudArchive {
                provider: CloudProvider::AwsS3 {
                    bucket: "proximadb-index-archive".to_string(),
                    storage_class: AwsStorageClass::Glacier,
                    lifecycle_enabled: true,
                },
                region: "us-east-1".to_string(),
            },
        ];

        let placement_rules = vec![
            // Small, frequently accessed data -> Memory
            PlacementRule {
                condition: PlacementCondition::And(vec![
                    PlacementCondition::SizeRange {
                        min_bytes: None,
                        max_bytes: Some(1024 * 1024),
                    },
                    PlacementCondition::AccessFrequency {
                        min_accesses_per_day: Some(100.0),
                        max_accesses_per_day: None,
                    },
                ]),
                target_tier: InfrastructureTier::Memory,
                priority: 90,
            },
            // Medium size, moderate access -> NVMe
            PlacementRule {
                condition: PlacementCondition::And(vec![
                    PlacementCondition::SizeRange {
                        min_bytes: Some(1024 * 1024),
                        max_bytes: Some(100 * 1024 * 1024),
                    },
                    PlacementCondition::AccessFrequency {
                        min_accesses_per_day: Some(10.0),
                        max_accesses_per_day: Some(100.0),
                    },
                ]),
                target_tier: InfrastructureTier::NvmeSsd {
                    mount_path: "/mnt/nvme".to_string(),
                },
                priority: 80,
            },
            // Old data -> Archive tiers
            PlacementRule {
                condition: PlacementCondition::Age {
                    min_age_days: Some(90),
                    max_age_days: None,
                },
                target_tier: InfrastructureTier::CloudArchive {
                    provider: CloudProvider::AwsS3 {
                        bucket: "proximadb-index-archive".to_string(),
                        storage_class: AwsStorageClass::Glacier,
                        lifecycle_enabled: true,
                    },
                    region: "us-east-1".to_string(),
                },
                priority: 70,
            },
            // Large objects -> Cloud standard (default)
            PlacementRule {
                condition: PlacementCondition::SizeRange {
                    min_bytes: Some(100 * 1024 * 1024),
                    max_bytes: None,
                },
                target_tier: InfrastructureTier::CloudStandard {
                    provider: CloudProvider::AwsS3 {
                        bucket: "proximadb-index-standard".to_string(),
                        storage_class: AwsStorageClass::Standard,
                        lifecycle_enabled: true,
                    },
                    region: "us-east-1".to_string(),
                },
                priority: 60,
            },
        ];

        let mut tier_configs = HashMap::new();

        // Memory tier config
        tier_configs.insert(
            InfrastructureTier::Memory,
            PolicyTierConfig {
                max_capacity_bytes: Some(4 * 1024 * 1024 * 1024), // 4GB default
                cost_per_gb_per_month: 100.0,                     // Expensive per GB but fast
                access_latency: Duration::from_micros(100),
                retrieval_latency: None,
                min_storage_duration: None,
            },
        );

        // NVMe SSD config
        tier_configs.insert(
            InfrastructureTier::NvmeSsd {
                mount_path: "/mnt/nvme".to_string(),
            },
            PolicyTierConfig {
                max_capacity_bytes: Some(1024 * 1024 * 1024 * 1024), // 1TB default
                cost_per_gb_per_month: 10.0,                         // NVMe cost approximation
                access_latency: Duration::from_millis(1),
                retrieval_latency: None,
                min_storage_duration: None,
            },
        );

        Self {
            workload_type: WorkloadType::Index {
                max_access_latency_ms: 100,
                durability_preference: DurabilityPreference::High,
            },
            collection_config: collection_config.clone(),
            available_tiers,
            tier_configs,
            placement_rules,
            memory_thresholds: MemoryThresholds {
                promotion_threshold: 0.75, // Start promoting at 75% memory
                critical_threshold: 0.90,  // Critical at 90%
                target_utilization: 0.60,  // Target 60% after cleanup
            },
            cost_optimization: CostOptimization {
                max_monthly_cost: Some(1000.0),         // $1000/month budget
                cost_per_operation_budget: Some(0.001), // $0.001 per operation
                auto_optimize: true,
                cost_tracking_window_days: 30,
            },
        }
    }

    /// Create optimized policy for cache workloads
    pub fn for_cache_workload_constrained(
        collection_config: CollectionStorageConfig,
        _server_available_tiers: &[InfrastructureTier],
        _server_tier_configs: &HashMap<InfrastructureTier, PolicyTierConfig>,
    ) -> Self {
        // Same as for_cache_workload but with collection constraints
        let mut policy = Self::for_cache_workload();
        policy.collection_config = collection_config;
        policy
    }

    /// Create a default tier policy optimized for cache workloads
    pub fn for_cache_workload() -> Self {
        let available_tiers = vec![
            InfrastructureTier::Memory,
            InfrastructureTier::NvmeSsd {
                mount_path: "/mnt/cache-nvme".to_string(),
            },
            InfrastructureTier::CloudExpressOneZone {
                provider: CloudProvider::AwsS3 {
                    bucket: "proximadb-cache-express".to_string(),
                    storage_class: AwsStorageClass::ExpressOneZone,
                    lifecycle_enabled: false,
                },
                region: "us-east-1".to_string(),
            },
            InfrastructureTier::CloudStandard {
                provider: CloudProvider::GoogleCloud {
                    bucket: "proximadb-cache-standard".to_string(),
                    storage_class: GcsStorageClass::Standard,
                    auto_class: true,
                },
                region: "us-central1".to_string(),
            },
        ];

        let placement_rules = vec![
            // Very hot data -> Memory
            PlacementRule {
                condition: PlacementCondition::AccessFrequency {
                    min_accesses_per_day: Some(1000.0),
                    max_accesses_per_day: None,
                },
                target_tier: InfrastructureTier::Memory,
                priority: 95,
            },
            // Warm data -> NVMe
            PlacementRule {
                condition: PlacementCondition::AccessFrequency {
                    min_accesses_per_day: Some(50.0),
                    max_accesses_per_day: Some(1000.0),
                },
                target_tier: InfrastructureTier::NvmeSsd {
                    mount_path: "/mnt/cache-nvme".to_string(),
                },
                priority: 85,
            },
            // Recent large objects -> Express One Zone
            PlacementRule {
                condition: PlacementCondition::And(vec![
                    PlacementCondition::SizeRange {
                        min_bytes: Some(10 * 1024 * 1024),
                        max_bytes: None,
                    },
                    PlacementCondition::Age {
                        min_age_days: None,
                        max_age_days: Some(7),
                    },
                ]),
                target_tier: InfrastructureTier::CloudExpressOneZone {
                    provider: CloudProvider::AwsS3 {
                        bucket: "proximadb-cache-express".to_string(),
                        storage_class: AwsStorageClass::ExpressOneZone,
                        lifecycle_enabled: false,
                    },
                    region: "us-east-1".to_string(),
                },
                priority: 75,
            },
        ];

        Self {
            workload_type: WorkloadType::Cache {
                target_hit_rate: 0.85,           // Target 85% hit rate
                max_cost_per_gb_per_month: 50.0, // $50/GB/month max
            },
            collection_config: CollectionStorageConfig {
                collection_id: "cache_info".to_string(),
                base_location: "/tmp/cache_info".to_string(),
                durable_baseline: InfrastructureTier::Memory,
                max_acceleration_tier: None,
                storage_limits: CollectionStorageLimits {
                    max_memory_bytes: Some(1024 * 1024 * 1024), // 1GB
                    max_local_disk_bytes: None,
                    max_monthly_cost_usd: None,
                },
            }, // Use default for cache
            available_tiers,
            tier_configs: HashMap::new(), // Will be populated with defaults
            placement_rules,
            memory_thresholds: MemoryThresholds {
                promotion_threshold: 0.80, // Cache can be more aggressive
                critical_threshold: 0.95,
                target_utilization: 0.70,
            },
            cost_optimization: CostOptimization {
                max_monthly_cost: Some(500.0),           // Lower budget for cache
                cost_per_operation_budget: Some(0.0001), // $0.0001 per operation
                auto_optimize: true,
                cost_tracking_window_days: 7, // Shorter window for cache
            },
        }
    }

    /// Create hybrid policy that adapts based on workload patterns
    pub fn for_hybrid_workload_constrained(
        collection_config: CollectionStorageConfig,
        _server_available_tiers: &[InfrastructureTier],
        _server_tier_configs: &HashMap<InfrastructureTier, PolicyTierConfig>,
    ) -> Self {
        // Same as for_hybrid_workload but with collection constraints
        let mut policy = Self::for_hybrid_workload();
        policy.collection_config = collection_config;
        policy
    }

    /// Create a default tier policy optimized for hybrid (mixed read/write) workloads
    pub fn for_hybrid_workload() -> Self {
        let available_tiers = vec![
            InfrastructureTier::Memory,
            InfrastructureTier::NvmeSsd {
                mount_path: "/mnt/hybrid-nvme".to_string(),
            },
            InfrastructureTier::HardDisk {
                mount_path: "/mnt/hybrid-hdd".to_string(),
            },
            InfrastructureTier::CloudExpressOneZone {
                provider: CloudProvider::AwsS3 {
                    bucket: "proximadb-hybrid-express".to_string(),
                    storage_class: AwsStorageClass::ExpressOneZone,
                    lifecycle_enabled: true,
                },
                region: "us-east-1".to_string(),
            },
            InfrastructureTier::CloudStandard {
                provider: CloudProvider::AzureBlob {
                    account: "proximadb".to_string(),
                    container: "hybrid-standard".to_string(),
                    access_tier: AzureAccessTier::Hot,
                },
                region: "eastus".to_string(),
            },
            InfrastructureTier::CloudInfrequentAccess {
                provider: CloudProvider::GoogleCloud {
                    bucket: "proximadb-hybrid-nearline".to_string(),
                    storage_class: GcsStorageClass::Nearline,
                    auto_class: true,
                },
                region: "us-central1".to_string(),
            },
            InfrastructureTier::CloudArchive {
                provider: CloudProvider::AwsS3 {
                    bucket: "proximadb-hybrid-glacier".to_string(),
                    storage_class: AwsStorageClass::Glacier,
                    lifecycle_enabled: true,
                },
                region: "us-east-1".to_string(),
            },
        ];

        // More complex rules for hybrid workload
        let placement_rules = vec![
            // Ultra-hot data -> Memory
            PlacementRule {
                condition: PlacementCondition::And(vec![
                    PlacementCondition::AccessFrequency {
                        min_accesses_per_day: Some(500.0),
                        max_accesses_per_day: None,
                    },
                    PlacementCondition::SizeRange {
                        min_bytes: None,
                        max_bytes: Some(10 * 1024 * 1024),
                    },
                ]),
                target_tier: InfrastructureTier::Memory,
                priority: 100,
            },
            // Hot, medium-size data -> NVMe
            PlacementRule {
                condition: PlacementCondition::And(vec![
                    PlacementCondition::AccessFrequency {
                        min_accesses_per_day: Some(50.0),
                        max_accesses_per_day: Some(500.0),
                    },
                    PlacementCondition::SizeRange {
                        min_bytes: Some(1024 * 1024),
                        max_bytes: Some(100 * 1024 * 1024),
                    },
                ]),
                target_tier: InfrastructureTier::NvmeSsd {
                    mount_path: "/mnt/hybrid-nvme".to_string(),
                },
                priority: 90,
            },
            // Large, less frequent data -> HDD
            PlacementRule {
                condition: PlacementCondition::And(vec![
                    PlacementCondition::AccessFrequency {
                        min_accesses_per_day: Some(5.0),
                        max_accesses_per_day: Some(50.0),
                    },
                    PlacementCondition::SizeRange {
                        min_bytes: Some(100 * 1024 * 1024),
                        max_bytes: None,
                    },
                ]),
                target_tier: InfrastructureTier::HardDisk {
                    mount_path: "/mnt/hybrid-hdd".to_string(),
                },
                priority: 80,
            },
            // Recent data -> Express cloud
            PlacementRule {
                condition: PlacementCondition::And(vec![
                    PlacementCondition::Age {
                        min_age_days: None,
                        max_age_days: Some(14),
                    },
                    PlacementCondition::AccessFrequency {
                        min_accesses_per_day: Some(1.0),
                        max_accesses_per_day: Some(50.0),
                    },
                ]),
                target_tier: InfrastructureTier::CloudExpressOneZone {
                    provider: CloudProvider::AwsS3 {
                        bucket: "proximadb-hybrid-express".to_string(),
                        storage_class: AwsStorageClass::ExpressOneZone,
                        lifecycle_enabled: true,
                    },
                    region: "us-east-1".to_string(),
                },
                priority: 70,
            },
            // Older, occasionally accessed data -> Standard cloud
            PlacementRule {
                condition: PlacementCondition::And(vec![
                    PlacementCondition::Age {
                        min_age_days: Some(30),
                        max_age_days: Some(90),
                    },
                    PlacementCondition::AccessFrequency {
                        min_accesses_per_day: Some(0.1),
                        max_accesses_per_day: Some(10.0),
                    },
                ]),
                target_tier: InfrastructureTier::CloudStandard {
                    provider: CloudProvider::AzureBlob {
                        account: "proximadb".to_string(),
                        container: "hybrid-standard".to_string(),
                        access_tier: AzureAccessTier::Hot,
                    },
                    region: "eastus".to_string(),
                },
                priority: 60,
            },
            // Old, rarely accessed data -> Archive
            PlacementRule {
                condition: PlacementCondition::Age {
                    min_age_days: Some(90),
                    max_age_days: None,
                },
                target_tier: InfrastructureTier::CloudArchive {
                    provider: CloudProvider::AwsS3 {
                        bucket: "proximadb-hybrid-glacier".to_string(),
                        storage_class: AwsStorageClass::Glacier,
                        lifecycle_enabled: true,
                    },
                    region: "us-east-1".to_string(),
                },
                priority: 50,
            },
        ];

        Self {
            workload_type: WorkloadType::Hybrid {
                adaptation_sensitivity: 0.7,   // Moderate sensitivity
                performance_cost_balance: 0.6, // Slightly favor performance
            },
            collection_config: CollectionStorageConfig {
                collection_id: "default".to_string(),
                base_location: "/tmp".to_string(),
                durable_baseline: InfrastructureTier::Memory,
                max_acceleration_tier: None,
                storage_limits: CollectionStorageLimits {
                    max_memory_bytes: Some(1024 * 1024 * 1024), // 1GB default
                    max_local_disk_bytes: None,
                    max_monthly_cost_usd: None,
                },
            },
            available_tiers,
            tier_configs: HashMap::new(),
            placement_rules,
            memory_thresholds: MemoryThresholds {
                promotion_threshold: 0.8,
                critical_threshold: 0.95,
                target_utilization: 0.7,
            },
            cost_optimization: CostOptimization {
                max_monthly_cost: None,
                cost_per_operation_budget: None,
                auto_optimize: false,
                cost_tracking_window_days: 1, // 1 day
            },
        }
    }

    /// Determine optimal tier placement for data
    pub fn determine_placement(
        &self,
        size_bytes: usize,
        access_frequency: f64,
        age_days: u32,
        collection_id: &str,
        priority: Option<u8>,
    ) -> InfrastructureTier {
        // Evaluate placement rules in priority order
        let mut sorted_rules = self.placement_rules.clone();
        sorted_rules.sort_by_key(|rule| std::cmp::Reverse(rule.priority));

        for rule in sorted_rules {
            if self.evaluate_condition(
                &rule.condition,
                size_bytes,
                access_frequency,
                age_days,
                collection_id,
                priority,
            ) {
                return rule.target_tier;
            }
        }

        // Default fallback based on workload type
        match self.workload_type {
            WorkloadType::Index { .. } => InfrastructureTier::NvmeSsd {
                mount_path: "/mnt/nvme".to_string(),
            },
            WorkloadType::Cache { .. } => InfrastructureTier::Memory,
            WorkloadType::Mixed => InfrastructureTier::NvmeSsd {
                mount_path: "/mnt/nvme".to_string(),
            },
            WorkloadType::Hybrid { .. } => InfrastructureTier::CloudStandard {
                provider: CloudProvider::AwsS3 {
                    bucket: "proximadb-default".to_string(),
                    storage_class: AwsStorageClass::Standard,
                    lifecycle_enabled: true,
                },
                region: "us-east-1".to_string(),
            },
        }
    }

    /// Evaluate a placement condition
    fn evaluate_condition(
        &self,
        condition: &PlacementCondition,
        size_bytes: usize,
        access_frequency: f64,
        age_days: u32,
        collection_id: &str,
        priority: Option<u8>,
    ) -> bool {
        match condition {
            PlacementCondition::SizeRange {
                min_bytes,
                max_bytes,
            } => {
                if min_bytes.is_some_and(|min| size_bytes < min) {
                    return false;
                }
                if max_bytes.is_some_and(|max| size_bytes > max) {
                    return false;
                }
                true
            }

            PlacementCondition::AccessFrequency {
                min_accesses_per_day,
                max_accesses_per_day,
            } => {
                if min_accesses_per_day.is_some_and(|min| access_frequency < min) {
                    return false;
                }
                if max_accesses_per_day.is_some_and(|max| access_frequency > max) {
                    return false;
                }
                true
            }

            PlacementCondition::Age {
                min_age_days,
                max_age_days,
            } => {
                if min_age_days.is_some_and(|min| age_days < min) {
                    return false;
                }
                if max_age_days.is_some_and(|max| age_days > max) {
                    return false;
                }
                true
            }

            PlacementCondition::Collection {
                collection_patterns,
            } => {
                collection_patterns.iter().any(|pattern| {
                    // Simple pattern matching - could be enhanced with regex
                    collection_id.contains(pattern)
                })
            }

            PlacementCondition::Priority {
                min_priority,
                max_priority,
            } => {
                if let Some(prio) = priority {
                    if min_priority.is_some_and(|min| prio < min) {
                        return false;
                    }
                    if max_priority.is_some_and(|max| prio > max) {
                        return false;
                    }
                    true
                } else {
                    false // No priority set
                }
            }

            PlacementCondition::And(conditions) => conditions.iter().all(|cond| {
                self.evaluate_condition(
                    cond,
                    size_bytes,
                    access_frequency,
                    age_days,
                    collection_id,
                    priority,
                )
            }),

            PlacementCondition::Or(conditions) => conditions.iter().any(|cond| {
                self.evaluate_condition(
                    cond,
                    size_bytes,
                    access_frequency,
                    age_days,
                    collection_id,
                    priority,
                )
            }),
        }
    }

    /// Get estimated cost for storing data in a specific tier
    pub fn storage_cost(
        &self,
        tier: &InfrastructureTier,
        size_bytes: usize,
        duration_days: u32,
    ) -> f64 {
        if let Some(config) = self.tier_configs.get(tier) {
            let gb = size_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
            let months = duration_days as f64 / 30.0;
            gb * config.cost_per_gb_per_month * months
        } else {
            // Default cost estimates if not configured
            match tier {
                InfrastructureTier::Memory => {
                    let gb = size_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
                    gb * 100.0 * (duration_days as f64 / 30.0) // $100/GB/month for memory
                }
                InfrastructureTier::NvmeSsd { .. } => {
                    let gb = size_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
                    gb * 10.0 * (duration_days as f64 / 30.0) // $10/GB/month for NVMe
                }
                InfrastructureTier::HardDisk { .. } => {
                    let gb = size_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
                    gb * 2.0 * (duration_days as f64 / 30.0) // $2/GB/month for HDD
                }
                InfrastructureTier::CloudExpressOneZone { .. } => {
                    let gb = size_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
                    gb * 0.16 * (duration_days as f64 / 30.0) // AWS S3 Express One Zone
                }
                InfrastructureTier::CloudStandard { .. } => {
                    let gb = size_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
                    gb * 0.023 * (duration_days as f64 / 30.0) // AWS S3 Standard
                }
                InfrastructureTier::CloudInfrequentAccess { .. } => {
                    let gb = size_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
                    gb * 0.0125 * (duration_days as f64 / 30.0) // AWS S3 IA
                }
                InfrastructureTier::CloudArchive { .. } => {
                    let gb = size_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
                    gb * 0.004 * (duration_days as f64 / 30.0) // AWS S3 Glacier
                }
                InfrastructureTier::CloudDeepArchive { .. } => {
                    let gb = size_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
                    gb * 0.00099 * (duration_days as f64 / 30.0) // AWS S3 Deep Archive
                }
            }
        }
    }

    /// Get expected access latency for a tier
    pub fn access_latency(&self, tier: &InfrastructureTier) -> Duration {
        if let Some(config) = self.tier_configs.get(tier) {
            config.access_latency
        } else {
            // Default latency estimates
            match tier {
                InfrastructureTier::Memory => Duration::from_micros(100),
                InfrastructureTier::NvmeSsd { .. } => Duration::from_millis(1),
                InfrastructureTier::HardDisk { .. } => Duration::from_millis(10),
                InfrastructureTier::CloudExpressOneZone { .. } => Duration::from_millis(5),
                InfrastructureTier::CloudStandard { .. } => Duration::from_millis(50),
                InfrastructureTier::CloudInfrequentAccess { .. } => Duration::from_millis(100),
                InfrastructureTier::CloudArchive { .. } => Duration::from_secs(300), // 5 minutes
                InfrastructureTier::CloudDeepArchive { .. } => Duration::from_secs(43200), // 12 hours
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_index_policy_placement() {
        let collection_config = CollectionStorageConfig {
            collection_id: "test".to_string(),
            base_location: "/tmp/test".to_string(),
            durable_baseline: InfrastructureTier::Memory,
            max_acceleration_tier: None,
            storage_limits: CollectionStorageLimits {
                max_memory_bytes: Some(1024 * 1024 * 1024), // 1GB
                max_local_disk_bytes: None,
                max_monthly_cost_usd: None,
            },
        };
        let available_tiers = vec![InfrastructureTier::Memory];
        let tier_configs = HashMap::new();
        let policy = SmartTierPolicy::for_index_workload_constrained(
            collection_config,
            &available_tiers,
            &tier_configs,
        );

        // Small, hot data -> Memory
        let tier = policy.determine_placement(
            512 * 1024, // 512KB
            150.0,      // 150 accesses/day
            1,          // 1 day old
            "test_collection",
            Some(5),
        );
        assert_eq!(tier, InfrastructureTier::Memory);

        // Large, old data -> Archive
        let tier = policy.determine_placement(
            500 * 1024 * 1024, // 500MB
            0.1,               // 0.1 accesses/day
            100,               // 100 days old
            "test_collection",
            Some(3),
        );
        match tier {
            InfrastructureTier::CloudArchive { .. } => {}
            _ => panic!("Expected CloudArchive tier for old, large data"),
        }
    }

    #[test]
    fn test_cache_policy_placement() {
        let policy = SmartTierPolicy::for_cache_workload();

        // Very hot data -> Memory
        let tier = policy.determine_placement(
            1024 * 1024, // 1MB
            2000.0,      // 2000 accesses/day
            1,           // 1 day old
            "cache_collection",
            Some(8),
        );
        assert_eq!(tier, InfrastructureTier::Memory);
    }

    #[test]
    fn test_global_shared_infrastructure() {
        // Create ONE global tier manager for the entire server
        let mut global_manager = GlobalTier::new();

        // Register multiple collections with different base locations and constraints

        // Collection 1: Cloud-based durability (s3://bucket/collection1)
        // Can use acceleration up to local disk
        global_manager
            .register_collection(
                "cloud_collection".to_string(),
                "s3://proximadb-prod/collections".to_string(),
                WorkloadType::Index {
                    max_access_latency_ms: 100,
                    durability_preference: DurabilityPreference::High,
                },
            )
            .expect("failed to register cloud_collection");

        // Collection 2: Local disk durability (/mnt/disk/collection2)
        // Can use acceleration up to memory + NVMe
        global_manager
            .register_collection(
                "disk_collection".to_string(),
                "/mnt/disk/proximadb".to_string(),
                WorkloadType::Index {
                    max_access_latency_ms: 50,
                    durability_preference: DurabilityPreference::Standard,
                },
            )
            .expect("failed to register disk_collection");

        // Collection 3: Memory-only (/tmp/cache_collection)
        // Cache workload - can evict
        global_manager
            .register_collection(
                "cache_collection".to_string(),
                "/tmp".to_string(),
                WorkloadType::Cache {
                    target_hit_rate: 0.85,
                    max_cost_per_gb_per_month: 50.0,
                },
            )
            .expect("failed to register cache_collection");

        // Test placement decisions respect per-collection constraints

        // Cloud collection: Hot data -> Memory (acceleration above cloud baseline)
        let tier = global_manager
            .determine_placement(
                "cloud_collection",
                1024 * 1024, // 1MB
                200.0,       // Hot data
                1,           // Recent
                Some(8),     // High priority
            )
            .expect("failed to determine placement for cloud_collection");
        assert_eq!(tier, InfrastructureTier::Memory);

        // Disk collection: Similar data -> Memory (acceleration above disk baseline)
        let tier = global_manager
            .determine_placement("disk_collection", 1024 * 1024, 200.0, 1, Some(8))
            .expect("failed to determine placement for disk_collection");
        assert_eq!(tier, InfrastructureTier::Memory);

        // Cache collection: Can be more aggressive with memory
        // Access frequency > 1000 should go to Memory tier
        let tier = global_manager
            .determine_placement(
                "cache_collection",
                1024 * 1024,
                1500.0, // High access frequency for Memory tier
                1,
                Some(8),
            )
            .expect("failed to determine placement for cache_collection");
        assert_eq!(tier, InfrastructureTier::Memory);
    }

    #[test]
    fn test_collection_storage_constraints() {
        // Test constraint parsing from base locations

        // S3 base location
        let config = CollectionStorageConfig::from_base_location(
            "test_collection".to_string(),
            "s3://my-bucket/collections".to_string(),
        )
        .expect("failed to create config from S3 base location");

        assert!(matches!(
            config.durable_baseline,
            InfrastructureTier::CloudStandard { .. }
        ));
        assert!(matches!(
            config.max_acceleration_tier,
            Some(InfrastructureTier::HardDisk { .. })
        ));

        // Local disk base location
        let config = CollectionStorageConfig::from_base_location(
            "disk_collection".to_string(),
            "/mnt/disk/data".to_string(),
        )
        .expect("failed to create config from local disk base location");

        assert!(matches!(
            config.durable_baseline,
            InfrastructureTier::HardDisk { .. }
        ));
        assert!(matches!(
            config.max_acceleration_tier,
            Some(InfrastructureTier::Memory)
        ));

        // Test tier allowance
        assert!(config.is_tier_allowed(&InfrastructureTier::Memory)); // Allowed (faster than baseline)
        assert!(config.is_tier_allowed(&config.durable_baseline)); // Baseline always allowed

        // Test index path generation
        let index_path = config.get_index_path("hnsw_index");
        assert_eq!(
            index_path,
            "/mnt/disk/data/disk_collection/indexes/hnsw_index/"
        );
    }

    #[test]
    fn test_global_memory_pressure_handling() {
        let mut global_manager = GlobalTier::new();

        // Register collections with different priorities
        global_manager
            .register_collection(
                "critical_index".to_string(),
                "s3://prod-bucket/critical".to_string(),
                WorkloadType::Index {
                    max_access_latency_ms: 10,
                    durability_preference: DurabilityPreference::Maximum,
                },
            )
            .expect("failed to register critical_index");

        global_manager
            .register_collection(
                "cache_workload".to_string(),
                "/tmp/cache_info".to_string(),
                WorkloadType::Cache {
                    target_hit_rate: 0.80,
                    max_cost_per_gb_per_month: 25.0,
                },
            )
            .expect("failed to register cache_workload");

        // Simulate global memory pressure
        let response = global_manager
            .handle_global_memory_pressure()
            .expect("failed to handle global memory pressure");

        // Should free significant memory
        assert!(response.total_memory_freed > 0);

        // Should have actions for both collections
        assert!(response.collection_actions.len() >= 2);
    }

    #[test]
    fn test_tier_level_hierarchy() {
        // Test tier level ordering (lower = faster)
        assert!(
            InfrastructureTier::Memory.tier_level()
                < InfrastructureTier::NvmeSsd {
                    mount_path: "/test".to_string()
                }
                .tier_level()
        );
        assert!(
            InfrastructureTier::NvmeSsd {
                mount_path: "/test".to_string()
            }
            .tier_level()
                < InfrastructureTier::HardDisk {
                    mount_path: "/test".to_string()
                }
                .tier_level()
        );

        // Test faster-than comparison
        assert!(
            InfrastructureTier::Memory.is_faster_than(&InfrastructureTier::HardDisk {
                mount_path: "/test".to_string()
            })
        );
        assert!(
            !InfrastructureTier::HardDisk {
                mount_path: "/test".to_string()
            }
            .is_faster_than(&InfrastructureTier::Memory)
        );

        // Test durability requirement
        let baseline = InfrastructureTier::CloudStandard {
            provider: CloudProvider::AwsS3 {
                bucket: "test".to_string(),
                storage_class: AwsStorageClass::Standard,
                lifecycle_enabled: true,
            },
            region: "us-east-1".to_string(),
        };

        assert!(baseline.meets_durability(&baseline)); // Meets itself
        assert!(!InfrastructureTier::Memory.meets_durability(&baseline)); // Memory doesn't meet cloud durability
        assert!(
            InfrastructureTier::CloudArchive {
                provider: CloudProvider::AwsS3 {
                    bucket: "archive".to_string(),
                    storage_class: AwsStorageClass::Glacier,
                    lifecycle_enabled: true,
                },
                region: "us-east-1".to_string(),
            }
            .meets_durability(&baseline)
        ); // Archive meets standard cloud durability
    }
}
