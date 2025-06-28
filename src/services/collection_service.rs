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
use chrono::Utc;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::proto::proximadb::{CollectionConfig, CollectionResponse};
use crate::storage::metadata::backends::filestore_backend::{CollectionRecord, FilestoreMetadataBackend};

/// Collection service for unified business logic
#[derive(Debug)]
pub struct CollectionService {
    metadata_backend: Arc<FilestoreMetadataBackend>,
}

impl CollectionService {
    /// Create new collection service
    pub fn new(metadata_backend: Arc<FilestoreMetadataBackend>) -> Self {
        Self { metadata_backend }
    }

    /// Create collection from gRPC request
    pub async fn create_collection_from_grpc(
        &self,
        config: &CollectionConfig,
    ) -> Result<CollectionServiceResponse> {
        info!("🆕 Creating collection: {}", config.name);
        let start_time = std::time::Instant::now();

        // Input validation
        self.validate_collection_config(config)?;

        // Check if collection already exists
        if let Some(_) = self
            .metadata_backend
            .get_collection_record_by_name(&config.name)
            .await?
        {
            return Ok(CollectionServiceResponse {
                success: false,
                collection_uuid: None,
                storage_path: None,
                error_message: Some(format!("Collection '{}' already exists", config.name)),
                error_code: Some("COLLECTION_EXISTS".to_string()),
                processing_time_us: start_time.elapsed().as_micros() as i64,
            });
        }

        // Convert gRPC config to Avro record (minimal translation)
        let record = CollectionRecord::from_grpc_config(config.name.clone(), config)
            .context("Failed to convert gRPC config to Avro record")?;

        let collection_uuid = record.uuid.clone();
        let storage_path = record.storage_path("${base_path}"); // Template - will be filled by storage engine

        // Store in metadata backend
        self.metadata_backend
            .upsert_collection_record(record)
            .await
            .context("Failed to store collection metadata")?;

        info!(
            "✅ Collection created: {} (UUID: {}) in {}μs",
            config.name,
            collection_uuid,
            start_time.elapsed().as_micros()
        );

        Ok(CollectionServiceResponse {
            success: true,
            collection_uuid: Some(collection_uuid),
            storage_path: Some(storage_path),
            error_message: None,
            error_code: None,
            processing_time_us: start_time.elapsed().as_micros() as i64,
        })
    }

    /// Get collection by name
    pub async fn get_collection_by_name(
        &self,
        collection_name: &str,
    ) -> Result<Option<CollectionRecord>> {
        debug!("🔍 Getting collection: {}", collection_name);
        
        self.metadata_backend
            .get_collection_record_by_name(collection_name)
            .await
    }
    
    /// Get collection by name or UUID - handles both transparently
    pub async fn get_collection_by_name_or_uuid(
        &self,
        identifier: &str,
    ) -> Result<Option<CollectionRecord>> {
        debug!("🔍 Getting collection by name or UUID: {}", identifier);
        
        self.metadata_backend
            .get_collection_record_by_name_or_uuid(identifier)
            .await
    }

    /// Get collection UUID by name (for storage operations)
    pub async fn get_collection_uuid(&self, collection_name: &str) -> Result<Option<String>> {
        debug!("🔍 Getting UUID for collection: {}", collection_name);
        
        self.metadata_backend
            .get_collection_uuid_string(collection_name)
            .await
    }

    /// List all collections
    pub async fn list_collections(&self) -> Result<Vec<CollectionRecord>> {
        debug!("📋 Listing all collections");
        
        self.metadata_backend.list_collections(None).await
    }

    /// Delete collection by name or UUID - handles both transparently
    pub async fn delete_collection(
        &self,
        collection_identifier: &str,
    ) -> Result<CollectionServiceResponse> {
        info!("🗑️ Deleting collection: {}", collection_identifier);
        let start_time = std::time::Instant::now();

        let deleted = self
            .metadata_backend
            .delete_collection_by_name_or_uuid(collection_identifier)
            .await?;

        if deleted {
            info!(
                "✅ Collection deleted: {} in {}μs",
                collection_identifier,
                start_time.elapsed().as_micros()
            );

            Ok(CollectionServiceResponse {
                success: true,
                collection_uuid: None,
                storage_path: None,
                error_message: None,
                error_code: None,
                processing_time_us: start_time.elapsed().as_micros() as i64,
            })
        } else {
            Ok(CollectionServiceResponse {
                success: false,
                collection_uuid: None,
                storage_path: None,
                error_message: Some(format!("Collection '{}' not found", collection_identifier)),
                error_code: Some("COLLECTION_NOT_FOUND".to_string()),
                processing_time_us: start_time.elapsed().as_micros() as i64,
            })
        }
    }

