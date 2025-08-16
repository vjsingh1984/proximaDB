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
use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use super::types::CollectionMetadata;
use anyhow::Context;
use std::collections::HashMap as StdHashMap;

// MIGRATION: Import universal quantization adapter
use crate::storage::engines::common::{
    UniversalQuantizationAdapter, UniversalQuantizationConfig,
    quantization_common::{
        ProgressiveQuantizationStage, UniversalQuantizationLevel,
        BinaryThresholdStrategy, CodebookStrategy,
    },
};

/// VIPER Engine - Main coordination point for the modular VIPER storage engine
#[derive(Debug)]
pub struct ViperEngine {
    /// Configuration (internal engine config)
    config: ViperEngineConfig,
    
    /// User-facing core config (for passing to flush operations)
    core_config: crate::core::config::ViperConfig,
    
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
    /// Create a new VIPER engine from user-facing core config
    pub async fn from_core_config(core_config: crate::core::config::ViperConfig, filesystem: Arc<FilesystemFactory>) -> Result<Self> {
        let config = ViperEngineConfig::from_core_config(&core_config);
        Self::new_internal(config, core_config, filesystem).await
    }
    
    /// Standard constructor matching SST engine interface
    /// This provides consistency across storage engines
    /// 
    /// Note: While VIPER can handle multiple collections, it still needs
    /// collection metadata for compression, filterable fields, dimensions, etc.
    /// The collection_id here is used for initial setup if needed.
    pub async fn new(
        collection_id: String,  // Used for logging and initial setup
        core_config: crate::core::config::ViperConfig,
        filesystem: Arc<FilesystemFactory>,
        _distance_compute: Arc<crate::compute::distance_computation::engine::UnifiedDistanceCompute>,  // VIPER creates its own internally
    ) -> Result<Self> {
        info!("🔧 Creating VIPER engine with initial collection: {}", collection_id);
        // VIPER manages multiple collections, so we just log the initial one
        Self::from_core_config(core_config, filesystem).await
    }
    
    /// Constructor with explicit base location (for consistency with SST)
    /// 
    /// Note: VIPER manages storage locations per-collection through collection metadata,
    /// but this constructor is provided for interface consistency with SST engine.
    pub async fn new_with_location(
        collection_id: String,  // Used for logging and initial setup
        core_config: crate::core::config::ViperConfig,
        filesystem: Arc<FilesystemFactory>,
        _distance_compute: Arc<crate::compute::distance_computation::engine::UnifiedDistanceCompute>,
        base_location: String,  // Can be used to override default storage paths
    ) -> Result<Self> {
        info!("🔧 Creating VIPER engine for collection: {} with base location: {}", 
              collection_id, base_location);
        // VIPER gets per-collection storage locations from collection metadata
        // The base_location here could be used as a fallback or override
        Self::from_core_config(core_config, filesystem).await
    }
    
