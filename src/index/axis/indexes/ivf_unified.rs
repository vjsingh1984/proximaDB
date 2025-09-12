/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Unified IVF index with dual-store architecture
//!
//! This module provides a single IVF implementation that internally manages:
//! - Inelastic centroid store (always in memory)
//! - Elastic posting list store (tierable)
//! Both stores are properly partitioned by collection_id.

use anyhow::{Result, anyhow};
use dashmap::DashMap;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tracing::info;

use crate::compute::distance_computation::DistanceMetric;
use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use crate::index::axis::types::IndexAlgorithm;
use crate::infrastructure::adaptive_structures::{
    AdaptiveStore, AdaptiveStoreConfig, BackendType, DemotionCriteria, EvictionPolicy,
    IndexStructure, MetricsConfig, PromotionCriteria, TierConfig, UnifiedTierPolicy,
};
use crate::infrastructure::tier_policy_engine::InfrastructureTier;
// VectorRecord eliminated - using ZeroOverheadVector for optimal memory
use crate::index::axis::clustering::{AxisClusteringEngine, ClusteringAlgorithm, ClusteringConfig};
use crate::index::axis::eventlog::{ExtractionMode, IndexEvent};
use crate::index::axis::zero_overhead_vector::{CollectionConfig, ZeroOverheadCollection};
use crate::proto::proximadb_v1::VectorRecord;

/// Partitioned key for collection-aware storage
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct PartitionedKey<K> {
    pub collection_id: String,
    pub key: K,
}

impl<K> PartitionedKey<K> {
    pub fn new(collection_id: String, key: K) -> Self {
        Self { collection_id, key }
    }
}

impl<K: std::fmt::Display> std::fmt::Display for PartitionedKey<K> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.collection_id, self.key)
    }
}

/// Clustering method for IVF training
#[derive(Debug, Clone)]
pub enum IvfClusteringMethod {
    /// Standard K-means (fast, reasonable quality)
    KMeans,
    /// K-means++ (better initialization, more accurate)
    KMeansPlusPlus,
    /// Mini-batch K-means (faster for large datasets)
    MiniBatchKMeans { batch_size: usize },
    /// Balanced K-means (ensures equal cluster sizes)
    BalancedKMeans,
    /// Hierarchical K-means (for very large K)
    HierarchicalKMeans { branching_factor: usize },
    /// Use external clustering engine
    External(ClusteringAlgorithm),
}

impl Default for IvfClusteringMethod {
    fn default() -> Self {
        Self::KMeansPlusPlus // Better than standard K-means
    }
}

/// Configuration for unified IVF index
#[derive(Debug, Clone)]
pub struct UnifiedIvfConfig {
    /// Number of clusters (n_lists for compatibility)
    pub n_clusters: usize,
    /// Number of clusters to probe during search
    pub n_probe: usize,
    /// Vector dimension
    pub dimension: usize,
    /// Distance metric
    pub distance_metric: DistanceMetric,

    // Quantization settings
    /// Bits for scalar quantization (0 = no quantization)
    pub quantization_bits: usize,
    /// Use product quantization
    pub use_pq: bool,
    /// Number of PQ subspaces (subquantizers)
    pub pq_subspaces: usize,

    // Training settings
    /// Clustering method for training
    pub clustering_method: IvfClusteringMethod,
    /// Retrain on every insert
    pub train_on_insert: bool,
    /// Minimum size to trigger training
    pub min_train_size: usize,
    /// Maximum iterations for clustering
    pub max_iterations: usize,
    /// Convergence tolerance
    pub tolerance: f32,
    /// Number of training runs (for stability)
    pub n_init: usize,

    // Centroid store config (inelastic)
    pub centroid_config: CentroidConfig,

    // Posting list store config (elastic)
    pub posting_list_config: PostingListConfig,
}

