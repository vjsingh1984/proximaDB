// Hierarchical Statistics Cache for NOVA Engine
// Optimized for 3-tier statistics (SuperBlock -> Block -> RowGroup)

use anyhow::Result;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::{debug, info};

use crate::storage::persistence::filesystem::FileSystem;

/// NOVA's 3-tier hierarchical statistics structure
#[derive(Debug, Clone)]
pub struct HierarchicalStats {
    /// SuperBlock level statistics (highest level)
    pub superblock_stats: SuperBlockStats,

    /// Block level statistics (middle level)
    pub block_stats: Vec<BlockStats>,

    /// RowGroup level statistics (lowest level)
    pub rowgroup_stats: Vec<RowGroupStats>,
}

#[derive(Debug, Clone)]
pub struct SuperBlockStats {
    pub id: u64,
    pub num_blocks: usize,
    pub total_vectors: u64,
    pub dimension: usize,

    /// Global statistics for pruning
    pub global_min: Vec<f32>,
    pub global_max: Vec<f32>,
    pub global_mean: Vec<f32>,
    pub global_std_dev: Vec<f32>,

    /// Quantization selectivity for progressive search
    pub quantized_selectivity: f32,

    /// Compression statistics
    pub compression_ratio: f32,
    pub compressed_size_bytes: u64,
    pub uncompressed_size_bytes: u64,

    /// Access patterns for caching decisions
    pub access_count: u64,
    pub last_access: Instant,
    pub creation_time: Instant,
}

#[derive(Debug, Clone)]
pub struct BlockStats {
    pub block_id: u64,
    pub superblock_id: u64,
    pub num_rowgroups: usize,
    pub num_vectors: u64,

    /// Block-level zone maps
    pub zone_map: ZoneMap,

    /// Cost estimation for query planning
    pub search_cost_estimate: f32,
    pub io_cost_estimate: f32,