    /// Internal constructor with both configs
    async fn new_internal(config: ViperEngineConfig, core_config: crate::core::config::ViperConfig, filesystem: Arc<FilesystemFactory>) -> Result<Self> {
        let collection_service = Arc::new(RwLock::new(None));
        
        // MIGRATION: Initialize universal quantization adapter (REQUIRED)
        let quantization_adapter = Arc::new(
            UniversalQuantizationAdapter::new()
                .context("Failed to initialize universal quantization adapter")?
        );
        
        // Configure quantization for columnar storage
        let mut quant_config = UniversalQuantizationConfig::default();
        quant_config.enabled = true;
        
        // VIPER-specific quantization stages for columnar data
        quant_config.stages = vec![
            ProgressiveQuantizationStage {
                level: UniversalQuantizationLevel::Binary {
                    threshold_strategy: BinaryThresholdStrategy::Adaptive,
                },
                candidate_reduction: 0.8, // Filter 80% using binary
                quality_threshold: 0.3,
            },
            ProgressiveQuantizationStage {
                level: UniversalQuantizationLevel::Int8 {
                    scale_strategy: crate::storage::engines::common::quantization_common::ScaleStrategy::PerDimensionMinMax,
                    zero_point_strategy: crate::storage::engines::common::quantization_common::ZeroPointStrategy::Symmetric,
                },
                candidate_reduction: 0.5, // Further reduce using INT8
                quality_threshold: 0.85,
            },
            ProgressiveQuantizationStage {
                level: UniversalQuantizationLevel::ProductQuantization {
                    segments: 96, // Default for high-dimensional vectors
                    bits_per_segment: 8,
                    codebook_strategy: CodebookStrategy::KMeans,
                },
                candidate_reduction: 0.0, // Keep all for final ranking
                quality_threshold: 0.95,
            },
        ];
        
        // Add VIPER-specific engine overrides
        quant_config.engine_overrides.insert(
            "viper_columnar_optimization".to_string(),
            serde_json::json!(true)
        );
        quant_config.engine_overrides.insert(
            "viper_parquet_encoding".to_string(),
            serde_json::json!("bit_packed")
        );
        
        quantization_adapter.set_default_config(quant_config);
        
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
            core_config,
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
                Arc::new(crate::compute::quantization::unified::UnifiedQuantizationEngine::new(
                    Arc::new(UnifiedDistanceCompute::default()),
                    Arc::new(crate::compute::quantization::unified::InMemoryCodebookStore::new()),
                )),
            )),
            stats: Arc::new(RwLock::new(EngineStats::default())),
            collections: Arc::new(RwLock::new(HashMap::new())),
            quantization_adapter,
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
        self.flush_manager.flush_vectors(collection_id, vector_records, batch_ids, force, synchronous, &self.core_config, None).await
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
    /// Note: This method requires collection config to be passed, use do_compact for automatic config lookup
    pub async fn compact_parquet_files(
        &self,
        collection_id: &str,
        input_files: Vec<String>,
        collection_config: Option<&crate::proto::proximadb::Collection>,
    ) -> Result<Vec<String>> {
        info!(
            "🗜️ VIPER Engine: Compacting {} files for collection {}",
            input_files.len(),
            collection_id
        );
        
        // Delegate to the compaction manager with collection config
        let result = self.compaction_manager.compact_parquet_files(collection_id, input_files, collection_config).await?;
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
                        let _created_at = batch.column_by_name("created_at")
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
                            timestamp: timestamp as u32,
                            updated_at: Some(updated_at as u32),
                            expires_at: expires_at.map(|v| v as u32),
                            version: Some(version as u32),
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
    
    // 🔴 UNUSED SCHEMA CACHE METHODS - CANDIDATES FOR REMOVAL
    // These schema cache management methods have no callers found in the codebase.
    // Schema caching is managed internally and these public methods are not used.
    /*
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
    */
    
    // 🔴 UNUSED HEALTH CHECK METHOD - CANDIDATE FOR REMOVAL
    // This internal health check method has no callers found.
    // Health checking is handled by the UnifiedStorageEngine trait's health_check method.
    /*
    /// Internal health check
    pub async fn internal_health_check(&self) -> Result<bool> {
        // Basic health check - can be extended to check:
        // - Filesystem connectivity
        // - Collection service availability
        // - Internal state consistency
        
        Ok(true)
    }
    */
    
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
    pub fn get_config(&self) -> &crate::core::config::ViperConfig {
        &self.core_config
    }
    
    
    // 🟡 INTERNAL UTILITY METHODS - CONSIDER MAKING PRIVATE
    // These utility methods have no external callers found in the codebase.
    // They are only used internally and could be made private or moved to internal modules.
    
    /// Record operation performance metrics - INTERNAL USE
    async fn record_operation_metrics(&self, metrics: super::utilities::OperationMetrics) -> Result<()> {
        self.utilities.record_operation(metrics).await
    }
    
    /// Get performance statistics - INTERNAL USE
    async fn get_performance_report(&self, collection_id: Option<&String>) -> Result<super::utilities::PerformanceReport> {
        self.utilities.get_performance_stats(collection_id).await
    }
    
    /// Optimize compression for a collection - INTERNAL USE
    async fn optimize_compression(&self, collection_id: &str) -> Result<super::utilities::CompressionRecommendation> {
        self.utilities.optimize_compression(collection_id).await
    }
    
    /// Start background utilities services - INTERNAL USE
    async fn start_background_services(&mut self) -> Result<()> {
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
    /// Public search method - requires storage URL
    /// 
    /// **Note**: This is a convenience method primarily intended for testing.
    /// Production code should use `search_vectors_unified` via the UnifiedStorageEngine trait
    /// for full control over search parameters including distance metrics and filters.
    /// 
    /// This method requires the storage URL to be passed explicitly.
    pub async fn search_vectors(
        &self,
        collection_id: &str,
        storage_url: &str,
        query_vector: &[f32],
        k: usize,
    ) -> Result<Vec<crate::core::search::SearchResult>> {
        info!("🔍 VIPER Engine: search_vectors called - collection={}, storage_url={}, k={}", collection_id, storage_url, k);
        
        // Delegate to the unified search implementation with default parameters
        self.search_vectors_unified(
            collection_id,
            storage_url,
            query_vector,
            k,
            &crate::compute::distance_computation::DistanceMetric::Cosine, // Default metric
            None, // No filters
            true, // Include vectors
            true, // Include metadata
        ).await
    }
    
    // REMOVED: search_vectors_in_cluster - Clustering is handled by AXIS indexing service
    // VIPER provides raw vector retrieval; AXIS determines which files to search

    /// Get all Parquet files using the provided storage URL
    pub async fn get_parquet_files_with_storage_url(&self, collection_id: &str, storage_url: &str) -> Result<Vec<String>> {
        debug!("📁 Getting Parquet files for collection: {} from URL: {}", collection_id, storage_url);
        info!("🔍 VIPER get_parquet_files: collection_id={}, storage_url={}", collection_id, storage_url);
        debug!("📁 [DEBUG] get_parquet_files_with_storage_url called:");
        debug!("    collection_id: {}", collection_id);
        debug!("    storage_url: {}", storage_url);
        
        // Handle different storage backends
        let parquet_files = if storage_url.starts_with("file://") {
            // The storage_url already contains the full path including collection_id and /data
            // Don't append collection_id again
            let full_path = storage_url.to_string();
            
            info!("📁 Listing local files at: {}", full_path);
            debug!("📁 Listing local files at: {}", full_path);
            debug!("📁 [DEBUG] Listing local files at: {}", full_path);
            
            // Use filesystem API to list files
            match self.filesystem.list(&full_path).await {
                Ok(files) => {
                    debug!("📁 [DEBUG] filesystem.list returned {} entries", files.len());
                    for (i, file) in files.iter().enumerate() {
                        debug!("    [{}] name={}, url={}", i, file.name, file.url);
                    }
                    let parquet_files: Vec<String> = files
                        .into_iter()
                        .filter(|f| f.name.ends_with(".parquet"))
                        .map(|f| f.url)  // Use the full URL from DirEntry
                        .collect();
                    debug!("📁 Found {} Parquet files in {}", parquet_files.len(), full_path);
                    debug!("📁 [DEBUG] Found {} Parquet files", parquet_files.len());
                    for (i, file) in parquet_files.iter().enumerate() {
                        debug!("    [{}] {}", i, file);
                    }
                    parquet_files
                }
                Err(e) => {
                    debug!("📁 Error listing files in {}: {}", full_path, e);
                    vec![]
                }
            }
        } else {
            // Cloud storage - use the storage URL pattern
            debug!("📁 Listing cloud files for collection: {}", collection_id);
            
            match self.filesystem.list(storage_url).await {
                Ok(files) => {
                    let parquet_files: Vec<String> = files
                        .into_iter()
                        .filter(|f| f.name.ends_with(".parquet"))
                        .map(|f| f.url)  // Use the full URL from DirEntry
                        .collect();
                    debug!("📁 Found {} Parquet files in cloud storage", parquet_files.len());
                    parquet_files
                }
                Err(e) => {
                    debug!("📁 Error listing cloud files: {}", e);
                    vec![]
                }
            }
        };
        
        Ok(parquet_files)
    }
    
    /// Get all Parquet files associated with a collection (legacy - uses collection service)
    pub async fn get_parquet_files_for_collection(&self, collection_id: &str) -> Result<Vec<String>> {
        debug!("📁 Getting Parquet files for collection: {}", collection_id);
        
        // Get storage URL from collection metadata
        let collection_service = self.collection_service.read().await;
        let collection_service = collection_service.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Collection service not initialized"))?;
        
        let collection = collection_service.get_collection(collection_id).await?
            .ok_or_else(|| anyhow::anyhow!("Collection {} not found", collection_id))?;
        
        let storage_assignment = collection.storage_assignment
            .ok_or_else(|| anyhow::anyhow!("No storage assignment found for collection {}", collection_id))?;
        
        let storage_url = format!("{}/{}/data", storage_assignment.base_location, collection_id);
        debug!("📁 Storage URL for collection {}: {}", collection_id, storage_url);
        
        // Handle different storage backends
        let parquet_files = if storage_url.starts_with("file://") {
            // Local filesystem - storage_url already includes collection_id from assignment service
            // Use filesystem API to list files
            let fs = self.filesystem.get_filesystem(&storage_url)?;
            
            // Check if directory exists by trying to list it
            let entries = match fs.list(&storage_url).await {
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
                Self::from_core_config(crate::core::config::ViperConfig::default(), filesystem).await
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
        crate::version::PROXIMADB_VERSION
    }
    
    fn strategy(&self) -> crate::storage::traits::StorageEngineStrategy {
        crate::storage::traits::StorageEngineStrategy::Viper
    }
    
    async fn do_flush(&self, params: &crate::storage::traits::FlushParameters) -> Result<FlushResult> {
        let collection_id = params.collection_id.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Collection ID required for VIPER flush"))?;
        
        debug!("🔍 VIPER DO_FLUSH: Checking compression configuration");
        if let Some(ref collection_config) = params.collection_config {
            if let Some(ref config) = collection_config.config {
                if let Some(ref compression) = config.compression {
                    debug!("   ✅ Found compression in collection_config: algorithm={}, level={:?}",
                        compression.algorithm, compression.level);
                } else {
                    debug!("   ⚠️ No compression config in collection_config");
                }
            } else {
                debug!("   ⚠️ No config field in collection");
            }
        } else {
            debug!("   ⚠️ No collection_config in params");
        }
        
        debug!("Starting flush for collection {} with {} vectors", 
              collection_id, params.vector_records.len());
        info!("🚿 VIPER Engine: Starting flush for collection {} with {} vectors", 
              collection_id, params.vector_records.len());
        
        // Convert batch IDs to strings for compatibility
        let batch_id_strings: Vec<String> = params.batch_ids.iter()
            .map(|id| id.to_string())
            .collect();
        
        // Use the modular flush manager to flush vectors with provided collection config
        let mut flush_result = self.flush_manager.flush_vectors(
            collection_id,
            &params.vector_records,
            &batch_id_strings,
            params.force,
            params.synchronous,
            &self.core_config,
            params.collection_config.as_ref(), // Pass collection config from params
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
            serde_json::Value::String(crate::version::PROXIMADB_VERSION.to_string())
        );
        flush_result.engine_metrics.insert(
            "engine_name".to_string(),
            serde_json::Value::String("VIPER".to_string())
        );
        
        // Step 3: Notify EventLog for async AXIS indexing (synchronous acknowledgment)
        let flush_handler = crate::storage::engines::viper::flush_eventlog_integration::ViperFlushHandler::new();
        let file_paths = vec![parquet_path.to_string_lossy().to_string()];
        
        if let Err(e) = flush_handler.notify_flush_complete(params, file_paths, &params.vector_records).await {
            // Log but don't fail the flush - EventLog notification is best-effort
            warn!("⚠️ VIPER: Failed to notify EventLog for AXIS indexing: {}", e);
        } else {
            info!("✅ VIPER: Successfully notified EventLog for AXIS indexing");
        }
        
        Ok(flush_result)
    }
    
    async fn do_compact(&self, params: &crate::storage::traits::CompactionParameters) -> Result<crate::storage::traits::CompactionResult> {
        let start_time = std::time::Instant::now();
        
        debug!("🗜️ VIPER do_compact called with params: collection_id={:?}, force={}, synchronous={}, timeout_ms={:?}", 
               params.collection_id, params.force, params.synchronous, params.timeout_ms);
        
        debug!("🔍 VIPER DO_COMPACT: Checking compression configuration");
        if let Some(ref collection_config) = params.collection_config {
            if let Some(ref config) = collection_config.config {
                if let Some(ref compression) = config.compression {
                    debug!("   ✅ Found compression in collection_config: algorithm={}, level={:?}",
                        compression.algorithm, compression.level);
                } else {
                    debug!("   ⚠️ No compression config in collection_config");
                }
            } else {
                debug!("   ⚠️ No config field in collection");
            }
        } else {
            debug!("   ⚠️ No collection_config in params");
        }
        
        let collection_id = params.collection_id.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Collection ID required for VIPER compaction"))?;
        debug!("🗜️ VIPER compaction collection ID: {}", collection_id);
        
        // Get input files from hints or use default empty list
        let input_files = params.hints.get("input_files")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).map(|s| s.to_string()).collect::<Vec<String>>())
            .unwrap_or_default();
        
        info!("🗜️ VIPER Engine: Starting compaction for collection {} with {} hinted input files", 
              collection_id, input_files.len());
        debug!("🗜️ VIPER input files: {:?}", input_files);
        
        // Use the modular compaction manager to compact Parquet files
        // If no input files specified, the compaction manager will discover them
        // Pass the collection config from parameters to avoid collection service lookups
        debug!("🗜️ VIPER calling compaction_manager.compact_parquet_files");
        debug!("🗜️ VIPER collection_config present: {}", params.collection_config.is_some());
        
        let compaction_result = self.compaction_manager
            .compact_parquet_files(collection_id, input_files.clone(), params.collection_config.as_ref())
            .await;
            
        match &compaction_result {
            Ok(result) => {
                debug!("🗜️ VIPER compaction_manager returned success");
                debug!("🗜️ VIPER compaction result details: {:?}", result);
            }
            Err(e) => {
                debug!("🗜️ VIPER compaction_manager failed: {}", e);
                return Err(anyhow::anyhow!("Compaction failed: {}", e));
            }
        }
        
        let compaction_result = compaction_result?;
        
        let duration_ms = start_time.elapsed().as_millis() as u64;
        
        // Calculate bytes reclaimed (this is an approximation)
        let bytes_reclaimed = input_files.len() as u64 * 1024 * 1024; // Estimate 1MB per file
        
        // Calculate entries processed - estimate based on input files
        let _entries_processed = if input_files.is_empty() {
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
        storage_url: &str,  // Storage URL provided by VectorOperationsService
        query_vector: &[f32],
        k: usize,
        distance_metric: &crate::compute::distance_computation::DistanceMetric,
        filter_expression: Option<&crate::core::search::FilterExpression>,
        include_vectors: bool,
        include_metadata: bool,
    ) -> Result<Vec<crate::core::search::SearchResult>> {
        debug!("search_vectors_unified called for collection: {} with storage_url: {}", collection_id, storage_url);
        // VIPER ENGINE OPTIMIZATION: Use unified search engine
        info!("🔍 VIPER: Searching collection {} with unified search engine at {}", collection_id, storage_url);
        
        // Build search params from parameters
        if let Some(filter_expr) = filter_expression {
            debug!("Search with filter expression: {:?}", filter_expr);
        }
        let search_params = crate::core::search::SearchParams {
            query_vectors: Some(vec![query_vector.to_vec()]),
            top_k: Some(k),
            filter_expression: filter_expression.cloned(),
            distance_metric: Some(distance_metric.clone()),
            accuracy_threshold: None,
            custom_hints: None,
            include_expired: None,
            quantization_hint: None,
            enable_two_stage: None,
            enable_clustering_hint: None,
            enable_metadata_filtering_hint: None,
            timeout_ms: None,
            requires_ordering: None, // Default to None for internal engine searches
        };
        
        // Get collection metadata
        debug!("Getting collection config for: {}", collection_id);
        let collection_opt = self.get_collection_config(collection_id).await?;
        if collection_opt.is_none() {
            debug!("Collection config not found, continuing with defaults");
        }
        
        // Get parquet files for the collection using the provided storage URL
        let parquet_files = self.get_parquet_files_with_storage_url(collection_id, storage_url).await?;
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
                file_paths: Some(parquet_files.clone()),
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
                &crate::compute::distance_computation::engine::UnifiedDistanceCompute::default(),
                None, // TODO: Add quantization engine when needed
            ).await {
            Ok(rs) => {
                rs
            },
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
        let mut results: Vec<crate::core::search::SearchResult> = result_set.results.iter().cloned().collect();
        
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
        metrics.insert("engine_version".to_string(), serde_json::Value::String(crate::version::PROXIMADB_VERSION.to_string()));
        metrics.insert("ml_clustering_enabled".to_string(), serde_json::Value::Bool(false)); // Moved to AXIS
        metrics.insert("simd_processing_enabled".to_string(), serde_json::Value::Bool(true));
        metrics.insert("utilities_enabled".to_string(), serde_json::Value::Bool(true));
        metrics.insert("healthy".to_string(), serde_json::Value::Bool(true));
        
        Ok(metrics)
    }

    async fn health_check(&self) -> Result<crate::storage::traits::EngineHealth> {
        // Assume healthy for now since internal_health_check is commented out
        let healthy = true;
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
    /// Returns enhanced result with vector tracking for AXIS integration
    /// Compact a specific collection - returns standard CompactionResult
    pub async fn compact_collection(&self, collection_id: &str, collection_config: Option<&crate::proto::proximadb::Collection>) -> Result<crate::storage::traits::CompactionResult> {
        info!("🗜️ VIPER Engine: Starting collection compaction for {}", collection_id);
        
        // If collection_config not provided, try to get it from service
        let owned_config = if collection_config.is_none() {
            if let Some(service) = self.collection_service.read().await.as_ref() {
                service.get_collection(collection_id).await.ok().flatten()
            } else {
                None
            }
        } else {
            None
        };
        
        let config_to_use = collection_config.or(owned_config.as_ref());
        
        // Create compaction parameters with collection config
        let params = crate::storage::traits::CompactionParameters {
            collection_id: Some(collection_id.to_string()),
            force: true,
            synchronous: false,
            hints: std::collections::HashMap::new(),
            timeout_ms: None,
            priority: crate::storage::traits::OperationPriority::Medium,
            collection_config: config_to_use.cloned(),
        };
        
        // Use the existing do_compact implementation
        self.do_compact(&params).await
    }
}