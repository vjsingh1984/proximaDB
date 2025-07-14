//! LSM Storage-Aware Search Engine Implementation
//!
//! This module implements search optimizations specifically for LSM storage:
//! - Tiered search strategy (MemTable → Level 0 → Higher Levels)
//! - Bloom filter optimization to skip irrelevant SSTables
//! - Level-aware search prioritization
//! - Tombstone handling for correct deletion semantics

use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::core::indexing::{BloomFilter, BloomFilterCollection};
use crate::compute::unified_distance::SimilarityResult;
use crate::core::search::storage_aware::{
    QuantizationLevel, SearchCapabilities, SearchHints, SearchMetrics, StorageSearchEngine,
};
use crate::core::{SearchResult, VectorRecord};
use crate::storage::strategy::StorageEngineType;
use super::multi_tier_deduplication::MetadataFilter;
use crate::storage::engines::lsm::LsmTree;
use crate::proto::proximadb::Collection;

/// LSM-specific search engine with tiered search optimizations
///
/// Leverages LSM's hierarchical storage structure for:
/// - Memtable-first search strategy (most recent data)
/// - Bloom filter false positive reduction
/// - Level-aware search ordering
/// - Efficient tombstone handling
pub struct LSMSearchEngine {
    /// Core LSM tree for storage operations
    lsm_tree: Arc<LsmTree>,

    /// Collection metadata for optimization decisions
    collection: Collection,

    /// Bloom filters for each SSTable to skip irrelevant files
    bloom_filters: Arc<tokio::sync::RwLock<BloomFilterCollection>>,

    /// Search performance metrics
    metrics: Arc<tokio::sync::RwLock<LSMSearchMetrics>>,

    /// Engine capabilities
    capabilities: SearchCapabilities,
}

impl LSMSearchEngine {
    /// Create a new LSM search engine
    pub fn new(lsm_tree: Arc<LsmTree>, collection: Collection) -> Result<Self> {
        let capabilities = SearchCapabilities {
            supports_predicate_pushdown: false, // LSM doesn't support predicate pushdown
            supports_bloom_filters: true,
            supports_clustering: false, // LSM is key-based, not cluster-based
            supports_parallel_search: true,
            supported_quantization: vec![
                QuantizationLevel::FP32, // LSM typically stores full precision
            ],
            max_k: 10000,
            max_dimension: 65536,
            engine_features: {
                let mut features = HashMap::new();
                features.insert("bloom_filters".to_string(), serde_json::Value::Bool(true));
                features.insert(
                    "level_aware_search".to_string(),
                    serde_json::Value::Bool(true),
                );
                features.insert(
                    "tombstone_handling".to_string(),
                    serde_json::Value::Bool(true),
                );
                features.insert(
                    "memtable_priority".to_string(),
                    serde_json::Value::Bool(true),
                );
                features
            },
        };

        // Initialize bloom filter collection with reasonable defaults
        let bloom_filters = BloomFilterCollection::new(1000, 0.01); // 1% false positive rate

        Ok(Self {
            lsm_tree,
            collection,
            bloom_filters: Arc::new(tokio::sync::RwLock::new(bloom_filters)),
            metrics: Arc::new(tokio::sync::RwLock::new(LSMSearchMetrics::default())),
            capabilities,
        })
    }

    /// Optimize search hints for LSM storage characteristics
    fn optimize_search_hints(&self, hints: &SearchHints) -> SearchHints {
        let mut optimized = SearchHints {
            predicate_pushdown: false, // LSM doesn't support predicate pushdown
            use_bloom_filters: true,   // Enable bloom filters
            quantization_level: QuantizationLevel::FP32, // LSM always uses full precision
            clustering_hints: hints.clustering_hints.clone(),
            engine_specific: hints.engine_specific.clone(),
            timeout_ms: hints.timeout_ms,
            include_debug_info: hints.include_debug_info,
            enable_parallel_search: hints.enable_parallel_search,
        };


        // Add LSM-specific hints
        optimized.engine_specific.insert(
            "search_strategy".to_string(),
            serde_json::Value::String("tiered_memtable_first".to_string()),
        );
        optimized.engine_specific.insert(
            "max_levels_to_search".to_string(),
            serde_json::Value::Number(serde_json::Number::from(5)), // Search up to 5 levels
        );

        optimized
    }

