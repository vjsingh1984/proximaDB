// Memory-First Optimization for PRISM Engine
// Maximizes in-memory caching and minimizes I/O for read-heavy workloads

use crate::storage::persistence::filesystem::FileSystem;
use crate::utils::cache::LruCache;
use anyhow::{Result, anyhow};
use dashmap::DashMap;
use memmap2::Mmap;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::{debug, info};

use crate::core::VectorRecord;
use crate::storage::persistence::filesystem::zero_copy_filesystem::ZeroCopyFilesystem;

/// Memory tier configuration for PRISM's hierarchical caching
/// Aligned with PRISM's progressive quantization levels
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryTierConfig {
    /// Memory-resident tiers (always cached):
    /// Binary sketches: 1 bit/dim - fits entirely in L2 cache
    pub binary_cache_size_mb: usize,

    /// INT8 quantized: 8 bits/dim - fits in main memory
    pub int8_cache_size_mb: usize,

    /// PQ codes: 4-8 bits/dim - fits in main memory
    pub pq_cache_size_mb: usize,

    /// Cloud-resident tier (L0 - selectively cached):
    /// FP32 vectors: 32 bits/dim - stored in cloud, cached on demand
    pub fp32_cache_size_mb: usize,
    pub fp32_cache_ttl_sec: u64,

    /// Metadata always in memory
    pub metadata_cache_size_mb: usize,

    /// Prefetch configuration
    pub prefetch_ahead_count: usize,
    pub prefetch_batch_size: usize,

    /// Eviction policy for FP32 cache only (others are persistent)
    pub fp32_eviction_policy: EvictionPolicy,

    /// Memory pressure threshold (0.0 - 1.0)
    pub memory_pressure_threshold: f32,
}

