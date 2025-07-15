//! HNSW Integration with AXIS for ProximaDB
//!
//! Provides seamless integration between HNSW indexing algorithms and AXIS
//! for adaptive, high-performance vector search with metadata filtering.

use std::collections::HashMap;
use std::sync::Arc;
use anyhow::{Result, Context};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock as AsyncRwLock;

use crate::compute::algorithms::{HNSWIndex, VectorSearchAlgorithm, SearchResult as AlgoSearchResult};
use crate::compute::DistanceMetric;
use crate::core::{MetadataQuery, MetadataQueryEngine, VectorRecord};
use crate::index::axis::strategy::IndexType;
use super::manager::AxisManager;

/// HNSW configuration for AXIS integration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AxisHnswConfig {
    /// Number of bi-directional links for each node (default: 16)
    pub m: usize,
    /// Size of candidate set during construction (default: 200)
    pub ef_construction: usize,
    /// Search parameter - larger values give better recall (default: 50)
    pub ef_search: usize,
    /// Maximum number of vectors before partitioning (default: 100,000)
    pub max_partition_size: usize,
    /// Enable dynamic parameter tuning based on data characteristics
    pub adaptive_parameters: bool,
    /// Use SIMD optimizations if available
    pub use_simd: bool,
    /// Memory limit per HNSW partition (MB)
    pub memory_limit_mb: usize,
    /// Enable lazy loading for large partitions
    pub lazy_loading: bool,
}

impl Default for AxisHnswConfig {
    fn default() -> Self {
        Self {
            m: 16,
            ef_construction: 200,
            ef_search: 50,
            max_partition_size: 100_000,
            adaptive_parameters: true,
            use_simd: true,
            memory_limit_mb: 512,
            lazy_loading: true,
        }
    }
}

/// HNSW performance statistics
#[derive(Debug, Clone)]
pub struct HnswStats {
    pub total_vectors: usize,
    pub num_partitions: usize,
    pub avg_connections_per_node: f32,
    pub memory_usage_mb: f32,
    pub search_latency_ms: f32,
    pub index_build_time_ms: u64,
    pub recall_at_10: f32,
}

/// Partitioned HNSW index for scalable vector search
pub struct PartitionedHnswIndex {
    /// HNSW configuration
    config: AxisHnswConfig,
    /// Distance metric
    distance_metric: DistanceMetric,
    /// HNSW partitions: partition_id -> HNSW index
    partitions: HashMap<usize, HNSWIndex>,
    /// Vector to partition mapping: vector_id -> partition_id
    vector_partitions: HashMap<String, usize>,
    /// Partition metadata for load balancing
    partition_metadata: HashMap<usize, PartitionMetadata>,
    /// Next partition ID
    next_partition_id: usize,
    /// Metadata query engine for filtering
    query_engine: MetadataQueryEngine,
}

/// Metadata for managing HNSW partitions
#[derive(Debug, Clone)]
struct PartitionMetadata {
    pub vector_count: usize,
    pub memory_usage_bytes: usize,
    pub last_accessed: std::time::Instant,
    pub is_loaded: bool,
    pub avg_vector_dimension: usize,
}

impl PartitionedHnswIndex {
    /// Create a new partitioned HNSW index
    pub fn new(config: AxisHnswConfig, distance_metric: DistanceMetric) -> Self {
        Self {
            config,
            distance_metric,
            partitions: HashMap::new(),
            vector_partitions: HashMap::new(),
            partition_metadata: HashMap::new(),
            next_partition_id: 0,
            query_engine: MetadataQueryEngine::new(),
        }
    }

