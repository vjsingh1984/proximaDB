//! Spatial Pruning Infrastructure for Block Selection
//!
//! This module provides unified block-level pruning for all storage engines
//! (SST, HELIX, SWIFT) using spatial codes from space-filling curves.
//!
//! # Pruning Modes
//!
//! - **Sqrt**: Select max(3, sqrt(n)) blocks - good balance of recall and speed
//! - **Ratio**: Select a percentage of blocks
//! - **Fixed**: Select a fixed number of blocks
//!
//! # Usage
//!
//! ```rust,ignore
//! use proximadb::storage::engines::core::formats::proximablocks::spatial_pruning::{
//!     SpatialPruner, PruningConfig, PruningMode,
//! };
//!
//! let config = PruningConfig::sqrt_mode();
//! let pruner = SpatialPruner::new(encoder, config);
//!
//! // During search, select blocks to scan
//! let selected = pruner.select_blocks(&query_code, &block_codes, &block_centroids, &query_vector);
//! ```

use std::cmp::Ordering;

use super::spatial_encoding::SpatialCode;
use super::spatial_traits::CurveType;
use crate::compute::distance_computation::DistanceMetric;

/// Pruning mode for block selection
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PruningMode {
    /// Select max(min_blocks, sqrt(n)) blocks
    /// Good balance of recall and performance
    Sqrt {
        /// Minimum blocks to select (default: 3)
        min_blocks: usize,
    },
    /// Select a ratio of total blocks
    Ratio {
        /// Ratio of blocks to select (0.0-1.0)
        ratio: f32,
        /// Minimum blocks to select
        min_blocks: usize,
    },
    /// Select a fixed number of blocks
    Fixed {
        /// Number of blocks to select
        k: usize,
    },
    /// Select all blocks (no pruning, exact search)
    Exact,
}

impl Default for PruningMode {
    fn default() -> Self {
        Self::Sqrt { min_blocks: 3 }
    }
}

impl PruningMode {
    /// Calculate number of blocks to select
    pub fn num_blocks_to_select(&self, total_blocks: usize) -> usize {
        match self {
            PruningMode::Sqrt { min_blocks } => {
                let sqrt_blocks = (total_blocks as f32).sqrt().ceil() as usize;
                (*min_blocks).max(sqrt_blocks).min(total_blocks)
            }
            PruningMode::Ratio { ratio, min_blocks } => {
                let ratio_blocks = (total_blocks as f32 * ratio) as usize;
                (*min_blocks).max(ratio_blocks).min(total_blocks)
            }
            PruningMode::Fixed { k } => (*k).min(total_blocks),
            PruningMode::Exact => total_blocks,
        }
    }
}

/// Configuration for spatial pruning
#[derive(Debug, Clone)]
pub struct PruningConfig {
    /// Pruning mode
    pub mode: PruningMode,
    /// Use centroid distance as secondary ranking
    pub use_centroid_distance: bool,
    /// Weight for spatial code distance (0.0-1.0)
    pub spatial_weight: f32,
    /// Weight for centroid distance (0.0-1.0)
    pub centroid_weight: f32,
    /// Distance metric for centroid comparison
    pub distance_metric: DistanceMetric,
}

impl Default for PruningConfig {
    fn default() -> Self {
        Self {
            mode: PruningMode::default(),
            use_centroid_distance: true,
            spatial_weight: 0.7,
            centroid_weight: 0.3,
            distance_metric: DistanceMetric::Euclidean,
        }
    }
}

impl PruningConfig {
    /// Create sqrt mode configuration
    pub fn sqrt_mode() -> Self {
        Self {
            mode: PruningMode::Sqrt { min_blocks: 3 },
            ..Default::default()
        }
    }

    /// Create ratio mode configuration
    pub fn ratio_mode(ratio: f32) -> Self {
        Self {
            mode: PruningMode::Ratio {
                ratio,
                min_blocks: 3,
            },
            ..Default::default()
        }
    }

    /// Create fixed mode configuration
    pub fn fixed_mode(k: usize) -> Self {
        Self {
            mode: PruningMode::Fixed { k },
            ..Default::default()
        }
    }

    /// Create exact mode (no pruning)
    pub fn exact_mode() -> Self {
        Self {
            mode: PruningMode::Exact,
            ..Default::default()
        }
    }

