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

//! Examples showing how to refactor existing index implementations to use common utilities
//! 
//! This demonstrates the before/after patterns for using the utils module.

use super::*;
use crate::compute::distance_computation::DistanceMetric;
use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use crate::index::axis::types::IndexAlgorithm;
use dashmap::DashMap;
use std::sync::Arc;

/// Example: How IVF Index would look using common utilities
/// 
/// BEFORE: Custom DashMap implementations with duplicated patterns
/// AFTER: Reusable ConcurrentVectorStore and AtomicStats
pub struct RefactoredIvfIndex {
    /// Configuration
    config: IvfConfig,
    
    /// USING UTILS: Standardized vector storage
    vectors: ConcurrentVectorStore,
    
    /// USING UTILS: Standardized statistics tracking
    stats: AtomicStats,
    
    /// Cluster centroids (IVF-specific)
    centroids: Vec<Vec<f32>>,
    
    /// Inverted lists (IVF-specific, but could be abstracted)
    inverted_lists: DashMap<usize, Vec<String>>,
    
    /// Distance computation
    distance_compute: UnifiedDistanceCompute,
    
    /// Whether the index has been trained
    trained: bool,
    
    /// The algorithm specification
    algorithm: IndexAlgorithm,
}

#[derive(Debug, Clone)]
struct IvfConfig {
    pub n_clusters: usize,
    pub n_probe: usize,
    pub distance_metric: DistanceMetric,
}

impl RefactoredIvfIndex {
    /// Create a new IVF index using common utilities
    pub fn new(config: IvfConfig, dimension: usize) -> anyhow::Result<Self> {
        // USING UTILS: Validate configuration
        validation::validate_dimension(dimension)?;
        
        let distance_compute = UnifiedDistanceCompute::new(config.distance_metric);
        let algorithm = IndexAlgorithm::IVF {
            nlist: config.n_clusters as u32,
            nprobe: config.n_probe as u32,
            quantizer: None,
        };
        
        Ok(Self {
            config,
            // USING UTILS: Standardized vector storage
            vectors: ConcurrentVectorStore::new(dimension),
            // USING UTILS: Standardized statistics
            stats: AtomicStats::new(),
            centroids: Vec::new(),
            inverted_lists: DashMap::new(),
            distance_compute,
            trained: false,
            algorithm,
        })
    }

    /// Add a vector to the index
    pub async fn add(&self, id: String, vector: Arc<VectorRecord>) -> anyhow::Result<()> {
        let start = std::time::Instant::now();
        
        // USING UTILS: Validate vector ID
        validation::validate_vector_id(&id)?;
        
        // USING UTILS: Store vector with automatic validation and counting
        self.vectors.insert(id.clone(), vector.clone())?;
        
        // IVF-specific logic: assign to cluster and update inverted lists
        if self.trained {
            let cluster_id = self.assign_to_cluster(&vector.vector)?;
            self.inverted_lists
                .entry(cluster_id)
                .or_default()
                .push(id);
        }
        
        // USING UTILS: Record operation statistics
        let duration = start.elapsed().as_micros() as u64;
        self.stats.record_success(duration);
        
        Ok(())
    }

    /// Get vector count using utilities
    pub fn vector_count(&self) -> usize {
        // USING UTILS: Thread-safe count from ConcurrentVectorStore
        self.vectors.len()
    }

    /// Get memory usage estimation
    pub fn memory_usage(&self) -> usize {
        // USING UTILS: Standardized memory estimation
        let vector_memory = self.vectors.memory_usage();
        let centroid_memory = memory::vector_memory(self.vectors.dimension()) * self.config.n_clusters;
        let inverted_lists_memory = memory::dashmap_overhead::<usize, Vec<String>>(self.config.n_clusters);
        
        vector_memory + centroid_memory + inverted_lists_memory
    }

    /// Get performance statistics
    pub fn performance_stats(&self) -> PerformanceStats {
        // USING UTILS: Standardized statistics
        PerformanceStats {
            total_operations: self.stats.total_operations(),
            success_rate: self.stats.success_rate(),
            avg_operation_time_us: self.stats.avg_time_us(),
        }
    }

    // IVF-specific methods would remain unchanged
    fn assign_to_cluster(&self, _vector: &[f32]) -> anyhow::Result<usize> {
        // IVF-specific clustering logic
        Ok(0) // Simplified
    }
}

#[derive(Debug)]
pub struct PerformanceStats {
    pub total_operations: usize,
    pub success_rate: f64,
    pub avg_operation_time_us: f64,
}

/// Example: How HNSW Index would look using common utilities
pub struct RefactoredHnswIndex {
    /// Configuration  
    config: HnswConfig,
    
    /// USING UTILS: Standardized vector storage
    vectors: ConcurrentVectorStore,
    
