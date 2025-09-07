//! HELIX Storage Engine - High-Efficiency Locality-Indexed eXecution
//!
//! A disk-only LSM engine that uses PCA + Hilbert curve clustering to physically
//! co-locate similar vectors on disk for efficient pruning during queries.
//!
//! ## Key Features
//! - Disk-only LSM (no memtable/WAL - uses global infrastructure)
//! - PCA dimensionality reduction for clustering
//! - Hilbert curve mapping for locality preservation
//! - FastLane columnar blocks for SIMD optimization
//! - Liquid clustering based on query patterns
//! - Aggressive pruning via Hilbert range filtering

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

// Core modules
pub mod clustering;
pub mod compaction;
pub mod eventlog_integration;
pub mod fastlane;
pub mod hilbert_curve;
pub mod liquid_clustering;
pub mod pca_impl;
pub mod pca_manager;
pub mod progressive_search;
pub mod query_optimization;
pub mod readers;
pub mod zone_maps;

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "tests/integration_tests.rs"]
mod integration_tests;

use crate::core::search::InternalSearchResult;
use crate::core::VectorRecord;
use crate::services::EventLog;
use crate::storage::common::compaction_orchestrator::FilenameCodec;
use crate::storage::engines::constants::{ENGINE_HELIX, HELIX_FILE_EXT, HELIX_MAGIC};
use crate::storage::persistence::filesystem::{FileSystem, FilesystemFactory};
use crate::storage::traits::{
    CompactionParameters, CompactionResult, FlushParameters, FlushResult, 
    StorageEngineStrategy, StorageQueryContext, UnifiedStorageEngine,
};

use self::clustering::{HilbertKey, PCAModel};
use self::compaction::LeveledCompactor;
use self::query_optimization::QueryOptimizer;
use crate::storage::engines::core::formats::fastlanes_blocks::block_structures::FastLanesBlockMetadata;

/// HELIX engine configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelixConfig {
    /// Number of L0 files to trigger compaction
    pub level0_file_num_compaction_trigger: usize,
    /// Maximum number of LSM levels
    pub max_levels: usize,
    /// Size ratio between levels
    pub size_ratio: f64,
    /// PCA dimensions for clustering
    pub pca_dimensions: usize,
    /// FastLane block size (number of vectors per block)
    pub fastlane_block_size: usize,
    /// Enable liquid clustering
    pub enable_liquid_clustering: bool,
    /// Storage quantization settings
    pub storage_quantization: bool,
    /// Bloom filter bits per key
    pub bloom_filter_bits_per_key: u32,
    /// Block cache size in MB
    pub block_cache_size_mb: usize,
    /// PCA model retrain interval in hours
    pub pca_retrain_interval_hours: u64,
    /// Hilbert curve bits per dimension (resolution)
    pub hilbert_bits_per_dimension: usize,
    
    /// Parallel search configuration
    /// Enable parallel search for multiple SSTables
    pub parallel_search_enabled: bool,
    /// Minimum number of files to trigger parallel search (default: 3)
    pub parallel_search_threshold: usize,
    /// Maximum concurrent search threads (default: CPU cores / 2)
    pub max_search_threads: usize,
}

impl Default for HelixConfig {
    fn default() -> Self {
        Self {
            level0_file_num_compaction_trigger: 4,
            max_levels: 7,
            size_ratio: 10.0,
            pca_dimensions: 16,
            fastlane_block_size: 128,
            enable_liquid_clustering: true,
            storage_quantization: false,
            bloom_filter_bits_per_key: 10,
            block_cache_size_mb: 1024,
            pca_retrain_interval_hours: 24,
            hilbert_bits_per_dimension: 16,  // Smart default for good resolution
            parallel_search_enabled: true,  // Enable parallel search by default
            parallel_search_threshold: 3,   // Use parallel for 3+ files
            max_search_threads: num_cpus::get().max(2) / 2,  // Half of CPU cores, min 1
        }
    }
}

/// Metadata for a HELIX SSTable file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SStableMetadata {
    /// File path
    pub path: PathBuf,
    /// LSM level (0 = unsorted flush files)
    pub level: usize,
    /// Hilbert key range [min, max]
    pub hilbert_range: Option<(HilbertKey, HilbertKey)>,
    /// Number of vectors
    pub num_vectors: usize,
    /// File size in bytes
    pub size_bytes: u64,
    /// Creation timestamp
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// FastLane block metadata
    pub blocks: Vec<FastLanesBlockMetadata>,
    /// Bloom filter (serialized)
    pub bloom_filter: Option<Vec<u8>>,
}

