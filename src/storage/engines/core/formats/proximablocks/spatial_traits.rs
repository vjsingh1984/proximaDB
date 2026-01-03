//! Spatial Curve Encoder Trait for Unified Block Clustering
//!
//! This module defines the `SpatialCurveEncoder` trait that abstracts over
//! different space-filling curve implementations:
//!
//! - **SST Engine**: Z-order (Morton) curve - fast, simple
//! - **HELIX Engine**: Hilbert curve - better locality, orthogonal traversal
//! - **SWIFT Engine**: AdaCurve - learned, data-adaptive curves
//!
//! Each engine implements this trait with its own curve type while sharing
//! the common pruning and encoding infrastructure.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │              SpatialCurveEncoder Trait                      │
//! │  + encode(pca_coords) → SpatialCode                         │
//! │  + encode_batch(pca_coords[]) → SpatialCode[]               │
//! │  + compute_epsilon(n_blocks, selectivity) → SpatialCode     │
//! │  + in_range(block_code, query_code, epsilon) → bool         │
//! └─────────────────────────────────────────────────────────────┘
//!                              │
//!          ┌───────────────────┼───────────────────┐
//!          ▼                   ▼                   ▼
//!    ┌───────────┐       ┌───────────┐       ┌───────────┐
//!    │  Z-Order  │       │  Hilbert  │       │ AdaCurve  │
//!    │  Encoder  │       │  Encoder  │       │  Encoder  │
//!    │   (SST)   │       │  (HELIX)  │       │  (SWIFT)  │
//!    └───────────┘       └───────────┘       └───────────┘
//! ```

use super::spatial_encoding::{CodeType, SpatialCode};

/// Type of space-filling curve
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurveType {
    /// Z-order (Morton) curve: Fast computation, good locality
    /// Used by SST engine
    ZOrder,

    /// Hilbert curve: Better locality than Z-order, orthogonal traversal
    /// Used by HELIX engine
    Hilbert,

    /// Adaptive curve: Learned from data distribution
    /// Used by SWIFT engine
    AdaCurve,
}

impl CurveType {
    /// Get the locality quality estimate (higher = better clustering)
    pub fn locality_quality(&self) -> f32 {
        match self {
            CurveType::ZOrder => 0.82,
            CurveType::Hilbert => 0.95,
            CurveType::AdaCurve => 0.92,
        }
    }

    /// Get relative computation cost (higher = slower)
    pub fn computation_cost(&self) -> f32 {
        match self {
            CurveType::ZOrder => 1.0,
            CurveType::Hilbert => 2.5,
            CurveType::AdaCurve => 1.8,
        }
    }
}

/// Trait for spatial curve encoding used by storage engines
///
/// This trait abstracts over different space-filling curve implementations
/// to enable unified block-level pruning across SST, HELIX, and SWIFT engines.
pub trait SpatialCurveEncoder: Send + Sync {
    /// Get the curve type
    fn curve_type(&self) -> CurveType;

    /// Get the number of dimensions this encoder handles
    fn dimensions(&self) -> usize;

    /// Get the bits per dimension
    fn bits_per_dim(&self) -> usize;

    /// Get the code type (64/128/256/512-bit)
    fn code_type(&self) -> CodeType;

    /// Encode PCA-projected coordinates to a spatial code
    ///
    /// # Arguments
    /// * `pca_coords` - Normalized coordinates in [0, 1] range from PCA projection
    ///
    /// # Returns
    /// A spatial code representing the position on the space-filling curve
    fn encode(&self, pca_coords: &[f32]) -> SpatialCode;

    /// Batch encode multiple coordinate sets for efficiency
    ///
    /// Default implementation calls encode() for each, but implementations
    /// can override for SIMD optimization.
    fn encode_batch(&self, pca_coords_batch: &[Vec<f32>]) -> Vec<SpatialCode> {
        pca_coords_batch.iter().map(|c| self.encode(c)).collect()
    }

    /// Compute pruning epsilon (search radius) based on block count
    ///
    /// # Arguments
    /// * `num_blocks` - Total number of blocks
    /// * `selectivity` - Desired selectivity (0.0 = check all, 1.0 = check minimum)
    ///
    /// # Returns
    /// Epsilon value to use for range checking
    fn compute_epsilon(&self, num_blocks: usize, selectivity: f32) -> SpatialCode;

    /// Check if a block is within pruning range of the query
    ///
    /// # Arguments
    /// * `block_code` - Spatial code of the block centroid
    /// * `query_code` - Spatial code of the query vector
    /// * `epsilon` - Search radius / epsilon value
    ///
    /// # Returns
    /// true if the block should be searched, false if it can be pruned
    fn in_range(
        &self,
        block_code: &SpatialCode,
        query_code: &SpatialCode,
        epsilon: &SpatialCode,
    ) -> bool;

