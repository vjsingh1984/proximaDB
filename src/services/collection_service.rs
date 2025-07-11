// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Collection Service - Common Business Logic Layer
//!
//! This service provides a unified interface for collection operations that both
//! gRPC and REST handlers can use. It handles:
//! - Minimal translation between gRPC protobuf and Avro records
//! - Business logic validation
//! - Storage coordination with UUID-based paths
//! - Error handling and response formatting
//!
//! ## Design Principles:
//! - Single source of truth using Avro records
//! - Minimal object allocation and translation
//! - UUID-based storage organization
//! - Atomic operations with proper error handling

use anyhow::{Context, Result};
use async_trait::async_trait;
use std::sync::Arc;
use std::collections::HashMap;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};


// Using String directly instead of String alias for proto-first architecture
use crate::proto::proximadb::{CollectionConfig, Collection};
// Using proto types directly - CollectionRecord is obsolete
use crate::storage::assignment_service::{
    get_assignment_service, AssignmentService, StorageAssignmentConfig, StorageComponentType,
};
use crate::storage::metadata::backends::filestore_backend::FilestoreMetadataBackend;
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::traits::CollectionMetadataProvider;

// Proto-first architecture - use crate::proto::proximadb::Collection directly

/// Collection service for unified business logic with multi-disk coordination
pub struct CollectionService {
    metadata_backend: Arc<FilestoreMetadataBackend>,
    assignment_service: Arc<dyn AssignmentService>,
    filesystem_factory: Arc<FilesystemFactory>,
    /// Cache for IndexConfig to avoid repeated deserialization
    index_config_cache: Arc<RwLock<HashMap<String, crate::index::config::IndexConfig>>>,
}

