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

//! LSH (Locality Sensitive Hashing) index implementation for AXIS
//!
//! This module provides a production-ready LSH index that integrates seamlessly
//! with the AXIS adaptive indexing system. LSH is ideal for approximate nearest
//! neighbor search, especially for high-dimensional data and binary vectors.

use anyhow::{anyhow, Result};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::hash::Hash;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tracing::{debug, info};

use crate::compute::distance_computation::DistanceMetric;
use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use crate::core::VectorRecord;
use crate::index::axis::index_factory::{AxisVectorIndex, IndexStats};
use crate::index::axis::types::IndexAlgorithm;
use async_trait::async_trait;

/// LSH index configuration aligned with AXIS standards
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AxisLshConfig {
    /// Number of hash tables
    pub n_tables: usize,
    /// Number of hash functions per table
    pub n_hashes: usize,
    /// Width of hash buckets (for LSH with real vectors)
    pub hash_width: f32,
    /// Random seed for reproducibility
    pub seed: u64,
    /// Distance metric (typically Cosine or Hamming)
    pub distance_metric: DistanceMetric,
    /// Whether to use binary hashing (for binary vectors)
    pub binary_mode: bool,
}

impl Default for AxisLshConfig {
    fn default() -> Self {
        Self {
            n_tables: 20,
            n_hashes: 10,
            hash_width: 1.0,
            seed: 42,
            distance_metric: DistanceMetric::Cosine,
            binary_mode: false,
        }
    }
}

/// Statistics for LSH index
#[derive(Debug, Clone, Default)]
pub struct LshStats {
    pub vector_count: usize,
    pub table_count: usize,
    pub hash_functions_per_table: usize,
    pub memory_usage_bytes: usize,
    pub avg_bucket_size: f32,
    pub collision_rate: f32,
}

/// Hash function for LSH
#[derive(Clone)]
struct HashFunction {
    /// Random projection vector
    projection: Vec<f32>,
    /// Bias term
    bias: f32,
    /// Hash width
    width: f32,
}

impl HashFunction {
    fn new(dimension: usize, width: f32, rng: &mut impl rand::Rng) -> Self {
        // Generate random normal distribution values using Box-Muller transform
        let projection: Vec<f32> = (0..dimension)
            .map(|_| {
                // Box-Muller transform for normal distribution
                let u1 = rng.gen::<f32>();
                let u2 = rng.gen::<f32>();
                let z0 = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos();
                z0
            })
            .collect();
        
        let bias = rng.gen::<f32>() * width;
        
        Self { projection, bias, width }
    }
    
    fn hash(&self, vector: &[f32]) -> i32 {
        let dot_product: f32 = vector.iter()
            .zip(&self.projection)
            .map(|(a, b)| a * b)
            .sum();
        
        ((dot_product + self.bias) / self.width).floor() as i32
    }
}

/// Partitioned key for collection-aware storage
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct PartitionedKey<K: Hash + Eq> {
    pub collection_id: String,
    pub key: K,
}

impl<K: Hash + Eq> PartitionedKey<K> {
    pub fn new(collection_id: String, key: K) -> Self {
        Self { collection_id, key }
    }
}

/// AXIS-integrated LSH index with collection partitioning
pub struct AxisLshIndex {
    /// Collection identifier for partitioning (optional for backward compatibility)
    collection_id: Option<String>,
    /// Configuration
    config: AxisLshConfig,
    /// Hash tables mapping hash values to vector IDs
    /// Partitioned by collection: (collection_id, hash_value) -> vector_ids
    hash_tables: Vec<Arc<DashMap<PartitionedKey<u64>, HashSet<String>>>>,
    /// Hash functions for each table
    hash_functions: Vec<Vec<HashFunction>>,
    /// Vector data storage - partitioned by collection
    vectors: Arc<DashMap<PartitionedKey<String>, Arc<VectorRecord>>>,
    /// Dimension
    dimension: usize,
    /// Distance compute instance
    distance_compute: UnifiedDistanceCompute,
    /// Statistics
    vector_count: Arc<AtomicUsize>,
    /// The algorithm specification
    algorithm: IndexAlgorithm,
}

impl AxisLshIndex {
    /// Create a new LSH index
    pub fn new(config: AxisLshConfig, dimension: usize) -> Self {
        Self::new_with_collection(None, config, dimension)
    }
    
