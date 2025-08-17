use async_trait::async_trait;
use arrow::record_batch::RecordBatch;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use anyhow::Result;
use uuid::Uuid;

use crate::storage::engines::{UnifiedStorageEngine, EngineType};
use crate::proto::proximadb::{VectorRecord, Collection};
use crate::storage::common::{VectorSearchResult, SearchOptions};
use super::{RaptorConfig, RowGroupManager, RaptorWriter, RaptorReader};
use super::compaction::CompactionManager;
use super::hnsw_manager::HnswManager;

// Deep integration with AXIS clustering
use crate::index::axis::clustering::{ClusteringConfig, ClusteringAlgorithm, KMeansConfig, ClusterManager};
use crate::index::axis::types::ClusterAssignment;

// Deep integration with filesystem API for cloud-aware I/O
use crate::storage::persistence::filesystem::{FileSystem, FilesystemFactory, FileOptions, StorageTier};
use crate::storage::persistence::filesystem::TierConfig;

pub struct RaptorEngine {
    config: RaptorConfig,
    collection_id: String,
    base_path: String,
    
    // Core components
    rowgroup_manager: Arc<RwLock<RowGroupManager>>,
    writer: Arc<RwLock<RaptorWriter>>,
    reader: Arc<RaptorReader>,
    compaction_manager: Arc<CompactionManager>,
    hnsw_manager: Arc<RwLock<HnswManager>>,
    
    // Deep integration with AXIS clustering
    cluster_manager: Arc<RwLock<ClusterManager>>,
    clustering_config: ClusteringConfig,
    cluster_assignments: Arc<RwLock<HashMap<u32, Vec<ClusterAssignment>>>>, // RowGroup -> Clusters
    
    // Deep integration with filesystem API
    filesystem: Arc<dyn FileSystem>,
    tier_config: TierConfig,
    file_options: FileOptions,
    
    // Cache and metadata
    cache: Arc<RwLock<RowGroupCache>>,
    file_registry: Arc<RwLock<FileRegistry>>,
    metrics: Arc<RwLock<EngineMetrics>>,
}