    /// Decode a spatial code back to coordinates (for debugging/visualization)
    fn decode(&self, code: &SpatialCode) -> Vec<f32>;

    /// Calculate the distance between two spatial codes
    ///
    /// This is an approximation of actual distance based on curve position
    fn code_distance(&self, code1: &SpatialCode, code2: &SpatialCode) -> SpatialCode {
        match (code1, code2) {
            (SpatialCode::Code64(a), SpatialCode::Code64(b)) => SpatialCode::Code64(a.abs_diff(*b)),
            (SpatialCode::Code128(a), SpatialCode::Code128(b)) => {
                SpatialCode::Code128(a.abs_diff(*b))
            }
            (
                SpatialCode::Code256 { low: al, high: ah },
                SpatialCode::Code256 { low: bl, high: bh },
            ) => {
                // Compute absolute difference for 256-bit codes
                let (low, borrow) = if al >= bl {
                    (al - bl, false)
                } else {
                    (u128::MAX - bl + al + 1, true)
                };
                let high = if borrow && ah > bh {
                    ah - bh - 1
                } else if !borrow && ah >= bh {
                    ah - bh
                } else {
                    bh - ah // swap order for absolute diff
                };
                SpatialCode::Code256 { low, high }
            }
            (SpatialCode::Code512(a), SpatialCode::Code512(b)) => {
                SpatialCode::Code512(a.abs_diff(b))
            }
            _ => code1.clone(), // Type mismatch
        }
    }

    /// Select blocks to search based on spatial codes
    ///
    /// Returns indices of blocks that should be searched (not pruned).
    ///
    /// # Arguments
    /// * `query_code` - Spatial code of the query vector
    /// * `block_codes` - Spatial codes of all block centroids
    /// * `max_blocks` - Maximum number of blocks to return
    ///
    /// # Returns
    /// Vector of block indices sorted by estimated relevance
    fn select_blocks(
        &self,
        query_code: &SpatialCode,
        block_codes: &[SpatialCode],
        max_blocks: usize,
    ) -> Vec<usize> {
        if block_codes.is_empty() {
            return Vec::new();
        }

        // Calculate distance to each block
        let mut scored: Vec<(SpatialCode, usize)> = block_codes
            .iter()
            .enumerate()
            .map(|(idx, code)| (self.code_distance(query_code, code), idx))
            .collect();

        // Sort by distance (smaller = closer = higher priority)
        scored.sort_by(|a, b| a.0.cmp(&b.0));

        // Return top max_blocks indices
        scored
            .into_iter()
            .take(max_blocks)
            .map(|(_, idx)| idx)
            .collect()
    }
}

/// Wrapper for Z-order encoder that implements SpatialCurveEncoder
pub struct ZOrderSpatialEncoder {
    inner: super::spatial_clustering::ZOrderEncoder,
    dimensions: usize,
    bits_per_dim: usize,
}

impl ZOrderSpatialEncoder {
    /// Create a new Z-order spatial encoder
    pub fn new(dimensions: usize, bits_per_dim: usize) -> Self {
        Self {
            inner: super::spatial_clustering::ZOrderEncoder::new(dimensions, bits_per_dim),
            dimensions,
            bits_per_dim,
        }
    }
}

