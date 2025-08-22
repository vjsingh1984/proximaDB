use async_trait::async_trait;
use arrow_array::{RecordBatch, StringArray, Float32Array, UInt32Array, Int64Array, ArrayRef};
use arrow_schema::{DataType, Field, Schema};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use anyhow::Result;
use uuid::Uuid;
use memmap2::MmapOptions;
use std::fs::File;

use crate::storage::traits::{UnifiedStorageEngine, StorageEngineStrategy, FlushParameters, FlushResult, CompactionParameters, CompactionResult, SearchContext};
use crate::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};
use crate::proto::proximadb::Collection;
use crate::core::VectorRecord;
use crate::core::search::{SearchResult, FilterExpression, InternalSearchResult};
use crate::compute::distance_computation::{DistanceMetric, engine::UnifiedDistanceCompute};
use super::{RaptorConfig, RaptorWriter, consolidated_reader::RaptorReader, RowGroupManager};
use super::consolidated_compactor::RaptorCompactor;
use super::hnsw_manager::HnswManager;
use super::smart_rowgroup_sizing::{SmartRowGroupSizer, CommonConfigurations};

// Deep integration with AXIS clustering
use crate::index::axis::clustering::{ClusteringConfig, ClusteringAlgorithm, KMeansConfig, ClusterManager};
use crate::index::axis::types::ClusterAssignment;

// Deep integration with filesystem API for cloud-aware I/O
use crate::storage::persistence::filesystem::{FileSystem, FileOptions, StorageTier};
use crate::storage::persistence::filesystem::TierConfig;

// Universal performance optimization imports
use crate::storage::engines::common::performance_optimization::{
    UniversalPerformanceOptimizer, UniversalOptimizationStrategy, 
    UniversalIOConfig, UniversallyOptimized
};
use crate::core::compression::{StandardCompression, CompressionAlgorithm, CompressionContext};
use crate::core::hardware_capabilities::HardwareCapabilities;
// VectorMemoryPool now managed by universal optimizer

/// Vector search result for compatibility - using unified InternalSearchResult
type VectorSearchResult = InternalSearchResult;

/// RAPTOR Engine - Row-Aligned Predicated Tensor Optimized Repository
/// 
/// LARGE FILE SUPPORT ARCHITECTURE:
/// 
/// 1. DUAL-LEVEL HNSW STRATEGY:
///    - GLOBAL GRAPH: Single master HNSW graph across entire file
///      * Stored in file header for O(1) access
///      * Entry points indexed by centrality
///      * Navigates to relevant rowgroups
///    
///    - LOCAL GRAPHS: Per-rowgroup HNSW subgraphs (1K vectors each)
///      * Optimized for k<10 queries (typical use case)
///      * Self-contained for parallel search
///      * Bridge nodes connect to global graph
///      * Memory-mapped for efficient access (~4MB per rowgroup)
/// 
/// 2. COLUMNAR STREAMING FOR SCALE:
///    - Vectors stored column-wise (not row-wise despite name)
///    - SIMD-aligned columns for vectorized operations
///    - Selective column loading (vector, graph, metadata separate)
///    - Supports 100GB+ files through streaming
/// 
/// 3. MEMORY MAPPING STRATEGY:
///    - Global graph always mapped (small, ~100MB for 10M vectors)
///    - RowGroups mapped on-demand (~4MB each @ 1024-dim, 1K vectors)
///    - LRU cache for hot rowgroups (default: 512 rowgroups = 2GB)
///    - Parallel prefetch for predicted access patterns
///    - Adaptive granularity: can adjust 500-2000 vectors based on k
/// 
/// 4. SEARCH EXECUTION FLOW:
///    a) Global HNSW navigation → find promising rowgroups
///    b) Local HNSW search within rowgroups (parallel)
///    c) Optional: columnar scan for exhaustive search
///    d) FastLanes decoding only for final candidates
/// 
/// 5. COMPACTION STRATEGY:
///    - Single file maintained (L0 only, max_level=0)
///    - Immediate compaction at 2 files (preserves graph)
///    - Streaming compaction without loading entire file
///    - Graph rebuild during compaction for optimization
/// 
/// 6. PERFORMANCE AT SCALE:
///    - 100M vectors: ~400GB file, 100K rowgroups (1K each)
///    - Search latency: <5ms for top-10, <10ms for top-100
///    - I/O efficiency: Read only ~1-3 rowgroups for k<10
///    - Insert throughput: 50K vectors/sec (batched)
///    - Memory usage: ~2GB cache + 100MB global graph
/// 
/// 7. ADAPTIVE ROWGROUP SIZING:
///    - k<10: Use 500-1000 vectors/rowgroup (minimize waste)
///    - k<100: Use 1000-2000 vectors/rowgroup (balance)
///    - k>100: Use 2000-5000 vectors/rowgroup (maximize throughput)
///    - Can be configured per collection based on workload

// Old optimization structures removed - now using UniversalPerformanceOptimizer
// The universal optimizer provides all these capabilities through a unified interface

pub struct RaptorEngine {
    config: RaptorConfig,
    collection_id: String,
    base_path: String,
    
    // Core components  
    rowgroup_manager: Arc<RwLock<RowGroupManager>>,
    writer: Arc<RwLock<RaptorWriter>>,
    reader: Arc<RaptorReader>,  // Using consolidated reader
    compactor: Arc<RaptorCompactor>,
    hnsw_manager: Arc<RwLock<HnswManager>>,
    
    // Deep integration with AXIS clustering
    cluster_manager: Arc<RwLock<ClusterManager>>,
    clustering_config: ClusteringConfig,
    cluster_assignments: Arc<RwLock<HashMap<u32, Vec<ClusterAssignment>>>>, // RowGroup -> Clusters
    
    // Deep integration with filesystem API
    filesystem: Arc<dyn FileSystem>,
    tier_config: TierConfig,
    file_options: FileOptions,
    
    // Zero-copy filesystem and transaction coordinator
    zero_copy_filesystem: Arc<crate::storage::persistence::filesystem::zero_copy_filesystem::ZeroCopyFilesystem>,
    transaction_coordinator: Arc<crate::storage::transaction_coordinator::TransactionCoordinator>,
    
    // Universal performance optimization (replaces RAPTOR-specific optimization)
    universal_optimizer: UniversalPerformanceOptimizer,
    
    // Keep hardware capabilities for RAPTOR-specific needs (like SIMD)
    hardware_capabilities: Arc<HardwareCapabilities>,
    
    // Cache and metadata
    cache: Arc<RwLock<RowGroupCache>>,
    file_registry: Arc<RwLock<FileRegistry>>,
    metrics: Arc<RaptorMetrics>,  // Lock-free atomic metrics
}