impl RaptorEngine {
    pub async fn new(
        collection_id: String,
        base_path: String,
        config: RaptorConfig,
    ) -> Result<Self> {
        let schema = Self::create_default_schema();
        let rowgroup_manager = Arc::new(RwLock::new(RowGroupManager::new(schema.clone())));
        
        // Initialize filesystem with proper abstraction
        let filesystem = FilesystemFactory::create(&base_path).await?;
        
        // Determine storage tier from URL
        let tier = Self::determine_storage_tier(&base_path);
        let tier_config = TierConfig {
            tier,
            base_url: base_path.clone(),
            max_capacity_bytes: None,
            current_usage_bytes: 0,
            compression: config.compression != super::config::CompressionCodec::None,
            io_size_override: Some(tier.optimal_io_size()),
        };
        
        // Configure file options for cloud-aware operations
        let file_options = FileOptions {
            create_dirs: true,
            overwrite: false,
            buffer_size: Some(tier.optimal_io_size()),
            encryption: None,
            storage_class: Self::get_storage_class(&tier),
            metadata: None,
            temp_path: None,
        };
        
        let writer = Arc::new(RwLock::new(
            RaptorWriter::new(base_path.clone(), config.clone(), schema.clone()).await?
        ));
        
        let reader = Arc::new(
            RaptorReader::new(base_path.clone(), config.clone()).await?
        );
        
        let compaction_manager = Arc::new(
            CompactionManager::new(base_path.clone(), config.clone())
        );
        
        let hnsw_manager = Arc::new(RwLock::new(
            HnswManager::new(config.clone()).await?
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
        
        let metrics = Arc::new(RwLock::new(
            EngineMetrics::new()
        ));
        
        Ok(Self {
            config,
            collection_id,
            base_path,
            rowgroup_manager,
            writer,
            reader,
            compaction_manager,
            hnsw_manager,
            cluster_manager,
            clustering_config,
            cluster_assignments,
            filesystem,
            tier_config,
            file_options,
            cache,
            file_registry,
            metrics,
        })
    }
    
    fn create_default_schema() -> Arc<arrow::datatypes::Schema> {
        use arrow::datatypes::{DataType, Field, Schema};
        
        let fields = vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("vector", DataType::Float32, false),
            Field::new("metadata", DataType::Utf8, true), // JSON string for now
            Field::new("version", DataType::UInt32, true),
            Field::new("timestamp", DataType::Int64, true),
        ];
        
        Arc::new(Schema::new(fields))
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
        
        // Update metrics
        let mut metrics = self.metrics.write().await;
        metrics.total_vectors += row_count;
        metrics.insert_operations += 1;
        
        // Check if compaction is needed
        if self.should_compact().await {
            let compaction_manager = self.compaction_manager.clone();
            tokio::spawn(async move {
                let _ = compaction_manager.compact().await;
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
        if let Some(current_rg) = rowgroup_manager.rowgroups.last() {
            let mut cluster_assignments = self.cluster_assignments.write().await;
            cluster_assignments.insert(current_rg.id, assignments);
            
            // Update rowgroup centroid for fast pruning
            drop(rowgroup_manager);
            let mut rowgroup_manager = self.rowgroup_manager.write().await;
            if let Some(rg) = rowgroup_manager.rowgroups.last_mut() {
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
            .downcast_ref::<arrow_array::Float32Array>()
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
    ) -> Result<Vec<VectorSearchResult>> {
        // Use clustering for efficient rowgroup pruning
        let selected_rowgroups = self.select_rowgroups_by_clustering(query).await?;
        
        // First, use HNSW for candidate selection if available
        let candidates = if self.config.enable_hnsw {
            let hnsw = self.hnsw_manager.read().await;
            hnsw.search(query, k * 2).await?
        } else {
            // Clustered search with pruning
            self.clustered_search(query, k * 2, selected_rowgroups).await?
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
            for rowgroup in &rowgroup_manager.rowgroups {
                if let Some(centroid) = &rowgroup.centroid {
                    let distance = self.compute_distance(query, centroid)?;
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
    ) -> Result<Vec<VectorSearchResult>> {
        let mut all_results = Vec::new();
        
        for rg_id in selected_rowgroups {
            // Use filesystem API for efficient range reads
            let batch = self.read_rowgroup_with_range(rg_id).await?;
            
            // Compute distances using SIMD if available
            let distances = if self.config.enable_simd {
                super::simd_ops::compute_distances_simd(query, &batch)?
            } else {
                self.compute_distances_scalar(query, &batch)?
            };
            
            // Collect results
            for (i, distance) in distances.iter().enumerate() {
                all_results.push(VectorSearchResult {
                    id: self.get_id_from_batch(&batch, i)?,
                    score: *distance,
                    vector: None,
                    metadata: None,
                });
            }
        }
        
        // Sort by distance and take top k
        all_results.sort_by(|a, b| a.score.partial_cmp(&b.score).unwrap());
        all_results.truncate(k);
        
        Ok(all_results)
    }
    
    async fn read_rowgroup_with_range(&self, rg_id: u32) -> Result<RecordBatch> {
        let rowgroup_manager = self.rowgroup_manager.read().await;
        let rowgroup = rowgroup_manager.rowgroups.iter()
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
        use arrow::ipc::reader::StreamReader;
        use std::io::Cursor;
        
        let cursor = Cursor::new(data);
        let reader = StreamReader::try_new(cursor, None)?;
        let batches: Result<Vec<_>, _> = reader.collect();
        let batches = batches?;
        
        if batches.is_empty() {
            return Err(anyhow::anyhow!("No batches found"));
        }
        
        Ok(batches[0].clone())
    }
    
    async fn full_scan_search(&self, query: &[f32], k: usize) -> Result<Vec<VectorSearchResult>> {
        let rowgroup_manager = self.rowgroup_manager.read().await;
        let predicates = vec![]; // No predicates for full scan
        let selected_rowgroups = rowgroup_manager.filter_rowgroups(&predicates);
        
        let mut all_results = Vec::new();
        
        for rg_id in selected_rowgroups {
            // Check cache first
            let cache_key = format!("{}_{}", self.collection_id, rg_id);
            let batch = if let Some(cached) = self.get_cached_rowgroup(&cache_key).await {
                cached
            } else {
                // Read from storage
                let batch = self.reader.read_rowgroup(rg_id).await?;
                self.cache_rowgroup(&cache_key, batch.clone()).await;
                batch
            };
            
            // Compute distances using SIMD if available
            let distances = if self.config.enable_simd {
                super::simd_ops::compute_distances_simd(query, &batch)?
            } else {
                self.compute_distances_scalar(query, &batch)?
            };
            
            // Collect results
            for (i, distance) in distances.iter().enumerate() {
                all_results.push(VectorSearchResult {
                    id: self.get_id_from_batch(&batch, i)?,
                    score: *distance,
                    vector: None, // Populated if needed
                    metadata: None, // Populated if needed
                });
            }
        }
        
        // Sort by distance and take top k
        all_results.sort_by(|a, b| a.score.partial_cmp(&b.score).unwrap());
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
        use arrow::array::{Float32Array, StringArray, UInt32Array, Int64Array};
        
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
            timestamps.push(record.timestamp.map(|t| t as i64));
        }
        
        let id_array = Arc::new(StringArray::from(ids)) as arrow::array::ArrayRef;
        let vector_array = Arc::new(Float32Array::from(vectors)) as arrow::array::ArrayRef;
        let metadata_array = Arc::new(StringArray::from(metadata_strs)) as arrow::array::ArrayRef;
        let version_array = Arc::new(UInt32Array::from(versions)) as arrow::array::ArrayRef;
        let timestamp_array = Arc::new(Int64Array::from(timestamps)) as arrow::array::ArrayRef;
        
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
            .downcast_ref::<arrow_array::Float32Array>()
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
            .downcast_ref::<arrow_array::StringArray>()
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
    fn engine_type(&self) -> EngineType {
        EngineType::Custom("RAPTOR".to_string())
    }
    
    async fn insert_vector(&self, record: VectorRecord) -> Result<()> {
        self.insert_batch_internal(vec![record]).await
    }
    
    async fn insert_batch(&self, records: Vec<VectorRecord>) -> Result<()> {
        self.insert_batch_internal(records).await
    }
    
    async fn get_vector(&self, id: &str) -> Result<Option<VectorRecord>> {
        // Use bloom filter for quick existence check
        let rowgroup_manager = self.rowgroup_manager.read().await;
        
        for rowgroup in &rowgroup_manager.rowgroups {
            if let Some(bloom) = rowgroup_manager.bloom_filters.get(&rowgroup.id) {
                if !bloom.check(&id.to_string()) {
                    continue;
                }
            }
            
            // Read the rowgroup and search for the ID
            let batch = self.reader.read_rowgroup(rowgroup.id).await?;
            
            let id_column = batch.column_by_name("id")
                .ok_or_else(|| anyhow::anyhow!("ID column not found"))?;
            
            let string_array = id_column
                .as_any()
                .downcast_ref::<arrow_array::StringArray>()
                .ok_or_else(|| anyhow::anyhow!("ID column is not StringArray"))?;
            
            for i in 0..batch.num_rows() {
                if string_array.value(i) == id {
                    // Found the vector, reconstruct VectorRecord
                    return Ok(Some(self.reconstruct_vector_record(&batch, i)?));
                }
            }
        }
        
        Ok(None)
    }
    
    async fn update_vector(&self, record: VectorRecord) -> Result<()> {
        // RAPTOR uses append-only storage with versioning
        // Updates are implemented as new inserts with higher version
        let mut updated_record = record.clone();
        updated_record.version = Some(updated_record.version.unwrap_or(0) + 1);
        self.insert_vector(updated_record).await
    }
    
    async fn delete_vector(&self, id: &str) -> Result<()> {
        // Mark as deleted with tombstone
        let tombstone = VectorRecord {
            id: id.to_string(),
            vector: vec![],
            metadata: HashMap::new(),
            version: Some(u32::MAX), // Special version for deletion
            timestamp: Some(chrono::Utc::now().timestamp() as u32),
            ..Default::default()
        };
        self.insert_vector(tombstone).await
    }
    
    async fn search_vectors(
        &self,
        query: &[f32],
        k: usize,
        filter: Option<HashMap<String, String>>,
    ) -> Result<Vec<VectorSearchResult>> {
        self.search_internal(query, k, filter).await
    }
    
    async fn flush(&self) -> Result<()> {
        let mut writer = self.writer.write().await;
        writer.flush().await?;
        
        if self.config.enable_hnsw {
            let hnsw = self.hnsw_manager.read().await;
            hnsw.flush().await?;
        }
        
        Ok(())
    }
    
    async fn compact(&self) -> Result<()> {
        self.compaction_manager.compact().await
    }
    
    async fn get_stats(&self) -> Result<HashMap<String, serde_json::Value>> {
        let metrics = self.metrics.read().await;
        let mut stats = HashMap::new();
        
        stats.insert("total_vectors".to_string(), 
            serde_json::json!(metrics.total_vectors));
        stats.insert("insert_operations".to_string(), 
            serde_json::json!(metrics.insert_operations));
        stats.insert("search_operations".to_string(), 
            serde_json::json!(metrics.search_operations));
        stats.insert("cache_hit_ratio".to_string(), 
            serde_json::json!(metrics.cache_hit_ratio()));
        stats.insert("compression_ratio".to_string(), 
            serde_json::json!(metrics.compression_ratio));
        
        Ok(stats)
    }
    
    async fn optimize(&self) -> Result<()> {
        // Trigger optimization tasks
        if self.config.enable_hnsw {
            let mut hnsw = self.hnsw_manager.write().await;
            hnsw.optimize().await?;
        }
        
        // Optimize cache
        let mut cache = self.cache.write().await;
        cache.optimize();
        
        Ok(())
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

struct EngineMetrics {
    total_vectors: usize,
    insert_operations: u64,
    search_operations: u64,
    cache_hits: u64,
    cache_misses: u64,
    compression_ratio: f32,
}

impl EngineMetrics {
    fn new() -> Self {
        Self {
            total_vectors: 0,
            insert_operations: 0,
            search_operations: 0,
            cache_hits: 0,
            cache_misses: 0,
            compression_ratio: 1.0,
        }
    }
    
    fn cache_hit_ratio(&self) -> f32 {
        if self.cache_hits + self.cache_misses == 0 {
            0.0
        } else {
            self.cache_hits as f32 / (self.cache_hits + self.cache_misses) as f32
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
    
    fn get_storage_class(tier: &StorageTier) -> Option<String> {
        match tier {
            StorageTier::S3Express => Some("EXPRESS_ONEZONE".to_string()),
            StorageTier::S3Standard => Some("STANDARD".to_string()),
            StorageTier::S3GlacierInstant => Some("GLACIER_IR".to_string()),
            StorageTier::AzurePremium => Some("Premium_LRS".to_string()),
            StorageTier::AzureStandard => Some("Standard_LRS".to_string()),
            _ => None,
        }
    }
    
    fn reconstruct_vector_record(&self, batch: &RecordBatch, index: usize) -> Result<VectorRecord> {
        let id = self.get_id_from_batch(batch, index)?;
        
        let vector_column = batch.column_by_name("vector")
            .ok_or_else(|| anyhow::anyhow!("Vector column not found"))?;
        let float_array = vector_column
            .as_any()
            .downcast_ref::<arrow_array::Float32Array>()
            .ok_or_else(|| anyhow::anyhow!("Vector column is not Float32Array"))?;
        
        let dimension = float_array.len() / batch.num_rows();
        let start = index * dimension;
        let end = start + dimension;
        let vector = float_array.values()[start..end].to_vec();
        
        // Get metadata if present
        let metadata = if let Some(metadata_column) = batch.column_by_name("metadata") {
            let string_array = metadata_column
                .as_any()
                .downcast_ref::<arrow_array::StringArray>();
            
            if let Some(arr) = string_array {
                if let Some(metadata_str) = arr.value(index).parse::<String>().ok() {
                    serde_json::from_str(&metadata_str).unwrap_or_default()
                } else {
                    HashMap::new()
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
                .downcast_ref::<arrow::array::UInt32Array>();
            
            uint_array.and_then(|arr| Some(arr.value(index)))
        } else {
            None
        };
        
        // Get timestamp
        let timestamp = if let Some(timestamp_column) = batch.column_by_name("timestamp") {
            let int_array = timestamp_column
                .as_any()
                .downcast_ref::<arrow::array::Int64Array>();
            
            int_array.and_then(|arr| Some(arr.value(index) as u32))
        } else {
            None
        };
        
        Ok(VectorRecord {
            id,
            vector,
            metadata,
            version,
            timestamp,
            ..Default::default()
        })
    }
}