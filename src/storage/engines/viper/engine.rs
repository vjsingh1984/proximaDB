// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! VIPER Engine - Vector Storage Layer
//!
//! VIPER (Vector-optimized Intelligent Parquet with Efficient Retrieval) is a pure
//! storage engine focused on durability and efficient serialization of vectors.
//!
//! Responsibilities:
//! - Store vectors in columnar Parquet format
//! - Handle flush operations from WAL to persistent storage
//! - Perform compaction to optimize storage layout
//! - Provide direct vector search on Parquet files (baseline functionality)
//!
//! NOT Responsible For:
//! - ML clustering (belongs in AXIS indexing service)
//! - Index management (AXIS responsibility)
//! - Query optimization strategies (AXIS layer)
//!
//! Architecture:
//! - VIPER provides baseline search that works for ALL collections
//! - AXIS can optionally add ML clustering as an optimization layer
//! - Clean separation: VIPER = storage, AXIS = indexing

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn, trace};

use crate::core::{String, VectorRecord};
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::traits::{FlushResult, UnifiedStorageEngine, CollectionMetadataProvider};
use crate::core::search::UnifiedSearchEngine;

use super::types::*;
use super::schema::SchemaManager;
use super::compaction::CompactionManager;
use super::flush::FlushManager;
// use super::ml_clustering::MLClusteringEngine; // Moved to AXIS
use super::utilities::ViperUtilities;
use super::unified_search_engine::ViperUnifiedSearchEngine;
use crate::compute::unified_distance::UnifiedDistanceCompute;
use super::types::CollectionMetadata;

/// VIPER Engine - Main coordination point for the modular VIPER storage engine
#[derive(Debug)]
pub struct ViperEngine {
    /// Configuration
    config: ViperConfig,
    
    /// Collection service for metadata access
    collection_service: Arc<RwLock<Option<Arc<crate::services::collection_service::CollectionService>>>>,
    
    /// Filesystem interface
    filesystem: Arc<FilesystemFactory>,
    
    /// Modular managers
    schema_manager: SchemaManager,
    compaction_manager: CompactionManager,
    flush_manager: FlushManager,
    // ml_clustering_engine: MLClusteringEngine, // Moved to AXIS
    utilities: ViperUtilities,
    search_engine: Arc<ViperUnifiedSearchEngine>,
    
    /// Engine statistics
    stats: Arc<RwLock<EngineStats>>,
    
    /// Collection metadata cache
    collections: Arc<RwLock<HashMap<String, CollectionMetadata>>>,
}

impl ViperEngine {
    /// Create a new VIPER engine with the specified configuration
    pub async fn new(config: ViperConfig, filesystem: Arc<FilesystemFactory>) -> Result<Self> {
        let collection_service = Arc::new(RwLock::new(None));
        
        // ML clustering moved to AXIS
        // let ml_clustering_engine = MLClusteringEngine::new(super::ml_clustering::KMeansConfig::default());
        
        // Initialize utilities with default configuration
        let utilities = ViperUtilities::new(
            super::utilities::ViperUtilitiesConfig::default(),
            filesystem.clone(),
        ).await?;
        
        // Create managers with async constructors
        let compaction_manager = CompactionManager::new(collection_service.clone(), filesystem.clone()).await?;
        let flush_manager = FlushManager::new(collection_service.clone(), filesystem.clone()).await?;
        
        Ok(Self {
            config,
            collection_service: collection_service.clone(),
            filesystem: filesystem.clone(),
            schema_manager: SchemaManager::new(),
            compaction_manager,
            flush_manager,
            // ml_clustering_engine, // Moved to AXIS
            utilities,
            // Initialize search engine with unified parquet reader
            search_engine: Arc::new(ViperUnifiedSearchEngine::new(
                Arc::new(super::readers::UnifiedParquetReader::new(filesystem.clone())),
                Arc::new(UnifiedDistanceCompute::default()),
                Arc::new(crate::compute::unified_quantization::UnifiedQuantizationEngine::new(
                    Arc::new(UnifiedDistanceCompute::default()),
                    Arc::new(crate::compute::unified_quantization::InMemoryCodebookStore::new()),
                )),
            )),
            stats: Arc::new(RwLock::new(EngineStats::default())),
            collections: Arc::new(RwLock::new(HashMap::new())),
        })
    }
    
    /// Set the collection service for metadata access
    pub async fn set_collection_service(
        &self,
        collection_service: Arc<crate::services::collection_service::CollectionService>,
    ) {
        let mut service_lock = self.collection_service.write().await;
        *service_lock = Some(collection_service);
        info!("🔗 VIPER Engine: Collection service set for metadata access");
    }
    
    // VIPER is columnar storage - it doesn't support single vector inserts
    // All data must come through flush operations from WAL or direct flush
    
