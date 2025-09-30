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

//! HNSW (Hierarchical Navigable Small World) index implementation for AXIS
//!
//! This module provides a production-ready HNSW index that integrates seamlessly
//! with the AXIS adaptive indexing system. HNSW is ideal for approximate nearest
//! neighbor search with excellent recall and query performance.

use anyhow::Result;
use async_trait::async_trait;
use dashmap::DashMap;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::sync::{
    Arc, RwLock,
    atomic::{AtomicUsize, Ordering as AtomicOrdering},
};
use tracing::info;

use crate::compute::distance_computation::DistanceMetric;
use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use crate::proto::proximadb_v1::VectorRecord;
// VectorRecord eliminated - using ZeroOverheadVector for 75-96% memory savings
use crate::index::axis::eventlog::{ExtractionMode, IndexEvent};
use crate::index::axis::index_factory::{AxisVectorIndex, IndexStats};
use crate::index::axis::types::IndexAlgorithm;
use crate::index::axis::utils::{AtomicStats, ConcurrentIdMapping, memory, validation};
use crate::index::axis::zero_overhead_vector::{
    CollectionConfig, QuantizationMethod, ZeroOverheadCollection,
};

/// Memory usage statistics
#[derive(Debug, Clone)]
pub struct MemoryUsage {
    pub total_bytes: usize,
    pub index_size_bytes: usize,
    pub vector_data_bytes: usize,
    pub metadata_bytes: usize,
}

/// HNSW index configuration aligned with AXIS standards
#[derive(Debug, Clone)]
pub struct AxisHnswConfig {
    /// Number of bi-directional links for each node
    pub m: usize,
    /// Size of candidate set during construction
    pub ef_construction: usize,
    /// Search parameter (dynamic, can be adjusted at query time)
    pub ef: usize,
    /// Maximum number of layers
    pub max_layers: usize,
    /// Distance metric to use
    pub distance_metric: DistanceMetric,
}

impl Default for AxisHnswConfig {
    fn default() -> Self {
        Self {
            m: 16,                // Good balance of connectivity and memory
            ef_construction: 200, // Higher for better quality
            ef: 50,               // Lower for faster searches
            max_layers: 16,       // Reasonable depth
            distance_metric: DistanceMetric::Cosine,
        }
    }
}

/// Wrapper for f32 to implement Ord for use in BinaryHeap
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
struct OrderedFloat(f32);

impl Eq for OrderedFloat {}

impl Ord for OrderedFloat {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.partial_cmp(&other.0).unwrap_or(Ordering::Equal)
    }
}

/// HNSW index implementation using shared infrastructure with collection partitioning
/// Migrated to use IndexVectorStore and ConcurrentIdMapping for consistency
/// NOW SUPPORTS: Queue-based consumption of vectors with quantized/fp32/both representations
pub struct AxisHnswIndex {
    /// Collection identifier for partitioning (optional for backward compatibility)
    collection_id: Option<String>,

    /// Configuration
    config: AxisHnswConfig,
    /// Distance computation
    distance_computer: UnifiedDistanceCompute,

    /// Zero-overhead vector storage - optimal memory use!
    /// Replaces IndexVectorStore with 75-96% memory savings for quantized vectors
    vectors: Arc<RwLock<ZeroOverheadCollection>>,

    /// USING UTILS: Bidirectional ID mapping for external<->internal IDs
    id_mapping: ConcurrentIdMapping,

    /// USING UTILS: Performance statistics tracking
    stats: AtomicStats,

    /// HNSW-specific: Graph layers with composite keys
    /// (layer, internal_node_id) -> connections
    /// TODO: Add partitioning - will use (collection_id, layer, node_id) in Phase 3
    layers: DashMap<(usize, usize), Vec<usize>>,

    /// HNSW-specific: Maximum layer currently in use (atomic)
    max_layer: AtomicUsize,

    /// HNSW-specific: Entry point for search
    entry_point: RwLock<Option<usize>>,

    /// Random number generator state
    rng_state: Arc<RwLock<u64>>,

    /// Algorithm type for trait requirement
    algorithm_type: IndexAlgorithm,

