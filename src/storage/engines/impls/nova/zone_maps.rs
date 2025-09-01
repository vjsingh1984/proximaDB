// Zone maps and cost-based optimization for NOVA engine
// Advanced multi-dimensional pruning and search cost estimation

use anyhow::{anyhow, Result};
use parquet::file::metadata::{RowGroupMetaData, ColumnChunkMetaData};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, instrument};

use crate::compute::distance_computation::DistanceMetric;
use super::hierarchical_stats::{SuperBlock, EnhancedRowGroupStats, ZoneMap};

/// Advanced zone map with multiple optimization strategies
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdvancedZoneMap {
    /// Basic zone map
    pub base_zone_map: ZoneMap,
    
    /// Hierarchical zone maps for different granularities
    pub hierarchical_zones: Vec<HierarchicalZone>,
    
    /// Probabilistic zone map for approximate queries
    pub probabilistic_zone: Option<ProbabilisticZone>,
    
    /// Adaptive zone map that learns from query patterns
    pub adaptive_zone: Option<AdaptiveZone>,
    
    /// Multi-scale zone maps for different distance metrics
    pub multi_scale_zones: HashMap<DistanceMetric, ScaledZoneMap>,
}

/// Hierarchical zone for multi-resolution pruning
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HierarchicalZone {
    /// Resolution level (0 = finest, higher = coarser)
    pub level: u32,
    
    /// Dimensions at this resolution level
    pub dimensions: Vec<u32>,
    
    /// Zone map at this resolution
    pub zone_map: ZoneMap,
    
    /// Selectivity at this level
    pub selectivity: f32,
    
    /// Cost to evaluate at this level
    pub evaluation_cost: f32,
}

/// Probabilistic zone map using sketches
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbabilisticZone {
    /// Count-Min Sketch for frequency estimation
    pub frequency_sketch: CountMinSketch,
    
    /// HyperLogLog for cardinality estimation
    pub cardinality_sketch: HyperLogLog,
    
    /// Bloom filter for existence checks
    pub existence_filter: BloomFilter,
    
    /// Confidence bounds
    pub confidence_bounds: (f32, f32),
}

/// Adaptive zone map that learns from queries
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdaptiveZone {
    /// Query pattern history
    pub query_patterns: Vec<QueryPattern>,
    
    /// Learned selectivity model
    pub selectivity_model: SelectivityModel,
    
    /// Adaptive thresholds
    pub adaptive_thresholds: HashMap<String, f32>,
    
    /// Learning rate for adaptation
    pub learning_rate: f32,
    
    /// Last update timestamp
    pub last_updated: chrono::DateTime<chrono::Utc>,
}

/// Zone map scaled for specific distance metric
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScaledZoneMap {
    /// Distance metric this zone is optimized for
    pub distance_metric: DistanceMetric,
    
    /// Transformed bounds for this metric
    pub transformed_bounds: TransformedBounds,
    
    /// Precomputed distance bounds
    pub distance_bounds: (f32, f32),
    
    /// Approximation quality
    pub approximation_quality: f32,
}

/// Transformed bounds for specific distance metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformedBounds {
    /// Original min/max values
    pub original_min: Vec<f32>,
    pub original_max: Vec<f32>,
    
    /// Transformed min/max for the distance metric
    pub transformed_min: Vec<f32>,
    pub transformed_max: Vec<f32>,
    
    /// Additional metric-specific parameters
    pub metric_params: HashMap<String, f32>,
}

/// Cost-based row group ordering engine
pub struct CostBasedOptimizer {
    /// Cost model parameters
    pub cost_model: CostModel,
    
    /// Historical performance data
    pub performance_history: PerformanceHistory,
    
    /// Query workload characteristics
    pub workload_stats: WorkloadStats,
    
    /// Hardware characteristics
    pub hardware_profile: HardwareProfile,
}

/// Cost model for estimating search costs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostModel {
    /// I/O cost parameters
    pub io_cost_params: IOCostParams,
    
    /// CPU cost parameters
    pub cpu_cost_params: CPUCostParams,
    
    /// Memory cost parameters
    pub memory_cost_params: MemoryCostParams,
    
    /// Network cost parameters (for distributed setups)
    pub network_cost_params: Option<NetworkCostParams>,
}