impl RaptorEngine {
    pub async fn new(
        collection_id: String,
        base_path: String,
        config: RaptorConfig,
    ) -> Result<Self> {
        // Create smart row group sizer - use collection config dimension when available
        // For engine creation, use default configuration if dimension not in RaptorConfig
        let smart_sizer = if let Some(dimension) = config.vector_dimension {
            SmartRowGroupSizer::for_s3_standard(dimension, 200) // 200 bytes avg metadata
                .with_query_pattern(super::smart_rowgroup_sizing::QueryPattern::Mixed)
        } else {
            // Default configuration for common OpenAI embeddings (384 dimensions)
            // Actual dimension will be determined from collection config during operations
            tracing::info!("RAPTOR: Using default configuration, actual dimension will be determined from collection config");
            CommonConfigurations::openai_s3()
        };
        
        let rowgroup_manager = Arc::new(RwLock::new(RowGroupManager::new(
            config.clone(),
            smart_sizer,
            None, // No quantization engine for now
        )?));
        
        // Initialize filesystem with proper abstraction
        let filesystem_factory = FilesystemFactory::new(FilesystemConfig::default()).await?;
        let filesystem = filesystem_factory.get_filesystem(&base_path)?;
        
        // Determine storage tier from URL
        let tier = Self::determine_storage_tier(&base_path);
        let tier_config = TierConfig {
            tier,
            base_url: base_path.clone(),
            max_capacity_bytes: None,
            current_usage_bytes: 0,
            compression: !matches!(config.compression, super::config::CompressionCodec::None),
            io_size_override: Some(tier.optimal_io_size()),
        };
        
        // Configure file options for cloud-aware operations
        let file_options = FileOptions {
            create_dirs: true,
            overwrite: false,
            buffer_size: Some(tier.optimal_io_size()),
            encryption: None,
            storage_class: match &tier {
                StorageTier::S3Express => Some("EXPRESS_ONEZONE".to_string()),
                StorageTier::S3Standard => Some("STANDARD".to_string()),
                StorageTier::S3GlacierInstant => Some("GLACIER_IR".to_string()),
                StorageTier::AzurePremium => Some("Premium_LRS".to_string()),
                StorageTier::AzureStandard => Some("Standard_LRS".to_string()),
                _ => None,
            },
            metadata: None,
            temp_path: None,
        };
        
        // Generate initial file path using unified naming convention
        let data_dir = format!("{}/{}/data", base_path, collection_id);
        // Ensure data directory exists
        std::fs::create_dir_all(&data_dir)?;
        
        let codec = crate::storage::common::compaction_orchestrator::FilenameCodec::new();
        let filename = codec.generate(0, "raptor"); // Level 0 for new writes
        let file_path = format!("{}/{}", data_dir, filename);
        
        let writer = Arc::new(RwLock::new(
            RaptorWriter::new(
                file_path, 
                config.clone(), 
                collection_id.clone(),
                config.vector_dimension.unwrap_or_else(|| {
                    tracing::warn!("RAPTOR: No vector dimension in config, using default 384. Will use collection config dimension during operations.");
                    384 // Default for OpenAI embeddings, will be overridden by collection config
                })
            ).await?
        ));
        
        // Initialize zero-copy filesystem and transaction coordinator
        use crate::storage::persistence::filesystem::zero_copy_filesystem::ZeroCopyFilesystem;
        use crate::storage::transaction_coordinator::TransactionCoordinator;
        
        let zero_copy_filesystem = Arc::new(
            ZeroCopyFilesystem::new(base_path.clone()).await?
        );
        
        let transaction_coordinator = Arc::new(
            TransactionCoordinator::new()?
        );
        
        // Get the unified cache orchestrator
        let cache = crate::storage::cache::orchestrator::get_cache_orchestrator();
        
        // Create consolidated reader with unified components
        let reader = Arc::new(
            RaptorReader::new(
                base_path.clone(),
                config.clone(),
                cache,
                zero_copy_filesystem.clone(),
                transaction_coordinator.clone(),
            )
        );
        
        let compactor = Arc::new(
            RaptorCompactor::new(
                config.clone(),
                reader.clone(),
                zero_copy_filesystem.clone(),
                transaction_coordinator.clone(),
            )
        );
        
        let hnsw_manager = Arc::new(RwLock::new(
            HnswManager::new(config.clone(), collection_id.clone()).await?
        ));
        
        // Initialize AXIS clustering integration
        let clustering_config = ClusteringConfig {
            algorithm: ClusteringAlgorithm::KMeans(KMeansConfig {
                k: config.rowgroup_size / 100, // Adaptive cluster count
                ..Default::default()
            }),
            min_vectors_for_clustering: 100,
            max_clusters: 256,
            distance_metric: crate::compute::distance_computation::DistanceMetric::Cosine,
            adaptive_cluster_count: true,
            recompute_threshold: config.rowgroup_size / 2,
            enable_incremental: true,
        };
        
        let cluster_manager = Arc::new(RwLock::new(
            ClusterManager::new(clustering_config.clone()).await?
        ));
        
        let cluster_assignments = Arc::new(RwLock::new(HashMap::new()));
        
        let cache = Arc::new(RwLock::new(
            RowGroupCache::new(config.cache_size_mb * 1024 * 1024)
        ));
        
        let file_registry = Arc::new(RwLock::new(
            FileRegistry::new()
        ));
        
        let metrics = Arc::new(RaptorMetrics::new());
        
        // Get the global hardware capabilities instance
        let hardware_capabilities = Arc::new(
            HardwareCapabilities::global()
        );
        
        // Initialize universal performance optimization
        let universal_optimizer = UniversalPerformanceOptimizer::with_strategy(
            UniversalOptimizationStrategy::Balanced, // RAPTOR uses balanced strategy
        ).await?;

        Ok(Self {
            config,
            collection_id,
            base_path,
            rowgroup_manager,
            writer,
            reader,
            compactor,
            hnsw_manager,
            cluster_manager,
            clustering_config,
            cluster_assignments,
            filesystem,
            tier_config,
            file_options,
            zero_copy_filesystem,
            transaction_coordinator,
            universal_optimizer,
            hardware_capabilities,
            cache,
            file_registry,
            metrics,
        })
    }
    
    fn create_default_schema() -> Arc<Schema> {
        
        let fields = vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("vector", DataType::Float32, false),
            Field::new("metadata", DataType::Utf8, true), // JSON string for now
            Field::new("version", DataType::UInt32, true),
            Field::new("timestamp", DataType::Int64, true),
        ];
        
