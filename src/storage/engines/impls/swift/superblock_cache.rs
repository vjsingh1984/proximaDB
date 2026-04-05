// SuperBlock Cache for SWIFT Engine - Tree-Based Navigation Optimized
// Focused on SWIFT's actual design: hierarchical tree navigation with instant traversal

use anyhow::Result;
use dashmap::DashMap;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::info;

// Zero-copy filesystem handled by storage layer
// Block structures handled internally
// Bloom filter handled internally

/// SWIFT-specific superblock cache optimized for tree navigation and instant traversal
pub struct SwiftSuperBlockCache {
    /// SuperBlock metadata cache (always in memory for fast tree navigation)
    superblock_cache: Arc<DashMap<String, Arc<CachedSuperBlockMetadata>>>,

    /// Tree navigation hints cache
    tree_navigation_cache: Arc<DashMap<String, Arc<TreeNavigationHints>>>,

    /// DataBlock metadata cache with LRU eviction
    #[allow(dead_code)]
    datablock_cache:
        Arc<RwLock<crate::utils::cache::LruCache<String, Arc<CachedDataBlockMetadata>>>>,
    #[allow(dead_code)]
    datablock_ttl_sec: u64,

    /// Bloom filter cache for instant filtering
    bloom_filter_cache: Arc<DashMap<String, Arc<BloomFilterMetadata>>>,

    /// Progressive search cache
    #[allow(dead_code)]
    progressive_search_cache:
        Arc<RwLock<HashMap<String, (Arc<ProgressiveSearchMetadata>, Instant)>>>,
    #[allow(dead_code)]
    progressive_ttl_sec: u64,

    /// Tree path optimization cache
    tree_path_cache: Arc<DashMap<String, Arc<OptimalTreePath>>>,

    /// Filesystem for loading/storing cache data
    filesystem: Arc<dyn crate::storage::persistence::filesystem::FileSystem>,

    /// Cache statistics
    cache_stats: Arc<SwiftCacheStatistics>,
}

/// SWIFT SuperBlock metadata focused on tree navigation and instant traversal
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CachedSuperBlockMetadata {
    /// SuperBlock identification
    pub superblock_id: u32,
    pub collection_id: String,

    /// Tree navigation data
    pub tree_depth: u16,
    pub tree_node_count: u32,
    pub leaf_node_count: u32,
    pub tree_balance_factor: f32,

    /// Hierarchical structure (SWIFT design: SuperBlock → DataBlock → Records)
    pub datablock_count: u32,
    pub total_records: u64,
    pub records_per_datablock: u32,

    /// Proxima encoding information
    pub superblock_encoding_marker: u8,
    pub encoding_efficiency: f32,
    pub compression_ratio: f32,

    /// Progressive quantization levels available
    pub available_quantization_levels: Vec<QuantizationLevelMetadata>,

    /// Access patterns for tree optimization
    pub access_frequency: u64,
    #[serde(skip)]
    pub last_access: Option<Instant>,
    pub hot_datablocks: Vec<u32>,

    /// Bloom filter statistics
    pub bloom_filter_size: u32,
    pub bloom_filter_false_positive_rate: f32,
    pub bloom_filter_selectivity: f32,

    /// Instant traversal metrics
    pub avg_lookup_time_us: u64,
    pub tree_cache_hit_rate: f32,
    pub navigation_efficiency: f32,
}

/// Tree navigation hints for instant traversal optimization  
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TreeNavigationHints {
    /// Optimal tree traversal paths
    pub frequent_paths: Vec<TreePath>,

    /// Prefetch recommendations
    pub prefetch_nodes: Vec<String>,

    /// Branch prediction hints
    pub branch_probabilities: HashMap<String, f32>,

    /// Cache locality hints
    pub locality_groups: Vec<LocalityGroup>,

    /// Performance optimization suggestions
    pub optimization_hints: Vec<TreeOptimizationHint>,
}

/// Tree path for optimized navigation
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TreePath {
    pub path_id: String,
    pub nodes: Vec<String>,
    pub estimated_cost: f32,
    pub success_rate: f32,
    pub usage_frequency: u64,
}

/// Locality group for cache optimization
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LocalityGroup {
    pub group_id: String,
    pub related_nodes: Vec<String>,
    pub access_correlation: f32,
    pub cache_priority: u8,
}