/// I/O cost modeling
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IOCostParams {
    /// Sequential read throughput (MB/s)
    pub sequential_throughput: f32,
    
    /// Random read throughput (MB/s)
    pub random_throughput: f32,
    
    /// Seek time (milliseconds)
    pub seek_time_ms: f32,
    
    /// Page cache hit rate
    pub cache_hit_rate: f32,
    
    /// Compression decompression cost (MB/s)
    pub decompression_throughput: f32,
}

/// CPU cost modeling
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CPUCostParams {
    /// Distance computation rate (vectors/second)
    pub distance_computation_rate: f32,
    
    /// Quantization computation rate (vectors/second)
    pub quantization_rate: f32,
    
    /// Filtering rate (candidates/second)
    pub filtering_rate: f32,
    
    /// Sorting rate (comparisons/second)
    pub sorting_rate: f32,
}

/// Memory cost modeling
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryCostParams {
    /// Memory bandwidth (GB/s)
    pub memory_bandwidth: f32,
    
    /// Cache miss penalty (nanoseconds)
    pub cache_miss_penalty: f32,
    
    /// Memory allocation overhead
    pub allocation_overhead: f32,
}

/// Network cost modeling for distributed systems
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkCostParams {
    /// Network bandwidth (Mbps)
    pub bandwidth: f32,
    
    /// Network latency (milliseconds)
    pub latency_ms: f32,
    
    /// Packet loss rate
    pub packet_loss_rate: f32,
}

/// Query pattern for adaptive learning
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryPattern {
    /// Query vector characteristics
    pub query_characteristics: QueryCharacteristics,
    
    /// Observed selectivity
    pub observed_selectivity: f32,
    
    /// Actual cost
    pub actual_cost: f32,
    
    /// Predicted cost
    pub predicted_cost: f32,
    
    /// Timestamp
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Query characteristics for pattern matching
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryCharacteristics {
    /// Query vector norm
    pub norm: f32,
    
    /// Query vector sparsity
    pub sparsity: f32,
    
    /// Dominant dimensions
    pub dominant_dimensions: Vec<u32>,
    
    /// Distance metric used
    pub distance_metric: DistanceMetric,
    
    /// Top-k value
    pub top_k: u32,
}

/// Learned selectivity model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectivityModel {
    /// Model parameters
    pub parameters: Vec<f32>,
    
    /// Model type
    pub model_type: ModelType,
    
    /// Training accuracy
    pub accuracy: f32,
    
    /// Number of training samples
    pub training_samples: u32,
}

/// Model types for selectivity prediction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ModelType {
    Linear,
    Polynomial { degree: u32 },
    NeuralNetwork { layers: Vec<u32> },
    DecisionTree { depth: u32 },
}

impl AdvancedZoneMap {
    /// Create a comprehensive zone map from vectors and metadata
    pub fn from_row_group(
        vectors: &[Vec<f32>],
        metadata: &RowGroupMetaData,
        config: &ZoneMapConfig,
    ) -> Result<Self> {
        let base_zone_map = ZoneMap::from_vectors(vectors)?;
        
        // Build hierarchical zones
        let hierarchical_zones = Self::build_hierarchical_zones(vectors, config)?;
        
        // Build probabilistic zone if enabled
        let probabilistic_zone = if config.enable_probabilistic {
            Some(Self::build_probabilistic_zone(vectors, config)?)
        } else {
            None
        };
        
        // Build adaptive zone if enabled
        let adaptive_zone = if config.enable_adaptive {
            Some(Self::build_adaptive_zone(vectors, config)?)
        } else {
            None
        };
        
        // Build multi-scale zones
        let multi_scale_zones = Self::build_multi_scale_zones(vectors, config)?;
        
        Ok(Self {
            base_zone_map,
            hierarchical_zones,
            probabilistic_zone,
            adaptive_zone,
            multi_scale_zones,
        })
    }
    
