//! Unified Distance Computation System for ProximaDB
//!
//! This module provides a unified abstraction for distance calculations across:
//! - Storage engines (VIPER, LSM, WAL)
//! - Memory operations (memtable, cache)
//! - Distributed systems (multi-node, heterogeneous CPUs)
//!
//! Key features:
//! - Hardware acceleration with runtime SIMD detection
//! - Distance metric hierarchy (request → collection → system default)
//! - Batch processing for optimal performance
//! - Consistent results across storage tiers
//! - **Normalized distance semantics**: ALL metrics return values where LOWER = MORE SIMILAR
//! - Future-ready for distributed computing
//!
//! ## Distance Normalization
//! 
//! Different distance algorithms have different semantics:
//! - Euclidean/Manhattan: Lower values = more similar (native)
//! - Cosine Distance: Lower values = more similar (native)
//! - Dot Product: Higher values = more similar (INVERTED to lower = more similar)
//! - Cosine Similarity: Higher values = more similar (INVERTED to lower = more similar)
//!
//! The unified system normalizes ALL metrics so that:
//! **LOWER VALUES ALWAYS MEAN MORE SIMILAR**
//!
//! This provides consistent behavior for calling modules across storage and WAL.

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::debug;

use super::distance::{create_distance_calculator, DistanceMetric, PlatformCapability, detect_platform_capability};
use crate::services::collection_service::CollectionService;
use std::sync::{Arc, OnceLock};

/// Global hardware capability cache - detected once at startup
static UNIFIED_PLATFORM_CAPABILITY: OnceLock<PlatformCapability> = OnceLock::new();

/// Forward declaration for distributed distance manager
use super::distributed_distance::DistributedDistanceManager;

/// Unified distance computation manager with hardware acceleration and optional distributed support
#[derive(Clone)]
pub struct UnifiedDistanceCompute {
    /// System default distance metric
    system_default: DistanceMetric,
    /// Hardware capability for SIMD optimization
    platform_capability: PlatformCapability,
    /// Optional distributed distance manager for multi-node computation
    distributed_manager: Option<Arc<DistributedDistanceManager>>,
}

impl std::fmt::Debug for UnifiedDistanceCompute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnifiedDistanceCompute")
            .field("system_default", &self.system_default)
            .field("platform_capability", &self.platform_capability)
            .field("has_distributed_manager", &self.distributed_manager.is_some())
            .finish()
    }
}

impl Default for UnifiedDistanceCompute {
    fn default() -> Self {
        Self {
            system_default: DistanceMetric::Cosine,
            platform_capability: Self::get_or_detect_platform_capability(),
            distributed_manager: None,
        }
    }
}

impl UnifiedDistanceCompute {
    /// Get or detect platform capability (cached globally)
    fn get_or_detect_platform_capability() -> PlatformCapability {
        *UNIFIED_PLATFORM_CAPABILITY.get_or_init(|| {
            let capability = detect_platform_capability();
            debug!("🚀 Unified Distance Compute detected platform capability: {}", capability);
            capability
        })
    }

    /// Create a new unified distance compute manager with default metric
    pub fn new(default_metric: DistanceMetric) -> Self {
        Self {
            system_default: default_metric,
            platform_capability: Self::get_or_detect_platform_capability(),
            distributed_manager: None,
        }
    }

    /// Create a new unified distance compute manager with distributed support
    pub fn with_distributed_manager(
        default_metric: DistanceMetric,
        distributed_manager: Arc<DistributedDistanceManager>,
    ) -> Self {
        Self {
            system_default: default_metric,
            platform_capability: Self::get_or_detect_platform_capability(),
            distributed_manager: Some(distributed_manager),
        }
    }

    /// Get the detected platform capability
    pub fn platform_capability(&self) -> PlatformCapability {
        self.platform_capability
    }