    /// Bloom filter for existence checks
    pub bloom_filter: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct RowGroupStats {
    pub rowgroup_id: u64,
    pub block_id: u64,
    pub num_rows: u64,
    pub file_offset: u64,
    pub compressed_size: u64,

    /// Fine-grained statistics for pruning
    pub column_stats: HashMap<String, ColumnStats>,

    /// Inverted index hints
    pub indexed_columns: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ZoneMap {
    /// Per-dimension min/max values for pruning
    pub dimension_ranges: Vec<DimensionRange>,

    /// Timestamp range for time-based queries
    pub timestamp_min: Option<i64>,
    pub timestamp_max: Option<i64>,

    /// Metadata value ranges
    pub metadata_ranges: HashMap<String, ValueRange>,
}

#[derive(Debug, Clone)]
pub struct DimensionRange {
    pub dim_index: usize,
    pub min_value: f32,
    pub max_value: f32,
}

#[derive(Debug, Clone)]
pub struct ValueRange {
    pub min: serde_json::Value,
    pub max: serde_json::Value,
    pub null_count: u64,
    pub distinct_count: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct ColumnStats {
    pub min_value: serde_json::Value,
    pub max_value: serde_json::Value,
    pub null_count: u64,
    pub distinct_count: Option<u64>,
}

/// Hierarchical cache optimized for NOVA's statistics structure
pub struct NovaHierarchicalCache {
    /// SuperBlock stats - always in memory (small, frequently accessed)
    superblock_cache: Arc<DashMap<String, Arc<SuperBlockStats>>>,

    /// Block stats - LRU cache with higher capacity
    block_cache: Arc<RwLock<crate::utils::cache::LruCache<String, Arc<BlockStats>>>>,

    /// RowGroup stats - on-demand loading with TTL
    rowgroup_cache: Arc<RwLock<HashMap<String, (Arc<RowGroupStats>, Instant)>>>,
    rowgroup_ttl_sec: u64,

    /// Zone maps - always cached for fast pruning
    zonemap_cache: Arc<DashMap<String, Arc<ZoneMap>>>,

    /// Sidecar global statistics
    global_stats: Arc<RwLock<GlobalStatistics>>,

    /// Filesystem for loading stats from disk
    filesystem: Arc<dyn FileSystem>,

    /// Cache statistics
    cache_stats: Arc<CacheStatistics>,
}

/// Global statistics maintained as sidecar for cross-collection optimization
#[derive(Debug, Clone)]
pub struct GlobalStatistics {
    /// Collection-level statistics
    pub collection_stats: HashMap<String, CollectionStatistics>,

    /// Cross-collection correlations
    pub correlations: HashMap<(String, String), f32>,

    /// Global query patterns
    pub query_patterns: QueryPatterns,
}

#[derive(Debug, Clone)]
pub struct CollectionStatistics {
    pub total_vectors: u64,
    pub avg_vector_size: usize,
    pub total_size_bytes: u64,
    pub compression_ratio: f32,
    pub last_compaction: Instant,
    pub hot_zones: Vec<ZoneMap>,
}

#[derive(Debug, Clone)]
pub struct QueryPatterns {
    pub frequent_filters: HashMap<String, u64>,
    pub common_projections: Vec<Vec<String>>,
    pub avg_k_value: f32,
    pub peak_qps_times: Vec<Instant>,
}

struct CacheStatistics {
    superblock_hits: AtomicU64,
    block_hits: AtomicU64,
    rowgroup_hits: AtomicU64,
    cache_misses: AtomicU64,
    bytes_saved: AtomicU64,
}

impl NovaHierarchicalCache {
    pub fn new(
        filesystem: Arc<dyn FileSystem>,
        block_cache_size: usize,
        rowgroup_ttl_sec: u64,
    ) -> Self {
        Self {
            superblock_cache: Arc::new(DashMap::new()),
            block_cache: Arc::new(RwLock::new(crate::utils::cache::LruCache::new(
                if block_cache_size == 0 {
                    100
                } else {
                    block_cache_size
                },
            ))),
            rowgroup_cache: Arc::new(RwLock::new(HashMap::new())),
            rowgroup_ttl_sec,
            zonemap_cache: Arc::new(DashMap::new()),
            global_stats: Arc::new(RwLock::new(GlobalStatistics {
                collection_stats: HashMap::new(),
                correlations: HashMap::new(),
                query_patterns: QueryPatterns {
                    frequent_filters: HashMap::new(),
                    common_projections: Vec::new(),
                    avg_k_value: 10.0,
                    peak_qps_times: Vec::new(),
                },
            })),
            filesystem,
            cache_stats: Arc::new(CacheStatistics {
                superblock_hits: AtomicU64::new(0),
                block_hits: AtomicU64::new(0),
                rowgroup_hits: AtomicU64::new(0),
                cache_misses: AtomicU64::new(0),
                bytes_saved: AtomicU64::new(0),
            }),
        }
    }

    /// Get SuperBlock statistics (always cached)
    pub async fn get_superblock_stats(&self, superblock_id: &str) -> Result<Arc<SuperBlockStats>> {
        // Check cache first
        if let Some(stats) = self.superblock_cache.get(superblock_id) {
            self.cache_stats
                .superblock_hits
                .fetch_add(1, Ordering::Relaxed);
            return Ok(stats.clone());
        }

        // Load from disk
        let stats = self.load_superblock_stats(superblock_id).await?;
        let stats_arc = Arc::new(stats);

        // Cache it
        self.superblock_cache
            .insert(superblock_id.to_string(), stats_arc.clone());

        Ok(stats_arc)
    }

    /// Get Block statistics with LRU eviction
    pub async fn get_block_stats(&self, block_id: &str) -> Result<Arc<BlockStats>> {
        // Check cache
        {
            let mut cache = self.block_cache.write().await;
            if let Some(stats) = cache.get(&block_id.to_string()) {
                self.cache_stats.block_hits.fetch_add(1, Ordering::Relaxed);
                return Ok(Arc::clone(stats));
            }
        }

        // Load from disk
        let stats = self.load_block_stats(block_id).await?;
        let stats_arc = Arc::new(stats);

        // Update cache
        {
            let mut cache = self.block_cache.write().await;
            cache.put(block_id.to_string(), stats_arc.clone());
        }

        Ok(stats_arc)
    }

    /// Get RowGroup statistics with TTL
    pub async fn get_rowgroup_stats(&self, rowgroup_id: &str) -> Result<Arc<RowGroupStats>> {
        // Check cache with TTL
        {
            let cache = self.rowgroup_cache.read().await;
            if let Some((stats, timestamp)) = cache.get(rowgroup_id) {
                if timestamp.elapsed().as_secs() < self.rowgroup_ttl_sec {
                    self.cache_stats
                        .rowgroup_hits
                        .fetch_add(1, Ordering::Relaxed);
                    return Ok(stats.clone());
                }
            }
        }

        // Load from disk
        let stats = self.load_rowgroup_stats(rowgroup_id).await?;
        let stats_arc = Arc::new(stats);

        // Update cache
        {
            let mut cache = self.rowgroup_cache.write().await;
            cache.insert(rowgroup_id.to_string(), (stats_arc.clone(), Instant::now()));

            // Clean expired entries
            cache.retain(|_, (_, timestamp)| timestamp.elapsed().as_secs() < self.rowgroup_ttl_sec);
        }

        Ok(stats_arc)
    }

    /// Use zone maps for query pruning
    pub async fn prune_with_zonemaps(
        &self,
        query_range: &DimensionRange,
        collections: &[String],
    ) -> Result<Vec<String>> {
        let mut matching_blocks = Vec::new();

        for collection in collections {
            let cache_key = format!("{}_zonemap", collection);

            if let Some(zonemap) = self.zonemap_cache.get(&cache_key) {
                // Check if query range overlaps with zone map
                for dim_range in &zonemap.dimension_ranges {
                    if dim_range.dim_index == query_range.dim_index {
                        if !(query_range.max_value < dim_range.min_value
                            || query_range.min_value > dim_range.max_value)
                        {
                            matching_blocks.push(collection.clone());
                            break;
                        }
                    }
                }
            }
        }

        debug!(
            "Zone map pruning: {} -> {} blocks",
            collections.len(),
            matching_blocks.len()
        );

        Ok(matching_blocks)
    }

    /// Update global statistics sidecar
    pub async fn update_global_stats(&self, collection_id: &str, stats: CollectionStatistics) {
        let mut global = self.global_stats.write().await;
        global
            .collection_stats
            .insert(collection_id.to_string(), stats);
    }

    /// Get query optimization hints from global statistics
    pub async fn get_optimization_hints(&self, collection_id: &str) -> OptimizationHints {
        let global = self.global_stats.read().await;

        let hot_zones = global
            .collection_stats
            .get(collection_id)
            .map(|s| s.hot_zones.clone())
            .unwrap_or_default();

        let common_projections = global.query_patterns.common_projections.clone();

        OptimizationHints {
            hot_zones,
            common_projections,
            estimated_selectivity: 0.1, // Would calculate based on patterns
            suggested_parallelism: 4,
        }
    }

    /// Preload statistics for a collection
    pub async fn preload_collection_stats(&self, collection_id: &str) -> Result<()> {
        info!(
            "Preloading hierarchical statistics for collection {}",
            collection_id
        );

        // Load all SuperBlock stats (small, critical)
        let superblock_path = format!("{}/superblock_stats.bin", collection_id);
        if self
            .filesystem
            .exists(&superblock_path)
            .await
            .unwrap_or(false)
        {
            let data = self.filesystem.read(&superblock_path).await?;
            let stats: Vec<SuperBlockStats> = bincode::deserialize(&data)?;

            for stat in stats {
                let key = format!("{}_{}", collection_id, stat.id);
                self.superblock_cache.insert(key, Arc::new(stat));
            }
        }

        // Load zone maps (critical for pruning)
        let zonemap_path = format!("{}/zonemaps.bin", collection_id);
        if self.filesystem.exists(&zonemap_path).await.unwrap_or(false) {
            let data = self.filesystem.read(&zonemap_path).await?;
            let zonemaps: HashMap<String, ZoneMap> = bincode::deserialize(&data)?;

            for (key, zonemap) in zonemaps {
                self.zonemap_cache.insert(key, Arc::new(zonemap));
            }
        }

        Ok(())
    }

    // Helper methods for loading from disk
    async fn load_superblock_stats(&self, superblock_id: &str) -> Result<SuperBlockStats> {
        let path = format!("stats/superblock/{}.bin", superblock_id);
        let data = self.filesystem.read(&path).await?;
        Ok(bincode::deserialize(&data)?)
    }

    async fn load_block_stats(&self, block_id: &str) -> Result<BlockStats> {
        let path = format!("stats/block/{}.bin", block_id);
        let data = self.filesystem.read(&path).await?;
        Ok(bincode::deserialize(&data)?)
    }

    async fn load_rowgroup_stats(&self, rowgroup_id: &str) -> Result<RowGroupStats> {
        let path = format!("stats/rowgroup/{}.bin", rowgroup_id);
        let data = self.filesystem.read(&path).await?;
        Ok(bincode::deserialize(&data)?)
    }
}

#[derive(Debug, Clone)]
pub struct OptimizationHints {
    pub hot_zones: Vec<ZoneMap>,
    pub common_projections: Vec<Vec<String>>,
    pub estimated_selectivity: f32,
    pub suggested_parallelism: usize,
}