    /// Advanced query intersection with multiple optimization strategies
    #[instrument(skip(self, query))]
    pub fn can_intersect_advanced(
        &self,
        query: &[f32],
        distance_metric: DistanceMetric,
        max_similarity: f32,
        optimization_strategy: OptimizationStrategy,
    ) -> AdvancedIntersectionResult {
        let mut result = AdvancedIntersectionResult::default();
        
        // Start with basic zone map check
        let basic_intersects = self.base_zone_map.intersects_query(query, distance_metric, max_similarity);
        if !basic_intersects {
            result.intersects = false;
            result.confidence = 1.0;
            result.pruning_strategy = PruningStrategy::BasicZoneMap;
            return result;
        }
        
        match optimization_strategy {
            OptimizationStrategy::Hierarchical => {
                self.check_hierarchical_intersection(query, distance_metric, max_similarity, &mut result)
            }
            OptimizationStrategy::Probabilistic => {
                self.check_probabilistic_intersection(query, distance_metric, max_similarity, &mut result)
            }
            OptimizationStrategy::Adaptive => {
                self.check_adaptive_intersection(query, distance_metric, max_similarity, &mut result)
            }
            OptimizationStrategy::MultiScale => {
                self.check_multi_scale_intersection(query, distance_metric, max_similarity, &mut result)
            }
            OptimizationStrategy::Hybrid => {
                self.check_hybrid_intersection(query, distance_metric, max_similarity, &mut result)
            }
        }
        
        result
    }
    
    fn build_hierarchical_zones(vectors: &[Vec<f32>], config: &ZoneMapConfig) -> Result<Vec<HierarchicalZone>> {
        let mut zones = Vec::new();
        let dimension = vectors[0].len();
        
        // Build zones at different resolution levels
        for level in 0..config.hierarchical_levels {
            let resolution = 1 << level; // 1, 2, 4, 8, ...
            let dimensions_per_group = (dimension + resolution - 1) / resolution;
            
            let mut grouped_dimensions = Vec::new();
            for group in 0..resolution {
                let start_dim = group * dimensions_per_group;
                let end_dim = ((group + 1) * dimensions_per_group).min(dimension);
                
                if start_dim < dimension {
                    grouped_dimensions.extend(start_dim..end_dim);
                }
            }
            
            // Create zone map for this level
            let zone_vectors: Vec<Vec<f32>> = vectors.iter()
                .map(|v| grouped_dimensions.iter().map(|&i| v[i]).collect())
                .collect();
            
            let zone_map = ZoneMap::from_vectors(&zone_vectors)?;
            
            zones.push(HierarchicalZone {
                level,
                dimensions: grouped_dimensions,
                zone_map,
                selectivity: 1.0 / (level + 1) as f32, // Coarser levels are less selective
                evaluation_cost: 10.0 * (level + 1) as f32, // Higher levels cost more
            });
        }
        
        Ok(zones)
    }
    
    fn build_probabilistic_zone(vectors: &[Vec<f32>], config: &ZoneMapConfig) -> Result<ProbabilisticZone> {
        let frequency_sketch = CountMinSketch::new(config.sketch_width, config.sketch_depth);
        let cardinality_sketch = HyperLogLog::new(config.hll_precision);
        let existence_filter = BloomFilter::new(vectors.len(), config.bloom_false_positive_rate);
        
        Ok(ProbabilisticZone {
            frequency_sketch,
            cardinality_sketch,
            existence_filter,
            confidence_bounds: (0.9, 0.99), // 90-99% confidence
        })
    }
    
    fn build_adaptive_zone(_vectors: &[Vec<f32>], _config: &ZoneMapConfig) -> Result<AdaptiveZone> {
        Ok(AdaptiveZone {
            query_patterns: Vec::new(),
            selectivity_model: SelectivityModel {
                parameters: vec![1.0, 0.0], // Linear model: y = 1*x + 0
                model_type: ModelType::Linear,
                accuracy: 0.5,
                training_samples: 0,
            },
            adaptive_thresholds: HashMap::new(),
            learning_rate: 0.01,
            last_updated: chrono::Utc::now(),
        })
    }
    
    fn build_multi_scale_zones(vectors: &[Vec<f32>], _config: &ZoneMapConfig) -> Result<HashMap<DistanceMetric, ScaledZoneMap>> {
        let mut multi_scale = HashMap::new();
        
        // Build zone maps for different distance metrics
        for &metric in &[DistanceMetric::Euclidean, DistanceMetric::Cosine, DistanceMetric::DotProduct] {
            let scaled_zone = Self::build_scaled_zone_map(vectors, metric)?;
            multi_scale.insert(metric, scaled_zone);
        }
        
        Ok(multi_scale)
    }
    
