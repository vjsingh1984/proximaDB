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

//! Vector Search Algorithms Module
//! 
//! This module provides integration with the advanced indexing algorithms
//! implemented in the compute module.

use anyhow::Result;
use std::sync::Arc;

// Re-export indexing algorithms from compute module
pub use crate::compute::indexing::{
    IvfIndex, IvfConfig,
    LshIndex, LshConfig,
};

/// Vector search algorithm types
#[derive(Debug, Clone)]
pub enum SearchAlgorithm {
    /// Brute force search (exact)
    BruteForce,
    /// IVF (Inverted File) index
    IVF(IvfConfig),
    /// LSH (Locality Sensitive Hashing)
    LSH(LshConfig),
    /// HNSW (Hierarchical Navigable Small World) - uses AXIS integration
    HNSW,
}

/// Vector search engine that can use different algorithms
pub struct VectorSearchEngine {
    /// Selected algorithm
    algorithm: SearchAlgorithm,
    /// IVF index (if using IVF)
    ivf_index: Option<IvfIndex>,
    /// LSH index (if using LSH)
    lsh_index: Option<LshIndex>,
}

impl VectorSearchEngine {
    /// Create new search engine with specified algorithm
    pub fn new(algorithm: SearchAlgorithm, dimension: usize) -> Self {
        let (ivf_index, lsh_index) = match &algorithm {
            SearchAlgorithm::IVF(config) => {
                (Some(IvfIndex::new(config.clone(), dimension)), None)
            }
            SearchAlgorithm::LSH(config) => {
                (None, Some(LshIndex::new(config.clone(), dimension)))
            }
            _ => (None, None),
        };
        
        Self {
            algorithm,
            ivf_index,
            lsh_index,
        }
    }
    
    /// Train the index (for algorithms that require training)
    pub fn train(&mut self, training_vectors: &[Vec<f32>]) -> Result<()> {
        match &mut self.ivf_index {
            Some(ivf) => ivf.train(training_vectors),
            None => Ok(()), // Other algorithms don't require training
        }
    }
    
    /// Add vectors to the index
    pub fn add_vectors(
        &self,
        vectors: Vec<(String, Arc<crate::core::VectorRecord>)>,
    ) -> Result<()> {
        match (&self.ivf_index, &self.lsh_index) {
            (Some(ivf), _) => {
                for (id, record) in vectors {
                    ivf.add(id, record)?;
                }
                Ok(())
            }
            (_, Some(lsh)) => {
                for (id, record) in vectors {
                    lsh.add(id, record)?;
                }
                Ok(())
            }
            _ => Ok(()), // Brute force doesn't maintain an index
        }
    }
    
    /// Search for k nearest neighbors
    pub async fn search(
        &self,
        query: &[f32],
        k: usize,
    ) -> Result<Vec<(String, f32)>> {
        match (&self.ivf_index, &self.lsh_index) {
            (Some(ivf), _) => ivf.search(query, k),
            (_, Some(lsh)) => lsh.search(query, k),
            _ => {
                // Brute force search would go through all vectors
                // This should integrate with the storage engine
                Ok(Vec::new())
            }
        }
    }
    
    /// Get algorithm info
    pub fn algorithm_info(&self) -> String {
        match &self.algorithm {
            SearchAlgorithm::BruteForce => "Brute Force (Exact Search)".to_string(),
            SearchAlgorithm::IVF(config) => {
                format!("IVF with {} clusters, {} probe", config.n_clusters, config.n_probe)
            }
            SearchAlgorithm::LSH(config) => {
                format!("LSH with {} tables, {} hashes", config.n_tables, config.n_hashes)
            }
            SearchAlgorithm::HNSW => "HNSW (via AXIS integration)".to_string(),
        }
    }
}

/// Search algorithm factory
pub struct SearchAlgorithmFactory;

impl SearchAlgorithmFactory {
    /// Select best algorithm based on dataset characteristics
    pub fn select_algorithm(
        num_vectors: usize,
        dimension: usize,
        accuracy_target: f32,
    ) -> SearchAlgorithm {
        // Simple heuristics for algorithm selection
        if num_vectors < 10_000 {
            // Small dataset - brute force is fine
            SearchAlgorithm::BruteForce
        } else if dimension > 512 && accuracy_target < 0.95 {
            // High dimension, moderate accuracy - use LSH
            SearchAlgorithm::LSH(LshConfig::default())
        } else if num_vectors < 1_000_000 {
            // Medium dataset - use IVF
            let n_clusters = (num_vectors as f64).sqrt() as usize;
            SearchAlgorithm::IVF(IvfConfig {
                n_clusters: n_clusters.min(4096).max(256),
                n_probe: 32,
                ..Default::default()
            })
        } else {
            // Large dataset - use HNSW (would integrate with AXIS)
            SearchAlgorithm::HNSW
        }
    }
    
    /// Create optimal configuration for IVF
    pub fn optimize_ivf_config(num_vectors: usize, recall_target: f32) -> IvfConfig {
        let n_clusters = (num_vectors as f64).sqrt() as usize;
        let n_probe = if recall_target >= 0.99 {
            n_clusters / 4 // Search 25% of clusters for high recall
        } else if recall_target >= 0.95 {
            n_clusters / 10 // Search 10% for good recall
        } else {
            n_clusters / 20 // Search 5% for fast search
        };
        
        IvfConfig {
            n_clusters: n_clusters.min(4096).max(256),
            n_probe: n_probe.min(256).max(1),
            ..Default::default()
        }
    }
}