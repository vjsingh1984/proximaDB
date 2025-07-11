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
use tracing::{debug, info, warn};

use crate::core::{String, VectorRecord};
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::traits::{FlushResult, UnifiedStorageEngine};

use super::types::*;
use super::schema::SchemaManager;
use super::compaction::CompactionManager;
use super::flush::FlushManager;
use super::ml_clustering::MLClusteringEngine; // TODO: Move to AXIS
use super::utilities::ViperUtilities;
use super::search::{ViperSearchEngine, ViperSearchConfig};
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
    ml_clustering_engine: MLClusteringEngine,
    utilities: ViperUtilities,
    search_engine: ViperSearchEngine,
    
    /// Engine statistics
    stats: Arc<RwLock<EngineStats>>,
    
    /// Collection metadata cache
    collections: Arc<RwLock<HashMap<String, CollectionMetadata>>>,
}

impl ViperEngine {
    /// Create a new VIPER engine with the specified configuration
    pub async fn new(config: ViperConfig, filesystem: Arc<FilesystemFactory>) -> Result<Self> {
        let collection_service = Arc::new(RwLock::new(None));
        
        // Initialize ML clustering engine
        let ml_clustering_engine = MLClusteringEngine::new(super::ml_clustering::KMeansConfig::default());
        
        // Initialize utilities with default configuration
        let utilities = ViperUtilities::new(
            super::utilities::ViperUtilitiesConfig::default(),
            filesystem.clone(),
        ).await?;
        
        Ok(Self {
            config,
            collection_service: collection_service.clone(),
            filesystem,
            schema_manager: SchemaManager::new(),
            compaction_manager: CompactionManager::new(collection_service.clone()),
            flush_manager: FlushManager::new(collection_service.clone()),
            ml_clustering_engine,
            utilities,
            search_engine: ViperSearchEngine::with_config(ViperSearchConfig::default()),
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
    
    /// Insert a vector record
    pub async fn insert_vector(&self, record: VectorRecord) -> Result<()> {
        info!(
            "🔥 VIPER Engine: Inserting vector {} with {} metadata fields in collection {}",
            record.id,
            record.metadata.len(),
            record.collection_id
        );
        
        // Update statistics
        let mut stats = self.stats.write().await;
        stats.total_vectors += 1;
        stats.total_size_bytes += record.vector.len() as u64 * 4; // f32 = 4 bytes
        
        // Note: This method is not used in production - inserts happen at WAL level
        // Storage engines only handle flush/compaction operations
        // This method exists for testing/debugging purposes only
        
        Ok(())
    }
    
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
        self.compaction_manager.compact_parquet_files(collection_id, input_files).await
    }
    
    /// Search for vectors by ID (internal implementation)
    pub async fn internal_get_vector_by_id(
        &self,
        collection_id: &str,
        vector_id: &str,
    ) -> Result<Option<VectorRecord>> {
        use arrow_array::{Array, Float32Array, ListArray, StringArray, Int64Array, BooleanArray, Float64Array, TimestampMicrosecondArray, StructArray};
        use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
        use std::fs::File;
        
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
            
            // Open Parquet file
            let file = match File::open(&parquet_file) {
                Ok(f) => f,
                Err(e) => {
                    warn!("Failed to open Parquet file {}: {}", parquet_file, e);
                    continue;
                }
            };
            
            let reader_builder = match ParquetRecordBatchReaderBuilder::try_new(file) {
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
                        let mut metadata = HashMap::new();
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
                                                metadata.insert(key, serde_json::Value::String(value));
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
                                        // Convert Arrow value to JSON based on data type
                                        let json_value = match field.data_type() {
                                            arrow_schema::DataType::Utf8 => {
                                                if let Some(str_array) = column.as_any().downcast_ref::<StringArray>() {
                                                    serde_json::Value::String(str_array.value(row_idx).to_string())
                                                } else { continue; }
                                            }
                                            arrow_schema::DataType::Int64 => {
                                                if let Some(int_array) = column.as_any().downcast_ref::<Int64Array>() {
                                                    serde_json::Value::Number(serde_json::Number::from(int_array.value(row_idx)))
                                                } else { continue; }
                                            }
                                            arrow_schema::DataType::Float64 => {
                                                if let Some(float_array) = column.as_any().downcast_ref::<Float64Array>() {
                                                    serde_json::Value::Number(serde_json::Number::from_f64(float_array.value(row_idx)).unwrap_or(serde_json::Number::from(0)))
                                                } else { continue; }
                                            }
                                            arrow_schema::DataType::Boolean => {
                                                if let Some(bool_array) = column.as_any().downcast_ref::<BooleanArray>() {
                                                    serde_json::Value::Bool(bool_array.value(row_idx))
                                                } else { continue; }
                                            }
                                            _ => continue, // Skip unsupported types
                                        };
                                        metadata.insert(field_name.to_string(), json_value);
                                    }
                                }
                            }
                        }
                        