impl CollectionService {
    /// Create new collection service with multi-disk coordination
    pub async fn new(metadata_backend: Arc<FilestoreMetadataBackend>) -> Result<Self> {
        let assignment_service = get_assignment_service();

        let filesystem_factory = Arc::new(
            FilesystemFactory::new(Default::default())
                .await
                .context("Failed to initialize filesystem factory")?,
        );

        Ok(Self {
            metadata_backend,
            assignment_service,
            filesystem_factory,
            index_config_cache: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Create collection - single method for all handlers (REST, gRPC, etc)
    /// Takes native types directly, no proto/avro conversions needed
    pub async fn create_collection(
        &self,
        config: &crate::proto::proximadb::CollectionConfig,
    ) -> Result<CollectionServiceResponse> {
        info!("🆕 Creating collection: {}", config.name);
        let start_time = std::time::Instant::now();

        // Input validation
        if config.name.is_empty() {
            return Ok(CollectionServiceResponse::error(
                "Collection name cannot be empty".to_string(),
                "INVALID_NAME".to_string(),
                start_time.elapsed().as_micros() as i64,
            ));
        }
        
        if config.dimension == 0 || config.dimension > 1_000_000 {
            return Ok(CollectionServiceResponse::error(
                "Invalid dimension: must be between 1 and 1,000,000".to_string(),
                "INVALID_DIMENSION".to_string(),
                start_time.elapsed().as_micros() as i64,
            ));
        }

        // Check if collection already exists
        if let Some(_) = self
            .metadata_backend
            .find_collection(&config.name)
        {
            return Ok(CollectionServiceResponse {
                success: false,
                collection: None,
                storage_path: None,
                error_message: Some(format!("Collection '{}' already exists", config.name)),
                error_code: Some("COLLECTION_EXISTS".to_string()),
                processing_time_us: start_time.elapsed().as_micros() as i64,
            });
        }

        // Create proto collection directly - no Avro conversion needed!
        let uuid = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp_micros();
        
        // Create proto collection with stats
        let proto_collection = Collection {
            id: uuid.clone(),
            config: Some(config.clone()),
            stats: Some(crate::proto::proximadb::CollectionStats {
                vector_count: 0,
                index_size_bytes: 0,
                data_size_bytes: 0,
            }),
            created_at: now,
            updated_at: now,
        };

        // Create storage directories using assignment service
        let storage_assignments = self
            .create_storage_directories(&config.name, &uuid)
            .await
            .context("Failed to create storage directories")?;

        // Store proto collection using protobuf serialization (zero-copy)
        self.metadata_backend
            .upsert_collection_proto(&proto_collection)
            .await
            .context("Failed to store collection metadata")?;

        info!(
            "✅ Collection created: {} (UUID: {}) with storage assignments: {:?} in {}μs",
            config.name,
            uuid,
            storage_assignments.len(),
            start_time.elapsed().as_micros()
        );

        // Use proto collection directly - no conversion needed in proto-first architecture
        
        // Generate storage path template
        let storage_path = format!("${{base_path}}/collections/{}", uuid);
        
        Ok(CollectionServiceResponse {
            success: true,
            collection: Some(proto_collection),  // Direct proto usage - no conversion!
            storage_path: Some(storage_path),
            error_message: None,
            error_code: None,
            processing_time_us: start_time.elapsed().as_micros() as i64,
        })
    }

    
    // Removed legacy conversion methods - use get_proto_collection() directly for proto-first architecture
    
    /// Get the full proto collection with all metadata - direct access to deserialized object
    pub async fn get_proto_collection(&self, identifier: &str) -> Result<Option<Collection>> {
        self.get_native_proto(identifier).await
    }
    
    /// Get Collection by name or UUID
    async fn get_native_proto(&self, identifier: &str) -> Result<Option<Collection>> {
        // Use the metadata backend's get_collection_metadata which handles both name and UUID
        self.metadata_backend.get_collection_metadata(identifier).await
    }
    
    /// Convert Collection to core Collection - direct proto to core mapping
    // Removed second convert_proto_to_collection method - use proto types directly

    /// Get IndexConfig for a collection by name or UUID with caching
    pub async fn get_native_index_config(&self, identifier: &str) -> Result<Option<crate::index::config::IndexConfig>> {
        debug!("🔍 Getting IndexConfig for collection: {}", identifier);

        // Check cache first
        {
            let cache = self.index_config_cache.read().await;
            if let Some(cached_config) = cache.get(identifier) {
                debug!("📋 Retrieved IndexConfig from cache for collection: {}", identifier);
                return Ok(Some(cached_config.clone()));
            }
        }

        if let Some(proto_collection) = self.get_native_proto(identifier).await? {
            let index_config = self.parse_index_config_from_proto(&proto_collection)?;
            
            // Cache the result
            {
                let mut cache = self.index_config_cache.write().await;
                cache.insert(identifier.to_string(), index_config.clone());
                cache.insert(proto_collection.id.clone(), index_config.clone()); // Cache by UUID too
            }
            
            debug!("📋 Cached IndexConfig for collection: {}", identifier);
            Ok(Some(index_config))
        } else {
            Ok(None)
        }
    }

    /// Convert proto IndexConfig to internal IndexConfig
    fn convert_proto_index_config(&self, _proto_config: &crate::proto::proximadb::IndexConfig) -> Result<crate::index::config::IndexConfig> {
        // Extract algorithm name from proto config
        let algorithm_name = match _proto_config.algorithm {
            1 => "HNSW",
            2 => "IVF", 
            3 => "PQ",
            4 => "FLAT",
            5 => "ANNOY",
            _ => "HNSW", // Default to HNSW
        };
        
        // Use the from_proto method that handles all the config extraction
        crate::index::config::IndexConfig::from_proto(_proto_config)
    }

    /// Parse IndexConfig from Collection
    fn parse_index_config_from_proto(&self, proto: &Collection) -> Result<crate::index::config::IndexConfig> {
        // Check if proto has index_config field
        if let Some(config) = proto.config.as_ref() {
            if !config.index_configs.is_empty() {
                // Take the first IndexConfig from proto (index_configs is a Vec)
                if let Some(first_config) = config.index_configs.first() {
                    // Convert from proto IndexConfig to internal IndexConfig
                    return Ok(self.convert_proto_index_config(first_config)?);
                }
            }
        }
        
        // No IndexConfig found, create smart defaults based on algorithm
        let config = proto.config.as_ref().ok_or_else(|| anyhow::anyhow!("Collection has no config"))?;
        let indexing_algorithm = match config.primary_indexing_algorithm {
            1 => crate::core::avro_unified::IndexingAlgorithm::Hnsw,
            2 => crate::core::avro_unified::IndexingAlgorithm::Ivf,
            3 => crate::core::avro_unified::IndexingAlgorithm::Pq,
            4 => crate::core::avro_unified::IndexingAlgorithm::Flat,
            5 => crate::core::avro_unified::IndexingAlgorithm::Annoy,
            _ => crate::core::avro_unified::IndexingAlgorithm::Hnsw,
        };
        
        let algorithm_str = match indexing_algorithm {
            crate::core::avro_unified::IndexingAlgorithm::Hnsw => "HNSW",
            crate::core::avro_unified::IndexingAlgorithm::Ivf => "IVF",
            crate::core::avro_unified::IndexingAlgorithm::Pq => "PQ",
            crate::core::avro_unified::IndexingAlgorithm::Flat => "FLAT",
            crate::core::avro_unified::IndexingAlgorithm::Annoy => "ANNOY",
            crate::core::avro_unified::IndexingAlgorithm::Unspecified => "HNSW", // Default to HNSW
        };
        
        let smart_config = crate::index::config::IndexConfig::create_smart_default(
            algorithm_str,
            config.dimension as usize,
            None, // Collection size hint not available
        );
        
        debug!("📋 Created smart default IndexConfig for collection: {}", config.name);
        Ok(smart_config)
    }

    // Configuration parsing helper methods
    
    /// Get quantization configuration for a collection
    pub async fn get_native_quantization_config(&self, identifier: &str) -> Result<Option<crate::proto::proximadb::QuantizationConfig>> {
        debug!("🔍 Getting quantization config for collection: {}", identifier);
        
        if let Some(proto) = self.get_native_proto(identifier).await? {
            Ok(proto.config.and_then(|c| c.quantization_config))
        } else {
            Ok(None)
        }
    }
    
    /// Get search hints for a collection  
    pub async fn get_native_search_hints(&self, identifier: &str) -> Result<Option<serde_json::Value>> {
        debug!("🔍 Getting search hints for collection: {}", identifier);
        
        if let Some(_proto) = self.get_native_proto(identifier).await? {
            // Extract search hints from proto config
            if let Some(config) = _proto.config.as_ref() {
                // Build search hints from collection configuration
                let mut hints = serde_json::json!({
                    "ef_search": 200,
                    "max_candidates": 500,
                    "use_quantized": config.quantization_config.is_some(),
                    "enable_reranking": true
                });
                
                // Extract hints from index configs
                if let Some(first_index) = config.index_configs.first() {
                    // Override with algorithm-specific parameters
                    if let Some(hnsw_config) = &first_index.hnsw_config {
                        hints["ef_search"] = serde_json::json!(hnsw_config.ef_search);
                        hints["max_candidates"] = serde_json::json!(hnsw_config.ef_search * 2);
                    }
                    if let Some(ivf_config) = &first_index.ivf_config {
                        hints["n_probe"] = serde_json::json!(ivf_config.n_probe);
                        hints["max_candidates"] = serde_json::json!(ivf_config.n_probe * 100);
                    }
                }
                
                // Add storage engine specific hints
                hints["storage_engine"] = match config.storage_engine {
                    1 => serde_json::json!("VIPER"),
                    2 => serde_json::json!("LSM"),
                    _ => serde_json::json!("LSM"),
                };
                
                Ok(Some(hints))
            } else {
                // Return default hints if no config
                Ok(Some(serde_json::json!({
                    "ef_search": 200,
                    "max_candidates": 500,
                    "use_quantized": false,
                    "enable_reranking": true
                })))
            }
        } else {
            Ok(None)
        }
    }
    
    /// Get index parameters for a collection
    pub async fn get_native_index_params(&self, identifier: &str) -> Result<Option<serde_json::Value>> {
        debug!("🔍 Getting index params for collection: {}", identifier);
        
        if let Some(proto) = self.get_native_proto(identifier).await? {
            if let Some(config) = proto.config {
                // Proto uses index_configs (plural) which is a vector of IndexConfig
                Ok(Some(serde_json::to_value(&config.index_configs)?))
            } else {
                Ok(None)
            }
        } else {
            Ok(None)
        }
    }
    
    /// Get storage configuration for a collection
    pub async fn get_native_storage_config(&self, identifier: &str) -> Result<Option<serde_json::Value>> {
        debug!("🔍 Getting storage config for collection: {}", identifier);
        
        if let Some(_proto) = self.get_native_proto(identifier).await? {
            if let Some(config) = _proto.config.as_ref() {
                // Build storage config from proto
                let engine_name = match config.storage_engine {
                    1 => "VIPER",
                    2 => "LSM",
                    _ => "LSM", // Default
                };
                
                let mut storage_config = serde_json::json!({
                    "engine": engine_name,
                    "enable_compression": true,
                    "enable_deduplication": false, // Not exposed in proto yet
                    "enable_multi_tenancy": false,  // Not exposed in proto yet
                });
                
                // Add engine-specific configurations
                if engine_name == "VIPER" {
                    storage_config["parquet_config"] = serde_json::json!({
                        "row_group_size": 65536,
                        "compression": "snappy",
                        "enable_statistics": true,
                        "enable_bloom_filter": true,
                    });
                    
                    // Add quantization config if present
                    if let Some(quant_config) = &config.quantization_config {
                        storage_config["quantization_enabled"] = serde_json::json!(true);
                        storage_config["quantization_level"] = serde_json::json!(quant_config);
                    }
                }
                
                Ok(Some(storage_config))
            } else {
                // Return default storage config
                Ok(Some(serde_json::json!({
                    "engine": "LSM",
                    "enable_compression": true,
                    "enable_deduplication": false,
                    "enable_multi_tenancy": false,
                })))
            }
        } else {
            Ok(None)
        }
    }
    
    // Removed get_full_config - use get_proto_collection() directly for proto-first architecture

    /// List all collections - returns proto Collections directly (proto-first architecture)
    pub async fn list_collections(&self) -> Result<Vec<Collection>> {
        debug!("📋 Listing all collections");
        self.metadata_backend.list_collections().await
    }

    /// Delete collection with comprehensive cleanup across all storage components
    pub async fn delete_collection(
        &self,
        collection_identifier: &str,
    ) -> Result<CollectionServiceResponse> {
        info!("🗑️ Deleting collection: {}", collection_identifier);
        let start_time = std::time::Instant::now();

        // Get collection record first to retrieve UUID and other details
        let collection_record = self
            .metadata_backend
            .find_collection(collection_identifier);

        if let Some(record) = collection_record {
            let collection_uuid = record.id.clone();
            let collection_name = record.config.as_ref()
                .map(|c| c.name.clone())
                .unwrap_or_default();

            info!(
                "🔍 Found collection to delete: {} (UUID: {})",
                collection_name, collection_uuid
            );

            // Step 1: Clean up all storage directories and files
            let cleanup_results = self
                .cleanup_storage_directories(&collection_name, &collection_uuid)
                .await;
            match cleanup_results {
                Ok(cleaned_components) => {
                    info!(
                        "🧹 Cleaned up {} storage components for collection {}",
                        cleaned_components, collection_name
                    );
                }
                Err(e) => {
                    warn!(
                        "⚠️ Some storage cleanup failed for collection {}: {}",
                        collection_name, e
                    );
                    // Continue with metadata deletion even if storage cleanup partially fails
                }
            }

            // Step 2: Remove from assignment service
            for component_type in &[
                StorageComponentType::Wal,
                StorageComponentType::Storage,
                StorageComponentType::Index,
            ] {
                if let Err(e) = self
                    .assignment_service
                    .remove_assignment(&collection_name, *component_type)
                    .await
                {
                    warn!(
                        "⚠️ Failed to remove assignment for {}/{}: {}",
                        collection_name, component_type, e
                    );
                }
            }

            // Step 3: Delete from metadata backend
            self.metadata_backend
                .delete_collection(&collection_name)
                .await?;
            let deleted = true;

            if deleted {
                info!(
                    "✅ Collection deleted: {} (UUID: {}) in {}μs",
                    collection_name,
                    collection_uuid,
                    start_time.elapsed().as_micros()
                );

                Ok(CollectionServiceResponse {
                    success: true,
                    collection: Some(record.clone()), // Include the deleted collection record
                    storage_path: None,
                    error_message: None,
                    error_code: None,
                    processing_time_us: start_time.elapsed().as_micros() as i64,
                })
            } else {
                Ok(CollectionServiceResponse {
                    success: false,
                    collection: Some(record.clone()), // Include the collection that failed to delete
                    storage_path: None,
                    error_message: Some(format!(
                        "Failed to delete collection metadata for '{}'",
                        collection_name
                    )),
                    error_code: Some("METADATA_DELETE_FAILED".to_string()),
                    processing_time_us: start_time.elapsed().as_micros() as i64,
                })
            }
        } else {
            Ok(CollectionServiceResponse {
                success: false,
                collection: None,
                storage_path: None,
                error_message: Some(format!("Collection '{}' not found", collection_identifier)),
                error_code: Some("COLLECTION_NOT_FOUND".to_string()),
                processing_time_us: start_time.elapsed().as_micros() as i64,
            })
        }
    }

    /// Update collection statistics (called by storage engine after vector operations)
    pub async fn update_stats(
        &self,
        collection_name: &str,
        vector_delta: i64,
        size_delta: i64,
    ) -> Result<()> {
        debug!(
            "📊 Updating stats for {}: vectors={:+}, size={:+}",
            collection_name, vector_delta, size_delta
        );

        // Get current record, update stats, and save back
        if let Some(mut record) = self
            .metadata_backend
            .find_collection(collection_name)
        {
            // Update stats manually for Collection
            if let Some(stats) = record.stats.as_mut() {
                stats.vector_count += vector_delta;
                stats.data_size_bytes += size_delta;
            }
            record.updated_at = chrono::Utc::now().timestamp_millis();
            
            self.metadata_backend
                .upsert_collection_proto(&record)
                .await?;
        } else {
            warn!(
                "⚠️ Attempted to update stats for non-existent collection: {}",
                collection_name
            );
        }

        Ok(())
    }

    /// Get collection UUID by name or UUID
    pub async fn get_uuid(&self, collection_id: &str) -> Result<Option<String>> {
        debug!("🔍 Getting UUID for collection: {}", collection_id);
        
        // First check if it's already a UUID
        if uuid::Uuid::parse_str(collection_id).is_ok() {
            // Verify it exists
            if let Some(collection) = self.get_collection(collection_id).await? {
                return Ok(Some(collection.id));
            }
            return Ok(None);
        }
        
        // Otherwise look up by name
        if let Some(collection) = self.get_collection(collection_id).await? {
            Ok(Some(collection.id))
        } else {
            Ok(None)
        }
    }
    
    /// Update collection - type-safe method with native parameters
    pub async fn update_collection(
        &self,
        identifier: &str,
        config_update: Option<CollectionConfig>, // Use native proto type!
    ) -> Result<CollectionServiceResponse> {
        info!("📝 Updating collection: {}", identifier);
        let start_time = std::time::Instant::now();

        // Get current record (supports both names and UUIDs)
        let mut record = match self
            .metadata_backend
            .find_collection(identifier)
        {
            Some(record) => record,
            None => {
                return Ok(CollectionServiceResponse {
                    success: false,
                    collection: None,
                    storage_path: None,
                    error_message: Some(format!("Collection '{}' not found", identifier)),
                    error_code: Some("COLLECTION_NOT_FOUND".to_string()),
                    processing_time_us: start_time.elapsed().as_micros() as i64,
                });
            }
        };

        // Apply updates using native proto types
        if let Some(new_config) = config_update {
            // Merge the new config with existing one to preserve unchanged fields
            if let Some(existing_config) = record.config.as_mut() {
                // Only update fields that are provided in new_config
                if new_config.name != "" {
                    existing_config.name = new_config.name;
                }
                if new_config.dimension > 0 {
                    existing_config.dimension = new_config.dimension;
                }
                if new_config.distance_metric != 0 {
                    existing_config.distance_metric = new_config.distance_metric;
                }
                if new_config.storage_engine != 0 {
                    existing_config.storage_engine = new_config.storage_engine;
                }
                if new_config.description.is_some() {
                    existing_config.description = new_config.description;
                }
                if !new_config.tags.is_empty() {
                    existing_config.tags = new_config.tags;
                }
                if new_config.owner.is_some() {
                    existing_config.owner = new_config.owner;
                }
                if !new_config.filterable_columns.is_empty() {
                    existing_config.filterable_columns = new_config.filterable_columns;
                }
                // Add other fields as needed
            } else {
                // No existing config, use the new one
                record.config = Some(new_config);
            }
        }

        // Update timestamp
        record.updated_at = chrono::Utc::now().timestamp_millis();

        // Store updated record
        self.metadata_backend
            .upsert_collection_proto(&record)
            .await
            .context("Failed to update collection metadata")?;

        info!(
            "✅ Collection updated: {} in {}μs",
            identifier,
            start_time.elapsed().as_micros()
        );

        // Record is already a proto Collection, no conversion needed
        let collection = record;
        
        Ok(CollectionServiceResponse {
            success: true,
            collection: Some(collection),
            storage_path: None,
            error_message: None,
            error_code: None,
            processing_time_us: start_time.elapsed().as_micros() as i64,
        })
    }

    /// Get access to the metadata backend for recovery operations
    pub fn get_metadata_backend(&self) -> &Arc<FilestoreMetadataBackend> {
        &self.metadata_backend
    }

    /// Validate collection configuration
    fn validate_collection_config(&self, config: &CollectionConfig) -> Result<()> {
        if config.name.is_empty() {
            return Err(anyhow::anyhow!("Collection name cannot be empty"));
        }

        if config.name.len() > 255 {
            return Err(anyhow::anyhow!(
                "Collection name too long (max 255 characters)"
            ));
        }

        if config.dimension <= 0 {
            return Err(anyhow::anyhow!("Dimension must be positive"));
        }

        if config.dimension > 65536 {
            return Err(anyhow::anyhow!("Dimension too large (max 65536)"));
        }

        // Validate name contains only allowed characters
        if !config
            .name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.')
        {
            return Err(anyhow::anyhow!(
                "Collection name contains invalid characters (only alphanumeric, _, -, . allowed)"
            ));
        }

        Ok(())
    }

    /// Create storage directories for a new collection using assignment service
    async fn create_storage_directories(
        &self,
        collection_name: &str,
        collection_uuid: &str,
    ) -> Result<Vec<StorageComponentType>> {
        info!(
            "🏗️ Creating storage directories for collection {} (UUID: {})",
            collection_name, collection_uuid
        );

        let mut created_components = Vec::new();

        // Define storage components that need assignment service
        // Only assign: WAL, Storage (unified for any engine), and Index
        // Metadata has dedicated storage URL from config
        let storage_components = vec![
            (
                StorageComponentType::Wal,
                self.get_wal_assignment_config().await?,
            ),
            (
                StorageComponentType::Storage,
                self.get_storage_assignment_config().await?,
            ), // Engine-agnostic storage
            (
                StorageComponentType::Index,
                self.get_index_assignment_config().await?,
            ),
        ];

        for (component_type, config) in storage_components {
            match self
                .assignment_service
                .assign_storage_url(&String::from(collection_name), &config)
                .await
            {
                Ok(assignment) => {
                    // Create the directory structure
                    let collection_dir = format!("{}/{}", assignment.storage_url, collection_name);

                    match self
                        .filesystem_factory
                        .get_filesystem(&assignment.storage_url)
                    {
                        Ok(filesystem) => {
                            // Create collection directory (with parent directories)
                            if let Err(e) = filesystem.create_dir_all(&collection_dir).await {
                                warn!(
                                    "⚠️ Failed to create {} directory {}: {}",
                                    component_type, collection_dir, e
                                );
                                continue;
                            }

                            // Create component-specific subdirectories
                            let subdirs = match component_type {
                                StorageComponentType::Wal => vec!["logs", "checkpoints"],
                                StorageComponentType::Storage => {
                                    vec!["data", "indexes", "metadata"]
                                }
                                StorageComponentType::Index => vec!["axis", "hnsw", "ivf"],
                            };

                            for subdir in subdirs {
                                let full_subdir = format!("{}/{}", collection_dir, subdir);
                                if let Err(e) = filesystem.create_dir_all(&full_subdir).await {
                                    warn!(
                                        "⚠️ Failed to create {} subdirectory {}: {}",
                                        component_type, full_subdir, e
                                    );
                                }
                            }

                            info!(
                                "✅ Created {} storage directory: {}",
                                component_type, collection_dir
                            );
                            created_components.push(component_type);
                        }
                        Err(e) => {
                            warn!(
                                "⚠️ Failed to get filesystem for {}: {}",
                                assignment.storage_url, e
                            );
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        "⚠️ Failed to assign storage for {} component: {}",
                        component_type, e
                    );
                }
            }
        }

        info!(
            "🏗️ Created {} storage components for collection {}",
            created_components.len(),
            collection_name
        );
        Ok(created_components)
    }

    /// Clean up storage directories for a deleted collection
    async fn cleanup_storage_directories(
        &self,
        collection_name: &str,
        collection_uuid: &str,
    ) -> Result<usize> {
        info!(
            "🧹 Cleaning up storage directories for collection {} (UUID: {})",
            collection_name, collection_uuid
        );

        let mut cleaned_components = 0;

        // Get assignments for all storage components
        let component_types = vec![
            StorageComponentType::Wal,
            StorageComponentType::Storage,
            StorageComponentType::Index,
        ];

        for component_type in component_types {
            if let Some(assignment) = self
                .assignment_service
                .get_assignment(&String::from(collection_name), component_type)
                .await
            {
                let collection_dir = format!("{}/{}", assignment.storage_url, collection_name);

                match self
                    .filesystem_factory
                    .get_filesystem(&assignment.storage_url)
                {
                    Ok(filesystem) => {
                        // Check if directory exists before attempting to delete
                        match filesystem.exists(&collection_dir).await {
                            Ok(true) => {
                                // Recursively delete the entire collection directory
                                match filesystem.delete(&collection_dir).await {
                                    Ok(_) => {
                                        info!(
                                            "✅ Deleted {} storage directory: {}",
                                            component_type, collection_dir
                                        );
                                        cleaned_components += 1;
                                    }
                                    Err(e) => {
                                        warn!(
                                            "⚠️ Failed to delete {} directory {}: {}",
                                            component_type, collection_dir, e
                                        );
                                    }
                                }
                            }
                            Ok(false) => {
                                debug!(
                                    "📂 {} directory {} does not exist (already cleaned up)",
                                    component_type, collection_dir
                                );
                                cleaned_components += 1; // Count as cleaned
                            }
                            Err(e) => {
                                warn!(
                                    "⚠️ Failed to check existence of {} directory {}: {}",
                                    component_type, collection_dir, e
                                );
                            }
                        }
                    }
                    Err(e) => {
                        warn!(
                            "⚠️ Failed to get filesystem for {}: {}",
                            assignment.storage_url, e
                        );
                    }
                }
            } else {
                debug!(
                    "📂 No assignment found for {}/{} (may not have been created)",
                    collection_name, component_type
                );
            }
        }

        info!(
            "🧹 Cleaned up {} storage components for collection {}",
            cleaned_components, collection_name
        );
        Ok(cleaned_components)
    }

    /// Get WAL assignment configuration from system config
    async fn get_wal_assignment_config(&self) -> Result<StorageAssignmentConfig> {
        // Get configuration from environment or config file
        let base_path = std::env::var("PROXIMADB_DATA_PATH")
            .unwrap_or_else(|_| "/workspace/data".to_string());
        
        // Check for multi-disk configuration
        let disk_count = std::env::var("PROXIMADB_DISK_COUNT")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(3);
        
        let mut storage_urls = Vec::new();
        for i in 1..=disk_count {
            storage_urls.push(format!("file://{}/disk{}/wal", base_path, i));
        }
        
        Ok(StorageAssignmentConfig {
            storage_urls,
            component_type: StorageComponentType::Wal,
            collection_affinity: true,
        })
    }

    /// Get storage assignment configuration (engine-agnostic)
    async fn get_storage_assignment_config(&self) -> Result<StorageAssignmentConfig> {
        Ok(StorageAssignmentConfig {
            storage_urls: vec![
                "file:///workspace/data/disk1/storage".to_string(),
                "file:///workspace/data/disk2/storage".to_string(),
                "file:///workspace/data/disk3/storage".to_string(),
            ],
            component_type: StorageComponentType::Storage,
            collection_affinity: true,
        })
    }

    /// Get Index assignment configuration
    async fn get_index_assignment_config(&self) -> Result<StorageAssignmentConfig> {
        Ok(StorageAssignmentConfig {
            storage_urls: vec![
                "file:///workspace/data/disk1/storage/index".to_string(),
                "file:///workspace/data/disk2/storage/index".to_string(),
                "file:///workspace/data/disk3/storage/index".to_string(),
            ],
            component_type: StorageComponentType::Index,
            collection_affinity: true,
        })
    }
}

impl std::fmt::Debug for CollectionService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CollectionService")
            .field("metadata_backend", &"FilestoreMetadataBackend")
            .field("assignment_service", &"AssignmentService")
            .field("filesystem_factory", &"FilesystemFactory")
            .field("index_config_cache", &"HashMap<String, IndexConfig>")
            .finish()
    }
}

/// Response for collection operations - includes the full collection data
#[derive(Debug, Clone)]
pub struct CollectionServiceResponse {
    pub success: bool,
    pub collection: Option<Collection>,  // Proto-first architecture
    pub storage_path: Option<String>,
    pub error_message: Option<String>,
    pub error_code: Option<String>,
    pub processing_time_us: i64,
}

impl CollectionServiceResponse {

    /// Create success response
    pub fn success(_collection_uuid: String, storage_path: String, processing_time_us: i64) -> Self {
        Self {
            success: true,
            collection: None, // Collection should be passed in if needed
            storage_path: Some(storage_path),
            error_message: None,
            error_code: None,
            processing_time_us,
        }
    }
    
    /// Create success response with collection
    pub fn success_with_collection(collection: Collection, storage_path: String, processing_time_us: i64) -> Self {
        Self {
            success: true,
            collection: Some(collection),
            storage_path: Some(storage_path),
            error_message: None,
            error_code: None,
            processing_time_us,
        }
    }

    /// Create error response
    pub fn error(error_message: String, error_code: String, processing_time_us: i64) -> Self {
        Self {
            success: false,
            collection: None,
            storage_path: None,
            error_message: Some(error_message),
            error_code: Some(error_code),
            processing_time_us,
        }
    }
}

/// Builder for collection service with dependencies
pub struct CollectionServiceBuilder {
    metadata_backend: Option<Arc<FilestoreMetadataBackend>>,
}

impl CollectionServiceBuilder {
    pub fn new() -> Self {
        Self {
            metadata_backend: None,
        }
    }

    pub fn with_metadata_backend(mut self, backend: Arc<FilestoreMetadataBackend>) -> Self {
        self.metadata_backend = Some(backend);
        self
    }

    pub async fn build(self) -> Result<CollectionService> {
        let metadata_backend = self
            .metadata_backend
            .ok_or_else(|| anyhow::anyhow!("Metadata backend is required"))?;

        CollectionService::new(metadata_backend).await
    }
}

impl Default for CollectionServiceBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_collection_validation() {
        // Use filestore backend with temporary directory for testing
        use crate::storage::metadata::backends::filestore_backend::{
            FilestoreMetadataBackend, FilestoreMetadataConfig,
        };
        use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let temp_path = format!("file://{}", temp_dir.path().display());

        let filestore_config = FilestoreMetadataConfig {
            storage_url: temp_path.clone(),
            enable_compression: false,
            enable_snapshots: false,
            snapshot_threshold: 1000,
            keep_snapshots: 3,
            backup_url: None,
            temp_dir: None,
        };

        let filesystem_config = FilesystemConfig::default();
        let filesystem_factory = Arc::new(FilesystemFactory::new(filesystem_config).await.unwrap());

        let backend = Arc::new(
            FilestoreMetadataBackend::new(filestore_config, filesystem_factory)
                .await
                .unwrap(),
        );

        let service = CollectionService::new(backend).await.unwrap();

        // Valid config
        let valid_config = CollectionConfig {
            name: "valid_collection".to_string(),
            dimension: 128,
            distance_metric: 1,
            storage_engine: 1,
            primary_indexing_algorithm: 1,
            filterable_columns: vec![],
            index_configs: vec![],
            quantization_config: None,
            primary_index_name: "default".to_string(),
            enable_automatic_index_selection: false,
            description: Some("Test collection".to_string()),
            tags: vec![],
            owner: Some("test".to_string()),
        };
        assert!(service.validate_collection_config(&valid_config).is_ok());

        // Invalid configs
        let empty_name = CollectionConfig {
            name: "".to_string(),
            ..valid_config.clone()
        };
        assert!(service.validate_collection_config(&empty_name).is_err());

        let invalid_dimension = CollectionConfig {
            dimension: 0,
            ..valid_config.clone()
        };
        assert!(service
            .validate_collection_config(&invalid_dimension)
            .is_err());
    }

    #[test]
    fn test_response_conversion() {
        let response = CollectionServiceResponse::success(
            "test-uuid".to_string(),
            "/path/to/storage".to_string(),
            1000,
        );

        assert!(response.success);
        assert_eq!(response.processing_time_us, 1000);
    }
}

// Implement CollectionMetadataProvider trait to break circular dependency
#[async_trait]
impl CollectionMetadataProvider for CollectionService {
    async fn get_uuid(&self, collection_id: &str) -> Result<Option<String>> {
        // Call the actual implementation method to avoid recursion  
        CollectionService::get_uuid(self, collection_id).await
    }
    
    async fn get_collection_metadata(&self, collection_id: &str) -> Result<Option<Collection>> {
        self.get_native_proto(collection_id).await
    }
    
    async fn get_collection(&self, collection_id: &str) -> Result<Option<Collection>> {
        // Call the actual implementation method to avoid recursion
        CollectionService::get_collection(self, collection_id).await
    }
    
    async fn list_collections(&self) -> Result<Vec<Collection>> {
        self.metadata_backend.list_collections().await
    }
}