    /// Convert similarity values to distance values for consistent semantics
    /// 
    /// This ensures ALL metrics follow "lower = more similar" semantics:
    /// - For Dot Product: Inverts positive values, handles negative appropriately
    /// - For Cosine Similarity: Converts to Cosine Distance (1 - similarity)
    fn invert_similarity_to_distance(&self, similarity_value: f32) -> f32 {
        match similarity_value {
            // For positive similarities, simple inversion works well
            val if val >= 0.0 => {
                // Convert similarity [0,1] to distance [1,0] or [0,∞] to [∞,0]
                if val <= 1.0 {
                    1.0 - val  // Standard cosine similarity to distance conversion
                } else {
                    1.0 / val  // For unbounded similarities like dot product
                }
            }
            // For negative similarities (like negative cosine similarity),
            // map to distance > 1.0 to maintain ordering
            val => 1.0 - val  // -0.5 becomes 1.5, -1.0 becomes 2.0
        }
    }

    /// Get the native behavior description for a metric (for debugging/logging)
    pub fn metric_behavior_description(&self, metric: &DistanceMetric) -> &'static str {
        match metric {
            DistanceMetric::Euclidean => "Euclidean Distance (lower = more similar, native)",
            DistanceMetric::Manhattan => "Manhattan Distance (lower = more similar, native)", 
            DistanceMetric::Cosine => "Cosine Distance (lower = more similar, native)",
            DistanceMetric::DotProduct => "Dot Product Similarity (higher = more similar, inverted to lower = more similar)",
            DistanceMetric::Hamming => "Hamming Distance (lower = more similar, native)",
            DistanceMetric::Jaccard => "Jaccard Distance (lower = more similar, native)",
            DistanceMetric::Custom(_name) => "Custom metric (fallback to cosine distance)",
        }
    }

    /// Calculate normalized distance between two vectors using specified metric
    /// 
    /// **IMPORTANT**: This method normalizes ALL distance metrics so that:
    /// **LOWER VALUES ALWAYS MEAN MORE SIMILAR**
    /// 
    /// This provides consistent semantics across all algorithms:
    /// - Euclidean/Manhattan/Cosine Distance: Return native values (lower = more similar)
    /// - Dot Product/Cosine Similarity: Return inverted values (higher similarity becomes lower distance)
    /// 
    /// **Dimension Mismatch Handling**:
    /// - Returns appropriate fallback values when vector dimensions don't match
    /// - Ensures calling code doesn't panic on dimension mismatches
    pub fn calculate_distance(&self, vec_a: &[f32], vec_b: &[f32], metric: &DistanceMetric) -> f32 {
        // Handle dimension mismatches gracefully
        if vec_a.len() != vec_b.len() {
            return self.handle_dimension_mismatch(metric, vec_a.len(), vec_b.len());
        }
        
        let calculator = create_distance_calculator(metric.clone());
        let raw_value = calculator.distance(vec_a, vec_b);
        
        // Normalize so that LOWER values ALWAYS mean MORE SIMILAR
        if calculator.is_similarity() {
            // For similarity metrics (higher = more similar), invert to distance semantics
            self.invert_similarity_to_distance(raw_value)
        } else {
            // For distance metrics (lower = more similar), return as-is
            raw_value
        }
    }

    /// Handle dimension mismatches with appropriate fallback values
    fn handle_dimension_mismatch(&self, metric: &DistanceMetric, len_a: usize, len_b: usize) -> f32 {
        debug!(
            "⚠️ Dimension mismatch for {:?}: {} vs {} dimensions",
            metric, len_a, len_b
        );
        
        match metric {
            // For similarity-based metrics, return maximum distance (least similar)
            DistanceMetric::Cosine | DistanceMetric::DotProduct => {
                // Return maximum distance to indicate no similarity
                2.0  // Worst case for cosine distance range [0,2]
            }
            // For distance-based metrics, return infinity to indicate infinite distance
            DistanceMetric::Euclidean | DistanceMetric::Manhattan => {
                f32::INFINITY
            }
            // For discrete metrics, return maximum discrete distance
            DistanceMetric::Hamming | DistanceMetric::Jaccard => {
                1.0  // Maximum discrete distance
            }
            // For custom metrics, fall back to cosine behavior
            DistanceMetric::Custom(_) => 2.0,
        }
    }

    /// Get system default distance metric
    pub fn system_default(&self) -> &DistanceMetric {
        &self.system_default
    }

    /// Calculate normalized distances for batch processing with hardware acceleration
    /// 
    /// **IMPORTANT**: This method normalizes ALL distance metrics so that:
    /// **LOWER VALUES ALWAYS MEAN MORE SIMILAR**
    /// 
    /// **Dimension Mismatch Handling**: Gracefully handles dimension mismatches in batch
    /// **Hardware Acceleration**: Uses optimal SIMD implementation for current platform
    pub fn calculate_distance_batch(
        &self,
        query: &[f32],
        vectors: &[&[f32]],
        metric: &DistanceMetric,
    ) -> Vec<f32> {
        // Use hardware-accelerated calculator
        let calculator = create_distance_calculator(metric.clone());
        let is_similarity = calculator.is_similarity();
        
        // For performance, check if all vectors have the same dimension as query
        let query_dim = query.len();
        let uniform_dimensions = vectors.iter().all(|v| v.len() == query_dim);
        
        if uniform_dimensions && !vectors.is_empty() {
            // Use optimized batch computation when dimensions are uniform
            let raw_distances = calculator.distance_batch(query, vectors);
            
            if is_similarity {
                // Invert similarity values to distance semantics
                raw_distances
                    .into_iter()
                    .map(|val| self.invert_similarity_to_distance(val))
                    .collect()
            } else {
                // Distance metrics - return as-is
                raw_distances
            }
        } else {
            // Fall back to individual distance calculations for dimension mismatches
            vectors
                .iter()
                .map(|vector| {
                    // Use the unified calculate_distance method which handles dimension mismatches
                    self.calculate_distance(query, vector, metric)
                })
                .collect()
        }
    }

    /// Calculate normalized distances for large batch processing with chunking
    /// 
    /// **Hardware Optimization**: Processes in chunks for optimal memory usage and SIMD efficiency
    pub fn calculate_distance_batch_chunked(
        &self,
        query: &[f32],
        vectors: &[&[f32]],
        metric: &DistanceMetric,
        chunk_size: usize,
    ) -> Vec<f32> {
        let mut results = Vec::with_capacity(vectors.len());
        
        for chunk in vectors.chunks(chunk_size) {
            let mut chunk_results = self.calculate_distance_batch(query, chunk, metric);
            results.append(&mut chunk_results);
        }
        
        results
    }

    /// Calculate distances using distributed computation if available
    /// 
    /// **Distributed Computing**: Routes computation to multiple nodes if distributed manager is available
    /// **Unified Semantics**: All results follow "lower = more similar" semantics
    pub async fn calculate_distance_distributed(
        &self,
        query: &[f32],
        node_vectors: &[(&str, &[&[f32]])], 
        metric: &DistanceMetric,
    ) -> Result<Vec<(String, Vec<f32>)>> {
        if let Some(ref distributed_manager) = self.distributed_manager {
            // Use distributed computation
            debug!("🌐 Using distributed distance computation across {} nodes", node_vectors.len());
            distributed_manager
                .calculate_distance_distributed(query, node_vectors, metric)
                .await
        } else {
            // Fall back to local computation for each node
            debug!("🖥️ Using local computation for {} node batches", node_vectors.len());
            let mut results = Vec::new();
            
            for (node_id, vectors) in node_vectors {
                let distances = self.calculate_distance_batch(query, vectors, metric);
                results.push((node_id.to_string(), distances));
            }
            
            Ok(results)
        }
    }

    /// Aggregate distributed results with unified semantics
    /// 
    /// **Unified Semantics**: Always sorts by ascending order (lower = more similar)
    pub async fn aggregate_distributed_results(
        &self,
        node_results: &[(String, Vec<(f32, String)>)],
        metric: &DistanceMetric,
        k: usize,
    ) -> Result<Vec<(f32, String)>> {
        if let Some(ref distributed_manager) = self.distributed_manager {
            // Use distributed manager's aggregation
            distributed_manager
                .aggregate_distributed_results(node_results, metric, k)
                .await
        } else {
            // Local aggregation using unified semantics
            let mut all_results = Vec::new();
            for (_node_id, results) in node_results {
                for (distance, vector_id) in results {
                    all_results.push((*distance, vector_id.clone()));
                }
            }
            
            // Sort and limit using unified semantics
            DistanceResultOrdering::sort_and_limit(&mut all_results, metric, self, k);
            Ok(all_results)
        }
    }

    /// Check if distributed computation is available
    pub fn has_distributed_support(&self) -> bool {
        self.distributed_manager.is_some()
    }

    /// Get number of distributed compute nodes (if available)
    pub async fn distributed_nodes_count(&self) -> usize {
        if let Some(ref distributed_manager) = self.distributed_manager {
            distributed_manager.nodes_count().await
        } else {
            0
        }
    }

    /// Check if a metric represents similarity (higher is better) or distance (lower is better)
    pub fn is_similarity_metric(&self, metric: &DistanceMetric) -> bool {
        // Query the actual distance calculator to determine if it's a similarity metric
        let calculator = create_distance_calculator(metric.clone());
        calculator.is_similarity()
    }

    /// Resolve distance metric using hierarchy: request → collection → system default
    pub async fn resolve_distance_metric(
        &self,
        request_metric: Option<DistanceMetric>,
        collection_service: Option<&CollectionService>,
        collection_id: &str,
    ) -> DistanceMetric {
        // 1. Use request override if provided
        if let Some(metric) = request_metric {
            debug!("🎯 Using request-specified distance metric: {:?}", metric);
            return metric;
        }

        // 2. Try to get collection default
        if let Some(service) = collection_service {
            if let Ok(Some(collection)) =
                service.get_collection_by_name_or_uuid(collection_id).await
            {
                // Parse distance metric from string to enum
                let metric = match collection.distance_metric.as_str() {
                    "cosine" => DistanceMetric::Cosine,
                    "euclidean" => DistanceMetric::Euclidean,
                    "manhattan" => DistanceMetric::Manhattan,
                    "dot_product" => DistanceMetric::DotProduct,
                    "hamming" => DistanceMetric::Hamming,
                    "jaccard" => DistanceMetric::Jaccard,
                    other => DistanceMetric::Custom(other.to_string()),
                };
                debug!("🎯 Using collection default distance metric: {:?}", metric);
                return metric;
            }
        }

        // 3. Fall back to system default
        debug!(
            "🎯 Using system default distance metric: {:?}",
            self.system_default
        );
        self.system_default.clone()
    }
}

