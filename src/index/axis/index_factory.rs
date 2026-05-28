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

use anyhow::{Result, anyhow};
use async_trait::async_trait;
// Removed unused imports
// use std::collections::HashMap;
// use std::sync::Arc;

use crate::compute::distance_computation::DistanceMetric;
// VectorRecord eliminated from AXIS - zero-overhead storage only
use crate::index::axis::filterable_metadata::{FilterableFieldsConfig, FilterableHnswMetadata};
use crate::index::axis::indexes::annoy_index::{AxisAnnoyConfig, AxisAnnoyIndex};
use crate::index::axis::indexes::dual_store_ivf::{UnifiedIvfConfig, UnifiedIvfIndex};
use crate::index::axis::indexes::hnsw_index::{AxisHnswConfig, AxisHnswIndex};
use crate::index::axis::indexes::lsh_index::{AxisLshConfig, AxisLshIndex};
use crate::index::axis::types::IndexAlgorithm;
use crate::index::edr::{EdrIndex, EdrIndexConfig};

/// Trait for vector indexes that can be used by AXIS
///
/// TD-064: This trait now supports filterable metadata for predicate-aware search.
/// All AXIS indexes (HNSW, IVF, Annoy, LSH) can cache filterable metadata for
/// early pruning during index traversal.
///
/// Clean design: No VectorRecord, just raw vector data + optional filterable metadata
#[async_trait]
pub trait AxisVectorIndex: Send + Sync {
    /// Add a vector to the index - just ID and raw data
    async fn add(&self, id: String, vector_data: Vec<f32>) -> Result<()>;

    /// TD-064: Add a vector with filterable metadata
    ///
    /// This allows indexes to cache metadata for predicate-aware search.
    /// Default implementation falls back to add() without metadata.
    async fn add_with_metadata(
        &self,
        id: String,
        vector_data: Vec<f32>,
        _metadata: &FilterableHnswMetadata,
    ) -> Result<()> {
        // Default: ignore metadata and call standard add
        self.add(id, vector_data).await
    }

    /// Search for nearest neighbors with optional metadata filter
    async fn search(
        &self,
        query: &[f32],
        top_k: usize,
        filter: Option<&std::collections::HashMap<String, String>>,
    ) -> Result<Vec<(String, f32)>>;

    /// TD-064: Search with predicate-aware metadata filter
    ///
    /// This method allows indexes to use cached metadata for early pruning.
    /// Default implementation falls back to search() with HashMap filter.
    async fn search_with_predicate(
        &self,
        query: &[f32],
        top_k: usize,
        _tenant_id: Option<&str>,
        _time_range_ns: Option<(i64, i64)>,
        _rls_tags: Option<&[String]>,
    ) -> Result<Vec<(String, f32)>> {
        // Default: fall back to standard search
        self.search(query, top_k, None).await
    }

    /// Remove a vector from the index
    async fn remove(&self, id: &str) -> Result<()>;

    /// Get the algorithm type
    fn algorithm(&self) -> &IndexAlgorithm;

    /// Get index statistics
    fn stats(&self) -> AxisIndexStats;

    /// TD-064: Check if this index supports predicate-aware search
    ///
    /// Returns true if the index has cached metadata for filtering.
    /// Default implementation returns false (metadata not supported).
    fn supports_predicate_search(&self) -> bool {
        false
    }

    /// TD-064: Configure filterable fields for metadata extraction
    ///
    /// This tells the index which fields to extract and cache from records.
    /// Default implementation does nothing (metadata not configurable).
    fn configure_filterable_fields(&self, _config: &FilterableFieldsConfig) -> Result<()> {
        Ok(())
    }
}

/// Backwards-compat alias for [`AxisIndexStats`].
pub type IndexStats = AxisIndexStats;

/// Index statistics
#[derive(Debug, Clone)]
pub struct AxisIndexStats {
    /// Number of vectors currently stored in the index.
    pub vector_count: usize,
    /// Approximate memory consumption of the index in bytes.
    pub memory_usage_bytes: usize,
    /// Human-readable name of the index algorithm (e.g., "HNSW", "IVF").
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
    /// HNSW index (ready to use, no training required)
    Hnsw(Box<AxisHnswIndex>),
    /// EDR index (ready to use, no training required)
    Edr(Box<EdrIndex>),
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
            IndexAlgorithm::HNSW {
                m,
                ef_construction,
                ef_search,
                max_elements: _,
            } => {
                let config = AxisHnswConfig {
                    m: *m as usize,
                    ef_construction: *ef_construction as usize,
                    ef: *ef_search as usize,
                    max_layers: 16,
                    distance_metric,
                    ..AxisHnswConfig::default()
                };

                let index = AxisHnswIndex::new(config, dimension)?;
                Ok(AxisIndexCreationResult::Hnsw(Box::new(index)))
            }