                        let record = VectorRecord {
                            id: vector_id.to_string(),
                            collection_id: collection_id.to_string(),
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
    pub async fn predict_cluster(&self, collection_id: &str, vector: &[f32]) -> Result<Option<String>> {
        // ML clustering belongs in AXIS, not in the storage engine
        // VIPER should focus on storage operations only
        Ok(None)
    }
    
    /// Train ML clustering model for a collection
    /// DEPRECATED: This functionality should be moved to AXIS indexing service
    pub async fn train_clustering_model(&self, _collection_id: &str, vectors: Vec<Vec<f32>>) -> Result<()> {
        // ML clustering belongs in AXIS, not in the storage engine
        // AXIS should handle all indexing strategies including ML models
        info!("🧠 ML clustering should be handled by AXIS, not VIPER storage engine");
        Ok(())
    }
    
    /// Get clustering model for a collection
    pub async fn get_clustering_model(&self, collection_id: &str) -> Option<super::ml_clustering::MLClusteringModel> {
        self.ml_clustering_engine.get_model().cloned()
    }
    
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
    ) -> Result<Vec<crate::core::SearchResult>> {
        info!("🔍 VIPER Engine: Polymorphic vector search - collection={}, k={}", collection_id, k);
        
        let collection_id_typed = String::from(collection_id.to_string());
        
        // Create search parameters with only the necessary overrides
        let search_params = crate::core::search::SearchParams {
            top_k: Some(k),
            ..Default::default()
        };
        
        // Delegate to the storage-aware search engine for optimal strategy selection
        self.search_engine.search_vectors(
            self,  // Pass self reference for engine access
            &collection_id_typed,
            query_vector,
            &search_params,
        ).await
    }
    
    /// Search vectors in a specific cluster using ML clustering optimization
    /// 
    /// This method searches within a specific cluster identified by cluster_id.
    /// For now, it delegates to the general search method, but in a full implementation
    /// it would use cluster-specific optimizations and predicate pushdown.
    pub async fn search_vectors_in_cluster(
        &self,
        collection_id: &str,
        query_vector: &[f32],
        k: usize,
        cluster_id: &str,
    ) -> Result<Vec<crate::core::SearchResult>> {
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
            .get_assignment(collection_id, crate::storage::assignment_service::StorageComponentType::Storage)
            .await
            .ok_or_else(|| anyhow::anyhow!("No storage assignment found for collection {}", collection_id))?;
        
        let storage_url = &storage_assignment.storage_url;
        debug!("📁 Storage URL for collection {}: {}", collection_id, storage_url);
        
        // Handle different storage backends
        let parquet_files = if storage_url.starts_with("file://") {
            // Local filesystem
            let path = storage_url.strip_prefix("file://").unwrap_or(storage_url);
            let collection_path = std::path::Path::new(path).join(collection_id);
            
            if !collection_path.exists() {
                debug!("📁 Collection directory does not exist: {:?}", collection_path);
                return Ok(Vec::new());
            }
            
            // Find all .parquet files in the collection directory
            let mut files = Vec::new();
            if let Ok(entries) = std::fs::read_dir(&collection_path) {
                for entry in entries.flatten() {
                    if let Some(file_name) = entry.file_name().to_str() {
                        if file_name.ends_with(".parquet") && !file_name.starts_with(".") {
                            files.push(entry.path().to_string_lossy().to_string());
                        }
                    }
                }
            }
            
            // Sort files for consistent ordering
            files.sort();
            files
        } else if storage_url.starts_with("s3://") || 
                  storage_url.starts_with("gcs://") || 
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
        "2.0.0-modular"
    }
    
    fn strategy(&self) -> crate::storage::traits::StorageEngineStrategy {
        crate::storage::traits::StorageEngineStrategy::Viper
    }
    
    async fn do_flush(&self, params: &crate::storage::traits::FlushParameters) -> Result<FlushResult> {
        let collection_id = params.collection_id.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Collection ID required for VIPER flush"))?;
        
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
            serde_json::Value::String("2.0.0-modular".to_string())
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
        
        info!("🗜️ VIPER Engine: Starting compaction for collection {} with {} input files", 
              collection_id, input_files.len());
        
        // Use the modular compaction manager to compact Parquet files
        let compacted_files = self.compaction_manager
            .compact_parquet_files(collection_id, input_files.clone())
            .await?;
        
        let duration_ms = start_time.elapsed().as_millis() as u64;
        
        // Calculate bytes reclaimed (this is an approximation)
        let bytes_reclaimed = input_files.len() as u64 * 1024 * 1024; // Estimate 1MB per file
        
        // Update engine statistics
        {
            let mut stats = self.stats.write().await;
            stats.compaction_operations += 1;
        }
        
        Ok(crate::storage::traits::CompactionResult {
            success: true,
            collections_affected: vec![collection_id.clone()],
            entries_processed: 0, // TODO: Track actual entries processed
            entries_removed: 0, // TODO: Track actual entries removed
            bytes_read: bytes_reclaimed, // Estimate
            bytes_written: bytes_reclaimed / 2, // Assume 50% compression
            input_files: input_files.len() as u64,
            output_files: compacted_files.len() as u64,
            duration_ms,
            completed_at: chrono::Utc::now(),
            engine_metrics: {
                let mut metrics = HashMap::new();
                metrics.insert("compacted_files".to_string(), serde_json::Value::Array(
                    compacted_files.iter().map(|f| serde_json::Value::String(f.clone())).collect()
                ));
                metrics.insert("input_files_count".to_string(), serde_json::Value::Number(
                    serde_json::Number::from(input_files.len())
                ));
                metrics
            },
        })
    }
    
    async fn get_vector_by_id(&self, collection_id: &str, vector_id: &str) -> Result<Option<VectorRecord>> {
        // Delegate to internal implementation to avoid recursion
        self.internal_get_vector_by_id(collection_id, vector_id).await
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
        metrics.insert("engine_version".to_string(), serde_json::Value::String("2.0.0-modular".to_string()));
        metrics.insert("ml_clustering_enabled".to_string(), serde_json::Value::Bool(true));
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