/// Tree optimization hint
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum TreeOptimizationHint {
    PreloadSubtree { root_node: String, depth: u8 },
    CacheNodeGroup { nodes: Vec<String> },
    RebalanceRecommendation { node: String, reason: String },
    PrefetchSequence { sequence: Vec<String> },
}

/// Optimal tree path for specific access patterns
#[derive(Debug, Clone)]
pub struct OptimalTreePath {
    pub query_pattern: String,
    pub optimal_nodes: Vec<String>,
    pub estimated_latency_us: u64,
    pub cache_requirements: u64,
    pub success_rate: f32,
}

/// Cached DataBlock metadata for SWIFT's hierarchical structure
#[derive(Debug, Clone)]
pub struct CachedDataBlockMetadata {
    /// DataBlock identification
    pub datablock_id: u32,
    pub superblock_id: u32,

    /// Record organization
    pub record_count: u32,
    pub has_deletes: bool,
    pub has_updates: bool,

    /// Tree navigation data
    pub tree_leaf_position: Option<u32>,
    pub navigation_keys: Vec<String>,
    pub key_range: (String, String),

    /// Progressive quantization data
    pub quantization_summary: QuantizationSummary,

    /// Access optimization
    pub access_stats: DataBlockAccessStats,
    pub cache_priority: u8,

    /// Feature-rich metadata (SWIFT design focus)
    pub bloom_filter_present: bool,
    pub inverted_index_present: bool,
    pub adaptive_index_present: bool,
    pub sketch_filter_present: bool,
}

/// Quantization level metadata for progressive search
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct QuantizationLevelMetadata {
    pub level_name: String,
    pub bits_per_dimension: u8,
    pub accuracy_estimate: f32,
    pub speed_multiplier: f32,
    pub memory_usage_mb: u32,
    pub availability: bool,
}

/// Quantization summary for a DataBlock
#[derive(Debug, Clone)]
pub struct QuantizationSummary {
    pub binary_available: bool,
    pub int8_available: bool,
    pub pq_available: bool,
    pub full_precision_available: bool,
    pub recommended_level: String,
    pub quality_score: f32,
}

/// DataBlock access statistics
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DataBlockAccessStats {
    pub access_count: u64,
    #[serde(skip)]
    pub last_access: Option<Instant>,
    pub avg_response_time_us: u64,
    pub cache_hit_rate: f32,
    pub tree_navigation_efficiency: f32,
}

/// Bloom filter metadata for instant filtering
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BloomFilterMetadata {
    pub filter_id: String,
    pub superblock_id: u32,
    pub filter_type: BloomFilter,
    pub size_bytes: u32,
    pub expected_elements: u64,
    pub false_positive_rate: f32,
    pub key_count: u64,
    pub selectivity_estimates: HashMap<String, f32>,
}

/// Types of bloom filters in SWIFT
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum BloomFilter {
    KeyFilter,       // For ID lookups
    MetadataFilter,  // For metadata filtering
    CompositeFilter, // For complex queries
    SketchFilter,    // For approximate queries
}

/// Progressive search metadata for optimization
#[derive(Debug, Clone)]
pub struct ProgressiveSearchMetadata {
    pub search_pattern: String,
    pub optimal_stages: Vec<ProgressiveStage>,
    pub stage_selectivity: Vec<f32>,
    pub total_estimated_cost: f32,
    pub accuracy_at_stage: Vec<f32>,
    pub early_termination_thresholds: Vec<u32>,
}

/// Progressive search stage
#[derive(Debug, Clone)]
pub struct ProgressiveStage {
    pub stage_name: String,
    pub quantization_level: String,
    pub expected_candidates: u32,
    pub stage_cost: f32,
    pub cumulative_accuracy: f32,
}

struct SwiftCacheStatistics {
    superblock_hits: AtomicU64,
    tree_navigation_hits: AtomicU64,
    datablock_hits: AtomicU64,
    bloom_filter_hits: AtomicU64,
    progressive_search_hits: AtomicU64,
    cache_misses: AtomicU64,
    tree_optimization_saves: AtomicU64,
    instant_traversal_saves: AtomicU64,
}