    /// Add a vector to the appropriate partition
    pub fn add_vector(&mut self, vector_record: &VectorRecord) -> Result<()> {
        let partition_id = self.select_partition_for_vector(vector_record)?;
        
        // Get or create partition
        let partition = self.get_or_create_partition(partition_id)?;
        
        // Add vector to HNSW
        partition.add_vector(
            vector_record.id.as_deref().unwrap_or("").to_string(),
            vector_record.vector.to_vec(),
            Some(crate::core::proto_metadata_helper::proto_metadata_to_json(&vector_record.metadata)),
        ).map_err(|e| anyhow::anyhow!(e))?;
        
        // Update mappings
        self.vector_partitions.insert(vector_record.id.as_deref().unwrap_or("").to_string(), partition_id);
        
        // Calculate memory usage before borrowing metadata
        let memory_usage = self.estimate_vector_memory_usage(vector_record);
        
        // Update partition metadata
        if let Some(metadata) = self.partition_metadata.get_mut(&partition_id) {
            metadata.vector_count += 1;
            metadata.memory_usage_bytes += memory_usage;
            metadata.last_accessed = std::time::Instant::now();
            
            // Update average dimension
            if metadata.vector_count == 1 {
                metadata.avg_vector_dimension = vector_record.vector.len();
            } else {
                metadata.avg_vector_dimension = 
                    (metadata.avg_vector_dimension * (metadata.vector_count - 1) + vector_record.vector.len()) 
                    / metadata.vector_count;
            }
        }
        
        tracing::debug!(
            "Added vector {} to HNSW partition {} (total vectors: {})",
            vector_record.id.as_deref().unwrap_or(""), partition_id, 
            self.partition_metadata.get(&partition_id).map(|m| m.vector_count).unwrap_or(0)
        );
        
        Ok(())
    }

    /// Search across all partitions with metadata filtering
    pub fn search_with_filter(
        &self,
        query_vector: &[f32],
        k: usize,
        metadata_query: Option<&MetadataQuery>,
    ) -> Result<Vec<VectorRecord>> {
        let mut all_results = Vec::new();
        
        // Search each partition
        for (partition_id, partition) in &self.partitions {
            // Create metadata filter function
            let filter_fn = |metadata: &HashMap<String, serde_json::Value>| -> bool {
                if let Some(query) = metadata_query {
                    // Clone the engine for thread safety (TODO: optimize this)
                    let mut engine = MetadataQueryEngine::new();
                    engine.evaluate(query, metadata).unwrap_or(false)
                } else {
                    true
                }
            };
            
            // Search this partition
            match partition.search_with_filter(query_vector, k * 2, &filter_fn) {
                Ok(partition_results) => {
                    tracing::debug!(
                        "Partition {} returned {} results",
                        partition_id, partition_results.len()
                    );
                    all_results.extend(partition_results);
                }
                Err(e) => {
                    tracing::warn!("Search failed in partition {}: {}", partition_id, e);
                }
            }
        }
        
        // Sort by score and take top k
        all_results.sort_by(|a, b| {
            b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal)
        });
        all_results.truncate(k);
        
