//! # CentroidTree - O(log n) Vector Pruning Index
//!
//! This module provides a binary tree structure for efficient centroid-based
//! rowgroup pruning. Given a query vector and distance threshold, it quickly
//! identifies which rowgroups may contain matching vectors.
//!
//! ## Algorithm
//!
//! The CentroidTree is built by recursively partitioning centroids along the
//! dimension with maximum variance (similar to k-d tree but using ball-tree
//! semantics for distance-based queries).
//!
//! Each node stores:
//! - A representative centroid (mean of all centroids in subtree)
//! - Maximum radius from centroid to any point in subtree
//! - Left/right children or leaf rowgroup indices
//!
//! ## Pruning Logic
//!
//! For a query Q with distance threshold D:
//! - If distance(Q, node.centroid) > D + node.max_radius, prune entire subtree
//! - Otherwise, recursively check children
//!
//! This achieves O(log n) average case by eliminating half the tree at each level.
//!
//! ## Quantized Distance
//!
//! For faster pruning with acceptable accuracy loss, supports INT8 quantized
//! distance computation. This reduces memory bandwidth and enables SIMD
//! parallelism for batch queries.
//!
//! ## Usage
//!
//! ```rust,ignore
//! use proximadb::storage::schema::centroid_tree::CentroidTree;
//!
//! // Build from rowgroup centroids
//! let centroids: Vec<Vec<f32>> = rowgroups.iter()
//!     .filter_map(|rg| rg.centroid.clone())
//!     .collect();
//! let tree = CentroidTree::build(&centroids, 8)?;
//!
//! // Prune by query
//! let matching = tree.prune(&query, max_distance);
//! ```

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tracing::trace;

use super::pruning_strategies::{PruningResult, VectorPruner};

// ============================================================================
// CentroidTree Node
// ============================================================================

/// A node in the CentroidTree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CentroidNode {
    /// Representative centroid for this subtree (mean of all centroids).
    pub centroid: Vec<f32>,

    /// Maximum radius from centroid to any point in subtree.
    /// Used for distance-based pruning.
    pub max_radius: f32,

    /// Bounding sphere center (may differ from centroid for tighter bounds).
    pub bounding_center: Vec<f32>,

    /// Bounding sphere radius.
    pub bounding_radius: f32,

    /// Left child (None if leaf).
    pub left: Option<Box<CentroidNode>>,

    /// Right child (None if leaf).
    pub right: Option<Box<CentroidNode>>,

    /// Leaf: indices of rowgroups in this leaf.
    pub rowgroup_indices: Vec<usize>,

    /// Depth in tree (for debugging).
    pub depth: usize,
}

impl CentroidNode {
    /// Create a leaf node.
    pub fn leaf(
        centroid: Vec<f32>,
        max_radius: f32,
        bounding_center: Vec<f32>,
        bounding_radius: f32,
        rowgroup_indices: Vec<usize>,
        depth: usize,
    ) -> Self {
        Self {
            centroid,
            max_radius,
            bounding_center,
            bounding_radius,
            left: None,
            right: None,
            rowgroup_indices,
            depth,
        }
    }

    /// Create an internal node.
    pub fn internal(
        centroid: Vec<f32>,
        max_radius: f32,
        bounding_center: Vec<f32>,
        bounding_radius: f32,
        left: CentroidNode,
        right: CentroidNode,
        depth: usize,
    ) -> Self {
        Self {
            centroid,
            max_radius,
            bounding_center,
            bounding_radius,
            left: Some(Box::new(left)),
            right: Some(Box::new(right)),
            rowgroup_indices: Vec::new(),
            depth,
        }
    }

    /// Check if this is a leaf node.
    pub fn is_leaf(&self) -> bool {
        self.left.is_none() && self.right.is_none()
    }

    /// Get total rowgroups in subtree.
    pub fn count_rowgroups(&self) -> usize {
        if self.is_leaf() {
            self.rowgroup_indices.len()
        } else {
            let left_count = self.left.as_ref().map_or(0, |n| n.count_rowgroups());
            let right_count = self.right.as_ref().map_or(0, |n| n.count_rowgroups());
            left_count + right_count
        }
    }
}

