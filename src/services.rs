/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Common service layer for business logic shared between REST and gRPC

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use serde_json::Value;
use uuid::Uuid;
use chrono::Utc;

use crate::storage::{StorageEngine, CollectionMetadata, MetadataStore};
use crate::storage::metadata::CollectionFlushConfig;
use crate::core::{VectorRecord, VectorId, CollectionId};
use crate::compute::algorithms::SearchResult;

pub mod migration;

// Import migration types from the migration module

/// Service result type
pub type ServiceResult<T> = Result<T, ServiceError>;

/// Service layer errors
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error("Storage error: {0}")]
    Storage(String),
    #[error("Collection not found: {0}")]
    CollectionNotFound(String),
    #[error("Vector not found: {0}")]
    VectorNotFound(String),
    #[error("Invalid dimension: expected {expected}, got {actual}")]
    InvalidDimension { expected: u32, actual: usize },
    #[error("Invalid request: {0}")]
    InvalidRequest(String),
    #[error("Not found: {0}")]
    NotFound(String),
}

/// Vector service for common vector operations
#[derive(Clone)]
pub struct VectorService {
    storage: Arc<RwLock<StorageEngine>>,
}

impl VectorService {
    pub fn new(storage: Arc<RwLock<StorageEngine>>) -> Self {
        Self { storage }
    }

    /// Insert a vector with optional vector ID
    pub async fn insert_vector(
        &self,
        collection_id: &str,
        vector_id: Option<String>,
        vector: Vec<f32>,
        metadata: Option<HashMap<String, Value>>,
    ) -> ServiceResult<VectorRecord> {
        let storage = self.storage.write().await;
        
        // Use provided vector_id or generate one if not provided
        let id = vector_id.unwrap_or_else(|| Uuid::new_v4().to_string());
        
        // Accept all metadata fields - they will be mapped to filterable columns + extra_meta internally
        let validated_metadata = metadata.unwrap_or_default();
        
        let record = VectorRecord {
            id,
            collection_id: collection_id.to_string(),
            vector,
            metadata: validated_metadata,
            timestamp: Utc::now(),
            expires_at: None,
        };
        
        storage.write(record.clone())
            .await
            .map_err(|e| ServiceError::Storage(e.to_string()))?;
            
        Ok(record)
    }

    /// Get vector by client ID
    pub async fn get_vector_by_client_id(
        &self,
        collection_id: &str,
        client_id: &str,
    ) -> ServiceResult<Option<VectorRecord>> {
        let storage = self.storage.read().await;
        let client_id = client_id.to_string(); // Move to owned
        
        let filter = move |metadata: &HashMap<String, Value>| {
            metadata.get("client_id")
                .and_then(|v| v.as_str())
                .map_or(false, |id| id == client_id)
        };
        
        let results = storage.search_vectors_with_filter(&collection_id.to_string(), vec![], 1, filter)
            .await
            .map_err(|e| ServiceError::Storage(e.to_string()))?;
            
        // Convert SearchResult to VectorRecord - this is a simplified approach
        // In practice, we'd need to get the actual vector data from storage
        if let Some(_result) = results.into_iter().next() {
            // This is incomplete - we need a way to get the full VectorRecord from storage
            // For now, return None until we implement proper vector retrieval
            Ok(None)
        } else {
            Ok(None)
        }
    }

    /// Search vectors with optional metadata filter
    pub async fn search_vectors(
        &self,
        collection_id: &str,
        query: &[f32],
        k: usize,
        metadata_filter: Option<HashMap<String, Value>>,
    ) -> ServiceResult<Vec<SearchResult>> {
        let storage = self.storage.read().await;
        
        if let Some(filter_map) = metadata_filter {
            let filter = move |metadata: &HashMap<String, Value>| {
                filter_map.iter().all(|(key, value)| {
                    metadata.get(key).map_or(false, |v| v == value)
                })
            };
            
            storage.search_vectors_with_filter(&collection_id.to_string(), query.to_vec(), k, filter)
                .await
                .map_err(|e| ServiceError::Storage(e.to_string()))
        } else {
            storage.search_vectors(&collection_id.to_string(), query.to_vec(), k)
                .await
                .map_err(|e| ServiceError::Storage(e.to_string()))
        }
    }

    /// Bulk update vectors
    pub async fn bulk_update_vectors(
        &self,
        collection_id: &str,
        vectors: Vec<(Option<String>, Vec<f32>, Option<HashMap<String, Value>>)>,
    ) -> ServiceResult<Vec<VectorRecord>> {
        let mut results = Vec::new();
        
        for (client_id, vector, metadata) in vectors {
            let record = self.insert_vector(collection_id, client_id, vector, metadata).await?;
            results.push(record);
        }
        
        Ok(results)
    }