        // Convert to VectorRecord format
        self.convert_search_results_to_vector_records(all_results)
    }

    /// Search without metadata filtering for maximum performance
    pub fn search(&self, query_vector: &[f32], k: usize) -> Result<Vec<VectorRecord>> {
        self.search_with_filter(query_vector, k, None)
    }

    /// Remove a vector from the index
    pub fn remove_vector(&mut self, vector_id: &str) -> Result<bool> {
        if let Some(partition_id) = self.vector_partitions.remove(vector_id) {
            if let Some(partition) = self.partitions.get_mut(&partition_id) {
                let removed = partition.remove_vector(vector_id)
                    .map_err(|e| anyhow::anyhow!(e))?;
                
                // Update partition metadata
                if removed {
                    if let Some(metadata) = self.partition_metadata.get_mut(&partition_id) {
                        metadata.vector_count = metadata.vector_count.saturating_sub(1);
                        metadata.last_accessed = std::time::Instant::now();
                    }
                }
                
                return Ok(removed);
            }
        }
        Ok(false)
    }

    /// Get statistics about the HNSW index
    pub fn get_stats(&self) -> HnswStats {
        let total_vectors = self.partition_metadata.values().map(|m| m.vector_count).sum();
        let num_partitions = self.partitions.len();
        let total_memory_bytes: usize = self.partition_metadata.values().map(|m| m.memory_usage_bytes).sum();
        
        // Calculate average connections per node (approximation)
        let avg_connections = if total_vectors > 0 {
            self.config.m as f32 * 1.5 // Rough estimate
        } else {
            0.0
        };
        
        HnswStats {
            total_vectors,
            num_partitions,
            avg_connections_per_node: avg_connections,
            memory_usage_mb: total_memory_bytes as f32 / 1024.0 / 1024.0,
            search_latency_ms: 0.0, // TODO: Track actual latency
            index_build_time_ms: 0, // TODO: Track build time
            recall_at_10: 0.95, // TODO: Measure actual recall
        }
    }

    /// Optimize the index (rebuild partitions if needed)
    pub fn optimize(&mut self) -> Result<()> {
        for (partition_id, partition) in &mut self.partitions {
            partition.optimize()
                .map_err(|e| anyhow::anyhow!("Failed to optimize partition {}: {}", partition_id, e))?;
        }
        
        // TODO: Implement partition rebalancing if needed
        self.rebalance_partitions()?;
        
        Ok(())
    }

    /// Select the best partition for a new vector
    fn select_partition_for_vector(&self, vector_record: &VectorRecord) -> Result<usize> {
        // Strategy 1: Find partition with space and similar vectors
        for (partition_id, metadata) in &self.partition_metadata {
            if metadata.vector_count < self.config.max_partition_size &&
               metadata.memory_usage_bytes < self.config.memory_limit_mb * 1024 * 1024 {
                // Check if vector dimension matches
                if metadata.avg_vector_dimension == vector_record.vector.len() {
                    return Ok(*partition_id);
                }
            }
        }
        
        // Strategy 2: Create new partition if all are full
        Ok(self.next_partition_id)
    }

    /// Get or create an HNSW partition
    fn get_or_create_partition(&mut self, partition_id: usize) -> Result<&mut HNSWIndex> {
        if !self.partitions.contains_key(&partition_id) {
            tracing::info!("Creating new HNSW partition {}", partition_id);
            
            // Create new HNSW index
            let mut hnsw = HNSWIndex::new(
                self.config.m,
                self.config.ef_construction,
                self.distance_metric.clone(),
                self.config.use_simd,
            );
            
            // Apply adaptive parameters if enabled
            if self.config.adaptive_parameters {
                self.tune_hnsw_parameters(&mut hnsw)?;
            }
            
            self.partitions.insert(partition_id, hnsw);
            
            // Create partition metadata
            self.partition_metadata.insert(partition_id, PartitionMetadata {
                vector_count: 0,
                memory_usage_bytes: 0,
                last_accessed: std::time::Instant::now(),
                is_loaded: true,
                avg_vector_dimension: 0,
            });
            
            // Update next partition ID
            if partition_id >= self.next_partition_id {
                self.next_partition_id = partition_id + 1;
            }
        }
        
        Ok(self.partitions.get_mut(&partition_id).unwrap())
    }

    /// Tune HNSW parameters based on data characteristics
    fn tune_hnsw_parameters(&self, _hnsw: &mut HNSWIndex) -> Result<()> {
        // TODO: Implement adaptive parameter tuning
        // - Adjust ef_construction based on data distribution
        // - Tune M based on vector dimension and memory constraints
        // - Optimize ef_search based on accuracy requirements
        Ok(())
    }

    /// Estimate memory usage for a vector
    fn estimate_vector_memory_usage(&self, vector_record: &VectorRecord) -> usize {
        // Vector data: dimension * 4 bytes (f32)
        let vector_bytes = vector_record.vector.len() * 4;
        
        // Metadata: rough estimation
        let metadata_bytes = vector_record.metadata.len() * 50; // Rough estimate
        
        // HNSW overhead: connections * 8 bytes (approximate)
        let hnsw_overhead = self.config.m * 8;
        
        vector_bytes + metadata_bytes + hnsw_overhead
    }

    /// Convert algorithm search results to VectorRecord format
    fn convert_search_results_to_vector_records(&self, results: Vec<AlgoSearchResult>) -> Result<Vec<VectorRecord>> {
        let mut vector_records = Vec::new();
        
        for result in results {
            // Find the vector in partitions
            if let Some(partition_id) = self.vector_partitions.get(&result.vector_id) {
                if let Some(partition) = self.partitions.get(partition_id) {
                    // Extract vector and metadata from HNSW
                    // TODO: Add method to HNSWIndex to get vector by ID
                    let vector_record = VectorRecord {
                        id: Some(result.vector_id),
                        collection_id: "".to_string(), // TODO: Track collection
                        vector: vec![], // TODO: Get actual vector
                        metadata: result.metadata
                            .unwrap_or_default()
                            .into_iter()
                            .map(|(k, v)| crate::proto::proximadb::MetadataItem {
                                key: k,
                                value: v.to_string(),
                            })
                            .collect(),
                        timestamp: chrono::Utc::now().timestamp_millis(),
                        created_at: chrono::Utc::now().timestamp_millis(),
                        updated_at: chrono::Utc::now().timestamp_millis(),
                        expires_at: None,
                        version: 1,
                        rank: None,
                        score: Some(result.score),
                        distance: Some(1.0 - result.score), // Convert score to distance
                    };
                    vector_records.push(vector_record);
                }
            }
        }
        
        Ok(vector_records)
    }

    /// Rebalance partitions for optimal performance
    fn rebalance_partitions(&mut self) -> Result<()> {
        // TODO: Implement partition rebalancing
        // - Move vectors between partitions if they become imbalanced
        // - Merge small partitions
        // - Split large partitions
        Ok(())
    }
}