/// Main HELIX storage engine implementation
pub struct HelixEngine {
    /// Engine configuration
    config: HelixConfig,
    /// Collection ID
    collection_id: String,
    /// Data directory path
    data_dir: PathBuf,
    /// Filesystem abstraction
    filesystem: Arc<dyn FileSystem>,
    /// Filesystem factory
    filesystem_factory: Arc<FilesystemFactory>,
    /// Unified distance computation engine
    distance_compute: Arc<crate::compute::distance_computation::engine::UnifiedDistanceCompute>,
    /// Unified quantization engine for storage
    quantization_engine: Option<Arc<crate::compute::quantization::storage_engine::StorageQuantizationEngine>>,
    /// Unified cache orchestrator
    cache_orchestrator: Option<Arc<crate::storage::cache::orchestrator::CrossCacheOrchestrator>>,
    /// PCA model for clustering
    pca_model: Arc<RwLock<Option<PCAModel>>>,
    /// Level metadata (level -> list of SSTables)
    levels: Arc<RwLock<HashMap<usize, Vec<SStableMetadata>>>>,
    /// Compactor for background compaction
    compactor: Arc<LeveledCompactor>,
    /// Query optimizer (prefetching + caching)
    query_optimizer: Arc<QueryOptimizer>,
    /// EventLog for AXIS integration
    event_log: Option<Arc<EventLog>>,
    /// Filename codec for consistent naming
    filename_codec: FilenameCodec,
    /// Metrics collector
    metrics: Arc<RwLock<EngineMetrics>>,
}

/// Engine metrics for monitoring
#[derive(Debug, Default, Clone)]
struct EngineMetrics {
    pub total_vectors: u64,
    pub total_sstables: usize,
    pub total_size_bytes: u64,
    pub compaction_count: u64,
    pub query_count: u64,
    pub pruning_ratio_sum: f64,
    pub pca_model_version: u32,
}

impl HelixEngine {
    /// Create a new HELIX engine instance
    pub async fn new(
        collection_id: String,
        config: HelixConfig,
        data_dir: PathBuf,
        event_log: Option<Arc<EventLog>>,
    ) -> Result<Self> {
        // Create filesystem factory
        let filesystem_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
        let filesystem_factory = Arc::new(
            FilesystemFactory::new(filesystem_config).await?
        );
        
        // Get the local filesystem
        let filesystem = filesystem_factory.get_filesystem("file://")?;
        
        // Create data directory if it doesn't exist
        filesystem.create_dir_all(data_dir.to_str().unwrap_or("/tmp/helix")).await?;

        // Initialize unified components (similar to SST)
        let distance_compute = Arc::new(
            crate::compute::distance_computation::engine::UnifiedDistanceCompute::default()
        );
        
        // Initialize quantization engine if enabled
        let quantization_engine = if config.storage_quantization {
            let codebook_store = Arc::new(
                crate::compute::quantization::unified::InMemoryCodebookStore::new()
            );
            let unified_engine = Arc::new(
                crate::compute::quantization::unified::UnifiedQuantizationEngine::new(
                    distance_compute.clone(),
                    codebook_store,
                )
            );
            let storage_config = crate::compute::quantization::storage_engine::StorageQuantizationConfig::default();
            Some(Arc::new(
                crate::compute::quantization::storage_engine::StorageQuantizationEngine::new(
                    unified_engine,
                    distance_compute.clone(),
                    storage_config,
                )
            ))
        } else {
            None
        };
        
        // Initialize cache orchestrator
        let cache_orchestrator = if config.block_cache_size_mb > 0 {
            Some(Arc::new(
                crate::storage::cache::orchestrator::CrossCacheOrchestrator::new(
                    config.block_cache_size_mb * 1024 * 1024  // Convert MB to bytes
                )
            ))
        } else {
            None
        };

        // Initialize levels from existing files
        let levels = Self::load_levels(&filesystem, &data_dir).await?;
        
        // Create compactor
        let compactor = Arc::new(LeveledCompactor::new(
            config.clone(),
            filesystem.clone(),
            data_dir.clone(),
        ));
        
        // Create query optimizer
        let query_optimizer = Arc::new(QueryOptimizer::new(
            1000,  // Max query history
            500,   // Cache capacity
            300,   // Cache TTL (5 minutes)
        ));

        // Create engine instance
        let engine = Self {
            config,
            collection_id,
            data_dir: data_dir.clone(),
            filesystem,
            filesystem_factory,
            distance_compute,
            quantization_engine,
            cache_orchestrator,
            pca_model: Arc::new(RwLock::new(None)),
            levels: Arc::new(RwLock::new(levels)),
            compactor,
            query_optimizer,
            event_log,
            filename_codec: FilenameCodec::new(),
            metrics: Arc::new(RwLock::new(EngineMetrics::default())),
        };
        
        // Simple initialization - just load existing PCA model if present
        if let Ok(pca_model_bytes) = engine.filesystem.read(&engine.data_dir.join("pca_model.bin").to_string_lossy()).await {
            if let Ok(model) = bincode::deserialize::<PCAModel>(&pca_model_bytes) {
                *engine.pca_model.write().await = Some(model);
                info!("Loaded existing PCA model for HELIX engine");
            }
        }
        
        Ok(engine)
    }