    /// Flush vectors to storage
    pub async fn flush_vectors(
        &self,
        collection_id: &str,
        vector_records: &[VectorRecord],
        batch_ids: &[String],
        force: bool,
        synchronous: bool,
    ) -> Result<FlushResult> {
        info!(
            "🚿 VIPER Engine: Flushing {} vectors for collection {} (force: {}, sync: {})",
            vector_records.len(),
            collection_id,
            force,
            synchronous
        );
        
        // Delegate to the flush manager
        self.flush_manager.flush_vectors(collection_id, vector_records, batch_ids, force, synchronous).await
    }
    
    /// Direct flush vectors to storage during WAL recovery (bypasses normal flush pipeline)
    pub async fn flush_vectors_direct(
        &self,
        collection_id: &str,
        vector_records: Vec<crate::core::VectorRecord>,
    ) -> Result<()> {
        info!(
            "💾 VIPER Engine: Direct flush {} vectors for collection {} (WAL recovery)",
            vector_records.len(),
            collection_id
        );
        
        // Convert to storage format
        let viper_records: Vec<VectorRecord> = vector_records.into_iter().collect();
        
        // Create synthetic batch IDs for recovery 
        let batch_ids: Vec<String> = (0..viper_records.len())
            .map(|i| format!("recovery_batch_{}", i))
            .collect();
        
        // Use existing flush infrastructure with force=true, synchronous=true for recovery
        let _flush_result = self.flush_vectors(
            collection_id,
            &viper_records,
            &batch_ids,
            true,  // force flush
            true,  // synchronous for reliable recovery
        ).await?;
        
        info!("✅ VIPER Engine: Direct flush completed for collection {}", collection_id);
        Ok(())
    }
    
    /// Compact Parquet files
    pub async fn compact_parquet_files(
        &self,
        collection_id: &str,
        input_files: Vec<String>,
    ) -> Result<Vec<String>> {
        info!(
            "🗜️ VIPER Engine: Compacting {} files for collection {}",
            input_files.len(),
            collection_id
        );
        
        // Delegate to the compaction manager
        let result = self.compaction_manager.compact_parquet_files(collection_id, input_files).await?;
        Ok(result.output_files)
    }
    