/// Create a new unified distance manager

/// Trait for components that need distance computation
#[async_trait]
pub trait DistanceComputeProvider {
    /// Get the unified distance compute manager
    fn distance_compute(&self) -> &UnifiedDistanceCompute;

    /// Resolve distance metric with collection context
    async fn resolve_metric(
        &self,
        request_metric: Option<DistanceMetric>,
        collection_id: &str,
    ) -> DistanceMetric {
        self.distance_compute()
            .resolve_distance_metric(request_metric, None, collection_id)
            .await
    }

    /// Calculate distance with automatic metric resolution
    async fn calculate_distance_resolved(
        &self,
        a: &[f32],
        b: &[f32],
        request_metric: Option<DistanceMetric>,
        collection_id: &str,
    ) -> f32 {
        let metric = self.resolve_metric(request_metric, collection_id).await;
        self.distance_compute().calculate_distance(a, b, &metric)
    }

    /// Calculate batch distances with automatic metric resolution
    async fn calculate_distance_batch_resolved(
        &self,
        query: &[f32],
        vectors: &[&[f32]],
        request_metric: Option<DistanceMetric>,
        collection_id: &str,
    ) -> Vec<f32> {
        let metric = self.resolve_metric(request_metric, collection_id).await;
        self.distance_compute()
            .calculate_distance_batch(query, vectors, &metric)
    }
}