    /// Create a new LSH index for a specific collection
    pub fn new_with_collection(
        collection_id: Option<String>,
        config: AxisLshConfig,
        dimension: usize
    ) -> Self {
        let coll_str = collection_id.as_ref().map(|s| s.as_str()).unwrap_or("default");
        info!(
            "Creating AXIS LSH index for collection '{}': {} tables, {} hashes, {} dim",
            coll_str, config.n_tables, config.n_hashes, dimension
        );
        
        use rand::SeedableRng;
        use rand::rngs::StdRng;
        
        let mut rng = StdRng::seed_from_u64(config.seed);
        let distance_compute = UnifiedDistanceCompute::new(config.distance_metric);
        
        // Initialize hash tables
        let mut hash_tables = Vec::new();
        for _ in 0..config.n_tables {
            hash_tables.push(Arc::new(DashMap::new()));
        }
        
        // Generate hash functions
        let mut hash_functions = Vec::new();
        for _ in 0..config.n_tables {
            let table_functions: Vec<HashFunction> = (0..config.n_hashes)
                .map(|_| HashFunction::new(dimension, config.hash_width, &mut rng))
                .collect();
            hash_functions.push(table_functions);
        }
        
        let algorithm = IndexAlgorithm::LSH {
            n_projections: config.n_hashes as u32,
            n_hash_tables: config.n_tables as u32,
            hash_width: config.hash_width,
        };
        
        Self {
            collection_id,
            config,
            hash_tables,
            hash_functions,
            vectors: Arc::new(DashMap::new()),
            dimension,
            distance_compute,
            vector_count: Arc::new(AtomicUsize::new(0)),
            algorithm,
        }
    }
    
    /// Add a vector to the index
    pub async fn add(&self, id: String, vector_record: Arc<VectorRecord>) -> Result<()> {
        if vector_record.vector.len() != self.dimension {
            return Err(anyhow!(
                "Vector dimension mismatch: expected {}, got {}",
                self.dimension,
                vector_record.vector.len()
            ));
        }
        
        // Compute hash for each table
        for (table_idx, table) in self.hash_tables.iter().enumerate() {
            let hash_value = self.compute_hash(table_idx, &vector_record.vector);
            
            // Create partitioned key for hash table
            let key = if let Some(ref coll_id) = self.collection_id {
                PartitionedKey::new(coll_id.clone(), hash_value)
            } else {
                PartitionedKey::new("default".to_string(), hash_value)
            };
            
            table
                .entry(key)
                .or_insert_with(HashSet::new)
                .insert(id.clone());
        }
        
        // Store vector with partitioned key
        let vector_key = if let Some(ref coll_id) = self.collection_id {
            PartitionedKey::new(coll_id.clone(), id)
        } else {
            PartitionedKey::new("default".to_string(), id)
        };
        
        self.vectors.insert(vector_key, vector_record);
        self.vector_count.fetch_add(1, Ordering::Relaxed);
        
        Ok(())
    }
    