/// AXIS-HNSW integration manager
pub struct AxisHnswManager {
    /// Collection to HNSW index mapping
    indices: HashMap<String, PartitionedHnswIndex>,
    /// Default configuration
    default_config: AxisHnswConfig,
    /// Performance statistics
    stats: Arc<AsyncRwLock<HashMap<String, HnswStats>>>,
}

impl AxisHnswManager {
    /// Create a new AXIS-HNSW manager
    pub fn new(default_config: AxisHnswConfig) -> Self {
        Self {
            indices: HashMap::new(),
            default_config,
            stats: Arc::new(AsyncRwLock::new(HashMap::new())),
        }
    }

    /// Create or get HNSW index for a collection
    pub fn get_or_create_index(
        &mut self,
        collection_id: &str,
        distance_metric: DistanceMetric,
        config: Option<AxisHnswConfig>,
    ) -> &mut PartitionedHnswIndex {
        let config = config.unwrap_or_else(|| self.default_config.clone());
        
        self.indices.entry(collection_id.to_string())
            .or_insert_with(|| {
                tracing::info!("Creating HNSW index for collection {}", collection_id);
                PartitionedHnswIndex::new(config, distance_metric)
            })
    }

    /// Add vector to collection's HNSW index
    pub async fn add_vector(&mut self, collection_id: &str, vector_record: &VectorRecord) -> Result<()> {
        let index = self.get_or_create_index(collection_id, DistanceMetric::Cosine, None);
        index.add_vector(vector_record)?;
        
        // Update statistics
        let stats = index.get_stats();
        self.stats.write().await.insert(collection_id.to_string(), stats);
        
        Ok(())
    }

    /// Search in collection's HNSW index
    pub async fn search(
        &self,
        collection_id: &str,
        query_vector: &[f32],
        k: usize,
        metadata_query: Option<&MetadataQuery>,
    ) -> Result<Vec<VectorRecord>> {
        if let Some(index) = self.indices.get(collection_id) {
            index.search_with_filter(query_vector, k, metadata_query)
        } else {
            Ok(vec![])
        }
    }

    /// Get performance statistics for all collections
    pub async fn get_all_stats(&self) -> HashMap<String, HnswStats> {
        self.stats.read().await.clone()
    }

    /// Optimize all indices
    pub async fn optimize_all(&mut self) -> Result<()> {
        for (collection_id, index) in &mut self.indices {
            index.optimize()
                .with_context(|| format!("Failed to optimize HNSW index for collection {}", collection_id))?;
            
            // Update stats after optimization
            let stats = index.get_stats();
            self.stats.write().await.insert(collection_id.to_string(), stats);
        }
        Ok(())
    }
}