    /// Create configuration for a specific curve type
    pub fn for_curve(curve_type: CurveType) -> Self {
        match curve_type {
            CurveType::ZOrder => Self {
                mode: PruningMode::Sqrt { min_blocks: 3 },
                spatial_weight: 0.6,
                centroid_weight: 0.4,
                ..Default::default()
            },
            CurveType::Hilbert => Self {
                mode: PruningMode::Sqrt { min_blocks: 3 },
                spatial_weight: 0.75,
                centroid_weight: 0.25,
                ..Default::default()
            },
            CurveType::AdaCurve => Self {
                mode: PruningMode::Sqrt { min_blocks: 3 },
                spatial_weight: 0.7,
                centroid_weight: 0.3,
                ..Default::default()
            },
        }
    }
}

/// Block information for pruning
#[derive(Debug, Clone)]
pub struct BlockPruningInfo {
    /// Block index
    pub index: usize,
    /// Spatial code of block centroid
    pub spatial_code: SpatialCode,
    /// Block centroid (optional, for secondary ranking)
    pub centroid: Option<Vec<f32>>,
}

impl BlockPruningInfo {
    /// Create from spatial code only
    pub fn from_code(index: usize, spatial_code: SpatialCode) -> Self {
        Self {
            index,
            spatial_code,
            centroid: None,
        }
    }

    /// Create with centroid
    pub fn with_centroid(index: usize, spatial_code: SpatialCode, centroid: Vec<f32>) -> Self {
        Self {
            index,
            spatial_code,
            centroid: Some(centroid),
        }
    }
}

/// Backwards-compat alias for [`SpatialPruningResult`].
pub type PruningResult = SpatialPruningResult;

/// Result of block selection
#[derive(Debug)]
pub struct SpatialPruningResult {
    /// Selected block indices (in priority order)
    pub selected_indices: Vec<usize>,
    /// Number of blocks pruned
    pub pruned_count: usize,
    /// Total blocks considered
    pub total_blocks: usize,
    /// Pruning ratio (0.0-1.0)
    pub pruning_ratio: f32,
}

/// Unified spatial pruner for block selection
///
/// Works with any SpatialCurveEncoder implementation to provide
/// consistent pruning across SST, HELIX, and SWIFT engines.
pub struct SpatialPruner {
    /// Configuration
    config: PruningConfig,
}

impl SpatialPruner {
    /// Create a new spatial pruner
    pub fn new(config: PruningConfig) -> Self {
        Self { config }
    }

    /// Select blocks to search based on spatial codes and centroids
    ///
    /// # Arguments
    /// * `query_code` - Spatial code of the query vector
    /// * `query_vector` - The query vector (for centroid distance)
    /// * `blocks` - Block information including spatial codes and centroids
    ///
    /// # Returns
    /// Pruning result with selected block indices
    pub fn select_blocks(
        &self,
        query_code: &SpatialCode,
        query_vector: &[f32],
        blocks: &[BlockPruningInfo],
    ) -> SpatialPruningResult {
        let total_blocks = blocks.len();

        if total_blocks == 0 {
            return SpatialPruningResult {
                selected_indices: Vec::new(),
                pruned_count: 0,
                total_blocks: 0,
                pruning_ratio: 0.0,
            };
        }

        let num_to_select = self.config.mode.num_blocks_to_select(total_blocks);

        if num_to_select >= total_blocks {
            // Select all blocks
            return SpatialPruningResult {
                selected_indices: (0..total_blocks).collect(),
                pruned_count: 0,
                total_blocks,
                pruning_ratio: 0.0,
            };
        }

        // Score each block
        let mut scored_blocks: Vec<(f32, usize)> = blocks
            .iter()
            .map(|block| {
                let score = self.score_block(query_code, query_vector, block);
                (score, block.index)
            })
            .collect();

        // Sort by score (lower is better)
        scored_blocks.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(Ordering::Equal));

        // Select top N
        let selected_indices: Vec<usize> = scored_blocks
            .into_iter()
            .take(num_to_select)
            .map(|(_, idx)| idx)
            .collect();

        let pruned_count = total_blocks - selected_indices.len();