    fn build_scaled_zone_map(vectors: &[Vec<f32>], metric: DistanceMetric) -> Result<ScaledZoneMap> {
        let dimension = vectors[0].len();
        let mut transformed_min = vec![f32::INFINITY; dimension];
        let mut transformed_max = vec![f32::NEG_INFINITY; dimension];
        let original_min = vec![f32::INFINITY; dimension];
        let original_max = vec![f32::NEG_INFINITY; dimension];
        
        // Transform vectors according to distance metric
        for vector in vectors {
            let transformed = Self::transform_vector_for_metric(vector, metric);
            
            for (i, &value) in transformed.iter().enumerate() {
                transformed_min[i] = transformed_min[i].min(value);
                transformed_max[i] = transformed_max[i].max(value);
            }
        }
        
        let transformed_bounds = TransformedBounds {
            original_min,
            original_max,
            transformed_min,
            transformed_max,
            metric_params: HashMap::new(),
        };
        
        Ok(ScaledZoneMap {
            distance_metric: metric,
            transformed_bounds,
            distance_bounds: (0.0, f32::INFINITY),
            approximation_quality: 0.95,
        })
    }
    
    fn transform_vector_for_metric(vector: &[f32], metric: DistanceMetric) -> Vec<f32> {
        match metric {
            DistanceMetric::Cosine => {
                // Normalize vector for cosine distance
                let norm: f32 = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
                if norm > 0.0 {
                    vector.iter().map(|x| x / norm).collect()
                } else {
                    vector.to_vec()
                }
            }
            DistanceMetric::DotProduct => {
                // For dot product, we might want to use negative values
                vector.iter().map(|&x| -x).collect()
            }
            _ => vector.to_vec(), // No transformation for Euclidean and others
        }
    }
    
    fn check_hierarchical_intersection(
        &self,
        query: &[f32],
        distance_metric: DistanceMetric,
        max_similarity: f32,
        result: &mut AdvancedIntersectionResult,
    ) {
        // Check from coarsest to finest resolution
        for zone in self.hierarchical_zones.iter().rev() {
            let intersects = zone.zone_map.intersects_query(query, distance_metric, max_similarity);
            
            if !intersects {
                result.intersects = false;
                result.confidence = 0.8 + 0.1 * zone.level as f32; // Higher confidence at coarser levels
                result.pruning_strategy = PruningStrategy::Hierarchical(zone.level);
                result.estimated_cost_savings = zone.evaluation_cost;
                return;
            }
        }
        
        result.intersects = true;
        result.confidence = 0.7; // Lower confidence when all levels pass
        result.pruning_strategy = PruningStrategy::NoPruning;
    }
    
    fn check_probabilistic_intersection(
        &self,
        _query: &[f32],
        _distance_metric: DistanceMetric,
        _max_similarity: f32,
        result: &mut AdvancedIntersectionResult,
    ) {
        if let Some(prob_zone) = &self.probabilistic_zone {
            // Use probabilistic sketches for intersection estimation
            let estimated_selectivity = 0.5; // Placeholder
            
            result.intersects = estimated_selectivity > 0.1;
            result.confidence = prob_zone.confidence_bounds.0;
            result.pruning_strategy = PruningStrategy::Probabilistic;
            result.estimated_selectivity = Some(estimated_selectivity);
        } else {
            result.intersects = true;
            result.confidence = 0.5;
        }
    }
    
    fn check_adaptive_intersection(
        &self,
        query: &[f32],
        distance_metric: DistanceMetric,
        max_similarity: f32,
        result: &mut AdvancedIntersectionResult,
    ) {
        if let Some(adaptive_zone) = &self.adaptive_zone {
            // Use learned model to predict selectivity
            let query_characteristics = QueryCharacteristics::from_query(query, distance_metric, 10);
            let predicted_selectivity = adaptive_zone.selectivity_model.predict(&query_characteristics);
            
            result.intersects = predicted_selectivity > 0.05;
            result.confidence = adaptive_zone.selectivity_model.accuracy;
            result.pruning_strategy = PruningStrategy::Adaptive;
            result.estimated_selectivity = Some(predicted_selectivity);
        } else {
            result.intersects = true;
            result.confidence = 0.5;
        }
    }
    
    fn check_multi_scale_intersection(
        &self,
        query: &[f32],
        distance_metric: DistanceMetric,
        max_similarity: f32,
        result: &mut AdvancedIntersectionResult,
    ) {
        if let Some(scaled_zone) = self.multi_scale_zones.get(&distance_metric) {
            // Use metric-specific optimized bounds
            let transformed_query = Self::transform_vector_for_metric(query, distance_metric);
            
            // Check intersection with transformed bounds
            let intersects = self.check_transformed_intersection(
                &transformed_query,
                &scaled_zone.transformed_bounds,
                max_similarity,
            );
            
            result.intersects = intersects;
            result.confidence = scaled_zone.approximation_quality;
            result.pruning_strategy = PruningStrategy::MultiScale(distance_metric);
        } else {
            result.intersects = true;
            result.confidence = 0.5;
        }
    }
    