impl SpatialCurveEncoder for ZOrderSpatialEncoder {
    fn curve_type(&self) -> CurveType {
        CurveType::ZOrder
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn bits_per_dim(&self) -> usize {
        self.bits_per_dim
    }

    fn code_type(&self) -> CodeType {
        CodeType::select(self.dimensions, self.bits_per_dim)
    }

    fn encode(&self, pca_coords: &[f32]) -> SpatialCode {
        self.inner.encode(pca_coords)
    }

    fn compute_epsilon(&self, num_blocks: usize, selectivity: f32) -> SpatialCode {
        // For Z-order, epsilon is based on code range
        let selectivity = selectivity.clamp(0.0, 1.0);
        let epsilon_factor = 1.0 - selectivity; // Higher selectivity = smaller epsilon

        // Base epsilon on sqrt(n) blocks with adjustment
        let base_blocks = (num_blocks as f32).sqrt().ceil() as u64;
        let epsilon_value = (base_blocks as f64 * epsilon_factor as f64 * 1000.0) as u64;

        match self.code_type() {
            CodeType::Bits64 => SpatialCode::Code64(epsilon_value),
            CodeType::Bits128 => SpatialCode::Code128(epsilon_value as u128),
            CodeType::Bits256 => SpatialCode::Code256 {
                low: epsilon_value as u128,
                high: 0,
            },
            CodeType::Bits512 => {
                SpatialCode::Code512(super::spatial_encoding::U512::from_u64(epsilon_value))
            }
        }
    }

    fn in_range(
        &self,
        block_code: &SpatialCode,
        query_code: &SpatialCode,
        epsilon: &SpatialCode,
    ) -> bool {
        let min_code = query_code.saturating_sub(epsilon);
        let max_code = query_code.saturating_add(epsilon);
        block_code.in_range(&min_code, &max_code)
    }

    fn decode(&self, code: &SpatialCode) -> Vec<f32> {
        self.inner.decode(code)
    }
}

/// Wrapper for Hilbert curve encoder that implements SpatialCurveEncoder
///
/// This wraps the HELIX HilbertCurve implementation
pub struct HilbertSpatialEncoder {
    dimensions: usize,
    bits_per_dim: usize,
    code_type: CodeType,
}

impl HilbertSpatialEncoder {
    /// Create a new Hilbert spatial encoder
    pub fn new(dimensions: usize, bits_per_dim: usize) -> Self {
        Self {
            dimensions,
            bits_per_dim,
            code_type: CodeType::select(dimensions, bits_per_dim),
        }
    }

    /// Encode normalized coordinates to Hilbert index
    fn encode_hilbert(&self, coords: &[f32]) -> u64 {
        // Quantize to discrete coordinates
        let max_val = (1u32 << self.bits_per_dim) - 1;
        let quantized: Vec<u32> = coords
            .iter()
            .map(|&c| {
                let clamped = c.clamp(0.0, 1.0);
                (clamped * max_val as f32) as u32
            })
            .collect();

        // Use HELIX's Hilbert curve implementation
        let hilbert = crate::storage::engines::impls::helix::hilbert_curve::HilbertCurve::new(
            self.dimensions,
            self.bits_per_dim,
        );
        hilbert.encode(&quantized)
    }
}

impl SpatialCurveEncoder for HilbertSpatialEncoder {
    fn curve_type(&self) -> CurveType {
        CurveType::Hilbert
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn bits_per_dim(&self) -> usize {
        self.bits_per_dim
    }

    fn code_type(&self) -> CodeType {
        self.code_type
    }

    fn encode(&self, pca_coords: &[f32]) -> SpatialCode {
        let hilbert_index = self.encode_hilbert(pca_coords);

        // Hilbert implementation returns u64, wrap in appropriate SpatialCode
        match self.code_type {
            CodeType::Bits64 => SpatialCode::Code64(hilbert_index),
            CodeType::Bits128 => SpatialCode::Code128(hilbert_index as u128),
            CodeType::Bits256 => SpatialCode::Code256 {
                low: hilbert_index as u128,
                high: 0,
            },
            CodeType::Bits512 => {
                SpatialCode::Code512(super::spatial_encoding::U512::from_u64(hilbert_index))
            }
        }
    }

    fn compute_epsilon(&self, num_blocks: usize, selectivity: f32) -> SpatialCode {
        // Hilbert curves have better locality, so we can use tighter epsilon
        let selectivity = selectivity.clamp(0.0, 1.0);
        let epsilon_factor = 1.0 - selectivity;

        // Hilbert curve needs less epsilon for same quality due to better locality
        let locality_bonus = 0.85; // 15% tighter than Z-order
        let base_blocks = (num_blocks as f32).sqrt().ceil() as u64;
        let epsilon_value =
            (base_blocks as f64 * epsilon_factor as f64 * 800.0 * locality_bonus) as u64;

        match self.code_type {
            CodeType::Bits64 => SpatialCode::Code64(epsilon_value),
            CodeType::Bits128 => SpatialCode::Code128(epsilon_value as u128),
            CodeType::Bits256 => SpatialCode::Code256 {
                low: epsilon_value as u128,
                high: 0,
            },
            CodeType::Bits512 => {
                SpatialCode::Code512(super::spatial_encoding::U512::from_u64(epsilon_value))
            }
        }
    }

    fn in_range(
        &self,
        block_code: &SpatialCode,
        query_code: &SpatialCode,
        epsilon: &SpatialCode,
    ) -> bool {
        let min_code = query_code.saturating_sub(epsilon);
        let max_code = query_code.saturating_add(epsilon);
        block_code.in_range(&min_code, &max_code)
    }

    fn decode(&self, code: &SpatialCode) -> Vec<f32> {
        // Hilbert decode is more complex, use placeholder
        // In production, would implement proper Hilbert decode
        let hilbert_index = match code {
            SpatialCode::Code64(v) => *v,
            SpatialCode::Code128(v) => *v as u64,
            SpatialCode::Code256 { low, .. } => *low as u64,
            SpatialCode::Code512(v) => v.parts[0] as u64,
        };

        // Approximate decode - return uniform distribution
        // Proper implementation would use HilbertCurve::decode
        let max_val = (1u64 << (self.bits_per_dim * self.dimensions)) - 1;
        let normalized = if max_val > 0 {
            hilbert_index as f32 / max_val as f32
        } else {
            0.0
        };

        vec![normalized; self.dimensions]
    }
}

/// Factory for creating spatial encoders based on engine type
pub struct SpatialEncoderFactory;

impl SpatialEncoderFactory {
    /// Create a Z-order encoder (for SST engine)
    pub fn create_zorder(dimensions: usize, bits_per_dim: usize) -> Box<dyn SpatialCurveEncoder> {
        Box::new(ZOrderSpatialEncoder::new(dimensions, bits_per_dim))
    }