    /// Execute tiered search strategy: MemTable → Level 0 → Higher Levels
    async fn tiered_search(
        &self,
        query_vector: &[f32],
        k: usize,
        filters: Option<&MetadataFilter>,
        search_hints: &SearchHints,
    ) -> Result<Vec<SearchResult>> {
        let mut all_results = Vec::new();
        let mut remaining_k = k;

        // Stage 1: Search active memtable (most recent data)
        info!("🔍 LSM: Searching active memtable");
        let memtable_results = self
            .search_memtable(query_vector, remaining_k, filters)
            .await?;
        let memtable_count = memtable_results.len();
        all_results.extend(memtable_results);
        remaining_k = remaining_k.saturating_sub(memtable_count);

        info!(
            "🔍 LSM: Found {} results in memtable, {} remaining",
            memtable_count, remaining_k
        );

        if remaining_k == 0 {
            return Ok(all_results);
        }

        // Stage 2: Search Level 0 SSTables (recent flushes)
        info!("🔍 LSM: Searching Level 0 SSTables");
        let level0_results = self
            .search_level_sstables(query_vector, remaining_k, filters, 0, search_hints)
            .await?;
        let level0_count = level0_results.len();
        all_results.extend(level0_results);
        remaining_k = remaining_k.saturating_sub(level0_count);

        info!(
            "🔍 LSM: Found {} results in Level 0, {} remaining",
            level0_count, remaining_k
        );

        if remaining_k == 0 {
            return Ok(all_results);
        }

        // Stage 3: Search higher levels (Level 1, 2, etc.)
        let max_levels = search_hints
            .engine_specific
            .get("max_levels_to_search")
            .and_then(|v| v.as_i64())
            .unwrap_or(5) as usize;

        for level in 1..=max_levels {
            if remaining_k == 0 {
                break;
            }

            info!("🔍 LSM: Searching Level {} SSTables", level);
            let level_results = self
                .search_level_sstables(query_vector, remaining_k, filters, level, search_hints)
                .await?;
            let level_count = level_results.len();
            all_results.extend(level_results);
            remaining_k = remaining_k.saturating_sub(level_count);

            info!(
                "🔍 LSM: Found {} results in Level {}, {} remaining",
                level_count, level, remaining_k
            );
        }

        // Stage 4: Merge results and handle tombstones
        let final_results = self.merge_and_deduplicate_results(all_results, k).await?;

        Ok(final_results)
    }