    fn check_hybrid_intersection(
        &self,
        query: &[f32],
        distance_metric: DistanceMetric,
        max_similarity: f32,
        result: &mut AdvancedIntersectionResult,
    ) {
        // Combine multiple strategies for best accuracy
        let mut sub_results = Vec::new();
        
        // Try hierarchical
        let mut hierarchical_result = AdvancedIntersectionResult::default();
        self.check_hierarchical_intersection(query, distance_metric, max_similarity, &mut hierarchical_result);
        sub_results.push(hierarchical_result);
        
        // Try multi-scale
        let mut multi_scale_result = AdvancedIntersectionResult::default();
        self.check_multi_scale_intersection(query, distance_metric, max_similarity, &mut multi_scale_result);
        sub_results.push(multi_scale_result);
        
        // Combine results using weighted voting
        let total_confidence: f32 = sub_results.iter().map(|r| r.confidence).sum();
        let weighted_intersection: f32 = sub_results.iter()
            .map(|r| if r.intersects { r.confidence } else { 0.0 })
            .sum();
        
        result.intersects = weighted_intersection / total_confidence > 0.5;
        result.confidence = total_confidence / sub_results.len() as f32;
        result.pruning_strategy = PruningStrategy::Hybrid;
    }
    
    fn check_transformed_intersection(
        &self,
        transformed_query: &[f32],
        bounds: &TransformedBounds,
        max_similarity: f32,
    ) -> bool {
        // Check intersection in transformed space
        let mut min_distance_sq = 0.0;
        
        for (i, &q) in transformed_query.iter().enumerate() {
            if i >= bounds.transformed_min.len() {
                break;
            }
            
            let min_val = bounds.transformed_min[i];
            let max_val = bounds.transformed_max[i];
            
            if q < min_val {
                let diff = min_val - q;
                min_distance_sq += diff * diff;
            } else if q > max_val {
                let diff = q - max_val;
                min_distance_sq += diff * diff;
            }
        }
        
        min_distance_sq.sqrt() <= max_similarity
    }
}

/// Configuration for zone map construction
#[derive(Debug, Clone)]
pub struct ZoneMapConfig {
    pub enable_hierarchical: bool,
    pub hierarchical_levels: u32,
    pub enable_probabilistic: bool,
    pub enable_adaptive: bool,
    pub sketch_width: usize,
    pub sketch_depth: usize,
    pub hll_precision: u8,
    pub bloom_false_positive_rate: f64,
}

impl Default for ZoneMapConfig {
    fn default() -> Self {
        Self {
            enable_hierarchical: true,
            hierarchical_levels: 3,
            enable_probabilistic: false, // Disabled by default for simplicity
            enable_adaptive: false,      // Disabled by default for simplicity
            sketch_width: 1024,
            sketch_depth: 4,
            hll_precision: 12,
            bloom_false_positive_rate: 0.01,
        }
    }
}

/// Result of advanced intersection testing
#[derive(Debug, Default)]
pub struct AdvancedIntersectionResult {
    pub intersects: bool,
    pub pruning_strategy: PruningStrategy,
    pub estimated_selectivity: Option<f32>,
    pub estimated_cost_savings: f32,
    pub confidence: f32,
}

/// Pruning strategies
#[derive(Debug, Default)]
pub enum PruningStrategy {
    #[default]
    NoPruning,
    BasicZoneMap,
    Hierarchical(u32),
    Probabilistic,
    Adaptive,
    MultiScale(DistanceMetric),
    Hybrid,
}

/// Optimization strategies for zone map usage
#[derive(Debug, Clone)]
pub enum OptimizationStrategy {
    Hierarchical,
    Probabilistic,
    Adaptive,
    MultiScale,
    Hybrid,
}

// Placeholder implementations for probabilistic data structures
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CountMinSketch {
    width: usize,
    depth: usize,
}