/// Configuration for unified distance computation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedDistanceConfig {
    /// System default distance metric
    pub system_default: DistanceMetric,
    /// Enable hardware acceleration
    pub enable_simd: bool,
    /// Maximum batch size for distance calculations
    pub max_batch_size: usize,
    /// Cache size for distance calculators
    pub calculator_cache_size: usize,
}

impl Default for UnifiedDistanceConfig {
    fn default() -> Self {
        Self {
            system_default: DistanceMetric::Cosine,
            enable_simd: true,
            max_batch_size: 1000,
            calculator_cache_size: 16,
        }
    }
}

/// Result ordering helper for search results with unified semantics
pub struct DistanceResultOrdering;

impl DistanceResultOrdering {
    /// Sort results with unified semantics: LOWER values are ALWAYS MORE SIMILAR
    /// 
    /// Since the unified distance system normalizes all metrics to "lower = more similar",
    /// we ALWAYS sort in ascending order regardless of the underlying metric type.
    pub fn sort_results<T>(
        results: &mut Vec<(f32, T)>,
        metric: &DistanceMetric,
        unified_compute: &UnifiedDistanceCompute,
    ) {
        // With unified semantics, we ALWAYS sort ascending (lower = more similar)
        results.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        
        debug!(
            "🔄 Sorted {} results for {} - using unified semantics (lower = more similar)", 
            results.len(),
            unified_compute.metric_behavior_description(metric)
        );
    }