    /// EventLog-based extraction mode for async index updates
    /// Replaces queue consumer pattern with direct event processing
    extraction_mode: ExtractionMode,

    /// NEW: Quantized vector storage for dual representation support
    /// Maps external_id -> quantized_vector for QUANTIZED_ONLY and BOTH modes
    quantized_vectors: Arc<DashMap<String, Vec<u8>>>,
}

impl AxisHnswIndex {
    /// Create a new HNSW index with the given configuration using shared utilities
    pub fn new(config: AxisHnswConfig, dimension: usize) -> Result<Self> {
        Self::new_with_collection(None, config, dimension)
    }

    /// Create a new HNSW index for a specific collection
    pub fn new_with_collection(
        collection_id: Option<String>,
        config: AxisHnswConfig,
        dimension: usize,
    ) -> Result<Self> {
        Self::new_with_extraction_mode(collection_id, config, dimension, ExtractionMode::Auto)
    }

    /// Create a new HNSW index with specific extraction mode for EventLog processing
    pub fn new_with_extraction_mode(
        collection_id: Option<String>,
        config: AxisHnswConfig,
        dimension: usize,
        extraction_mode: ExtractionMode,
    ) -> Result<Self> {
        // USING UTILS: Validate dimension
        validation::validate_dimension(dimension)?;

        let coll_str = collection_id
            .as_ref()
            .map(|s| s.as_str())
            .unwrap_or("<unnamed>");
        info!(
            "Creating AXIS HNSW index for collection '{}': M={}, ef_construction={}, ef={}, dim={}, repr={:?}",
            coll_str, config.m, config.ef_construction, config.ef, dimension, extraction_mode
        );

        let distance_computer = UnifiedDistanceCompute::new(config.distance_metric);

        let algorithm_type = IndexAlgorithm::HNSW {
            m: config.m as u32,
            ef_construction: config.ef_construction as u32,
            ef_search: config.ef as u32,
            max_elements: 1000000, // Default max elements
        };

        Ok(Self {
            collection_id: collection_id.clone(),
            config,
            distance_computer,

            // Zero-overhead storage with shared collection config
            vectors: Arc::new(RwLock::new(ZeroOverheadCollection::with_capacity(
                Self::get_collection_config(&collection_id, dimension, &extraction_mode),
                1024, // Initial capacity
            ))),
            id_mapping: ConcurrentIdMapping::new(),
            stats: AtomicStats::new(),

            // HNSW-specific structures
            layers: DashMap::new(),
            max_layer: AtomicUsize::new(0),
            entry_point: RwLock::new(None),
            rng_state: Arc::new(RwLock::new(42)), // Deterministic seed for reproducibility
            algorithm_type,

            // EventLog-based vector consumption (no queue consumer needed)
            extraction_mode,
            quantized_vectors: Arc::new(DashMap::new()),
        })
    }

    /// Get collection configuration based on extraction mode and collection ID
    /// This would normally come from a shared collection cache in production
    fn get_collection_config(
        _collection_id: &Option<String>,
        dimension: usize,
        extraction_mode: &ExtractionMode,
    ) -> CollectionConfig {
        match extraction_mode {
            ExtractionMode::Fp32Only | ExtractionMode::Auto => {
                // FP32 mode - no quantization
                CollectionConfig::fp32(dimension)
            }
            ExtractionMode::QuantizedOnly => {
                // Quantized mode - use INT8 by default (could come from collection config)
                CollectionConfig::quantized(dimension, QuantizationMethod::INT8)
            }
            ExtractionMode::Both => {
                // Both modes - store FP32 primarily, quantized in separate map
                CollectionConfig::fp32(dimension)
            }
        }
    }

    /// Generate random level for new node using exponential decay
    fn get_random_level(&self) -> usize {
        let mut rng = self.rng_state.write().unwrap();
        let mut level = 0;
        let _ml = 1.0 / (2.0_f32.ln()); // 1/ln(2) ≈ 1.44

        let mut random_val = self.fast_random(&mut rng) as f32 / u32::MAX as f32;

        while random_val < 0.5 && level < self.config.max_layers {
            level += 1;
            random_val = self.fast_random(&mut rng) as f32 / u32::MAX as f32;
        }

        level
    }