    /// Load existing SSTable levels from disk
    async fn load_levels(
        filesystem: &Arc<dyn FileSystem>,
        data_dir: &Path,
    ) -> Result<HashMap<usize, Vec<SStableMetadata>>> {
        let mut levels = HashMap::new();
        
        // List all files in directory
        let files = filesystem.list(data_dir.to_str().unwrap_or("/tmp/helix")).await?;
        
        for file in files {
            let file_name = &file.name;
            if file_name.ends_with(HELIX_FILE_EXT) {
                // Parse level from filename
                let codec = FilenameCodec::new();
                let level = codec.parse_level(file_name) as usize;
                
                // Load metadata (would read from file footer in production)
                let metadata = SStableMetadata {
                    path: PathBuf::from(&file.url),
                    level,
                    hilbert_range: None, // Would load from file
                    num_vectors: 0, // Would load from file
                    size_bytes: file.metadata.size,
                    created_at: chrono::Utc::now(),
                    blocks: Vec::new(),
                    bloom_filter: None,
                };
                
                levels.entry(level).or_insert_with(Vec::new).push(metadata);
            }
        }
        
        Ok(levels)
    }

    /// Generate a new SSTable filename for the given level
    fn generate_sstable_filename(&self, level: usize) -> String {
        self.filename_codec.generate(level as u32, "helix")
    }

    /// Check if compaction is needed
    async fn should_compact(&self) -> bool {
        let levels = self.levels.read().await;
        
        // Check L0 trigger
        if let Some(l0_files) = levels.get(&0) {
            if l0_files.len() >= self.config.level0_file_num_compaction_trigger {
                return true;
            }
        }
        
        // Check size ratio triggers for other levels
        for level in 1..self.config.max_levels {
            if let (Some(curr_level), Some(next_level)) = 
                (levels.get(&level), levels.get(&(level + 1))) {
                
                let curr_size: u64 = curr_level.iter().map(|f| f.size_bytes).sum();
                let next_size: u64 = next_level.iter().map(|f| f.size_bytes).sum();
                
                if next_size > 0 && (curr_size as f64 / next_size as f64) > self.config.size_ratio {
                    return true;
                }
            }
        }
        
        false
    }

    /// Update PCA model based on current data distribution
    async fn update_pca_model(&self, vectors: &[VectorRecord]) -> Result<()> {
        if vectors.is_empty() {
            return Ok(());
        }

        let new_model = PCAModel::train(vectors, self.config.pca_dimensions)?;
        *self.pca_model.write().await = Some(new_model);
        
        // Update metrics
        self.metrics.write().await.pca_model_version += 1;
        
        Ok(())
    }
}

