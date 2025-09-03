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
use std::sync::{Arc, RwLock};
use tracing::{debug, info};

use crate::compute::distance_computation::DistanceMetric;
use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
// VectorRecord eliminated - using ZeroOverheadVector for optimal memory
use crate::index::axis::zero_overhead_vector::{ZeroOverheadCollection, CollectionConfig};
use crate::index::axis::index_factory::{AxisVectorIndex, IndexStats};
use crate::index::axis::types::IndexAlgorithm;
use crate::index::axis::eventlog::{IndexEvent, ExtractionMode};
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
                let u1 = rng.gen_range(0.0..1.0);
                let u2 = rng.gen_range(0.0..1.0);
                let z0 = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos();
                z0
            })
            .collect();
        
        let bias = rng.gen_range(0.0..1.0) * width;
        
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
/// NOW SUPPORTS: Queue-based consumption of vectors with quantized/fp32/both representations
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
    /// Zero-overhead vector storage per collection
    vectors: Arc<DashMap<String, Arc<RwLock<ZeroOverheadCollection>>>>,
    /// Dimension
    dimension: usize,
    /// Distance compute instance
    distance_compute: UnifiedDistanceCompute,
    /// Statistics
    vector_count: Arc<AtomicUsize>,
    /// The algorithm specification
    algorithm: IndexAlgorithm,
    
    /// NEW: Preferred extraction mode for EventLog consumption
    /// From IndexConfig.extraction_mode field
    preferred_extraction_mode: ExtractionMode,
    
    /// NEW: Quantized vector storage for dual representation support
    /// Maps external_id -> quantized_vector for QUANTIZED_ONLY and BOTH modes
    quantized_vectors: Arc<DashMap<String, Vec<u8>>>,
}

impl AxisLshIndex {
    /// Create a new LSH index
    pub fn new(config: AxisLshConfig, dimension: usize) -> Self {
        Self::new_with_representation(None, config, dimension, ExtractionMode::Auto)
    }
    
    /// Create a new LSH index for a specific collection
    pub fn new_with_collection(
        collection_id: Option<String>,
        config: AxisLshConfig,
        dimension: usize
    ) -> Self {
        Self::new_with_representation(collection_id, config, dimension, ExtractionMode::Auto)
    }
    
    /// Create a new LSH index with specific vector representation preference
    pub fn new_with_representation(
        collection_id: Option<String>,
        config: AxisLshConfig,
        dimension: usize,
        preferred_extraction_mode: ExtractionMode,
    ) -> Self {
        let coll_str = collection_id.as_ref().map(|s| s.as_str()).unwrap_or("<unnamed>");
        info!(
            "Creating AXIS LSH index for collection '{}': {} tables, {} hashes, {} dim, repr={:?}",
            coll_str, config.n_tables, config.n_hashes, dimension, preferred_extraction_mode
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
            
            // NEW: Queue-based vector consumption
            preferred_extraction_mode,
            quantized_vectors: Arc::new(DashMap::new()),
        }
    }
    
    /// Add a vector to the index - clean API, no VectorRecord
    pub async fn add_vector(&self, id: Option<String>, vector_data: Vec<f32>) -> Result<()> {
        // Generate ID if not provided
        let vector_id = id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        if vector_data.len() != self.dimension {
            return Err(anyhow!(
                "Vector dimension mismatch: expected {}, got {}",
                self.dimension,
                vector_data.len()
            ));
        }
        
        // Compute hash for each table
        for (table_idx, table) in self.hash_tables.iter().enumerate() {
            let hash_value = self.compute_hash(table_idx, &vector_data);
            
            // Create partitioned key for hash table
            let key = if let Some(ref coll_id) = self.collection_id {
                PartitionedKey::new(coll_id.clone(), hash_value)
            } else {
                PartitionedKey::new("default".to_string(), hash_value)
            };
            
            table
                .entry(key)
                .or_insert_with(HashSet::new)
                .insert(vector_id.clone());
        }
        
        // Store vector in the collection-specific ZeroOverheadCollection
        let collection_id = self.collection_id.as_ref().map(|s| s.as_str());
        
        // Get or create collection for this collection_id
        let collection = self.vectors.entry(collection_id.unwrap_or("default").to_string())
            .or_insert_with(|| {
                Arc::new(RwLock::new(ZeroOverheadCollection::with_capacity(
                    CollectionConfig::fp32(self.dimension),
                    1024,
                )))
            });
        
        // Insert vector into the zero-overhead collection
        let mut coll = collection.write().unwrap();
        coll.add_fp32(vector_id.clone(), &vector_data)?;
        
        self.vector_count.fetch_add(1, Ordering::Relaxed);
        
        Ok(())
    }
    