impl SwiftSuperBlockCache {
    pub fn new(
        filesystem: Arc<dyn crate::storage::persistence::filesystem::FileSystem>,
        datablock_cache_size: usize,
        datablock_ttl_sec: u64,
        progressive_ttl_sec: u64,
    ) -> Self {
        Self {
            superblock_cache: Arc::new(DashMap::new()),
            tree_navigation_cache: Arc::new(DashMap::new()),
            datablock_cache: Arc::new(RwLock::new(crate::utils::cache::LruCache::new(
                if datablock_cache_size == 0 {
                    100
                } else {
                    datablock_cache_size
                },
            ))),
            datablock_ttl_sec,
            bloom_filter_cache: Arc::new(DashMap::new()),
            progressive_search_cache: Arc::new(RwLock::new(HashMap::new())),
            progressive_ttl_sec,
            tree_path_cache: Arc::new(DashMap::new()),
            filesystem,
            cache_stats: Arc::new(SwiftCacheStatistics {
                superblock_hits: AtomicU64::new(0),
                tree_navigation_hits: AtomicU64::new(0),
                datablock_hits: AtomicU64::new(0),
                bloom_filter_hits: AtomicU64::new(0),
                progressive_search_hits: AtomicU64::new(0),
                cache_misses: AtomicU64::new(0),
                tree_optimization_saves: AtomicU64::new(0),
                instant_traversal_saves: AtomicU64::new(0),
            }),
        }
    }

    /// Get SuperBlock metadata for tree navigation (SWIFT's core feature)
    pub async fn superblock_metadata(
        &self,
        superblock_id: &str,
    ) -> Result<Arc<CachedSuperBlockMetadata>> {
        // Check cache first
        if let Some(metadata) = self.superblock_cache.get(superblock_id) {
            self.cache_stats
                .superblock_hits
                .fetch_add(1, Ordering::Relaxed);
            return Ok(metadata.clone());
        }

        // Load from disk
        let metadata = self.load_superblock_metadata(superblock_id).await?;
        let metadata_arc = Arc::new(metadata);

        // Cache it permanently (SuperBlocks are always in memory for instant access)
        self.superblock_cache
            .insert(superblock_id.to_string(), metadata_arc.clone());

        Ok(metadata_arc)
    }

    /// Get tree navigation hints for instant traversal optimization
    pub async fn tree_navigation_hints(
        &self,
        superblock_id: &str,
    ) -> Result<Arc<TreeNavigationHints>> {
        // Check cache first
        if let Some(hints) = self.tree_navigation_cache.get(superblock_id) {
            self.cache_stats
                .tree_navigation_hits
                .fetch_add(1, Ordering::Relaxed);
            return Ok(hints.clone());
        }

        // Load from disk or generate
        let hints = self
            .load_or_generate_navigation_hints(superblock_id)
            .await?;
        let hints_arc = Arc::new(hints);

        // Cache it (navigation hints are critical for performance)
        self.tree_navigation_cache
            .insert(superblock_id.to_string(), hints_arc.clone());

        Ok(hints_arc)
    }

    /// Optimize tree navigation path for instant traversal
    pub async fn optimize_tree_navigation(
        &self,
        query_pattern: &str,
        superblock_ids: &[String],
    ) -> Result<Arc<OptimalTreePath>> {
        let cache_key = format!("{}:{}", query_pattern, superblock_ids.join(","));

        // Check if we already have an optimal path
        if let Some(optimal_path) = self.tree_path_cache.get(&cache_key) {
            self.cache_stats
                .tree_optimization_saves
                .fetch_add(1, Ordering::Relaxed);
            return Ok(optimal_path.clone());
        }

        // Generate optimal path based on SuperBlock tree structures
        let optimal_path = self
            .generate_optimal_tree_path(query_pattern, superblock_ids)
            .await?;
        let optimal_path_arc = Arc::new(optimal_path);

        // Cache the optimization
        self.tree_path_cache
            .insert(cache_key, optimal_path_arc.clone());

        self.cache_stats
            .instant_traversal_saves
            .fetch_add(1, Ordering::Relaxed);
        info!("Generated optimal tree path for pattern: {}", query_pattern);

        Ok(optimal_path_arc)
    }