    /// Limit results to top-k
    pub fn limit_results<T>(results: &mut Vec<(f32, T)>, k: usize) {
        if results.len() > k {
            results.truncate(k);
        }
    }

    /// Sort and limit results to top-k with unified semantics
    /// 
    /// Always returns the k MOST SIMILAR results (lowest distance values)
    pub fn sort_and_limit<T>(
        results: &mut Vec<(f32, T)>,
        metric: &DistanceMetric,
        unified_compute: &UnifiedDistanceCompute,
        k: usize,
    ) {
        Self::sort_results(results, metric, unified_compute);
        Self::limit_results(results, k);
        
        debug!(
            "✂️ Limited to top-{} most similar results for {}",
            k,
            unified_compute.metric_behavior_description(metric)
        );
    }
}

/// Distributed distance computation trait for multi-node support
#[async_trait]
pub trait DistributedDistanceCompute: Send + Sync {
    /// Calculate distances across multiple nodes
    async fn calculate_distance_distributed(
        &self,
        query: &[f32],
        node_vectors: &[(&str, &[&[f32]])], // (node_id, vectors)
        metric: &DistanceMetric,
    ) -> Result<Vec<(String, Vec<f32>)>>; // (node_id, distances)

    /// Aggregate results from multiple nodes
    async fn aggregate_distributed_results(
        &self,
        node_results: &[(String, Vec<(f32, String)>)], // (node_id, (distance, vector_id))
        metric: &DistanceMetric,
        k: usize,
    ) -> Result<Vec<(f32, String)>>; // Final top-k results
}

/// Implement distributed distance computation for UnifiedDistanceCompute
#[async_trait]
impl DistributedDistanceCompute for UnifiedDistanceCompute {
    async fn calculate_distance_distributed(
        &self,
        query: &[f32],
        node_vectors: &[(&str, &[&[f32]])],
        metric: &DistanceMetric,
    ) -> Result<Vec<(String, Vec<f32>)>> {
        self.calculate_distance_distributed(query, node_vectors, metric).await
    }

