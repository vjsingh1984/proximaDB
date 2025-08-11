/*
 * Copyright 2025 ProximaDB
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

//! Factory for creating AXIS-native index implementations
//!
//! This module provides a clean, adapter-free factory for creating AXIS indexes.
//! All index types are first-class citizens in the AXIS ecosystem with deep integration.

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::sync::Arc;

use crate::compute::distance_computation::DistanceMetric;
use crate::core::VectorRecord;
// use crate::index::axis::hnsw_integration::{AxisHnswConfig, PartitionedHnswIndex};
use crate::index::axis::annoy_index::{AxisAnnoyConfig, AxisAnnoyIndex};
use crate::index::axis::ivf_unified::{UnifiedIvfConfig, UnifiedIvfIndex};
use crate::index::axis::lsh_index::{AxisLshConfig, AxisLshIndex};
use crate::index::axis::types::IndexAlgorithm;

/// Trait for vector indexes that can be used by AXIS
#[async_trait]
pub trait AxisVectorIndex: Send + Sync {
    /// Add a vector to the index
    async fn add(&self, id: String, vector: Arc<VectorRecord>) -> Result<()>;
    
    /// Search for nearest neighbors
    async fn search(
        &self,
        query: &[f32],
        top_k: usize,
        filter: Option<&(dyn for<'a> Fn(&'a VectorRecord) -> bool + Send + Sync)>,
    ) -> Result<Vec<(String, f32)>>;
    
    /// Remove a vector from the index
    async fn remove(&self, id: &str) -> Result<()>;
    
    /// Get the algorithm type
    fn algorithm(&self) -> &IndexAlgorithm;
    
    /// Get index statistics
    fn stats(&self) -> IndexStats;
}

/// Index statistics
#[derive(Debug, Clone)]
pub struct IndexStats {
    pub vector_count: usize,
    pub memory_usage_bytes: usize,
    pub index_type: String,
}

/// AXIS index creation result
pub enum AxisIndexCreationResult {
    /// IVF index (requires training)
    Ivf(Box<UnifiedIvfIndex>),
    /// LSH index (ready to use)
    Lsh(Box<AxisLshIndex>),
    /// Annoy index (requires building)
    Annoy(Box<AxisAnnoyIndex>),
    // HNSW will be added once async wrapper is implemented
    // /// HNSW index (ready to use)
    // Hnsw(Box<PartitionedHnswIndex>),
}

/// Factory for creating AXIS-native index implementations
pub struct IndexFactory;

impl IndexFactory {
    /// Create an AXIS-native index based on the algorithm specification
    /// Returns the specific index type to allow proper initialization (e.g., training for IVF)
    pub fn create_index(
        algorithm: &IndexAlgorithm,
        dimension: usize,
        distance_metric: DistanceMetric,
    ) -> Result<AxisIndexCreationResult> {
        match algorithm {
            IndexAlgorithm::HNSW { m: _m, ef_construction: _ef_construction, ef_search: _ef_search, max_elements: _max_elements } => {
                // HNSW requires more complex setup with existing implementation
                // For now, we'll return an error and implement a proper async wrapper later
                Err(anyhow!(
                    "HNSW index creation requires async initialization. Use AxisHnswManager for HNSW indexes."
                ))
            }
            
            IndexAlgorithm::IVF { nlist, nprobe, quantizer: _ } => {
                let config = UnifiedIvfConfig {
                    n_clusters: *nlist as usize,
                    n_probe: *nprobe as usize,
                    dimension,
                    distance_metric,
                    ..UnifiedIvfConfig::default()
                };
                
                // For factory, use a default collection ID (will be updated when attached to collection)
                let index = UnifiedIvfIndex::new("default".to_string(), config)?;
                Ok(AxisIndexCreationResult::Ivf(Box::new(index)))
            }
            
            IndexAlgorithm::LSH { n_projections, n_hash_tables, hash_width } => {
                let config = AxisLshConfig {
                    n_tables: *n_hash_tables as usize,
                    n_hashes: *n_projections as usize,
                    hash_width: *hash_width,
                    seed: 42,
                    distance_metric,
                    binary_mode: distance_metric == DistanceMetric::Hamming,
                };
                
                let index = AxisLshIndex::new(config, dimension);
                Ok(AxisIndexCreationResult::Lsh(Box::new(index)))
            }
            
            IndexAlgorithm::Annoy { n_trees, search_k, max_leaf_size } => {
                let config = AxisAnnoyConfig {
                    n_trees: *n_trees as usize,
                    search_k: *search_k,
                    max_leaf_size: *max_leaf_size as usize,
                    seed: 42,
                    distance_metric,
                };
                
                let index = AxisAnnoyIndex::new(config, dimension)?;
                Ok(AxisIndexCreationResult::Annoy(Box::new(index)))
            }
            
            IndexAlgorithm::PQ { .. } => {
                Err(anyhow!("Product Quantization will be integrated in next phase"))
            }
            
            _ => Err(anyhow!("Index algorithm {:?} not supported for vector search", algorithm)),
        }
    }
    
    /// Create a pre-trained index that's ready to use
    /// This is a convenience method that handles training for algorithms that require it
    pub async fn create_trained_index(
        algorithm: &IndexAlgorithm,
        dimension: usize,
        distance_metric: DistanceMetric,
        training_data: Option<&[Vec<f32>]>,
    ) -> Result<Box<dyn AxisVectorIndex>> {
        match Self::create_index(algorithm, dimension, distance_metric)? {
            AxisIndexCreationResult::Ivf(mut index) => {
                if let Some(data) = training_data {
                    index.train(data.to_vec()).await?;
                }
                Ok(index as Box<dyn AxisVectorIndex>)
            }
            AxisIndexCreationResult::Lsh(index) => Ok(index as Box<dyn AxisVectorIndex>),
            AxisIndexCreationResult::Annoy(index) => {
                // Annoy needs to be built after adding vectors
                // For now, return it as-is; the user must call build() separately
                Ok(index as Box<dyn AxisVectorIndex>)
            }
            // AxisIndexCreationResult::Hnsw(index) => Ok(index as Box<dyn AxisVectorIndex>),
        }
    }
}

// No adapters needed - all index types implement AxisVectorIndex directly!

// Implementation of AxisVectorIndex for AXIS-native indexes is in their respective modules:
// - UnifiedIvfIndex in ivf_unified.rs
// - AxisLshIndex in lsh_index.rs
// - AxisAnnoyIndex in annoy_index.rs
// - PartitionedHnswIndex in hnsw_integration.rs

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_create_ivf_index() {
        let algorithm = IndexAlgorithm::IVF {
            nlist: 100,
            nprobe: 10,
            quantizer: None,
        };
        
        let result = IndexFactory::create_index(
            &algorithm,
            128,
            DistanceMetric::Cosine,
        );
        
        assert!(result.is_ok());
        match result.unwrap() {
            AxisIndexCreationResult::Ivf(index) => {
                assert_eq!(index.stats().cluster_count, 100);
            }
            _ => panic!("Expected IVF index"),
        }
    }
    
    #[test]
    fn test_create_lsh_index() {
        let algorithm = IndexAlgorithm::LSH {
            n_projections: 8,
            n_hash_tables: 10,
            hash_width: 1.0,
        };
        
        let result = IndexFactory::create_index(
            &algorithm,
            128,
            DistanceMetric::Cosine,
        );
        
        assert!(result.is_ok());
        match result.unwrap() {
            AxisIndexCreationResult::Lsh(index) => {
                assert_eq!(index.stats().table_count, 10);
            }
            _ => panic!("Expected LSH index"),
        }
    }
    
    #[test]
    fn test_create_annoy_index() {
        let algorithm = IndexAlgorithm::Annoy {
            n_trees: 5,
            search_k: -1,
            max_leaf_size: 10,
        };
        
        let result = IndexFactory::create_index(
            &algorithm,
            128,
            DistanceMetric::Cosine,
        );
        
        assert!(result.is_ok());
        match result.unwrap() {
            AxisIndexCreationResult::Annoy(index) => {
                let annoy_stats = index.stats();
                // Trees are not created until build() is called
                assert_eq!(annoy_stats.tree_count, 0);
                assert!(!annoy_stats.is_built);
                // Verify the config was set correctly
                assert_eq!(index.algorithm(), &algorithm);
            }
            _ => panic!("Expected Annoy index"),
        }
    }
    
    #[tokio::test]
    async fn test_create_trained_index() {
        let algorithm = IndexAlgorithm::IVF {
            nlist: 4,
            nprobe: 2,
            quantizer: None,
        };
        
        let training_data = vec![
            vec![1.0, 0.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0, 0.0],
            vec![0.0, 0.0, 1.0, 0.0],
            vec![0.0, 0.0, 0.0, 1.0],
        ];
        
        let index = IndexFactory::create_trained_index(
            &algorithm,
            4,
            DistanceMetric::Euclidean,
            Some(&training_data),
        ).await;
        
        assert!(index.is_ok());
        let index = index.unwrap();
        assert_eq!(index.stats().index_type, "IVF");
    }
}