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

//! Metadata storage for collections and system configuration

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use serde::{Serialize, Deserialize};
use chrono::{DateTime, Utc};

use crate::core::{CollectionId, StorageError};
use crate::storage::Result;

/// Collection metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionMetadata {
    pub id: CollectionId,
    pub name: String,
    pub dimension: u32,
    pub distance_metric: String,
    pub indexing_algorithm: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub vector_count: u64,
    pub total_size_bytes: u64,
    pub config: HashMap<String, serde_json::Value>,
}

/// System metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMetadata {
    pub version: String,
    pub node_id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_checkpoint: DateTime<Utc>,
    pub total_collections: u64,
    pub total_vectors: u64,
}

/// Metadata storage manager
#[derive(Debug)]
pub struct MetadataStore {
    data_dir: PathBuf,
    collections: Arc<RwLock<HashMap<CollectionId, CollectionMetadata>>>,
    system: Arc<RwLock<SystemMetadata>>,
}

impl MetadataStore {
    pub async fn new(data_dir: PathBuf) -> Result<Self> {
        // Ensure metadata directory exists
        let metadata_dir = data_dir.join("metadata");
        tokio::fs::create_dir_all(&metadata_dir)
            .await
            .map_err(StorageError::DiskIO)?;
        
        let mut store = Self {
            data_dir: metadata_dir,
            collections: Arc::new(RwLock::new(HashMap::new())),
            system: Arc::new(RwLock::new(SystemMetadata {
                version: "0.1.0".to_string(),
                node_id: uuid::Uuid::new_v4().to_string(),
                created_at: Utc::now(),
                updated_at: Utc::now(),
                last_checkpoint: Utc::now(),
                total_collections: 0,
                total_vectors: 0,
            })),
        };
        
        // Load existing metadata
        store.load().await?;
        
        Ok(store)
    }
    
    /// Load metadata from disk
    async fn load(&mut self) -> Result<()> {
        // Load system metadata
        let system_path = self.data_dir.join("system.json");
        if system_path.exists() {
            let data = tokio::fs::read_to_string(&system_path)
                .await
                .map_err(StorageError::DiskIO)?;
            if let Ok(system) = serde_json::from_str::<SystemMetadata>(&data) {
                *self.system.write().await = system;
            }
        }
        
        // Load collection metadata
        let collections_path = self.data_dir.join("collections.json");
        if collections_path.exists() {
            let data = tokio::fs::read_to_string(&collections_path)
                .await
                .map_err(StorageError::DiskIO)?;
            if let Ok(collections) = serde_json::from_str::<HashMap<CollectionId, CollectionMetadata>>(&data) {
                *self.collections.write().await = collections;
            }
        }
        
        Ok(())
    }
    
    /// Persist metadata to disk
    async fn persist(&self) -> Result<()> {
        // Save system metadata
        let system = self.system.read().await;
        let system_data = serde_json::to_string_pretty(&*system)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;
        let system_path = self.data_dir.join("system.json");
        tokio::fs::write(&system_path, system_data)
            .await
            .map_err(StorageError::DiskIO)?;
        
        // Save collection metadata
        let collections = self.collections.read().await;
        let collections_data = serde_json::to_string_pretty(&*collections)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;
        let collections_path = self.data_dir.join("collections.json");
        tokio::fs::write(&collections_path, collections_data)
            .await
            .map_err(StorageError::DiskIO)?;
        
        Ok(())
    }
    
    /// Create a new collection
    pub async fn create_collection(&self, metadata: CollectionMetadata) -> Result<()> {
        let mut collections = self.collections.write().await;
        let collection_id = metadata.id.clone();
        
        if collections.contains_key(&collection_id) {
            return Err(StorageError::AlreadyExists(format!("Collection {} already exists", collection_id)));
        }
        
        collections.insert(collection_id, metadata);
        
        // Update system metadata
        let mut system = self.system.write().await;
        system.total_collections += 1;
        system.updated_at = Utc::now();
        drop(system);
        drop(collections);
        
        // Persist to disk
        self.persist().await?;
        
        Ok(())
    }
    