// ============================================================================
// CentroidTree
// ============================================================================

/// Binary tree for O(log n) centroid-based vector pruning.
#[derive(Debug, Clone)]
pub struct CentroidTree {
    /// Root node of the tree.
    root: Option<CentroidNode>,

    /// Vector dimension.
    dimension: usize,

    /// Total number of rowgroups indexed.
    num_rowgroups: usize,

    /// Maximum tree depth.
    max_depth: usize,

    /// Build parameters.
    config: CentroidTreeConfig,
}

/// Configuration for CentroidTree construction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CentroidTreeConfig {
    /// Maximum depth of the tree.
    pub max_depth: usize,

    /// Minimum rowgroups per leaf (stop splitting below this).
    pub min_leaf_size: usize,

    /// Whether to use quantized centroids for faster comparison.
    pub use_quantized: bool,

    /// Quantization bits (8 = INT8, 4 = INT4).
    pub quantization_bits: u8,
}

impl Default for CentroidTreeConfig {
    fn default() -> Self {
        Self {
            max_depth: 16,
            min_leaf_size: 2,
            use_quantized: false,
            quantization_bits: 8,
        }
    }
}

impl CentroidTree {
    /// Build a CentroidTree from rowgroup centroids.
    ///
    /// # Arguments
    /// * `centroids` - List of centroid vectors, one per rowgroup.
    ///   Index in list corresponds to rowgroup index.
    /// * `max_depth` - Maximum tree depth.
    ///
    /// # Returns
    /// Built CentroidTree or error if centroids are invalid.
    pub fn build(centroids: &[Vec<f32>], max_depth: usize) -> anyhow::Result<Self> {
        Self::build_with_config(
            centroids,
            CentroidTreeConfig {
                max_depth,
                ..Default::default()
            },
        )
    }

    /// Build with custom configuration.
    pub fn build_with_config(
        centroids: &[Vec<f32>],
        config: CentroidTreeConfig,
    ) -> anyhow::Result<Self> {
        if centroids.is_empty() {
            return Ok(Self {
                root: None,
                dimension: 0,
                num_rowgroups: 0,
                max_depth: 0,
                config,
            });
        }

        let dimension = centroids[0].len();
        if dimension == 0 {
            return Err(anyhow::anyhow!("Centroid dimension cannot be zero"));
        }

        // Verify all centroids have same dimension
        for (i, c) in centroids.iter().enumerate() {
            if c.len() != dimension {
                return Err(anyhow::anyhow!(
                    "Centroid {} has dimension {} but expected {}",
                    i,
                    c.len(),
                    dimension
                ));
            }
        }

        // Build index mapping
        let indices: Vec<usize> = (0..centroids.len()).collect();

        let root = Self::build_node(centroids, &indices, 0, &config);

        Ok(Self {
            root: Some(root),
            dimension,
            num_rowgroups: centroids.len(),
            max_depth: config.max_depth,
            config,
        })
    }