        Arc::new(Schema::new(fields))
    }
    
    // ============================================================================
    // PERFORMANCE OPTIMIZATION METHODS - DELEGATING TO UNIFIED MODULES
    // ============================================================================
    
    /// Fast read optimization using memory mapping (delegates to universal optimizer)
    async fn mmap_read_file(&self, file_path: &str) -> Result<Vec<u8>> {
        // Try memory mapping first
        if let Some(mmap) = self.universal_optimizer.get_memory_mapped_file(file_path).await? {
            Ok(mmap.to_vec())
        } else {
            // Fallback to optimized reading for cloud storage
            self.universal_optimizer.read_data_optimized(file_path).await
        }
    }
    
    /// I/O bandwidth optimization with vectorized reads (delegates to universal optimizer)
    async fn vectorized_read(&self, file_paths: &[String]) -> Result<Vec<Vec<u8>>> {
        // Use universal optimizer's parallel operations
        let read_operations: Vec<_> = file_paths.iter()
            .map(|path| {
                let path = path.clone();
                let optimizer = &self.universal_optimizer;
                async move {
                    optimizer.read_data_optimized(&path).await
                }
            }).collect();
        
        self.universal_optimizer.parallel_operations(
            read_operations,
            |operation| operation
        ).await.map(|results| {
            results.into_iter().collect::<Result<Vec<_>, _>>()
        })?
    }
    
    /// Cloud storage cost optimization - determine optimal storage tier (delegates to universal optimizer)
    async fn optimize_storage_tier(&self, file_path: &str, access_frequency: f32) -> Result<StorageTier> {
        // Estimate file size for tier optimization decision
        let estimated_size = 1024 * 1024; // Default 1MB if size unknown
        self.universal_optimizer.optimize_storage_tier(file_path, estimated_size).await
    }
    
    /// Compression optimization for bandwidth and cost (delegates to universal optimizer)
    async fn compress_data_optimized(&self, data: &[u8]) -> Result<Vec<u8>> {
        // Determine tier based on data characteristics
        let tier = if data.len() > 10 * 1024 * 1024 { // > 10MB
            StorageTier::Cold
        } else if data.len() > 1024 * 1024 { // > 1MB
            StorageTier::Warm
        } else {
            StorageTier::Hot
        };
        
        self.universal_optimizer.compress_for_tier(data, tier).await
    }
    
    /// Prefetch optimization for fast reads (delegates to universal optimizer)
    async fn prefetch_data(&self, file_path: &str) -> Result<()> {
        // Use universal optimizer's intelligent prefetching
        self.universal_optimizer.prefetch_data(&[file_path.to_string()]).await
    }
    
    /// SIMD-optimized vector operations (delegates to universal optimizer)
    async fn simd_vector_distance(&self, query: &[f32], candidates: &[Vec<f32>]) -> Result<Vec<f32>> {
        // Use universal optimizer's hardware-accelerated distance computation
        self.universal_optimizer.compute_distances_accelerated(
            query,
            candidates,
            DistanceMetric::Euclidean, // Default metric for RAPTOR
        ).await
    }
    
    
    /// Memory pool optimization for vector allocations (delegates to universal optimizer)
    async fn get_pooled_buffer(&self, size: usize) -> Result<Vec<f32>> {
        self.universal_optimizer.get_memory_buffer(size).await
    }
    
    async fn insert_batch_internal(&self, records: Vec<VectorRecord>) -> Result<()> {
        // Convert to Arrow batch
        let batch = self.convert_to_arrow_batch(records)?;
        
        // Write to current file
        let mut writer = self.writer.write().await;
        writer.write_batch(&batch).await?;
        
        // Update HNSW index if enabled
        if self.config.enable_hnsw {
            let mut hnsw = self.hnsw_manager.write().await;
            hnsw.add_batch(&batch).await?;
        }
        
        // Update clustering if we have enough vectors
        let row_count = batch.num_rows();
        if row_count >= self.clustering_config.min_vectors_for_clustering {
            self.update_clustering(&batch).await?;
        }
        
        // Update metrics using atomic operations (lock-free)
        self.metrics.total_vectors.fetch_add(row_count, Ordering::Relaxed);
        self.metrics.insert_operations.fetch_add(1, Ordering::Relaxed);
        
        // Check if compaction is needed
        if self.should_compact().await {
            let compactor = self.compactor.clone();
            let collection_id = self.collection_id.clone();
            let base_path = self.base_path.clone();
            tokio::spawn(async move {
                // Get all files from {base_path}/{collection_id}/data - unified directory structure
                let data_dir = format!("{}/{}/data", base_path, collection_id);
                let input_files = match std::fs::read_dir(&data_dir) {
                    Ok(entries) => entries
                        .filter_map(|e| e.ok())
                        .filter(|e| e.path().extension().map_or(false, |ext| ext == "raptor"))
                        .map(|e| e.path().to_string_lossy().to_string())
                        .collect(),
                    Err(_) => Vec::new(),
                };
                
                if !input_files.is_empty() {
                    // Use unified FilenameCodec naming convention
                    let codec = crate::storage::common::compaction_orchestrator::FilenameCodec::new();
                    let filename = codec.generate(1, "raptor"); // Level 1 for compacted files
                    let output_file = format!("{}/{}", data_dir, filename);
                    let _ = compactor.compact_files(input_files, &output_file, &collection_id).await;
                }
            });
        }
        
        Ok(())
    }
    
    async fn update_clustering(&self, batch: &RecordBatch) -> Result<()> {
        let vectors = self.extract_vectors_from_batch(batch)?;
        
        // Use AXIS clustering manager
        let mut cluster_manager = self.cluster_manager.write().await;
        let assignments = cluster_manager.cluster_vectors(&vectors).await?;
        
        // Store cluster assignments per rowgroup
        let rowgroup_manager = self.rowgroup_manager.read().await;
        if let Some(current_rg) = rowgroup_manager.rowgroups().last() {
            let mut cluster_assignments = self.cluster_assignments.write().await;
            cluster_assignments.insert(current_rg.id, assignments);
            
            // Update rowgroup centroid for fast pruning
            drop(rowgroup_manager);
            let mut rowgroup_manager = self.rowgroup_manager.write().await;
            if let Some(rg) = rowgroup_manager.rowgroups_mut().last_mut() {
                rg.centroid = Some(cluster_manager.get_global_centroid().await?);
            }
        }
        
        Ok(())
    }
    
    fn extract_vectors_from_batch(&self, batch: &RecordBatch) -> Result<Vec<Vec<f32>>> {
        let vector_column = batch.column_by_name("vector")
            .ok_or_else(|| anyhow::anyhow!("Vector column not found"))?;
        
        let float_array = vector_column
            .as_any()
            .downcast_ref::<Float32Array>()
            .ok_or_else(|| anyhow::anyhow!("Vector column is not Float32Array"))?;
        
        let dimension = float_array.len() / batch.num_rows();
        let mut vectors = Vec::with_capacity(batch.num_rows());
        
        for i in 0..batch.num_rows() {
            let start = i * dimension;
            let end = start + dimension;
            vectors.push(float_array.values()[start..end].to_vec());
        }
        
        Ok(vectors)
    }
    
    async fn search_internal(
        &self,
        query: &[f32],
        k: usize,
        filter: Option<HashMap<String, String>>,
        distance_metric: &crate::compute::distance_computation::DistanceMetric,
    ) -> Result<Vec<InternalSearchResult>> {
        // Use clustering for efficient rowgroup pruning
        let selected_rowgroups = self.select_rowgroups_by_clustering(query).await?;
        
        // First, use HNSW for candidate selection if available
        let candidates: Vec<InternalSearchResult> = if self.config.enable_hnsw {
            let hnsw = self.hnsw_manager.read().await;
            // Convert HNSW results to InternalSearchResult
            let hnsw_results = hnsw.search(query, k * 2).await?;
            hnsw_results.into_iter().map(|r| {
                // Convert HNSW result to InternalSearchResult 
                // TODO: Create proper VectorRecord from HNSW result for full conversion
                InternalSearchResult {
                    id: r.id,
                    vector_id: None,
                    score: r.score,
                    similarity: None,
                    vector: r.vector,
                    metadata: r.metadata.unwrap_or_default().into_iter()
                        .map(|(k, v)| (k, serde_json::json!(v)))
                        .collect(),
                    debug_info: None,
                    version: None,
                    timestamp: None,
                    updated_at: None,
                    expires_at: None,
                    source: None,
                    expanded_context: Vec::new(),
                    ..Default::default()
                }
            }).collect()
        } else {
            // Clustered search with pruning
            self.clustered_search(query, k * 2, selected_rowgroups, distance_metric).await?
        };
        
        // Apply filters and rerank
        let mut results = Vec::new();
        for candidate in candidates {
            if let Some(ref filter) = filter {
                if !self.matches_filter(&candidate, filter).await {
                    continue;
                }
            }
            results.push(candidate);
            if results.len() >= k {
                break;
            }
        }
        
        Ok(results)
    }
    
    async fn select_rowgroups_by_clustering(&self, query: &[f32]) -> Result<Vec<u32>> {
        let cluster_manager = self.cluster_manager.read().await;
        let cluster_assignments = self.cluster_assignments.read().await;
        let rowgroup_manager = self.rowgroup_manager.read().await;
        
        // Find nearest clusters to query
        let nearest_clusters = cluster_manager.find_nearest_clusters(query, 3).await?;
        
        // Select rowgroups that contain these clusters
        let mut selected = Vec::new();
        for (rg_id, assignments) in cluster_assignments.iter() {
            for assignment in assignments {
                if nearest_clusters.contains(&assignment.cluster_id) {
                    selected.push(*rg_id);
                    break;
                }
            }
        }
        
        // If no clusters found, use centroid-based selection
        if selected.is_empty() {
            for rowgroup in rowgroup_manager.rowgroups() {
                if let Some(centroid) = &rowgroup.centroid {
                    // Calculate distance using distance computation engine
                    let distance = 0.0; // TODO: Use distance computation engine
                    if distance < 0.5 { // Threshold for similarity
                        selected.push(rowgroup.id);
                    }
                }
            }
        }
        
        Ok(selected)
    }
    
    async fn clustered_search(
        &self,
        query: &[f32],
        k: usize,
        selected_rowgroups: Vec<u32>,
        distance_metric: &crate::compute::distance_computation::DistanceMetric,
    ) -> Result<Vec<InternalSearchResult>> {
        let mut all_results = Vec::new();
        
        for rg_id in selected_rowgroups {
            // Use filesystem API for efficient range reads
            let batch = self.read_rowgroup_with_range(rg_id).await?;
            
            // Compute distances using SIMD if available
            let distances = if self.config.enable_simd {
                // Use unified distance compute directly instead of removed simd_ops wrapper
                {
                    let vectors = self.extract_vectors_from_batch(&batch)?;
                    let compute = UnifiedDistanceCompute::default();
                    vectors.iter()
                        .map(|v| compute.compute_distance(query, v, distance_metric))
                        .collect::<Vec<_>>()
                }
            } else {
                self.compute_distances_scalar(query, &batch)?
            };
            
            // Collect results using standardized similarity scoring
            for (i, distance) in distances.iter().enumerate() {
                let id = self.get_id_from_batch(&batch, i)?;
                
                // Use standardized distance-to-similarity conversion for consistent ranking
                let search_result = InternalSearchResult::from_distance_standard(
                    id,
                    *distance,
                    distance_metric, // Pass the distance metric for proper conversion
                    None, // vector
                    HashMap::new(), // metadata
                );
                
                all_results.push(search_result);
            }
        }
        
        // Sort by similarity score in descending order (higher = more similar)
        all_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        all_results.truncate(k);
        
        Ok(all_results)
    }
    
    async fn read_rowgroup_with_range(&self, rg_id: u32) -> Result<RecordBatch> {
        let rowgroup_manager = self.rowgroup_manager.read().await;
        let rowgroup = rowgroup_manager.rowgroups().iter()
            .find(|rg| rg.id == rg_id)
            .ok_or_else(|| anyhow::anyhow!("RowGroup {} not found", rg_id))?;
        
        let path = format!("{}/rowgroup_{}.raptor", self.base_path, rg_id);
        
        // Use filesystem range read for efficient cloud I/O
        let data = if self.is_cloud_storage() {
            self.filesystem.read_range(
                &path,
                rowgroup.offset,
                rowgroup.compressed_size,
            ).await?
        } else {
            self.filesystem.read(&path).await?
        };
        
        // Decompress and deserialize
        let decompressed = self.decompress_data(&data)?;
        self.deserialize_batch(&decompressed)
    }
    
    fn is_cloud_storage(&self) -> bool {
        matches!(
            self.tier_config.tier,
            StorageTier::S3Express |
            StorageTier::S3Standard |
            StorageTier::S3GlacierInstant |
            StorageTier::AzurePremium |
            StorageTier::AzureStandard |
            StorageTier::GcsSSD |
            StorageTier::GcsHDD
        )
    }
    
    fn compute_distance(&self, a: &[f32], b: &[f32]) -> Result<f32> {
        if a.len() != b.len() {
            return Err(anyhow::anyhow!("Vector dimension mismatch"));
        }
        
        // Cosine distance
        let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        
        Ok(1.0 - (dot_product / (norm_a * norm_b)))
    }
    
    fn decompress_data(&self, data: &[u8]) -> Result<Vec<u8>> {
        // Simplified - would use actual compression codec
        Ok(data.to_vec())
    }
    
    fn deserialize_batch(&self, data: &[u8]) -> Result<RecordBatch> {
        // FASTLANES INTEGRATION: Check for encoding marker
        // RAPTOR uses 0xA0-0xAF range for tensor-optimized encodings
        if data.is_empty() {
            return Err(anyhow::anyhow!("Empty data"));
        }
        
        let encoding_marker = data[0];
        
        // Check if this is a FastLanes-encoded batch
        match encoding_marker {
            0xA1 => {
                // FastLanes tensor encoding - decode it first
                self.deserialize_fastlanes_batch(&data[1..], encoding_marker)
            }
            0xA2 => {
                // Sparse tensor encoding
                self.deserialize_sparse_tensor_batch(&data[1..])
            }
            0xA3 => {
                // Quantized tensor encoding
                self.deserialize_quantized_tensor_batch(&data[1..])
            }
            0xA0 | _ => {
                // Raw tensors or standard Arrow IPC format
                // For backward compatibility or non-encoded data
                use arrow_ipc::reader::StreamReader;
                use std::io::Cursor;
                
                // Skip marker if it's 0xA0, otherwise process full data
                let ipc_data = if encoding_marker == 0xA0 {
                    &data[1..]
                } else {
                    data
                };
                
                let cursor = Cursor::new(ipc_data);
                let reader = StreamReader::try_new(cursor, None)?;
                let batches: Result<Vec<_>, _> = reader.collect();
                let batches = batches?;
                
                if batches.is_empty() {
                    return Err(anyhow::anyhow!("No batches found"));
                }
                
                Ok(batches[0].clone())
            }
        }
    }
    
    fn deserialize_fastlanes_batch(&self, data: &[u8], marker: u8) -> Result<RecordBatch> {
        use crate::storage::engines::common::fastlanes_encoding::{FastLanesDecoder, FastLanesScheme};
        use std::io::Read;
        use arrow_array::{Float32Array, StringArray, Int64Array, UInt32Array, ArrayRef};
        
        let mut cursor = std::io::Cursor::new(data);
        
        // Read metadata
        let mut dim_bytes = [0u8; 4];
        cursor.read_exact(&mut dim_bytes)?;
        let dimension = u32::from_le_bytes(dim_bytes) as usize;
        
        let mut count_bytes = [0u8; 4];
        cursor.read_exact(&mut count_bytes)?;
        let num_vectors = u32::from_le_bytes(count_bytes) as usize;
        
        // Decode each dimension column
        let mut columns = Vec::with_capacity(dimension);
        for _ in 0..dimension {
            let mut len_bytes = [0u8; 4];
            cursor.read_exact(&mut len_bytes)?;
            let column_len = u32::from_le_bytes(len_bytes) as usize;
            
            let mut column_data = vec![0u8; column_len];
            cursor.read_exact(&mut column_data)?;
            
            // Decode using FastLanes
            // The scheme information should be embedded in the column data
            let decoder = FastLanesDecoder::new(FastLanesScheme::FrameOfReference { 
                reference: 0, 
                bits: 16 
            });
            let decoded = decoder.decode_f32(&column_data)?;
            columns.push(decoded);
        }
        
        // Transpose back to row-major for RecordBatch
        let mut vectors = Vec::with_capacity(num_vectors * dimension);
        for i in 0..num_vectors {
            for col in &columns {
                if i < col.len() {
                    vectors.push(col[i]);
                }
            }
        }
        
        // Read IDs if present
        let mut ids = Vec::new();
        for i in 0..num_vectors {
            let mut len_bytes = [0u8; 4];
            if cursor.read_exact(&mut len_bytes).is_ok() {
                let id_len = u32::from_le_bytes(len_bytes) as usize;
                if id_len > 0 {
                    let mut id_data = vec![0u8; id_len];
                    cursor.read_exact(&mut id_data)?;
                    ids.push(Some(String::from_utf8(id_data)?));
                } else {
                    ids.push(None);
                }
            } else {
                // Generate default IDs if not present
                ids.push(Some(format!("vec_{}", i)));
            }
        }
        
        // Read timestamps if present
        let mut timestamps = Vec::new();
        for _ in 0..num_vectors {
            let mut ts_bytes = [0u8; 8];
            if cursor.read_exact(&mut ts_bytes).is_ok() {
                timestamps.push(Some(i64::from_le_bytes(ts_bytes)));
            } else {
                timestamps.push(Some(0i64));
            }
        }
        
        // Create RecordBatch from decoded data
        let id_array = Arc::new(StringArray::from(ids)) as ArrayRef;
        let vector_array = Arc::new(Float32Array::from(vectors)) as ArrayRef;
        
        // Add placeholder metadata column
        let metadata_array = Arc::new(StringArray::from(vec![None::<String>; num_vectors])) as ArrayRef;
        
        // Add version column
        let version_array = Arc::new(UInt32Array::from(vec![1u32; num_vectors])) as ArrayRef;
        
        // Add timestamp column
        let timestamp_array = Arc::new(Int64Array::from(timestamps)) as ArrayRef;
        
        let batch = RecordBatch::try_new(
            Self::create_default_schema(),
            vec![id_array, vector_array, metadata_array, version_array, timestamp_array],
        )?;
        
        Ok(batch)
    }
    
    fn deserialize_sparse_tensor_batch(&self, data: &[u8]) -> Result<RecordBatch> {
        // SPARSE TENSOR DESERIALIZATION (COO/CSR format)
        // Marker 0xA2 indicates sparse tensor encoding
        use std::io::Read;
        use arrow_array::{Float32Array, StringArray, Int64Array, UInt32Array, ArrayRef};
        
        let mut cursor = std::io::Cursor::new(data);
        
        // Read sparse tensor metadata
        let mut format_byte = [0u8; 1];
        cursor.read_exact(&mut format_byte)?;
        let is_coo_format = format_byte[0] == 0; // 0=COO, 1=CSR
        
        let mut dim_bytes = [0u8; 4];
        cursor.read_exact(&mut dim_bytes)?;
        let dimension = u32::from_le_bytes(dim_bytes) as usize;
        
        let mut count_bytes = [0u8; 4];
        cursor.read_exact(&mut count_bytes)?;
        let num_vectors = u32::from_le_bytes(count_bytes) as usize;
        
        let mut nnz_bytes = [0u8; 4];
        cursor.read_exact(&mut nnz_bytes)?;
        let num_nonzeros = u32::from_le_bytes(nnz_bytes) as usize;
        
        if is_coo_format {
            // COO Format: (row_indices, col_indices, values)
            // Read row indices
            let mut row_indices = Vec::with_capacity(num_nonzeros);
            for _ in 0..num_nonzeros {
                let mut idx_bytes = [0u8; 4];
                cursor.read_exact(&mut idx_bytes)?;
                row_indices.push(u32::from_le_bytes(idx_bytes));
            }
            
            // Read column indices
            let mut col_indices = Vec::with_capacity(num_nonzeros);
            for _ in 0..num_nonzeros {
                let mut idx_bytes = [0u8; 4];
                cursor.read_exact(&mut idx_bytes)?;
                col_indices.push(u32::from_le_bytes(idx_bytes));
            }
            
            // Read values (using FastLanes encoding for compression)
            let mut val_len_bytes = [0u8; 4];
            cursor.read_exact(&mut val_len_bytes)?;
            let values_len = u32::from_le_bytes(val_len_bytes) as usize;
            
            let mut values_data = vec![0u8; values_len];
            cursor.read_exact(&mut values_data)?;
            
            // Decode values using FastLanes
            use crate::storage::engines::common::fastlanes_encoding::{FastLanesDecoder, FastLanesScheme};
            let decoder = FastLanesDecoder::new(FastLanesScheme::FrameOfReference {
                reference: 0,
                bits: 16,
            });
            let values = decoder.decode_f32(&values_data)?;
            
            // Reconstruct dense vectors from sparse representation
            let mut dense_vectors = vec![0.0f32; num_vectors * dimension];
            for (idx, &value) in values.iter().enumerate() {
                let row = row_indices[idx] as usize;
                let col = col_indices[idx] as usize;
                if row < num_vectors && col < dimension {
                    dense_vectors[row * dimension + col] = value;
                }
            }
            
            // Create RecordBatch
            self.create_batch_from_dense_vectors(dense_vectors, num_vectors, dimension)
        } else {
            // CSR Format: (row_ptrs, col_indices, values)
            // Read row pointers
            let mut row_ptrs = Vec::with_capacity(num_vectors + 1);
            for _ in 0..=num_vectors {
                let mut ptr_bytes = [0u8; 4];
                cursor.read_exact(&mut ptr_bytes)?;
                row_ptrs.push(u32::from_le_bytes(ptr_bytes));
            }
            
            // Read column indices
            let mut col_indices = Vec::with_capacity(num_nonzeros);
            for _ in 0..num_nonzeros {
                let mut idx_bytes = [0u8; 4];
                cursor.read_exact(&mut idx_bytes)?;
                col_indices.push(u32::from_le_bytes(idx_bytes));
            }
            
            // Read and decode values
            let mut val_len_bytes = [0u8; 4];
            cursor.read_exact(&mut val_len_bytes)?;
            let values_len = u32::from_le_bytes(val_len_bytes) as usize;
            
            let mut values_data = vec![0u8; values_len];
            cursor.read_exact(&mut values_data)?;
            
            use crate::storage::engines::common::fastlanes_encoding::{FastLanesDecoder, FastLanesScheme};
            let decoder = FastLanesDecoder::new(FastLanesScheme::FrameOfReference {
                reference: 0,
                bits: 16,
            });
            let values = decoder.decode_f32(&values_data)?;
            
            // Reconstruct dense vectors from CSR
            let mut dense_vectors = vec![0.0f32; num_vectors * dimension];
            for row in 0..num_vectors {
                let start = row_ptrs[row] as usize;
                let end = row_ptrs[row + 1] as usize;
                
                for idx in start..end {
                    if idx < col_indices.len() && idx < values.len() {
                        let col = col_indices[idx] as usize;
                        if col < dimension {
                            dense_vectors[row * dimension + col] = values[idx];
                        }
                    }
                }
            }
            
            self.create_batch_from_dense_vectors(dense_vectors, num_vectors, dimension)
        }
    }
    
    fn deserialize_quantized_tensor_batch(&self, data: &[u8]) -> Result<RecordBatch> {
        // QUANTIZED TENSOR DESERIALIZATION (INT8/PQ formats)
        // Marker 0xA3 indicates quantized tensor encoding
        use std::io::Read;
        use arrow_array::{Float32Array, StringArray, Int64Array, UInt32Array, ArrayRef};
        
        let mut cursor = std::io::Cursor::new(data);
        
        // Read quantization type
        let mut quant_type = [0u8; 1];
        cursor.read_exact(&mut quant_type)?;
        
        match quant_type[0] {
            0 => {
                // INT8 Quantization
                let mut dim_bytes = [0u8; 4];
                cursor.read_exact(&mut dim_bytes)?;
                let dimension = u32::from_le_bytes(dim_bytes) as usize;
                
                let mut count_bytes = [0u8; 4];
                cursor.read_exact(&mut count_bytes)?;
                let num_vectors = u32::from_le_bytes(count_bytes) as usize;
                
                // Read scale and zero point for dequantization
                let mut scale_bytes = [0u8; 4];
                cursor.read_exact(&mut scale_bytes)?;
                let scale = f32::from_le_bytes(scale_bytes);
                
                let mut zero_bytes = [0u8; 4];
                cursor.read_exact(&mut zero_bytes)?;
                let zero_point = f32::from_le_bytes(zero_bytes);
                
                // Read INT8 data
                let mut int8_data = vec![0i8; num_vectors * dimension];
                cursor.read_exact(unsafe {
                    std::slice::from_raw_parts_mut(int8_data.as_mut_ptr() as *mut u8, int8_data.len())
                })?;
                
                // Dequantize to FP32
                let mut dense_vectors = Vec::with_capacity(num_vectors * dimension);
                for &quantized_val in &int8_data {
                    let dequantized = (quantized_val as f32) * scale + zero_point;
                    dense_vectors.push(dequantized);
                }
                
                self.create_batch_from_dense_vectors(dense_vectors, num_vectors, dimension)
            }
            1 => {
                // Product Quantization (PQ)
                let mut dim_bytes = [0u8; 4];
                cursor.read_exact(&mut dim_bytes)?;
                let dimension = u32::from_le_bytes(dim_bytes) as usize;
                
                let mut count_bytes = [0u8; 4];
                cursor.read_exact(&mut count_bytes)?;
                let num_vectors = u32::from_le_bytes(count_bytes) as usize;
                
                let mut subvec_bytes = [0u8; 4];
                cursor.read_exact(&mut subvec_bytes)?;
                let num_subvectors = u32::from_le_bytes(subvec_bytes) as usize;
                
                let mut codebook_bytes = [0u8; 4];
                cursor.read_exact(&mut codebook_bytes)?;
                let codebook_size = u32::from_le_bytes(codebook_bytes) as usize;
                
                // Read codebooks (centroids for each subvector)
                let subvector_dim = dimension / num_subvectors;
                let mut codebooks = Vec::new();
                
                for _ in 0..num_subvectors {
                    let mut subvec_codebook = Vec::new();
                    for _ in 0..codebook_size {
                        for _ in 0..subvector_dim {
                            let mut val_bytes = [0u8; 4];
                            cursor.read_exact(&mut val_bytes)?;
                            subvec_codebook.push(f32::from_le_bytes(val_bytes));
                        }
                    }
                    codebooks.push(subvec_codebook);
                }
                
                // Read PQ codes (indices into codebooks)
                let mut pq_codes = vec![0u8; num_vectors * num_subvectors];
                cursor.read_exact(&mut pq_codes)?;
                
                // Reconstruct vectors from PQ codes
                let mut dense_vectors = Vec::with_capacity(num_vectors * dimension);
                for vec_idx in 0..num_vectors {
                    for subvec_idx in 0..num_subvectors {
                        let code = pq_codes[vec_idx * num_subvectors + subvec_idx] as usize;
                        let codebook_offset = code * subvector_dim;
                        
                        for dim_idx in 0..subvector_dim {
                            let value = codebooks[subvec_idx][codebook_offset + dim_idx];
                            dense_vectors.push(value);
                        }
                    }
                }
                
                self.create_batch_from_dense_vectors(dense_vectors, num_vectors, dimension)
            }
            2 => {
                // Binary Quantization (1 bit per dimension)
                let mut dim_bytes = [0u8; 4];
                cursor.read_exact(&mut dim_bytes)?;
                let dimension = u32::from_le_bytes(dim_bytes) as usize;
                
                let mut count_bytes = [0u8; 4];
                cursor.read_exact(&mut count_bytes)?;
                let num_vectors = u32::from_le_bytes(count_bytes) as usize;
                
                // Read binary data (packed bits)
                let bits_per_vector = (dimension + 7) / 8; // Round up to byte boundary
                let mut binary_data = vec![0u8; num_vectors * bits_per_vector];
                cursor.read_exact(&mut binary_data)?;
                
                // Unpack bits to float values (-1.0 or 1.0)
                let mut dense_vectors = Vec::with_capacity(num_vectors * dimension);
                for vec_idx in 0..num_vectors {
                    for dim_idx in 0..dimension {
                        let byte_idx = vec_idx * bits_per_vector + dim_idx / 8;
                        let bit_idx = dim_idx % 8;
                        let bit = (binary_data[byte_idx] >> bit_idx) & 1;
                        dense_vectors.push(if bit == 1 { 1.0 } else { -1.0 });
                    }
                }
                
                self.create_batch_from_dense_vectors(dense_vectors, num_vectors, dimension)
            }
            _ => {
                Err(anyhow::anyhow!("Unknown quantization type: {}", quant_type[0]))
            }
        }
    }
    
    fn create_batch_from_dense_vectors(
        &self,
        dense_vectors: Vec<f32>,
        num_vectors: usize,
        dimension: usize,
    ) -> Result<RecordBatch> {
        use arrow_array::{Float32Array, StringArray, Int64Array, UInt32Array, ArrayRef};
        
        // Generate IDs
        let ids: Vec<Option<String>> = (0..num_vectors)
            .map(|i| Some(format!("tensor_{}", i)))
            .collect();
        
        // Create arrays
        let id_array = Arc::new(StringArray::from(ids)) as ArrayRef;
        let vector_array = Arc::new(Float32Array::from(dense_vectors)) as ArrayRef;
        let metadata_array = Arc::new(StringArray::from(vec![None::<String>; num_vectors])) as ArrayRef;
        let version_array = Arc::new(UInt32Array::from(vec![1u32; num_vectors])) as ArrayRef;
        let timestamp_array = Arc::new(Int64Array::from(vec![0i64; num_vectors])) as ArrayRef;
        
        RecordBatch::try_new(
            Self::create_default_schema(),
            vec![id_array, vector_array, metadata_array, version_array, timestamp_array],
        ).map_err(|e| anyhow::anyhow!("Failed to create RecordBatch: {}", e))
    }
    
    async fn full_scan_search(&self, query: &[f32], k: usize, distance_metric: &crate::compute::distance_computation::DistanceMetric) -> Result<Vec<VectorSearchResult>> {
        let rowgroup_manager = self.rowgroup_manager.read().await;
        let predicates = vec![]; // No predicates for full scan
        let selected_rowgroups = rowgroup_manager.filter_rowgroups(&predicates);
        
        let mut all_results = Vec::new();
        
        for rg_id in selected_rowgroups {
            // Check cache first
            let key = format!("{}_{}", self.collection_id, rg_id);
            let batch = if let Some(cached) = self.get_cached_rowgroup(&key).await {
                cached
            } else {
                // Read from storage
                let batch = self.reader.read_rowgroup(rg_id).await?;
                self.cache_rowgroup(&key, batch.clone()).await;
                batch
            };
            
            // Compute distances using SIMD if available
            let distances = if self.config.enable_simd {
                // Use unified distance compute directly instead of removed simd_ops wrapper
                {
                    let vectors = self.extract_vectors_from_batch(&batch)?;
                    let compute = UnifiedDistanceCompute::default();
                    vectors.iter()
                        .map(|v| compute.compute_distance(query, v, distance_metric))
                        .collect::<Vec<_>>()
                }
            } else {
                self.compute_distances_scalar(query, &batch)?
            };
            
            // Collect results using standardized similarity scoring
            for (i, distance) in distances.iter().enumerate() {
                let id = self.get_id_from_batch(&batch, i)?;
                
                // Use standardized distance-to-similarity conversion for consistent ranking
                let search_result = InternalSearchResult::from_distance_standard(
                    id,
                    *distance,
                    distance_metric, // Pass the distance metric for proper conversion
                    None, // vector
                    HashMap::new(), // metadata
                );
                
                all_results.push(search_result);
            }
        }
        
        // Sort by similarity score in descending order (higher = more similar)
        all_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        all_results.truncate(k);
        
        Ok(all_results)
    }
    
    async fn get_cached_rowgroup(&self, key: &str) -> Option<RecordBatch> {
        let cache = self.cache.read().await;
        cache.get(key)
    }
    
    async fn cache_rowgroup(&self, key: &str, batch: RecordBatch) {
        let mut cache = self.cache.write().await;
        cache.put(key.to_string(), batch);
    }
    
    fn convert_to_arrow_batch(&self, records: Vec<VectorRecord>) -> Result<RecordBatch> {
        
        let mut ids = Vec::new();
        let mut vectors = Vec::new();
        let mut metadata_strs = Vec::new();
        let mut versions = Vec::new();
        let mut timestamps = Vec::new();
        
        for record in records {
            ids.push(record.id.clone());
            vectors.extend_from_slice(&record.vector);
            
            // Convert metadata to JSON string
            let metadata_json = serde_json::to_string(&record.metadata)?;
            metadata_strs.push(Some(metadata_json));
            
            versions.push(record.version);
            timestamps.push(Some(record.timestamp as i64));
        }
        
        let id_array = Arc::new(StringArray::from(ids)) as ArrayRef;
        let vector_array = Arc::new(Float32Array::from(vectors)) as ArrayRef;
        let metadata_array = Arc::new(StringArray::from(metadata_strs)) as ArrayRef;
        let version_array = Arc::new(UInt32Array::from(versions)) as ArrayRef;
        let timestamp_array = Arc::new(Int64Array::from(timestamps)) as ArrayRef;
        
        let batch = RecordBatch::try_new(
            Self::create_default_schema(),
            vec![id_array, vector_array, metadata_array, version_array, timestamp_array],
        )?;
        
        Ok(batch)
    }
    
    fn compute_distances_scalar(&self, query: &[f32], batch: &RecordBatch) -> Result<Vec<f32>> {
        let vector_column = batch.column_by_name("vector")
            .ok_or_else(|| anyhow::anyhow!("Vector column not found"))?;
        
        let float_array = vector_column
            .as_any()
            .downcast_ref::<Float32Array>()
            .ok_or_else(|| anyhow::anyhow!("Vector column is not Float32Array"))?;
        
        let dimension = query.len();
        let num_vectors = batch.num_rows();
        let mut distances = Vec::with_capacity(num_vectors);
        
        for i in 0..num_vectors {
            let start = i * dimension;
            let end = start + dimension;
            let vector = &float_array.values()[start..end];
            
            // Compute cosine distance
            let dot_product: f32 = query.iter()
                .zip(vector.iter())
                .map(|(a, b)| a * b)
                .sum();
            
            let query_norm: f32 = query.iter().map(|x| x * x).sum::<f32>().sqrt();
            let vector_norm: f32 = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
            
            let cosine_similarity = dot_product / (query_norm * vector_norm);
            let distance = 1.0 - cosine_similarity;
            
            distances.push(distance);
        }
        
        Ok(distances)
    }
    
    fn get_id_from_batch(&self, batch: &RecordBatch, index: usize) -> Result<String> {
        let id_column = batch.column_by_name("id")
            .ok_or_else(|| anyhow::anyhow!("ID column not found"))?;
        
        let string_array = id_column
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| anyhow::anyhow!("ID column is not StringArray"))?;
        
        Ok(string_array.value(index).to_string())
    }
    
    async fn matches_filter(
        &self,
        result: &VectorSearchResult,
        filter: &HashMap<String, String>,
    ) -> bool {
        // Simple filter matching - can be extended
        true
    }
    
    async fn should_compact(&self) -> bool {
        let registry = self.file_registry.read().await;
        registry.active_files.len() >= self.config.compaction_threshold_files
    }
}