    /// Preload SuperBlock cache for instant access (SWIFT design requirement)
    pub async fn preload_superblocks_for_instant_access(&self, collection_id: &str) -> Result<()> {
        info!(
            "Preloading SuperBlocks for instant access in collection {}",
            collection_id
        );

        // Load all SuperBlock metadata (critical for instant traversal)
        // Filesystem operations handled by caller
        let _superblock_path = format!("{}/superblocks_metadata.bin", collection_id);
        // Deferred: Load superblock metadata from filesystem

        // Load tree navigation hints
        let _navigation_path = format!("{}/tree_navigation_hints.bin", collection_id);
        // Deferred: Load navigation hints from filesystem

        // Load bloom filter metadata (for instant filtering)
        let bloom_path = format!("{}/bloom_filters_metadata.bin", collection_id);
        if self.filesystem.exists(&bloom_path).await.unwrap_or(false) {
            let data = self.filesystem.read(&bloom_path).await?;
            let filters: HashMap<String, BloomFilterMetadata> = bincode::deserialize(&data)?;

            for (key, filter) in filters {
                self.bloom_filter_cache.insert(key, Arc::new(filter));
            }
        }

        Ok(())
    }

    /// Get cache performance statistics
    pub fn cache_statistics(&self) -> SwiftCacheStats {
        SwiftCacheStats {
            superblock_hits: self.cache_stats.superblock_hits.load(Ordering::Relaxed),
            tree_navigation_hits: self
                .cache_stats
                .tree_navigation_hits
                .load(Ordering::Relaxed),
            datablock_hits: self.cache_stats.datablock_hits.load(Ordering::Relaxed),
            bloom_filter_hits: self.cache_stats.bloom_filter_hits.load(Ordering::Relaxed),
            progressive_search_hits: self
                .cache_stats
                .progressive_search_hits
                .load(Ordering::Relaxed),
            cache_misses: self.cache_stats.cache_misses.load(Ordering::Relaxed),
            tree_optimization_saves: self
                .cache_stats
                .tree_optimization_saves
                .load(Ordering::Relaxed),
            instant_traversal_saves: self
                .cache_stats
                .instant_traversal_saves
                .load(Ordering::Relaxed),
        }
    }

    // Helper methods for loading from disk
    async fn load_superblock_metadata(
        &self,
        superblock_id: &str,
    ) -> Result<CachedSuperBlockMetadata> {
        let path = format!("cache/superblock/{}.bin", superblock_id);
        let data = self.filesystem.read(&path).await?;
        Ok(bincode::deserialize(&data)?)
    }

    async fn load_or_generate_navigation_hints(
        &self,
        superblock_id: &str,
    ) -> Result<TreeNavigationHints> {
        let path = format!("cache/navigation/{}.bin", superblock_id);
        if self.filesystem.exists(&path).await.unwrap_or(false) {
            let data = self.filesystem.read(&path).await?;
            Ok(bincode::deserialize(&data)?)
        } else {
            // Generate default navigation hints
            Ok(TreeNavigationHints {
                frequent_paths: Vec::new(),
                prefetch_nodes: Vec::new(),
                branch_probabilities: HashMap::new(),
                locality_groups: Vec::new(),
                optimization_hints: Vec::new(),
            })
        }
    }

    async fn generate_optimal_tree_path(
        &self,
        query_pattern: &str,
        superblock_ids: &[String],
    ) -> Result<OptimalTreePath> {
        // This would analyze the SuperBlock tree structures and generate optimal paths
        // For now, return a basic path
        Ok(OptimalTreePath {
            query_pattern: query_pattern.to_string(),
            optimal_nodes: superblock_ids.to_vec(),
            estimated_latency_us: 100,       // 0.1ms estimate
            cache_requirements: 1024 * 1024, // 1MB
            success_rate: 0.95,
        })
    }
}

/// Cache statistics for monitoring SWIFT's performance
#[derive(Debug, Clone)]
pub struct SwiftCacheStats {
    pub superblock_hits: u64,
    pub tree_navigation_hits: u64,
    pub datablock_hits: u64,
    pub bloom_filter_hits: u64,
    pub progressive_search_hits: u64,
    pub cache_misses: u64,
    pub tree_optimization_saves: u64,
    pub instant_traversal_saves: u64,
}

impl SwiftCacheStats {
    pub fn total_hits(&self) -> u64 {
        self.superblock_hits
            + self.tree_navigation_hits
            + self.datablock_hits
            + self.bloom_filter_hits
            + self.progressive_search_hits
    }