        SpatialPruningResult {
            selected_indices,
            pruned_count,
            total_blocks,
            pruning_ratio: pruned_count as f32 / total_blocks as f32,
        }
    }

    /// Score a block (lower is better)
    fn score_block(
        &self,
        query_code: &SpatialCode,
        query_vector: &[f32],
        block: &BlockPruningInfo,
    ) -> f32 {
        // Spatial code distance
        let spatial_distance = self.spatial_code_distance(query_code, &block.spatial_code);

        // Centroid distance (if available and configured)
        let centroid_distance = if self.config.use_centroid_distance {
            if let Some(ref centroid) = block.centroid {
                if centroid.len() == query_vector.len() {
                    self.vector_distance(query_vector, centroid)
                } else {
                    0.0 // Dimension mismatch, use 0 (include block)
                }
            } else {
                0.0 // No centroid, rely on spatial code only
            }
        } else {
            0.0
        };

        // Combine scores
        self.config.spatial_weight * spatial_distance
            + self.config.centroid_weight * centroid_distance
    }

    /// Calculate normalized spatial code distance
    fn spatial_code_distance(&self, a: &SpatialCode, b: &SpatialCode) -> f32 {
        match (a, b) {
            (SpatialCode::Code64(a), SpatialCode::Code64(b)) => {
                let diff = a.abs_diff(*b);
                // Normalize to [0, 1]
                diff as f32 / u64::MAX as f32 * 1000.0
            }
            (SpatialCode::Code128(a), SpatialCode::Code128(b)) => {
                let diff = a.abs_diff(*b);
                diff as f32 / u128::MAX as f32 * 1000.0
            }
            (
                SpatialCode::Code256 { low: _, high: ah },
                SpatialCode::Code256 { low: _, high: bh },
            ) => {
                // Use high bits for distance approximation
                let high_diff = ah.abs_diff(*bh);
                high_diff as f32 / u128::MAX as f32 * 1000.0
            }
            (SpatialCode::Code512(a), SpatialCode::Code512(b)) => {
                // Use most significant part
                let diff = a.parts[3].abs_diff(b.parts[3]);
                diff as f32 / u128::MAX as f32 * 1000.0
            }
            _ => 0.0, // Type mismatch
        }
    }

    /// Calculate vector distance
    fn vector_distance(&self, a: &[f32], b: &[f32]) -> f32 {
        match self.config.distance_metric {
            DistanceMetric::Euclidean => {
                let sum: f32 = a.iter().zip(b.iter()).map(|(x, y)| (x - y).powi(2)).sum();
                sum.sqrt()
            }
            DistanceMetric::Cosine => {
                let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
                let norm_a: f32 = a.iter().map(|x| x.powi(2)).sum::<f32>().sqrt();
                let norm_b: f32 = b.iter().map(|x| x.powi(2)).sum::<f32>().sqrt();
                if norm_a > 0.0 && norm_b > 0.0 {
                    1.0 - (dot / (norm_a * norm_b))
                } else {
                    1.0
                }
            }
            DistanceMetric::DotProduct => {
                // Negative dot product (higher dot = more similar = lower score)
                let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
                -dot
            }
            _ => {
                // Default to Euclidean for other metrics
                let sum: f32 = a.iter().zip(b.iter()).map(|(x, y)| (x - y).powi(2)).sum();
                sum.sqrt()
            }
        }
    }

    /// Quick selection using only spatial codes (faster, less accurate)
    pub fn select_blocks_by_code(
        &self,
        query_code: &SpatialCode,
        block_codes: &[SpatialCode],
    ) -> Vec<usize> {
        let total_blocks = block_codes.len();

        if total_blocks == 0 {
            return Vec::new();
        }

        let num_to_select = self.config.mode.num_blocks_to_select(total_blocks);

        if num_to_select >= total_blocks {
            return (0..total_blocks).collect();
        }

        // Score by spatial code distance
        let mut scored: Vec<(f32, usize)> = block_codes
            .iter()
            .enumerate()
            .map(|(idx, code)| (self.spatial_code_distance(query_code, code), idx))
            .collect();

        scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(Ordering::Equal));

        scored
            .into_iter()
            .take(num_to_select)
            .map(|(_, idx)| idx)
            .collect()
    }

    /// Get the configured pruning mode
    pub fn mode(&self) -> &PruningMode {
        &self.config.mode
    }

    /// Get the configuration
    pub fn config(&self) -> &PruningConfig {
        &self.config
    }
}