    /// Search for k nearest neighbors
    pub async fn search(
        &self,
        query: &[f32],
        k: usize,
        filter: Option<&(dyn for<'a> Fn(&'a VectorRecord) -> bool + Send + Sync)>,
    ) -> Result<Vec<(String, f32)>> {
        if query.len() != self.dimension {
            return Err(anyhow!(
                "Query dimension mismatch: expected {}, got {}",
                self.dimension,
                query.len()
            ));
        }
        
        // Find candidate vectors from all hash tables
        let mut candidates = HashSet::new();
        
        for (table_idx, table) in self.hash_tables.iter().enumerate() {
            let hash_value = self.compute_hash(table_idx, query);
            
            // Create partitioned key for lookup
            let key = if let Some(ref coll_id) = self.collection_id {
                PartitionedKey::new(coll_id.clone(), hash_value)
            } else {
                PartitionedKey::new("default".to_string(), hash_value)
            };
            
            // Look in the same bucket
            if let Some(bucket) = table.get(&key) {
                for id in bucket.iter() {
                    candidates.insert(id.clone());
                }
            }
            
            // Optional: Look in adjacent buckets for better recall
            if self.config.n_hashes <= 5 {
                for offset in &[-1, 1] {
                    let adjacent_hash = (hash_value as i64 + offset) as u64;
                    let adjacent_key = if let Some(ref coll_id) = self.collection_id {
                        PartitionedKey::new(coll_id.clone(), adjacent_hash)
                    } else {
                        PartitionedKey::new("default".to_string(), adjacent_hash)
                    };
                    
                    if let Some(bucket) = table.get(&adjacent_key) {
                        for id in bucket.iter() {
                            candidates.insert(id.clone());
                        }
                    }
                }
            }
        }
        
        // Compute actual distances for candidates
        let mut results = Vec::new();
        for id in &candidates {
            // Create partitioned key for vector lookup
            let vector_key = if let Some(ref coll_id) = self.collection_id {
                PartitionedKey::new(coll_id.clone(), id.clone())
            } else {
                PartitionedKey::new("default".to_string(), id.clone())
            };
            
            if let Some(vector_record) = self.vectors.get(&vector_key) {
                // Apply filter if provided
                if let Some(f) = filter {
                    if !f(&vector_record) {
                        continue;
                    }
                }
                
                let dist = self.compute_distance(query, &vector_record.vector);
                results.push((id.clone(), dist));
            }
        }
        
        // Sort by distance and return top-k
        results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        results.truncate(k);
        
        debug!(
            "LSH search: {} candidates examined, {} results returned",
            candidates.len(),
            results.len()
        );
        
        Ok(results)
    }
    
    /// Remove a vector from the index
    pub async fn remove(&self, id: &str) -> Result<()> {
        let collection_id = self.collection_id.as_ref().map(|s| s.as_str()).unwrap_or("default");
        let key = PartitionedKey::new(collection_id.to_string(), id.to_string());
        if let Some((_, vector_record)) = self.vectors.remove(&key) {
            // Remove from all hash tables
            for (table_idx, table) in self.hash_tables.iter().enumerate() {
                let hash_value = self.compute_hash(table_idx, &vector_record.vector);
                
                let hash_key = PartitionedKey::new(collection_id.to_string(), hash_value);
                if let Some(mut bucket) = table.get_mut(&hash_key) {
                    bucket.remove(id);
                    if bucket.is_empty() {
                        drop(bucket);
                        table.remove(&hash_key);
                    }
                }
            }
            
            self.vector_count.fetch_sub(1, Ordering::Relaxed);
            Ok(())
        } else {
            Err(anyhow!("Vector {} not found in index", id))
        }
    }
    
    /// Get index statistics
    pub fn stats(&self) -> LshStats {
        let vector_count = self.vector_count.load(Ordering::Relaxed);
        
        let mut total_buckets = 0;
        let mut total_items = 0;
        
        for table in &self.hash_tables {
            total_buckets += table.len();
            for bucket in table.iter() {
                total_items += bucket.value().len();
            }
        }
        
        let avg_bucket_size = if total_buckets > 0 {
            total_items as f32 / total_buckets as f32
        } else {
            0.0
        };
        
        // Collision rate: average number of times each vector appears across tables
        let collision_rate = if vector_count > 0 {
            total_items as f32 / vector_count as f32
        } else {
            0.0
        };
        
        // Estimate memory usage
        let hash_function_memory = self.config.n_tables * self.config.n_hashes * self.dimension * 4;
        let vector_memory = vector_count * self.dimension * 4;
        let index_overhead = total_items * 64; // Rough estimate
        
        LshStats {
            vector_count,
            table_count: self.config.n_tables,
            hash_functions_per_table: self.config.n_hashes,
            memory_usage_bytes: hash_function_memory + vector_memory + index_overhead,
            avg_bucket_size,
            collision_rate,
        }
    }
    
    // Private helper methods
    
    fn compute_distance(&self, a: &[f32], b: &[f32]) -> f32 {
        let result = self.distance_compute.calculate_distance(a, b, &self.config.distance_metric);
        result.raw_value
    }
    
    fn compute_hash(&self, table_idx: usize, vector: &[f32]) -> u64 {
        if self.config.binary_mode {
            // Binary LSH: treat vector as binary and hash directly
            self.compute_binary_hash(table_idx, vector)
        } else {
            // Real-valued LSH: use hash functions
            self.compute_real_hash(table_idx, vector)
        }
    }
    
    fn compute_real_hash(&self, table_idx: usize, vector: &[f32]) -> u64 {
        let functions = &self.hash_functions[table_idx];
        let mut hash_values = Vec::new();
        
        for func in functions {
            hash_values.push(func.hash(vector));
        }
        
        // Combine hash values into a single hash
        let mut combined_hash = 0u64;
        for (i, &h) in hash_values.iter().enumerate() {
            combined_hash = combined_hash.wrapping_mul(31).wrapping_add(h as u64);
            combined_hash = combined_hash.rotate_left((i % 64) as u32);
        }
        
        combined_hash
    }
    
    fn compute_binary_hash(&self, table_idx: usize, vector: &[f32]) -> u64 {
        // For binary vectors, use random sampling
        use rand::{Rng, SeedableRng};
        use rand::rngs::StdRng;
        
        let mut rng = StdRng::seed_from_u64(self.config.seed + table_idx as u64);
        let mut hash = 0u64;
        
        // Sample n_hashes positions
        for i in 0..self.config.n_hashes.min(64) {
            let pos = rng.gen_range(0..self.dimension);
            if vector[pos] > 0.5 {
                hash |= 1u64 << i;
            }
        }
        
        hash
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_lsh_basic_operations() {
        let config = AxisLshConfig {
            n_tables: 5,
            n_hashes: 3,
            hash_width: 2.0,
            ..Default::default()
        };
        
        let index = AxisLshIndex::new(config, 4);
        
        // Add vectors
        let vectors = vec![
            vec![1.0, 0.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0, 0.0],
            vec![0.0, 0.0, 1.0, 0.0],
            vec![0.0, 0.0, 0.0, 1.0],
            vec![0.9, 0.1, 0.0, 0.0], // Similar to first
        ];
        
        for (i, vector) in vectors.iter().enumerate() {
            let record = VectorRecord {
                id: Some(format!("vec_{}", i)),
                vector: vector.clone(),
                metadata: vec![],
                timestamp: 0,
                updated_at: Some(0),
                expires_at: None,
                version: Some(1),
                rank: None,
                score: None,
                distance: None,
            };
            index.add(format!("vec_{}", i), Arc::new(record)).await.unwrap();
        }
        
        // Search for similar to first vector
        let query = vec![0.95, 0.05, 0.0, 0.0];
        let results = index.search(&query, 3, None).await.unwrap();
        
        assert!(!results.is_empty());
        // Should find vec_0 and vec_4 as most similar
        let result_ids: Vec<String> = results.iter().map(|(id, _)| id.clone()).collect();
        assert!(result_ids.contains(&"vec_0".to_string()) || result_ids.contains(&"vec_4".to_string()));
    }
    
    #[tokio::test]
    async fn test_lsh_binary_mode() {
        let config = AxisLshConfig {
            n_tables: 3,
            n_hashes: 8,
            binary_mode: true,
            distance_metric: DistanceMetric::Hamming,
            ..Default::default()
        };
        
        let index = AxisLshIndex::new(config, 8);
        
        // Add binary vectors (represented as 0.0 or 1.0)
        let vectors = vec![
            vec![1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0],
            vec![0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0],
            vec![1.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0],
        ];
        
        for (i, vector) in vectors.iter().enumerate() {
            let record = VectorRecord {
                id: Some(format!("binary_{}", i)),
                vector: vector.clone(),
                metadata: vec![],
                timestamp: 0,
                updated_at: Some(0),
                expires_at: None,
                version: Some(1),
                rank: None,
                score: None,
                distance: None,
            };
            index.add(format!("binary_{}", i), Arc::new(record)).await.unwrap();
        }
        
        // Search
        let query = vec![1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]; // Similar to first
        let results = index.search(&query, 2, None).await.unwrap();
        
        assert!(!results.is_empty());
        assert_eq!(results[0].0, "binary_0"); // Should find the most similar
    }
}

// Implement AxisVectorIndex trait for deep AXIS integration
#[async_trait]
impl AxisVectorIndex for AxisLshIndex {
    async fn add(&self, id: String, vector: Arc<VectorRecord>) -> Result<()> {
        AxisLshIndex::add(self, id, vector).await
    }
    
    async fn search(
        &self,
        query: &[f32],
        top_k: usize,
        filter: Option<&(dyn for<'a> Fn(&'a VectorRecord) -> bool + Send + Sync)>,
    ) -> Result<Vec<(String, f32)>> {
        AxisLshIndex::search(self, query, top_k, filter).await
    }
    
    async fn remove(&self, id: &str) -> Result<()> {
        AxisLshIndex::remove(self, id).await
    }
    
    fn algorithm(&self) -> &IndexAlgorithm {
        &self.algorithm
    }
    
    fn stats(&self) -> IndexStats {
        let lsh_stats = self.stats();
        IndexStats {
            vector_count: lsh_stats.vector_count,
            memory_usage_bytes: lsh_stats.memory_usage_bytes,
            index_type: "LSH".to_string(),
        }
    }
}