    async fn aggregate_distributed_results(
        &self,
        node_results: &[(String, Vec<(f32, String)>)],
        metric: &DistanceMetric,
        k: usize,
    ) -> Result<Vec<(f32, String)>> {
        self.aggregate_distributed_results(node_results, metric, k).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unified_distance_compute_creation() {
        let compute = UnifiedDistanceCompute::new(DistanceMetric::Cosine);
        assert_eq!(*compute.system_default(), DistanceMetric::Cosine);
    }

    #[test]
    fn test_custom_system_default() {
        let compute = UnifiedDistanceCompute::new(DistanceMetric::Euclidean);
        assert_eq!(*compute.system_default(), DistanceMetric::Euclidean);
    }

    #[test]
    fn test_unified_distance_calculation() {
        let compute = UnifiedDistanceCompute::default();

        let vec_a = vec![1.0, 0.0, 0.0];
        let vec_b = vec![0.0, 1.0, 0.0]; // Orthogonal vectors
        let vec_c = vec![1.0, 0.0, 0.0]; // Identical to vec_a

        // Test Cosine Distance (native distance metric)
        let cosine_distance_ab = compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Cosine);
        let cosine_distance_ac = compute.calculate_distance(&vec_a, &vec_c, &DistanceMetric::Cosine);
        assert!((cosine_distance_ab - 1.0).abs() < 1e-6); // Orthogonal = distance 1.0
        assert!((cosine_distance_ac - 0.0).abs() < 1e-6); // Identical = distance 0.0
        assert!(cosine_distance_ab > cosine_distance_ac); // More distance = less similar

        // Test Euclidean Distance (native distance metric)
        let euclidean_distance_ab = compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Euclidean);
        let euclidean_distance_ac = compute.calculate_distance(&vec_a, &vec_c, &DistanceMetric::Euclidean);
        assert!((euclidean_distance_ab - 1.414214).abs() < 1e-5); // sqrt(2)
        assert!((euclidean_distance_ac - 0.0).abs() < 1e-6); // Identical = distance 0.0
        assert!(euclidean_distance_ab > euclidean_distance_ac); // More distance = less similar