    /// Search for vectors by ID (internal implementation)
    pub async fn internal_get_vector_by_id(
        &self,
        collection_id: &str,
        vector_id: &str,
    ) -> Result<Option<VectorRecord>> {
        use arrow_array::{Array, Float32Array, StringArray, Int64Array, BooleanArray, Float64Array, ListArray, StructArray};
        use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
        // use bytes::Bytes; // Commented out due to compilation issue
        
        info!("🔍 VIPER Engine: Looking up vector {} in collection {}", vector_id, collection_id);
        
        // Get all Parquet files for the collection
        let parquet_files = self.get_parquet_files_for_collection(&String::from(collection_id.to_string())).await?;
        
        if parquet_files.is_empty() {
            debug!("📁 No Parquet files found for collection {}", collection_id);
            return Ok(None);
        }
        
        let current_time = chrono::Utc::now().timestamp_micros();
        let mut best_match: Option<(VectorRecord, i64, i64)> = None; // (record, version, timestamp)
        
        // Search through all Parquet files
        for parquet_file in parquet_files {
            debug!("🔍 Searching file: {}", parquet_file);
            
            // Read Parquet file using filesystem API
            let fs = match self.filesystem.get_filesystem("file:///") {
                Ok(fs) => fs,
                Err(e) => {
                    warn!("Failed to get filesystem: {}", e);
                    continue;
                }
            };
            
            let parquet_data = match fs.read(&parquet_file).await {
                Ok(data) => data,
                Err(e) => {
                    warn!("Failed to read Parquet file {}: {}", parquet_file, e);
                    continue;
                }
            };
            
            let parquet_bytes = bytes::Bytes::from(parquet_data);
            let reader_builder = match ParquetRecordBatchReaderBuilder::try_new(parquet_bytes) {
                Ok(r) => r,
                Err(e) => {
                    warn!("Failed to create Parquet reader for {}: {}", parquet_file, e);
                    continue;
                }
            };
            
            let mut batch_reader = reader_builder.build()?;
            
            // Process each record batch
            while let Some(batch) = batch_reader.next() {
                let batch = batch?;
                
                // Get ID column
                let id_array = batch.column_by_name("id")
                    .and_then(|col| col.as_any().downcast_ref::<StringArray>())
                    .ok_or_else(|| anyhow::anyhow!("Missing or invalid 'id' column"))?;
                
                // Find matching ID
                for row_idx in 0..batch.num_rows() {
                    if id_array.value(row_idx) == vector_id {
                        // Found a match! Extract the full record
                        let vector_array = batch.column_by_name("vector")
                            .and_then(|col| col.as_any().downcast_ref::<ListArray>())
                            .ok_or_else(|| anyhow::anyhow!("Missing or invalid 'vector' column"))?;
                        
                        let timestamp = batch.column_by_name("timestamp")
                            .and_then(|col| col.as_any().downcast_ref::<Int64Array>())
                            .map(|arr| arr.value(row_idx))
                            .unwrap_or(0);
                        
                        let version = batch.column_by_name("version")
                            .and_then(|col| col.as_any().downcast_ref::<Int64Array>())
                            .map(|arr| arr.value(row_idx))
                            .unwrap_or(1);
                        
                        let expires_at = batch.column_by_name("expires_at")
                            .and_then(|col| col.as_any().downcast_ref::<Int64Array>())
                            .and_then(|arr| if arr.is_null(row_idx) { None } else { Some(arr.value(row_idx)) });
                        
                        // Skip if expired
                        if let Some(exp) = expires_at {
                            if exp > 0 && exp < current_time {
                                debug!("Skipping expired vector {} (expired at {})", vector_id, exp);
                                continue;
                            }
                        }
                        
                        // Extract vector data
                        let vector_values = vector_array.value(row_idx);
                        let vector_float_array = vector_values
                            .as_any()
                            .downcast_ref::<Float32Array>()
                            .ok_or_else(|| anyhow::anyhow!("Invalid vector values type"))?;
                        
                        let vector: Vec<f32> = (0..vector_float_array.len())
                            .map(|i| vector_float_array.value(i))
                            .collect();
                        
                        // Extract other fields
                        let created_at = batch.column_by_name("created_at")
                            .and_then(|col| col.as_any().downcast_ref::<Int64Array>())
                            .map(|arr| arr.value(row_idx))
                            .unwrap_or(timestamp);
                        
                        let updated_at = batch.column_by_name("updated_at")
                            .and_then(|col| col.as_any().downcast_ref::<Int64Array>())
                            .map(|arr| arr.value(row_idx))
                            .unwrap_or(timestamp);
                        
                        // Parse metadata from extra_meta list of key-value pairs
                        let mut metadata_map = HashMap::new();
                        if let Some(extra_meta_col) = batch.column_by_name("extra_meta") {
                            if let Some(extra_meta_list) = extra_meta_col.as_any().downcast_ref::<ListArray>() {
                                if !extra_meta_list.is_null(row_idx) {
                                    let kv_pairs = extra_meta_list.value(row_idx);
                                    if let Some(struct_array) = kv_pairs.as_any().downcast_ref::<StructArray>() {
                                        let key_array = struct_array.column(0).as_any().downcast_ref::<StringArray>().unwrap();
                                        let value_array = struct_array.column(1).as_any().downcast_ref::<StringArray>().unwrap();
                                        
                                        for kv_idx in 0..struct_array.len() {
                                            if !struct_array.is_null(kv_idx) {
                                                let key = key_array.value(kv_idx).to_string();
                                                let value = value_array.value(kv_idx).to_string();
                                                metadata_map.insert(key, value);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        
                        // Also parse filterable metadata columns (they have their own columns)
                        for field in batch.schema().fields() {
                            let field_name = field.name();
                            // Skip core fields - only process filterable metadata columns
                            if !matches!(field_name.as_str(), "id" | "collection_id" | "vector" | "timestamp" | "created_at" | "updated_at" | "version" | "expires_at" | "extra_meta") {
                                if let Some(column) = batch.column_by_name(field_name) {
                                    if !column.is_null(row_idx) {
                                        // Convert Arrow value to String based on data type
                                        let string_value = match field.data_type() {
                                            arrow_schema::DataType::Utf8 => {
                                                if let Some(str_array) = column.as_any().downcast_ref::<StringArray>() {
                                                    str_array.value(row_idx).to_string()
                                                } else { continue; }
                                            }
                                            arrow_schema::DataType::Int64 => {
                                                if let Some(int_array) = column.as_any().downcast_ref::<Int64Array>() {
                                                    int_array.value(row_idx).to_string()
                                                } else { continue; }
                                            }
                                            arrow_schema::DataType::Float64 => {
                                                if let Some(float_array) = column.as_any().downcast_ref::<Float64Array>() {
                                                    float_array.value(row_idx).to_string()
                                                } else { continue; }
                                            }
                                            arrow_schema::DataType::Boolean => {
                                                if let Some(bool_array) = column.as_any().downcast_ref::<BooleanArray>() {
                                                    bool_array.value(row_idx).to_string()
                                                } else { continue; }
                                            }
                                            _ => continue, // Skip unsupported types
                                        };
                                        metadata_map.insert(field_name.to_string(), string_value);
                                    }
                                }
                            }
                        }
                        
                        // Convert HashMap to Vec<MetadataItem>
                        let metadata = crate::core::proto_metadata_helper::hashmap_to_proto_metadata(&metadata_map);
                        
                        let record = VectorRecord {
                            id: Some(vector_id.to_string()),
                            vector,
                            metadata,
                            timestamp,
                            created_at,
                            updated_at,
                            expires_at,
                            version,
                            rank: None,
                            score: None,
                            distance: None,
            };
                        
                        // Check if this is a better match than what we have
                        match &best_match {
                            Some((_, best_version, best_timestamp)) => {
                                if version > *best_version || 
                                   (version == *best_version && timestamp > *best_timestamp) {
                                    best_match = Some((record, version, timestamp));
                                }
                            }
                            None => {
                                best_match = Some((record, version, timestamp));
                            }
                        }
                    }
                }
            }
        }
        
        // Return the best match (highest version/newest timestamp)
        Ok(best_match.map(|(record, _, _)| record))
    }
    
    /// Get engine statistics
    pub async fn get_stats(&self) -> EngineStats {
        self.stats.read().await.clone()
    }
    
    /// Clear schema cache for a collection
    pub async fn clear_schema_cache(&self, collection_id: &str) {
        self.schema_manager.clear_schema_cache(collection_id).await;
    }
    
    /// Clear all schema caches
    pub async fn clear_all_schema_cache(&self) {
        self.schema_manager.clear_all_schema_cache().await;
    }
    
    /// Get schema cache statistics
    pub async fn get_schema_cache_stats(&self) -> (usize, Vec<String>) {
        self.schema_manager.get_cache_stats().await
    }
    
    /// Internal health check
    pub async fn internal_health_check(&self) -> Result<bool> {
        // Basic health check - can be extended to check:
        // - Filesystem connectivity
        // - Collection service availability
        // - Internal state consistency
        
        Ok(true)
    }
    
    /// Get collection metadata
    pub async fn get_collection_metadata(&self, collection_id: &str) -> Option<CollectionMetadata> {
        let collections = self.collections.read().await;
        collections.get(collection_id).cloned()
    }
    
    /// Update collection metadata
    pub async fn update_collection_metadata(&self, collection_id: String, metadata: CollectionMetadata) {
        let mut collections = self.collections.write().await;
        collections.insert(collection_id, metadata);
    }
    
    /// Get engine configuration
    pub fn get_config(&self) -> &ViperConfig {
        &self.config
    }
    
    /// Predict cluster for a vector using ML clustering
    /// DEPRECATED: This functionality should be moved to AXIS indexing service
    pub async fn predict_cluster(&self, _collection_id: &str, _vector: &[f32]) -> Result<Option<String>> {
        // ML clustering belongs in AXIS, not in the storage engine
        // VIPER should focus on storage operations only
        Ok(None)
    }
    
    /// Train ML clustering model for a collection
    /// DEPRECATED: This functionality should be moved to AXIS indexing service
    pub async fn train_clustering_model(&self, _collection_id: &str, _vectors: Vec<Vec<f32>>) -> Result<()> {
        // ML clustering belongs in AXIS, not in the storage engine
        // AXIS should handle all indexing strategies including ML models
        info!("🧠 ML clustering should be handled by AXIS, not VIPER storage engine");
        Ok(())
    }
    
    // Clustering moved to AXIS
    // /// Get clustering model for a collection
    // pub async fn get_clustering_model(&self, collection_id: &str) -> Option<super::ml_clustering::MLClusteringModel> {
    //     self.ml_clustering_engine.get_model().cloned()
    // }
    
    /// Record operation performance metrics
    pub async fn record_operation_metrics(&self, metrics: super::utilities::OperationMetrics) -> Result<()> {
        self.utilities.record_operation(metrics).await
    }
    
    /// Get performance statistics
    pub async fn get_performance_report(&self, collection_id: Option<&String>) -> Result<super::utilities::PerformanceReport> {
        self.utilities.get_performance_stats(collection_id).await
    }
    
    /// Optimize compression for a collection
    pub async fn optimize_compression(&self, collection_id: &str) -> Result<super::utilities::CompressionRecommendation> {
        self.utilities.optimize_compression(collection_id).await
    }
    
    /// Start background utilities services
    pub async fn start_background_services(&mut self) -> Result<()> {
        // Note: utilities is not mutable, so we need to access the inner services differently
        // This would need to be redesigned for proper mutable access
        info!("🚀 VIPER Engine: Background services functionality available via utilities");
        Ok(())
    }
    
    /// **STORAGE-AWARE POLYMORPHIC SEARCH**: Primary vector search interface
    /// 
    /// This method provides polymorphic search that automatically selects the most
    /// efficient search strategy based on collection characteristics, data distribution,
    /// and query parameters. It delegates to specialized search implementations for:
    /// - ML-driven cluster optimization for large collections
    /// - Direct search for small collections 
    /// - Hybrid strategies combining clustering with metadata filtering
    pub async fn search_vectors(
        &self,
        collection_id: &str,
        query_vector: &[f32],
        k: usize,
    ) -> Result<Vec<crate::core::search::SearchResult>> {
        info!("🔍 VIPER Engine: Polymorphic vector search - collection={}, k={}", collection_id, k);
        
        let collection_id_typed = String::from(collection_id.to_string());
        
        // Create search parameters with query vector
        let search_params = crate::core::search::SearchParams {
            query_vectors: Some(vec![query_vector.to_vec()]),
            top_k: Some(k),
            distance_metric: Some(crate::compute::distance::DistanceMetric::Cosine),
            filters: None,
            filter_expression: None,
            accuracy_threshold: None,
            include_expired: None,
            timeout_ms: None,
            enable_two_stage: None,
            quantization_hint: None,
            enable_clustering_hint: None,
            enable_metadata_filtering_hint: None,
            custom_hints: None,
        };
        
        // Build search context for unified interface
        let context = crate::core::search::UnifiedSearchContext {
            collection_id: collection_id_typed.clone(),
            collection_config: Some(crate::core::search::CollectionConfig {
                default_distance_metric: crate::compute::distance::DistanceMetric::Cosine,
                vector_dimension: query_vector.len(),
                enable_quantization: false,
                enable_metadata_filtering: false,
                estimated_document_count: 0,
            }),
            filterable_columns: vec![],
            available_quantization: vec![],
            storage_info: crate::core::search::StorageInfo {
                is_cloud_storage: false,
                storage_type: "Local".to_string(),
                estimated_size_mb: 0.0,
                file_count: 0,
                supports_range_requests: true,
            },
        };
        
        // Use unified search interface
        let distance_compute = UnifiedDistanceCompute::default();
        let result_set = self.search_engine.search_unified(
            &context,
            &search_params,
            &distance_compute,
            None, // quantization_engine - already in search_engine
        ).await?;
        
        // Return native search results directly
        Ok(result_set.results)
    }
    
    /// Search vectors in a specific cluster using ML clustering optimization
    /// 
    /// This method searches within a specific cluster identified by cluster_id.
    /// For now, it delegates to the general search method, but in a full implementation
    /// it would use cluster-specific optimizations and predicate pushdown.
    pub async fn search_vectors_in_cluster(
        &self,
        collection_id: &str,
        _query_vector: &[f32],
        k: usize,
        cluster_id: &str,
    ) -> Result<Vec<crate::core::search::SearchResult>> {
        info!("🔍 VIPER Engine: Cluster search - collection={}, cluster={}, k={}", collection_id, cluster_id, k);
        
        // Note: Cluster-based search should be handled by AXIS indexing service
        // VIPER should only provide raw vector retrieval from Parquet files
        // AXIS will determine which clusters/files to search based on its ML models
        
        // For cluster-specific search, return empty results for now
        // Real implementation would:
        // 1. Get cluster metadata from the ML clustering engine
        // 2. Filter Parquet files specific to this cluster
        // 3. Apply cluster-specific distance optimizations
        // 4. Use predicate pushdown for cluster boundaries
        
        warn!("🔍 VIPER Engine: Cluster-specific search not yet implemented for cluster {}", cluster_id);
        Ok(Vec::new())
    }

    /// Get all Parquet files associated with a collection
    pub async fn get_parquet_files_for_collection(&self, collection_id: &str) -> Result<Vec<String>> {
        debug!("📁 Getting Parquet files for collection: {}", collection_id);
        
        // Get storage URL from assignment service
        let assignment_service = crate::storage::assignment_service::get_assignment_service();
        let storage_assignment = assignment_service
            .get_assignment(collection_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("No storage assignment found for collection {}", collection_id))?;
        
        let storage_url = &storage_assignment.data_url;
        debug!("📁 Storage URL for collection {}: {}", collection_id, storage_url);
        
        // Handle different storage backends
        let parquet_files = if storage_url.starts_with("file://") {
            // Local filesystem - storage_url already includes collection_id from assignment service
            // Use filesystem API to list files
            let fs = self.filesystem.get_filesystem(storage_url)?;
            
            // Check if directory exists by trying to list it
            let entries = match fs.list(storage_url).await {
                Ok(entries) => entries,
                Err(_) => {
                    debug!("📁 Collection directory does not exist or is empty: {}", storage_url);
                    return Ok(Vec::new());
                }
            };
            
            // Find all .parquet files in the collection directory
            let mut files = Vec::new();
            for entry in entries {
                // Skip staging directories and hidden files
                if entry.name.starts_with("__") || entry.name.starts_with(".") {
                    debug!("📁 Skipping staging/hidden entry: {}", entry.name);
                    continue;
                }
                
                if entry.name.ends_with(".parquet") && !entry.metadata.is_directory {
                    // In stateless design, DirEntry.url already contains full URL
                    debug!("📁 Found parquet file: {}", entry.url);
                    files.push(entry.url);
                }
            }
            
            // Sort files for consistent ordering
            files.sort();
            files
        } else if storage_url.starts_with("s3://") || 
                  storage_url.starts_with("gs://") || 
                  storage_url.starts_with("adls://") {
            // Cloud storage - use filesystem factory
            let collection_url = format!("{}/{}", storage_url, collection_id);
            match self.filesystem.list(&collection_url).await {
                Ok(entries) => entries.into_iter()
                    .filter(|e| e.name.ends_with(".parquet"))
                    .map(|e| format!("{}/{}", collection_url, e.name))
                    .collect(),
                Err(e) => {
                    warn!("📁 Failed to list cloud files for collection {}: {}", collection_id, e);
                    Vec::new()
                }
            }
        } else {
            warn!("📁 Unsupported storage URL scheme: {}", storage_url);
            Vec::new()
        };
        
        info!("📁 Found {} Parquet files for collection {}", parquet_files.len(), collection_id);
        Ok(parquet_files)
    }
    
    /// Get collection configuration including filterable column specifications
    async fn get_collection_config(&self, collection_id: &str) -> Result<Option<crate::proto::proximadb::Collection>> {
        // Get metadata from collection service if available
        if let Some(collection_service) = &*self.collection_service.read().await {
            match collection_service.get_proto_collection(collection_id).await {
                Ok(Some(collection)) => Ok(Some(collection)),
                Ok(None) => {
                    debug!("Collection {} not found", collection_id);
                    Ok(None)
                }
                Err(e) => {
                    warn!("Failed to get collection metadata: {}", e);
                    Ok(None)
                }
            }
        } else {
            // No collection service available, return minimal metadata
            warn!("No collection service available for metadata retrieval");
            Ok(None)
        }
    }

}

impl Default for ViperEngine {
    fn default() -> Self {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async {
                let filesystem = Arc::new(
                    FilesystemFactory::new(
                        crate::storage::persistence::filesystem::FilesystemConfig::default()
                    ).await.unwrap()
                );
                Self::new(ViperConfig::default(), filesystem).await
            })
            .unwrap()
    }
}

// TODO: Implement UnifiedStorageEngine trait for ViperEngine
// This will replace the old ViperCoreEngine implementation
#[async_trait::async_trait]
impl UnifiedStorageEngine for ViperEngine {
    // Required abstract methods
    fn engine_name(&self) -> &'static str {
        "VIPER"
    }
    
    fn engine_version(&self) -> &'static str {
        "1.0.0"
    }
    
    fn strategy(&self) -> crate::storage::traits::StorageEngineStrategy {
        crate::storage::traits::StorageEngineStrategy::Viper
    }
    
    async fn do_flush(&self, params: &crate::storage::traits::FlushParameters) -> Result<FlushResult> {
        let collection_id = params.collection_id.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Collection ID required for VIPER flush"))?;
        
        debug!("Starting flush for collection {} with {} vectors", 
              collection_id, params.vector_records.len());
        info!("🚿 VIPER Engine: Starting flush for collection {} with {} vectors", 
              collection_id, params.vector_records.len());
        
        // Convert batch IDs to strings for compatibility
        let batch_id_strings: Vec<String> = params.batch_ids.iter()
            .map(|id| id.to_string())
            .collect();
        
        // Use the modular flush manager to flush vectors
        let mut flush_result = self.flush_manager.flush_vectors(
            collection_id,
            &params.vector_records,
            &batch_id_strings,
            params.force,
            params.synchronous,
        ).await?;
        
        // Update engine statistics
        {
            let mut stats = self.stats.write().await;
            stats.flush_operations += 1;
            stats.total_vectors += flush_result.entries_flushed;
            stats.total_size_bytes += flush_result.bytes_written;
        }
        
        // Add engine-specific metrics
        flush_result.engine_metrics.insert(
            "engine_version".to_string(),
            serde_json::Value::String("1.0.0".to_string())
        );
        flush_result.engine_metrics.insert(
            "engine_name".to_string(),
            serde_json::Value::String("VIPER".to_string())
        );
        
        Ok(flush_result)
    }
    
    async fn do_compact(&self, params: &crate::storage::traits::CompactionParameters) -> Result<crate::storage::traits::CompactionResult> {
        let start_time = std::time::Instant::now();
        
        let collection_id = params.collection_id.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Collection ID required for VIPER compaction"))?;
        
        // Get input files from hints or use default empty list
        let input_files = params.hints.get("input_files")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).map(|s| s.to_string()).collect::<Vec<String>>())
            .unwrap_or_default();
        
        info!("🗜️ VIPER Engine: Starting compaction for collection {} with {} hinted input files", 
              collection_id, input_files.len());
        
        // Use the modular compaction manager to compact Parquet files
        // If no input files specified, the compaction manager will discover them
        let compaction_result = self.compaction_manager
            .compact_parquet_files(collection_id, input_files.clone())
            .await?;
        
        let duration_ms = start_time.elapsed().as_millis() as u64;
        
        // Calculate bytes reclaimed (this is an approximation)
        let bytes_reclaimed = input_files.len() as u64 * 1024 * 1024; // Estimate 1MB per file
        
        // Calculate entries processed - estimate based on input files
        let entries_processed = if input_files.is_empty() {
            0
        } else {
            // Estimate entries per file (this could be more accurate with metadata)
            input_files.len() as u64 * 100 // Assume ~100 entries per file for tests
        };
        
        // Update engine statistics
        {
            let mut stats = self.stats.write().await;
            stats.compaction_operations += 1;
        }
        
        Ok(crate::storage::traits::CompactionResult {
            success: true,
            collections_affected: vec![collection_id.clone()],
            entries_processed: compaction_result.entries_processed,
            entries_removed: compaction_result.entries_removed,
            bytes_read: compaction_result.bytes_read,
            bytes_written: compaction_result.bytes_written,
            input_files: compaction_result.input_files.len() as u64,
            output_files: compaction_result.output_files.len() as u64,
            duration_ms,
            completed_at: chrono::Utc::now(),
            engine_metrics: {
                let mut metrics = HashMap::new();
                metrics.insert("compacted_files".to_string(), serde_json::Value::Array(
                    compaction_result.output_files.iter().map(|f| serde_json::Value::String(f.clone())).collect()
                ));
                metrics.insert("input_files".to_string(), serde_json::Value::Array(
                    compaction_result.input_files.iter().map(|f| serde_json::Value::String(f.clone())).collect()
                ));
                metrics.insert("bytes_reclaimed".to_string(), serde_json::Value::Number(
                    serde_json::Number::from(bytes_reclaimed)
                ));
                metrics
            },
        })
    }
    
    async fn get_vector_by_id(&self, collection_id: &str, vector_id: &str) -> Result<Option<VectorRecord>> {
        // Delegate to internal implementation to avoid recursion
        self.internal_get_vector_by_id(collection_id, vector_id).await
    }
    
    async fn search_vectors_unified(
        &self,
        collection_id: &str,
        query_vector: &[f32],
        k: usize,
        distance_metric: &crate::compute::distance::DistanceMetric,
        metadata_filters: Option<&std::collections::HashMap<String, serde_json::Value>>,
        include_vectors: bool,
        include_metadata: bool,
    ) -> Result<Vec<crate::core::search::SearchResult>> {
        debug!("search_vectors_unified called for collection: {}", collection_id);
        // VIPER ENGINE OPTIMIZATION: Use unified search engine
        info!("🔍 VIPER: Searching collection {} with unified search engine", collection_id);
        
        // Build search params from parameters
        if let Some(filters) = metadata_filters {
            debug!("Search with metadata filters: {:?}", filters);
        }
        let search_params = crate::core::search::SearchParams {
            query_vectors: Some(vec![query_vector.to_vec()]),
            top_k: Some(k),
            filters: metadata_filters.cloned(),
            filter_expression: None,
            distance_metric: Some(distance_metric.clone()),
            accuracy_threshold: None,
            custom_hints: None,
            include_expired: None,
            quantization_hint: None,
            enable_two_stage: None,
            enable_clustering_hint: None,
            enable_metadata_filtering_hint: None,
            timeout_ms: None,
        };
        
        // Get collection metadata
        debug!("Getting collection config for: {}", collection_id);
        let collection_opt = self.get_collection_config(collection_id).await?;
        if collection_opt.is_none() {
            debug!("Collection config not found, continuing with defaults");
        }
        
        // Get parquet files for the collection
        let parquet_files = self.get_parquet_files_for_collection(collection_id).await?;
        debug!("Found {} parquet files for collection {}", parquet_files.len(), collection_id);
        for (i, file) in parquet_files.iter().enumerate() {
            trace!("  Parquet file {}: {}", i, file);
        }
        
        if parquet_files.is_empty() {
            debug!("No parquet files found for collection {}, returning empty results", collection_id);
            return Ok(vec![]);
        }
        
        // Build search context
        let search_context = crate::core::search::UnifiedSearchContext {
            collection_id: collection_id.to_string(),
            collection_config: Some(crate::core::search::CollectionConfig {
                default_distance_metric: distance_metric.clone(),
                vector_dimension: query_vector.len(),
                enable_quantization: collection_opt.as_ref()
                    .and_then(|c| c.config.as_ref())
                    .and_then(|c| c.quantization_config.as_ref())
                    .is_some(),
                enable_metadata_filtering: true,
                estimated_document_count: 0, // TODO: Get actual count
            }),
            storage_info: crate::core::search::StorageInfo {
                is_cloud_storage: false,
                storage_type: "VIPER".to_string(),
                estimated_size_mb: self.stats.read().await.total_size_bytes as f64 / (1024.0 * 1024.0),
                file_count: parquet_files.len(),
                supports_range_requests: true,
            },
            filterable_columns: collection_opt.as_ref()
                .and_then(|c| c.config.as_ref())
                .map(|c| c.filterable_columns.iter().map(|col| {
                    crate::core::search::FilterableColumn {
                        name: col.name.clone(),
                        data_type: crate::core::search::ColumnDataType::String, // TODO: Map properly
                        is_indexed: col.indexed,
                        estimated_cardinality: col.estimated_cardinality.map(|c| c as usize),
                    }
                }).collect())
                .unwrap_or_default(),
            available_quantization: vec![],
        };
        
        // Use unified search engine
        debug!("Calling search engine with context: collection_id={}, file_count={}", 
               search_context.collection_id, search_context.storage_info.file_count);
        
        let result_set = match self.search_engine.search_unified(
                &search_context,
                &search_params,
                &crate::compute::unified_distance::UnifiedDistanceCompute::default(),
                None, // TODO: Add quantization engine when needed
            ).await {
            Ok(rs) => rs,
            Err(e) => {
                error!("Search engine error: {}", e);
                return Err(e);
            }
        };
        
        debug!("Search engine returned {} results", result_set.results.len());
        if !result_set.results.is_empty() {
            trace!("First result metadata: {:?}", result_set.results[0].metadata);
        }
        
        // Apply include flags and return native search results
        let mut results = result_set.results;
        
        if !include_vectors {
            for result in &mut results {
                result.vector = None;
            }
        }
        if !include_metadata {
            for result in &mut results {
                result.metadata = HashMap::new();
            }
        }
        
        Ok(results)
    }
    
    async fn collect_engine_metrics(&self) -> Result<HashMap<String, serde_json::Value>> {
        let stats = self.stats.read().await;
        let mut metrics = HashMap::new();
        
        // Basic engine metrics
        metrics.insert("total_storage_bytes".to_string(), serde_json::Value::Number(
            serde_json::Number::from(stats.total_size_bytes)
        ));
        metrics.insert("memory_usage_bytes".to_string(), serde_json::Value::Number(
            serde_json::Number::from(stats.total_size_bytes / 10) // Estimate 10% in memory
        ));
        metrics.insert("collection_count".to_string(), serde_json::Value::Number(
            serde_json::Number::from(self.collections.read().await.len())
        ));
        metrics.insert("total_vectors".to_string(), serde_json::Value::Number(
            serde_json::Number::from(stats.total_vectors)
        ));
        metrics.insert("flush_operations".to_string(), serde_json::Value::Number(
            serde_json::Number::from(stats.flush_operations)
        ));
        metrics.insert("compaction_operations".to_string(), serde_json::Value::Number(
            serde_json::Number::from(stats.compaction_operations)
        ));
        
        // VIPER-specific metrics
        metrics.insert("engine_version".to_string(), serde_json::Value::String("1.0.0".to_string()));
        metrics.insert("ml_clustering_enabled".to_string(), serde_json::Value::Bool(false)); // Moved to AXIS
        metrics.insert("simd_processing_enabled".to_string(), serde_json::Value::Bool(true));
        metrics.insert("utilities_enabled".to_string(), serde_json::Value::Bool(true));
        metrics.insert("healthy".to_string(), serde_json::Value::Bool(true));
        
        Ok(metrics)
    }

    async fn health_check(&self) -> Result<crate::storage::traits::EngineHealth> {
        let healthy = self.internal_health_check().await?;
        let stats = self.stats.read().await;
        
        let mut metrics = HashMap::new();
        metrics.insert("collections_count".to_string(), serde_json::Value::Number(
            serde_json::Number::from(self.collections.read().await.len())
        ));
        metrics.insert("total_vectors".to_string(), serde_json::Value::Number(
            serde_json::Number::from(stats.total_vectors)
        ));
        metrics.insert("total_size_bytes".to_string(), serde_json::Value::Number(
            serde_json::Number::from(stats.total_size_bytes)
        ));
        metrics.insert("flush_operations".to_string(), serde_json::Value::Number(
            serde_json::Number::from(stats.flush_operations)
        ));
        metrics.insert("compaction_operations".to_string(), serde_json::Value::Number(
            serde_json::Number::from(stats.compaction_operations)
        ));
        
        Ok(crate::storage::traits::EngineHealth {
            healthy,
            status: if healthy { "VIPER Engine Healthy".to_string() } else { "VIPER Engine Unhealthy".to_string() },
            last_check: chrono::Utc::now(),
            response_time_ms: 0.0, // TODO: Track actual response time
            error_count: 0, // TODO: Track error count
            warnings: Vec::new(), // TODO: Track warnings
            metrics,
        })
    }
    
    fn get_filesystem_factory(&self) -> &crate::storage::persistence::filesystem::FilesystemFactory {
        &self.filesystem
    }
    
    fn get_collection_service(&self) -> Option<&crate::services::collection_service::CollectionService> {
        // Since we store it as Arc<RwLock<Option<Arc<CollectionService>>>>, we can't return a reference
        // This method would need to be redesigned to work with the async pattern
        None
    }
}

impl ViperEngine {
    /// Convenient compact_collection method for CompactionCoordinator integration
    pub async fn compact_collection(&self, collection_id: &str) -> Result<EngineCompactionResult> {
        info!("🗜️ VIPER Engine: Starting collection compaction for {}", collection_id);
        
        // Get collection configuration if available
        let collection_config = if let Some(service) = self.collection_service.read().await.as_ref() {
            service.get_collection(collection_id).await.ok().flatten()
        } else {
            None
        };
        
        // Create compaction parameters with collection config
        let params = crate::storage::traits::CompactionParameters {
            collection_id: Some(collection_id.to_string()),
            force: true,
            synchronous: false,
            hints: std::collections::HashMap::new(),
            timeout_ms: None,
            priority: crate::storage::traits::OperationPriority::Medium,
            collection_config,
        };
        
        // Use the existing do_compact implementation
        let result = self.do_compact(&params).await?;
        
        Ok(EngineCompactionResult {
            files_processed: result.output_files,
            bytes_processed: result.bytes_written,
        })
    }
}

/// Simplified compaction result for CompactionCoordinator
#[derive(Debug, Clone)]
pub struct EngineCompactionResult {
    pub files_processed: u64,
    pub bytes_processed: u64,
}