impl Default for UnifiedIvfConfig {
    fn default() -> Self {
        Self {
            n_clusters: 1000,
            n_probe: 1,
            dimension: 0, // Must be set
            distance_metric: DistanceMetric::Euclidean,
            quantization_bits: 8,
            use_pq: false,
            pq_subspaces: 8,
            clustering_method: IvfClusteringMethod::default(),
            train_on_insert: false,
            min_train_size: 1000,
            max_iterations: 20,
            tolerance: 1e-4,
            n_init: 3, // Run clustering 3 times for stability
            centroid_config: CentroidConfig::default(),
            posting_list_config: PostingListConfig::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CentroidConfig {
    /// Centroids are never evicted
    pub evictable: bool, // Always false
    /// Priority for memory allocation
    pub priority: MemoryPriority,
    /// Minimum memory guarantee
    pub min_memory_guarantee: bool, // Always true
}

impl Default for CentroidConfig {
    fn default() -> Self {
        Self {
            evictable: false,
            priority: MemoryPriority::Critical,
            min_memory_guarantee: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PostingListConfig {
    /// Posting lists can be evicted
    pub evictable: bool, // Always true
    /// Access count to be considered hot
    pub promotion_threshold: usize,
    /// Seconds idle before demotion
    pub demotion_threshold: u64,
    /// Maximum memory for posting lists (MB)
    pub max_memory_mb: usize,
    /// Enable predictive prefetch
    pub enable_prefetch: bool,
}

impl Default for PostingListConfig {
    fn default() -> Self {
        Self {
            evictable: true,
            promotion_threshold: 100,
            demotion_threshold: 3600,
            max_memory_mb: 1500,
            enable_prefetch: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum MemoryPriority {
    Critical, // Never evict (centroids)
    High,     // Evict last (hot posting lists)
    Normal,   // Standard eviction (warm posting lists)
    Low,      // Evict first (cold posting lists)
}

/// Inelastic centroid store - always in memory
struct CentroidStore {
    centroids: Arc<Vec<Vec<f32>>>,
    dimension: usize,
    trained: bool,

    // Small metadata always in memory
    cluster_sizes: Vec<AtomicUsize>,
    cluster_stats: Vec<ClusterStats>,
}

#[derive(Debug, Clone, Default)]
struct ClusterStats {
    pub vector_count: usize,
    pub last_updated: Option<Instant>,
    pub variance: f32,
}

impl CentroidStore {
    fn new(n_clusters: usize, dimension: usize) -> Self {
        Self {
            centroids: Arc::new(Vec::with_capacity(n_clusters)),
            dimension,
            trained: false,
            cluster_sizes: (0..n_clusters).map(|_| AtomicUsize::new(0)).collect(),
            cluster_stats: vec![ClusterStats::default(); n_clusters],
        }
    }

    fn is_trained(&self) -> bool {
        self.trained
    }

    fn train(&mut self, training_vectors: &[Vec<f32>]) -> Result<()> {
        use rand::seq::SliceRandom;

        info!(
            "Training IVF centroids with {} vectors",
            training_vectors.len()
        );

        if training_vectors.is_empty() {
            return Err(anyhow!("Cannot train with empty vectors"));
        }

        let n_clusters = self.centroids.capacity();
        let dimension = training_vectors[0].len();

        // K-means++ initialization
        let mut rng = rand::thread_rng();
        let mut centroids = Vec::with_capacity(n_clusters);

        // Choose first centroid randomly
        centroids.push(training_vectors.choose(&mut rng).unwrap().clone());

        // Choose remaining centroids with K-means++ probability
        for _ in 1..n_clusters {
            let mut distances = Vec::with_capacity(training_vectors.len());

            for vector in training_vectors {
                let min_dist = centroids
                    .iter()
                    .map(|c| euclidean_distance(vector, c))
                    .min_by(|a, b| a.partial_cmp(b).unwrap())
                    .unwrap();
                distances.push(min_dist * min_dist); // Square for probability
            }

            // Choose next centroid with probability proportional to squared distance
            let total: f32 = distances.iter().sum();
            let mut threshold = rng.gen_range(0.0..1.0) * total;

            for (idx, &dist) in distances.iter().enumerate() {
                threshold -= dist;
                if threshold <= 0.0 {
                    centroids.push(training_vectors[idx].clone());
                    break;
                }
            }
        }

        // Run K-means iterations
        let max_iter = 20;
        let tolerance = 1e-4;

        for iter in 0..max_iter {
            // Assign vectors to clusters
            let mut clusters: Vec<Vec<&Vec<f32>>> = vec![Vec::new(); n_clusters];

            for vector in training_vectors {
                let mut min_dist = f32::MAX;
                let mut best_cluster = 0;

                for (idx, centroid) in centroids.iter().enumerate() {
                    let dist = euclidean_distance(vector, centroid);
                    if dist < min_dist {
                        min_dist = dist;
                        best_cluster = idx;
                    }
                }

                clusters[best_cluster].push(vector);
            }

            // Update centroids
            let mut converged = true;
            for (idx, cluster) in clusters.iter().enumerate() {
                if !cluster.is_empty() {
                    let mut new_centroid = vec![0.0; dimension];

                    for vector in cluster {
                        for (i, &val) in vector.iter().enumerate() {
                            new_centroid[i] += val;
                        }
                    }

                    for val in &mut new_centroid {
                        *val /= cluster.len() as f32;
                    }

                    // Check convergence
                    let movement = euclidean_distance(&centroids[idx], &new_centroid);
                    if movement > tolerance {
                        converged = false;
                    }

                    centroids[idx] = new_centroid;
                }
            }

            if converged {
                info!("K-means converged after {} iterations", iter + 1);
                break;
            }
        }

        // Store centroids
        self.centroids = Arc::new(centroids);
        self.trained = true;

        // Update cluster stats
        for i in 0..n_clusters {
            self.cluster_stats[i] = ClusterStats {
                vector_count: 0,
                last_updated: Some(Instant::now()),
                variance: 0.0,
            };
        }

        Ok(())
    }

    fn find_nearest_centroid(
        &self,
        vector: &[f32],
        distance_compute: &UnifiedDistanceCompute,
    ) -> usize {
        let mut min_dist = f32::MAX;
        let mut nearest = 0;

        for (idx, centroid) in self.centroids.iter().enumerate() {
            let dist =
                distance_compute.calculate_distance(vector, centroid, &DistanceMetric::Euclidean);
            if dist.rank_value < min_dist {
                min_dist = dist.rank_value;
                nearest = idx;
            }
        }

        nearest
    }

    fn find_nearest_centroids(
        &self,
        vector: &[f32],
        n: usize,
        distance_compute: &UnifiedDistanceCompute,
    ) -> Vec<(usize, f32)> {
        let mut distances: Vec<(usize, f32)> = self
            .centroids
            .iter()
            .enumerate()
            .map(|(idx, centroid)| {
                let dist = distance_compute.calculate_distance(
                    vector,
                    centroid,
                    &DistanceMetric::Euclidean,
                );
                (idx, dist.rank_value)
            })
            .collect();

        distances.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        distances.truncate(n);
        distances
    }

    fn memory_usage_bytes(&self) -> usize {
        let centroids_size = self.centroids.len() * self.dimension * std::mem::size_of::<f32>();
        let metadata_size = self.cluster_sizes.len() * std::mem::size_of::<AtomicUsize>()
            + self.cluster_stats.len() * std::mem::size_of::<ClusterStats>();

        centroids_size + metadata_size
    }
}

/// Posting list that can be tiered
#[derive(Debug, Clone)]
pub struct PostingList {
    pub cluster_id: usize,
    pub vector_ids: Vec<String>,
    pub vectors: Option<Vec<Vec<f32>>>, // None when on disk
    pub quantized_vectors: Option<Vec<Vec<u8>>>, // PQ codes when enabled
    pub last_access: u64,               // Unix timestamp
    pub access_count: u64,
}

/// Product Quantizer for vector compression
pub struct ProductQuantizer {
    /// Number of subspaces
    pub n_subspaces: usize,
    /// Dimension per subspace
    pub subspace_dim: usize,
    /// Codebook for each subspace (256 centroids per subspace)
    pub codebooks: Vec<Vec<Vec<f32>>>,
    /// Number of bits per code (usually 8)
    pub bits_per_code: usize,
}

impl ProductQuantizer {
    pub fn new(dimension: usize, n_subspaces: usize) -> Self {
        let subspace_dim = dimension / n_subspaces;
        Self {
            n_subspaces,
            subspace_dim,
            codebooks: vec![vec![vec![0.0; subspace_dim]; 256]; n_subspaces],
            bits_per_code: 8,
        }
    }

    /// Train PQ codebooks on training data
    pub fn train(&mut self, vectors: &[Vec<f32>]) -> Result<()> {
        for subspace_idx in 0..self.n_subspaces {
            let start_idx = subspace_idx * self.subspace_dim;
            let end_idx = start_idx + self.subspace_dim;

            // Extract subspace vectors
            let subspace_vectors: Vec<Vec<f32>> = vectors
                .iter()
                .map(|v| v[start_idx..end_idx].to_vec())
                .collect();

            // Train k-means with k=256 for this subspace
            let centroids = self.train_subspace_kmeans(&subspace_vectors, 256)?;
            self.codebooks[subspace_idx] = centroids;
        }
        Ok(())
    }

    /// Encode a vector using PQ
    pub fn encode(&self, vector: &[f32]) -> Vec<u8> {
        let mut codes = Vec::with_capacity(self.n_subspaces);

        for subspace_idx in 0..self.n_subspaces {
            let start_idx = subspace_idx * self.subspace_dim;
            let end_idx = start_idx + self.subspace_dim;
            let subvector = &vector[start_idx..end_idx];

            // Find nearest centroid in codebook
            let mut min_dist = f32::MAX;
            let mut best_code = 0u8;

            for (code, centroid) in self.codebooks[subspace_idx].iter().enumerate() {
                let dist = euclidean_distance(subvector, centroid);
                if dist < min_dist {
                    min_dist = dist;
                    best_code = code as u8;
                }
            }

            codes.push(best_code);
        }

        codes
    }

    /// Decode PQ codes back to approximate vector
    pub fn decode(&self, codes: &[u8]) -> Vec<f32> {
        let mut vector = Vec::with_capacity(self.n_subspaces * self.subspace_dim);

        for (subspace_idx, &code) in codes.iter().enumerate() {
            let centroid = &self.codebooks[subspace_idx][code as usize];
            vector.extend_from_slice(centroid);
        }

        vector
    }

    /// Compute asymmetric distance between query and PQ codes
    pub fn asymmetric_distance(&self, query: &[f32], codes: &[u8]) -> f32 {
        let mut total_dist = 0.0;

        for subspace_idx in 0..self.n_subspaces {
            let start_idx = subspace_idx * self.subspace_dim;
            let end_idx = start_idx + self.subspace_dim;
            let subquery = &query[start_idx..end_idx];

            let code = codes[subspace_idx] as usize;
            let centroid = &self.codebooks[subspace_idx][code];

            total_dist += euclidean_distance(subquery, centroid);
        }

        total_dist
    }

    fn train_subspace_kmeans(&self, vectors: &[Vec<f32>], k: usize) -> Result<Vec<Vec<f32>>> {
        // Simple k-means implementation for subspace
        // In production, use optimized k-means from clustering module
        use rand::seq::SliceRandom;
        let mut rng = rand::thread_rng();

        // Random initialization
        let mut centroids: Vec<Vec<f32>> = vectors.choose_multiple(&mut rng, k).cloned().collect();

        // Run iterations (simplified)
        for _ in 0..10 {
            // Assign points to clusters
            let mut clusters: Vec<Vec<Vec<f32>>> = vec![Vec::new(); k];

            for vector in vectors {
                let mut min_dist = f32::MAX;
                let mut best_cluster = 0;

                for (idx, centroid) in centroids.iter().enumerate() {
                    let dist = euclidean_distance(vector, centroid);
                    if dist < min_dist {
                        min_dist = dist;
                        best_cluster = idx;
                    }
                }

                clusters[best_cluster].push(vector.clone());
            }

            // Update centroids
            for (idx, cluster) in clusters.iter().enumerate() {
                if !cluster.is_empty() {
                    let dim = cluster[0].len();
                    let mut new_centroid = vec![0.0; dim];

                    for vector in cluster {
                        for (i, &val) in vector.iter().enumerate() {
                            new_centroid[i] += val;
                        }
                    }

                    for val in &mut new_centroid {
                        *val /= cluster.len() as f32;
                    }

                    centroids[idx] = new_centroid;
                }
            }
        }

        Ok(centroids)
    }
}

fn euclidean_distance(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x - y).powi(2))
        .sum::<f32>()
        .sqrt()
}

/// Unified IVF index with dual stores
/// NOW SUPPORTS: Queue-based consumption of vectors with quantized/fp32/both representations
pub struct UnifiedIvfIndex {
    /// Collection identifier for partitioning
    collection_id: String,

    /// INELASTIC: Centroid store (always in memory)
    centroids: CentroidStore,

    /// ELASTIC: Posting list store (tierable)
    posting_lists: Arc<dyn AdaptiveStore<PartitionedKey<usize>, PostingList>>,

    /// Vector storage (separate from posting lists for flexibility)
    // Zero-overhead vector storage per collection
    vectors: Arc<DashMap<String, Arc<RwLock<ZeroOverheadCollection>>>>,

    /// Product Quantizer (optional, for compression)
    product_quantizer: Option<Arc<ProductQuantizer>>,

    /// Distance computation
    distance_compute: UnifiedDistanceCompute,

    /// Configuration
    config: UnifiedIvfConfig,

    /// Algorithm configuration
    algorithm: IndexAlgorithm,

    /// Global statistics
    vector_count: Arc<AtomicUsize>,
    search_count: Arc<AtomicU64>,

    /// Access pattern tracking for prefetch
    access_correlations: Arc<DashMap<usize, Vec<(usize, f32)>>>,

    /// NEW: Preferred extraction mode for EventLog consumption
    /// From IndexConfig.extraction_mode field
    preferred_extraction_mode: ExtractionMode,

    /// NEW: Quantized vector storage for dual representation support
    /// Maps external_id -> quantized_vector for QUANTIZED_ONLY and BOTH modes
    quantized_vectors: Arc<DashMap<String, Vec<u8>>>,
}

impl UnifiedIvfIndex {
    /// Train mini-batch K-means (more efficient for large datasets)
    async fn train_minibatch_kmeans(
        &self,
        vectors: &[Vec<f32>],
        batch_size: usize,
    ) -> Result<Arc<Vec<Vec<f32>>>> {
        use rand::seq::SliceRandom;
        let mut rng = rand::thread_rng();

        let n_clusters = self.config.n_clusters;
        let dimension = vectors[0].len();

        // Initialize centroids with K-means++
        let mut centroids = self.kmeans_plusplus_init(vectors, n_clusters)?;

        // Mini-batch iterations
        let n_iterations = self.config.max_iterations;
        let n_samples = vectors.len();

        for _ in 0..n_iterations {
            // Sample a mini-batch
            let batch: Vec<&Vec<f32>> = vectors
                .choose_multiple(&mut rng, batch_size.min(n_samples))
                .collect();

            // Update centroids based on mini-batch
            let mut cluster_counts = vec![0usize; n_clusters];
            let mut cluster_sums = vec![vec![0.0; dimension]; n_clusters];

            for vector in batch {
                let nearest = self.find_nearest_centroid_idx(vector, &centroids);
                cluster_counts[nearest] += 1;

                for (i, &val) in vector.iter().enumerate() {
                    cluster_sums[nearest][i] += val;
                }
            }

            // Update centroids with learning rate
            let learning_rate = 0.1;
            for (idx, count) in cluster_counts.iter().enumerate() {
                if *count > 0 {
                    for i in 0..dimension {
                        let new_val = cluster_sums[idx][i] / *count as f32;
                        centroids[idx][i] =
                            (1.0 - learning_rate) * centroids[idx][i] + learning_rate * new_val;
                    }
                }
            }
        }

        Ok(Arc::new(centroids))
    }

    /// Train balanced K-means (ensures roughly equal cluster sizes)
    async fn train_balanced_kmeans(&self, vectors: &[Vec<f32>]) -> Result<Arc<Vec<Vec<f32>>>> {
        let n_clusters = self.config.n_clusters;
        let n_vectors = vectors.len();
        let target_size = n_vectors / n_clusters;

        // Start with regular K-means
        let mut centroids = self.kmeans_plusplus_init(vectors, n_clusters)?;

        // Iteratively balance clusters
        for _ in 0..self.config.max_iterations {
            // Assign vectors with size constraints
            let mut assignments = vec![0; n_vectors];
            let mut cluster_sizes = vec![0; n_clusters];

            // Sort vectors by distance to nearest centroid
            let mut vector_distances: Vec<(usize, usize, f32)> = Vec::new();
            for (v_idx, vector) in vectors.iter().enumerate() {
                let (c_idx, dist) = self.find_nearest_centroid_with_distance(vector, &centroids);
                vector_distances.push((v_idx, c_idx, dist));
            }
            vector_distances.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap());

            // Assign vectors respecting balance
            for (v_idx, c_idx, _) in vector_distances {
                if cluster_sizes[c_idx] < target_size + target_size / 10 {
                    assignments[v_idx] = c_idx;
                    cluster_sizes[c_idx] += 1;
                } else {
                    // Find alternative cluster
                    let alt_cluster = self.find_alternative_cluster(
                        &vectors[v_idx],
                        &centroids,
                        &cluster_sizes,
                        target_size,
                    );
                    assignments[v_idx] = alt_cluster;
                    cluster_sizes[alt_cluster] += 1;
                }
            }

            // Update centroids based on balanced assignments
            centroids = self.update_centroids_from_assignments(vectors, &assignments, n_clusters);
        }

        Ok(Arc::new(centroids))
    }

    /// Train hierarchical K-means (for very large K)
    async fn train_hierarchical_kmeans(
        &self,
        vectors: &[Vec<f32>],
        _branching_factor: usize,
    ) -> Result<Arc<Vec<Vec<f32>>>> {
        // Two-level clustering: first coarse, then fine
        let n_coarse = (self.config.n_clusters as f64).sqrt() as usize;
        let n_fine_per_coarse = self.config.n_clusters / n_coarse;

        // Train coarse clusters
        let coarse_centroids = self.kmeans_plusplus_init(vectors, n_coarse)?;

        // Assign vectors to coarse clusters
        let mut coarse_assignments = vec![Vec::new(); n_coarse];
        for (idx, vector) in vectors.iter().enumerate() {
            let nearest = self.find_nearest_centroid_idx(vector, &coarse_centroids);
            coarse_assignments[nearest].push(idx);
        }

        // Train fine clusters within each coarse cluster
        let mut all_centroids = Vec::new();
        for coarse_vectors_idx in coarse_assignments {
            if !coarse_vectors_idx.is_empty() {
                let coarse_vectors: Vec<Vec<f32>> = coarse_vectors_idx
                    .iter()
                    .map(|&idx| vectors[idx].clone())
                    .collect();

                let n_fine = n_fine_per_coarse.min(coarse_vectors.len());
                if n_fine > 0 {
                    let fine_centroids = self.kmeans_plusplus_init(&coarse_vectors, n_fine)?;
                    all_centroids.extend(fine_centroids);
                }
            }
        }

        // Ensure we have exactly n_clusters centroids
        while all_centroids.len() < self.config.n_clusters {
            all_centroids.push(vectors[all_centroids.len() % vectors.len()].clone());
        }
        all_centroids.truncate(self.config.n_clusters);

        Ok(Arc::new(all_centroids))
    }

    /// K-means++ initialization
    fn kmeans_plusplus_init(&self, vectors: &[Vec<f32>], k: usize) -> Result<Vec<Vec<f32>>> {
        use rand::Rng;
        use rand::seq::SliceRandom;

        let mut rng = rand::thread_rng();
        let mut centroids = Vec::with_capacity(k);

        // Choose first centroid randomly
        centroids.push(vectors.choose(&mut rng).unwrap().clone());

        // Choose remaining centroids
        for _ in 1..k {
            let mut distances = Vec::with_capacity(vectors.len());

            for vector in vectors {
                let min_dist = centroids
                    .iter()
                    .map(|c| euclidean_distance(vector, c))
                    .min_by(|a, b| a.partial_cmp(b).unwrap())
                    .unwrap();
                distances.push(min_dist * min_dist);
            }

            // Choose next centroid with probability proportional to squared distance
            let total: f32 = distances.iter().sum();
            let mut threshold = rng.gen_range(0.0..1.0) * total;

            for (idx, &dist) in distances.iter().enumerate() {
                threshold -= dist;
                if threshold <= 0.0 {
                    centroids.push(vectors[idx].clone());
                    break;
                }
            }
        }

        Ok(centroids)
    }

    /// Helper functions for clustering
    fn find_nearest_centroid_idx(&self, vector: &[f32], centroids: &[Vec<f32>]) -> usize {
        let mut min_dist = f32::MAX;
        let mut nearest = 0;

        for (idx, centroid) in centroids.iter().enumerate() {
            let dist = euclidean_distance(vector, centroid);
            if dist < min_dist {
                min_dist = dist;
                nearest = idx;
            }
        }

        nearest
    }

    fn find_nearest_centroid_with_distance(
        &self,
        vector: &[f32],
        centroids: &[Vec<f32>],
    ) -> (usize, f32) {
        let mut min_dist = f32::MAX;
        let mut nearest = 0;

        for (idx, centroid) in centroids.iter().enumerate() {
            let dist = euclidean_distance(vector, centroid);
            if dist < min_dist {
                min_dist = dist;
                nearest = idx;
            }
        }

        (nearest, min_dist)
    }

    fn find_alternative_cluster(
        &self,
        vector: &[f32],
        centroids: &[Vec<f32>],
        cluster_sizes: &[usize],
        target_size: usize,
    ) -> usize {
        let mut candidates: Vec<(usize, f32)> = centroids
            .iter()
            .enumerate()
            .filter(|(idx, _)| cluster_sizes[*idx] < target_size)
            .map(|(idx, c)| (idx, euclidean_distance(vector, c)))
            .collect();

        candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        candidates.first().map(|c| c.0).unwrap_or(0)
    }

    fn update_centroids_from_assignments(
        &self,
        vectors: &[Vec<f32>],
        assignments: &[usize],
        n_clusters: usize,
    ) -> Vec<Vec<f32>> {
        let dimension = vectors[0].len();
        let mut centroids = vec![vec![0.0; dimension]; n_clusters];
        let mut counts = vec![0; n_clusters];

        for (v_idx, &c_idx) in assignments.iter().enumerate() {
            counts[c_idx] += 1;
            for (i, &val) in vectors[v_idx].iter().enumerate() {
                centroids[c_idx][i] += val;
            }
        }

        for (c_idx, count) in counts.iter().enumerate() {
            if *count > 0 {
                for val in &mut centroids[c_idx] {
                    *val /= *count as f32;
                }
            }
        }

        centroids
    }

    pub fn new(collection_id: String, config: UnifiedIvfConfig) -> Result<Self> {
        Self::new_with_extraction_mode(collection_id, config, ExtractionMode::Fp32Only)
    }

    /// Create IVF index with specific extraction mode preference
    pub fn new_with_extraction_mode(
        collection_id: String,
        config: UnifiedIvfConfig,
        preferred_extraction_mode: ExtractionMode,
    ) -> Result<Self> {
        info!(
            "Creating unified IVF index for collection '{}': {} clusters, {} probe, mode={:?}",
            collection_id, config.n_clusters, config.n_probe, preferred_extraction_mode
        );

        // Create inelastic centroid store
        let centroids = CentroidStore::new(config.n_clusters, config.dimension);

        // Create elastic posting list store with collection partitioning
        let _posting_store_config = AdaptiveStoreConfig {
            collection_id: collection_id.clone(),
            backend_type: BackendType::Index {
                structure: IndexStructure::DashMap {
                    initial_capacity: 1000,
                    memory_limit_mb: Some(config.posting_list_config.max_memory_mb),
                },
                tier_policy: UnifiedTierPolicy {
                    eviction_policy: EvictionPolicy::Lru { max_entries: 10000 },
                    promotion_criteria: PromotionCriteria {
                        min_access_frequency: config.posting_list_config.promotion_threshold as u64,
                        frequency_window: Duration::from_secs(300),
                        min_promotion_tier: InfrastructureTier::Memory,
                    },
                    demotion_criteria: DemotionCriteria {
                        max_idle_time: Duration::from_secs(
                            config.posting_list_config.demotion_threshold as u64,
                        ),
                        memory_pressure_threshold: 0.85,
                        min_tier: InfrastructureTier::Memory,
                    },
                    reload_strategy: crate::infrastructure::adaptive_structures::ReloadStrategy {
                        load_on_startup: false,
                        prefetch_hot_data: false,
                        max_initial_load: 0,
                        axis_storage_path: format!("/tmp/axis/{}", collection_id),
                    },
                },
            },
            tier_config: TierConfig {
                enable_tiering: true,
                rebalance_interval: Duration::from_secs(600), // Optimized: 60s -> 600s (10 minutes)
                memory_pressure_threshold: 0.8,
                max_concurrent_operations: 4,
            },
            metrics_config: MetricsConfig {
                enable_workload_metrics: true,
                collection_interval: Duration::from_secs(60), // Optimized: 10s -> 60s (1 minute)
                history_retention: Duration::from_secs(3600),
            },
        };

        // Use proper IndexBackend implementation for production performance
        // Replaces simple wrapper with full-featured backend
        struct SimpleAdaptiveStore<K, V> {
            store: Arc<DashMap<K, V>>,
        }

        #[async_trait::async_trait]
        impl<K, V> AdaptiveStore<K, V> for SimpleAdaptiveStore<K, V>
        where
            K: Hash + Eq + Clone + Send + Sync + 'static,
            V: Clone + Send + Sync + 'static,
        {
            async fn insert(&self, key: K, value: V) -> Result<Option<V>> {
                Ok(self.store.insert(key, value))
            }

            async fn get(&self, key: &K) -> Option<V> {
                self.store.get(key).map(|v| v.clone())
            }

            async fn remove(&self, key: &K) -> Option<V> {
                self.store.remove(key).map(|(_, v)| v)
            }

            async fn contains(&self, key: &K) -> bool {
                self.store.contains_key(key)
            }

            async fn len(&self) -> usize {
                self.store.len()
            }

            async fn is_empty(&self) -> bool {
                self.store.is_empty()
            }

            async fn keys(&self) -> Vec<K> {
                self.store.iter().map(|e| e.key().clone()).collect()
            }

            async fn clear(&self) {
                self.store.clear()
            }

            async fn metrics(
                &self,
            ) -> crate::infrastructure::concurrent_structures::MetricsSnapshot {
                Default::default()
            }

            async fn workload_metrics(
                &self,
            ) -> crate::infrastructure::tier_policy_engine::WorkloadMetrics {
                Default::default()
            }

            async fn rebalance_tiers(
                &self,
            ) -> anyhow::Result<crate::infrastructure::adaptive_structures::TierRebalanceResult>
            {
                Ok(Default::default())
            }
        }

        let posting_lists: Arc<dyn AdaptiveStore<PartitionedKey<usize>, PostingList>> =
            Arc::new(SimpleAdaptiveStore {
                store: Arc::new(DashMap::new()),
            });

        // Create distance compute
        let distance_compute = UnifiedDistanceCompute::new(config.distance_metric);

        Ok(Self {
            collection_id,
            centroids,
            posting_lists,
            vectors: Arc::new(DashMap::new()),
            distance_compute,
            algorithm: IndexAlgorithm::IVF {
                nlist: config.n_clusters as u32,
                nprobe: config.n_probe as u32,
                quantizer: None,
            },
            config,
            vector_count: Arc::new(AtomicUsize::new(0)),
            search_count: Arc::new(AtomicU64::new(0)),
            access_correlations: Arc::new(DashMap::new()),
            product_quantizer: None,

            // NEW: Queue-based vector consumption - handled externally
            preferred_extraction_mode,
            quantized_vectors: Arc::new(DashMap::new()),
        })
    }

    /// Train the index with sample vectors
    pub async fn train(&mut self, training_vectors: Vec<Vec<f32>>) -> Result<()> {
        if self.centroids.is_trained() {
            return Err(anyhow!("Index already trained"));
        }

        // Use the configured clustering method
        let centroids = match &self.config.clustering_method {
            IvfClusteringMethod::KMeans | IvfClusteringMethod::KMeansPlusPlus => {
                // Use built-in implementation
                self.centroids.train(&training_vectors)?;
                self.centroids.centroids.clone()
            }
            IvfClusteringMethod::MiniBatchKMeans { batch_size } => {
                self.train_minibatch_kmeans(&training_vectors, *batch_size)
                    .await?
            }
            IvfClusteringMethod::BalancedKMeans => {
                self.train_balanced_kmeans(&training_vectors).await?
            }
            IvfClusteringMethod::HierarchicalKMeans { branching_factor } => {
                self.train_hierarchical_kmeans(&training_vectors, *branching_factor)
                    .await?
            }
            IvfClusteringMethod::External(algorithm) => {
                // Use the external clustering engine
                let config = ClusteringConfig {
                    algorithm: algorithm.clone(),
                    min_vectors_for_clustering: self.config.min_train_size,
                    max_clusters: self.config.n_clusters,
                    distance_metric: DistanceMetric::Euclidean,
                    adaptive_cluster_count: true,
                    recompute_threshold: 10000,
                    enable_incremental: true,
                };

                let engine = AxisClusteringEngine::new(config);
                // Pass vectors directly to clustering engine
                // No need for VectorRecord conversion
                let vector_data: Vec<VectorRecord> = training_vectors
                    .iter()
                    .enumerate()
                    .map(|(i, v)| VectorRecord {
                        id: format!("training_{}", i),
                        vector: v.clone(),
                        metadata: std::collections::HashMap::new(),
                        timestamp: 0,
                        updated_at: None,
                        expires_at: None,
                        version: None,
                        quantized_vector: Vec::new(),
                        source: None,
                    })
                    .collect();
                let model = engine.train_model(&self.collection_id, vector_data).await?;
                Arc::new(model.centroids)
            }
        };

        // Store centroids
        self.centroids.centroids = centroids;
        self.centroids.trained = true;

        // Initialize empty posting lists for each cluster
        for cluster_id in 0..self.config.n_clusters {
            let key = PartitionedKey::new(self.collection_id.clone(), cluster_id);
            let posting_list = PostingList {
                cluster_id,
                vector_ids: Vec::new(),
                vectors: Some(Vec::new()), // Start in memory
                quantized_vectors: None,   // No PQ codes initially
                last_access: 0,
                access_count: 0,
            };

            self.posting_lists.insert(key, posting_list).await?;
        }

        Ok(())
    }

    /// Add a vector to the index
    pub async fn add_vector(
        &self,
        id: String,
        vector: Vec<f32>,
        metadata: Option<HashMap<String, serde_json::Value>>,
    ) -> Result<()> {
        if !self.centroids.is_trained() {
            return Err(anyhow!("Index must be trained before adding vectors"));
        }

        // Find nearest centroid
        let cluster_id = self
            .centroids
            .find_nearest_centroid(&vector, &self.distance_compute);

        // Update posting list
        let key = PartitionedKey::new(self.collection_id.clone(), cluster_id);

        // Get or create posting list
        let mut posting_list = match self.posting_lists.get(&key).await {
            Some(list) => list,
            None => PostingList {
                cluster_id,
                vector_ids: Vec::new(),
                vectors: Some(Vec::new()),
                quantized_vectors: None,
                last_access: 0,
                access_count: 0,
            },
        };

        // Add vector ID to posting list
        posting_list.vector_ids.push(id.clone());

        // If vectors are stored in posting list (for small clusters)
        if let Some(ref mut vectors) = posting_list.vectors {
            if vectors.len() < 1000 {
                // Keep small clusters in posting list
                vectors.push(vector.clone());
            } else {
                // Large clusters: store vectors separately
                posting_list.vectors = None;
            }
        }

        // Update posting list
        self.posting_lists.insert(key, posting_list).await?;

        // Store vector separately (for efficient random access)
        let _vector_key = PartitionedKey::new(self.collection_id.clone(), id.clone());

        // Convert HashMap metadata to Vec<MetadataItem>
        let _metadata_items = metadata
            .map(|map| {
                map.into_iter()
                    .map(|(key, value)| crate::proto::proximadb_v1::MetadataItem {
                        key,
                        value: Some(match value {
                            serde_json::Value::String(s) => {
                                crate::proto::proximadb_v1::metadata_item::Value::StringValue(s)
                            }
                            serde_json::Value::Number(n) => {
                                crate::proto::proximadb_v1::metadata_item::Value::NumberValue(
                                    n.as_f64().unwrap_or(0.0),
                                )
                            }
                            serde_json::Value::Bool(b) => {
                                crate::proto::proximadb_v1::metadata_item::Value::BoolValue(b)
                            }
                            _ => crate::proto::proximadb_v1::metadata_item::Value::StringValue(
                                value.to_string(),
                            ),
                        }),
                    })
                    .collect()
            })
            .unwrap_or_else(|| Vec::new());

        // Get or create zero-overhead collection for this collection_id
        let collections = self.vectors.clone();
        let collection = collections
            .entry(self.collection_id.clone())
            .or_insert_with(|| {
                let config = CollectionConfig::fp32(self.config.dimension);
                Arc::new(RwLock::new(ZeroOverheadCollection::with_capacity(
                    config, 1024,
                )))
            });

        // Add vector to zero-overhead collection
        {
            let mut coll = collection.write().unwrap();
            coll.add_fp32(id, &vector)?;
        }

        // Update statistics
        self.vector_count.fetch_add(1, Ordering::Relaxed);
        self.centroids.cluster_sizes[cluster_id].fetch_add(1, Ordering::Relaxed);

        Ok(())
    }

    /// Search for nearest neighbors
    pub async fn search(
        &self,
        query: &[f32],
        k: usize,
        n_probe: Option<usize>,
    ) -> Result<Vec<(String, f32)>> {
        if !self.centroids.is_trained() {
            return Err(anyhow!("Index must be trained before searching"));
        }

        let n_probe = n_probe.unwrap_or(1); // Default to 1 probe if not specified
        self.search_count.fetch_add(1, Ordering::Relaxed);

        // Step 1: Find nearest centroids (always in memory - fast)
        let nearest_clusters =
            self.centroids
                .find_nearest_centroids(query, n_probe, &self.distance_compute);

        // Step 2: Record access pattern for correlation learning
        self.record_access_pattern(&nearest_clusters).await;

        // Step 3: Predictive prefetch if enabled
        if self.config.posting_list_config.enable_prefetch {
            self.prefetch_correlated_clusters(&nearest_clusters).await;
        }

        // Step 4: Search posting lists (may trigger tier promotion)
        let mut candidates = Vec::new();

        for (cluster_id, _centroid_dist) in nearest_clusters {
            let key = PartitionedKey::new(self.collection_id.clone(), cluster_id);

            // This access may promote the posting list to memory
            if let Some(posting_list) = self.posting_lists.get(&key).await {
                // Search within posting list
                for vector_id in &posting_list.vector_ids {
                    // Get vector from zero-overhead collection
                    if let Some(collection_entry) = self.vectors.get(vector_id) {
                        let collection = collection_entry.read().unwrap();
                        if let Some(view) = collection.get(vector_id) {
                            if let Some(vector_data) = view.as_f32() {
                                let distance = self
                                    .distance_compute
                                    .calculate_distance(
                                        query,
                                        vector_data,
                                        &DistanceMetric::Euclidean,
                                    )
                                    .rank_value;
                                candidates.push((vector_id.clone(), distance));
                            }
                        }
                    }
                }
            }
        }

        // Step 5: Sort and return top-k
        candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        candidates.truncate(k);

        Ok(candidates)
    }

    /// Record access pattern for correlation learning
    async fn record_access_pattern(&self, clusters: &[(usize, f32)]) {
        if clusters.len() < 2 {
            return;
        }

        // Update correlation matrix
        for i in 0..clusters.len() {
            for j in i + 1..clusters.len() {
                let cluster_i = clusters[i].0;
                let cluster_j = clusters[j].0;

                // Update correlation score
                self.access_correlations
                    .entry(cluster_i)
                    .or_insert_with(Vec::new)
                    .push((cluster_j, 0.9)); // Decay over time

                self.access_correlations
                    .entry(cluster_j)
                    .or_insert_with(Vec::new)
                    .push((cluster_i, 0.9));
            }
        }
    }

    /// Prefetch correlated clusters
    async fn prefetch_correlated_clusters(&self, clusters: &[(usize, f32)]) {
        for (cluster_id, _) in clusters {
            if let Some(correlations) = self.access_correlations.get(cluster_id) {
                for (corr_cluster, score) in correlations.value() {
                    if *score > 0.7 {
                        // High correlation threshold
                        let key = PartitionedKey::new(self.collection_id.clone(), *corr_cluster);

                        // Trigger async prefetch
                        let store = self.posting_lists.clone();
                        tokio::spawn(async move {
                            let _ = store.get(&key).await;
                        });
                    }
                }
            }
        }
    }

    /// Get index statistics
    pub fn stats(&self) -> IvfStats {
        IvfStats {
            collection_id: self.collection_id.clone(),
            vector_count: self.vector_count.load(Ordering::Relaxed),
            cluster_count: self.config.n_clusters,
            trained: self.centroids.is_trained(),
            search_count: self.search_count.load(Ordering::Relaxed),
            centroid_memory_bytes: self.centroids.memory_usage_bytes(),
            posting_list_memory_bytes: 0, // Would query AdaptiveStore
            total_memory_bytes: self.centroids.memory_usage_bytes(),
        }
    }

    /// Clear all data for this collection
    pub async fn clear_collection(&self) -> Result<()> {
        // Clear posting lists
        for cluster_id in 0..self.config.n_clusters {
            let key = PartitionedKey::new(self.collection_id.clone(), cluster_id);
            let _ = self.posting_lists.remove(&key).await;
        }

        // Clear vectors
        self.vectors.clear();

        // Reset counters
        self.vector_count.store(0, Ordering::Relaxed);

        info!("Cleared all data for collection '{}'", self.collection_id);
        Ok(())
    }

    /// NEW: Process EventLog event for async index updates
    pub async fn process_event(&self, event: &IndexEvent) -> Result<()> {
        info!("Processing EventLog event {} for IVF index", event.event_id);

        // Process based on extraction mode and data availability
        match self.preferred_extraction_mode {
            ExtractionMode::Fp32Only => {
                // Process FP32 vectors only
                if event.has_fp32 {
                    // TODO: Read FP32 vectors from flushed files in event.file_paths
                    info!("FP32 vectors available in flushed files for processing");
                }
            }
            ExtractionMode::QuantizedOnly => {
                // Process quantized vectors only
                if event.has_quantized {
                    // TODO: Read quantized vectors from flushed files in event.file_paths
                    info!("Quantized vectors available in flushed files for processing");
                }
            }
            ExtractionMode::Both => {
                // Process both representations
                if event.has_fp32 {
                    // TODO: Read FP32 vectors from flushed files in event.file_paths
                    info!("FP32 vectors available in flushed files for processing");
                }
                if event.has_quantized {
                    // TODO: Read quantized vectors from flushed files in event.file_paths
                    info!("Quantized vectors available in flushed files for processing");
                }
            }
            ExtractionMode::Auto => {
                // Auto mode: choose best representation based on what's available
                match (event.has_fp32, event.has_quantized) {
                    (true, true) => {
                        // Both available, prefer FP32 for IVF clustering but keep quantized for fast search
                        info!("Auto mode: processing both FP32 and quantized vectors");
                        // TODO: Process both with preference for FP32 in clustering
                    }
                    (true, false) => {
                        // Only FP32 available
                        info!("Auto mode: processing FP32 vectors only");
                        // TODO: Process FP32 vectors
                    }
                    (false, true) => {
                        // Only quantized available, need to dequantize for IVF
                        info!(
                            "Auto mode: processing quantized vectors (will dequantize for clustering)"
                        );
                        // TODO: Process quantized vectors with dequantization
                    }
                    (false, false) => {
                        // No vectors available
                        info!("Auto mode: no vectors to process");
                    }
                }
            }
        }

        Ok(())
    }

    /// NEW: Process queue payloads for async index updates
    /// TODO: This will be integrated with the EventLog consumer when available
    pub async fn process_queue_updates(&self) -> Result<()> {
        tracing::debug!("IVF queue update processing (placeholder implementation)");
        // TODO: Integrate with EventLog consumer from src/index/axis/eventlog_consumer.rs
        // For now, this is a placeholder that doesn't fail compilation
        Ok(())
    }

    /// NEW: Process a single IndexEvent based on representation type
    async fn process_index_payload(&self, payload: IndexEvent) -> Result<()> {
        // Handle based on what type of vectors are available
        match (payload.has_fp32, payload.has_quantized) {
            (true, false) => {
                // Process FP32 vectors only
                tracing::info!(
                    "Processing FP32-only event for collection {}, {} vectors from {} files",
                    payload.collection_id,
                    payload.vector_count,
                    payload.file_paths.len()
                );
                self.process_fp32_vectors(&payload.file_paths).await?;
            }

            (false, true) => {
                // Process quantized vectors only
                tracing::info!(
                    "Processing quantized-only event for collection {}, {} vectors from {} files",
                    payload.collection_id,
                    payload.vector_count,
                    payload.file_paths.len()
                );
                self.process_quantized_vectors(&payload.file_paths).await?;
            }

            (true, true) => {
                // Process both FP32 and quantized vectors
                tracing::info!(
                    "Processing mixed FP32+quantized event for collection {}, {} vectors from {} files",
                    payload.collection_id,
                    payload.vector_count,
                    payload.file_paths.len()
                );
                self.process_mixed_vectors(&payload.file_paths).await?;
            }

            (false, false) => {
                // Nothing to process
                tracing::debug!(
                    "Empty event with no vectors for collection {}",
                    payload.collection_id
                );
            }
        }

        Ok(())
    }

    /// Process FP32 vectors from file paths
    async fn process_fp32_vectors(&self, file_paths: &[String]) -> Result<()> {
        for file_path in file_paths {
            // TODO: Load vectors from file and add to IVF index
            tracing::debug!("Processing FP32 vectors from {}", file_path);
            // Placeholder implementation
        }
        Ok(())
    }

    /// Process quantized vectors from file paths
    async fn process_quantized_vectors(&self, file_paths: &[String]) -> Result<()> {
        for file_path in file_paths {
            // TODO: Load quantized vectors, dequantize, and add to IVF index
            tracing::debug!("Processing quantized vectors from {}", file_path);
            // Placeholder implementation
        }
        Ok(())
    }

    /// Process mixed FP32 and quantized vectors from file paths
    async fn process_mixed_vectors(&self, file_paths: &[String]) -> Result<()> {
        for file_path in file_paths {
            // TODO: Load both FP32 and quantized vectors
            tracing::debug!("Processing mixed vectors from {}", file_path);
            // Placeholder implementation
        }
        Ok(())
    }

    /// NEW: Dequantize vector for IVF clustering
    /// TODO: Integrate with actual quantization module from storage engines
    fn dequantize_vector(
        &self,
        _quantized: &[u8],
        _method: &str,
        dimension: usize,
    ) -> Result<Vec<f32>> {
        // PLACEHOLDER: In production, this would use the actual quantization module
        // from src/storage/quantization/ to properly dequantize vectors
        tracing::warn!(
            "Using placeholder dequantization - integrate with storage quantization module"
        );

        // Create a placeholder FP32 vector
        Ok(vec![0.0; dimension])
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
        n_probe: Option<usize>,
    ) -> Result<Vec<(String, f32)>> {
        if !self.has_quantized_storage() || self.product_quantizer.is_none() {
            // No quantized vectors or PQ available, use standard search
            return self.search(query, k, n_probe).await;
        }

        // TODO: Implement two-stage search with quantized filtering
        // Stage 1: Fast filtering using quantized vectors with asymmetric distance
        // Stage 2: FP32 reranking of top candidates
        tracing::warn!("Quantized acceleration not yet implemented - using standard search");

        self.search(query, k, n_probe).await
    }

    /// NEW: Train Product Quantizer for quantized search acceleration
    pub async fn train_product_quantizer(&mut self, training_vectors: &[Vec<f32>]) -> Result<()> {
        if !self.config.use_pq {
            return Ok(());
        }

        let mut pq = ProductQuantizer::new(self.config.dimension, self.config.pq_subspaces);
        pq.train(training_vectors)?;

        self.product_quantizer = Some(Arc::new(pq));
        info!(
            "Trained Product Quantizer for collection '{}'",
            self.collection_id
        );

        Ok(())
    }
}

// Implementation of AxisVectorIndex trait for UnifiedIvfIndex
#[async_trait::async_trait]
impl crate::index::axis::index_factory::AxisVectorIndex for UnifiedIvfIndex {
    async fn add(&self, id: String, vector_data: Vec<f32>) -> Result<()> {
        // Direct vector addition - no VectorRecord overhead
        // Clean API: just ID and vector data
        self.add_vector(id, vector_data, None).await
    }

    async fn search(
        &self,
        query: &[f32],
        k: usize,
        _filter: Option<&HashMap<String, String>>, // Metadata filter at storage layer
    ) -> Result<Vec<(String, f32)>> {
        // Call the existing search method with default parameters
        let results = self.search(query, k, None).await?;
        Ok(results)
    }

    async fn remove(&self, id: &str) -> Result<()> {
        // Remove vector from vectors map
        let key = PartitionedKey::new(self.collection_id.clone(), id.to_string());
        self.vectors.remove(&key.to_string());

        // Note: We don't remove from posting lists here as that would require
        // scanning all clusters. This will be handled during compaction.

        self.vector_count.fetch_sub(1, Ordering::Relaxed);
        Ok(())
    }

    fn algorithm(&self) -> &IndexAlgorithm {
        &self.algorithm
    }

    fn stats(&self) -> crate::index::axis::index_factory::IndexStats {
        let ivf_stats = self.stats();
        crate::index::axis::index_factory::IndexStats {
            vector_count: ivf_stats.vector_count,
            memory_usage_bytes: ivf_stats.total_memory_bytes,
            index_type: "IVF".to_string(),
        }
    }
}

/// Factory function to create IVF index instances
pub fn create_ivf_index(
    collection_id: String,
    config: UnifiedIvfConfig,
) -> Result<Box<dyn crate::index::axis::index_factory::AxisVectorIndex>> {
    Ok(Box::new(UnifiedIvfIndex::new(collection_id, config)?))
}

/// Factory function to create IVF index instances with vector representation preference
pub fn create_ivf_index_with_representation(
    collection_id: String,
    config: UnifiedIvfConfig,
    preferred_extraction_mode: ExtractionMode,
) -> Result<Box<dyn crate::index::axis::index_factory::AxisVectorIndex>> {
    Ok(Box::new(UnifiedIvfIndex::new_with_extraction_mode(
        collection_id,
        config,
        preferred_extraction_mode,
    )?))
}

#[derive(Debug, Clone)]
pub struct IvfStats {
    pub collection_id: String,
    pub vector_count: usize,
    pub cluster_count: usize,
    pub trained: bool,
    pub search_count: u64,
    pub centroid_memory_bytes: usize,
    pub posting_list_memory_bytes: usize,
    pub total_memory_bytes: usize,
}

#[cfg(test)]
mod tests {
    use super::{IvfClusteringMethod, PartitionedKey};
    use crate::compute::distance_computation::DistanceMetric;
    use crate::index::axis::*;

    #[tokio::test]
    async fn test_unified_ivf_basic() {
        // Initialize hardware capabilities for testing
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

        let config = UnifiedIvfConfig {
            n_clusters: 2, // Reduce clusters to match small dataset
            n_probe: 2,    // Search all clusters
            dimension: 4,
            distance_metric: DistanceMetric::Euclidean,
            quantization_bits: 0,
            use_pq: false,
            pq_subspaces: 0,
            clustering_method: IvfClusteringMethod::KMeans,
            train_on_insert: false,
            min_train_size: 100,
            max_iterations: 20,
            tolerance: 0.01,
            n_init: 1,
            centroid_config: CentroidConfig::default(),
            posting_list_config: PostingListConfig::default(),
        };

        let mut index = UnifiedIvfIndex::new("test_collection".to_string(), config).unwrap();

        // Train with sample vectors
        let training_vectors = vec![
            vec![1.0, 0.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0, 0.0],
            vec![0.0, 0.0, 1.0, 0.0],
            vec![0.0, 0.0, 0.0, 1.0],
        ];

        index.train(training_vectors).await.unwrap();

        // Add vectors
        index
            .add_vector("vec1".to_string(), vec![1.0, 0.0, 0.0, 0.0], None)
            .await
            .unwrap();
        index
            .add_vector("vec2".to_string(), vec![0.0, 1.0, 0.0, 0.0], None)
            .await
            .unwrap();

        // Search
        let results = index.search(&[1.0, 0.0, 0.0, 0.0], 2, None).await.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "vec1");
    }

    #[test]
    fn test_partitioned_key() {
        let key1 = PartitionedKey::new("collection1".to_string(), 42);
        let key2 = PartitionedKey::new("collection2".to_string(), 42);

        assert_ne!(key1, key2); // Different collections

        let key3 = PartitionedKey::new("collection1".to_string(), 43);
        assert_ne!(key1, key3); // Different keys

        let key4 = PartitionedKey::new("collection1".to_string(), 42);
        assert_eq!(key1, key4); // Same collection and key
    }
}