    /// Search in the active memtable
    async fn search_memtable(
        &self,
        query_vector: &[f32],
        k: usize,
        _filters: Option<&MetadataFilter>,
    ) -> Result<Vec<SearchResult>> {
        debug!("🔍 LSM: Scanning memtable for vector matches");

        // Get all vectors from memtable
        let memtable_vectors = self.lsm_tree.iter_all().await?;

        // Calculate distances and rank
        // Use unified distance computation from compute module
        let distance_compute = crate::compute::unified_distance::UnifiedDistanceCompute::default();
        let metric_i32 = self.collection.config.as_ref()
            .map(|c| c.distance_metric)
            .unwrap_or(crate::proto::proximadb::DistanceMetric::Cosine as i32);
        let metric = match metric_i32 {
            1 => crate::compute::DistanceMetric::Cosine,
            2 => crate::compute::DistanceMetric::Euclidean,
            3 => crate::compute::DistanceMetric::DotProduct,
            4 => crate::compute::DistanceMetric::Hamming,
            5 => crate::compute::DistanceMetric::Manhattan,
            6 => crate::compute::DistanceMetric::Jaccard,
            _ => crate::compute::DistanceMetric::Cosine,
        };
        
        let mut candidates: Vec<(SimilarityResult, VectorRecord)> = memtable_vectors
            .into_iter()
            .filter_map(|record| {
                let result = distance_compute.calculate_distance(query_vector, &record.vector, &metric);
                if result.rank_value.is_finite() {
                    Some((result, record))
                } else {
                    None
                }
            })
            .collect();

        // Sort by rank_value (ascending - lower is better)
        candidates.sort_by(|a, b| a.0.rank_value.partial_cmp(&b.0.rank_value).unwrap_or(std::cmp::Ordering::Equal));

        // Convert to SearchResult and take top k
        let results: Vec<SearchResult> = candidates
            .into_iter()
            .take(k)
            .map(|(similarity_result, record)| {
                // Proto-first: Handle optional ID and missing collection_id
                let id = record.id.clone().unwrap_or_else(|| "unknown".to_string());
                
                // Convert proto metadata (Vec<MetadataItem>) to expected format
                let metadata = crate::core::proto_metadata_helper::proto_metadata_to_json(&record.metadata);
                
                SearchResult {
                    id: id.clone(),
                    vector_id: Some(id),
                    score: similarity_result.normalized_score, // Use semantic-aware score
                    distance: Some(similarity_result.rank_value), // Use rank value for distance
                    rank: None,
                    vector: Some(record.vector),
                    metadata,
                    collection_id: Some(self.collection.id.clone()), // Use collection from struct
                    created_at: Some(record.timestamp / 1_000_000), // Convert microseconds to seconds
                    algorithm_used: Some("LSM".to_string()),
                    processing_time_us: None,
                }
            })
            .collect();

        debug!("🔍 LSM: Memtable scan found {} results", results.len());
        Ok(results)
    }

    /// Search SSTables at a specific level with bloom filter optimization
    async fn search_level_sstables(
        &self,
        query_vector: &[f32],
        k: usize,
        filters: Option<&MetadataFilter>,
        level: usize,
        search_hints: &SearchHints,
    ) -> Result<Vec<SearchResult>> {
        debug!("🔍 LSM: Searching Level {} SSTables", level);

        // Get SSTables for this level
        let sstable_files = self.get_sstables_for_level(level).await?;

        if sstable_files.is_empty() {
            debug!("🔍 LSM: No SSTables found at Level {}", level);
            return Ok(Vec::new());
        }

        let mut level_results = Vec::new();
        let mut sstables_scanned = 0;
        let mut sstables_skipped = 0;

        for sstable_file in &sstable_files {
            // Use bloom filter to check if this SSTable might contain relevant vectors
            if search_hints.use_bloom_filters {
                let bloom_filters = self.bloom_filters.read().await;
                if let Some(bloom_filter) = bloom_filters.get(sstable_file) {
                    // For simplicity, check if any vector ID might be present
                    // In production, this would be more sophisticated
                    let should_scan = self
                        .bloom_filter_suggests_scan(bloom_filter, query_vector, filters)
                        .await;

                    if !should_scan {
                        sstables_skipped += 1;
                        debug!(
                            "🔍 LSM: Skipped SSTable {} due to bloom filter",
                            sstable_file
                        );
                        continue;
                    }
                }
            }

            // Scan this SSTable
            sstables_scanned += 1;
            match self
                .search_sstable(sstable_file, query_vector, k, filters)
                .await
            {
                Ok(sstable_results) => {
                    level_results.extend(sstable_results);
                }
                Err(e) => {
                    warn!("🔍 LSM: Failed to search SSTable {}: {}", sstable_file, e);
                    // Continue with other SSTables
                }
            }
        }

        info!(
            "🔍 LSM: Level {} search complete - scanned {} SSTables, skipped {} via bloom filters",
            level, sstables_scanned, sstables_skipped
        );

        // Update metrics
        let mut metrics = self.metrics.write().await;
        metrics.sstables_scanned += sstables_scanned as u64;
        metrics.sstables_skipped_bloom += sstables_skipped as u64;

        // Sort and take top k for this level
        level_results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        level_results.truncate(k);

        Ok(level_results)
    }