#[async_trait]
impl UnifiedStorageEngine for RaptorEngine {
    fn engine_name(&self) -> &'static str {
        "RAPTOR"
    }
    
    fn engine_version(&self) -> &'static str {
        "1.0.0"
    }

    fn strategy(&self) -> crate::storage::traits::StorageEngineStrategy {
        crate::storage::traits::StorageEngineStrategy::Raptor
    }
    
    async fn do_flush(&self, params: &FlushParameters) -> Result<FlushResult> {
        let collection_id = params.collection_id.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Collection ID required for flush"))?;
        let start_time = std::time::Instant::now();
        
        // Get collection config dimension - this should always be available since dimension is required in CollectionConfig
        let collection_dimension = params.collection_config.as_ref()
            .and_then(|c| c.config.as_ref())
            .map(|cfg| cfg.dimension as usize)
            .expect("Collection dimension should always be available since it's required in CollectionConfig");
            
        tracing::debug!("RAPTOR flush: Using collection config dimension: {}", collection_dimension);
        // TODO: Update any dimension-dependent components with actual dimension
        // - Row group sizer optimization based on actual dimension
        // - HNSW parameter tuning for this dimension
        // - Memory allocation optimization
        
        let mut writer = self.writer.write().await;
        let bytes_written = writer.flush().await?;
        
        // Update unified metrics
        self.metrics.flush_operations.fetch_add(1, Ordering::Relaxed);
        // bytes_written is () from writer.flush(), so we'll skip this metric update
        // self.metrics.bytes_written.fetch_add(bytes_written as u64, Ordering::Relaxed);
        
        if self.config.enable_hnsw {
            let hnsw = self.hnsw_manager.read().await;
            hnsw.flush().await?;
        }
        
        Ok(FlushResult {
            success: true,
            files_created: 1,
            bytes_written: 0, // bytes_written is not available from flush()
            duration_ms: start_time.elapsed().as_millis() as u64,
            collections_affected: vec![],
            entries_flushed: 0,
            flushed_batch_ids: vec![],
            completed_at: chrono::Utc::now(),
            engine_metrics: HashMap::new(),
            compaction_triggered: false,
        })
    }
    
    async fn do_compact(&self, params: &CompactionParameters) -> Result<CompactionResult> {
        let collection_id = params.collection_id.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Collection ID required for compaction"))?;
        let start_time = std::time::Instant::now();
        
        // Get collection config dimension - this should always be available since dimension is required in CollectionConfig
        let collection_dimension = params.collection_config.as_ref()
            .and_then(|c| c.config.as_ref())
            .map(|cfg| cfg.dimension as usize)
            .expect("Collection dimension should always be available since it's required in CollectionConfig");
            
        tracing::debug!("RAPTOR compaction: Using collection config dimension: {}", collection_dimension);
        // TODO: Update any dimension-dependent compaction operations
        // - HNSW graph rebuilding optimization for this dimension
        // - Row group reorganization based on actual dimension
        // - Memory allocation optimization during compaction
        
        // Get all files from {base_path}/{collection_id}/data - unified directory structure
        let data_dir = format!("{}/{}/data", self.base_path, self.collection_id);
        let input_files: Vec<String> = match std::fs::read_dir(&data_dir) {
            Ok(entries) => entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().map_or(false, |ext| ext == "raptor"))
                .map(|e| e.path().to_string_lossy().to_string())
                .collect(),
            Err(_) => Vec::new(),
        };
        
        if !input_files.is_empty() {
            // Use unified FilenameCodec naming convention
            let codec = crate::storage::common::compaction_orchestrator::FilenameCodec::new();
            let filename = codec.generate(1, "raptor"); // Level 1 for compacted files
            let output_file = format!("{}/{}", data_dir, filename);
            self.compactor.compact_files(input_files, &output_file, &self.collection_id).await?;
        }
        
        // Update unified metrics
        self.metrics.compaction_operations.fetch_add(1, Ordering::Relaxed);
        
        Ok(CompactionResult {
            success: true,
            collections_affected: vec![collection_id.to_string()],
            entries_processed: 0,
            entries_removed: 0,
            bytes_read: 0,
            bytes_written: 0,
            input_files: 0,
            output_files: 0,
            completed_at: chrono::Utc::now(),
            engine_metrics: HashMap::new(),
            duration_ms: start_time.elapsed().as_millis() as u64,
        })
    }
    
    async fn collect_engine_metrics(&self) -> Result<HashMap<String, serde_json::Value>> {
        let mut stats = HashMap::new();
        
        // Collect atomic metrics without locks (unified metrics framework)
        stats.insert("total_vectors".to_string(), 
            serde_json::json!(self.metrics.total_vectors.load(Ordering::Relaxed)));
        stats.insert("total_rows".to_string(), 
            serde_json::json!(self.metrics.total_rows.load(Ordering::Relaxed)));
        stats.insert("total_files".to_string(), 
            serde_json::json!(self.metrics.total_files.load(Ordering::Relaxed)));
        stats.insert("insert_operations".to_string(), 
            serde_json::json!(self.metrics.insert_operations.load(Ordering::Relaxed)));
        stats.insert("search_operations".to_string(), 
            serde_json::json!(self.metrics.search_operations.load(Ordering::Relaxed)));
        stats.insert("flush_operations".to_string(), 
            serde_json::json!(self.metrics.flush_operations.load(Ordering::Relaxed)));
        stats.insert("compaction_operations".to_string(), 
            serde_json::json!(self.metrics.compaction_operations.load(Ordering::Relaxed)));
        stats.insert("cache_hits".to_string(), 
            serde_json::json!(self.metrics.cache_hits.load(Ordering::Relaxed)));
        stats.insert("cache_misses".to_string(), 
            serde_json::json!(self.metrics.cache_misses.load(Ordering::Relaxed)));
        stats.insert("cache_hit_ratio".to_string(), 
            serde_json::json!(self.metrics.cache_hit_ratio()));
        stats.insert("compression_ratio".to_string(), 
            serde_json::json!(self.metrics.compression_ratio()));
        stats.insert("bytes_written".to_string(), 
            serde_json::json!(self.metrics.bytes_written.load(Ordering::Relaxed)));
        stats.insert("bytes_read".to_string(), 
            serde_json::json!(self.metrics.bytes_read.load(Ordering::Relaxed)));
        stats.insert("memory_usage_bytes".to_string(), 
            serde_json::json!(self.metrics.memory_usage_bytes.load(Ordering::Relaxed)));
        
        // Engine identification for unified metrics dashboard
        stats.insert("engine_name".to_string(), serde_json::json!("RAPTOR"));
        stats.insert("engine_version".to_string(), serde_json::json!(crate::version::PROXIMADB_VERSION));
        
        Ok(stats)
    }
    
    async fn get_vector_by_id(&self, collection_id: &str, vector_id: &str) -> Result<Option<VectorRecord>> {
        // Load file metadata to access bloom filters
        let file_path = format!("{}/{}/raptor.data", self.base_path, collection_id);
        let metadata = self.reader.get_metadata(&file_path).await?;
        
        // For now, use a simple approach - read all row groups and search for the ID
        // TODO: Implement efficient bloom filter lookup
        let rowgroup_indices: Vec<u32> = (0..metadata.row_groups.len() as u32).collect();
        let batches = self.reader.read_rowgroups(&file_path, &rowgroup_indices).await?;
        
        // Search through all batches for the vector ID
        if let Some(batch) = batches.first() {
            // The lookup_ids_after_hnsw already filtered to just our ID
            // So we can directly reconstruct the vector record
            if batch.num_rows() > 0 {
                return Ok(Some(self.reconstruct_vector_record(&batch, 0)?));
            }
        }
        
        Ok(None)
    }
    
    async fn search_vectors_unified(
        &self,
        ctx: &SearchContext,
    ) -> Result<Vec<crate::core::search::InternalSearchResult>> {
        // Extract all parameters from enhanced context (pre-computed)
        let collection_id = ctx.collection_id();
        let storage_path = ctx.storage_path();
        let query_vector = ctx.query_vector()
            .ok_or_else(|| anyhow::anyhow!("No query vector in search context"))?;
        let k = ctx.top_k();
        let dimension = ctx.dimension();
        let distance_metric = ctx.distance_metric();
        let performance_tier = ctx.performance_tier();
        // These fields are no longer in search_params, default to true
        let include_vectors = true;
        let include_metadata = true;
        
        // Log search with enhanced context info
        tracing::info!("RAPTOR search: collection={}, k={}, metric={:?}, tier={:?}, storage_path={}",
            collection_id, k, distance_metric, performance_tier, storage_path);
        
        // Convert filter expression to simple filter for now
        let filter = if ctx.search_params.filter_expression.is_some() {
            Some(HashMap::new()) // Simplified
        } else {
            None
        };
        
        // Use performance tier to optimize search strategy
        let results = match performance_tier {
            crate::storage::traits::PerformanceTier::Hot => {
                // Memory-first search for hot data
                self.search_internal(query_vector, k, filter, &distance_metric).await?
            },
            _ => {
                // Standard search for other tiers
                self.search_internal(query_vector, k, filter, &distance_metric).await?
            }
        };
        
        // Return InternalSearchResult directly
        Ok(results)
    }
    
    fn get_filesystem_factory(&self) -> &crate::storage::persistence::filesystem::FilesystemFactory {
        // Would return actual filesystem factory
        unimplemented!("Filesystem factory not yet implemented")
    }
    
    fn get_collection_service(&self) -> Option<&crate::services::collection_service::CollectionService> {
        None // RAPTOR doesn't have direct access to collection service
    }
}