impl Default for MemoryTierConfig {
    fn default() -> Self {
        Self {
            binary_cache_size_mb: 128, // 128MB for binary sketches (covers millions of vectors)
            int8_cache_size_mb: 512,   // 512MB for INT8 quantized
            pq_cache_size_mb: 1024,    // 1GB for PQ codes
            fp32_cache_size_mb: 512,   // 512MB for selective FP32 caching from cloud
            fp32_cache_ttl_sec: 300,   // 5 minute TTL for FP32 cache
            metadata_cache_size_mb: 256, // 256MB for metadata/bloom filters
            prefetch_ahead_count: 10,
            prefetch_batch_size: 100,
            fp32_eviction_policy: EvictionPolicy::AdaptiveLRU,
            memory_pressure_threshold: 0.85,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EvictionPolicy {
    LRU,
    LFU,
    ARC,         // Adaptive Replacement Cache
    AdaptiveLRU, // LRU with frequency boost
}

/// Memory-optimized storage aligned with PRISM's progressive quantization levels
/// Binary/INT8/PQ always in memory, FP32 in cloud with selective caching
pub struct MemoryOptimizedStorage {
    /// Configuration
    config: MemoryTierConfig,

    /// ALWAYS IN MEMORY (Small, frequently accessed):
    /// Binary sketches: 1 bit/dim - Memory-mapped for SIMD operations
    binary_sketches: Arc<RwLock<HashMap<String, Arc<Mmap>>>>,

    /// INT8 quantized: 8 bits/dim - Memory-mapped for fast ranking
    int8_vectors: Arc<RwLock<HashMap<String, Arc<Mmap>>>>,

    /// PQ codes: 4-8 bits/dim - Memory-mapped for refined ranking
    pq_codes: Arc<RwLock<HashMap<String, Arc<Mmap>>>>,

    /// Metadata: Bloom filters, inverted indices - always resident
    metadata_cache: Arc<RwLock<HashMap<String, MetadataItem>>>,

    /// SELECTIVELY CACHED (Large, infrequently accessed):
    /// FP32 vectors from cloud L0 files - LRU cache with TTL
    fp32_cache: Arc<RwLock<LruCache<String, (Arc<VectorRecord>, Instant)>>>,

    /// Cloud storage interface for L0 FP32 files
    filesystem: Arc<ZeroCopyFilesystem>,

    /// Access statistics for adaptive caching
    access_stats: Arc<AccessStatistics>,

    /// Prefetch queue for predictive loading
    prefetch_queue: Arc<RwLock<Vec<String>>>,

    /// Memory pressure monitor
    memory_monitor: Arc<MemoryMonitor>,
}

/// Metadata entry for fast filtering
#[derive(Clone)]
pub struct MetadataItem {
    pub bloom_filter: Vec<u8>,
    pub inverted_index: HashMap<String, Vec<String>>,
    pub statistics: VectorStatistics,
}

#[derive(Clone)]
pub struct VectorStatistics {
    pub min_value: f32,
    pub max_value: f32,
    pub mean: f32,
    pub std_dev: f32,
}

/// Access statistics for adaptive caching decisions
pub struct AccessStatistics {
    /// Access counts per vector ID
    access_counts: DashMap<String, AtomicUsize>,

    /// Last access time per vector ID
    last_access: DashMap<String, Instant>,

    /// Access patterns (sequential vs random)
    sequential_ratio: AtomicU64,
    random_ratio: AtomicU64,

    /// Hit rates per tier
    l1_hits: AtomicU64,
    l2_hits: AtomicU64,
    l3_hits: AtomicU64,
    misses: AtomicU64,
}

/// Memory pressure monitor
pub struct MemoryMonitor {
    /// Current memory usage in bytes
    current_usage: AtomicU64,

    /// Maximum allowed memory in bytes
    max_memory: u64,

    /// Memory pressure events
    pressure_events: AtomicU64,
}

impl MemoryOptimizedStorage {
    pub fn new(config: MemoryTierConfig, filesystem: Arc<ZeroCopyFilesystem>) -> Result<Self> {
        let fp32_capacity =
            (config.fp32_cache_size_mb * 1024 * 1024) / std::mem::size_of::<VectorRecord>();

        let total_memory = (config.binary_cache_size_mb
            + config.int8_cache_size_mb
            + config.pq_cache_size_mb
            + config.fp32_cache_size_mb
            + config.metadata_cache_size_mb) as u64
            * 1024
            * 1024;

        Ok(Self {
            config: config.clone(),
            binary_sketches: Arc::new(RwLock::new(HashMap::new())),
            int8_vectors: Arc::new(RwLock::new(HashMap::new())),
            pq_codes: Arc::new(RwLock::new(HashMap::new())),
            metadata_cache: Arc::new(RwLock::new(HashMap::new())),
            fp32_cache: Arc::new(RwLock::new(LruCache::new(if fp32_capacity == 0 {
                100
            } else {
                fp32_capacity
            }))),
            filesystem,
            access_stats: Arc::new(AccessStatistics::new()),
            prefetch_queue: Arc::new(RwLock::new(Vec::new())),
            memory_monitor: Arc::new(MemoryMonitor::new(total_memory)),
        })
    }

    /// Progressive search through PRISM's quantization levels
    /// Only fetches FP32 from cloud when absolutely necessary
    pub async fn progressive_search(
        &self,
        query: &[f32],
        k: usize,
        metadata_filter: Option<HashMap<String, String>>,
    ) -> Result<Vec<(String, f32)>> {
        // Phase 1: Metadata filtering (always in memory)
        let candidates = if let Some(filter) = metadata_filter {
            self.filter_by_metadata(&filter).await?
        } else {
            self.get_all_vector_ids().await?
        };

        // Phase 2: Binary sketch filtering (always in memory, ultra-fast)
        let binary_candidates = self
            .search_binary_sketches(query, &candidates, k * 10)
            .await?;

        // Phase 3: INT8 ranking (always in memory, fast)
        let int8_candidates = self
            .search_int8_vectors(query, &binary_candidates, k * 5)
            .await?;

        // Phase 4: PQ refinement (always in memory, accurate)
        let pq_candidates = self.search_pq_codes(query, &int8_candidates, k * 2).await?;

        // Phase 5: FP32 reranking (fetch from cloud only for top candidates)
        let final_results = self.rerank_with_fp32(query, &pq_candidates, k).await?;

        Ok(final_results)
    }

    /// Get full precision vector (may require cloud fetch)
    pub async fn fp32_vector(&self, id: &str) -> Result<Option<Arc<VectorRecord>>> {
        // Check FP32 cache first
        {
            let mut cache = self.fp32_cache.write().await;
            if let Some((vector, timestamp)) = cache.get(&id.to_string()) {
                // Check TTL
                if timestamp.elapsed().as_secs() < self.config.fp32_cache_ttl_sec {
                    self.access_stats.record_l1_hit();
                    return Ok(Some(Arc::clone(vector)));
                } else {
                    // Expired, remove from cache
                    cache.pop(&id.to_string());
                }
            }
        }

        // Fetch from cloud L0 file
        match self.fetch_fp32_from_cloud(id).await {
            Ok(vector) => {
                // Add to cache
                let mut cache = self.fp32_cache.write().await;
                cache.put(id.to_string(), (vector.clone(), Instant::now()));
                Ok(Some(vector))
            }
            Err(_) => {
                // Cloud fetch failed, return None
                Ok(None)
            }
        }
    }

    /// Batch get with prefetching
    pub async fn vectors_batch(&self, ids: &[String]) -> Result<Vec<Option<Arc<VectorRecord>>>> {
        // Trigger prefetch for likely next accesses
        self.prefetch_next_batch(ids).await?;

        let mut results = Vec::with_capacity(ids.len());
        for id in ids {
            results.push(self.fp32_vector(id).await?);
        }

        Ok(results)
    }

    /// Memory-map a file for ultra-fast access
    pub async fn mmap_file(
        &self,
        file_path: &str,
        tier: MemoryTier,
        filesystem: &Arc<dyn FileSystem>,
    ) -> Result<Arc<Mmap>> {
        // Use filesystem API for cloud compatibility
        let mmap = filesystem
            .get_mmap(file_path)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Memory mapping not supported for {}", file_path))?;
        let mmap_arc = Arc::new(mmap);

        match tier {
            MemoryTier::L2Binary => {
                let mut cache = self.binary_sketches.write().await;
                cache.insert(file_path.to_string(), mmap_arc.clone());
            }
            MemoryTier::L2Quantized => {
                let mut cache = self.int8_vectors.write().await;
                cache.insert(file_path.to_string(), mmap_arc.clone());
            }
            _ => {}
        }

        Ok(mmap_arc)
    }

    /// Get vector from L2 memory-mapped cache
    async fn get_from_l2_mmap(&self, id: &str) -> Result<Option<Arc<VectorRecord>>> {
        // Check binary sketches first (smallest, fastest)
        if let Some(binary_data) = self.get_binary_sketch(id).await? {
            // For initial filtering, we might return a lightweight version
            // Real implementation would decode the binary sketch
            return Ok(None); // Placeholder
        }

        // Check quantized vectors
        if let Some(quantized_data) = self.get_quantized_vector(id).await? {
            // Decode quantized vector
            // Real implementation would use the quantization engine
            return Ok(None); // Placeholder
        }

        Ok(None)
    }

    /// Get vector from L3 compressed cache
    async fn get_from_l3_compressed(&self, id: &str) -> Result<Option<Arc<VectorRecord>>> {
        let cache = self.pq_codes.read().await;
        if let Some(compressed_data) = cache.get(id) {
            // Decompress vector
            // Real implementation would use compression library
            return Ok(None); // Placeholder
        }
        Ok(None)
    }

    /// Promote vector to L1 cache
    async fn promote_to_l1(&self, id: &str, vector: Arc<VectorRecord>) -> Result<()> {
        let mut l1 = self.fp32_cache.write().await;

        // Check memory pressure before promotion
        if self.memory_monitor.is_under_pressure() {
            self.evict_cold_entries().await?;
        }

        l1.put(id.to_string(), (vector, Instant::now()));
        Ok(())
    }

    /// Prefetch next batch based on access patterns
    async fn prefetch_next_batch(&self, current_ids: &[String]) -> Result<()> {
        // Analyze access pattern
        let is_sequential = self.access_stats.is_sequential_pattern(current_ids);

        if is_sequential {
            // Prefetch next N vectors in sequence
            let mut prefetch_queue = self.prefetch_queue.write().await;

            // Simple sequential prefetch (would be more sophisticated in practice)
            if let Some(last_id) = current_ids.last() {
                for i in 0..self.config.prefetch_ahead_count {
                    let next_id = format!("{}_next_{}", last_id, i);
                    prefetch_queue.push(next_id);
                }
            }

            // Note: Can't spawn async task from &self reference
            // This would need to be redesigned to work with Arc<Self>
            // For now, skip the async prefetch
        }

        Ok(())
    }

    /// Execute prefetch in background
    async fn execute_prefetch(self: Arc<Self>) -> Result<()> {
        let queue = {
            let mut q = self.prefetch_queue.write().await;
            q.drain(..).collect::<Vec<_>>()
        };

        for id in queue {
            // Load into appropriate tier based on access stats
            if self.access_stats.is_hot(&id) {
                // Load into L1
                // Implementation would fetch from disk
            } else if self.access_stats.is_warm(&id) {
                // Load into L2
                // Implementation would memory-map the file
            } else {
                // Load into L3 compressed
                // Implementation would load and compress
            }
        }

        Ok(())
    }

    /// Evict cold entries when under memory pressure
    async fn evict_cold_entries(&self) -> Result<()> {
        info!("Memory pressure detected, evicting cold entries");

        // Evict from L1 first
        let l1 = self.fp32_cache.write().await;

        // Simple eviction: remove least recently used
        // LruCache handles this automatically

        // If still under pressure, evict from L2
        if self.memory_monitor.is_under_pressure() {
            let mut l2_binary = self.binary_sketches.write().await;
            let mut l2_quantized = self.int8_vectors.write().await;

            // Remove least accessed mmaps
            // Real implementation would track access stats
            l2_binary.clear();
            l2_quantized.clear();
        }

        Ok(())
    }

    /// Get binary sketch from memory-mapped file
    async fn get_binary_sketch(&self, _id: &str) -> Result<Option<Vec<u8>>> {
        // Placeholder - would read from mmap
        Ok(None)
    }

    /// Get quantized vector from memory-mapped file
    async fn get_quantized_vector(&self, _id: &str) -> Result<Option<Vec<u8>>> {
        // Placeholder - would read from mmap
        Ok(None)
    }

    /// Filter vectors by metadata criteria using inverted indices and bloom filters
    async fn filter_by_metadata(&self, filter: &HashMap<String, String>) -> Result<Vec<String>> {
        debug!("🔍 Filtering vectors by metadata: {:?}", filter);
        
        let metadata_cache = self.metadata_cache.read().await;
        let mut matching_ids = Vec::new();
        
        // Iterate through all cached metadata entries
        for (vector_id, metadata_entry) in metadata_cache.iter() {
            let mut matches_all_filters = true;
            
            // Check each filter condition
            for (key, expected_value) in filter {
                if let Some(actual_value) = metadata_entry.get(key) {
                    // Support different comparison modes based on value format
                    let matches = if expected_value.starts_with(">=") {
                        // Numeric greater-than-or-equal comparison
                        if let (Ok(expected), Ok(actual)) = (
                            expected_value[2..].parse::<f64>(),
                            actual_value.parse::<f64>()
                        ) {
                            actual >= expected
                        } else {
                            false
                        }
                    } else if expected_value.starts_with("<=") {
                        // Numeric less-than-or-equal comparison
                        if let (Ok(expected), Ok(actual)) = (
                            expected_value[2..].parse::<f64>(),
                            actual_value.parse::<f64>()
                        ) {
                            actual <= expected
                        } else {
                            false
                        }
                    } else if expected_value.contains('*') {
                        // Wildcard string matching (simplified - no regex dependency)
                        if expected_value == "*" {
                            true // Match any value
                        } else if expected_value.ends_with('*') {
                            let prefix = &expected_value[..expected_value.len()-1];
                            actual_value.starts_with(prefix)
                        } else if expected_value.starts_with('*') {
                            let suffix = &expected_value[1..];
                            actual_value.ends_with(suffix)
                        } else {
                            // More complex patterns - fall back to exact match
                            actual_value == expected_value
                        }
                    } else {
                        // Exact string matching
                        actual_value == expected_value
                    };
                    
                    if !matches {
                        matches_all_filters = false;
                        break;
                    }
                } else {
                    // Missing metadata field means no match
                    matches_all_filters = false;
                    break;
                }
            }
            
            if matches_all_filters {
                matching_ids.push(vector_id.clone());
            }
        }
        
        info!("📊 Metadata filtering found {} matching vectors out of {} total", 
              matching_ids.len(), metadata_cache.len());
        
        Ok(matching_ids)
    }

    /// Get all vector IDs currently stored in the memory optimizer
    async fn get_all_vector_ids(&self) -> Result<Vec<String>> {
        debug!("📋 Retrieving all vector IDs from PRISM memory optimizer");
        
        let metadata = self.metadata_cache.read().await;
        let vector_ids: Vec<String> = metadata.keys().cloned().collect();
        
        debug!("📊 Found {} vector IDs in PRISM memory cache", vector_ids.len());
        Ok(vector_ids)
    }

    async fn search_binary_sketches(
        &self,
        _query: &[f32],
        candidates: &[String],
        k: usize,
    ) -> Result<Vec<String>> {
        // TODO: Implement binary sketch search
        Ok(candidates.iter().take(k).cloned().collect())
    }

    async fn search_int8_vectors(
        &self,
        _query: &[f32],
        candidates: &[String],
        k: usize,
    ) -> Result<Vec<String>> {
        // TODO: Implement INT8 vector search
        Ok(candidates.iter().take(k).cloned().collect())
    }

    async fn search_pq_codes(
        &self,
        _query: &[f32],
        candidates: &[String],
        k: usize,
    ) -> Result<Vec<String>> {
        // TODO: Implement PQ code search
        Ok(candidates.iter().take(k).cloned().collect())
    }

    async fn rerank_with_fp32(
        &self,
        _query: &[f32],
        candidates: &[String],
        k: usize,
    ) -> Result<Vec<(String, f32)>> {
        // TODO: Implement FP32 reranking
        Ok(candidates
            .iter()
            .take(k)
            .enumerate()
            .map(|(i, id)| (id.clone(), 1.0 - (i as f32 / k as f32)))
            .collect())
    }

    async fn fetch_fp32_from_cloud(&self, _id: &str) -> Result<Arc<VectorRecord>> {
        // TODO: Implement cloud fetch
        Err(anyhow!("FP32 cloud fetch not implemented"))
    }

    async fn vector(&self, id: &str) -> Result<VectorRecord> {
        // TODO: Implement vector retrieval
        self.fetch_fp32_from_cloud(id)
            .await
            .map(|arc| (*arc).clone())
    }
}

impl AccessStatistics {
    fn new() -> Self {
        Self {
            access_counts: DashMap::new(),
            last_access: DashMap::new(),
            sequential_ratio: AtomicU64::new(0),
            random_ratio: AtomicU64::new(0),
            l1_hits: AtomicU64::new(0),
            l2_hits: AtomicU64::new(0),
            l3_hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    fn record_l1_hit(&self) {
        self.l1_hits.fetch_add(1, Ordering::Relaxed);
    }

    fn record_l2_hit(&self) {
        self.l2_hits.fetch_add(1, Ordering::Relaxed);
    }

    fn record_l3_hit(&self) {
        self.l3_hits.fetch_add(1, Ordering::Relaxed);
    }

    fn record_miss(&self) {
        self.misses.fetch_add(1, Ordering::Relaxed);
    }

    fn is_sequential_pattern(&self, _ids: &[String]) -> bool {
        // Simplified - would analyze actual pattern
        self.sequential_ratio.load(Ordering::Relaxed) > self.random_ratio.load(Ordering::Relaxed)
    }

    fn is_hot(&self, id: &str) -> bool {
        if let Some(count) = self.access_counts.get(id) {
            count.load(Ordering::Relaxed) > 10
        } else {
            false
        }
    }

    fn is_warm(&self, id: &str) -> bool {
        if let Some(count) = self.access_counts.get(id) {
            let c = count.load(Ordering::Relaxed);
            c > 2 && c <= 10
        } else {
            false
        }
    }
}

impl MemoryMonitor {
    fn new(max_memory: u64) -> Self {
        Self {
            current_usage: AtomicU64::new(0),
            max_memory,
            pressure_events: AtomicU64::new(0),
        }
    }

    fn is_under_pressure(&self) -> bool {
        let usage = self.current_usage.load(Ordering::Relaxed);
        let pressure = usage as f64 / self.max_memory as f64;

        if pressure > 0.85 {
            self.pressure_events.fetch_add(1, Ordering::Relaxed);
            true
        } else {
            false
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MemoryTier {
    L1Heap,
    L2Binary,
    L2Quantized,
    L3Compressed,
}

/// Memory optimization strategies for PRISM
pub struct PrismMemoryOptimizer {
    storage: Arc<MemoryOptimizedStorage>,
    filesystem: Arc<ZeroCopyFilesystem>,
}

impl PrismMemoryOptimizer {
    pub fn new(config: MemoryTierConfig, filesystem: Arc<ZeroCopyFilesystem>) -> Result<Self> {
        Ok(Self {
            storage: Arc::new(MemoryOptimizedStorage::new(config, filesystem.clone())?),
            filesystem,
        })
    }

    /// Preload hot data into memory on startup
    pub async fn preload_hot_data(&self, collection_id: &str) -> Result<()> {
        info!("Preloading hot data for collection {}", collection_id);

        // Load binary sketches into L2 (smallest, most frequently accessed)
        let binary_path = format!("{}/binary_sketches.bin", collection_id);
        if self.filesystem.exists(&binary_path).await.unwrap_or(false) {
            self.storage
                .mmap_file(
                    &binary_path,
                    MemoryTier::L2Binary,
                    &(self.filesystem.clone() as Arc<dyn FileSystem>),
                )
                .await?;
            debug!("Loaded binary sketches into L2 memory-mapped cache");
        }

        // Load frequently accessed quantized vectors
        let quantized_path = format!("{}/quantized_vectors.pq", collection_id);
        if self
            .filesystem
            .exists(&quantized_path)
            .await
            .unwrap_or(false)
        {
            self.storage
                .mmap_file(
                    &quantized_path,
                    MemoryTier::L2Quantized,
                    &(self.filesystem.clone() as Arc<dyn FileSystem>),
                )
                .await?;
            debug!("Loaded quantized vectors into L2 memory-mapped cache");
        }

        Ok(())
    }

    /// Get cache statistics
    pub async fn stats(&self) -> CacheStats {
        let stats = &self.storage.access_stats;

        let total_hits = stats.l1_hits.load(Ordering::Relaxed)
            + stats.l2_hits.load(Ordering::Relaxed)
            + stats.l3_hits.load(Ordering::Relaxed);
        let total_requests = total_hits + stats.misses.load(Ordering::Relaxed);

        CacheStats {
            l1_hit_rate: stats.l1_hits.load(Ordering::Relaxed) as f32 / total_requests as f32,
            l2_hit_rate: stats.l2_hits.load(Ordering::Relaxed) as f32 / total_requests as f32,
            l3_hit_rate: stats.l3_hits.load(Ordering::Relaxed) as f32 / total_requests as f32,
            overall_hit_rate: total_hits as f32 / total_requests as f32,
            memory_usage_mb: self
                .storage
                .memory_monitor
                .current_usage
                .load(Ordering::Relaxed)
                / 1024
                / 1024,
            pressure_events: self
                .storage
                .memory_monitor
                .pressure_events
                .load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheStats {
    pub l1_hit_rate: f32,
    pub l2_hit_rate: f32,
    pub l3_hit_rate: f32,
    pub overall_hit_rate: f32,
    pub memory_usage_mb: u64,
    pub pressure_events: u64,
}