#[async_trait]
impl UnifiedStorageEngine for HelixEngine {
    fn engine_name(&self) -> &'static str {
        ENGINE_HELIX
    }

    fn engine_version(&self) -> &'static str {
        "1.0.0"
    }

    fn strategy(&self) -> StorageEngineStrategy {
        StorageEngineStrategy::Helix
    }

    async fn do_flush(&self, params: &FlushParameters) -> Result<FlushResult> {
        info!("HELIX flush started for collection {}", self.collection_id);
        
        let records = params.vector_records.clone();
        let num_records = records.len();
        
        if records.is_empty() {
            return Ok(FlushResult {
                success: true,
                collections_affected: vec![self.collection_id.clone()],
                entries_flushed: Some(0),
                bytes_written: Some(0),
                files_created: Some(0),
                duration_ms: Some(0),
                completed_at: chrono::Utc::now(),
                engine_metrics: HashMap::new(),
                compaction_triggered: false,
                flushed_batch_ids: Vec::new(),
            });
        }

        let start = std::time::Instant::now();
        
        // SORTED FLUSH OPTIMIZATION: Sort by Hilbert key during L0 flush
        // This enables immediate query pruning even before compaction
        
        // Step 1: Ensure PCA model is trained
        let pca_model = {
            let model_guard = self.pca_model.read().await;
            if model_guard.is_none() {
                drop(model_guard);
                // Train initial PCA model
                self.update_pca_model(&records).await?;
                self.pca_model.read().await.clone()
            } else {
                model_guard.clone()
            }
        };
        
        // Step 2: Compute Hilbert keys for all records
        let mut hilbert_keys = Vec::with_capacity(records.len());
        if let Some(ref model) = pca_model {
            for record in &records {
                let reduced = model.transform(&record.vector)?;
                let hilbert_key = clustering::compute_hilbert_key_with_config(&reduced, self.config.hilbert_bits_per_dimension);
                hilbert_keys.push(hilbert_key);
            }
        } else {
            // Fallback: use hash-based keys if no PCA model
            for record in &records {
                // Simple hash-based key as fallback
                let hilbert_key = {
                    let mut hash = 0u64;
                    for byte in record.id.bytes() {
                        hash = hash.wrapping_mul(31).wrapping_add(byte as u64);
                    }
                    hash
                };
                hilbert_keys.push(hilbert_key);
            }
        }
        
        // Step 3: Sort records by Hilbert key
        let mut indexed_records: Vec<(u64, VectorRecord)> = hilbert_keys
            .into_iter()
            .zip(records.into_iter())
            .collect();
        indexed_records.sort_by_key(|(key, _)| *key);
        
        // Extract sorted records and compute Hilbert range
        let sorted_records: Vec<VectorRecord> = indexed_records
            .iter()
            .map(|(_, record)| record.clone())
            .collect();
        
        let hilbert_range = if !indexed_records.is_empty() {
            Some((indexed_records.first().unwrap().0, indexed_records.last().unwrap().0))
        } else {
            None
        };
        
        // Create Level-0 SSTable (now sorted by Hilbert key)
        let filename = self.generate_sstable_filename(0);
        let file_path = self.data_dir.join(&filename);
        
        // Write FastLane blocks with Hilbert keys
        let hilbert_keys_for_write: Vec<u64> = indexed_records.iter().map(|(k, _)| *k).collect();
        let bytes_written = fastlane::write_helix_sstable(
            &self.filesystem,
            &file_path,
            &sorted_records,
            self.config.fastlane_block_size,
            HELIX_MAGIC,
            Some(&hilbert_keys_for_write),
        ).await?;
        
        // Update level metadata with Hilbert range
        {
            let mut levels = self.levels.write().await;
            let metadata = SStableMetadata {
                path: file_path.clone(),
                level: 0,
                hilbert_range, // Now L0 files have Hilbert ranges!
                num_vectors: num_records,
                size_bytes: bytes_written,
                created_at: chrono::Utc::now(),
                blocks: fastlane::extract_helix_metadata(&sorted_records, self.config.fastlane_block_size, Some(&hilbert_keys_for_write))
                    .into_iter()
                    .map(|h| h.fastlanes_metadata)
                    .collect(),
                bloom_filter: None,
            };
            levels.entry(0).or_insert_with(Vec::new).push(metadata);
        }
        
        // Notify EventLog for AXIS indexing
        let flush_handler = eventlog_integration::HelixFlushHandler::new();
        flush_handler.notify_flush_complete(
            params,
            vec![file_path.to_string_lossy().to_string()],
            &sorted_records,
            hilbert_range,
        ).await?;
        
        // Update metrics
        {
            let mut metrics = self.metrics.write().await;
            metrics.total_vectors += num_records as u64;
            metrics.total_sstables += 1;
            metrics.total_size_bytes += bytes_written;
        }
        
        // Trigger compaction if needed
        if self.should_compact().await {
            let compactor = self.compactor.clone();
            let levels = self.levels.clone();
            tokio::spawn(async move {
                if let Err(e) = compactor.compact_l0_to_l1(levels).await {
                    warn!("Background compaction failed: {}", e);
                }
            });
        }
        
        Ok(FlushResult {
            success: true,
            collections_affected: vec![self.collection_id.clone()],
            entries_flushed: Some(num_records as u64),
            bytes_written: Some(bytes_written),
            files_created: Some(1),
            duration_ms: Some(start.elapsed().as_millis() as u64),
            completed_at: chrono::Utc::now(),
            engine_metrics: HashMap::new(),
            compaction_triggered: false,
            flushed_batch_ids: Vec::new(),
        })
    }

    async fn do_compact(&self, params: &CompactionParameters) -> Result<CompactionResult> {
        info!("HELIX compaction started for collection {}", self.collection_id);
        
        let start = std::time::Instant::now();
        
        // Determine which level to compact (default to L0)
        let level_to_compact = 0; // TODO: Use hints from params if available
        
        // Track files being compacted for cache invalidation
        let files_to_invalidate = {
            let levels = self.levels.read().await;
            levels.get(&level_to_compact)
                .map(|files| files.iter()
                    .map(|f| f.path.to_string_lossy().to_string())
                    .collect::<Vec<_>>())
                .unwrap_or_default()
        };
        
        // Perform compaction based on level
        let (files_compacted, bytes_written) = if level_to_compact == 0 {
            // L0 to L1: Initial clustering with PCA + Hilbert
            self.compactor.compact_l0_to_l1(self.levels.clone()).await?
        } else {
            // Li to Li+1: Progressive refinement with liquid clustering
            self.compactor.compact_level_to_next(
                self.levels.clone(),
                level_to_compact,
                self.pca_model.clone(),
            ).await?
        };
        
        // Invalidate cache for compacted files
        if !files_to_invalidate.is_empty() {
            self.query_optimizer.invalidate_files(&files_to_invalidate).await;
            debug!("Invalidated cache for {} compacted files", files_to_invalidate.len());
        }
        
        // Update metrics
        {
            let mut metrics = self.metrics.write().await;
            metrics.compaction_count += 1;
        }
        
        Ok(CompactionResult {
            success: true,
            collections_affected: vec![self.collection_id.clone()],
            entries_processed: Some(0), // TODO: Track actual entries
            entries_removed: Some(0),
            bytes_read: Some(bytes_written), // Simplified
            bytes_written: Some(bytes_written),
            input_files: Some(files_compacted as u64),
            output_files: Some(1), // TODO: Track actual output files
            duration_ms: Some(start.elapsed().as_millis() as u64),
            completed_at: chrono::Utc::now(),
            engine_metrics: HashMap::new(),
        })
    }

    async fn search_vectors_unified(
        &self,
        ctx: &StorageQueryContext,
    ) -> Result<Vec<InternalSearchResult>> {
        let k = ctx.top_k();
        let distance_metric = ctx.distance_metric();
        debug!("HELIX search started with k={}", k);
        
        let start = std::time::Instant::now();
        let query_vector = ctx.query_vector()
            .ok_or_else(|| anyhow::anyhow!("No query vector provided"))?;
        
        // Calculate query hash for caching
        let query_hash = {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut hasher = DefaultHasher::new();
            query_vector.iter().for_each(|v| v.to_bits().hash(&mut hasher));
            k.hash(&mut hasher);
            hasher.finish()
        };
        
        // Get PCA model
        let pca_model = self.pca_model.read().await;
        
        // Calculate query Hilbert key if PCA model is available using configured bits
        let query_hilbert = if let Some(model) = pca_model.as_ref() {
            Some(model.project_and_compute_hilbert_with_config(
                query_vector, 
                self.config.hilbert_bits_per_dimension
            )?)
        } else {
            None
        };
        
        // Get optimization hints (cache check + prefetching)
        let hints = self.query_optimizer.optimize_query(query_hash, query_hilbert).await;
        
        // Check cache first
        if let Some(cached_results) = hints.cached_result {
            debug!("Query cache hit, returning cached results");
            return Ok(cached_results);
        }
        
        // Read levels
        let levels = self.levels.read().await;
        
        // Prune and select SSTables to search
        let mut sstables_to_search = Vec::new();
        
        for (_level, sstables) in levels.iter() {
            for sstable in sstables {
                // Pruning logic based on Hilbert range
                if let (Some(query_key), Some((min_key, max_key))) = 
                    (query_hilbert, sstable.hilbert_range) {
                    
                    // Simple range check (could be more sophisticated)
                    let distance_to_range = if query_key < min_key {
                        min_key - query_key
                    } else if query_key > max_key {
                        query_key - max_key
                    } else {
                        0 // Query is within range
                    };
                    
                    // Skip if too far from range
                    if distance_to_range > 1000 { // Threshold
                        continue;
                    }
                }
                
                sstables_to_search.push(sstable.clone());
            }
        }
        
        // Update pruning metrics
        let pruning_ratio = 1.0 - (sstables_to_search.len() as f64 / 
            levels.values().map(|v| v.len()).sum::<usize>().max(1) as f64);
        {
            let mut metrics = self.metrics.write().await;
            metrics.query_count += 1;
            metrics.pruning_ratio_sum += pruning_ratio;
        }
        
        info!("HELIX pruned {:.1}% of SSTables", pruning_ratio * 100.0);
        
        // Create thread-safe filter using unified evaluator
        let filter_fn = crate::storage::engines::core::create_filter_fn(
            ctx.search_params.filter_expression.as_ref()
        );
        
        // Decide whether to use parallel or sequential search based on config
        let use_parallel = self.config.parallel_search_enabled 
            && sstables_to_search.len() >= self.config.parallel_search_threshold;
        
        let (results, accessed_files) = if use_parallel {
            info!("Using parallel search for {} SSTables", sstables_to_search.len());
            
            // Collect file paths before moving sstables
            let files: Vec<String> = sstables_to_search
                .iter()
                .map(|s| s.path.to_string_lossy().to_string())
                .collect();
            
            // Use the Vec directly for parallel search
            let sstables_vec = sstables_to_search;
            
            // Use parallel search for better performance
            let results = readers::parallel_search(
                self.filesystem.clone(),
                sstables_vec,
                query_vector.to_vec(),
                k,
                distance_metric,
                filter_fn,
            ).await?;
            
            (results, files)
        } else {
            info!("Using sequential search for {} SSTables", sstables_to_search.len());
            
            // Sequential search for small number of files
            let mut results = Vec::new();
            let mut accessed_files = Vec::new();
            
            for sstable in sstables_to_search {
                accessed_files.push(sstable.path.to_string_lossy().to_string());
                
                let sstable_results = readers::search_sstable(
                    &self.filesystem,
                    &sstable,
                    query_vector,
                    k,
                    &distance_metric,
                    filter_fn.clone(),
                    None, // No specific IDs to check
                ).await?;
                
                results.extend(sstable_results);
            }
            
            // Sort by score (higher is better) and take top-k
            results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
            results.truncate(k);
            
            (results, accessed_files)
        };
        
        // Record query execution for learning
        let latency_ms = start.elapsed().as_millis() as u64;
        self.query_optimizer.record_execution(
            query_hash,
            query_hilbert,
            results.clone(),
            accessed_files,
            latency_ms,
        ).await;
        
        Ok(results)
    }

    async fn vector_by_id(
        &self,
        collection_id: &str,
        vector_id: &str,
    ) -> Result<Option<VectorRecord>> {
        if collection_id != self.collection_id {
            return Ok(None);
        }
        
        let levels = self.levels.read().await;
        
        // Search all SSTables for the vector
        for (_level, sstables) in levels.iter() {
            for sstable in sstables {
                if let Some(vector) = readers::find_vector_by_id(
                    &self.filesystem,
                    &sstable,
                    vector_id,
                ).await? {
                    return Ok(Some(vector));
                }
            }
        }
        
        Ok(None)
    }

    async fn collect_engine_metrics(&self) -> Result<HashMap<String, serde_json::Value>> {
        let metrics = self.metrics.read().await;
        let mut map = HashMap::new();
        
        map.insert("total_vectors".to_string(), 
            serde_json::json!(metrics.total_vectors));
        map.insert("total_sstables".to_string(), 
            serde_json::json!(metrics.total_sstables));
        map.insert("total_size_bytes".to_string(), 
            serde_json::json!(metrics.total_size_bytes));
        map.insert("compaction_count".to_string(), 
            serde_json::json!(metrics.compaction_count));
        map.insert("query_count".to_string(), 
            serde_json::json!(metrics.query_count));
        
        if metrics.query_count > 0 {
            let avg_pruning = metrics.pruning_ratio_sum / metrics.query_count as f64;
            map.insert("avg_pruning_ratio".to_string(), 
                serde_json::json!(avg_pruning));
        }
        
        map.insert("pca_model_version".to_string(), 
            serde_json::json!(metrics.pca_model_version));
        
        Ok(map)
    }
    
    fn get_filesystem_factory(&self) -> &FilesystemFactory {
        &self.filesystem_factory
    }
}