    /// Update collection statistics (called by storage engine after vector operations)
    pub async fn update_collection_stats(
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
            .get_collection_record_by_name(collection_name)
            .await?
        {
            record.update_stats(vector_delta, size_delta);
            self.metadata_backend.upsert_collection_record(record).await?;
        } else {
            warn!("⚠️ Attempted to update stats for non-existent collection: {}", collection_name);
        }

        Ok(())
    }

    /// Update collection metadata (description, tags, owner, config, etc.)
    pub async fn update_collection_metadata(
        &self,
        collection_name: &str,
        updates: &std::collections::HashMap<String, serde_json::Value>,
    ) -> Result<CollectionServiceResponse> {
        info!("📝 Updating collection metadata: {}", collection_name);
        let start_time = std::time::Instant::now();

        // Get current record (supports both names and UUIDs)
        let mut record = match self
            .metadata_backend
            .get_collection_record_by_name_or_uuid(collection_name)
            .await?
        {
            Some(record) => record,
            None => {
                return Ok(CollectionServiceResponse {
                    success: false,
                    collection_uuid: None,
                    storage_path: None,
                    error_message: Some(format!("Collection '{}' not found", collection_name)),
                    error_code: Some("COLLECTION_NOT_FOUND".to_string()),
                    processing_time_us: start_time.elapsed().as_micros() as i64,
                });
            }
        };

        // Validate and apply updates
        for (field, value) in updates {
            match field.as_str() {
                "description" => {
                    if let Some(desc_str) = value.as_str() {
                        record.description = Some(desc_str.to_string());
                    } else if value.is_null() {
                        record.description = None;
                    } else {
                        return Ok(CollectionServiceResponse::error(
                            "Description must be a string or null".to_string(),
                            "INVALID_DESCRIPTION".to_string(),
                            start_time.elapsed().as_micros() as i64,
                        ));
                    }
                }
                "tags" => {
                    if let Some(tags_array) = value.as_array() {
                        let mut tags = Vec::new();
                        for tag in tags_array {
                            if let Some(tag_str) = tag.as_str() {
                                tags.push(tag_str.to_string());
                            } else {
                                return Ok(CollectionServiceResponse::error(
                                    "All tags must be strings".to_string(),
                                    "INVALID_TAGS".to_string(),
                                    start_time.elapsed().as_micros() as i64,
                                ));
                            }
                        }
                        record.tags = tags;
                    } else {
                        return Ok(CollectionServiceResponse::error(
                            "Tags must be an array of strings".to_string(),
                            "INVALID_TAGS".to_string(),
                            start_time.elapsed().as_micros() as i64,
                        ));
                    }
                }
                "owner" => {
                    if let Some(owner_str) = value.as_str() {
                        record.owner = Some(owner_str.to_string());
                    } else if value.is_null() {
                        record.owner = None;
                    } else {
                        return Ok(CollectionServiceResponse::error(
                            "Owner must be a string or null".to_string(),
                            "INVALID_OWNER".to_string(),
                            start_time.elapsed().as_micros() as i64,
                        ));
                    }
                }
                "config" => {
                    // Config should be a JSON object that we serialize to string
                    if value.is_object() {
                        record.config = serde_json::to_string(value)
                            .context("Failed to serialize config JSON")?;
                    } else {
                        return Ok(CollectionServiceResponse::error(
                            "Config must be a JSON object".to_string(),
                            "INVALID_CONFIG".to_string(),
                            start_time.elapsed().as_micros() as i64,
                        ));
                    }
                }
                // Immutable fields that cannot be updated (affect embeddings/search)
                "name" | "dimension" | "distance_metric" | "indexing_algorithm" | 
                "storage_engine" | "created_at" | "uuid" | "version" | 
                "vector_count" | "total_size_bytes" | "filterable_metadata_fields" | 
                "filterable_columns" => {
                    return Ok(CollectionServiceResponse::error(
                        format!("Field '{}' cannot be updated (immutable)", field),
                        "IMMUTABLE_FIELD".to_string(),
                        start_time.elapsed().as_micros() as i64,
                    ));
                }
                _ => {
                    return Ok(CollectionServiceResponse::error(
                        format!("Unknown field '{}'", field),
                        "UNKNOWN_FIELD".to_string(),
                        start_time.elapsed().as_micros() as i64,
                    ));
                }
            }
        }

        // Update timestamps and version
        record.updated_at = Utc::now().timestamp_millis();
        record.version += 1;

        // Save updated record
        self.metadata_backend
            .upsert_collection_record(record.clone())
            .await
            .context("Failed to update collection metadata")?;

        info!(
            "✅ Collection metadata updated: {} (UUID: {}) in {}μs",
            collection_name,
            record.uuid,
            start_time.elapsed().as_micros()
        );

        let collection_uuid = record.uuid.clone();
        let storage_path = record.storage_path("${base_path}");

        Ok(CollectionServiceResponse {
            success: true,
            collection_uuid: Some(collection_uuid),
            storage_path: Some(storage_path),
            error_message: None,
            error_code: None,
            processing_time_us: start_time.elapsed().as_micros() as i64,
        })
    }