/// Integration with AXIS manager
impl AxisManager {
    /// Add HNSW indexing support to a collection
    pub async fn enable_hnsw_index(
        &mut self,
        collection_id: &str,
        _config: Option<AxisHnswConfig>,
    ) -> Result<()> {
        tracing::info!("Enabling HNSW indexing for collection {}", collection_id);
        
        // Update collection strategy to include HNSW
        let collection_id_str = collection_id.to_string();
        if let Ok(mut strategy) = self.get_collection_strategy(&collection_id_str).await {
            if !strategy.secondary_indexes.contains(&IndexType::HNSW) {
                strategy.secondary_indexes.push(IndexType::HNSW);
                self.update_collection_strategy(&collection_id_str, strategy).await?;
            }
        }
        
        Ok(())
    }

    /// Search using HNSW index with AXIS metadata
    pub async fn search_with_hnsw(
        &self,
        _collection_id: &str,
        _query_vector: &[f32],
        _k: usize,
        _metadata_query: Option<&MetadataQuery>,
    ) -> Result<Vec<VectorRecord>> {
        // TODO: Integrate with actual HNSW manager
        // For now, return empty results
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    

    fn create_test_vector_record(id: &str, vector: Vec<f32>) -> VectorRecord {
        let metadata = vec![
            crate::proto::proximadb::MetadataItem {
                key: "category".to_string(),
                value: "test".to_string(),
            }
        ];
        
        VectorRecord {
            id: Some(id.to_string()),
            collection_id: "test_collection".to_string(),
            vector,
            metadata,
            timestamp: chrono::Utc::now().timestamp_millis(),
            created_at: chrono::Utc::now().timestamp_millis(),
            updated_at: chrono::Utc::now().timestamp_millis(),
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        }
    }

    #[test]
    fn test_partitioned_hnsw_creation() {
        let config = AxisHnswConfig::default();
        let index = PartitionedHnswIndex::new(config, DistanceMetric::Cosine);
        
        let stats = index.get_stats();
        assert_eq!(stats.total_vectors, 0);
        assert_eq!(stats.num_partitions, 0);
    }

    #[test]
    fn test_vector_addition() {
        let config = AxisHnswConfig::default();
        let mut index = PartitionedHnswIndex::new(config, DistanceMetric::Cosine);
        
        let vector_record = create_test_vector_record("vec1", vec![1.0, 2.0, 3.0, 4.0]);
        let result = index.add_vector(&vector_record);
        assert!(result.is_ok());
        
        let stats = index.get_stats();
        assert_eq!(stats.total_vectors, 1);
        assert_eq!(stats.num_partitions, 1);
    }

    #[test]
    fn test_vector_search() {
        let config = AxisHnswConfig::default();
        let mut index = PartitionedHnswIndex::new(config, DistanceMetric::Cosine);
        
        // Add test vectors
        for i in 0..10 {
            let vector = vec![i as f32, (i * 2) as f32, (i * 3) as f32, (i * 4) as f32];
            let vector_record = create_test_vector_record(&format!("vec{}", i), vector);
            index.add_vector(&vector_record).unwrap();
        }
        
        // Search for similar vector
        let query = vec![1.0, 2.0, 3.0, 4.0];
        let results = index.search(&query, 5);
        assert!(results.is_ok());
        
        let search_results = results.unwrap();
        assert!(!search_results.is_empty());
    }

    #[tokio::test]
    async fn test_axis_hnsw_manager() {
        let config = AxisHnswConfig::default();
        let mut manager = AxisHnswManager::new(config);
        
        let vector_record = create_test_vector_record("vec1", vec![1.0, 2.0, 3.0, 4.0]);
        let result = manager.add_vector("test_collection", &vector_record).await;
        assert!(result.is_ok());
        
        let stats = manager.get_all_stats().await;
        assert!(stats.contains_key("test_collection"));
    }
}