    /// Create a Hilbert encoder (for HELIX engine)
    pub fn create_hilbert(dimensions: usize, bits_per_dim: usize) -> Box<dyn SpatialCurveEncoder> {
        Box::new(HilbertSpatialEncoder::new(dimensions, bits_per_dim))
    }

    /// Create encoder for a specific curve type
    pub fn create(
        curve_type: CurveType,
        dimensions: usize,
        bits_per_dim: usize,
    ) -> Box<dyn SpatialCurveEncoder> {
        match curve_type {
            CurveType::ZOrder => Self::create_zorder(dimensions, bits_per_dim),
            CurveType::Hilbert => Self::create_hilbert(dimensions, bits_per_dim),
            CurveType::AdaCurve => {
                // AdaCurve is more complex and requires training data
                // Fall back to Hilbert for now
                Self::create_hilbert(dimensions, bits_per_dim)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_curve_type_properties() {
        assert!(CurveType::Hilbert.locality_quality() > CurveType::ZOrder.locality_quality());
        assert!(CurveType::Hilbert.computation_cost() > CurveType::ZOrder.computation_cost());
    }

    #[test]
    fn test_zorder_encoder() {
        let encoder = ZOrderSpatialEncoder::new(4, 8);
        assert_eq!(encoder.curve_type(), CurveType::ZOrder);
        assert_eq!(encoder.dimensions(), 4);
        assert_eq!(encoder.bits_per_dim(), 8);
        assert_eq!(encoder.code_type(), CodeType::Bits64);
    }

    #[test]
    fn test_zorder_encode_decode() {
        let encoder = ZOrderSpatialEncoder::new(4, 8);
        let coords = vec![0.25, 0.5, 0.75, 1.0];

        let code = encoder.encode(&coords);
        let decoded = encoder.decode(&code);

        // Check roundtrip is approximately correct (quantization error expected)
        for (orig, dec) in coords.iter().zip(decoded.iter()) {
            assert!((orig - dec).abs() < 0.01);
        }
    }

    #[test]
    fn test_hilbert_encoder() {
        let encoder = HilbertSpatialEncoder::new(4, 8);
        assert_eq!(encoder.curve_type(), CurveType::Hilbert);
        assert_eq!(encoder.dimensions(), 4);
    }

    #[test]
    fn test_encoder_factory() {
        let zorder = SpatialEncoderFactory::create(CurveType::ZOrder, 4, 8);
        assert_eq!(zorder.curve_type(), CurveType::ZOrder);

        let hilbert = SpatialEncoderFactory::create(CurveType::Hilbert, 4, 8);
        assert_eq!(hilbert.curve_type(), CurveType::Hilbert);
    }

    #[test]
    fn test_block_selection() {
        let encoder = ZOrderSpatialEncoder::new(4, 8);

        // Create some block codes
        let block_coords = vec![
            vec![0.1, 0.1, 0.1, 0.1],
            vec![0.5, 0.5, 0.5, 0.5],
            vec![0.9, 0.9, 0.9, 0.9],
            vec![0.2, 0.2, 0.2, 0.2],
        ];
        let block_codes: Vec<SpatialCode> =
            block_coords.iter().map(|c| encoder.encode(c)).collect();

        // Query near first block
        let query_code = encoder.encode(&[0.15, 0.15, 0.15, 0.15]);

        // Select 2 blocks
        let selected = encoder.select_blocks(&query_code, &block_codes, 2);

        // Should select blocks closest to query (index 0 and 3)
        assert_eq!(selected.len(), 2);
        assert!(selected.contains(&0)); // Closest
        assert!(selected.contains(&3)); // Second closest
    }
}