    /// USING UTILS: Bidirectional ID mapping
    id_mapping: ConcurrentIdMapping,
    
    /// USING UTILS: Performance statistics
    stats: AtomicStats,
    
    /// HNSW-specific: Graph layers with composite keys
    /// (layer, internal_node_id) -> connections
    layers: DashMap<(usize, usize), Vec<usize>>,
    
    /// HNSW-specific: Entry point and max layer
    entry_point: std::sync::RwLock<Option<usize>>,
    max_layer: std::sync::atomic::AtomicUsize,
    
    /// Distance computation
    distance_compute: UnifiedDistanceCompute,
    
    /// Random number generator state
    rng_state: Arc<std::sync::RwLock<u64>>,
    
    /// Algorithm type
    algorithm: IndexAlgorithm,
}

#[derive(Debug, Clone)]
struct HnswConfig {
    pub m: usize,
    pub ef_construction: usize,
    pub ef: usize,
    pub max_layers: usize,
    pub distance_metric: DistanceMetric,
}

impl RefactoredHnswIndex {
    pub fn new(config: HnswConfig, dimension: usize) -> anyhow::Result<Self> {
        // USING UTILS: Validate configuration
        validation::validate_dimension(dimension)?;
        
        let distance_compute = UnifiedDistanceCompute::new(config.distance_metric);
        let algorithm = IndexAlgorithm::HNSW {
            m: config.m as u32,
            ef_construction: config.ef_construction as u32,
            ef_search: config.ef as u32,
            max_elements: 1000000,
        };
        
        Ok(Self {
            config,
            // USING UTILS: All the common patterns
            vectors: ConcurrentVectorStore::new(dimension),
            id_mapping: ConcurrentIdMapping::new(),
            stats: AtomicStats::new(),
            
            // HNSW-specific structures
            layers: DashMap::new(),
            entry_point: std::sync::RwLock::new(None),
            max_layer: std::sync::atomic::AtomicUsize::new(0),
            distance_compute,
            rng_state: Arc::new(std::sync::RwLock::new(42)),
            algorithm,
        })
    }

    pub async fn add(&self, id: String, vector: Arc<VectorRecord>) -> anyhow::Result<()> {
        let start = std::time::Instant::now();
        
        // USING UTILS: Validation and ID mapping
        validation::validate_vector_id(&id)?;
        let internal_id = self.id_mapping.register(id.clone())?;
        
        // USING UTILS: Store vector
        self.vectors.insert(id, vector.clone())?;
        
        // HNSW-specific logic would go here
        // ... graph building, layer assignment, etc.
        
        // USING UTILS: Record statistics
        let duration = start.elapsed().as_micros() as u64;
        self.stats.record_success(duration);
        
        Ok(())
    }
}

/// Benefits of using the utils module:
/// 
/// 1. **Consistency**: All indexes use the same patterns for:
///    - Vector storage (ConcurrentVectorStore)
///    - ID mapping (ConcurrentIdMapping) 
///    - Statistics (AtomicStats)
///    - Memory estimation (memory module)
///    - Validation (validation module)
///
/// 2. **Performance**: Optimized DashMap usage with:
///    - Proper atomic counters
///    - Lock-free operations where possible
///    - Efficient memory estimation
///
/// 3. **Maintainability**: 
///    - Single source of truth for common patterns
///    - Easy to optimize all indexes at once
///    - Consistent error handling and validation
///
/// 4. **Testing**: 
///    - Common utilities are thoroughly tested
///    - Index-specific logic is easier to isolate
///    - Consistent behavior across implementations
///
/// 5. **Code Reduction**: 
///    - ~200 lines removed from each index implementation
///    - Eliminates duplicate validation and error handling
///    - Standardizes metadata conversion patterns

#[cfg(test)]
mod integration_tests {
    use super::*;

    #[tokio::test]
    async fn test_refactored_ivf_integration() {
        let config = IvfConfig {
            n_clusters: 4,
            n_probe: 2,
            distance_metric: DistanceMetric::Cosine,
        };
        
        let index = RefactoredIvfIndex::new(config, 3).unwrap();
        
        // Test vector addition
        let vector = Arc::new(VectorRecord {
            id: Some("test1".to_string()),
            vector: vec![1.0, 2.0, 3.0],
            metadata: Vec::new(),
            timestamp: Some(0),
            updated_at: None,
            expires_at: None,
            version: None,
            // rank removed -  None,
            similarity: None,
            similarity: None,
        });
        
        index.add("test1".to_string(), vector).await.unwrap();
        
        // Verify statistics were recorded
        let stats = index.performance_stats();
        assert_eq!(stats.total_operations, 1);
        assert!(stats.success_rate > 0.99);
        
        // Verify vector count
        assert_eq!(index.vector_count(), 1);
        
        // Verify memory estimation works
        assert!(index.memory_usage() > 0);
    }
}