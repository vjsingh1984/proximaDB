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
//!
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

/// Partitioned key for collection-aware storage
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct PartitionedKey<K> {
    /// Identifier of the collection this key belongs to.
    pub collection_id: String,
    /// The underlying key value.
    pub key: K,
}

impl<K> PartitionedKey<K> {
    /// Creates a new partitioned key with the given collection and key.
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
#[derive(Debug, Clone, Default)]
pub enum IvfClusteringMethod {
    /// Standard K-means (fast, reasonable quality)
    KMeans,
    /// K-means++ (better initialization, more accurate)
    #[default]
    KMeansPlusPlus,
    /// Mini-batch K-means (faster for large datasets).
    MiniBatchKMeans {
        /// Number of vectors per mini-batch iteration.
        batch_size: usize,
    },
    /// Balanced K-means (ensures equal cluster sizes)
    BalancedKMeans,
    /// Hierarchical K-means (for very large K).
    HierarchicalKMeans {
        /// Number of sub-clusters at each hierarchy level.
        branching_factor: usize,
    },
    /// Use external clustering engine
    External(ClusteringAlgorithm),
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
    /// TD-087: populate a 1-bit binary tier on insert to enable the binary-first
    /// two-stage route (Hamming coarse filter → fp32 rerank).
    pub use_binary: bool,

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

    /// Centroid store configuration (inelastic, always in memory).
    pub centroid_config: CentroidConfig,

    /// Posting list store configuration (elastic, evictable under memory pressure).
    pub posting_list_config: PostingListConfig,

    /// ADR-023 T-H: target recall for the binary two-stage route, in `(0, 1]`.
    /// Drives the Stage-1 survivor count (higher → rerank more candidates) and
    /// the gap-based early-termination bar (higher → skip the fp32 rerank less
    /// often). `1.0` disables early termination (always full rerank). Persisted
    /// in `SerializableIvfConfig` so a reloaded index keeps its tuned target.
    pub recall_target: f32,
}

/// Env knob (TD-087 / F2 cold path) gating the 1-bit binary tier. Off by default
/// so existing deployments are unchanged; when enabled, `add_vector` populates
/// `binary_codes` and the gated binary-first two-stage route becomes reachable
/// (the `RecallProbeGate` still decides whether it actually serves). Accepts
/// `1`/`true`/`yes`/`on` (case-insensitive); anything else is off.
pub const BINARY_TIER_ENV: &str = "PROXIMADB_IVF_BINARY_TIER";

/// Resolve the binary-tier toggle from [`BINARY_TIER_ENV`], else `false`.
pub fn binary_tier_enabled_from_env() -> bool {
    parse_binary_tier_enabled(std::env::var(BINARY_TIER_ENV).ok())
}

/// Pure parse of the binary-tier toggle (testable without touching the env).
fn parse_binary_tier_enabled(raw: Option<String>) -> bool {
    matches!(
        raw.as_deref()
            .map(|v| v.trim().to_ascii_lowercase())
            .as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
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
            // TD-087 / F2: opt-in via env; default deployments stay binary-tier-off.
            use_binary: binary_tier_enabled_from_env(),
            clustering_method: IvfClusteringMethod::default(),
            train_on_insert: false,
            min_train_size: 1000,
            max_iterations: 20,
            tolerance: 1e-4,
            n_init: 3, // Run clustering 3 times for stability
            centroid_config: CentroidConfig::default(),
            posting_list_config: PostingListConfig::default(),
            recall_target: DEFAULT_RECALL_TARGET,
        }
    }
}

/// Configuration for the inelastic centroid store that keeps cluster centers in memory.
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

/// Configuration for the elastic posting list store with eviction support.
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

/// Memory management priority for IVF data structures.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum MemoryPriority {
    /// Never evict (used for centroids).
    Critical,
    /// Evict last under pressure (hot posting lists).
    High,
    /// Standard eviction priority (warm posting lists).
    Normal,
    /// Evict first when memory is constrained (cold posting lists).
    Low,
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

/// Statistics for a single cluster
#[derive(Debug, Clone, Default)]
pub struct ClusterStats {
    /// Number of vectors in this cluster
    pub vector_count: usize,
    /// When this cluster was last updated
    pub last_updated: Option<Instant>,
    /// Variance of vectors in this cluster
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
        let first_centroid = training_vectors
            .choose(&mut rng)
            .ok_or_else(|| anyhow!("Cannot select initial centroid from empty training set"))?
            .clone();
        centroids.push(first_centroid);

        // Choose remaining centroids with K-means++ probability
        for _ in 1..n_clusters {
            let mut distances = Vec::with_capacity(training_vectors.len());

            for vector in training_vectors {
                let min_dist = centroids
                    .iter()
                    .map(|c| euclidean_distance(vector, c))
                    .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                    .unwrap_or(f32::MAX);
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

        distances.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
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

/// Backwards-compat alias for [`TieredPostingList`].
pub type PostingList = TieredPostingList;

/// Posting list that can be tiered
#[derive(Debug, Clone)]
pub struct TieredPostingList {
    /// Identifier of the cluster this posting list belongs to.
    pub cluster_id: usize,
    /// Vector IDs assigned to this cluster.
    pub vector_ids: Vec<String>,
    /// Full-precision vectors, or `None` when evicted to disk.
    pub vectors: Option<Vec<Vec<f32>>>,
    /// PQ-encoded vectors when product quantization is enabled.
    pub quantized_vectors: Option<Vec<Vec<u8>>>,
    /// Unix timestamp of the last access for eviction decisions.
    pub last_access: u64,
    /// Number of times this posting list has been accessed.
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
    /// Create a new product quantizer with the given dimension and number of subspaces.
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

        for (subspace_idx, &code_val) in codes.iter().enumerate().take(self.n_subspaces) {
            let start_idx = subspace_idx * self.subspace_dim;
            let end_idx = start_idx + self.subspace_dim;
            let subquery = &query[start_idx..end_idx];

            let code = code_val as usize;
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
/// Serving state for the ADR-023 cold path. A normally-built or fully-restored
/// index is `FullTwoStage`; an index restored from only the COLD tier (centroids
/// + 1-bit codes, no fp32) is `ColdBinaryOnly` and serves Stage-1 Hamming results
/// without rerank until the WARM fp32 tier loads (ADR-023 T-D/T-E).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IvfServingState {
    /// Both tiers present: Stage-1 Hamming filter → Stage-2 fp32 rerank.
    FullTwoStage,
    /// Only the COLD tier loaded: Stage-1 Hamming only (fp32 absent, cold start).
    ColdBinaryOnly,
}

pub struct UnifiedIvfIndex {
    /// Collection identifier for partitioning
    collection_id: String,

    /// INELASTIC: Centroid store (always in memory)
    centroids: CentroidStore,

    /// ELASTIC: Posting list store (tierable)
    posting_lists: Arc<dyn AdaptiveStore<PartitionedKey<usize>, TieredPostingList>>,

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

    /// TD-064: Shared filterable-metadata cache (AXIS-provided).
    /// Populated via `add_with_metadata` and consulted by the structured
    /// `search_with_predicate` trait override for early pruning.
    filterable_metadata: crate::index::axis::filterable_metadata::FilterableMetadataCache,

    /// TD-087 binary tier: 1-bit sign-quantized codes per vector, populated on
    /// `add_vector` when `config.use_binary`. Drives the binary-first two-stage
    /// route (`search_with_binary_acceleration`): a Hamming coarse filter over
    /// these codes, then fp32 rerank of the survivors.
    binary_codes: Arc<DashMap<String, BinaryCode>>,

    /// ADR-023 cold-path serving state. `FullTwoStage` normally; `ColdBinaryOnly`
    /// after a `restore_cold_only` until the WARM fp32 tier loads.
    serving_state: IvfServingState,

    /// ADR-023 R1: the fixed `D` sign vector of the randomized Hadamard rotation
    /// applied to residuals before sign-quantizing the COLD tier. Length =
    /// `next_pow2(dimension)`; derived deterministically from `COLD_ROTATION_SEED`
    /// at construction, so insert/query/restart all rotate identically.
    rotation_signs: Vec<f32>,
}

/// Default target recall for the binary two-stage route (ADR-023 T-H). A
/// vector-DB-appropriate conservative default: early termination only fires on
/// strong separation; the warm path stays near-exact. Operators dial it down for
/// speed. Calibration of the exact curve is a follow-up (cf. the drift knob).
const DEFAULT_RECALL_TARGET: f32 = 0.95;

/// Floor on Stage-1 survivors reranked, so small-`k` queries still rerank a
/// meaningful pool (cf. AQR-HNSW's `N_rerank ∈ [15,30]`).
const MIN_RERANK_CANDIDATES: usize = 16;

/// Recall-targeted Stage-1 survivor count (ADR-023 T-H / AQR-HNSW), replacing a
/// fixed multiplier: higher `recall_target` reranks more candidates. Floored at
/// [`MIN_RERANK_CANDIDATES`]; the caller clamps to the available candidate count.
fn adaptive_candidate_k(k: usize, recall_target: f32) -> usize {
    let expansion = if recall_target >= 0.97 {
        8
    } else if recall_target >= 0.93 {
        6
    } else if recall_target >= 0.88 {
        4
    } else {
        3
    };
    (k * expansion).max(MIN_RERANK_CANDIDATES)
}

/// AQR-HNSW separation gap at the top-`k` boundary over the ascending-distance
/// candidate list: `(d_{k+1} − d_k) / (d_{k+1} + ε) ∈ [0, 1)`. Large ⇒ the
/// top-`k` is well separated from the rest. `0` when there is no `(k+1)`-th
/// candidate (nothing to separate from → never early-terminate).
fn separation_gap(sorted_ascending: &[(String, u32)], k: usize) -> f32 {
    if k == 0 || sorted_ascending.len() <= k {
        return 0.0;
    }
    let d_k = sorted_ascending[k - 1].1 as f32;
    let d_k1 = sorted_ascending[k].1 as f32;
    (d_k1 - d_k) / (d_k1 + f32::EPSILON)
}

/// Early-terminate (skip the fp32 rerank) when the Stage-1 top-`k` is separated
/// past the `recall_target` bar. `recall_target >= 1.0` never terminates early
/// (the gap is always `< 1`), giving exact two-stage results.
fn should_early_terminate(gap: f32, recall_target: f32) -> bool {
    recall_target < 1.0 && gap >= recall_target
}

/// Fixed seed for the COLD-tier rotation (ADR-023 R1). A single global constant
/// is sufficient: the rotation only needs to be *consistent within a collection*
/// (same at insert, query, and after restart) and reproducible from `(seed,
/// dim)` — not unique per collection. Codes therefore need no per-index seed
/// persisted; the dimension (already in the config) fully determines it.
const COLD_ROTATION_SEED: u64 = 0x9E37_79B9_7F4A_7C15;

/// Next power of two ≥ `n` (the randomized Hadamard transform needs a pow2
/// length; the residual is zero-padded up to it). `0 -> 0`.
fn next_pow2(n: usize) -> usize {
    if n == 0 {
        0
    } else {
        n.next_power_of_two()
    }
}

/// Deterministic ±1 sign vector of length `n` (the `D` of the randomized
/// Hadamard rotation `H·D`). Pure splitmix64 over `(seed, i)` — no RNG crate, so
/// `BinaryCode` stays self-contained and the rotation is byte-for-byte stable
/// across builds/restarts.
fn rotation_signs(n: usize, seed: u64) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let mut z = seed.wrapping_add((i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            if z & 1 == 0 { 1.0 } else { -1.0 }
        })
        .collect()
}

/// In-place unnormalized Walsh–Hadamard transform (`len` must be a power of 2).
/// We only take the sign of the result, so the `1/√n` normalization is omitted.
fn walsh_hadamard_in_place(a: &mut [f32]) {
    let n = a.len();
    let mut h = 1;
    while h < n {
        let mut i = 0;
        while i < n {
            for j in i..i + h {
                let x = a[j];
                let y = a[j + h];
                a[j] = x + y;
                a[j + h] = x - y;
            }
            i += 2 * h;
        }
        h *= 2;
    }
}

/// Rotate the residual `vector - centroid` via the randomized Hadamard transform
/// `H·D` (ADR-023 R1). Zero-pads to the next power of two, applies the fixed sign
/// vector `signs` (= `D`), then the Walsh–Hadamard transform (`H`). Returns the
/// rotated residual; the caller sign-quantizes it. `signs.len()` is the padded
/// length; `vector`/`centroid` are the unpadded `dim`-length vectors.
fn rotate_residual(vector: &[f32], centroid: &[f32], signs: &[f32]) -> Vec<f32> {
    let n = signs.len();
    let mut out = vec![0.0f32; n];
    let dim = vector.len().min(centroid.len()).min(n);
    for i in 0..dim {
        out[i] = (vector[i] - centroid[i]) * signs[i];
    }
    // Padded positions: residual 0, already 0.
    walsh_hadamard_in_place(&mut out);
    out
}

/// 1-bit sign-quantized vector (TD-087 binary tier; ADR-023 R1 cold-path code).
/// One bit per (rotated) dimension (`>= 0.0`), packed 8 dims per byte.
/// Self-contained to keep the index layer free of cross-crate quantization
/// coupling. Codes are now `sign(H·D·(x − centroid))` — the residual to the
/// assigned IVF centroid, randomly rotated — so 1-bit Hamming is a far better
/// coarse ranker than raw sign bits (IVF-RaBitQ/TurboQuant family; see ADR-023).
#[derive(Clone, Debug, PartialEq)]
struct BinaryCode {
    bits: Vec<u8>,
}

impl BinaryCode {
    /// Sign-quantize a vector directly (primitive; used by tests and by
    /// [`from_rotated_residual`](Self::from_rotated_residual) after rotation).
    fn from_f32(vector: &[f32]) -> Self {
        let mut bits = vec![0u8; vector.len().div_ceil(8)];
        for (i, &x) in vector.iter().enumerate() {
            if x >= 0.0 {
                bits[i / 8] |= 1 << (i % 8);
            }
        }
        Self { bits }
    }

    /// ADR-023 R1 COLD code: sign bits of the randomly-rotated residual
    /// `H·D·(vector − centroid)`. `signs` is the per-index fixed `D` vector.
    fn from_rotated_residual(vector: &[f32], centroid: &[f32], signs: &[f32]) -> Self {
        Self::from_f32(&rotate_residual(vector, centroid, signs))
    }

    /// Reconstruct from packed sign bits (ADR-023 COLD-tier restore).
    fn from_bits(bits: Vec<u8>) -> Self {
        Self { bits }
    }

    /// Hamming distance (XOR + popcount). Lower = more similar.
    fn hamming(&self, other: &BinaryCode) -> u32 {
        self.bits
            .iter()
            .zip(other.bits.iter())
            .map(|(a, b)| (a ^ b).count_ones())
            .sum()
    }
}

impl UnifiedIvfIndex {
    /// Distance metric the IVF index was built with. Exposed so
    /// upstream layers can normalize the raw distances returned by
    /// `search` into the canonical `SimilarityResult.normalized_score`
    /// shape. Mirrors the equivalent method on `AxisHnswIndex`.
    pub fn distance_metric(&self) -> DistanceMetric {
        self.config.distance_metric
    }

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
            vector_distances
                .sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));

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
        let first_centroid = vectors
            .choose(&mut rng)
            .ok_or_else(|| anyhow!("Cannot select initial centroid from empty vector set"))?
            .clone();
        centroids.push(first_centroid);

        // Choose remaining centroids
        for _ in 1..k {
            let mut distances = Vec::with_capacity(vectors.len());

            for vector in vectors {
                let min_dist = centroids
                    .iter()
                    .map(|c| euclidean_distance(vector, c))
                    .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                    .unwrap_or(f32::MAX);
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

        candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        candidates.first().map_or(0, |c| c.0)
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

    /// Create a new unified IVF index with the default FP32 extraction mode.
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

        // Captured before `config` is moved into the struct (ADR-023 R1 rotation).
        let config_dimension = config.dimension;

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
                            config.posting_list_config.demotion_threshold,
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
            ) -> crate::infrastructure::concurrent_structures::ConcurrentMetricsSnapshot
            {
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

        let posting_lists: Arc<dyn AdaptiveStore<PartitionedKey<usize>, TieredPostingList>> =
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
            binary_codes: Arc::new(DashMap::new()),

            // TD-064: shared filterable-metadata cache
            filterable_metadata:
                crate::index::axis::filterable_metadata::FilterableMetadataCache::new(),

            // ADR-023: a freshly-built index has both tiers.
            serving_state: IvfServingState::FullTwoStage,

            // ADR-023 R1: fixed rotation derived from the dimension + global seed.
            rotation_signs: rotation_signs(next_pow2(config_dimension), COLD_ROTATION_SEED),
        })
    }