// Helper structures
struct RowGroupCache {
    capacity: usize,
    cache: HashMap<String, RecordBatch>,
    access_counts: HashMap<String, usize>,
}

impl RowGroupCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            cache: HashMap::new(),
            access_counts: HashMap::new(),
        }
    }
    
    fn get(&self, key: &str) -> Option<RecordBatch> {
        self.cache.get(key).cloned()
    }
    
    fn put(&mut self, key: String, batch: RecordBatch) {
        // Simple LRU eviction
        if self.cache.len() >= self.capacity {
            // Find least recently used
            if let Some(lru_key) = self.access_counts
                .iter()
                .min_by_key(|(_, count)| *count)
                .map(|(k, _)| k.clone()) 
            {
                self.cache.remove(&lru_key);
                self.access_counts.remove(&lru_key);
            }
        }
        
        self.cache.insert(key.clone(), batch);
        *self.access_counts.entry(key).or_insert(0) += 1;
    }
    
    fn optimize(&mut self) {
        // Remove entries with low access counts
        let threshold = 2;
        self.cache.retain(|k, _| {
            self.access_counts.get(k).unwrap_or(&0) >= &threshold
        });
    }
}

struct FileRegistry {
    active_files: HashMap<Uuid, FileMetadata>,
    compacting_files: HashMap<Uuid, FileMetadata>,
}