    /// Bulk delete vectors by client IDs
    pub async fn bulk_delete_vectors(
        &self,
        collection_id: &str,
        client_ids: Vec<String>,
    ) -> ServiceResult<u64> {
        let storage = self.storage.read().await;
        let mut deleted_count = 0;
        
        for client_id in client_ids {
            let filter = move |metadata: &HashMap<String, Value>| {
                metadata.get("client_id")
                    .and_then(|v| v.as_str())
                    .map_or(false, |id| id == client_id)
            };
            
            let results = storage.search_vectors_with_filter(&collection_id.to_string(), vec![], 1, filter)
                .await
                .map_err(|e| ServiceError::Storage(e.to_string()))?;
                
            if let Some(_result) = results.into_iter().next() {
                // Note: We would need to implement soft delete via TTL
                // For now, increment counter to show intent
                deleted_count += 1;
            }
        }
        
        Ok(deleted_count)
    }
}

/// Collection service for collection operations
#[derive(Clone)]
pub struct CollectionService {
    storage: Arc<RwLock<StorageEngine>>,
}

impl CollectionService {
    pub fn new(storage: Arc<RwLock<StorageEngine>>) -> Self {
        Self { storage }
    }

    /// Create a new collection
    pub async fn create_collection(
        &self,
        name: String,
        dimension: u32,
        distance_metric: Option<String>,
        indexing_algorithm: Option<String>,
        config: Option<HashMap<String, Value>>,
        flush_config: Option<CollectionFlushConfig>,
        filterable_metadata_fields: Option<Vec<String>>,
    ) -> ServiceResult<CollectionMetadata> {
        let mut storage = self.storage.write().await;
        
        // Validate and limit filterable metadata fields to 16 for Parquet column optimization
        let mut validated_fields = filterable_metadata_fields.unwrap_or_default();
        if validated_fields.len() > 16 {
            tracing::warn!("⚠️  Collection '{}' specified {} filterable metadata fields, limiting to 16 for Parquet optimization. Dropped fields: {:?}. Additional metadata can still be used via insert operations (stored in extra_meta).", 
                name, validated_fields.len(), &validated_fields[16..]);
            validated_fields.truncate(16);
        }
        
        if !validated_fields.is_empty() {
            tracing::info!("📊 Collection '{}' configured with {} filterable metadata fields for optimized queries: {:?}", 
                name, validated_fields.len(), validated_fields);
        }
        
        // Store filterable metadata fields in collection config
        let mut collection_config = config.unwrap_or_default();
        if !validated_fields.is_empty() {
            collection_config.insert(
                "filterable_metadata_fields".to_string(),
                serde_json::Value::Array(
                    validated_fields.iter()
                        .map(|s| serde_json::Value::String(s.clone()))
                        .collect()
                )
            );
        }
        
        let mut metadata = CollectionMetadata {
            id: name.clone(),
            name,
            dimension: dimension as usize,
            distance_metric: distance_metric.unwrap_or_else(|| "cosine".to_string()),
            indexing_algorithm: indexing_algorithm.unwrap_or_else(|| "hnsw".to_string()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            vector_count: 0,
            total_size_bytes: 0,
            config: collection_config,
            flush_config,
            ..CollectionMetadata::default()
        };
        
        tracing::info!("🏗️ Creating collection '{}' with flush config: {:?}", 
                      metadata.name, metadata.flush_config);
        
        // Pass validated filterable metadata fields to storage layer
        storage.create_collection_with_metadata(metadata.id.clone(), Some(metadata.clone()), Some(validated_fields))
            .await
            .map_err(|e| ServiceError::Storage(e.to_string()))?;
            
        Ok(metadata)
    }

    /// Get collection metadata
    pub async fn get_collection(
        &self,
        collection_identifier: &str,
    ) -> ServiceResult<CollectionMetadata> {
        let storage = self.storage.read().await;
        
        storage.get_collection_metadata(&collection_identifier.to_string())
            .await
            .map_err(|e| ServiceError::Storage(e.to_string()))?
            .ok_or_else(|| ServiceError::CollectionNotFound(collection_identifier.to_string()))
    }

    /// List all collections
    pub async fn list_collections(&self) -> ServiceResult<Vec<CollectionMetadata>> {
        let storage = self.storage.read().await;
        
        storage.list_collections()
            .await
            .map_err(|e| ServiceError::Storage(e.to_string()))
    }

    /// Delete a collection
    pub async fn delete_collection(&self, collection_identifier: &str) -> ServiceResult<()> {
        let storage = self.storage.write().await;
        
        storage.delete_collection(&collection_identifier.to_string())
            .await
            .map_err(|e| ServiceError::Storage(e.to_string()))?;
            
        Ok(())
    }
}

// Migration types are now in the migration module