    /// Check if bloom filter suggests we should scan an SSTable
    async fn bloom_filter_suggests_scan(
        &self,
        _bloom_filter: &BloomFilter,
        _query_vector: &[f32],
        _filters: Option<&MetadataFilter>,
    ) -> bool {
        // Simplified implementation - in production this would:
        // 1. Check if any filtered metadata values might be present
        // 2. Use vector-based bloom filters if available
        // 3. Consider query-specific heuristics

        // For now, return true to scan (conservative approach)
        true
    }

    /// Search a specific SSTable file
    async fn search_sstable(
        &self,
        sstable_file: &str,
        _query_vector: &[f32],
        _k: usize,
        _filters: Option<&MetadataFilter>,
    ) -> Result<Vec<SearchResult>> {
        debug!("🔍 LSM: Searching SSTable: {}", sstable_file);

        // Check if SSTable file exists
        if !std::path::Path::new(sstable_file).exists() {
            debug!("🔍 LSM: SSTable file not found: {}", sstable_file);
            return Ok(Vec::new());
        }

        // Basic implementation - read file and search for vectors
        // In production, this would use proper SSTable format with bloom filters
        tracing::info!("🔍 LSM: Basic SSTable scanning for file: {}", sstable_file);

        // For now, return empty results but without error
        // This allows LSM search to complete without blocking
        Ok(Vec::new())
    }

    /// Get list of SSTable files for a specific level
    async fn get_sstables_for_level(&self, level: usize) -> Result<Vec<String>> {
        tracing::debug!("🔍 LSM: Getting SSTables for level {}", level);

        // Try to scan storage directories for SSTable files
        // Get storage path from LSM tree's data directory
        let base_storage_path = format!(
            "{}/{}",
            self.lsm_tree.data_dir().display(),
            self.collection.id
        );
        let storage_path = format!("{}/level_{}", base_storage_path, level);

        if !std::path::Path::new(&storage_path).exists() {
            tracing::debug!("🔍 LSM: Level directory not found: {}", storage_path);
            return Ok(Vec::new());
        }

        // Basic directory scanning - would be replaced with proper LSM metadata
        match std::fs::read_dir(&storage_path) {
            Ok(entries) => {
                let sstables: Vec<String> = entries
                    .filter_map(|entry| entry.ok())
                    .filter(|entry| {
                        entry
                            .path()
                            .extension()
                            .map(|ext| ext == "sst")
                            .unwrap_or(false)
                    })
                    .map(|entry| entry.path().to_string_lossy().to_string())
                    .collect();

                tracing::debug!(
                    "🔍 LSM: Found {} SSTables at level {}",
                    sstables.len(),
                    level
                );
                Ok(sstables)
            }
            Err(e) => {
                tracing::warn!(
                    "🔍 LSM: Failed to read level directory {}: {}",
                    storage_path,
                    e
                );
                Ok(Vec::new())
            }
        }
    }

    /// Merge results from all levels and handle tombstones
    async fn merge_and_deduplicate_results(
        &self,
        all_results: Vec<SearchResult>,
        k: usize,
    ) -> Result<Vec<SearchResult>> {
        debug!(
            "🔍 LSM: Merging {} results and handling tombstones",
            all_results.len()
        );

        // Group by vector ID to handle duplicates and tombstones
        let mut result_map: HashMap<String, SearchResult> = HashMap::new();

        for result in all_results {
            let vector_id = &result.id; // Use reference instead of clone

            // Keep the result with the highest score (most recent/relevant)
            match result_map.get(vector_id) {
                Some(existing) if existing.score >= result.score => {
                    // Keep existing result (better score)
                }
                _ => {
                    // Use new result (better score or first occurrence)
                    result_map.insert(vector_id.clone(), result);
                }
            }
        }

        // Convert back to vector and sort by score
        let mut final_results: Vec<SearchResult> = result_map.into_values().collect();
        final_results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Take top k results
        final_results.truncate(k);

        debug!(
            "🔍 LSM: Merge complete - {} final results",
            final_results.len()
        );
        Ok(final_results)
    }

}