    /// Convert collection record to gRPC response format
    pub fn record_to_grpc_response(&self, record: &CollectionRecord) -> CollectionConfig {
        record.to_grpc_config()
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
            return Err(anyhow::anyhow!("Collection name too long (max 255 characters)"));
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
}

/// Unified response format for collection operations
#[derive(Debug, Clone)]
pub struct CollectionServiceResponse {
    pub success: bool,
    pub collection_uuid: Option<String>,
    pub storage_path: Option<String>,
    pub error_message: Option<String>,
    pub error_code: Option<String>,
    pub processing_time_us: i64,
}

impl CollectionServiceResponse {
    /// Convert to gRPC CollectionResponse
    pub fn to_grpc_response(&self, operation: i32) -> CollectionResponse {
        CollectionResponse {
            success: self.success,
            operation,
            collection: None, // TODO: Include collection details if needed
            collections: vec![], // For list operations
            affected_count: if self.success { 1 } else { 0 },
            total_count: None,
            metadata: std::collections::HashMap::new(),
            error_message: self.error_message.clone(),
            error_code: self.error_code.clone(),
            processing_time_us: self.processing_time_us,
        }
    }

    /// Create success response
    pub fn success(collection_uuid: String, storage_path: String, processing_time_us: i64) -> Self {
        Self {
            success: true,
            collection_uuid: Some(collection_uuid),
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
            collection_uuid: None,
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

    pub fn build(self) -> Result<CollectionService> {
        let metadata_backend = self
            .metadata_backend
            .ok_or_else(|| anyhow::anyhow!("Metadata backend is required"))?;

        Ok(CollectionService::new(metadata_backend))
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
        use crate::storage::metadata::backends::filestore_backend::{FilestoreMetadataBackend, FilestoreMetadataConfig};
        use crate::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};
        use tempfile::TempDir;
        
        let temp_dir = TempDir::new().unwrap();
        let temp_path = format!("file://{}", temp_dir.path().display());
        
        let filestore_config = FilestoreMetadataConfig {
            filestore_url: temp_path.clone(),
            enable_compression: false,
            enable_backup: false,
            enable_snapshot_archival: false,
            max_archived_snapshots: 1,
            temp_directory: None,
        };
        
        let filesystem_config = FilesystemConfig::default();
        let filesystem_factory = Arc::new(
            FilesystemFactory::new(filesystem_config).await.unwrap()
        );
        
        let backend = Arc::new(
            FilestoreMetadataBackend::new(filestore_config, filesystem_factory).await.unwrap()
        );
        
        let service = CollectionService::new(backend);

        // Valid config
        let valid_config = CollectionConfig {
            name: "valid_collection".to_string(),
            dimension: 128,
            distance_metric: 1,
            storage_engine: 1,
            indexing_algorithm: 1,
            filterable_metadata_fields: vec![],
            indexing_config: std::collections::HashMap::new(),
            filterable_columns: vec![],
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
        assert!(service.validate_collection_config(&invalid_dimension).is_err());
    }

    #[test]
    fn test_response_conversion() {
        let response = CollectionServiceResponse::success(
            "test-uuid".to_string(),
            "/path/to/storage".to_string(),
            1000,
        );

        let grpc_response = response.to_grpc_response(1);
        assert!(grpc_response.success);
        assert_eq!(grpc_response.processing_time_us, 1000);
    }
}