impl CountMinSketch {
    fn new(width: usize, depth: usize) -> Self {
        Self { width, depth }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HyperLogLog {
    precision: u8,
}

impl HyperLogLog {
    fn new(precision: u8) -> Self {
        Self { precision }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BloomFilter {
    expected_items: usize,
    false_positive_rate: f64,
}

impl BloomFilter {
    fn new(expected_items: usize, false_positive_rate: f64) -> Self {
        Self {
            expected_items,
            false_positive_rate,
        }
    }
}

impl QueryCharacteristics {
    fn from_query(query: &[f32], distance_metric: DistanceMetric, top_k: u32) -> Self {
        let norm = query.iter().map(|x| x * x).sum::<f32>().sqrt();
        let sparsity = query.iter().filter(|&&x| x == 0.0).count() as f32 / query.len() as f32;
        
        // Find dominant dimensions (top 10% by magnitude)
        let mut indexed_values: Vec<(usize, f32)> = query.iter()
            .enumerate()
            .map(|(i, &v)| (i, v.abs()))
            .collect();
        indexed_values.sort_by(|a, b| b.1.partial_cmp(&a.1));
        
        let dominant_count = (query.len() / 10).max(1);
        let dominant_dimensions = indexed_values.iter()
            .take(dominant_count)
            .map(|(i, _)| *i as u32)
            .collect();
        
        Self {
            norm,
            sparsity,
            dominant_dimensions,
            distance_metric,
            top_k,
        }
    }
}

impl SelectivityModel {
    fn predict(&self, characteristics: &QueryCharacteristics) -> f32 {
        match self.model_type {
            ModelType::Linear => {
                // Simple linear model: selectivity = a * norm + b * sparsity + c
                let norm_factor = self.parameters.get("norm").unwrap_or(&0.0);
                let sparsity_factor = self.parameters.get("sparsity").unwrap_or(&0.0);
                let intercept = self.parameters.get("intercept").unwrap_or(&0.5);
                
                (norm_factor * characteristics.norm + sparsity_factor * characteristics.sparsity + intercept)
                    .max(0.0)
                    .min(1.0)
            }
            _ => 0.5, // Default selectivity for other model types
        }
    }
}

// Additional implementations for cost-based optimization and performance tracking would go here
// These are simplified for the scope of this implementation

/// Workload statistics for optimization
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkloadStats {
    pub avg_query_selectivity: f32,
    pub avg_top_k: u32,
    pub dominant_distance_metric: DistanceMetric,
    pub query_frequency: f32,
}

/// Hardware profile for cost estimation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareProfile {
    pub cpu_cores: u32,
    pub memory_gb: u32,
    pub storage_type: StorageType,
    pub network_bandwidth_mbps: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StorageType {
    HDD,
    SSD,
    NVMe,
    Cloud,
}

/// Performance history for learning
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PerformanceHistory {
    pub recent_queries: Vec<QueryPerformance>,
    pub avg_latency_ms: f32,
    pub avg_throughput: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryPerformance {
    pub query_id: String,
    pub latency_ms: f32,
    pub candidates_processed: usize,
    pub pruning_effectiveness: f32,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

// Default implementation is provided by prost::Enumeration derive

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_zone_map_config() {
        let config = ZoneMapConfig::default();
        assert!(config.enable_hierarchical);
        assert_eq!(config.hierarchical_levels, 3);
        assert_eq!(config.sketch_width, 1024);
    }
    
    #[test]
    fn test_query_characteristics() {
        let query = vec![1.0, 0.0, 2.0, 0.0, 3.0];
        let characteristics = QueryCharacteristics::from_query(&query, DistanceMetric::Euclidean, 10);
        
        assert_eq!(characteristics.top_k, 10);
        assert_eq!(characteristics.sparsity, 0.4); // 2/5 zeros
        assert!(characteristics.norm > 0.0);
        assert_eq!(characteristics.dominant_dimensions.len(), 1); // top 10% of 5 = 1
    }
    
    #[test]
    fn test_selectivity_model_prediction() {
        let model = SelectivityModel {
            parameters: vec![0.1, -0.2, 0.5], // norm_factor, sparsity_factor, intercept
            model_type: ModelType::Linear,
            accuracy: 0.8,
            training_samples: 100,
        };
        
        let characteristics = QueryCharacteristics {
            norm: 2.0,
            sparsity: 0.3,
            dominant_dimensions: vec![0, 1, 2],
            distance_metric: DistanceMetric::Euclidean,
            top_k: 10,
        };
        
        let selectivity = model.predict(&characteristics);
        // Expected: 0.1 * 2.0 + (-0.2) * 0.3 + 0.5 = 0.2 - 0.06 + 0.5 = 0.64
        assert!((selectivity - 0.64).abs() < 0.01);
    }
}