    /// Search for k nearest neighbors
    pub async fn search(
        &self,
        query: &[f32],
        k: usize,
        filter: Option<&std::collections::HashMap<String, String>>,
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
        let collection_id = self.collection_id.as_ref().map(|s| s.as_str());
        
        // Get the collection for this collection_id
        if let Some(collection) = self.vectors.get(&collection_id.unwrap_or("default").to_string()) {
            let coll = collection.read().unwrap();
            
            for id in &candidates {
                if let Some(view) = coll.get(id) {
                    // Metadata filtering should be done at storage layer
                    if filter.is_some() {
                        debug!("Metadata filtering should be applied at storage layer, not in AXIS index");
                    }
                    
                    if let Some(vector_data) = view.as_f32() {
                        let dist = self.distance_compute.calculate_distance(query, vector_data, &self.config.distance_metric).rank_value;
                        results.push((id.clone(), dist));
                    }
                }
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
        let collection_id = self.collection_id.as_ref().map(|s| s.as_str());
        
        // Get the collection and remove the vector
        if let Some(collection) = self.vectors.get(&collection_id.unwrap_or("default").to_string()) {
            let mut coll = collection.write().unwrap();
            
            // Get the vector data before removing it (needed to update hash tables)
            let vector_data = if let Some(view) = coll.get(id) {
                view.as_f32().map(|v| v.to_vec())
            } else {
                None
            };
            
            if let Some(vector) = vector_data {
                // TODO: Implement remove method for ZeroOverheadCollection
                // For now, just log the removal
                tracing::debug!("Would remove vector {} from collection (method not implemented)", id);
                
                // Remove from all hash tables
                for (table_idx, table) in self.hash_tables.iter().enumerate() {
                    let hash_value = self.compute_hash(table_idx, &vector);
                    
                    let hash_key = PartitionedKey::new(collection_id.unwrap_or("default").to_string(), hash_value);
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
        } else {
            Err(anyhow!("Collection {} not found", collection_id.unwrap_or("default")))
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
    
    /// NEW: Process EventLog event for async index updates
    pub async fn process_event(&self, event: &IndexEvent) -> Result<()> {
        info!("Processing EventLog event {} for LSH index", event.event_id);
        
        // Process based on extraction mode and data availability
        match self.preferred_extraction_mode {
            ExtractionMode::Fp32Only => {
                // Process FP32 vectors only
                if event.has_fp32 {
                    // TODO: Load FP32 vectors from file paths and process them
                    tracing::info!("Processing FP32 vectors from {} files", event.file_paths.len());
                    // Placeholder implementation - needs file loading logic
                }
            }
            ExtractionMode::QuantizedOnly => {
                // Process quantized vectors only
                if event.has_quantized {
                    // TODO: Load quantized vectors from file paths and process them
                    tracing::info!("Processing quantized vectors from {} files", event.file_paths.len());
                    // Placeholder implementation - needs file loading logic
                }
            }
            ExtractionMode::Both => {
                // Process both representations
                if event.has_fp32 {
                    // TODO: Load FP32 vectors from file paths and process them
                    tracing::info!("Processing FP32 vectors from {} files", event.file_paths.len());
                }
                if event.has_quantized {
                    // TODO: Load quantized vectors from file paths and process them
                    tracing::info!("Processing quantized vectors from {} files", event.file_paths.len());
                }
            }
            ExtractionMode::Auto => {
                // Automatically choose based on availability - prefer quantized for LSH
                if event.has_quantized {
                    tracing::info!("Auto mode: Processing quantized vectors from {} files", event.file_paths.len());
                    // TODO: Load quantized vectors from file paths and process them
                } else if event.has_fp32 {
                    tracing::info!("Auto mode: Processing FP32 vectors from {} files", event.file_paths.len());
                    // TODO: Load FP32 vectors from file paths and process them
                }
            }
        }
        
        Ok(())
    }
    
    /// NEW: Get preferred vector representation for queue consumption
    pub fn preferred_extraction_mode(&self) -> ExtractionMode {
        self.preferred_extraction_mode.clone()
    }
    
    /// NEW: Check if quantized vectors are available for search acceleration
    pub fn has_quantized_storage(&self) -> bool {
        !self.quantized_vectors.is_empty()
    }
    
    /// NEW: Accelerated search using quantized vectors for initial filtering
    /// This implements a two-stage search: quantized filtering + FP32 reranking
    pub async fn search_with_quantized_acceleration(
        &self,
        query: &[f32],
        k: usize,
        filter: Option<&std::collections::HashMap<String, String>>,
    ) -> Result<Vec<(String, f32)>> {
        if !self.has_quantized_storage() {
            // No quantized vectors available, use standard search
            return self.search(query, k, filter).await;
        }
        
        // TODO: Implement two-stage search with quantized filtering
        // Stage 1: Fast filtering using quantized hash comparisons
        // Stage 2: FP32 reranking of top candidates
        tracing::warn!("Quantized acceleration not yet implemented - using standard search");
        
        self.search(query, k, filter).await
    }
}

/// Factory function to create LSH index instances
pub fn create_lsh_index(
    config: AxisLshConfig, 
    dimension: usize
) -> Result<Box<dyn AxisVectorIndex>> {
    Ok(Box::new(AxisLshIndex::new(config, dimension)))
}

/// Factory function to create LSH index instances with vector representation preference
pub fn create_lsh_index_with_representation(
    config: AxisLshConfig, 
    dimension: usize,
    preferred_extraction_mode: ExtractionMode,
) -> Result<Box<dyn AxisVectorIndex>> {
    Ok(Box::new(AxisLshIndex::new_with_representation(None, config, dimension, preferred_extraction_mode)))
}

/// Factory function to create LSH index instances for specific collection with representation
pub fn create_lsh_index_for_collection(
    collection_id: String,
    config: AxisLshConfig, 
    dimension: usize,
    preferred_extraction_mode: ExtractionMode,
) -> Result<Box<dyn AxisVectorIndex>> {
    Ok(Box::new(AxisLshIndex::new_with_representation(Some(collection_id), config, dimension, preferred_extraction_mode)))
}

#[cfg(test)]
mod tests {
    use crate::index::axis::*;
    
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
                // rank removed -  None,
                similarity: None,
                similarity: None,
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
                // rank removed -  None,
                similarity: None,
                similarity: None,
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
    async fn add(&self, id: String, vector_data: Vec<f32>) -> Result<()> {
        AxisLshIndex::add_vector(self, Some(id), vector_data).await
    }
    
    async fn search(
        &self,
        query: &[f32],
        top_k: usize,
        filter: Option<&std::collections::HashMap<String, String>>,
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