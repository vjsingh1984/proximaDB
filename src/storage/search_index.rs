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

//! Vector search index management for ProximaDB

use crate::compute::algorithms::{VectorSearchAlgorithm, SearchResult, create_search_algorithm};
use crate::compute::{IndexAlgorithm, DistanceMetric};
use crate::core::{CollectionId, VectorId, VectorRecord, StorageError};
use crate::storage::{Result, CollectionMetadata};

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use serde::{Serialize, Deserialize};

/// Search request parameters
pub struct SearchRequest {
    pub query: Vec<f32>,
    pub k: usize,
    pub collection_id: CollectionId,
    pub filter: Option<Box<dyn Fn(&HashMap<String, serde_json::Value>) -> bool + Send + Sync>>,
}

impl std::fmt::Debug for SearchRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchRequest")
            .field("query", &self.query)
            .field("k", &self.k)
            .field("collection_id", &self.collection_id)
            .field("filter", &self.filter.is_some())
            .finish()
    }
}

/// Index configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexConfig {
    pub algorithm: String,
    pub distance_metric: String,
    pub parameters: HashMap<String, serde_json::Value>,
}

impl Default for IndexConfig {
    fn default() -> Self {
        let mut params = HashMap::new();
        params.insert("m".to_string(), serde_json::json!(16));
        params.insert("ef_construction".to_string(), serde_json::json!(200));
        
        Self {
            algorithm: "hnsw".to_string(),
            distance_metric: "cosine".to_string(),
            parameters: params,
        }
    }
}

/// Vector search index manager
pub struct SearchIndexManager {
    indexes: Arc<RwLock<HashMap<CollectionId, Box<dyn VectorSearchAlgorithm>>>>,
    configs: Arc<RwLock<HashMap<CollectionId, IndexConfig>>>,
    data_dir: PathBuf,
}

impl std::fmt::Debug for SearchIndexManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchIndexManager")
            .field("data_dir", &self.data_dir)
            .field("indexes_count", &"<HashMap>")
            .field("configs_count", &"<HashMap>")
            .finish()
    }
}

