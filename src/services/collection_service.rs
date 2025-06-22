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
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::proto::proximadb::{CollectionConfig, CollectionResponse};
use crate::storage::metadata::backends::filestore_backend::{CollectionRecord, FilestoreMetadataBackend};

/// Collection service for unified business logic
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

    /// Delete collection by name
    pub async fn delete_collection(
        &self,
        collection_name: &str,
    ) -> Result<CollectionServiceResponse> {
        info!("🗑️ Deleting collection: {}", collection_name);
        let start_time = std::time::Instant::now();

        let deleted = self
            .metadata_backend
            .delete_collection_by_name(collection_name)
            .await?;

        if deleted {
            info!(
                "✅ Collection deleted: {} in {}μs",
                collection_name,
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
                error_message: Some(format!("Collection '{}' not found", collection_name)),
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

    #[test]
    fn test_collection_validation() {
        let service = CollectionService {
            metadata_backend: Arc::new(todo!("Mock metadata backend")),
        };

        // Valid config
        let valid_config = CollectionConfig {
            name: "valid_collection".to_string(),
            dimension: 128,
            distance_metric: 1,
            storage_engine: 1,
            indexing_algorithm: 1,
            filterable_metadata_fields: vec![],
            indexing_config: std::collections::HashMap::new(),
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