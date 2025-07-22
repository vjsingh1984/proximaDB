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
use crate::core::config::StorageConfig;
// Using proto types directly - CollectionRecord is obsolete
use crate::storage::assignment_service::{
    get_assignment_service, AssignmentService, StorageComponentType,
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
    storage_config: StorageConfig,
}

impl CollectionService {
    /// Create new collection service with multi-disk coordination
    pub async fn new(
        metadata_backend: Arc<FilestoreMetadataBackend>,
        storage_config: StorageConfig,
    ) -> Result<Self> {
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
            storage_config,
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
        
        // Validate collection name length to prevent collision with IDs
        if config.name.len() < 8 {
            return Ok(CollectionServiceResponse::error(
                "Collection name must be at least 8 characters long".to_string(),
                "INVALID_NAME_LENGTH".to_string(),
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
        // Generate base62 ID from microsecond timestamp with collision detection
        let uuid = self.generate_unique_collection_id().await?;
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
    
    /// ✅ RESOLVE COLLECTION NAME/ID TO COLLECTION ID
    /// This is the key method for collection identifier resolution
    /// - Input: Collection name OR collection ID
    /// - Output: Collection ID (base62) for internal use
    /// - Used by WAL, storage, and index path resolution
    pub async fn resolve_collection_id(&self, identifier: &str) -> Result<Option<String>> {
        tracing::debug!("🔍 Resolving collection identifier: '{}'", identifier);
        
        // Check if this looks like a base62 collection ID (short alphanumeric)
        let _is_likely_id = identifier.len() <= 12 && identifier.chars().all(|c| c.is_alphanumeric());
        
        if let Some(collection) = self.get_proto_collection(identifier).await? {
            let collection_id = collection.id;
            tracing::debug!("✅ Resolved '{}' -> collection_id: '{}'", identifier, collection_id);
            Ok(Some(collection_id))
        } else {
            tracing::debug!("❌ Collection not found: '{}'", identifier);
            Ok(None)
        }
    }
    
    /// ✅ RESOLVE COLLECTION ID TO COLLECTION NAME  
    /// Reverse resolution for user-friendly displays
    pub async fn resolve_collection_name(&self, collection_id: &str) -> Result<Option<String>> {
        if let Some(collection) = self.get_proto_collection(collection_id).await? {
            if let Some(config) = &collection.config {
                return Ok(Some(config.name.clone()));
            }
        }
        Ok(None)
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
            1 => crate::core::IndexingAlgorithm::Hnsw,
            2 => crate::core::IndexingAlgorithm::Ivf,
            3 => crate::core::IndexingAlgorithm::Pq,
            4 => crate::core::IndexingAlgorithm::Flat,
            5 => crate::core::IndexingAlgorithm::Annoy,
            _ => crate::core::IndexingAlgorithm::Hnsw,
        };
        
        let algorithm_str = match indexing_algorithm {
            crate::core::IndexingAlgorithm::Hnsw => "HNSW",
            crate::core::IndexingAlgorithm::Ivf => "IVF",
            crate::core::IndexingAlgorithm::Pq => "PQ",
            crate::core::IndexingAlgorithm::Flat => "FLAT",
            crate::core::IndexingAlgorithm::Annoy => "ANNOY",
            crate::core::IndexingAlgorithm::Unspecified => "HNSW", // Default to HNSW
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
                    .remove_assignment(&collection_name)
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

    /// Create storage directories for a new collection using unified assignment
    async fn create_storage_directories(
        &self,
        collection_name: &str,
        collection_uuid: &str,
    ) -> Result<Vec<StorageComponentType>> {
        info!(
            "🏗️ Creating storage directories for collection {} (UUID: {})",
            collection_name, collection_uuid
        );

        // Get unified assignment for the collection (only need to do this once)
        let assignment = self
            .assignment_service
            .assign_collection(
                collection_name,
                &self.storage_config.storage_locations,
                &self.storage_config.assignment_config.strategy,
            )
            .await?;

        let mut created_components = Vec::new();

        // Create WAL directories
        if let Ok(filesystem) = self.filesystem_factory.get_filesystem(&assignment.location_url) {
            // Create WAL directory and subdirectories
            if let Err(e) = filesystem.create_dir_all(&assignment.wal_url).await {
                warn!("⚠️ Failed to create WAL directory {}: {}", assignment.wal_url, e);
            } else {
                for subdir in &["logs", "checkpoints"] {
                    let full_path = format!("{}/{}", assignment.wal_url, subdir);
                    if let Err(e) = filesystem.create_dir_all(&full_path).await {
                        warn!("⚠️ Failed to create WAL subdirectory {}: {}", full_path, e);
                    }
                }
                info!("✅ Created WAL storage directory: {}", assignment.wal_url);
                created_components.push(StorageComponentType::Wal);
            }

            // Create data directories
            if let Err(e) = filesystem.create_dir_all(&assignment.data_url).await {
                warn!("⚠️ Failed to create data directory {}: {}", assignment.data_url, e);
            } else {
                for subdir in &["data", "indexes", "metadata"] {
                    let full_path = format!("{}/{}", assignment.data_url, subdir);
                    if let Err(e) = filesystem.create_dir_all(&full_path).await {
                        warn!("⚠️ Failed to create data subdirectory {}: {}", full_path, e);
                    }
                }
                info!("✅ Created data storage directory: {}", assignment.data_url);
                created_components.push(StorageComponentType::Storage);
            }

            // Create index directories  
            if let Err(e) = filesystem.create_dir_all(&assignment.index_url).await {
                warn!("⚠️ Failed to create index directory {}: {}", assignment.index_url, e);
            } else {
                for subdir in &["axis", "hnsw", "ivf"] {
                    let full_path = format!("{}/{}", assignment.index_url, subdir);
                    if let Err(e) = filesystem.create_dir_all(&full_path).await {
                        warn!("⚠️ Failed to create index subdirectory {}: {}", full_path, e);
                    }
                }
                info!("✅ Created index storage directory: {}", assignment.index_url);
                created_components.push(StorageComponentType::Index);
            }
        } else {
            return Err(anyhow::anyhow!(
                "Failed to get filesystem for location: {}",
                assignment.location_url
            ));
        }

        info!(
            "🏗️ Created {} storage components for collection {} at location {}",
            created_components.len(),
            collection_name,
            assignment.location_url
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
                .get_assignment(collection_name)
                .await
            {
                // With collection-first structure, we only need to delete once
                if component_type == StorageComponentType::Wal {
                    // Delete the entire collection directory (includes wal/, data/, index/)
                    let collection_dir = format!("{}/{}", 
                        assignment.location_url.trim_end_matches('/'), 
                        collection_name
                    );

                    match self
                        .filesystem_factory
                        .get_filesystem(&assignment.location_url)
                    {
                        Ok(filesystem) => {
                            // Check if directory exists before attempting to delete
                            match filesystem.exists(&collection_dir).await {
                                Ok(true) => {
                                    // Recursively delete the entire collection directory
                                    match filesystem.delete(&collection_dir).await {
                                        Ok(_) => {
                                            info!(
                                                "✅ Deleted entire collection directory: {}",
                                                collection_dir
                                            );
                                            cleaned_components = 3; // All components deleted
                                            break; // No need to continue loop
                                        }
                                        Err(e) => {
                                            warn!(
                                                "⚠️ Failed to delete collection directory {}: {}",
                                                collection_dir, e
                                            );
                                        }
                                    }
                                }
                                Ok(false) => {
                                    debug!(
                                        "📂 Collection directory {} does not exist (already cleaned up)",
                                        collection_dir
                                    );
                                    cleaned_components = 3; // Count as all cleaned
                                    break; // No need to continue loop
                                }
                                Err(e) => {
                                    warn!(
                                        "⚠️ Failed to check existence of collection directory {}: {}",
                                        collection_dir, e
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            warn!(
                                "⚠️ Failed to get filesystem for {}: {}",
                                assignment.location_url, e
                            );
                        }
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

    // Removed old assignment config methods - no longer needed with unified assignment
    
    /// Generate unique collection ID using base62-encoded seconds with random padding
    /// Format: {base62(seconds)}{random_base62_char}
    async fn generate_unique_collection_id(&self) -> Result<String> {
        use crate::core::base62;
        
        const BASE62_CHARS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
        let base_timestamp = chrono::Utc::now().timestamp() as u64;
        let base_id = base62::encode(base_timestamp);
        
        // Generate initial ID with random padding using rand::random which is Send
        let random_index: u8 = rand::random::<u8>() % 62;
        let random_char = BASE62_CHARS[random_index as usize] as char;
        let id = format!("{}{}", base_id, random_char);
        
        // Check if ID is available
        if !self.metadata_backend.collection_id_exists(&id).await? {
            return Ok(id);
        }
        
        // If collision detected, try different random paddings
        for _ in 0..62 {
            let random_index: u8 = rand::random::<u8>() % 62;
            let random_char = BASE62_CHARS[random_index as usize] as char;
            let try_id = format!("{}{}", base_id, random_char);
            if !self.metadata_backend.collection_id_exists(&try_id).await? {
                return Ok(try_id);
            }
        }
        
        // If still colliding (very unlikely), try bidirectional search with padding
        const MAX_ATTEMPTS: u64 = 100;
        
        for offset in 1..=MAX_ATTEMPTS {
            // Try incrementing seconds
            let inc_timestamp = base_timestamp + offset;
            let inc_base = base62::encode(inc_timestamp);
            let random_index: u8 = rand::random::<u8>() % 62;
            let random_char = BASE62_CHARS[random_index as usize] as char;
            let inc_id = format!("{}{}", inc_base, random_char);
            if !self.metadata_backend.collection_id_exists(&inc_id).await? {
                return Ok(inc_id);
            }
            
            // Try decrementing seconds (if not underflow)
            if base_timestamp > offset {
                let dec_timestamp = base_timestamp - offset;
                let dec_base = base62::encode(dec_timestamp);
                let random_index: u8 = rand::random::<u8>() % 62;
                let random_char = BASE62_CHARS[random_index as usize] as char;
                let dec_id = format!("{}{}", dec_base, random_char);
                if !self.metadata_backend.collection_id_exists(&dec_id).await? {
                    return Ok(dec_id);
                }
            }
        }
        
        // Extremely unlikely case: append another random character
        let random_index: u8 = rand::random::<u8>() % 62;
        let random_suffix = BASE62_CHARS[random_index as usize] as char;
        Ok(format!("{}{}{}", base_id, random_char, random_suffix))
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

        // Use default storage config if not provided
        // In production, this should be provided from the server config
        CollectionService::new(metadata_backend, StorageConfig::default()).await
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
    use crate::proto::proximadb::CollectionConfig;

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

        let storage_config = StorageConfig::default();
        let service = CollectionService::new(backend, storage_config).await.unwrap();

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

        // Test create with valid config
        let result = service.create_collection(&valid_config).await.unwrap();
        assert!(result.success);
        
        // Test empty name
        let empty_name = CollectionConfig {
            name: "".to_string(),
            ..valid_config.clone()
        };
        let result = service.create_collection(&empty_name).await.unwrap();
        assert!(!result.success);
        assert_eq!(result.error_code, Some("INVALID_NAME".to_string()));
        
        // Test short name (less than 8 characters)
        let short_name = CollectionConfig {
            name: "short".to_string(),
            ..valid_config.clone()
        };
        let result = service.create_collection(&short_name).await.unwrap();
        assert!(!result.success);
        assert_eq!(result.error_code, Some("INVALID_NAME_LENGTH".to_string()));
        assert!(result.error_message.unwrap().contains("at least 8 characters"));
        
        // Test exactly 8 characters (should pass)
        let eight_chars = CollectionConfig {
            name: "exactly8".to_string(),
            ..valid_config.clone()
        };
        let result = service.create_collection(&eight_chars).await.unwrap();
        assert!(result.success);
        
        // Test invalid dimension
        let invalid_dimension = CollectionConfig {
            name: "valid_dimension_test".to_string(),
            dimension: 0,
            ..valid_config.clone()
        };
        let result = service.create_collection(&invalid_dimension).await.unwrap();
        assert!(!result.success);
        assert_eq!(result.error_code, Some("INVALID_DIMENSION".to_string()));
    }

    #[tokio::test]
    async fn test_collection_name_length_validation() {
        // Create a minimal test setup
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
        
        let storage_config = StorageConfig::default();
        let service = CollectionService::new(backend, storage_config).await.unwrap();
        
        // Test cases for collection name length
        let test_cases = vec![
            ("", false, "INVALID_NAME"),                    // Empty name
            ("a", false, "INVALID_NAME_LENGTH"),           // 1 char
            ("abc", false, "INVALID_NAME_LENGTH"),         // 3 chars
            ("seven77", false, "INVALID_NAME_LENGTH"),     // 7 chars
            ("exactly8", true, ""),                        // 8 chars (valid)
            ("ninechars", true, ""),                       // 9 chars (valid)
            ("this_is_a_long_collection_name", true, ""),  // Long name (valid)
        ];
        
        for (name, should_succeed, expected_error_code) in test_cases {
            let config = CollectionConfig {
                name: name.to_string(),
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
            
            let result = service.create_collection(&config).await.unwrap();
            
            assert_eq!(
                result.success, should_succeed,
                "Name '{}' validation failed: expected success={}, got success={}",
                name, should_succeed, result.success
            );
            
            if !should_succeed {
                assert_eq!(
                    result.error_code.as_deref(), Some(expected_error_code),
                    "Name '{}' error code mismatch", name
                );
                
                if expected_error_code == "INVALID_NAME_LENGTH" {
                    assert!(
                        result.error_message.as_ref().unwrap().contains("at least 8 characters"),
                        "Error message should mention 8 character requirement"
                    );
                }
            }
        }
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