        // Test Dot Product (similarity metric - should be inverted to distance)
        let dot_product_distance_ab = compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::DotProduct);
        let dot_product_distance_ac = compute.calculate_distance(&vec_a, &vec_c, &DistanceMetric::DotProduct);
        // Orthogonal vectors have dot product 0.0 → inverted to distance 1.0
        // Identical vectors have dot product 1.0 → inverted to distance 0.0  
        assert!((dot_product_distance_ab - 1.0).abs() < 1e-6); // Orthogonal
        assert!((dot_product_distance_ac - 0.0).abs() < 1e-6); // Identical
        assert!(dot_product_distance_ab > dot_product_distance_ac); // Unified: lower = more similar
    }

    #[test]
    fn test_dimension_mismatch_handling() {
        let compute = UnifiedDistanceCompute::default();

        let vec_a = vec![1.0, 0.0, 0.0];  // 3 dimensions
        let vec_b = vec![0.0, 1.0];       // 2 dimensions

        // Test dimension mismatch handling for different metric types
        
        // Cosine Distance: Should return maximum distance (2.0) for dimension mismatch
        let cosine_distance = compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Cosine);
        assert_eq!(cosine_distance, 2.0);

        // Euclidean Distance: Should return infinity for dimension mismatch  
        let euclidean_distance = compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Euclidean);
        assert!(euclidean_distance.is_infinite());
        
        // Dot Product: Should return maximum distance (2.0) for dimension mismatch
        let dot_product_distance = compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::DotProduct);
        assert_eq!(dot_product_distance, 2.0);
        
        // Manhattan Distance: Should return infinity for dimension mismatch
        let manhattan_distance = compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Manhattan);
        assert!(manhattan_distance.is_infinite());
        
        // Hamming Distance: Should return maximum discrete distance (1.0)
        let hamming_distance = compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Hamming);
        assert_eq!(hamming_distance, 1.0);
    }

    #[test]
    fn test_similarity_metric_detection() {
        let compute = UnifiedDistanceCompute::default();

        assert!(!compute.is_similarity_metric(&DistanceMetric::Cosine));
        assert!(!compute.is_similarity_metric(&DistanceMetric::Euclidean));
        assert!(compute.is_similarity_metric(&DistanceMetric::DotProduct));
    }

    #[test]
    fn test_unified_result_ordering() {
        let compute = UnifiedDistanceCompute::default();

        // Test unified semantics: ALL metrics should sort by ascending order (lower = more similar)
        
        // Test with Cosine Distance (native distance metric)
        let mut cosine_results = vec![
            (0.5, "vec1".to_string()),  // Medium distance
            (0.9, "vec2".to_string()),  // High distance (least similar)
            (0.1, "vec3".to_string()),  // Low distance (most similar)
        ];

        DistanceResultOrdering::sort_results(
            &mut cosine_results,
            &DistanceMetric::Cosine,
            &compute,
        );

        // With unified semantics: ALWAYS sort ascending (lower = more similar)
        assert_eq!(cosine_results[0].1, "vec3"); // 0.1 (most similar)
        assert_eq!(cosine_results[1].1, "vec1"); // 0.5 (medium)
        assert_eq!(cosine_results[2].1, "vec2"); // 0.9 (least similar)

        // Test with normalized Dot Product (similarity metric converted to distance)
        // Note: These should be the NORMALIZED distance values after inversion
        let mut normalized_dot_product_results = vec![
            (0.5, "vec1".to_string()),  // Normalized distance value
            (0.1, "vec2".to_string()),  // Lower normalized distance (more similar)
            (0.9, "vec3".to_string()),  // Higher normalized distance (less similar)
        ];

        DistanceResultOrdering::sort_results(
            &mut normalized_dot_product_results,
            &DistanceMetric::DotProduct,
            &compute,
        );

        // With unified semantics: ALWAYS sort ascending (lower = more similar)
        assert_eq!(normalized_dot_product_results[0].1, "vec2"); // 0.1 (most similar)
        assert_eq!(normalized_dot_product_results[1].1, "vec1"); // 0.5 (medium)
        assert_eq!(normalized_dot_product_results[2].1, "vec3"); // 0.9 (least similar)
    }

    #[test]
    fn test_similarity_to_distance_inversion() {
        let compute = UnifiedDistanceCompute::default();

        // Test inversion behavior for similarity metrics
        
        // Test standard similarity values [0, 1]
        assert_eq!(compute.invert_similarity_to_distance(1.0), 0.0); // Perfect similarity → zero distance
        assert_eq!(compute.invert_similarity_to_distance(0.5), 0.5); // Medium similarity → medium distance  
        assert_eq!(compute.invert_similarity_to_distance(0.0), 1.0); // Zero similarity → max standard distance
        
        // Test negative similarity values (like negative cosine similarity)
        assert_eq!(compute.invert_similarity_to_distance(-0.5), 1.5); // Negative similarity → distance > 1
        assert_eq!(compute.invert_similarity_to_distance(-1.0), 2.0); // Opposite vectors → max distance
        
        // Test unbounded similarity values (like large dot products)
        assert_eq!(compute.invert_similarity_to_distance(2.0), 0.5); // High similarity → low distance
        assert_eq!(compute.invert_similarity_to_distance(4.0), 0.25); // Very high similarity → very low distance
    }

    #[test]
    fn test_metric_behavior_descriptions() {
        let compute = UnifiedDistanceCompute::default();
        
        assert!(compute.metric_behavior_description(&DistanceMetric::Cosine).contains("native"));
        assert!(compute.metric_behavior_description(&DistanceMetric::Euclidean).contains("native"));
        assert!(compute.metric_behavior_description(&DistanceMetric::DotProduct).contains("inverted"));
        assert!(compute.metric_behavior_description(&DistanceMetric::Custom("test".to_string())).contains("Custom"));
    }

    #[tokio::test]
    async fn test_metric_resolution_hierarchy() {
        let compute = UnifiedDistanceCompute::default();

        // Test request override
        let resolved = compute
            .resolve_distance_metric(Some(DistanceMetric::Euclidean), None, "test_collection")
            .await;
        assert_eq!(resolved, DistanceMetric::Euclidean);

        // Test system default fallback
        let resolved = compute
            .resolve_distance_metric(None, None, "test_collection")
            .await;
        assert_eq!(resolved, DistanceMetric::Cosine);
    }
}