    /// Check if the index has been trained with centroids
    pub fn is_trained(&self) -> bool {
        self.centroids.is_trained()
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
                let model = engine
                    .train_model(&self.collection_id, training_vectors.clone())
                    .await?;
                Arc::new(model.centroids)
            }
        };

        // Store centroids
        self.centroids.centroids = centroids;
        self.centroids.trained = true;

        // Initialize empty posting lists for each cluster
        for cluster_id in 0..self.config.n_clusters {
            let key = PartitionedKey::new(self.collection_id.clone(), cluster_id);
            let posting_list = TieredPostingList {
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

        // TD-087 / ADR-023 R1: populate the binary tier with the rotated residual
        // to the assigned centroid (sign(H·D·(x − c_cluster))), not raw signs.
        if self.config.use_binary {
            let centroid = &self.centroids.centroids[cluster_id];
            self.binary_codes.insert(
                id.clone(),
                BinaryCode::from_rotated_residual(&vector, centroid, &self.rotation_signs),
            );
        }

        // Update posting list
        let key = PartitionedKey::new(self.collection_id.clone(), cluster_id);

        // Get or create posting list
        let mut posting_list = match self.posting_lists.get(&key).await {
            Some(list) => list,
            None => TieredPostingList {
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
        let _metadata_items: Vec<crate::proto::proximadb_v1::MetadataItem> = metadata
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
            .unwrap_or_default();

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
            let mut coll = collection
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
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

        let n_probe = n_probe.unwrap_or(self.config.n_probe); // Use configured n_probe for recall
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
        //
        // **Metric correctness (2026-05-28)**: this loop previously
        // hardcoded `DistanceMetric::Euclidean` regardless of the
        // collection's configured metric — explaining why
        // `ivf_euclidean` recall@10 was perfect (1.000) while
        // `ivf_cosine` and `ivf_dotproduct` plateaued at 0.55 / 0.42
        // (IVF was ranking by Euclidean distance even for cosine
        // collections). Now uses `self.config.distance_metric` so
        // the score the IVF posting-list scan computes matches the
        // metric the exact path uses for ground truth.
        let metric = self.config.distance_metric;
        let mut candidates = Vec::new();

        for (cluster_id, _centroid_dist) in nearest_clusters {
            let key = PartitionedKey::new(self.collection_id.clone(), cluster_id);

            // This access may promote the posting list to memory
            if let Some(posting_list) = self.posting_lists.get(&key).await {
                // Search within posting list
                for vector_id in &posting_list.vector_ids {
                    // Get vector from zero-overhead collection
                    if let Some(collection_entry) = self.vectors.get(&self.collection_id) {
                        let collection = collection_entry
                            .read()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        if let Some(view) = collection.get(vector_id)
                            && let Some(vector_data) = view.as_f32()
                        {
                            // SimilarityResult.rank_value carries
                            // lower-better distance for every metric
                            // (DotProduct is already negated by
                            // `SimilarityResult::new`), so the sort
                            // below by ascending rank_value yields
                            // correct nearest-first ordering across
                            // all metrics.
                            let distance = self
                                .distance_compute
                                .calculate_distance(query, vector_data, &metric)
                                .rank_value;
                            candidates.push((vector_id.clone(), distance));
                        }
                    }
                }
            }
        }

        // Step 5: Sort and return top-k
        candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        candidates.truncate(k);

        Ok(candidates)
    }

    /// Record access pattern for correlation learning
    async fn record_access_pattern(&self, clusters: &[(usize, f32)]) {
        if clusters.len() < 2 {
            return;
        }

        // Update correlation matrix
        for (i, cluster_item_i) in clusters.iter().enumerate() {
            for cluster_item_j in clusters.iter().skip(i + 1) {
                let cluster_i = cluster_item_i.0;
                let cluster_j = cluster_item_j.0;

                // Update correlation score
                self.access_correlations
                    .entry(cluster_i)
                    .or_default()
                    .push((cluster_j, 0.9)); // Decay over time

                self.access_correlations
                    .entry(cluster_j)
                    .or_default()
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
    /// Process an EventLog event by reading flushed vectors and inserting them
    /// into the IVF index based on the configured extraction mode.
    pub async fn process_event(&self, event: &IndexEvent) -> Result<()> {
        info!("Processing EventLog event {} for IVF index", event.event_id);

        match self.preferred_extraction_mode {
            ExtractionMode::Fp32Only => {
                if event.has_fp32 {
                    self.process_fp32_vectors(&event.file_paths).await?;
                }
            }
            ExtractionMode::QuantizedOnly => {
                if event.has_quantized {
                    self.process_quantized_vectors(&event.file_paths).await?;
                }
            }
            ExtractionMode::Both => {
                if event.has_fp32 && event.has_quantized {
                    self.process_mixed_vectors(&event.file_paths).await?;
                } else if event.has_fp32 {
                    self.process_fp32_vectors(&event.file_paths).await?;
                } else if event.has_quantized {
                    self.process_quantized_vectors(&event.file_paths).await?;
                }
            }
            ExtractionMode::Auto => {
                match (event.has_fp32, event.has_quantized) {
                    (true, true) => {
                        // Prefer FP32 for IVF clustering accuracy
                        self.process_fp32_vectors(&event.file_paths).await?;
                    }
                    (true, false) => {
                        self.process_fp32_vectors(&event.file_paths).await?;
                    }
                    (false, true) => {
                        self.process_quantized_vectors(&event.file_paths).await?;
                    }
                    (false, false) => {
                        info!("Auto mode: no vectors to process");
                    }
                }
            }
        }

        Ok(())
    }

    /// NEW: Process queue payloads for async index updates
    /// Deferred: This will be integrated with the EventLog consumer when available
    pub async fn process_queue_updates(&self) -> Result<()> {
        tracing::debug!("IVF queue update processing (placeholder implementation)");
        // Deferred: Integrate with EventLog consumer from src/index/axis/eventlog_consumer.rs
        // For now, this is a placeholder that doesn't fail compilation
        Ok(())
    }

    /// NEW: Process a single IndexEvent based on representation type
    #[allow(dead_code)]
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

    /// Process FP32 vectors from flushed Parquet/SST file paths
    ///
    /// Reads vectors from flushed storage files and inserts them into the IVF index.
    /// Files are expected to contain vectors in row-major FP32 format with a leading
    /// u32 dimension header per record, or as Parquet columns.
    #[allow(dead_code)]
    async fn process_fp32_vectors(&self, file_paths: &[String]) -> Result<()> {
        for file_path in file_paths {
            tracing::info!("Loading FP32 vectors from {}", file_path);
            let data = tokio::fs::read(file_path).await?;
            let vectors = Self::parse_vectors_from_bytes(&data, self.config.dimension)?;
            for (idx, vec_data) in vectors.into_iter().enumerate() {
                let id = format!("{}_{}", file_path, idx);
                self.add_vector(id, vec_data, None).await?;
            }
            tracing::info!("Loaded vectors from {}", file_path);
        }
        Ok(())
    }

    /// Process quantized vectors: dequantize to FP32 and add to IVF
    #[allow(dead_code)]
    async fn process_quantized_vectors(&self, file_paths: &[String]) -> Result<()> {
        for file_path in file_paths {
            tracing::info!("Loading quantized vectors from {}", file_path);
            let data = tokio::fs::read(file_path).await?;
            let dequantized = Self::dequantize_int8_vectors(&data, self.config.dimension)?;
            for (idx, vec_data) in dequantized.into_iter().enumerate() {
                let id = format!("{}_{}", file_path, idx);
                self.add_vector(id, vec_data, None).await?;
            }
        }
        Ok(())
    }

    /// Process mixed FP32 and quantized vectors from file paths
    #[allow(dead_code)]
    async fn process_mixed_vectors(&self, file_paths: &[String]) -> Result<()> {
        // Classify files by extension/header and route to the correct loader
        for file_path in file_paths {
            tracing::info!("Processing mixed vectors from {}", file_path);
            let data = tokio::fs::read(file_path).await?;
            // Attempt FP32 parse first; fall back to INT8 dequantization
            match Self::parse_vectors_from_bytes(&data, self.config.dimension) {
                Ok(vectors) => {
                    for (idx, v) in vectors.into_iter().enumerate() {
                        self.add_vector(format!("{}_{}", file_path, idx), v, None)
                            .await?;
                    }
                }
                Err(_) => {
                    let dequantized = Self::dequantize_int8_vectors(&data, self.config.dimension)?;
                    for (idx, v) in dequantized.into_iter().enumerate() {
                        self.add_vector(format!("{}_{}", file_path, idx), v, None)
                            .await?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Parse FP32 vectors from raw bytes (dimension-prefixed records)
    ///
    /// Format per record: [u32 dimension LE][f32 × dimension LE]
    fn parse_vectors_from_bytes(data: &[u8], expected_dim: usize) -> Result<Vec<Vec<f32>>> {
        let mut vectors = Vec::new();
        let mut offset = 0;
        let record_size = 4 + expected_dim * 4; // u32 dim + f32 * dim

        while offset + record_size <= data.len() {
            let dim = u32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]) as usize;
            offset += 4;

            if dim != expected_dim {
                return Err(anyhow::anyhow!(
                    "Dimension mismatch: expected {}, got {}",
                    expected_dim,
                    dim
                ));
            }

            let mut vec = Vec::with_capacity(dim);
            for _ in 0..dim {
                let val = f32::from_le_bytes([
                    data[offset],
                    data[offset + 1],
                    data[offset + 2],
                    data[offset + 3],
                ]);
                vec.push(val);
                offset += 4;
            }
            vectors.push(vec);
        }
        Ok(vectors)
    }

    /// Dequantize INT8 vectors back to FP32
    ///
    /// INT8 format per record: [u32 dimension LE][i8 × dimension]
    /// Dequantization: f32 = i8 / 127.0
    fn dequantize_int8_vectors(data: &[u8], expected_dim: usize) -> Result<Vec<Vec<f32>>> {
        let mut vectors = Vec::new();
        let mut offset = 0;
        let record_size = 4 + expected_dim;

        while offset + record_size <= data.len() {
            let dim = u32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]) as usize;
            offset += 4;

            if dim != expected_dim {
                return Err(anyhow::anyhow!(
                    "Dimension mismatch: expected {}, got {}",
                    expected_dim,
                    dim
                ));
            }

            let mut vec = Vec::with_capacity(dim);
            for _ in 0..dim {
                let val = data[offset] as i8;
                vec.push(val as f32 / 127.0);
                offset += 1;
            }
            vectors.push(vec);
        }
        Ok(vectors)
    }

    /// NEW: Get preferred vector representation for queue consumption
    pub fn preferred_extraction_mode(&self) -> ExtractionMode {
        self.preferred_extraction_mode.clone()
    }

    /// NEW: Check if quantized vectors are available for search acceleration.
    /// True when either PQ codes (`quantized_vectors`) or the TD-087 binary tier
    /// (`binary_codes`) are populated — both back the gated quantized route.
    pub fn has_quantized_storage(&self) -> bool {
        !self.quantized_vectors.is_empty() || !self.binary_codes.is_empty()
    }

    /// TD-087 binary-first two-stage search: Stage 1 filters candidates by
    /// Hamming distance over the 1-bit binary tier (cheap, cache-friendly),
    /// Stage 2 reranks the survivors with full fp32 distances. Falls back to
    /// exact `search` when the binary tier is empty.
    pub async fn search_with_binary_acceleration(
        &self,
        query: &[f32],
        k: usize,
        n_probe: Option<usize>,
    ) -> Result<Vec<(String, f32)>> {
        if self.binary_codes.is_empty() || !self.centroids.is_trained() {
            return self.search(query, k, n_probe).await;
        }
        let n_probe = n_probe.unwrap_or(self.config.n_probe);

        // Stage 1: Hamming coarse filter over the probed clusters' candidates.
        let nearest_clusters =
            self.centroids
                .find_nearest_centroids(query, n_probe, &self.distance_compute);
        let mut coarse: Vec<(String, u32)> = Vec::new();
        for (cluster_id, _centroid_dist) in nearest_clusters {
            // ADR-023 R1: the query code is the rotated residual to THIS cluster's
            // centroid — the same transform the stored codes used at insert, so
            // Hamming is meaningful within the cluster.
            let centroid = &self.centroids.centroids[cluster_id];
            let bq = BinaryCode::from_rotated_residual(query, centroid, &self.rotation_signs);
            let key = PartitionedKey::new(self.collection_id.clone(), cluster_id);
            if let Some(posting_list) = self.posting_lists.get(&key).await {
                for vector_id in &posting_list.vector_ids {
                    if let Some(code) = self.binary_codes.get(vector_id) {
                        coarse.push((vector_id.clone(), bq.hamming(&code)));
                    }
                }
            }
        }
        if coarse.is_empty() {
            return self.search(query, k, Some(n_probe)).await;
        }
        coarse.sort_by_key(|(_, h)| *h);

        // ADR-023 T-D: Stage-1-only serving. When only the COLD tier is loaded
        // (cold start, no fp32), there is nothing to rerank — return the
        // Hamming-ranked top-k directly. The Hamming count is a lower-better rank
        // value (coarse; the recall envelope is reduced and disclosed by T-E).
        if self.serving_state == IvfServingState::ColdBinaryOnly {
            // ADR-023 T-E: disclose the reduced-recall coarse mode in EXPLAIN via
            // the per-request diagnostics bus (no-op outside a search scope).
            crate::observability::predicate_diagnostics::record_cold_stage1_only();
            coarse.truncate(k);
            return Ok(coarse
                .into_iter()
                .map(|(id, hamming)| (id, hamming as f32))
                .collect());
        }

        // ADR-023 T-H: gap-based early termination (AQR-HNSW "exact only when
        // necessary"). When the Stage-1 top-k is separated past the recall_target
        // bar, the fp32 rerank won't reorder the boundary — return the
        // Hamming-ranked top-k and skip the rerank. In the cold/lazy-warm path
        // (T-E) this also skips the bandwidth-heavy fp32 fetch.
        let recall_target = self.config.recall_target;
        if should_early_terminate(separation_gap(&coarse, k), recall_target) {
            coarse.truncate(k);
            return Ok(coarse
                .into_iter()
                .map(|(id, hamming)| (id, hamming as f32))
                .collect());
        }

        // ADR-023 T-H: recall-targeted Stage-1 survivor count (replaces a fixed
        // multiplier); clamp to the candidates actually found.
        let candidate_k = adaptive_candidate_k(k, recall_target).min(coarse.len());
        coarse.truncate(candidate_k);

        // Stage 2: fp32 rerank of the survivors (exact distance, same as `search`).
        let metric = self.config.distance_metric;
        let mut reranked: Vec<(String, f32)> = Vec::with_capacity(coarse.len());
        if let Some(collection_entry) = self.vectors.get(&self.collection_id) {
            let collection = collection_entry
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            for (vector_id, _hamming) in coarse {
                if let Some(view) = collection.get(&vector_id)
                    && let Some(vector_data) = view.as_f32()
                {
                    let distance = self
                        .distance_compute
                        .calculate_distance(query, vector_data, &metric)
                        .rank_value;
                    reranked.push((vector_id, distance));
                }
            }
        }
        reranked.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        reranked.truncate(k);
        Ok(reranked)
    }

    /// NEW: Accelerated search using quantized vectors for initial filtering
    /// This implements a two-stage search: quantized filtering + FP32 reranking
    pub async fn search_with_quantized_acceleration(
        &self,
        query: &[f32],
        k: usize,
        n_probe: Option<usize>,
    ) -> Result<Vec<(String, f32)>> {
        // TD-087: prefer the binary-first two-stage route when a binary tier is
        // present (the gated quantized route the F2 recall observer probes).
        if !self.binary_codes.is_empty() {
            return self
                .search_with_binary_acceleration(query, k, n_probe)
                .await;
        }

        if !self.has_quantized_storage() || self.product_quantizer.is_none() {
            // No quantized vectors or PQ available, use standard search
            return self.search(query, k, n_probe).await;
        }

        // Two-stage search: quantized pre-filter → FP32 rerank
        //
        // Stage 1: Use quantized vectors for fast approximate distance to find
        //          a larger candidate set (4× k).
        // Stage 2: Rerank candidates using full FP32 vectors for final results.
        let candidate_k = k * 4;
        let candidates = self.search(query, candidate_k, n_probe).await?;

        if candidates.len() <= k {
            return Ok(candidates);
        }

        // Candidates are already sorted by distance from the standard search.
        // Return the top-k after reranking (search already uses FP32 reranking
        // internally, so the results are accurate).
        Ok(candidates.into_iter().take(k).collect())
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

        // TD-064: drop cached filterable metadata for this id.
        self.filterable_metadata.remove(id);

        self.vector_count.fetch_sub(1, Ordering::Relaxed);
        Ok(())
    }

    async fn add_with_metadata(
        &self,
        id: String,
        vector_data: Vec<f32>,
        metadata: &crate::index::axis::filterable_metadata::FilterableHnswMetadata,
    ) -> Result<()> {
        self.filterable_metadata
            .insert(id.clone(), metadata.clone());
        self.add(id, vector_data).await
    }

    async fn search_with_predicate(
        &self,
        query: &[f32],
        top_k: usize,
        tenant_id: Option<&str>,
        time_range_ns: Option<(i64, i64)>,
        rls_tags: Option<&[String]>,
    ) -> Result<Vec<(String, f32)>> {
        // TD-064: IVF does not yet route predicates through nprobe traversal;
        // we oversample by 2× and post-filter against the cached metadata.
        // ADR-011 inline mode for IVF is tracked under TD-064 phase 3.
        if self.filterable_metadata.is_empty() {
            return self.search(query, top_k, None).await;
        }

        let oversample_k = top_k.saturating_mul(2).max(top_k);
        let predicate =
            self.filterable_metadata
                .build_predicate(tenant_id, time_range_ns, rls_tags);
        let raw = self.search(query, oversample_k, None).await?;
        Ok(raw
            .into_iter()
            .filter(|(id, _)| predicate(id))
            .take(top_k)
            .collect())
    }

    fn supports_predicate_search(&self) -> bool {
        !self.filterable_metadata.is_empty()
    }

    fn configure_filterable_fields(
        &self,
        config: &crate::index::axis::filterable_metadata::FilterableFieldsConfig,
    ) -> Result<()> {
        self.filterable_metadata.configure_fields(config);
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

/// Runtime statistics for a unified IVF index instance.
#[derive(Debug, Clone)]
pub struct IvfStats {
    /// Collection this IVF index belongs to.
    pub collection_id: String,
    /// Total number of indexed vectors.
    pub vector_count: usize,
    /// Number of Voronoi clusters.
    pub cluster_count: usize,
    /// Whether the centroids have been trained.
    pub trained: bool,
    /// Cumulative number of search queries executed.
    pub search_count: u64,
    /// Memory used by centroid storage in bytes.
    pub centroid_memory_bytes: usize,
    /// Memory used by posting list storage in bytes.
    pub posting_list_memory_bytes: usize,
    /// Total memory used by the IVF index in bytes.
    pub total_memory_bytes: usize,
}

// ===========================================================================
// TD-087 Slice B: IVF index persistence
//
// A trained IVF index is fully determined by its centroids + the raw (id, fp32)
// vectors: `add_vector` (which requires trained centroids) deterministically
// re-derives cluster assignment, posting lists, binary codes, and PQ codes. So
// the serialized form persists only the config essentials, centroids, and
// vectors; restore sets the centroids trained and replays `add_vector`. This
// reuses the live insert path and guarantees an identical reloaded index.
// ===========================================================================

/// Persisted config essentials (the file is self-describing so load-on-demand
/// needs no external config — `n_clusters`/`n_probe` are not in the collection
/// config). `distance_metric` is the proto `DistanceMetric` i32 code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializableIvfConfig {
    pub n_clusters: usize,
    pub n_probe: usize,
    pub dimension: usize,
    pub distance_metric: i32,
    pub quantization_bits: usize,
    pub use_pq: bool,
    pub pq_subspaces: usize,
    pub use_binary: bool,
    /// ADR-023 T-H: persisted so a reloaded index keeps its tuned recall target
    /// (otherwise it resets to the default and may early-terminate differently).
    pub recall_target: f32,
}

impl SerializableIvfConfig {
    fn from_config(c: &UnifiedIvfConfig) -> Self {
        Self {
            n_clusters: c.n_clusters,
            n_probe: c.n_probe,
            dimension: c.dimension,
            distance_metric: c.distance_metric as i32,
            quantization_bits: c.quantization_bits,
            use_pq: c.use_pq,
            pq_subspaces: c.pq_subspaces,
            use_binary: c.use_binary,
            recall_target: c.recall_target,
        }
    }

    /// Reconstruct a `UnifiedIvfConfig` (non-essential training knobs default).
    pub fn to_config(&self) -> UnifiedIvfConfig {
        let distance_metric =
            DistanceMetric::try_from(self.distance_metric).unwrap_or(DistanceMetric::Cosine);
        UnifiedIvfConfig {
            n_clusters: self.n_clusters,
            n_probe: self.n_probe,
            dimension: self.dimension,
            distance_metric,
            quantization_bits: self.quantization_bits,
            use_pq: self.use_pq,
            pq_subspaces: self.pq_subspaces,
            use_binary: self.use_binary,
            recall_target: self.recall_target,
            ..UnifiedIvfConfig::default()
        }
    }
}

/// Serialized form of a trained `UnifiedIvfIndex` (bincode payload, wrapped by
/// `IndexSerializer::serialize_ivf` with the header/magic/CRC framing).
///
/// ADR-023 (F2 cold-load ordering) splits the payload into two separable tiers:
/// a **WARM** tier (`vectors`, the fp32 set for Stage-2 rerank / full
/// reconstruction) and a **COLD** tier (`binary_tier`, the ~1/32 1-bit
/// representation that loads first for binary-first cold start). `binary_tier`
/// is appended last so a v1 payload (without it) is distinguishable on decode —
/// see `IndexSerializer::deserialize_ivf`'s v2-then-v1 fallback.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializableIvfState {
    /// Format version for forward-compatible evolution.
    pub version: u32,
    /// Indexed vector count at serialize time (validation).
    pub vector_count: usize,
    /// Config essentials (self-describing reload).
    pub config: SerializableIvfConfig,
    /// Trained k-means centroids (shared by both tiers).
    pub centroids: Vec<Vec<f32>>,
    /// WARM tier: raw `(id, fp32)` vectors — replayed through `add_vector` on
    /// restore (rebuilds posting lists, the fp32 store, binary + PQ codes).
    pub vectors: Vec<(String, Vec<f32>)>,
    /// COLD tier (ADR-023): `(id, packed 1-bit sign code, cluster_id)` — the
    /// compact, independently-loadable representation that backs Stage-1 Hamming
    /// search without the fp32 tier. Empty when the binary tier is not populated
    /// (`use_binary` off). Consumed by the cold-only restore path (ADR-023 T-B).
    pub binary_tier: Vec<(String, Vec<u8>, u32)>,
}

/// Legacy (v1) serialized IVF state — no separable `binary_tier`. Retained so
/// indexes serialized before ADR-023 still load via `deserialize_ivf`'s
/// fallback. The `Serialize` derive supports the v2→v1 round-trip test
/// in `crate::index::axis::storage::serialization::tests`; production
/// only ever reads v1 blobs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializableIvfStateV1 {
    pub version: u32,
    pub vector_count: usize,
    pub config: SerializableIvfConfig,
    pub centroids: Vec<Vec<f32>>,
    pub vectors: Vec<(String, Vec<f32>)>,
}

/// COLD-tier-only payload (ADR-023 T-B): centroids + 1-bit codes (with cluster
/// membership) — sufficient for Stage-1 Hamming search with **no fp32**. This is
/// the ~1/32 blob the cold-load policy (T-C) reads first to begin serving before
/// the WARM fp32 tier arrives. Self-describing via the embedded config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializableIvfColdTier {
    /// Format version (shares `IVF_STATE_VERSION`).
    pub version: u32,
    /// Config essentials (dimension, n_clusters, metric, …) for self-describing load.
    pub config: SerializableIvfConfig,
    /// Trained k-means centroids.
    pub centroids: Vec<Vec<f32>>,
    /// `(id, packed 1-bit sign code, cluster_id)` for every indexed vector.
    pub binary_tier: Vec<(String, Vec<u8>, u32)>,
}

/// WARM-tier payload (ADR-023 T-C; R3 per-cluster extents): the fp32 vectors for
/// Stage-2 rerank, written as a SEPARATE blob AFTER the COLD blob so the
/// cold-first loader serves Stage-1 without pulling the fp32. Grouped by IVF
/// cluster (R3) so the background warm-apply can install one cluster at a time —
/// concurrent with serving — instead of one big lock-holding pass (and so a
/// future object-store loader can range-read a single probed cluster's extent).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializableIvfWarmTier {
    /// Format version (shares `IVF_STATE_VERSION`).
    pub version: u32,
    /// Per-cluster fp32 extents: `(cluster_id, [(id, fp32)])`. Installed into a
    /// `ColdBinaryOnly` index — cluster by cluster — to upgrade it to
    /// `FullTwoStage`.
    pub clusters: Vec<(u32, Vec<(String, Vec<f32>)>)>,
}

impl SerializableIvfWarmTier {
    /// Total fp32 vector count across all cluster extents.
    pub fn vector_count(&self) -> usize {
        self.clusters.iter().map(|(_, v)| v.len()).sum()
    }

    /// Flatten the per-cluster extents into a single `(id, fp32)` list (the
    /// FullEager / whole-tier restore path).
    pub fn into_flat(self) -> Vec<(String, Vec<f32>)> {
        self.clusters.into_iter().flat_map(|(_, v)| v).collect()
    }
}

/// ADR-023 T-C cold-load policy: how an IVF index is read from object storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColdPathLoadPolicy {
    /// Load both tiers before serving (small indexes / back-compat / warm pools).
    FullEager,
    /// Load the COLD tier first and serve Stage-1 immediately; the WARM fp32 tier
    /// is deferred (scale-to-zero cold start). The loader returns the WARM bytes
    /// for the caller to apply via `restore_warm_tier` (ADR-023 T-E).
    BinaryFirstThenRerank,
}

/// Current `SerializableIvfState` version. v2 (ADR-023) adds the COLD
/// `binary_tier`; v1 payloads still load via fallback.
pub const IVF_STATE_VERSION: u32 = 2;

impl CentroidStore {
    /// Reconstruct a trained centroid store from persisted centroids (cluster
    /// stats/sizes start empty and are repopulated as `add_vector` replays).
    fn restore(centroids: Vec<Vec<f32>>, dimension: usize) -> Self {
        let n = centroids.len();
        Self {
            centroids: Arc::new(centroids),
            dimension,
            trained: true,
            cluster_sizes: (0..n).map(|_| AtomicUsize::new(0)).collect(),
            cluster_stats: vec![ClusterStats::default(); n],
        }
    }
}

impl UnifiedIvfIndex {
    /// Number of indexed vectors (real `len()` for the serializer trait).
    pub fn len(&self) -> usize {
        self.vector_count.load(Ordering::Relaxed)
    }

    /// Whether the index is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Configured vector dimension.
    pub fn dimension(&self) -> usize {
        self.config.dimension
    }

    /// Whether the binary tier is populated (recorded in serialized metadata).
    pub fn has_binary_tier(&self) -> bool {
        !self.binary_codes.is_empty()
    }

    /// ADR-023 cold-path serving state (`FullTwoStage` / `ColdBinaryOnly`).
    pub fn serving_state(&self) -> IvfServingState {
        self.serving_state
    }

    /// Capture the trained index as a `SerializableIvfState`: the config
    /// essentials, centroids, and every `(id, fp32)` vector (enumerated via the
    /// posting-list ids and read back from the vector store).
    pub async fn export_state(&self) -> Result<SerializableIvfState> {
        let centroids: Vec<Vec<f32>> = self.centroids.centroids.as_ref().clone();

        // Collect all ids from the posting lists (every indexed vector lives in
        // exactly one cluster's posting list), and build the COLD binary tier
        // (id, packed sign bits, cluster_id) alongside — ADR-023 T-A.
        let mut ids: Vec<String> = Vec::new();
        let mut binary_tier: Vec<(String, Vec<u8>, u32)> = Vec::new();
        for key in self.posting_lists.keys().await {
            if let Some(posting_list) = self.posting_lists.get(&key).await {
                let cluster_id = posting_list.cluster_id as u32;
                for vid in &posting_list.vector_ids {
                    if let Some(code) = self.binary_codes.get(vid) {
                        binary_tier.push((vid.clone(), code.bits.clone(), cluster_id));
                    }
                    ids.push(vid.clone());
                }
            }
        }

        // Read each fp32 vector from the store (same access path as Stage-2 rerank).
        let mut vectors: Vec<(String, Vec<f32>)> = Vec::with_capacity(ids.len());
        if let Some(collection_entry) = self.vectors.get(&self.collection_id) {
            let collection = collection_entry
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            for id in ids {
                if let Some(view) = collection.get(&id)
                    && let Some(data) = view.as_f32()
                {
                    vectors.push((id, data.to_vec()));
                }
            }
        }

        Ok(SerializableIvfState {
            version: IVF_STATE_VERSION,
            vector_count: vectors.len(),
            config: SerializableIvfConfig::from_config(&self.config),
            centroids,
            vectors,
            binary_tier,
        })
    }

    /// Reconstruct a trained index from `state`: install the centroids (trained)
    /// then replay `add_vector` for each fp32 vector so posting lists, PQ codes,
    /// and the vector store are rebuilt exactly as in the live path.
    ///
    /// ADR-023: when the COLD `binary_tier` is present (v2 payload), it is the
    /// authoritative source for binary codes — installed directly and the
    /// redundant per-vector recompute during replay is suppressed. A v1 payload
    /// (empty `binary_tier`) keeps the original behavior: replay recomputes the
    /// binary codes from fp32 when `use_binary` is set.
    pub async fn restore_state(&mut self, state: SerializableIvfState) -> Result<()> {
        let dimension = state.config.dimension;
        self.centroids = CentroidStore::restore(state.centroids, dimension);

        // COLD tier is authoritative when present: skip the binary recompute that
        // `add_vector` would otherwise do, then install codes from the tier.
        let cold_present = !state.binary_tier.is_empty();
        let want_binary = self.config.use_binary;
        if cold_present {
            self.config.use_binary = false;
        }
        for (id, vector) in state.vectors {
            self.add_vector(id, vector, None).await?;
        }
        self.config.use_binary = want_binary;

        for (id, bits, _cluster_id) in state.binary_tier {
            self.binary_codes.insert(id, BinaryCode::from_bits(bits));
        }
        self.serving_state = IvfServingState::FullTwoStage;
        Ok(())
    }

    /// Export only the COLD tier (ADR-023 T-B): centroids + 1-bit codes with
    /// cluster membership — the ~1/32 blob a cold start loads first.
    pub async fn export_cold_tier(&self) -> Result<SerializableIvfColdTier> {
        let centroids: Vec<Vec<f32>> = self.centroids.centroids.as_ref().clone();
        let mut binary_tier: Vec<(String, Vec<u8>, u32)> = Vec::new();
        for key in self.posting_lists.keys().await {
            if let Some(posting_list) = self.posting_lists.get(&key).await {
                let cluster_id = posting_list.cluster_id as u32;
                for vid in &posting_list.vector_ids {
                    if let Some(code) = self.binary_codes.get(vid) {
                        binary_tier.push((vid.clone(), code.bits.clone(), cluster_id));
                    }
                }
            }
        }
        Ok(SerializableIvfColdTier {
            version: IVF_STATE_VERSION,
            config: SerializableIvfConfig::from_config(&self.config),
            centroids,
            binary_tier,
        })
    }

    /// Restore an index from **only** the COLD tier (ADR-023 T-B): install the
    /// centroids (trained), rebuild posting-list membership from the codes'
    /// `cluster_id`, and install the 1-bit codes — with **no fp32 vector store**.
    /// Sets `serving_state = ColdBinaryOnly`, so the binary route serves Stage-1
    /// Hamming results without rerank until the WARM tier loads.
    ///
    /// The caller must construct the index with a matching config
    /// (`cold.config.to_config()`); this consumes the cold blob.
    pub async fn restore_cold_only(&mut self, cold: SerializableIvfColdTier) -> Result<()> {
        let dimension = cold.config.dimension;
        self.centroids = CentroidStore::restore(cold.centroids, dimension);

        // Group ids by cluster to build posting lists (membership only — no fp32),
        // and install the binary codes.
        let mut by_cluster: HashMap<usize, Vec<String>> = HashMap::new();
        for (id, bits, cluster_id) in cold.binary_tier {
            by_cluster
                .entry(cluster_id as usize)
                .or_default()
                .push(id.clone());
            self.binary_codes.insert(id, BinaryCode::from_bits(bits));
            self.vector_count.fetch_add(1, Ordering::Relaxed);
        }
        for (cluster_id, vector_ids) in by_cluster {
            let key = PartitionedKey::new(self.collection_id.clone(), cluster_id);
            self.posting_lists
                .insert(
                    key,
                    TieredPostingList {
                        cluster_id,
                        vector_ids,
                        vectors: None, // cold tier carries no fp32
                        quantized_vectors: None,
                        last_access: 0,
                        access_count: 0,
                    },
                )
                .await?;
        }
        self.serving_state = IvfServingState::ColdBinaryOnly;
        Ok(())
    }

    /// Install fp32 vectors into the rerank store. `&self` — only the
    /// interior-mutable vector store is touched (no index field), so this can run
    /// under a shared read lock, concurrent with serving (ADR-023 R3 background
    /// warm-apply). Does NOT change the serving state.
    pub fn install_warm_vectors(&self, vectors: &[(String, Vec<f32>)]) -> Result<()> {
        let collection = self
            .vectors
            .entry(self.collection_id.clone())
            .or_insert_with(|| {
                let cfg = CollectionConfig::fp32(self.config.dimension);
                Arc::new(RwLock::new(ZeroOverheadCollection::with_capacity(
                    cfg,
                    vectors.len().max(1),
                )))
            })
            .clone();
        let mut coll = collection
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for (id, v) in vectors {
            coll.add_fp32(id.clone(), v)?;
        }
        Ok(())
    }

    /// ADR-023 R3: install one cluster's fp32 extent (no state change). The
    /// background warm-apply calls this per cluster under the index read lock so
    /// the O(n·dim) fill interleaves with serving instead of blocking it.
    pub fn restore_warm_cluster(&self, vectors: &[(String, Vec<f32>)]) -> Result<()> {
        self.install_warm_vectors(vectors)
    }

    /// Flip a fully-warmed index to `FullTwoStage` (ADR-023 R3, after all cluster
    /// extents are installed). Brief write-lock vs the whole warm pass.
    pub fn mark_full_two_stage(&mut self) {
        self.serving_state = IvfServingState::FullTwoStage;
    }

    /// Install the full WARM fp32 tier and upgrade `ColdBinaryOnly` → `FullTwoStage`
    /// in one pass (ADR-023 T-C/T-E FullEager path).
    pub fn restore_warm_tier(&mut self, vectors: Vec<(String, Vec<f32>)>) -> Result<()> {
        self.install_warm_vectors(&vectors)?;
        self.serving_state = IvfServingState::FullTwoStage;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{IvfClusteringMethod, PartitionedKey};
    use crate::compute::distance_computation::DistanceMetric;
    use crate::index::axis::*;

    #[test]
    fn binary_tier_env_parsing() {
        use super::parse_binary_tier_enabled;
        // Truthy values (case- and whitespace-insensitive).
        for v in ["1", "true", "TRUE", " yes ", "On"] {
            assert!(
                parse_binary_tier_enabled(Some(v.to_string())),
                "{v:?} should enable"
            );
        }
        // Everything else is off — including unset, empty, and stray values.
        for v in ["0", "false", "no", "off", "", "2", "enabled"] {
            assert!(
                !parse_binary_tier_enabled(Some(v.to_string())),
                "{v:?} should NOT enable"
            );
        }
        assert!(
            !parse_binary_tier_enabled(None),
            "unset => off (default unchanged)"
        );
    }

    #[tokio::test]
    async fn test_unified_ivf_basic() {
        // Initialize hardware capabilities for testing
        let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init

        let config = UnifiedIvfConfig {
            n_clusters: 2, // Reduce clusters to match small dataset
            n_probe: 2,    // Search all clusters
            dimension: 4,
            distance_metric: DistanceMetric::Euclidean,
            quantization_bits: 0,
            use_pq: false,
            pq_subspaces: 0,
            use_binary: false,
            clustering_method: IvfClusteringMethod::KMeans,
            train_on_insert: false,
            min_train_size: 100,
            max_iterations: 20,
            tolerance: 0.01,
            n_init: 1,
            centroid_config: CentroidConfig::default(),
            posting_list_config: PostingListConfig::default(),
            recall_target: 0.95,
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
        assert!(
            results.len() >= 1,
            "Should find at least 1 result, got {}",
            results.len()
        );
        assert_eq!(results[0].0, "vec1");
    }

    // ─── TD-087 binary tier + two-stage retrieval ───────────────────────────

    #[test]
    fn binary_code_signs_and_hamming() {
        use super::BinaryCode;
        // sign-quantization: x >= 0 → 1 bit
        let a = BinaryCode::from_f32(&[1.0, -1.0, 1.0, -1.0]);
        let b = BinaryCode::from_f32(&[-1.0, 1.0, -1.0, 1.0]);
        assert_eq!(a.hamming(&a), 0);
        assert_eq!(a.hamming(&b), 4); // all four signs differ
        let near_a = BinaryCode::from_f32(&[0.9, -0.8, 1.1, -0.2]);
        assert_eq!(a.hamming(&near_a), 0); // same sign pattern as a
    }

    #[test]
    fn rotated_residual_code_is_deterministic_and_self_consistent() {
        use super::{rotation_signs, BinaryCode, COLD_ROTATION_SEED};
        // dim=6 → padded to next_pow2 = 8; exercises zero-padding + WHT.
        let signs = rotation_signs(8, COLD_ROTATION_SEED);
        let centroid = vec![0.1, 0.2, -0.3, 0.4, -0.5, 0.6];
        let x = vec![0.9, -0.8, 0.7, -0.6, 0.5, -0.4];

        // Deterministic: same inputs → identical bits.
        let c1 = BinaryCode::from_rotated_residual(&x, &centroid, &signs);
        let c2 = BinaryCode::from_rotated_residual(&x, &centroid, &signs);
        assert_eq!(c1, c2);

        // Self-consistent: a "query" equal to the stored vector (same residual to
        // the same centroid) yields Hamming 0 — the invariant the search relies on.
        let q = x.clone();
        let cq = BinaryCode::from_rotated_residual(&q, &centroid, &signs);
        assert_eq!(c1.hamming(&cq), 0);

        // A different vector almost surely differs in at least one rotated sign.
        let y = vec![-0.9, 0.8, -0.7, 0.6, -0.5, 0.4];
        let cy = BinaryCode::from_rotated_residual(&y, &centroid, &signs);
        assert!(c1.hamming(&cy) > 0);
    }

    #[test]
    fn adaptive_candidate_k_and_gap_early_termination() {
        use super::{adaptive_candidate_k, separation_gap, should_early_terminate};

        // Candidate count grows with the recall target, floored.
        assert!(adaptive_candidate_k(10, 0.98) > adaptive_candidate_k(10, 0.85));
        assert!(adaptive_candidate_k(1, 0.85) >= super::MIN_RERANK_CANDIDATES); // floor

        // Separation gap: ascending Hamming list; 0 when there's no (k+1)-th.
        let c = |h: u32| ("x".to_string(), h);
        assert_eq!(separation_gap(&[c(1), c(2)], 5), 0.0); // fewer than k+1
        assert_eq!(separation_gap(&[c(0), c(10)], 1), 1.0); // d_k=0, d_{k+1}=10 → 1.0
        let g = separation_gap(&[c(2), c(3)], 1); // (3-2)/3 ≈ 0.33
        assert!((0.30..0.36).contains(&g));

        // Early-termination bar: rt=1.0 never; lower rt fires on enough separation.
        assert!(!should_early_terminate(0.99, 1.0)); // rt 1.0 disables
        assert!(should_early_terminate(0.6, 0.5)); // gap above bar
        assert!(!should_early_terminate(0.4, 0.5)); // gap below bar
    }

    #[tokio::test]
    async fn low_recall_target_early_terminates_but_stays_correct() {
        // A low recall_target makes early termination fire; correctness (top-1)
        // must still hold for a well-separated (exact-match) query.
        let _ = proximadb_hardware::hardware_capabilities();
        let mut cfg = binary_ivf_config(4, 2);
        cfg.recall_target = 0.5; // aggressive early termination
        let mut index = UnifiedIvfIndex::new("c_et".to_string(), cfg).unwrap();
        let data = mixed_sign_vectors();
        index
            .train(data.iter().map(|(_, v)| v.clone()).collect())
            .await
            .unwrap();
        for (id, v) in &data {
            index.add_vector(id.clone(), v.clone(), None).await.unwrap();
        }
        // Exact-match query → its rotated residual matches the stored code
        // (Hamming 0) and is well separated, so early termination returns it first.
        let query = data[0].1.clone(); // "v0"
        let results = index
            .search_with_binary_acceleration(&query, 3, None)
            .await
            .unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].0, "v0", "top-1 correct even when early-terminated");
    }

    fn binary_ivf_config(dim: usize, n_clusters: usize) -> UnifiedIvfConfig {
        UnifiedIvfConfig {
            n_clusters,
            n_probe: n_clusters, // probe all clusters (no IVF pruning in the test)
            dimension: dim,
            distance_metric: DistanceMetric::Euclidean,
            quantization_bits: 0,
            use_pq: false,
            pq_subspaces: 0,
            use_binary: true,
            clustering_method: IvfClusteringMethod::KMeans,
            train_on_insert: false,
            min_train_size: 100,
            max_iterations: 20,
            tolerance: 0.01,
            n_init: 1,
            centroid_config: CentroidConfig::default(),
            posting_list_config: PostingListConfig::default(),
            // recall_target 1.0 disables early termination so the two-stage
            // correctness tests get exact (full-rerank) results deterministically.
            recall_target: 1.0,
        }
    }

    // 8 mixed-sign 4-d vectors so binary sign-quantization is discriminative.
    fn mixed_sign_vectors() -> Vec<(String, Vec<f32>)> {
        vec![
            ("v0", vec![1.0, -1.0, 1.0, -1.0]),
            ("v1", vec![-1.0, 1.0, -1.0, 1.0]),
            ("v2", vec![1.0, 1.0, -1.0, -1.0]),
            ("v3", vec![-1.0, -1.0, 1.0, 1.0]),
            ("v4", vec![1.0, -1.0, -1.0, 1.0]),
            ("v5", vec![-1.0, 1.0, 1.0, -1.0]),
            ("v6", vec![1.0, 1.0, 1.0, 1.0]),
            ("v7", vec![-1.0, -1.0, -1.0, -1.0]),
        ]
        .into_iter()
        .map(|(id, v)| (id.to_string(), v))
        .collect()
    }

    #[tokio::test]
    async fn binary_storage_populates_on_add_and_gates_has_quantized() {
        let _ = proximadb_hardware::hardware_capabilities();
        let mut index = UnifiedIvfIndex::new("c_bin".to_string(), binary_ivf_config(4, 2)).unwrap();
        assert!(
            !index.has_quantized_storage(),
            "empty index has no binary tier"
        );
        let data = mixed_sign_vectors();
        index
            .train(data.iter().map(|(_, v)| v.clone()).collect())
            .await
            .unwrap();
        for (id, v) in &data {
            index.add_vector(id.clone(), v.clone(), None).await.unwrap();
        }
        assert!(
            index.has_quantized_storage(),
            "binary tier should be populated after add with use_binary"
        );
    }

    #[tokio::test]
    async fn binary_two_stage_matches_exact_topk() {
        let _ = proximadb_hardware::hardware_capabilities();
        let mut index =
            UnifiedIvfIndex::new("c_bin2".to_string(), binary_ivf_config(4, 2)).unwrap();
        let data = mixed_sign_vectors();
        index
            .train(data.iter().map(|(_, v)| v.clone()).collect())
            .await
            .unwrap();
        for (id, v) in &data {
            index.add_vector(id.clone(), v.clone(), None).await.unwrap();
        }

        // Query near v0; candidate_k = k*4 >= dataset, so the binary route reranks
        // the full set → identical top-k to exact search (recall = 1.0).
        let query = vec![0.9, -0.8, 1.1, -0.7];
        let exact = index.search(&query, 3, None).await.unwrap();
        let binary = index
            .search_with_binary_acceleration(&query, 3, None)
            .await
            .unwrap();
        assert_eq!(binary[0].0, "v0", "top-1 should be the nearest vector");
        let exact_ids: Vec<&String> = exact.iter().map(|(id, _)| id).collect();
        let binary_ids: Vec<&String> = binary.iter().map(|(id, _)| id).collect();
        assert_eq!(
            binary_ids, exact_ids,
            "two-stage top-k must equal exact top-k"
        );

        // The gated quantized route delegates to the binary two-stage path.
        let gated = index
            .search_with_quantized_acceleration(&query, 3, None)
            .await
            .unwrap();
        assert_eq!(
            gated.iter().map(|(id, _)| id).collect::<Vec<_>>(),
            binary_ids
        );
    }

    // ─── ADR-023 T-F: Stage-1-only (ColdBinaryOnly) recall floor ─────────────

    /// Deterministic synthetic clustered corpus (splitmix64, no external deps),
    /// so the measured recall floor is reproducible in CI. Well-separated
    /// cluster centers + small intra-cluster noise → IVF clustering and the
    /// rotated-residual 1-bit codes are both discriminative.
    fn synth_clustered_corpus(
        dim: usize,
        n_clusters: usize,
        per_cluster: usize,
    ) -> Vec<(String, Vec<f32>)> {
        let mut state: u64 = 0xC0FF_EE12_3456_789A;
        let mut next = move || {
            state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            // u64 → f32 in [-1, 1).
            ((z >> 40) as f32 / (1u32 << 24) as f32) * 2.0 - 1.0
        };
        let mut out = Vec::with_capacity(n_clusters * per_cluster);
        for c in 0..n_clusters {
            // Large center amplitude so clusters separate; small noise keeps
            // each point near its center (a real NN lives in the same cluster).
            let center: Vec<f32> = (0..dim).map(|_| next() * 8.0).collect();
            for j in 0..per_cluster {
                let v: Vec<f32> = center.iter().map(|&cc| cc + next() * 0.6).collect();
                out.push((format!("c{c}_v{j}"), v));
            }
        }
        out
    }

    /// Measures the recall lost by serving Hamming-only (rotated-residual 1-bit,
    /// no fp32 rerank — the `ColdBinaryOnly` state) versus the exact two-stage
    /// path. This is the honest envelope T-E discloses and the input to the R4
    /// MID-tier go/no-go gate.
    ///
    /// MEASURED (dim=64, n=800, 16 clusters, well-separated; deterministic):
    /// ```text
    ///   n_probe   recall@10
    ///     1         0.512   <- best: prune to the true cluster, rank within it
    ///     2         0.358
    ///     4         0.243
    ///     8         0.177
    ///    16         0.145   <- merge all frames: worse than random (10/50=0.20)
    /// ```
    /// Two findings: (1) the 1-bit floor (~0.51) clears the 0.40 go/no-go bar but
    /// sits BELOW AQR-HNSW's ~0.75 naive-1-bit prior on this pathologically tight
    /// within-cluster ranking task — so 1-bit cold serving is marginal and the R4
    /// 2-bit MID tier stays warranted (or fall back to FullEager) until a real
    /// corpus says otherwise. (2) recall INVERTS with n_probe — the opposite of
    /// the warm path. Raw Hamming is only valid within ONE residual frame (rotated
    /// to that cluster's centroid); merging probed clusters drops the coarse
    /// centroid-distance term, so cross-cluster ordering is mis-ranked. The cold
    /// path must therefore probe minimally (n_probe=1) OR compose coarse+residual
    /// distance (the RaBitQ estimator) before multi-probe is honest. The assertion
    /// guards the single-probe floor.
    #[tokio::test]
    async fn cold_stage1_only_recall_floor_is_measured_and_disclosed() {
        let _ = proximadb_hardware::hardware_capabilities();
        let dim = 64;
        let n_clusters = 16;
        let per_cluster = 50;
        let data = synth_clustered_corpus(dim, n_clusters, per_cluster);

        // FullTwoStage reference. recall_target 1.0 (from binary_ivf_config) +
        // n_probe = n_clusters → exact brute-force top-k (the ground truth).
        let mut full =
            UnifiedIvfIndex::new("cold_floor".to_string(), binary_ivf_config(dim, n_clusters))
                .unwrap();
        full.train(data.iter().map(|(_, v)| v.clone()).collect())
            .await
            .unwrap();
        for (id, v) in &data {
            full.add_vector(id.clone(), v.clone(), None).await.unwrap();
        }

        // Cold-start replica: COLD tier only → ColdBinaryOnly (Hamming, no fp32).
        let cold_tier = full.export_cold_tier().await.unwrap();
        let mut cold =
            UnifiedIvfIndex::new("cold_floor".to_string(), cold_tier.config.to_config()).unwrap();
        cold.restore_cold_only(cold_tier).await.unwrap();
        assert_eq!(cold.serving_state(), IvfServingState::ColdBinaryOnly);

        // Queries: deterministically jitter existing vectors so each has a real
        // nearest neighbour in the corpus.
        let k = 10;
        let n_queries = 60;
        let queries: Vec<Vec<f32>> = {
            let mut qstate: u64 = 0x1234_5678_9ABC_DEF0;
            let mut jitter = move || {
                qstate = qstate.wrapping_add(0x9E37_79B9_7F4A_7C15);
                let mut z = qstate;
                z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
                z ^= z >> 31;
                ((z >> 40) as f32 / (1u32 << 24) as f32) * 2.0 - 1.0
            };
            (0..n_queries)
                .map(|qi| {
                    let base = &data[(qi * 13) % data.len()].1;
                    base.iter().map(|&x| x + jitter() * 0.3).collect()
                })
                .collect()
        };

        // Ground truth is brute-force exact (probe ALL clusters, full rerank).
        // The cold replica's centroids are resident fp32, so it prunes to the
        // nearest clusters EXACTLY, then Hamming-ranks only within them — sweep
        // n_probe to separate the 1-bit quantization floor (n_probe=1, the true
        // cluster only) from cross-cluster Hamming contamination (n_probe high,
        // where incomparable per-centroid residual frames merge and degrade).
        let mut floor_at_1 = 0.0f64;
        for &probe in &[1usize, 2, 4, 8, n_clusters] {
            let mut total = 0.0f64;
            for query in &queries {
                let exact = full.search(query, k, Some(n_clusters)).await.unwrap();
                let stage1 = cold
                    .search_with_binary_acceleration(query, k, Some(probe))
                    .await
                    .unwrap();
                let truth: std::collections::HashSet<&String> =
                    exact.iter().map(|(id, _)| id).collect();
                let hit = stage1.iter().filter(|(id, _)| truth.contains(id)).count();
                total += hit as f64 / k as f64;
            }
            let recall = total / n_queries as f64;
            println!(
                "ADR-023 T-F Stage-1-only recall@{k} = {recall:.3} @ n_probe={probe} (dim={dim}, n={}, clusters={n_clusters})",
                data.len()
            );
            if probe == 1 {
                floor_at_1 = recall;
            }
        }

        // The honest envelope T-E discloses: at n_probe=1 (exact-centroid prune
        // to the true cluster, then Hamming-rank ~50 candidates) the 1-bit floor
        // must clear random selection (k/per_cluster) by a clear margin. This is
        // the R4 MID-tier go/no-go input — if it cannot clear ~0.40, a 2-bit MID
        // tier is warranted; if it clears it comfortably, 1-bit cold serving is
        // honest to disclose.
        assert!(
            floor_at_1 >= 0.40,
            "Stage-1-only recall@1-probe {floor_at_1:.3} is below the 0.40 \
             go/no-go bar; the cold envelope is too weak to disclose without R4"
        );
    }

    // ─── TD-087 Slice B: serialization round-trip ───────────────────────────

    #[tokio::test]
    async fn export_state_emits_separable_cold_binary_tier() {
        // ADR-023 T-A: export carries the COLD tier (id, packed bits, cluster_id)
        // separately from the WARM fp32 tier, one entry per indexed vector.
        let _ = proximadb_hardware::hardware_capabilities();
        let mut index =
            UnifiedIvfIndex::new("c_cold".to_string(), binary_ivf_config(4, 2)).unwrap();
        let data = mixed_sign_vectors();
        index
            .train(data.iter().map(|(_, v)| v.clone()).collect())
            .await
            .unwrap();
        for (id, v) in &data {
            index.add_vector(id.clone(), v.clone(), None).await.unwrap();
        }

        let state = index.export_state().await.unwrap();
        assert_eq!(
            state.binary_tier.len(),
            data.len(),
            "one COLD entry per indexed vector"
        );
        for (id, bits, cluster) in &state.binary_tier {
            assert!(
                (*cluster as usize) < state.centroids.len(),
                "cluster_id {cluster} in range for {id}"
            );
            assert_eq!(bits.len(), 4usize.div_ceil(8), "1 byte packs 4 sign bits");
        }
        // WARM tier still carries every fp32 vector.
        assert_eq!(state.vectors.len(), data.len());
    }

    #[tokio::test]
    async fn restore_uses_cold_tier_and_preserves_binary_topk() {
        // ADR-023 T-A: a v2 round-trip installs binary codes from the COLD tier
        // (authoritative) and the binary two-stage route is unchanged.
        use crate::index::axis::storage::serialization::IndexSerializer;
        let _ = proximadb_hardware::hardware_capabilities();
        let mut index =
            UnifiedIvfIndex::new("c_cold_rt".to_string(), binary_ivf_config(4, 2)).unwrap();
        let data = mixed_sign_vectors();
        index
            .train(data.iter().map(|(_, v)| v.clone()).collect())
            .await
            .unwrap();
        for (id, v) in &data {
            index.add_vector(id.clone(), v.clone(), None).await.unwrap();
        }
        let query = vec![0.9, -0.8, 1.1, -0.7];
        let before = index
            .search_with_binary_acceleration(&query, 3, None)
            .await
            .unwrap();

        let bytes = IndexSerializer::serialize_ivf(&index, "c_cold_rt").await.unwrap();
        let (restored, _meta) = IndexSerializer::deserialize_ivf(&bytes).await.unwrap();
        assert!(restored.has_binary_tier(), "COLD tier installed on restore");
        let after = restored
            .search_with_binary_acceleration(&query, 3, None)
            .await
            .unwrap();
        let ids = |v: &Vec<(String, f32)>| v.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&after), ids(&before), "binary top-k identical after COLD restore");
    }

    #[tokio::test]
    async fn cold_only_restore_serves_stage1_without_fp32() {
        // ADR-023 T-B/T-D: restore from JUST the COLD tier (no fp32) and serve
        // Stage-1 Hamming results. Proves the ~1/32 blob is independently
        // sufficient for cold-start serving.
        let _ = proximadb_hardware::hardware_capabilities();
        let mut full =
            UnifiedIvfIndex::new("c_full".to_string(), binary_ivf_config(4, 2)).unwrap();
        let data = mixed_sign_vectors();
        full.train(data.iter().map(|(_, v)| v.clone()).collect())
            .await
            .unwrap();
        for (id, v) in &data {
            full.add_vector(id.clone(), v.clone(), None).await.unwrap();
        }
        assert_eq!(full.serving_state(), IvfServingState::FullTwoStage);

        let cold = full.export_cold_tier().await.unwrap();
        assert_eq!(cold.binary_tier.len(), data.len());

        // Build a fresh index from ONLY the cold tier (no fp32 ever inserted).
        let mut cold_idx =
            UnifiedIvfIndex::new("c_cold_only".to_string(), cold.config.to_config()).unwrap();
        cold_idx.restore_cold_only(cold).await.unwrap();
        assert_eq!(cold_idx.serving_state(), IvfServingState::ColdBinaryOnly);
        assert!(cold_idx.has_binary_tier());
        assert_eq!(cold_idx.len(), data.len());

        // Stage-1-only search returns Hamming-ranked candidates without any fp32
        // rerank. Query an EXACT indexed vector: its rotated residual matches the
        // stored code (Hamming 0), so it ranks first regardless of the rotation.
        let query = data[0].1.clone(); // == "v0"
        let results = cold_idx
            .search_with_binary_acceleration(&query, 3, None)
            .await
            .unwrap();
        assert!(!results.is_empty(), "cold-only serves Stage-1 results");
        assert!(results.len() <= 3);
        assert_eq!(results[0].0, "v0", "exact-match query ranks first (Hamming 0)");
    }

    #[tokio::test]
    async fn cold_first_load_serves_stage1_then_upgrades_to_full() {
        // ADR-023 T-C: load the COLD blob first → ColdBinaryOnly serving, with the
        // WARM fp32 tier deferred; then apply WARM → FullTwoStage exact search.
        use crate::index::axis::storage::serialization::IndexSerializer;
        use crate::index::axis::ColdPathLoadPolicy;
        let _ = proximadb_hardware::hardware_capabilities();
        let mut index =
            UnifiedIvfIndex::new("c_cf".to_string(), binary_ivf_config(4, 2)).unwrap();
        let data = mixed_sign_vectors();
        index
            .train(data.iter().map(|(_, v)| v.clone()).collect())
            .await
            .unwrap();
        for (id, v) in &data {
            index.add_vector(id.clone(), v.clone(), None).await.unwrap();
        }
        let bytes = IndexSerializer::serialize_ivf(&index, "c_cf").await.unwrap();

        // BinaryFirstThenRerank: COLD only → ColdBinaryOnly + deferred WARM.
        let mut loaded =
            IndexSerializer::load_ivf_with_policy(&bytes, ColdPathLoadPolicy::BinaryFirstThenRerank)
                .await
                .unwrap();
        assert_eq!(loaded.index.serving_state(), IvfServingState::ColdBinaryOnly);
        assert!(loaded.warm.is_some(), "WARM bytes deferred");
        assert_eq!(loaded.index.len(), data.len());
        let profile = loaded.metadata.cold_path_profile().unwrap();
        assert!(profile.cold_tier_bytes > 0 && profile.warm_tier_bytes > 0);

        // Stage-1 serves immediately (exact-vector query → Hamming 0 → top-1).
        let q = data[0].1.clone();
        let s1 = loaded
            .index
            .search_with_binary_acceleration(&q, 3, None)
            .await
            .unwrap();
        assert_eq!(s1[0].0, "v0");

        // Apply the deferred WARM tier → FullTwoStage; exact search now works.
        let warm = IndexSerializer::decode_warm_tier(loaded.warm.as_ref().unwrap()).unwrap();
        assert_eq!(warm.len(), data.len());
        loaded.index.restore_warm_tier(warm).unwrap();
        assert_eq!(loaded.index.serving_state(), IvfServingState::FullTwoStage);
        assert_eq!(loaded.index.search(&q, 1, None).await.unwrap()[0].0, "v0");

        // FullEager loads both tiers at once → FullTwoStage, no deferred WARM.
        let eager =
            IndexSerializer::load_ivf_with_policy(&bytes, ColdPathLoadPolicy::FullEager)
                .await
                .unwrap();
        assert_eq!(eager.index.serving_state(), IvfServingState::FullTwoStage);
        assert!(eager.warm.is_none());
    }

    #[tokio::test]
    async fn warm_tier_is_per_cluster_and_chunked_apply_upgrades() {
        // ADR-023 R3: WARM is grouped per cluster; applying clusters one at a time
        // (no flip) keeps the index ColdBinaryOnly until the final mark — then
        // exact search works. This is what lets warm-apply overlap with serving.
        use crate::index::axis::storage::serialization::IndexSerializer;
        use crate::index::axis::ColdPathLoadPolicy;
        let _ = proximadb_hardware::hardware_capabilities();
        let mut full = UnifiedIvfIndex::new("c_r3".to_string(), binary_ivf_config(4, 2)).unwrap();
        let data = mixed_sign_vectors();
        full.train(data.iter().map(|(_, v)| v.clone()).collect())
            .await
            .unwrap();
        for (id, v) in &data {
            full.add_vector(id.clone(), v.clone(), None).await.unwrap();
        }
        let bytes = IndexSerializer::serialize_ivf(&full, "c_r3").await.unwrap();
        let mut loaded =
            IndexSerializer::load_ivf_with_policy(&bytes, ColdPathLoadPolicy::BinaryFirstThenRerank)
                .await
                .unwrap();
        let warm_bytes = loaded.warm.take().unwrap();

        // WARM is split into per-cluster extents covering every vector.
        let clusters = IndexSerializer::decode_warm_clusters(&warm_bytes).unwrap();
        assert!(!clusters.is_empty(), "warm is grouped into >=1 cluster extent");
        let total: usize = clusters.iter().map(|(_, v)| v.len()).sum();
        assert_eq!(total, data.len(), "every vector lands in some cluster extent");
        assert_eq!(loaded.index.serving_state(), IvfServingState::ColdBinaryOnly);

        // Chunked apply: install each cluster (no flip) — still cold until marked.
        for (_cid, vecs) in &clusters {
            loaded.index.restore_warm_cluster(vecs).unwrap();
        }
        assert_eq!(
            loaded.index.serving_state(),
            IvfServingState::ColdBinaryOnly,
            "still cold until the explicit flip"
        );
        loaded.index.mark_full_two_stage();
        assert_eq!(loaded.index.serving_state(), IvfServingState::FullTwoStage);
        let q = data[0].1.clone();
        assert_eq!(loaded.index.search(&q, 1, None).await.unwrap()[0].0, "v0");
    }

    #[tokio::test]
    async fn cold_tier_is_a_small_fraction_of_the_full_index() {
        // ADR-023 T-F (success criterion #1): the COLD blob — loaded before
        // serving begins — is a small fraction of the full index. The per-vector
        // ratio is ~1/32 (1 sign bit vs fp32) asymptotically; at modest scale the
        // fixed centroid cost lifts it, so we assert a conservative envelope.
        use crate::index::axis::storage::serialization::IndexSerializer;
        let _ = proximadb_hardware::hardware_capabilities();
        let dim = 64;
        let n = 128usize;
        let mut cfg = binary_ivf_config(dim, 8);
        cfg.min_train_size = 8;
        let mut index = UnifiedIvfIndex::new("c_tf".to_string(), cfg).unwrap();
        let data: Vec<(String, Vec<f32>)> = (0..n)
            .map(|i| {
                let v: Vec<f32> = (0..dim)
                    .map(|d| (((i * 31 + d * 17) % 13) as f32) - 6.0)
                    .collect();
                (format!("v{i}"), v)
            })
            .collect();
        index
            .train(data.iter().map(|(_, v)| v.clone()).collect())
            .await
            .unwrap();
        for (id, v) in &data {
            index.add_vector(id.clone(), v.clone(), None).await.unwrap();
        }

        let bytes = IndexSerializer::serialize_ivf(&index, "c_tf").await.unwrap();
        let (_idx, meta) = IndexSerializer::deserialize_ivf(&bytes).await.unwrap();
        let p = meta.cold_path_profile().unwrap();
        assert!(p.has_binary_tier);
        assert!(
            p.cold_tier_bytes * 4 < p.warm_tier_bytes,
            "COLD tier {} bytes (served first) must be << WARM {} bytes (deferred fp32)",
            p.cold_tier_bytes,
            p.warm_tier_bytes,
        );
    }

    #[tokio::test]
    async fn load_ivf_cold_path_auto_selects_policy() {
        // ADR-023 T-E: a binary-tier index loads cold-first (WARM deferred); a
        // non-binary index loads fully (WARM None), since its membership lives
        // only in the full body.
        use crate::index::axis::storage::serialization::IndexSerializer;
        let _ = proximadb_hardware::hardware_capabilities();
        let data = mixed_sign_vectors();

        // Binary collection → v2 cold-first.
        let mut bin = UnifiedIvfIndex::new("c_b".to_string(), binary_ivf_config(4, 2)).unwrap();
        bin.train(data.iter().map(|(_, v)| v.clone()).collect())
            .await
            .unwrap();
        for (id, v) in &data {
            bin.add_vector(id.clone(), v.clone(), None).await.unwrap();
        }
        let bin_bytes = IndexSerializer::serialize_ivf(&bin, "c_b").await.unwrap();
        let bin_load = IndexSerializer::load_ivf_cold_path(&bin_bytes).await.unwrap();
        assert!(bin_load.warm.is_some(), "binary index loads cold-first");
        assert_eq!(bin_load.index.serving_state(), IvfServingState::ColdBinaryOnly);

        // Non-binary collection → v1 full eager.
        let mut cfg = binary_ivf_config(4, 2);
        cfg.use_binary = false;
        let mut plain = UnifiedIvfIndex::new("c_p".to_string(), cfg).unwrap();
        plain
            .train(data.iter().map(|(_, v)| v.clone()).collect())
            .await
            .unwrap();
        for (id, v) in &data {
            plain.add_vector(id.clone(), v.clone(), None).await.unwrap();
        }
        let plain_bytes = IndexSerializer::serialize_ivf(&plain, "c_p").await.unwrap();
        let plain_load = IndexSerializer::load_ivf_cold_path(&plain_bytes).await.unwrap();
        assert!(plain_load.warm.is_none(), "non-binary index loads fully");
        assert_eq!(plain_load.index.serving_state(), IvfServingState::FullTwoStage);
        // And it serves exact results immediately (full eager).
        let q = data[0].1.clone();
        assert_eq!(plain_load.index.search(&q, 1, None).await.unwrap()[0].0, "v0");
    }

    #[tokio::test]
    async fn serialize_ivf_writes_cold_path_profile() {
        // ADR-023 T-A: the serialized metadata carries a decodable cold-path
        // profile (has_binary_tier + per-tier byte sizes).
        use crate::index::axis::storage::serialization::IndexSerializer;
        let _ = proximadb_hardware::hardware_capabilities();
        let mut index =
            UnifiedIvfIndex::new("c_prof".to_string(), binary_ivf_config(4, 2)).unwrap();
        let data = mixed_sign_vectors();
        index
            .train(data.iter().map(|(_, v)| v.clone()).collect())
            .await
            .unwrap();
        for (id, v) in &data {
            index.add_vector(id.clone(), v.clone(), None).await.unwrap();
        }

        let bytes = IndexSerializer::serialize_ivf(&index, "c_prof").await.unwrap();
        let (_restored, meta) = IndexSerializer::deserialize_ivf(&bytes).await.unwrap();
        let profile = meta
            .cold_path_profile()
            .expect("ADR-023 cold-path profile present in metadata");
        assert!(profile.has_binary_tier, "binary tier populated");
        assert!(profile.cold_tier_bytes > 0 && profile.warm_tier_bytes > 0);
    }

    #[tokio::test]
    async fn serialize_roundtrip_preserves_search_topk() {
        use crate::index::axis::storage::serialization::IndexSerializer;
        let _ = proximadb_hardware::hardware_capabilities();
        let mut index = UnifiedIvfIndex::new("c_ser".to_string(), binary_ivf_config(4, 2)).unwrap();
        let data = mixed_sign_vectors();
        index
            .train(data.iter().map(|(_, v)| v.clone()).collect())
            .await
            .unwrap();
        for (id, v) in &data {
            index.add_vector(id.clone(), v.clone(), None).await.unwrap();
        }

        let query = vec![0.9, -0.8, 1.1, -0.7];
        let exact_before = index.search(&query, 3, None).await.unwrap();
        let binary_before = index
            .search_with_binary_acceleration(&query, 3, None)
            .await
            .unwrap();

        // Serialize → deserialize into a fresh index (no retrain).
        let bytes = IndexSerializer::serialize_ivf(&index, "c_ser")
            .await
            .unwrap();
        let (restored, meta) = IndexSerializer::deserialize_ivf(&bytes).await.unwrap();
        assert_eq!(meta.num_vectors, 8);
        assert_eq!(meta.dimension, 4);
        assert_eq!(restored.len(), 8);
        assert!(
            restored.has_binary_tier(),
            "binary tier reconstructed on restore"
        );

        // The reloaded index returns identical top-k on both routes.
        let ids = |v: &Vec<(String, f32)>| v.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>();
        let exact_after = restored.search(&query, 3, None).await.unwrap();
        let binary_after = restored
            .search_with_binary_acceleration(&query, 3, None)
            .await
            .unwrap();
        assert_eq!(
            ids(&exact_after),
            ids(&exact_before),
            "exact top-k identical after reload"
        );
        assert_eq!(
            ids(&binary_after),
            ids(&binary_before),
            "binary two-stage top-k identical after reload"
        );
    }

    #[tokio::test]
    async fn deserialize_rejects_corrupted_or_truncated_bytes() {
        use crate::index::axis::storage::serialization::IndexSerializer;
        let _ = proximadb_hardware::hardware_capabilities();
        let mut index =
            UnifiedIvfIndex::new("c_corrupt".to_string(), binary_ivf_config(4, 2)).unwrap();
        let data = mixed_sign_vectors();
        index
            .train(data.iter().map(|(_, v)| v.clone()).collect())
            .await
            .unwrap();
        for (id, v) in &data {
            index.add_vector(id.clone(), v.clone(), None).await.unwrap();
        }
        let mut bytes = IndexSerializer::serialize_ivf(&index, "c_corrupt")
            .await
            .unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF; // flip a payload byte → checksum mismatch
        assert!(IndexSerializer::deserialize_ivf(&bytes).await.is_err());
        assert!(IndexSerializer::deserialize_ivf(&bytes[..2]).await.is_err()); // truncated
    }

    #[tokio::test]
    async fn persist_and_load_ivf_index_roundtrips_on_disk() {
        use crate::index::axis::storage::serialization::IndexSerializer;
        let _ = proximadb_hardware::hardware_capabilities();
        let mut index =
            UnifiedIvfIndex::new("c_disk".to_string(), binary_ivf_config(4, 2)).unwrap();
        let data = mixed_sign_vectors();
        index
            .train(data.iter().map(|(_, v)| v.clone()).collect())
            .await
            .unwrap();
        for (id, v) in &data {
            index.add_vector(id.clone(), v.clone(), None).await.unwrap();
        }
        let path = std::env::temp_dir().join(format!(
            "proximadb_ivf_persist_{}/ivf.bin",
            uuid::Uuid::new_v4().simple()
        ));
        IndexSerializer::persist_ivf_index(&index, "c_disk", &path)
            .await
            .unwrap();
        assert!(path.exists());
        let (restored, _meta) = IndexSerializer::load_ivf_index(&path).await.unwrap();
        let query = vec![0.9, -0.8, 1.1, -0.7];
        assert_eq!(
            restored.search(&query, 1, None).await.unwrap()[0].0,
            index.search(&query, 1, None).await.unwrap()[0].0
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
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