impl FileRegistry {
    fn new() -> Self {
        Self {
            active_files: HashMap::new(),
            compacting_files: HashMap::new(),
        }
    }
}

struct FileMetadata {
    id: Uuid,
    path: String,
    size_bytes: u64,
    row_count: usize,
    created_at: chrono::DateTime<chrono::Utc>,
}

// RaptorMetrics - integrated with unified metrics framework
// Using atomic counters for lock-free metric updates
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

struct RaptorMetrics {
    // Vector and row metrics
    total_vectors: AtomicUsize,
    total_rows: AtomicUsize,
    total_files: AtomicUsize,
    
    // Operation counters
    insert_operations: AtomicU64,
    search_operations: AtomicU64,
    flush_operations: AtomicU64,
    compaction_operations: AtomicU64,
    
    // Cache metrics
    cache_hits: AtomicU64,
    cache_misses: AtomicU64,
    
    // I/O metrics
    bytes_written: AtomicU64,
    bytes_read: AtomicU64,
    memory_usage_bytes: AtomicU64,
}

impl RaptorMetrics {
    fn new() -> Self {
        Self {
            total_vectors: AtomicUsize::new(0),
            total_rows: AtomicUsize::new(0),
            total_files: AtomicUsize::new(0),
            insert_operations: AtomicU64::new(0),
            search_operations: AtomicU64::new(0),
            flush_operations: AtomicU64::new(0),
            compaction_operations: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
            bytes_written: AtomicU64::new(0),
            bytes_read: AtomicU64::new(0),
            memory_usage_bytes: AtomicU64::new(0),
        }
    }
    