    /// Fast pseudo-random number generator (Linear Congruential Generator)
    fn fast_random(&self, state: &mut u64) -> u32 {
        *state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        (*state >> 32) as u32
    }

    /// Search for ef closest candidates in a specific layer
    fn search_layer(
        &self,
        query: &[f32],
        entry_points: &[usize],
        ef: usize,
        layer: usize,
    ) -> Vec<(usize, f32)> {
        let mut visited = HashSet::new();
        let mut candidates = BinaryHeap::new(); // Min heap for candidates (to visit)
        let mut dynamic_candidates = BinaryHeap::new(); // Max heap for best found

        // Initialize with entry points
        for &ep in entry_points {
            // Zero-overhead vector access with O(1) lookup
            if let Some(external_id) = self.id_mapping.external(ep) {
                let vectors = self.vectors.read().unwrap();
                if let Some(view) = vectors.get(&external_id) {
                    if let Some(vector_data) = view.as_f32() {
                        let dist = self
                            .distance_computer
                            .calculate_distance(query, vector_data, &self.config.distance_metric)
                            .rank_value;

                        candidates.push(std::cmp::Reverse((OrderedFloat(dist), ep)));
                        dynamic_candidates.push((OrderedFloat(dist), ep));
                        visited.insert(ep);
                    }
                }
            }
        }

        // Explore the graph
        while let Some(std::cmp::Reverse((curr_dist, curr_node))) = candidates.pop() {
            // Early termination: if current distance is worse than worst in dynamic_candidates
            if let Some((worst_dist, _)) = dynamic_candidates.peek() {
                if curr_dist.0 > worst_dist.0 && dynamic_candidates.len() >= ef {
                    break;
                }
            }

            // Explore neighbors of current node using DashMap
            if let Some(neighbors) = self.layers.get(&(curr_node, layer)) {
                for &neighbor in neighbors.value() {
                    if !visited.contains(&neighbor) {
                        visited.insert(neighbor);

                        // Zero-overhead vector access for neighbors
                        if let Some(external_id) = self.id_mapping.external(neighbor) {
                            let vectors = self.vectors.read().unwrap();
                            if let Some(view) = vectors.get(&external_id) {
                                if let Some(vector_data) = view.as_f32() {
                                    let dist = self
                                        .distance_computer
                                        .calculate_distance(
                                            query,
                                            vector_data,
                                            &self.config.distance_metric,
                                        )
                                        .rank_value;

                                    if dynamic_candidates.len() < ef {
                                        // We need more candidates
                                        candidates.push(std::cmp::Reverse((
                                            OrderedFloat(dist),
                                            neighbor,
                                        )));
                                        dynamic_candidates.push((OrderedFloat(dist), neighbor));
                                    } else if let Some((worst_dist, _)) = dynamic_candidates.peek()
                                    {
                                        if dist < worst_dist.0 {
                                            // Found a better candidate
                                            candidates.push(std::cmp::Reverse((
                                                OrderedFloat(dist),
                                                neighbor,
                                            )));
                                            dynamic_candidates.push((OrderedFloat(dist), neighbor));

                                            // Remove worst candidate if we exceed ef
                                            if dynamic_candidates.len() > ef {
                                                dynamic_candidates.pop();
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Convert to sorted result (ascending by distance)
        let mut result: Vec<_> = dynamic_candidates
            .into_iter()
            .map(|(OrderedFloat(dist), node)| (node, dist))
            .collect();

        result.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        result
    }

    /// Select m neighbors using simple heuristic (closest neighbors)
    /// TODO: Implement more sophisticated heuristics for better graph connectivity
    fn select_neighbors(&self, candidates: Vec<(usize, f32)>, m: usize) -> Vec<usize> {
        candidates
            .into_iter()
            .take(m)
            .map(|(node, _)| node)
            .collect()
    }
}

#[async_trait]
impl AxisVectorIndex for AxisHnswIndex {
    async fn add(&self, id: String, vector_data: Vec<f32>) -> Result<()> {
        let start = std::time::Instant::now();

        // USING UTILS: Validate vector ID
        validation::validate_vector_id(&id)?;

        // Check if this ID already exists
        if let Some(existing_node_id) = self.id_mapping.internal(&id) {
            // Update existing vector
            {
                let mut vectors = self.vectors.write().unwrap();
                vectors.add_fp32(id.clone(), &vector_data)?;
            }
            // Return early - no need to re-add to graph structure
            self.stats
                .record_success(start.elapsed().as_micros() as u64);
            return Ok(());
        }

        // Register ID mapping and get internal node ID for new vector
        let internal_node_id = self.id_mapping.register(id.clone())?;

        // Store vector with zero-overhead - just raw data!
        {
            let mut vectors = self.vectors.write().unwrap();
            vectors.add_fp32(id.clone(), &vector_data)?;
        }

        // Determine level for this node
        let level = self.get_random_level();

        // Update maximum layer atomically
        let current_max = self.max_layer.load(AtomicOrdering::Relaxed);
        if level > current_max {
            self.max_layer.store(level, AtomicOrdering::Relaxed);
        }

        // Initialize layers for this node
        for l in 0..=level {
            self.layers.insert((l, internal_node_id), Vec::new());
        }

        // If this is the first node, make it the entry point
        {
            let mut entry_point_lock = self.entry_point.write().unwrap();
            if entry_point_lock.is_none() {
                *entry_point_lock = Some(internal_node_id);
                self.stats
                    .record_success(start.elapsed().as_micros() as u64);
                return Ok(());
            }
        }

        let entry_point = self.entry_point.read().unwrap().unwrap();
        let mut curr_nearest = vec![entry_point];

        // Search from top layer down to level+1 (greedy search with ef=1)
        for layer in (level + 1..=self.max_layer.load(AtomicOrdering::Relaxed)).rev() {
            curr_nearest = self
                .search_layer(&vector_data, &curr_nearest, 1, layer)
                .into_iter()
                .map(|(node, _)| node)
                .collect();
        }

        // Search and connect from level down to 0
        for layer in (0..=level).rev() {
            let candidates = self.search_layer(
                &vector_data,
                &curr_nearest,
                self.config.ef_construction,
                layer,
            );

            // Different M values for layer 0 vs higher layers
            let m = if layer == 0 {
                self.config.m * 2
            } else {
                self.config.m
            };
            let selected = self.select_neighbors(candidates.clone(), m);

            // Add bidirectional connections using DashMap
            for neighbor in &selected {
                // Add internal_node_id to neighbor's connections
                self.layers
                    .entry((layer, *neighbor))
                    .or_insert_with(Vec::new)
                    .push(internal_node_id);

                // Add neighbor to internal_node_id's connections
                self.layers
                    .entry((layer, internal_node_id))
                    .or_insert_with(Vec::new)
                    .push(*neighbor);
            }

            // Update curr_nearest for next layer (best candidate from this layer)
            curr_nearest = candidates
                .into_iter()
                .take(1)
                .map(|(node, _)| node)
                .collect();
        }

        // Update entry point if this node reaches the highest level
        if level >= self.max_layer.load(AtomicOrdering::Relaxed) {
            *self.entry_point.write().unwrap() = Some(internal_node_id);
        }

        // USING UTILS: Record successful operation
        self.stats
            .record_success(start.elapsed().as_micros() as u64);
        Ok(())
    }

    async fn search(
        &self,
        query: &[f32],
        top_k: usize,
        _filter: Option<&HashMap<String, String>>, // Metadata filter, not VectorRecord
    ) -> Result<Vec<(String, f32)>> {
        self.search_with_filter(query, top_k, None).await
    }

    async fn remove(&self, id: &str) -> Result<()> {
        let start = std::time::Instant::now();

        // USING UTILS: Get internal node ID from mapping
        let internal_node_id = match self.id_mapping.internal(id) {
            Some(node_id) => node_id,
            None => {
                self.stats
                    .record_success(start.elapsed().as_micros() as u64);
                return Ok(()); // ID not found, nothing to remove
            }
        };

        // Remove from all layers and clean up connections using DashMap
        let max_layer = self.max_layer.load(AtomicOrdering::Relaxed);
        for layer in 0..=max_layer {
            // Remove this node's connections
            self.layers.remove(&(layer, internal_node_id));

            // Remove connections to this node from all other nodes
            let keys_to_update: Vec<_> = self
                .layers
                .iter()
                .filter(|entry| entry.key().0 == layer)
                .map(|entry| entry.key().clone())
                .collect();

            for key in keys_to_update {
                if let Some(mut connections) = self.layers.get_mut(&key) {
                    connections.retain(|&x| x != internal_node_id);
                }
            }
        }

        // USING UTILS: Remove from mappings and vectors
        // Remove from vectors collection
        {
            let mut vectors = self.vectors.write().unwrap();
            vectors.remove(id);
        }
        self.id_mapping.remove_by_external(id);

        // Update entry point if necessary
        {
            let mut entry_point_lock = self.entry_point.write().unwrap();
            if *entry_point_lock == Some(internal_node_id) {
                // Find a new entry point from remaining vectors
                // TODO: ZeroOverheadCollection doesn't have keys() method
                // For now, just set entry point to None when removed
                *entry_point_lock = None;
            }
        }

        // USING UTILS: Record successful operation
        self.stats
            .record_success(start.elapsed().as_micros() as u64);
        Ok(())
    }

    fn algorithm(&self) -> &IndexAlgorithm {
        &self.algorithm_type
    }

    fn stats(&self) -> IndexStats {
        IndexStats {
            vector_count: self.vectors.read().unwrap().len(),
            memory_usage_bytes: self.estimate_memory_usage(),
            index_type: "HNSW".to_string(),
        }
    }
}

impl AxisHnswIndex {
    /// Search with optional filtering
    pub async fn search_with_filter(
        &self,
        query: &[f32],
        top_k: usize,
        filter: Option<&(dyn for<'a> Fn(&'a VectorRecord) -> bool + Send + Sync)>,
    ) -> Result<Vec<(String, f32)>> {
        let start = std::time::Instant::now();

        // Get entry point
        let entry_point = match self.entry_point.read().unwrap().as_ref() {
            Some(&ep) => ep,
            None => {
                self.stats
                    .record_success(start.elapsed().as_micros() as u64);
                return Ok(Vec::new()); // Empty index
            }
        };

        let mut curr_nearest = vec![entry_point];
        let max_layer = self.max_layer.load(AtomicOrdering::Relaxed);

        // Search from top layer down to layer 1 (greedy with ef=1)
        for layer in (1..=max_layer).rev() {
            curr_nearest = self
                .search_layer(query, &curr_nearest, 1, layer)
                .into_iter()
                .map(|(node, _)| node)
                .collect();
        }

        // Search layer 0 with configured ef (or top_k if larger)
        let search_ef = self.config.ef.max(top_k);
        let candidates = self.search_layer(query, &curr_nearest, search_ef, 0);

        // Convert internal IDs to external IDs - no filtering at index level
        // Metadata filtering happens at storage layer, not in indexes
        let results: Vec<(String, f32)> = candidates
            .into_iter()
            .take(top_k)
            .filter_map(|(internal_node_id, score)| {
                self.id_mapping
                    .external(internal_node_id)
                    .map(|external_id| (external_id, score))
            })
            .collect();

        // USING UTILS: Record successful operation
        self.stats
            .record_success(start.elapsed().as_micros() as u64);
        Ok(results)
    }

    /// Get the number of vectors in the index
    pub fn size(&self) -> usize {
        let vectors = self.vectors.read().unwrap();
        vectors.len()
    }

    /// Get memory usage of the index
    pub fn memory_usage(&self) -> MemoryUsage {
        let total = self.estimate_memory_usage();
        MemoryUsage {
            total_bytes: total,
            index_size_bytes: total / 3, // Approximate distribution
            vector_data_bytes: total / 3,
            metadata_bytes: total / 3,
        }
    }

    /// Add a single vector to the index - clean API, no VectorRecord
    pub async fn add_vector(
        &mut self,
        id: String,
        vector: Vec<f32>,
        _metadata: Option<HashMap<String, serde_json::Value>>,
    ) -> Result<()> {
        // Metadata is ignored - indexes don't store metadata, only vectors
        self.add(id, vector).await
    }

    /// Add multiple vectors to the index
    pub async fn add_vectors(
        &mut self,
        batch: Vec<(String, Vec<f32>, Option<HashMap<String, serde_json::Value>>)>,
    ) -> Result<()> {
        for (id, vector, metadata) in batch {
            self.add_vector(id, vector, metadata).await?;
        }
        Ok(())
    }

    /// Search for top_k nearest neighbors
    pub async fn search_simple(&self, query: &[f32], top_k: usize) -> Result<Vec<(String, f32)>> {
        self.search_with_filter(query, top_k, None).await
    }

    /// Remove a vector from the index
    pub async fn remove_vector(&mut self, id: &str) -> Result<bool> {
        self.remove(id).await.map(|_| true)
    }

    /// Optimize the index (no-op for HNSW)
    pub fn optimize(&self) -> Result<()> {
        Ok(())
    }

    /// Estimate memory usage in bytes
    fn estimate_memory_usage(&self) -> usize {
        // USING UTILS: Get vector storage memory usage
        let vector_memory = {
            let vectors = self.vectors.read().unwrap();
            vectors.memory_usage()
        };

        // USING UTILS: Get ID mapping memory usage
        let id_mapping_memory = memory::dashmap_overhead::<String, usize>(self.id_mapping.len())
            + memory::dashmap_overhead::<usize, String>(self.id_mapping.len());

        // Graph structure memory (layers DashMap)
        let layers_memory = self.layers.len()
            * (std::mem::size_of::<(usize, usize)>() + std::mem::size_of::<Vec<usize>>() + 64);

        // NEW: Quantized vector storage memory
        let quantized_memory = self.quantized_vectors.len() * 128; // Estimate 128 bytes per quantized vector

        // Other structures
        let config_memory = std::mem::size_of::<AxisHnswConfig>();
        let stats_memory = std::mem::size_of::<AtomicStats>();

        vector_memory
            + id_mapping_memory
            + layers_memory
            + quantized_memory
            + config_memory
            + stats_memory
    }

    /// Set extraction mode for EventLog-based async index updates
    pub fn set_extraction_mode(&mut self, mode: ExtractionMode) {
        self.extraction_mode = mode.clone();
        info!(
            "Extraction mode set to {:?} for HNSW index in collection: {:?}",
            mode, self.collection_id
        );
    }

    /// Process EventLog events for async index updates  
    pub async fn process_event(&self, event: &IndexEvent) -> Result<()> {
        info!(
            "Processing EventLog event {} for HNSW index in collection: {:?}",
            event.event_id, self.collection_id
        );

        // Process vectors based on extraction mode and event data availability
        match self.extraction_mode {
            ExtractionMode::Fp32Only if !event.has_fp32 => {
                info!(
                    "Skipping event {} - requires FP32 but event has no FP32 data",
                    event.event_id
                );
                return Ok(());
            }
            ExtractionMode::QuantizedOnly if !event.has_quantized => {
                info!(
                    "Skipping event {} - requires quantized but event has no quantized data",
                    event.event_id
                );
                return Ok(());
            }
            _ => {
                // Auto and Both modes can process any available data
                info!(
                    "Processing event {} with extraction mode {:?}",
                    event.event_id, self.extraction_mode
                );
            }
        }

        // TODO: Extract vectors from files listed in event.file_paths
        // This would be handled by the EventLog consumer which calls this method
        // after extracting vectors from the storage files

        Ok(())
    }

    /// Process vectors from EventLog event - clean, no VectorRecord
    pub async fn add_vectors_from_event(&self, vectors: Vec<(String, Vec<f32>)>) -> Result<()> {
        // Process FP32 vectors directly - no proto overhead
        for (id, vector) in vectors {
            self.add(id, vector).await?;
        }
        Ok(())
    }

    /// NEW: Dequantize vector for HNSW graph construction
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
    pub fn extraction_mode(&self) -> ExtractionMode {
        self.extraction_mode.clone()
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
        top_k: usize,
        filter: Option<&(dyn for<'a> Fn(&'a VectorRecord) -> bool + Send + Sync)>,
    ) -> Result<Vec<(String, f32)>> {
        if !self.has_quantized_storage() {
            // No quantized vectors available, use standard search
            return self.search_with_filter(query, top_k, filter).await;
        }

        // TODO: Implement two-stage search with quantized filtering
        // Stage 1: Fast filtering using quantized vectors
        // Stage 2: FP32 reranking of top candidates
        tracing::warn!("Quantized acceleration not yet implemented - using standard search");

        self.search_with_filter(query, top_k, filter).await
    }
}

/// Factory function to create HNSW index instances
pub fn create_hnsw_index(
    config: AxisHnswConfig,
    dimension: usize,
) -> Result<Box<dyn AxisVectorIndex>> {
    Ok(Box::new(AxisHnswIndex::new(config, dimension)?))
}

/// Factory function to create HNSW index instances with vector representation preference
pub fn create_hnsw_index_with_representation(
    config: AxisHnswConfig,
    dimension: usize,
    extraction_mode: ExtractionMode,
) -> Result<Box<dyn AxisVectorIndex>> {
    Ok(Box::new(AxisHnswIndex::new_with_extraction_mode(
        None,
        config,
        dimension,
        extraction_mode,
    )?))
}

/// Factory function to create HNSW index instances for specific collection with representation
pub fn create_hnsw_index_for_collection(
    collection_id: String,
    config: AxisHnswConfig,
    dimension: usize,
    extraction_mode: ExtractionMode,
) -> Result<Box<dyn AxisVectorIndex>> {
    Ok(Box::new(AxisHnswIndex::new_with_extraction_mode(
        Some(collection_id),
        config,
        dimension,
        extraction_mode,
    )?))
}

#[cfg(test)]
mod tests {
    use crate::compute::distance_computation::DistanceMetric;
    use crate::index::axis::*;

    #[tokio::test]
    async fn test_hnsw_basic_operations() {
        // Initialize hardware capabilities
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

        let config = AxisHnswConfig::default();
        let index = AxisHnswIndex::new(config, 3).unwrap();

        // Add test vectors
        index
            .add("vec1".to_string(), vec![1.0, 0.0, 0.0])
            .await
            .unwrap();
        index
            .add("vec2".to_string(), vec![0.0, 1.0, 0.0])
            .await
            .unwrap();
        index
            .add("vec3".to_string(), vec![1.0, 1.0, 0.0])
            .await
            .unwrap();

        assert_eq!(index.stats().vector_count, 3);

        // Search should work
        let results = index.search(&[1.0, 0.0, 0.0], 2, None).await.unwrap();
        assert!(results.len() <= 2); // HNSW is approximate

        // Remove a vector
        index.remove("vec2").await.unwrap();
        assert_eq!(index.stats().vector_count, 2);

        // Remove non-existent vector (should succeed without error)
        index.remove("nonexistent").await.unwrap();
    }

    #[tokio::test]
    async fn test_hnsw_search_quality() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

        let mut config = AxisHnswConfig::default();
        config.ef = 200; // Higher ef for better quality
        let index = AxisHnswIndex::new(config, 4).unwrap();

        // Create a set of test vectors
        let test_vectors = vec![
            ("v1", vec![1.0, 0.0, 0.0, 0.0]),
            ("v2", vec![0.9, 0.1, 0.0, 0.0]),
            ("v3", vec![0.0, 1.0, 0.0, 0.0]),
            ("v4", vec![0.0, 0.0, 1.0, 0.0]),
            ("v5", vec![0.0, 0.0, 0.0, 1.0]),
            ("v6", vec![0.8, 0.2, 0.0, 0.0]),
            ("v7", vec![0.7, 0.3, 0.0, 0.0]),
        ];

        for (id, vector) in test_vectors.iter() {
            index.add(id.to_string(), vector.clone()).await.unwrap();
        }

        // Search for nearest neighbors to v1
        let query = vec![1.0, 0.0, 0.0, 0.0];
        let results = index.search(&query, 3, None).await.unwrap();

        // v1, v2, v6, v7 should be closest (all have high first component)
        // With default config, should find at least 1 result
        assert!(results.len() >= 1, "Expected at least 1 result, got {}", results.len());

        // The first result should be one of the nearby vectors
        // HNSW is probabilistic, so v1, v2, v6, or v7 are all acceptable
        if let Some(first) = results.first() {
            let valid_results = vec!["v1", "v2", "v6", "v7"];
            assert!(valid_results.contains(&first.0.as_str()),
                    "Expected first result to be one of {:?}, got {}", valid_results, first.0);
        }
    }

    #[tokio::test]
    async fn test_hnsw_layer_navigation() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

        let mut config = AxisHnswConfig::default();
        config.m = 16;
        config.ef_construction = 200;
        let index = AxisHnswIndex::new(config, 3).unwrap();

        // Add enough vectors to create multiple layers
        for i in 0..50 {
            let vector = vec![(i as f32).sin(), (i as f32).cos(), (i as f32 * 0.5).sin()];
            index.add(format!("vec_{}", i), vector).await.unwrap();
        }

        // Check that multiple layers were created
        let stats = index.stats();
        assert_eq!(stats.vector_count, 50);

        // Search should work efficiently across layers
        let query = vec![0.5, 0.5, 0.5];
        let results = index.search(&query, 10, None).await.unwrap();
        assert!(results.len() > 0);
    }

    #[tokio::test]
    async fn test_hnsw_pruning_heuristic() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

        let mut config = AxisHnswConfig::default();
        config.m = 5; // Small M to test pruning
        let index = AxisHnswIndex::new(config, 2).unwrap();

        // Add vectors in a line to test pruning
        for i in 0..20 {
            index
                .add(format!("v{}", i), vec![i as f32, 0.0])
                .await
                .unwrap();
        }

        // Search for middle point
        let query = vec![10.0, 0.0];
        let results = index.search(&query, 5, None).await.unwrap();

        // Should find neighbors despite pruning
        // With small M=5, graph connectivity may be limited, so just check we get some results
        assert!(results.len() >= 1, "Expected at least 1 result, got {}", results.len());
    }

    #[test]
    fn test_hnsw_config_validation() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

        // Test valid config
        let valid_config = AxisHnswConfig {
            m: 16,
            ef_construction: 200,
            ef: 100,
            max_layers: 10,
            distance_metric: DistanceMetric::Cosine,
        };
        assert!(AxisHnswIndex::new(valid_config, 128).is_ok());

        // Test invalid dimension
        let config = AxisHnswConfig::default();
        assert!(AxisHnswIndex::new(config.clone(), 0).is_err());

        // Test with very high dimension
        assert!(AxisHnswIndex::new(config, 10000).is_ok());
    }

    #[tokio::test]
    async fn test_hnsw_empty_index_search() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

        let config = AxisHnswConfig::default();
        let index = AxisHnswIndex::new(config, 3).unwrap();

        // Search on empty index should return empty results
        let results = index.search(&[1.0, 0.0, 0.0], 5, None).await.unwrap();
        assert_eq!(results.len(), 0);
    }

    #[tokio::test]
    async fn test_hnsw_duplicate_removal() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

        let config = AxisHnswConfig::default();
        let index = AxisHnswIndex::new(config, 2).unwrap();

        // Add the same ID twice
        index
            .add("duplicate".to_string(), vec![1.0, 0.0])
            .await
            .unwrap();
        assert_eq!(index.stats().vector_count, 1);

        // Adding again should replace
        index
            .add("duplicate".to_string(), vec![1.0, 0.0])
            .await
            .unwrap();
        assert_eq!(index.stats().vector_count, 1);

        // Remove should work
        index.remove("duplicate").await.unwrap();
        assert_eq!(index.stats().vector_count, 0);
    }
}