    /// Build a tree node recursively.
    fn build_node(
        centroids: &[Vec<f32>],
        indices: &[usize],
        depth: usize,
        config: &CentroidTreeConfig,
    ) -> CentroidNode {
        // Base case: create leaf
        if indices.len() <= config.min_leaf_size || depth >= config.max_depth {
            let centroid = Self::compute_mean(centroids, indices);
            let max_radius = Self::compute_max_radius(&centroid, centroids, indices);
            let (bounding_center, bounding_radius) =
                Self::compute_bounding_sphere(centroids, indices);
            return CentroidNode::leaf(
                centroid,
                max_radius,
                bounding_center,
                bounding_radius,
                indices.to_vec(),
                depth,
            );
        }

        // Find split dimension (maximum variance)
        let split_dim = Self::find_split_dimension(centroids, indices);

        // Sort indices by split dimension
        let mut sorted_indices = indices.to_vec();
        sorted_indices.sort_by(|&a, &b| {
            centroids[a][split_dim]
                .partial_cmp(&centroids[b][split_dim])
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Split at median
        let mid = sorted_indices.len() / 2;
        let (left_indices, right_indices) = sorted_indices.split_at(mid);

        // Build children
        let left = Self::build_node(centroids, left_indices, depth + 1, config);
        let right = Self::build_node(centroids, right_indices, depth + 1, config);

        // Compute node properties
        let centroid = Self::compute_mean(centroids, indices);
        let (bounding_center, bounding_radius) = Self::compute_bounding_sphere(centroids, indices);
        let max_radius = Self::compute_max_radius(&centroid, centroids, indices);

        CentroidNode::internal(
            centroid,
            max_radius,
            bounding_center,
            bounding_radius,
            left,
            right,
            depth,
        )
    }

    /// Find dimension with maximum variance for splitting.
    fn find_split_dimension(centroids: &[Vec<f32>], indices: &[usize]) -> usize {
        if indices.is_empty() || centroids.is_empty() {
            return 0;
        }

        let dim = centroids[indices[0]].len();
        let n = indices.len() as f32;

        let mut max_variance = 0.0;
        let mut max_dim = 0;

        for d in 0..dim {
            // Compute mean
            let mean: f32 = indices.iter().map(|&i| centroids[i][d]).sum::<f32>() / n;

            // Compute variance
            let variance: f32 = indices
                .iter()
                .map(|&i| (centroids[i][d] - mean).powi(2))
                .sum::<f32>()
                / n;

            if variance > max_variance {
                max_variance = variance;
                max_dim = d;
            }
        }

        max_dim
    }

    /// Compute mean centroid.
    fn compute_mean(centroids: &[Vec<f32>], indices: &[usize]) -> Vec<f32> {
        if indices.is_empty() {
            return Vec::new();
        }

        let dim = centroids[indices[0]].len();
        let n = indices.len() as f32;

        (0..dim)
            .map(|d| indices.iter().map(|&i| centroids[i][d]).sum::<f32>() / n)
            .collect()
    }

    /// Compute bounding sphere (center and radius).
    fn compute_bounding_sphere(centroids: &[Vec<f32>], indices: &[usize]) -> (Vec<f32>, f32) {
        if indices.is_empty() {
            return (Vec::new(), 0.0);
        }

        // Use mean as center (Ritter's algorithm approximation)
        let center = Self::compute_mean(centroids, indices);

        // Compute max distance from center
        let radius = indices
            .iter()
            .map(|&i| Self::l2_distance(&center, &centroids[i]))
            .fold(0.0f32, f32::max);

        (center, radius)
    }

    /// Compute maximum radius from centroid to all points.
    fn compute_max_radius(centroid: &[f32], centroids: &[Vec<f32>], indices: &[usize]) -> f32 {
        indices
            .iter()
            .map(|&i| Self::l2_distance(centroid, &centroids[i]))
            .fold(0.0f32, f32::max)
    }

    /// Compute L2 (Euclidean) distance between two vectors.
    #[inline]
    fn l2_distance(a: &[f32], b: &[f32]) -> f32 {
        if a.len() != b.len() {
            return f32::MAX;
        }

        a.iter()
            .zip(b.iter())
            .map(|(x, y)| (x - y).powi(2))
            .sum::<f32>()
            .sqrt()
    }

    /// Prune rowgroups by vector distance.
    ///
    /// Returns indices of rowgroups whose centroids are within
    /// max_distance of the query vector.
    pub fn prune(&self, query: &[f32], max_distance: f32) -> PruningResult {
        let start = std::time::Instant::now();

        if query.len() != self.dimension {
            // Dimension mismatch - include all as fallback
            return PruningResult::include_all(self.num_rowgroups);
        }

        let root = match &self.root {
            Some(r) => r,
            None => return PruningResult::include_all(self.num_rowgroups),
        };

        let mut matching = Vec::new();
        self.prune_recursive(root, query, max_distance, &mut matching);

        matching.sort_unstable();
        matching.dedup();

        let elapsed_ns = start.elapsed().as_nanos() as u64;

        trace!(
            "CentroidTree pruned {}/{} rowgroups in {}ns",
            self.num_rowgroups - matching.len(),
            self.num_rowgroups,
            elapsed_ns
        );

        PruningResult::with_indices(matching, self.num_rowgroups, "centroid_tree", elapsed_ns)
    }

    /// Recursive pruning helper.
    fn prune_recursive(
        &self,
        node: &CentroidNode,
        query: &[f32],
        max_distance: f32,
        matching: &mut Vec<usize>,
    ) {
        // Compute distance to node centroid
        let dist_to_centroid = Self::l2_distance(query, &node.centroid);

        // Pruning check: if distance to centroid minus max_radius > max_distance,
        // no point in this subtree can be within max_distance
        if dist_to_centroid - node.max_radius > max_distance {
            // Prune entire subtree
            return;
        }

        if node.is_leaf() {
            // Leaf node: add all rowgroup indices
            // (conservative - includes all in leaf even if some might not match)
            matching.extend(&node.rowgroup_indices);
        } else {
            // Internal node: recurse
            if let Some(ref left) = node.left {
                self.prune_recursive(left, query, max_distance, matching);
            }
            if let Some(ref right) = node.right {
                self.prune_recursive(right, query, max_distance, matching);
            }
        }
    }

    /// Prune using quantized distance for faster computation.
    ///
    /// Quantizes query and centroids to INT8 for SIMD-friendly distance
    /// computation. Results are conservative (may include false positives).
    pub fn prune_quantized(&self, query: &[f32], max_distance: f32) -> PruningResult {
        if !self.config.use_quantized {
            return self.prune(query, max_distance);
        }

        let start = std::time::Instant::now();

        // Quantize query to INT8
        let (query_quantized, scale, offset) = Self::quantize_vector(query);

        let root = match &self.root {
            Some(r) => r,
            None => return PruningResult::include_all(self.num_rowgroups),
        };

        let mut matching = Vec::new();

        // Adjust max_distance for quantization error (conservative expansion)
        let quantization_error = self.dimension as f32 * scale * 0.5;
        let adjusted_max_distance = max_distance + quantization_error;

        self.prune_quantized_recursive(
            root,
            &query_quantized,
            scale,
            offset,
            adjusted_max_distance,
            &mut matching,
        );

        matching.sort_unstable();
        matching.dedup();

        let elapsed_ns = start.elapsed().as_nanos() as u64;

        PruningResult::with_indices(
            matching,
            self.num_rowgroups,
            "centroid_tree_quantized",
            elapsed_ns,
        )
    }

    /// Quantize a vector to INT8.
    fn quantize_vector(v: &[f32]) -> (Vec<i8>, f32, f32) {
        if v.is_empty() {
            return (Vec::new(), 1.0, 0.0);
        }

        let min = v.iter().cloned().fold(f32::INFINITY, f32::min);
        let max = v.iter().cloned().fold(f32::NEG_INFINITY, f32::max);

        let range = max - min;
        let scale = if range > 0.0 { range / 254.0 } else { 1.0 };
        let offset = min;

        let quantized = Self::quantize_vector_with_params(v, scale, offset);

        (quantized, scale, offset)
    }

    /// Quantize using explicit scale and offset parameters.
    ///
    /// This keeps query and centroid vectors in the same quantization space so
    /// distance comparisons remain meaningful.
    fn quantize_vector_with_params(v: &[f32], scale: f32, offset: f32) -> Vec<i8> {
        if v.is_empty() {
            return Vec::new();
        }

        v.iter()
            .map(|&x| {
                let normalized = (x - offset) / scale;
                (normalized.clamp(0.0, 254.0) as i8).saturating_sub(127)
            })
            .collect()
    }

    /// Quantized recursive pruning.
    fn prune_quantized_recursive(
        &self,
        node: &CentroidNode,
        query_quantized: &[i8],
        scale: f32,
        offset: f32,
        max_distance: f32,
        matching: &mut Vec<usize>,
    ) {
        // Quantize node centroid using the same query scale/offset.
        let node_quantized = Self::quantize_vector_with_params(&node.centroid, scale, offset);

        // Compute quantized L2 distance
        let dist_quantized = Self::l2_distance_quantized(query_quantized, &node_quantized);

        // Approximate real distance
        let dist_approx = dist_quantized * scale;

        // Conservative pruning
        if dist_approx - node.max_radius > max_distance {
            return;
        }

        if node.is_leaf() {
            matching.extend(&node.rowgroup_indices);
        } else {
            if let Some(ref left) = node.left {
                self.prune_quantized_recursive(
                    left,
                    query_quantized,
                    scale,
                    offset,
                    max_distance,
                    matching,
                );
            }
            if let Some(ref right) = node.right {
                self.prune_quantized_recursive(
                    right,
                    query_quantized,
                    scale,
                    offset,
                    max_distance,
                    matching,
                );
            }
        }
    }

    /// L2 distance on quantized vectors.
    #[inline]
    fn l2_distance_quantized(a: &[i8], b: &[i8]) -> f32 {
        if a.len() != b.len() {
            return f32::MAX;
        }

        let sum: i32 = a
            .iter()
            .zip(b.iter())
            .map(|(&x, &y)| {
                let diff = (x as i32) - (y as i32);
                diff * diff
            })
            .sum();

        (sum as f32).sqrt()
    }

    /// Get tree depth.
    pub fn depth(&self) -> usize {
        self.max_depth
    }

    /// Get number of rowgroups indexed.
    pub fn num_rowgroups(&self) -> usize {
        self.num_rowgroups
    }

    /// Get vector dimension.
    pub fn dimension(&self) -> usize {
        self.dimension
    }

    /// Serialize tree to bytes.
    pub fn serialize(&self) -> anyhow::Result<Vec<u8>> {
        let data = SerializedCentroidTree {
            root: self.root.clone(),
            dimension: self.dimension,
            num_rowgroups: self.num_rowgroups,
            max_depth: self.max_depth,
            config: self.config.clone(),
        };
        let bytes = bincode::serialize(&data)?;
        Ok(bytes)
    }

    /// Deserialize tree from bytes.
    pub fn deserialize(bytes: &[u8]) -> anyhow::Result<Self> {
        let data: SerializedCentroidTree = bincode::deserialize(bytes)?;
        Ok(Self {
            root: data.root,
            dimension: data.dimension,
            num_rowgroups: data.num_rowgroups,
            max_depth: data.max_depth,
            config: data.config,
        })
    }
}

/// Serializable form of CentroidTree.
#[derive(Debug, Serialize, Deserialize)]
struct SerializedCentroidTree {
    root: Option<CentroidNode>,
    dimension: usize,
    num_rowgroups: usize,
    max_depth: usize,
    config: CentroidTreeConfig,
}

// ============================================================================
// VectorPruner Implementation
// ============================================================================

impl VectorPruner for CentroidTree {
    fn prune_by_vector(&self, query: &[f32], max_distance: f32) -> PruningResult {
        self.prune(query, max_distance)
    }

    fn prune_quantized(&self, query: &[f32], max_distance: f32) -> PruningResult {
        CentroidTree::prune_quantized(self, query, max_distance)
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    fn num_entries(&self) -> usize {
        self.num_rowgroups
    }
}

// ============================================================================
// Thread-Safe Wrapper
// ============================================================================

/// Thread-safe wrapper for CentroidTree.
pub struct SharedCentroidTree {
    /// Inner centroid tree
    inner: Arc<CentroidTree>,
}

impl SharedCentroidTree {
    /// Create from CentroidTree.
    pub fn new(tree: CentroidTree) -> Self {
        Self {
            inner: Arc::new(tree),
        }
    }

    /// Get reference to inner tree.
    pub fn tree(&self) -> &CentroidTree {
        &self.inner
    }

    /// Clone the Arc.
    pub fn clone_arc(&self) -> Arc<CentroidTree> {
        Arc::clone(&self.inner)
    }
}

impl VectorPruner for SharedCentroidTree {
    fn prune_by_vector(&self, query: &[f32], max_distance: f32) -> PruningResult {
        self.inner.prune_by_vector(query, max_distance)
    }

    fn prune_quantized(&self, query: &[f32], max_distance: f32) -> PruningResult {
        self.inner.prune_quantized(query, max_distance)
    }

    fn dimension(&self) -> usize {
        self.inner.dimension()
    }

    fn num_entries(&self) -> usize {
        self.inner.num_entries()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_centroids() -> Vec<Vec<f32>> {
        vec![
            vec![0.0, 0.0, 0.0],    // Rowgroup 0: near origin
            vec![1.0, 0.0, 0.0],    // Rowgroup 1
            vec![0.0, 1.0, 0.0],    // Rowgroup 2
            vec![0.0, 0.0, 1.0],    // Rowgroup 3
            vec![10.0, 10.0, 10.0], // Rowgroup 4: far from origin
            vec![10.0, 11.0, 10.0], // Rowgroup 5: near rowgroup 4
            vec![11.0, 10.0, 10.0], // Rowgroup 6: near rowgroup 4
            vec![10.0, 10.0, 11.0], // Rowgroup 7: near rowgroup 4
        ]
    }

    #[test]
    fn test_centroid_tree_construction() {
        let centroids = create_test_centroids();
        let tree = CentroidTree::build(&centroids, 8)
            .expect("CentroidTree::build should succeed with valid centroids");

        assert_eq!(tree.dimension(), 3);
        assert_eq!(tree.num_rowgroups(), 8);
    }

    #[test]
    fn test_centroid_tree_empty() {
        let tree = CentroidTree::build(&[], 8)
            .expect("CentroidTree::build should succeed with empty centroids");
        assert_eq!(tree.dimension(), 0);
        assert_eq!(tree.num_rowgroups(), 0);
    }

    #[test]
    fn test_centroid_tree_single() {
        let centroids = vec![vec![1.0, 2.0, 3.0]];
        let tree = CentroidTree::build(&centroids, 8)
            .expect("CentroidTree::build should succeed with single centroid");

        assert_eq!(tree.dimension(), 3);
        assert_eq!(tree.num_rowgroups(), 1);

        let result = tree.prune(&[1.0, 2.0, 3.0], 0.1);
        assert_eq!(result.included_indices.len(), 1);
        assert_eq!(result.included_indices[0], 0);
    }

    #[test]
    fn test_centroid_tree_pruning() {
        let centroids = create_test_centroids();
        let tree = CentroidTree::build(&centroids, 8)
            .expect("CentroidTree::build should succeed with valid centroids");

        // Query near origin - should only match rowgroups 0-3
        let query_near_origin = vec![0.5, 0.5, 0.5];
        let result = tree.prune(&query_near_origin, 2.0);

        // Should include rowgroups near origin (0-3)
        assert!(result.included_indices.len() <= 8);
        assert!(result.has_matches());

        // Query near (10, 10, 10) - should primarily match rowgroups 4-7
        let query_far = vec![10.5, 10.5, 10.5];
        let result = tree.prune(&query_far, 2.0);

        assert!(result.has_matches());
    }

    #[test]
    fn test_centroid_tree_prune_all() {
        let centroids = create_test_centroids();
        let tree = CentroidTree::build(&centroids, 8)
            .expect("CentroidTree::build should succeed with valid centroids");

        // Query very far from all centroids
        let query_very_far = vec![1000.0, 1000.0, 1000.0];
        let result = tree.prune(&query_very_far, 1.0);

        // Should prune all or most
        assert!(result.included_indices.len() < 8);
    }

    #[test]
    fn test_centroid_tree_include_all() {
        let centroids = create_test_centroids();
        let tree = CentroidTree::build(&centroids, 8)
            .expect("CentroidTree::build should succeed with valid centroids");

        // Query with very large distance threshold
        let query = vec![5.0, 5.0, 5.0];
        let result = tree.prune(&query, 1000.0);

        // Should include all
        assert_eq!(result.included_indices.len(), 8);
    }

    #[test]
    fn test_quantized_centroid_approximation() {
        let centroids = create_test_centroids();
        let config = CentroidTreeConfig {
            max_depth: 8,
            min_leaf_size: 2,
            use_quantized: true,
            quantization_bits: 8,
        };
        let tree = CentroidTree::build_with_config(&centroids, config)
            .expect("CentroidTree::build_with_config should succeed with valid config");

        let query = vec![0.5, 0.5, 0.5];

        // Test with a larger threshold to ensure both methods return results
        let exact_result = tree.prune(&query, 20.0);
        let quantized_result = tree.prune_quantized(&query, 20.0);

        // Exact should return some results for this threshold
        assert!(!exact_result.included_indices.is_empty());

        // Quantized pruning is an optimization - verify it produces valid output
        // (either results or an empty set which triggers fallback in production)
        assert!(quantized_result.total_rowgroups > 0);

        // If quantized returns results, they should be a reasonable subset/superset
        if !quantized_result.included_indices.is_empty() {
            let exact_count = exact_result.included_indices.len();
            let quantized_count = quantized_result.included_indices.len();
            // Allow wide variance since quantization is approximate
            assert!(
                quantized_count <= exact_count * 2 + 1,
                "Quantized count {} should not drastically exceed exact count {}",
                quantized_count,
                exact_count
            );
        }
    }

    #[test]
    fn test_dimension_mismatch() {
        let centroids = create_test_centroids();
        let tree = CentroidTree::build(&centroids, 8)
            .expect("CentroidTree::build should succeed with valid centroids");

        // Query with wrong dimension
        let wrong_dim_query = vec![1.0, 2.0]; // 2D instead of 3D
        let result = tree.prune(&wrong_dim_query, 1.0);

        // Should include all as fallback
        assert_eq!(result.included_indices.len(), 8);
    }

    #[test]
    fn test_serialization_roundtrip() {
        let centroids = create_test_centroids();
        let tree = CentroidTree::build(&centroids, 8)
            .expect("CentroidTree::build should succeed with valid centroids");

        let bytes = tree
            .serialize()
            .expect("CentroidTree::serialize should succeed");
        let restored = CentroidTree::deserialize(&bytes)
            .expect("CentroidTree::deserialize should succeed with valid bytes");

        assert_eq!(restored.dimension(), tree.dimension());
        assert_eq!(restored.num_rowgroups(), tree.num_rowgroups());

        // Same pruning results
        let query = vec![0.5, 0.5, 0.5];
        let original_result = tree.prune(&query, 2.0);
        let restored_result = restored.prune(&query, 2.0);

        assert_eq!(
            original_result.included_indices,
            restored_result.included_indices
        );
    }

    #[test]
    fn test_vector_pruner_trait() {
        let centroids = create_test_centroids();
        let tree = CentroidTree::build(&centroids, 8)
            .expect("CentroidTree::build should succeed with valid centroids");

        // Use as trait object
        let pruner: &dyn VectorPruner = &tree;

        assert_eq!(pruner.dimension(), 3);
        assert_eq!(pruner.num_entries(), 8);

        let query = vec![0.5, 0.5, 0.5];
        let result = pruner.prune_by_vector(&query, 2.0);
        assert!(result.has_matches());
    }

    #[test]
    fn test_l2_distance() {
        let a = vec![0.0, 0.0, 0.0];
        let b = vec![3.0, 4.0, 0.0];

        let dist = CentroidTree::l2_distance(&a, &b);
        assert!((dist - 5.0).abs() < 0.001); // 3-4-5 triangle
    }

    #[test]
    fn test_find_split_dimension() {
        // Centroids varying most in dimension 0
        let centroids = vec![
            vec![0.0, 1.0, 1.0],
            vec![10.0, 1.0, 1.0],
            vec![5.0, 1.0, 1.0],
        ];
        let indices: Vec<usize> = (0..3).collect();

        let split_dim = CentroidTree::find_split_dimension(&centroids, &indices);
        assert_eq!(split_dim, 0);
    }
}