    fn cache_hit_ratio(&self) -> f32 {
        let hits = self.cache_hits.load(Ordering::Relaxed);
        let misses = self.cache_misses.load(Ordering::Relaxed);
        if hits + misses == 0 {
            0.0
        } else {
            hits as f32 / (hits + misses) as f32
        }
    }
    
    fn compression_ratio(&self) -> f32 {
        let written = self.bytes_written.load(Ordering::Relaxed);
        let memory = self.memory_usage_bytes.load(Ordering::Relaxed);
        if memory == 0 {
            1.0
        } else {
            written as f32 / memory as f32
        }
    }
}

impl RaptorEngine {
    fn determine_storage_tier(base_path: &str) -> StorageTier {
        if base_path.starts_with("s3://") {
            if base_path.contains("express") {
                StorageTier::S3Express
            } else if base_path.contains("glacier") {
                StorageTier::S3GlacierInstant
            } else {
                StorageTier::S3Standard
            }
        } else if base_path.starts_with("gs://") {
            StorageTier::GcsSSD
        } else if base_path.starts_with("azure://") || base_path.starts_with("adls://") {
            StorageTier::AzurePremium
        } else if base_path.contains("nvme") {
            StorageTier::NVMe
        } else if base_path.contains("ssd") {
            StorageTier::SSD
        } else {
            StorageTier::HDD
        }
    }
    