    pub fn cache_hit_rate(&self) -> f32 {
        let total_requests = self.total_hits() + self.cache_misses;
        if total_requests > 0 {
            self.total_hits() as f32 / total_requests as f32
        } else {
            0.0
        }
    }

    pub fn tree_optimization_effectiveness(&self) -> f32 {
        let total_navigation = self.tree_navigation_hits + self.tree_optimization_saves;
        if total_navigation > 0 {
            self.tree_optimization_saves as f32 / total_navigation as f32
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // SwiftCacheStats tests
    // ========================================================================

    #[test]
    fn test_cache_stats_total_hits_all_zero() {
        let stats = SwiftCacheStats {
            superblock_hits: 0,
            tree_navigation_hits: 0,
            datablock_hits: 0,
            bloom_filter_hits: 0,
            progressive_search_hits: 0,
            cache_misses: 0,
            tree_optimization_saves: 0,
            instant_traversal_saves: 0,
        };
        assert_eq!(stats.total_hits(), 0);
    }

    #[test]
    fn test_cache_stats_total_hits_sums_all_hit_types() {
        let stats = SwiftCacheStats {
            superblock_hits: 10,
            tree_navigation_hits: 20,
            datablock_hits: 30,
            bloom_filter_hits: 40,
            progressive_search_hits: 50,
            cache_misses: 100,
            tree_optimization_saves: 5,
            instant_traversal_saves: 3,
        };
        // total_hits = 10 + 20 + 30 + 40 + 50 = 150
        assert_eq!(stats.total_hits(), 150);
    }

    #[test]
    fn test_cache_hit_rate_zero_requests() {
        let stats = SwiftCacheStats {
            superblock_hits: 0,
            tree_navigation_hits: 0,
            datablock_hits: 0,
            bloom_filter_hits: 0,
            progressive_search_hits: 0,
            cache_misses: 0,
            tree_optimization_saves: 0,
            instant_traversal_saves: 0,
        };
        assert_eq!(stats.cache_hit_rate(), 0.0);
    }

    #[test]
    fn test_cache_hit_rate_all_hits() {
        let stats = SwiftCacheStats {
            superblock_hits: 100,
            tree_navigation_hits: 0,
            datablock_hits: 0,
            bloom_filter_hits: 0,
            progressive_search_hits: 0,
            cache_misses: 0,
            tree_optimization_saves: 0,
            instant_traversal_saves: 0,
        };
        assert_eq!(stats.cache_hit_rate(), 1.0);
    }

    #[test]
    fn test_cache_hit_rate_mixed() {
        let stats = SwiftCacheStats {
            superblock_hits: 30,
            tree_navigation_hits: 20,
            datablock_hits: 0,
            bloom_filter_hits: 0,
            progressive_search_hits: 0,
            cache_misses: 50,
            tree_optimization_saves: 0,
            instant_traversal_saves: 0,
        };
        // total_hits = 50, total_requests = 100
        assert!((stats.cache_hit_rate() - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_tree_optimization_effectiveness_zero() {
        let stats = SwiftCacheStats {
            superblock_hits: 0,
            tree_navigation_hits: 0,
            datablock_hits: 0,
            bloom_filter_hits: 0,
            progressive_search_hits: 0,
            cache_misses: 0,
            tree_optimization_saves: 0,
            instant_traversal_saves: 0,
        };
        assert_eq!(stats.tree_optimization_effectiveness(), 0.0);
    }

    #[test]
    fn test_tree_optimization_effectiveness_all_saves() {
        let stats = SwiftCacheStats {
            superblock_hits: 0,
            tree_navigation_hits: 0,
            datablock_hits: 0,
            bloom_filter_hits: 0,
            progressive_search_hits: 0,
            cache_misses: 0,
            tree_optimization_saves: 100,
            instant_traversal_saves: 0,
        };
        assert_eq!(stats.tree_optimization_effectiveness(), 1.0);
    }

    #[test]
    fn test_tree_optimization_effectiveness_mixed() {
        let stats = SwiftCacheStats {
            superblock_hits: 0,
            tree_navigation_hits: 60,
            datablock_hits: 0,
            bloom_filter_hits: 0,
            progressive_search_hits: 0,
            cache_misses: 0,
            tree_optimization_saves: 40,
            instant_traversal_saves: 0,
        };
        // effectiveness = 40 / (60 + 40) = 0.4
        assert!((stats.tree_optimization_effectiveness() - 0.4).abs() < f32::EPSILON);
    }

    #[test]
    fn test_cache_stats_does_not_include_optimization_in_hits() {
        let stats = SwiftCacheStats {
            superblock_hits: 1,
            tree_navigation_hits: 1,
            datablock_hits: 1,
            bloom_filter_hits: 1,
            progressive_search_hits: 1,
            cache_misses: 0,
            tree_optimization_saves: 999,
            instant_traversal_saves: 999,
        };
        // tree_optimization_saves and instant_traversal_saves are NOT part of total_hits
        assert_eq!(stats.total_hits(), 5);
    }

    // ========================================================================
    // Data structure construction tests
    // ========================================================================

    #[test]
    fn test_tree_path_construction() {
        let path = TreePath {
            path_id: "path_1".to_string(),
            nodes: vec!["root".to_string(), "child_a".to_string(), "leaf_1".to_string()],
            estimated_cost: 0.5,
            success_rate: 0.95,
            usage_frequency: 42,
        };
        assert_eq!(path.nodes.len(), 3);
        assert!(path.success_rate > 0.0 && path.success_rate <= 1.0);
    }

    #[test]
    fn test_locality_group_construction() {
        let group = LocalityGroup {
            group_id: "group_1".to_string(),
            related_nodes: vec!["node_a".to_string(), "node_b".to_string()],
            access_correlation: 0.85,
            cache_priority: 1,
        };
        assert_eq!(group.related_nodes.len(), 2);
        assert_eq!(group.cache_priority, 1);
    }

    #[test]
    fn test_tree_optimization_hint_variants() {
        let preload = TreeOptimizationHint::PreloadSubtree {
            root_node: "root".to_string(),
            depth: 3,
        };
        let cache_group = TreeOptimizationHint::CacheNodeGroup {
            nodes: vec!["n1".to_string(), "n2".to_string()],
        };
        let rebalance = TreeOptimizationHint::RebalanceRecommendation {
            node: "unbalanced_node".to_string(),
            reason: "skewed subtree".to_string(),
        };
        let prefetch = TreeOptimizationHint::PrefetchSequence {
            sequence: vec!["s1".to_string(), "s2".to_string(), "s3".to_string()],
        };

        // Verify enum variants are constructible (compile-time check + runtime shape)
        assert!(matches!(preload, TreeOptimizationHint::PreloadSubtree { depth: 3, .. }));
        assert!(matches!(cache_group, TreeOptimizationHint::CacheNodeGroup { .. }));
        assert!(matches!(rebalance, TreeOptimizationHint::RebalanceRecommendation { .. }));
        assert!(matches!(prefetch, TreeOptimizationHint::PrefetchSequence { .. }));
    }

    #[test]
    fn test_optimal_tree_path_construction() {
        let path = OptimalTreePath {
            query_pattern: "range_scan".to_string(),
            optimal_nodes: vec!["sb_0".to_string(), "sb_1".to_string()],
            estimated_latency_us: 100,
            cache_requirements: 1024 * 1024,
            success_rate: 0.95,
        };
        assert_eq!(path.optimal_nodes.len(), 2);
        assert_eq!(path.estimated_latency_us, 100);
    }

    #[test]
    fn test_bloom_filter_variants() {
        let key = BloomFilter::KeyFilter;
        let metadata = BloomFilter::MetadataFilter;
        let composite = BloomFilter::CompositeFilter;
        let sketch = BloomFilter::SketchFilter;

        assert!(matches!(key, BloomFilter::KeyFilter));
        assert!(matches!(metadata, BloomFilter::MetadataFilter));
        assert!(matches!(composite, BloomFilter::CompositeFilter));
        assert!(matches!(sketch, BloomFilter::SketchFilter));
    }

    #[test]
    fn test_quantization_level_metadata() {
        let level = QuantizationLevelMetadata {
            level_name: "binary".to_string(),
            bits_per_dimension: 1,
            accuracy_estimate: 0.85,
            speed_multiplier: 32.0,
            memory_usage_mb: 4,
            availability: true,
        };
        assert_eq!(level.bits_per_dimension, 1);
        assert!(level.availability);
    }
}