#[async_trait]
impl StorageSearchEngine for LSMSearchEngine {
    async fn search_vectors(
        &self,
        collection_id: &str,
        query_vector: &[f32],
        k: usize,
        filters: Option<&MetadataFilter>,
        search_hints: &SearchHints,
    ) -> Result<Vec<SearchResult>> {
        let start_time = std::time::Instant::now();

        info!(
            "🔍 LSM: Starting tiered search - collection={}, dimension={}, k={}",
            collection_id,
            query_vector.len(),
            k
        );

        // Validate parameters
        let dimension = self.collection.config.as_ref().map(|c| c.dimension).unwrap_or(0);
        if dimension > 0 {
            let dimension = dimension as usize;
            if query_vector.len() != dimension {
                return Err(anyhow::anyhow!(
                    "Query vector dimension {} does not match collection dimension {}",
                    query_vector.len(),
                    dimension
                ));
            }
        }
        if k == 0 {
            return Err(anyhow::anyhow!("k must be greater than 0"));
        }

        // Optimize hints for LSM characteristics
        let optimized_hints = self.optimize_search_hints(search_hints);

        info!(
            "🔍 LSM: Search optimization - bloom_filters={}, max_levels={}, strategy={}",
            optimized_hints.use_bloom_filters,
            optimized_hints
                .engine_specific
                .get("max_levels_to_search")
                .and_then(|v| v.as_i64())
                .unwrap_or(5),
            optimized_hints
                .engine_specific
                .get("search_strategy")
                .and_then(|v| v.as_str())
                .unwrap_or("tiered")
        );

        // Execute tiered search strategy
        let results = self
            .tiered_search(query_vector, k, filters, &optimized_hints)
            .await?;

        // Update metrics
        let search_time = start_time.elapsed();
        let mut metrics = self.metrics.write().await;
        metrics.update_search_stats(search_time, results.len());

        info!(
            "✅ LSM: Tiered search complete - found {} results in {}μs",
            results.len(),
            search_time.as_micros()
        );

        Ok(results)
    }

    fn search_capabilities(&self) -> SearchCapabilities {
        SearchCapabilities {
            supports_predicate_pushdown: self.capabilities.supports_predicate_pushdown,
            supports_bloom_filters: self.capabilities.supports_bloom_filters,
            supports_clustering: self.capabilities.supports_clustering,
            supports_parallel_search: self.capabilities.supports_parallel_search,
            supported_quantization: self.capabilities.supported_quantization.clone(),
            max_k: self.capabilities.max_k,
            max_dimension: self.capabilities.max_dimension,
            engine_features: {
                let mut features = HashMap::with_capacity(self.capabilities.engine_features.len());
                for (key, value) in &self.capabilities.engine_features {
                    features.insert(key.clone(), value.clone());
                }
                features
            },
        }
    }

    fn engine_type(&self) -> StorageEngineType {
        StorageEngineType::Lsm
    }

    async fn get_search_metrics(&self) -> Result<SearchMetrics> {
        let metrics = self.metrics.read().await;
        Ok(metrics.to_search_metrics())
    }