    /// Get collection metadata
    pub async fn get_collection(&self, collection_id: &CollectionId) -> Result<Option<CollectionMetadata>> {
        let collections = self.collections.read().await;
        Ok(collections.get(collection_id).cloned())
    }
    
    /// List all collections
    pub async fn list_collections(&self) -> Result<Vec<CollectionMetadata>> {
        let collections = self.collections.read().await;
        Ok(collections.values().cloned().collect())
    }
    
    /// Update collection metadata
    pub async fn update_collection(&self, collection_id: &CollectionId, update_fn: impl FnOnce(&mut CollectionMetadata)) -> Result<()> {
        let mut collections = self.collections.write().await;
        
        if let Some(metadata) = collections.get_mut(collection_id) {
            update_fn(metadata);
            metadata.updated_at = Utc::now();
            drop(collections);
            
            // Persist to disk
            self.persist().await?;
            Ok(())
        } else {
            Err(StorageError::NotFound(format!("Collection {} not found", collection_id)))
        }
    }
    
    /// Delete collection metadata
    pub async fn delete_collection(&self, collection_id: &CollectionId) -> Result<bool> {
        let mut collections = self.collections.write().await;
        let removed = collections.remove(collection_id).is_some();
        
        if removed {
            // Update system metadata
            let mut system = self.system.write().await;
            system.total_collections = system.total_collections.saturating_sub(1);
            system.updated_at = Utc::now();
            drop(system);
            drop(collections);
            
            // Persist to disk
            self.persist().await?;
        }
        
        Ok(removed)
    }
    
    /// Update system statistics
    pub async fn update_stats(&self, collection_id: &CollectionId, vector_delta: i64, size_delta: i64) -> Result<()> {
        let mut collections = self.collections.write().await;
        
        if let Some(metadata) = collections.get_mut(collection_id) {
            if vector_delta > 0 {
                metadata.vector_count += vector_delta as u64;
            } else {
                metadata.vector_count = metadata.vector_count.saturating_sub((-vector_delta) as u64);
            }
            
            if size_delta > 0 {
                metadata.total_size_bytes += size_delta as u64;
            } else {
                metadata.total_size_bytes = metadata.total_size_bytes.saturating_sub((-size_delta) as u64);
            }
            
            metadata.updated_at = Utc::now();
        }
        
        // Update system totals
        let mut system = self.system.write().await;
        if vector_delta > 0 {
            system.total_vectors += vector_delta as u64;
        } else {
            system.total_vectors = system.total_vectors.saturating_sub((-vector_delta) as u64);
        }
        system.updated_at = Utc::now();
        
        drop(system);
        drop(collections);
        
        // Persist to disk
        self.persist().await?;
        
        Ok(())
    }
    
    /// Get system metadata
    pub async fn get_system_metadata(&self) -> Result<SystemMetadata> {
        let system = self.system.read().await;
        Ok(system.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    
    #[tokio::test]
    async fn test_metadata_persistence() {
        let temp_dir = TempDir::new().unwrap();
        let store = MetadataStore::new(temp_dir.path().to_path_buf()).await.unwrap();
        
        // Create collection
        let metadata = CollectionMetadata {
            id: "test_collection".to_string(),
            name: "Test Collection".to_string(),
            dimension: 128,
            distance_metric: "cosine".to_string(),
            indexing_algorithm: "hnsw".to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            vector_count: 0,
            total_size_bytes: 0,
            config: HashMap::new(),
        };
        
        store.create_collection(metadata.clone()).await.unwrap();
        
        // Verify it's stored
        let retrieved = store.get_collection(&metadata.id).await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().name, "Test Collection");
        
        // Create new store instance to test persistence
        let store2 = MetadataStore::new(temp_dir.path().to_path_buf()).await.unwrap();
        let retrieved2 = store2.get_collection(&metadata.id).await.unwrap();
        assert!(retrieved2.is_some());
        assert_eq!(retrieved2.unwrap().name, "Test Collection");
    }
}