            IndexAlgorithm::IVF {
                nlist,
                nprobe,
                quantizer: _,
            } => {
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

            IndexAlgorithm::LSH {
                n_projections,
                n_hash_tables,
                hash_width,
            } => {
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

            IndexAlgorithm::Annoy {
                n_trees,
                search_k,
                max_leaf_size,
            } => {
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

            IndexAlgorithm::EDR {
                num_query_expansions,
                num_document_vectors,
                top_k,
                enable_query_expansion,
                enable_document_expansion,
            } => {
                let config = EdrIndexConfig {
                    distance_metric,
                    num_query_expansions: *num_query_expansions,
                    num_document_vectors: *num_document_vectors,
                    top_k: *top_k,
                    enable_query_expansion: *enable_query_expansion,
                    enable_document_expansion: *enable_document_expansion,
                };

                let index = EdrIndex::new(config)?;
                Ok(AxisIndexCreationResult::Edr(Box::new(index)))
            }

            IndexAlgorithm::PQ { .. } => Err(anyhow!(
                "Product Quantization will be integrated in next phase"
            )),

            _ => Err(anyhow!(
                "Index algorithm {:?} not supported for vector search",
                algorithm
            )),
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
            AxisIndexCreationResult::Hnsw(index) => {
                // HNSW is ready to use immediately - no training required
                Ok(index as Box<dyn AxisVectorIndex>)
            }
            AxisIndexCreationResult::Edr(index) => {
                // EDR is ready to use immediately - no training required
                Ok(index as Box<dyn AxisVectorIndex>)
            }
        }
    }
}

// No adapters needed - all index types implement AxisVectorIndex directly!

// Implementation of AxisVectorIndex for AXIS-native indexes is in their respective modules:
// - UnifiedIvfIndex in dual_store_ivf.rs
// - AxisLshIndex in lsh_index.rs
// - AxisAnnoyIndex in annoy_index.rs
// - AxisHnswIndex in hnsw_index.rs

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

        let result = IndexFactory::create_index(&algorithm, 128, DistanceMetric::Cosine);

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

        let result = IndexFactory::create_index(&algorithm, 128, DistanceMetric::Cosine);

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

        let result = IndexFactory::create_index(&algorithm, 128, DistanceMetric::Cosine);

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

    #[test]
    fn test_create_hnsw_index() {
        // Initialize hardware capabilities for HNSW
        let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init

        let algorithm = IndexAlgorithm::HNSW {
            m: 16,
            ef_construction: 200,
            ef_search: 100,
            max_elements: 10000,
        };

        let result = IndexFactory::create_index(&algorithm, 128, DistanceMetric::Cosine);

        assert!(result.is_ok());
        match result.unwrap() {
            AxisIndexCreationResult::Hnsw(index) => {
                let stats = index.stats();
                assert_eq!(stats.index_type, "HNSW");
                assert_eq!(stats.vector_count, 0); // Empty initially
            }
            _ => panic!("Expected HNSW index"),
        }
    }

    #[tokio::test]
    async fn test_create_trained_hnsw_index() {
        // Initialize hardware capabilities for HNSW
        let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init

        let algorithm = IndexAlgorithm::HNSW {
            m: 16,
            ef_construction: 200,
            ef_search: 50,
            max_elements: 10000,
        };

        // HNSW doesn't require training - it's ready to use immediately
        let index = IndexFactory::create_trained_index(
            &algorithm,
            4,
            DistanceMetric::Euclidean,
            None, // No training data needed
        )
        .await;

        assert!(index.is_ok());
        let index = index.unwrap();
        assert_eq!(index.stats().index_type, "HNSW");

        // Test that we can add and search vectors
        index
            .add("test_vec".to_string(), vec![1.0, 0.0, 0.0, 0.0])
            .await
            .unwrap();
        assert_eq!(index.stats().vector_count, 1);

        let results = index.search(&[1.0, 0.0, 0.0, 0.0], 1, None).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "test_vec");
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
        )
        .await;

        assert!(index.is_ok());
        let index = index.unwrap();
        assert_eq!(index.stats().index_type, "IVF");
    }
}