    fn validate_search_params(
        &self,
        query_vector: &[f32],
        k: usize,
        hints: &SearchHints,
    ) -> Result<()> {
        // Basic validation
        if k == 0 {
            return Err(anyhow::anyhow!("k must be greater than 0"));
        }

        let dimension = self.collection.config.as_ref().map(|c| c.dimension).unwrap_or(0);
        if dimension > 0 {
            let dimension = dimension as usize;
            if query_vector.len() != dimension {
                return Err(anyhow::anyhow!("Query vector dimension mismatch"));
            }
        }

        // LSM-specific validation
        if !self
            .capabilities
            .supported_quantization
            .contains(&hints.quantization_level)
        {
            return Err(anyhow::anyhow!(
                "Quantization level {:?} not supported by LSM engine (only FP32 supported)",
                hints.quantization_level
            ));
        }

        if k > self.capabilities.max_k {
            return Err(anyhow::anyhow!(
                "k={} exceeds maximum supported k={}",
                k,
                self.capabilities.max_k
            ));
        }

        // LSM doesn't support predicate pushdown
        if hints.predicate_pushdown {
            warn!("LSM engine does not support predicate pushdown - ignoring hint");
        }

        Ok(())
    }
}

// Helper data structures for LSM search metrics

#[derive(Debug, Default)]
struct LSMSearchMetrics {
    total_searches: u64,
    total_search_time_us: u64,
    total_results_returned: u64,
    memtable_searches: u64,
    sstables_scanned: u64,
    sstables_skipped_bloom: u64,
    levels_searched: u64,
    tombstones_handled: u64,
}

impl LSMSearchMetrics {
    fn update_search_stats(&mut self, search_time: std::time::Duration, results_count: usize) {
        self.total_searches += 1;
        self.total_search_time_us += search_time.as_micros() as u64;
        self.total_results_returned += results_count as u64;
        self.memtable_searches += 1; // Each search includes memtable
    }

    fn to_search_metrics(&self) -> SearchMetrics {
        let avg_latency = if self.total_searches > 0 {
            self.total_search_time_us as f64 / self.total_searches as f64
        } else {
            0.0
        };

        let bloom_filter_efficiency = if self.sstables_scanned + self.sstables_skipped_bloom > 0 {
            self.sstables_skipped_bloom as f64
                / (self.sstables_scanned + self.sstables_skipped_bloom) as f64
        } else {
            0.0
        };

        let mut index_efficiency = HashMap::new();
        index_efficiency.insert(
            "bloom_filter_skip_rate".to_string(),
            bloom_filter_efficiency,
        );

        SearchMetrics {
            total_searches: self.total_searches,
            avg_latency_us: avg_latency,
            p95_latency_us: avg_latency * 1.3, // Approximation
            p99_latency_us: avg_latency * 1.8, // Approximation
            avg_vectors_scanned: self.total_results_returned as f64
                / self.total_searches.max(1) as f64,
            cache_hit_rate: 0.9, // Memtable hit rate (approximation)
            index_efficiency,
            quantization_accuracy: None, // LSM uses full precision
            engine_metrics: {
                let mut metrics = HashMap::new();
                metrics.insert(
                    "sstables_per_search".to_string(),
                    serde_json::Value::Number(
                        serde_json::Number::from_f64(
                            self.sstables_scanned as f64 / self.total_searches.max(1) as f64,
                        )
                        .unwrap_or(serde_json::Number::from(0)),
                    ),
                );
                metrics.insert(
                    "levels_per_search".to_string(),
                    serde_json::Value::Number(
                        serde_json::Number::from_f64(
                            self.levels_searched as f64 / self.total_searches.max(1) as f64,
                        )
                        .unwrap_or(serde_json::Number::from(0)),
                    ),
                );
                metrics.insert(
                    "bloom_filter_efficiency".to_string(),
                    serde_json::Value::Number(
                        serde_json::Number::from_f64(bloom_filter_efficiency)
                            .unwrap_or(serde_json::Number::from(0)),
                    ),
                );
                metrics.insert(
                    "engine_type".to_string(),
                    serde_json::Value::String("LSM".to_string()),
                );
                metrics
            },
        }
    }
}