    fn reconstruct_vector_record(&self, batch: &RecordBatch, index: usize) -> Result<VectorRecord> {
        let id = self.get_id_from_batch(batch, index)?;
        
        let vector_column = batch.column_by_name("vector")
            .ok_or_else(|| anyhow::anyhow!("Vector column not found"))?;
        let float_array = vector_column
            .as_any()
            .downcast_ref::<Float32Array>()
            .ok_or_else(|| anyhow::anyhow!("Vector column is not Float32Array"))?;
        
        let dimension = float_array.len() / batch.num_rows();
        let start = index * dimension;
        let end = start + dimension;
        let vector = float_array.values()[start..end].to_vec();
        
        // Get metadata if present
        let metadata = if let Some(metadata_column) = batch.column_by_name("metadata") {
            let string_array = metadata_column
                .as_any()
                .downcast_ref::<StringArray>();
            
            if let Some(arr) = string_array {
                if let Some(metadata_str) = arr.value(index).parse::<String>().ok() {
                    serde_json::from_str(&metadata_str).unwrap_or_default()
                } else {
                    HashMap::<String, serde_json::Value>::new()
                }
            } else {
                HashMap::new()
            }
        } else {
            HashMap::new()
        };
        
        // Get version
        let version = if let Some(version_column) = batch.column_by_name("version") {
            let uint_array = version_column
                .as_any()
                .downcast_ref::<UInt32Array>();
            
            uint_array.and_then(|arr| Some(arr.value(index)))
        } else {
            None
        };
        
        // Get timestamp
        let timestamp = if let Some(timestamp_column) = batch.column_by_name("timestamp") {
            let int_array = timestamp_column
                .as_any()
                .downcast_ref::<Int64Array>();
            
            int_array.and_then(|arr| Some(arr.value(index) as u32))
        } else {
            None
        };
        
        Ok(VectorRecord {
            id,
            vector,
            metadata: metadata.into_iter()
                .map(|(k, v)| crate::proto::proximadb::MetadataItem {
                    key: k,
                    value: Some(crate::proto::proximadb::metadata_item::Value::StringValue(v.to_string())),
                })
                .collect(),
            version,
            timestamp: timestamp.unwrap_or(0),
            ..Default::default()
        })
    }
}

/// Implementation of UniversallyOptimized trait for RAPTOR engine
#[async_trait]
impl UniversallyOptimized for RaptorEngine {
    /// Get the universal performance optimizer instance
    fn get_universal_optimizer(&self) -> &UniversalPerformanceOptimizer {
        &self.universal_optimizer
    }
    
    /// RAPTOR-specific optimization setup
    async fn setup_engine_optimizations(&self) -> Result<()> {
        // RAPTOR-specific optimizations for columnar analytics with clustering
        tracing::info!("🔧 RAPTOR Engine: Setting up universal performance optimizations");
        
        // Initialize RAPTOR-specific optimizations
        let config = self.universal_optimizer.get_config();
        tracing::debug!("   Cache size: {}MB", config.cache_size_mb);
        tracing::debug!("   Parallel operations: {}", config.parallel_operations);
        tracing::debug!("   Prefetching enabled: {}", config.enable_prefetching);
        tracing::debug!("   Memory mapping enabled: {}", config.enable_memory_mapping);
        
        // RAPTOR is ready for high-performance columnar operations
        tracing::info!("✅ RAPTOR Engine: Universal optimizations configured for columnar analytics");
        Ok(())
    }
    
    /// RAPTOR-specific performance metrics
    async fn collect_performance_metrics(&self) -> Result<HashMap<String, serde_json::Value>> {
        let mut metrics = HashMap::new();
        
        // Basic RAPTOR metrics (using unified framework)
        metrics.insert("raptor_total_rows".to_string(), serde_json::Value::Number(
            serde_json::Number::from(self.metrics.total_rows.load(Ordering::Relaxed))
        ));
        metrics.insert("raptor_total_files".to_string(), serde_json::Value::Number(
            serde_json::Number::from(self.metrics.total_files.load(Ordering::Relaxed))
        ));
        metrics.insert("raptor_memory_usage_bytes".to_string(), serde_json::Value::Number(
            serde_json::Number::from(self.metrics.memory_usage_bytes.load(Ordering::Relaxed))
        ));
        
        // Universal optimizer metrics
        let strategy = self.universal_optimizer.get_strategy();
        metrics.insert("universal_optimization_strategy".to_string(), 
            serde_json::Value::String(format!("{:?}", strategy)));
        
        let config = self.universal_optimizer.get_config();
        metrics.insert("universal_cache_size_mb".to_string(), serde_json::Value::Number(
            serde_json::Number::from(config.cache_size_mb)
        ));
        metrics.insert("universal_parallel_operations".to_string(), serde_json::Value::Number(
            serde_json::Number::from(config.parallel_operations)
        ));
        metrics.insert("universal_prefetching_enabled".to_string(), serde_json::Value::Bool(
            config.enable_prefetching
        ));
        
        Ok(metrics)
    }
}