impl SearchIndexManager {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            indexes: Arc::new(RwLock::new(HashMap::new())),
            configs: Arc::new(RwLock::new(HashMap::new())),
            data_dir,
        }
    }
    
    /// Create index for a collection
    pub async fn create_index(&self, collection_id: CollectionId, metadata: &CollectionMetadata) -> Result<()> {
        let index_config = self.parse_index_config(metadata)?;
        
        // Parse algorithm and distance metric
        let algorithm = self.parse_algorithm(&index_config)?;
        let distance_metric = self.parse_distance_metric(&index_config.distance_metric)?;
        
        // Create the search algorithm
        let search_algorithm = create_search_algorithm(algorithm, distance_metric, true);
        
        // Store index and config
        let mut indexes = self.indexes.write().await;
        let mut configs = self.configs.write().await;
        
        indexes.insert(collection_id.clone(), search_algorithm);
        configs.insert(collection_id, index_config);
        
        Ok(())
    }
    
    /// Add vector to index
    pub async fn add_vector(&self, collection_id: &CollectionId, record: &VectorRecord) -> Result<()> {
        let mut indexes = self.indexes.write().await;
        
        if let Some(index) = indexes.get_mut(collection_id) {
            let id = record.id.to_string();
            let vector = record.vector.clone();
            let metadata = if record.metadata.is_empty() {
                None
            } else {
                // Convert HashMap<String, String> to HashMap<String, serde_json::Value>
                let json_metadata: HashMap<String, serde_json::Value> = record.metadata
                    .iter()
                    .map(|(k, v)| (k.clone(), serde_json::Value::String(v.to_string())))
                    .collect();
                Some(json_metadata)
            };
            
            index.add_vector(id, vector, metadata)
                .map_err(|e| StorageError::IndexError(e))?;
        }
        
        Ok(())
    }
    
    /// Remove vector from index
    pub async fn remove_vector(&self, collection_id: &CollectionId, vector_id: &VectorId) -> Result<bool> {
        let mut indexes = self.indexes.write().await;
        
        if let Some(index) = indexes.get_mut(collection_id) {
            let removed = index.remove_vector(&vector_id.to_string())
                .map_err(|e| StorageError::IndexError(e))?;
            Ok(removed)
        } else {
            Ok(false)
        }
    }
    
    /// Search vectors
    pub async fn search(&self, request: SearchRequest) -> Result<Vec<SearchResult>> {
        let indexes = self.indexes.read().await;
        
        if let Some(index) = indexes.get(&request.collection_id) {
            let results = if let Some(filter) = request.filter {
                index.search_with_filter(&request.query, request.k, &*filter)
                    .map_err(|e| StorageError::IndexError(e))?
            } else {
                index.search(&request.query, request.k)
                    .map_err(|e| StorageError::IndexError(e))?
            };
            
            Ok(results)
        } else {
            Err(StorageError::NotFound(format!("Index for collection {} not found", request.collection_id)))
        }
    }
    
    /// Get index statistics
    pub async fn get_index_stats(&self, collection_id: &CollectionId) -> Result<Option<HashMap<String, serde_json::Value>>> {
        let indexes = self.indexes.read().await;
        
        if let Some(index) = indexes.get(collection_id) {
            let memory_usage = index.memory_usage();
            let mut stats = HashMap::new();
            
            stats.insert("size".to_string(), serde_json::json!(index.size()));
            stats.insert("index_size_bytes".to_string(), serde_json::json!(memory_usage.index_size_bytes));
            stats.insert("vector_data_bytes".to_string(), serde_json::json!(memory_usage.vector_data_bytes));
            stats.insert("metadata_bytes".to_string(), serde_json::json!(memory_usage.metadata_bytes));
            stats.insert("total_bytes".to_string(), serde_json::json!(memory_usage.total_bytes));
            
            Ok(Some(stats))
        } else {
            Ok(None)
        }
    }
    
    /// Optimize index
    pub async fn optimize_index(&self, collection_id: &CollectionId) -> Result<()> {
        let mut indexes = self.indexes.write().await;
        
        if let Some(index) = indexes.get_mut(collection_id) {
            index.optimize()
                .map_err(|e| StorageError::IndexError(e))?;
        }
        
        Ok(())
    }
    
    /// Remove index for collection
    pub async fn remove_index(&self, collection_id: &CollectionId) -> Result<bool> {
        let mut indexes = self.indexes.write().await;
        let mut configs = self.configs.write().await;
        
        let removed_index = indexes.remove(collection_id).is_some();
        let removed_config = configs.remove(collection_id).is_some();
        
        Ok(removed_index || removed_config)
    }
    
    /// Build index from existing vectors
    pub async fn rebuild_index(&self, collection_id: &CollectionId, vectors: Vec<VectorRecord>) -> Result<()> {
        // Remove existing index
        self.remove_index(collection_id).await?;
        
        // Add vectors in batch if the index exists
        let indexes = self.indexes.read().await;
        if let Some(_index) = indexes.get(collection_id) {
            let vector_data: Vec<(String, Vec<f32>, Option<HashMap<String, serde_json::Value>>)> = vectors
                .into_iter()
                .map(|record| {
                    let id = record.id.to_string();
                    let vector = record.vector;
                    let metadata = if record.metadata.is_empty() {
                        None
                    } else {
                        let json_metadata: HashMap<String, serde_json::Value> = record.metadata
                            .iter()
                            .map(|(k, v)| (k.clone(), serde_json::Value::String(v.to_string())))
                            .collect();
                        Some(json_metadata)
                    };
                    (id, vector, metadata)
                })
                .collect();
            
            // Add vectors in batch for better performance
            drop(indexes); // Release read lock
            let mut indexes = self.indexes.write().await;
            if let Some(index) = indexes.get_mut(collection_id) {
                index.add_vectors(vector_data)
                    .map_err(|e| StorageError::IndexError(e))?;
            }
        }
        
        Ok(())
    }
    
    fn parse_index_config(&self, metadata: &CollectionMetadata) -> Result<IndexConfig> {
        // Extract index configuration from collection metadata
        let algorithm = metadata.config
            .get("indexing_algorithm")
            .and_then(|v| v.as_str())
            .unwrap_or(&metadata.indexing_algorithm)
            .to_string();
        
        let distance_metric = metadata.config
            .get("distance_metric")
            .and_then(|v| v.as_str())
            .unwrap_or(&metadata.distance_metric)
            .to_string();
        
        let mut parameters = HashMap::new();
        
        // Extract HNSW parameters
        if let Some(m) = metadata.config.get("max_connections").or_else(|| metadata.config.get("m")) {
            parameters.insert("m".to_string(), m.clone());
        } else {
            parameters.insert("m".to_string(), serde_json::json!(16));
        }
        
        if let Some(ef) = metadata.config.get("ef_construction") {
            parameters.insert("ef_construction".to_string(), ef.clone());
        } else {
            parameters.insert("ef_construction".to_string(), serde_json::json!(200));
        }
        
        Ok(IndexConfig {
            algorithm,
            distance_metric,
            parameters,
        })
    }
    
    fn parse_algorithm(&self, config: &IndexConfig) -> Result<IndexAlgorithm> {
        match config.algorithm.to_lowercase().as_str() {
            "hnsw" => {
                let m = config.parameters
                    .get("m")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(16) as usize;
                
                let ef_construction = config.parameters
                    .get("ef_construction")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(200) as usize;
                
                Ok(IndexAlgorithm::HNSW {
                    m,
                    ef_construction,
                    max_elements: 1000000,
                })
            },
            "brute_force" | "flat" => Ok(IndexAlgorithm::BruteForce),
            _ => {
                // Default to HNSW
                Ok(IndexAlgorithm::HNSW {
                    m: 16,
                    ef_construction: 200,
                    max_elements: 1000000,
                })
            }
        }
    }
    
    fn parse_distance_metric(&self, metric: &str) -> Result<DistanceMetric> {
        match metric.to_lowercase().as_str() {
            "cosine" => Ok(DistanceMetric::Cosine),
            "euclidean" | "l2" => Ok(DistanceMetric::Euclidean),
            "manhattan" | "l1" => Ok(DistanceMetric::Manhattan),
            "dot" | "inner_product" => Ok(DistanceMetric::DotProduct),
            _ => {
                // Default to cosine
                Ok(DistanceMetric::Cosine)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use uuid::Uuid;
    use chrono::Utc;
    
    #[tokio::test]
    async fn test_search_index_basic() {
        let temp_dir = TempDir::new().unwrap();
        let index_manager = SearchIndexManager::new(temp_dir.path().to_path_buf());
        
        // Create test metadata
        let metadata = CollectionMetadata {
            id: "test_collection".to_string(),
            name: "Test Collection".to_string(),
            dimension: 3,
            distance_metric: "cosine".to_string(),
            indexing_algorithm: "hnsw".to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            vector_count: 0,
            total_size_bytes: 0,
            config: HashMap::new(),
        };
        
        // Create index
        index_manager.create_index("test_collection".to_string(), &metadata).await.unwrap();
        
        // Add test vectors
        let vectors = vec![
            VectorRecord {
                id: Uuid::new_v4(),
                collection_id: "test_collection".to_string(),
                vector: vec![1.0, 0.0, 0.0],
                metadata: HashMap::new(),
                timestamp: Utc::now(),
                expires_at: None,
            },
            VectorRecord {
                id: Uuid::new_v4(),
                collection_id: "test_collection".to_string(),
                vector: vec![0.0, 1.0, 0.0],
                metadata: HashMap::new(),
                timestamp: Utc::now(),
                expires_at: None,
            },
            VectorRecord {
                id: Uuid::new_v4(),
                collection_id: "test_collection".to_string(),
                vector: vec![0.0, 0.0, 1.0],
                metadata: HashMap::new(),
                timestamp: Utc::now(),
                expires_at: None,
            },
        ];
        
        for vector in &vectors {
            index_manager.add_vector(&"test_collection".to_string(), vector).await.unwrap();
        }
        
        // Search
        let search_request = SearchRequest {
            query: vec![1.0, 0.0, 0.0],
            k: 2,
            collection_id: "test_collection".to_string(),
            filter: None,
        };
        
        let results = index_manager.search(search_request).await.unwrap();
        assert_eq!(results.len(), 2);
        
        // Check stats
        let stats = index_manager.get_index_stats(&"test_collection".to_string()).await.unwrap();
        assert!(stats.is_some());
        let stats = stats.unwrap();
        assert_eq!(stats["size"], serde_json::json!(3));
    }
}