/// Create a pruner for a specific curve type with defaults
pub fn create_pruner_for_curve(curve_type: CurveType) -> SpatialPruner {
    SpatialPruner::new(PruningConfig::for_curve(curve_type))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pruning_mode_sqrt() {
        let mode = PruningMode::Sqrt { min_blocks: 3 };

        assert_eq!(mode.num_blocks_to_select(1), 1);
        assert_eq!(mode.num_blocks_to_select(4), 3); // sqrt(4)=2, min=3
        assert_eq!(mode.num_blocks_to_select(9), 3); // sqrt(9)=3
        assert_eq!(mode.num_blocks_to_select(100), 10); // sqrt(100)=10
        assert_eq!(mode.num_blocks_to_select(1000), 32); // sqrt(1000)~31.6 -> 32
    }

    #[test]
    fn test_pruning_mode_ratio() {
        let mode = PruningMode::Ratio {
            ratio: 0.1,
            min_blocks: 3,
        };

        assert_eq!(mode.num_blocks_to_select(10), 3); // 10*0.1=1, min=3
        assert_eq!(mode.num_blocks_to_select(100), 10); // 100*0.1=10
        assert_eq!(mode.num_blocks_to_select(1000), 100); // 1000*0.1=100
    }

    #[test]
    fn test_pruning_mode_fixed() {
        let mode = PruningMode::Fixed { k: 5 };

        assert_eq!(mode.num_blocks_to_select(3), 3); // min(5, 3)
        assert_eq!(mode.num_blocks_to_select(10), 5);
        assert_eq!(mode.num_blocks_to_select(100), 5);
    }

    #[test]
    fn test_spatial_pruner_creation() {
        let pruner = SpatialPruner::new(PruningConfig::sqrt_mode());
        assert!(matches!(pruner.mode(), PruningMode::Sqrt { .. }));

        let pruner = SpatialPruner::new(PruningConfig::fixed_mode(10));
        assert!(matches!(pruner.mode(), PruningMode::Fixed { k: 10 }));
    }

    #[test]
    fn test_select_blocks_by_code() {
        let config = PruningConfig::fixed_mode(2);
        let pruner = SpatialPruner::new(config);

        let query_code = SpatialCode::Code64(100);
        let block_codes = vec![
            SpatialCode::Code64(50),  // distance 50
            SpatialCode::Code64(200), // distance 100
            SpatialCode::Code64(110), // distance 10 (closest)
            SpatialCode::Code64(500), // distance 400
        ];

        let selected = pruner.select_blocks_by_code(&query_code, &block_codes);

        assert_eq!(selected.len(), 2);
        assert!(selected.contains(&2)); // Closest
        assert!(selected.contains(&0)); // Second closest
    }

    #[test]
    fn test_select_blocks_with_centroids() {
        let config = PruningConfig {
            mode: PruningMode::Fixed { k: 2 },
            use_centroid_distance: true,
            spatial_weight: 0.5,
            centroid_weight: 0.5,
            distance_metric: DistanceMetric::Euclidean,
        };
        let pruner = SpatialPruner::new(config);

        let query_code = SpatialCode::Code64(100);
        let query_vector = vec![1.0, 0.0, 0.0, 0.0];

        let blocks = vec![
            BlockPruningInfo::with_centroid(0, SpatialCode::Code64(50), vec![0.9, 0.1, 0.0, 0.0]),
            BlockPruningInfo::with_centroid(1, SpatialCode::Code64(200), vec![0.5, 0.5, 0.0, 0.0]),
            BlockPruningInfo::with_centroid(
                2,
                SpatialCode::Code64(110),
                vec![0.95, 0.05, 0.0, 0.0],
            ),
        ];

        let result = pruner.select_blocks(&query_code, &query_vector, &blocks);

        assert_eq!(result.selected_indices.len(), 2);
        assert_eq!(result.pruned_count, 1);
        assert!((result.pruning_ratio - 0.333).abs() < 0.01);
    }

    #[test]
    fn test_create_pruner_for_curve() {
        let zorder_pruner = create_pruner_for_curve(CurveType::ZOrder);
        assert_eq!(zorder_pruner.config().spatial_weight, 0.6);

        let hilbert_pruner = create_pruner_for_curve(CurveType::Hilbert);
        assert_eq!(hilbert_pruner.config().spatial_weight, 0.75);
    }

    #[test]
    fn test_exact_mode_no_pruning() {
        let config = PruningConfig::exact_mode();
        let pruner = SpatialPruner::new(config);

        let query_code = SpatialCode::Code64(100);
        let block_codes = vec![
            SpatialCode::Code64(50),
            SpatialCode::Code64(200),
            SpatialCode::Code64(300),
        ];

        let selected = pruner.select_blocks_by_code(&query_code, &block_codes);

        // Should select all blocks
        assert_eq!(selected.len(), 3);
    }
}
