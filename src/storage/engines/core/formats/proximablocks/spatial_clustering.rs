//! Advanced spatial clustering for vector blocks.
//!
//! Provides PCA-based clustering and Z-Order (Morton Code) spatial ordering
//! for improved pruning and cache locality in SST/SWIFT engines.

use std::cmp::Ordering;

use super::spatial_encoding::{CodeType, SpatialCode};

/// Incremental PCA for online computation of principal components.
///
/// Uses Welford's algorithm for stable online mean/variance computation,
/// extended to covariance matrices for PCA.
pub struct IncrementalPCA {
    /// Number of samples seen
    n_samples: usize,
    /// Running mean vector
    mean: Vec<f64>,
    /// Running covariance matrix (upper triangle only)
    covariance: Vec<Vec<f64>>,
    /// Number of components to compute
    n_components: usize,
    /// Computed principal components (after finalize)
    components: Option<Vec<Vec<f64>>>,
}

impl IncrementalPCA {
    /// Create new incremental PCA.
    ///
    /// # Arguments
    /// * `dimension` - Input vector dimension
    /// * `n_components` - Number of principal components (typically 1-8)
    pub fn new(dimension: usize, n_components: usize) -> Self {
        let n_components = n_components.min(dimension);
        Self {
            n_samples: 0,
            mean: vec![0.0; dimension],
            covariance: vec![vec![0.0; dimension]; dimension],
            n_components,
            components: None,
        }
    }

    /// Add a sample to the PCA computation.
    pub fn add_sample(&mut self, sample: &[f32]) {
        if sample.len() != self.mean.len() {
            return; // Dimension mismatch
        }

        self.n_samples += 1;
        let n = self.n_samples as f64;

        // Update mean using Welford's algorithm
        let mut delta = vec![0.0; sample.len()];
        for i in 0..sample.len() {
            delta[i] = sample[i] as f64 - self.mean[i];
            self.mean[i] += delta[i] / n;
        }

        // Update covariance (only upper triangle for efficiency)
        let delta2: Vec<f64> = sample
            .iter()
            .zip(&self.mean)
            .map(|(&s, &m)| s as f64 - m)
            .collect();

        for i in 0..sample.len() {
            for j in i..sample.len() {
                self.covariance[i][j] += delta[i] * delta2[j];
            }
        }
    }

    /// Finalize PCA computation and extract principal components.
    ///
    /// Uses power iteration method for efficient computation of top components.
    pub fn finalize(&mut self) {
        if self.n_samples == 0 {
            self.components = Some(vec![]);
            return;
        }

        let n = self.n_samples as f64;
        let dim = self.mean.len();

        // Normalize covariance matrix
        for i in 0..dim {
            for j in i..dim {
                self.covariance[i][j] /= n;
                if i != j {
                    self.covariance[j][i] = self.covariance[i][j]; // Mirror
                }
            }
        }

        // Extract top n_components using power iteration
        let mut components = Vec::new();
        let mut remaining_cov = self.covariance.clone();

        for _comp in 0..self.n_components {
            // Power iteration to find dominant eigenvector
            let eigenvector = power_iteration(&remaining_cov, 20);
            components.push(eigenvector.clone());

            // Deflate covariance matrix (remove this component's contribution)
            deflate_matrix(&mut remaining_cov, &eigenvector);
        }

        self.components = Some(components);
    }

    /// Project a sample onto the principal components.
    ///
    /// Returns a lower-dimensional representation.
    pub fn transform(&self, sample: &[f32]) -> Vec<f32> {
        let components = match &self.components {
            Some(c) => c,
            None => return vec![0.0; self.n_components],
        };

        let mut result = vec![0.0; components.len()];
        for (i, component) in components.iter().enumerate() {
            let mut dot = 0.0;
            for (j, &val) in sample.iter().enumerate() {
                if j < component.len() {
                    dot += (val as f64 - self.mean[j]) * component[j];
                }
            }
            result[i] = dot as f32;
        }
        result
    }

    /// Get the first principal component score for a sample.
    ///
    /// This is the primary clustering score for PCA-based ordering.
    pub fn score(&self, sample: &[f32]) -> f32 {
        let projected = self.transform(sample);
        projected.first().copied().unwrap_or(0.0)
    }
}

/// Power iteration to find dominant eigenvector.
fn power_iteration(matrix: &[Vec<f64>], iterations: usize) -> Vec<f64> {
    let dim = matrix.len();
    let mut v: Vec<f64> = (0..dim).map(|i| (i as f64 + 1.0).sin()).collect();

    for _ in 0..iterations {
        // Multiply matrix * v
        let mut new_v = vec![0.0; dim];
        for i in 0..dim {
            for j in 0..dim {
                new_v[i] += matrix[i][j] * v[j];
            }
        }

        // Normalize
        let norm: f64 = new_v.iter().map(|&x| x * x).sum::<f64>().sqrt();
        if norm > 1e-10 {
            for val in &mut new_v {
                *val /= norm;
            }
        }
        v = new_v;
    }

    v
}

/// Deflate matrix by removing contribution of given eigenvector.
fn deflate_matrix(matrix: &mut [Vec<f64>], eigenvector: &[f64]) {
    let dim = matrix.len();

    // Estimate eigenvalue (Rayleigh quotient)
    let mut eigenvalue = 0.0;
    for i in 0..dim {
        for j in 0..dim {
            eigenvalue += eigenvector[i] * matrix[i][j] * eigenvector[j];
        }
    }

    // Remove outer product: A' = A - λvv^T
    for i in 0..dim {
        for j in 0..dim {
            matrix[i][j] -= eigenvalue * eigenvector[i] * eigenvector[j];
        }
    }
}

/// Z-Order (Morton Code) encoder for spatial indexing.
///
/// Maps multi-dimensional points to a 1D space-filling curve that preserves
/// spatial locality. Simpler than Hilbert curve but still effective.
///
/// Supports 1-64 dimensions with adaptive code width (64/128/256/512-bit).
pub struct ZOrderEncoder {
    /// Number of dimensions (1-64)
    dimensions: usize,
    /// Number of bits per dimension (resolution)
    bits_per_dim: usize,
    /// Code type (auto-selected based on dimensions and bits_per_dim)
    code_type: CodeType,
}

impl ZOrderEncoder {
    /// Create new Z-Order encoder.
    ///
    /// # Arguments
    /// * `dimensions` - Number of dimensions (1-64)
    /// * `bits_per_dim` - Bits per dimension (typically 8)
    ///
    /// # Automatic Code Type Selection
    /// - 1-8 dims: 64-bit codes
    /// - 9-16 dims: 128-bit codes
    /// - 17-32 dims: 256-bit codes
    /// - 33-64 dims: 512-bit codes
    pub fn new(dimensions: usize, bits_per_dim: usize) -> Self {
        assert!(
            dimensions > 0 && dimensions <= 64,
            "Dimensions must be 1-64"
        );
        assert!(
            bits_per_dim > 0 && bits_per_dim <= 16,
            "Bits per dim must be 1-16"
        );

        // Auto-select code type based on required bits
        let code_type = CodeType::select(dimensions, bits_per_dim);

        // Validate total bits fit in selected code type
        let total_bits = dimensions * bits_per_dim;
        let max_bits = code_type.max_bits();
        assert!(
            total_bits <= max_bits,
            "Total bits ({} * {} = {}) exceeds {}-bit limit",
            dimensions,
            bits_per_dim,
            total_bits,
            max_bits
        );

        Self {
            dimensions,
            bits_per_dim,
            code_type,
        }
    }

    /// Encode a multi-dimensional point to Z-order code.
    ///
    /// # Arguments
    /// * `coords` - Normalized coordinates in [0, 1] range
    ///
    /// # Returns
    /// Z-order code (Morton code) as SpatialCode enum
    pub fn encode(&self, coords: &[f32]) -> SpatialCode {
        assert_eq!(
            coords.len(),
            self.dimensions,
            "Coordinate dimension mismatch"
        );

        match self.code_type {
            CodeType::Bits64 => SpatialCode::Code64(self.encode_64(coords)),
            CodeType::Bits128 => SpatialCode::Code128(self.encode_128(coords)),
            CodeType::Bits256 => {
                let (low, high) = self.encode_256(coords);
                SpatialCode::Code256 { low, high }
            }
            CodeType::Bits512 => SpatialCode::Code512(self.encode_512(coords)),
        }
    }

    /// Encode to 64-bit Z-order code (1-8 dimensions).
    ///
    /// Optimized implementation using:
    /// - Stack-based arrays (no heap allocation)
    /// - BMI2 pdep instruction on x86_64 when available
    /// - Lookup table fallback for other platforms
    fn encode_64(&self, coords: &[f32]) -> u64 {
        let max_val = (1u64 << self.bits_per_dim) - 1;

        // Stack-based quantization (max 8 dimensions for 64-bit)
        let mut quantized = [0u64; 8];
        for (i, &c) in coords.iter().enumerate().take(8) {
            let clamped = c.clamp(0.0, 1.0);
            quantized[i] = (clamped * max_val as f32) as u64;
        }

        // Use optimized bit interleaving
        self.interleave_bits_64(&quantized[..self.dimensions])
    }

    /// Fast bit interleaving using platform-specific optimizations
    #[inline(always)]
    fn interleave_bits_64(&self, quantized: &[u64]) -> u64 {
        // Try BMI2 pdep on x86_64
        #[cfg(all(target_arch = "x86_64", target_feature = "bmi2"))]
        {
            return self.interleave_bits_64_bmi2(quantized);
        }

        // Fallback: optimized scalar with unrolled inner loop
        #[cfg(not(all(target_arch = "x86_64", target_feature = "bmi2")))]
        {
            self.interleave_bits_64_scalar(quantized)
        }
    }

    /// BMI2-accelerated bit interleaving using pdep
    #[cfg(all(target_arch = "x86_64", target_feature = "bmi2"))]
    #[inline(always)]
    fn interleave_bits_64_bmi2(&self, quantized: &[u64]) -> u64 {
        use std::arch::x86_64::_pdep_u64;

        let mut code = 0u64;
        let dims = self.dimensions;

        // Create deposit mask for each dimension
        // For n dimensions, each dimension gets every nth bit
        for (dim_idx, &val) in quantized.iter().enumerate() {
            // Mask: bit at position dim_idx, then dim_idx + dims, then dim_idx + 2*dims, etc.
            let mut mask = 0u64;
            for bit in 0..self.bits_per_dim {
                mask |= 1u64 << (bit * dims + dim_idx);
            }
            // Use pdep to scatter the bits
            code |= unsafe { _pdep_u64(val, mask) };
        }

        code
    }

    /// Optimized scalar bit interleaving
    #[inline(always)]
    fn interleave_bits_64_scalar(&self, quantized: &[u64]) -> u64 {
        let mut code = 0u64;
        let dims = self.dimensions;
        let bits = self.bits_per_dim;

        // Process 4 bits at a time when possible
        let full_iters = bits / 4;
        let _remaining = bits % 4;

        for iter in 0..full_iters {
            let base_bit = iter * 4;
            for (dim_idx, &val) in quantized.iter().enumerate() {
                // Unrolled: 4 bits per iteration
                code |= ((val >> base_bit) & 1) << (base_bit * dims + dim_idx);
                code |= ((val >> (base_bit + 1)) & 1) << ((base_bit + 1) * dims + dim_idx);
                code |= ((val >> (base_bit + 2)) & 1) << ((base_bit + 2) * dims + dim_idx);
                code |= ((val >> (base_bit + 3)) & 1) << ((base_bit + 3) * dims + dim_idx);
            }
        }

        // Handle remaining bits
        for bit in (full_iters * 4)..bits {
            for (dim_idx, &val) in quantized.iter().enumerate() {
                let bit_val = (val >> bit) & 1;
                code |= bit_val << (bit * dims + dim_idx);
            }
        }

        code
    }

    /// Encode to 128-bit Z-order code (9-16 dimensions).
    ///
    /// Optimized with stack-based arrays and loop unrolling.
    fn encode_128(&self, coords: &[f32]) -> u128 {
        let max_val = (1u128 << self.bits_per_dim) - 1;

        // Stack-based quantization (max 16 dimensions for 128-bit)
        let mut quantized = [0u128; 16];
        for (i, &c) in coords.iter().enumerate().take(16) {
            let clamped = c.clamp(0.0, 1.0);
            quantized[i] = (clamped * max_val as f32) as u128;
        }

        // Optimized interleaving with loop unrolling
        let mut code = 0u128;
        let dims = self.dimensions;
        let bits = self.bits_per_dim;

        // Process 2 bits at a time
        let full_iters = bits / 2;

        for iter in 0..full_iters {
            let base_bit = iter * 2;
            for (dim_idx, &val) in quantized[..dims].iter().enumerate() {
                code |= ((val >> base_bit) & 1) << (base_bit * dims + dim_idx);
                code |= ((val >> (base_bit + 1)) & 1) << ((base_bit + 1) * dims + dim_idx);
            }
        }

        // Handle remaining bits
        for bit in (full_iters * 2)..bits {
            for (dim_idx, &val) in quantized[..dims].iter().enumerate() {
                let bit_val = (val >> bit) & 1;
                code |= bit_val << (bit * dims + dim_idx);
            }
        }

        code
    }

    /// Encode to 256-bit Z-order code (17-32 dimensions).
    fn encode_256(&self, coords: &[f32]) -> (u128, u128) {
        let max_val = (1u128 << self.bits_per_dim) - 1;
        let quantized: Vec<u128> = coords
            .iter()
            .map(|&c| {
                let clamped = c.clamp(0.0, 1.0);
                (clamped * max_val as f32) as u128
            })
            .collect();

        // Interleave bits - split across low (0-127) and high (128-255) parts
        let mut low = 0u128;
        let mut high = 0u128;

        for bit in 0..self.bits_per_dim {
            for (dim_idx, &val) in quantized.iter().enumerate() {
                let bit_val = (val >> bit) & 1;
                let shift = bit * self.dimensions + dim_idx;

                if shift < 128 {
                    low |= bit_val << shift;
                } else {
                    high |= bit_val << (shift - 128);
                }
            }
        }

        (low, high)
    }

    /// Encode to 512-bit Z-order code (33-64 dimensions).
    fn encode_512(&self, coords: &[f32]) -> super::spatial_encoding::U512 {
        use super::spatial_encoding::U512;

        let max_val = (1u128 << self.bits_per_dim) - 1;
        let quantized: Vec<u128> = coords
            .iter()
            .map(|&c| {
                let clamped = c.clamp(0.0, 1.0);
                (clamped * max_val as f32) as u128
            })
            .collect();

        // Interleave bits across four 128-bit parts
        let mut parts = [0u128; 4];

        for bit in 0..self.bits_per_dim {
            for (dim_idx, &val) in quantized.iter().enumerate() {
                let bit_val = (val >> bit) & 1;
                let shift = bit * self.dimensions + dim_idx;

                // Determine which 128-bit part this bit goes into
                let part_idx = shift / 128;
                let part_shift = shift % 128;

                if part_idx < 4 {
                    parts[part_idx] |= bit_val << part_shift;
                }
            }
        }

        U512 { parts }
    }

    /// Decode Z-order code back to coordinates.
    ///
    /// Useful for visualization and debugging.
    pub fn decode(&self, code: &SpatialCode) -> Vec<f32> {
        match code {
            SpatialCode::Code64(c) => self.decode_64(*c),
            SpatialCode::Code128(c) => self.decode_128(*c),
            SpatialCode::Code256 { low, high } => self.decode_256(*low, *high),
            SpatialCode::Code512(c) => self.decode_512(c),
        }
    }

    fn decode_64(&self, code: u64) -> Vec<f32> {
        let max_val = (1u64 << self.bits_per_dim) - 1;
        let mut quantized = vec![0u64; self.dimensions];

        // De-interleave bits
        for bit in 0..self.bits_per_dim {
            for dim_idx in 0..self.dimensions {
                let shift = bit * self.dimensions + dim_idx;
                let bit_val = (code >> shift) & 1;
                quantized[dim_idx] |= bit_val << bit;
            }
        }

        // Convert back to normalized coordinates
        quantized
            .into_iter()
            .map(|q| q as f32 / max_val as f32)
            .collect()
    }

    fn decode_128(&self, code: u128) -> Vec<f32> {
        let max_val = (1u128 << self.bits_per_dim) - 1;
        let mut quantized = vec![0u128; self.dimensions];

        // De-interleave bits
        for bit in 0..self.bits_per_dim {
            for dim_idx in 0..self.dimensions {
                let shift = bit * self.dimensions + dim_idx;
                let bit_val = (code >> shift) & 1;
                quantized[dim_idx] |= bit_val << bit;
            }
        }

        // Convert back to normalized coordinates
        quantized
            .into_iter()
            .map(|q| q as f32 / max_val as f32)
            .collect()
    }

    fn decode_256(&self, low: u128, high: u128) -> Vec<f32> {
        let max_val = (1u128 << self.bits_per_dim) - 1;
        let mut quantized = vec![0u128; self.dimensions];

        // De-interleave bits from both parts
        for bit in 0..self.bits_per_dim {
            for dim_idx in 0..self.dimensions {
                let shift = bit * self.dimensions + dim_idx;

                let bit_val = if shift < 128 {
                    (low >> shift) & 1
                } else {
                    (high >> (shift - 128)) & 1
                };

                quantized[dim_idx] |= bit_val << bit;
            }
        }

        // Convert back to normalized coordinates
        quantized
            .into_iter()
            .map(|q| q as f32 / max_val as f32)
            .collect()
    }

    fn decode_512(&self, code: &super::spatial_encoding::U512) -> Vec<f32> {
        let max_val = (1u128 << self.bits_per_dim) - 1;
        let mut quantized = vec![0u128; self.dimensions];

        // De-interleave bits from all four parts
        for bit in 0..self.bits_per_dim {
            for dim_idx in 0..self.dimensions {
                let shift = bit * self.dimensions + dim_idx;
                let part_idx = shift / 128;
                let part_shift = shift % 128;

                if part_idx < 4 {
                    let bit_val = (code.parts[part_idx] >> part_shift) & 1;
                    quantized[dim_idx] |= bit_val << bit;
                }
            }
        }

        // Convert back to normalized coordinates
        quantized
            .into_iter()
            .map(|q| q as f32 / max_val as f32)
            .collect()
    }

    /// Get Z-order range for a query box (for pruning).
    ///
    /// Returns (min_code, max_code) that covers the query region.
    /// Blocks outside this range can be pruned.
    pub fn range_for_box(
        &self,
        min_coords: &[f32],
        max_coords: &[f32],
    ) -> (SpatialCode, SpatialCode) {
        let min_code = self.encode(min_coords);
        let max_code = self.encode(max_coords);
        (min_code, max_code)
    }
}

/// Adaptive PCA configuration based on vector dimensionality.
///
/// Automatically selects optimal PCA dimensions for spatial indexing.
pub struct AdaptivePcaConfig {
    /// Number of PCA dimensions to use
    pub n_components: usize,
    /// Number of bits per dimension for Z-Order encoding
    pub bits_per_dim: usize,
    /// Expected code type
    pub code_type: CodeType,
}

impl AdaptivePcaConfig {
    /// Select optimal PCA configuration for given vector dimensionality.
    ///
    /// # Strategy
    /// - 1-16 dims: Use actual dims (no PCA reduction)
    /// - 17-64 dims: 8-16 PCA dims @ 8 bits/dim (64-128 bit codes)
    /// - 65-128 dims: 16-24 PCA dims @ 8 bits/dim (128-256 bit codes)
    /// - 129-384 dims: 32-48 PCA dims @ 8 bits/dim (256-512 bit codes)
    /// - 385-768 dims: 48-64 PCA dims @ 8 bits/dim (384-512 bit codes)
    /// - 769+ dims: 64 PCA dims @ 8 bits/dim (512 bit codes, max)
    pub fn for_vector_dim(vector_dim: usize) -> Self {
        let (n_components, bits_per_dim) = match vector_dim {
            0 => (1, 8),
            1..=16 => (vector_dim, 8),
            17..=64 => (8.max(vector_dim / 4), 8),
            65..=128 => (16.max(vector_dim / 6), 8),
            129..=384 => (32.max(vector_dim / 8), 8),
            385..=768 => (48.max(vector_dim / 12), 8),
            _ => (64, 8), // Max PCA dims
        };

        let n_components = n_components.clamp(1, 64);
        let code_type = CodeType::select(n_components, bits_per_dim);

        Self {
            n_components,
            bits_per_dim,
            code_type,
        }
    }

    /// Select configuration with specific target dimensions (bounded to safe limits).
    pub fn with_target(vector_dim: usize, target_dimensions: usize) -> Self {
        let target = target_dimensions.min(64).min(vector_dim).max(1);
        let bits_per_dim = 8;
        let code_type = CodeType::select(target, bits_per_dim);

        Self {
            n_components: target,
            bits_per_dim,
            code_type,
        }
    }
}

/// PCA-based clustering with Z-Order spatial indexing.
///
/// Combines PCA dimension reduction with Z-Order curve for optimal
/// spatial locality and efficient range-based pruning.
///
/// Returns: (blocks, index_entries, zorder_codes) where zorder_codes[i]
/// is the Z-Order code for index_entries[i] after clustering.
pub fn cluster_blocks_pca_zorder<B, I, F>(
    blocks: Vec<B>,
    index_entries: Vec<I>,
    get_centroid: F,
    target_dimensions: usize,
) -> (Vec<B>, Vec<I>, Vec<SpatialCode>)
where
    F: Fn(&I) -> &[f32],
{
    if blocks.is_empty() || blocks.len() != index_entries.len() {
        return (blocks, index_entries, vec![]);
    }

    let dimension = get_centroid(&index_entries[0]).len();

    // Guard against empty vectors (dimension 0) - cannot perform spatial clustering
    if dimension == 0 {
        return (blocks, index_entries, vec![]);
    }

    // Use adaptive PCA configuration
    let pca_config = AdaptivePcaConfig::with_target(dimension, target_dimensions);
    let n_components = pca_config.n_components;

    // Step 1: Compute PCA
    let mut pca = IncrementalPCA::new(dimension, n_components);
    for entry in &index_entries {
        pca.add_sample(get_centroid(entry));
    }
    pca.finalize();

    // Step 2: Project to PCA space and encode with Z-Order
    // Use adaptive bits_per_dim from config (typically 8)
    let bits_per_dim = pca_config.bits_per_dim;
    let zorder_encoder = ZOrderEncoder::new(n_components, bits_per_dim);

    let mut clustered: Vec<(SpatialCode, B, I)> = blocks
        .into_iter()
        .zip(index_entries)
        .map(|(block, entry)| {
            // Project to PCA space
            let pca_coords = pca.transform(get_centroid(&entry));

            // Normalize to [0, 1] for Z-Order encoding
            let normalized = normalize_coords(&pca_coords);

            // Encode to Z-Order
            let zorder_code = zorder_encoder.encode(&normalized);

            (zorder_code, block, entry)
        })
        .collect();

    // Step 3: Sort by Z-Order code
    clustered.sort_by(|a, b| a.0.cmp(&b.0));

    // Step 4: Extract sorted blocks, entries, and codes
    let (codes, blocks, index_entries): (Vec<SpatialCode>, Vec<B>, Vec<I>) = clustered
        .into_iter()
        .map(|(code, block, entry)| (code, block, entry))
        .fold(
            (Vec::new(), Vec::new(), Vec::new()),
            |(mut codes, mut blocks, mut entries), (code, block, entry)| {
                codes.push(code);
                blocks.push(block);
                entries.push(entry);
                (codes, blocks, entries)
            },
        );

    (blocks, index_entries, codes)
}

/// Normalize coordinates to [0, 1] range using min-max normalization.
fn normalize_coords(coords: &[f32]) -> Vec<f32> {
    if coords.is_empty() {
        return vec![];
    }

    let min = coords.iter().copied().fold(f32::INFINITY, f32::min);
    let max = coords.iter().copied().fold(f32::NEG_INFINITY, f32::max);

    if (max - min).abs() < 1e-6 {
        // All values are the same, return middle
        return vec![0.5; coords.len()];
    }

    coords.iter().map(|&c| (c - min) / (max - min)).collect()
}

/// Adaptive Curve (AdaCurve) - learns a space-filling curve from data distribution.
///
/// Unlike fixed curves (Z-Order, Hilbert), AdaCurves adapts to your specific
/// data patterns for better clustering and pruning.
pub struct AdaCurve {
    /// Number of dimensions
    dimensions: usize,
    /// Learned control points (cluster centers in PCA space)
    control_points: Vec<Vec<f32>>,
    /// Curve segment ordering (optimized traversal order)
    segment_order: Vec<usize>,
}

impl AdaCurve {
    /// Train AdaCurve from PCA-transformed centroids.
    ///
    /// Uses k-means to find natural clusters, then orders them for optimal locality.
    pub fn train(pca_coords: &[Vec<f32>], num_segments: usize) -> Self {
        if pca_coords.is_empty() {
            return Self {
                dimensions: 0,
                control_points: vec![],
                segment_order: vec![],
            };
        }

        let dimensions = pca_coords[0].len();
        let num_segments = num_segments.min(pca_coords.len()).max(8);

        // Step 1: K-means clustering to find natural groupings
        let control_points = kmeans_clustering(pca_coords, num_segments);

        // Step 2: Order control points to minimize jumps (greedy nearest neighbor)
        let segment_order = optimize_traversal_order(&control_points);

        Self {
            dimensions,
            control_points,
            segment_order,
        }
    }

    /// Encode a point to its position on the learned curve.
    ///
    /// Returns a u64 code representing the curve position.
    pub fn encode(&self, coords: &[f32]) -> u64 {
        if self.control_points.is_empty() || coords.len() != self.dimensions {
            return 0;
        }

        // Find nearest control point
        let (nearest_idx, _distance) = self
            .control_points
            .iter()
            .enumerate()
            .map(|(i, cp)| {
                let dist: f32 = coords
                    .iter()
                    .zip(cp.iter())
                    .map(|(a, b)| (a - b).powi(2))
                    .sum::<f32>()
                    .sqrt();
                (i, dist)
            })
            .min_by(|(_, d1), (_, d2)| d1.partial_cmp(d2).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap_or((0, 0.0));

        // Map to curve position via segment order
        let curve_position = self
            .segment_order
            .iter()
            .position(|&idx| idx == nearest_idx)
            .unwrap_or(0);

        // Encode as u64 with fine-grained positioning
        let _segment_bits = 48; // Upper bits for segment
        let offset_bits = 16; // Lower bits for offset within segment

        let segment_code = (curve_position as u64) << offset_bits;

        // Compute offset within segment based on distance to control point
        let nearest_cp = &self.control_points[nearest_idx];
        let dist_to_cp: f32 = coords
            .iter()
            .zip(nearest_cp.iter())
            .map(|(a, b)| (a - b).powi(2))
            .sum::<f32>()
            .sqrt();

        let offset = ((dist_to_cp * 65535.0).min(65535.0) as u64) & ((1 << offset_bits) - 1);

        segment_code | offset
    }

    /// Get curve range for a bounding box (for pruning).
    pub fn range_for_box(&self, min_coords: &[f32], max_coords: &[f32]) -> (u64, u64) {
        let min_code = self.encode(min_coords);
        let max_code = self.encode(max_coords);
        (min_code.min(max_code), min_code.max(max_code))
    }
}

/// K-means clustering to find natural groupings in data.
fn kmeans_clustering(points: &[Vec<f32>], k: usize) -> Vec<Vec<f32>> {
    if points.is_empty() {
        return vec![];
    }

    let dimensions = points[0].len();
    let k = k.min(points.len());

    // Initialize centroids using k-means++ for better starting positions
    let mut centroids = Vec::with_capacity(k);

    // First centroid: random point
    if let Some(first) = points.first() {
        centroids.push(first.clone());
    }

    // Remaining centroids: k-means++ selection
    for _ in 1..k {
        let distances: Vec<f32> = points
            .iter()
            .map(|p| {
                centroids
                    .iter()
                    .map(|c| {
                        p.iter()
                            .zip(c.iter())
                            .map(|(a, b)| (a - b).powi(2))
                            .sum::<f32>()
                    })
                    .fold(f32::INFINITY, f32::min)
            })
            .collect();

        // Select point with maximum distance to nearest centroid
        if let Some((idx, _)) = distances
            .iter()
            .enumerate()
            .max_by(|(_, d1), (_, d2)| d1.partial_cmp(d2).unwrap_or(std::cmp::Ordering::Equal))
        {
            centroids.push(points[idx].clone());
        }
    }

    // Run k-means iterations (10 iterations is usually enough)
    for _ in 0..10 {
        // Assignment step: assign each point to nearest centroid
        let mut clusters: Vec<Vec<Vec<f32>>> = vec![vec![]; k];

        for point in points {
            let nearest = centroids
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let dist: f32 = point
                        .iter()
                        .zip(c.iter())
                        .map(|(a, b)| (a - b).powi(2))
                        .sum();
                    (i, dist)
                })
                .min_by(|(_, d1), (_, d2)| d1.partial_cmp(d2).unwrap_or(std::cmp::Ordering::Equal))
                .map_or(0, |(i, _)| i);

            clusters[nearest].push(point.clone());
        }

        // Update step: recompute centroids
        for (i, cluster) in clusters.iter().enumerate() {
            if cluster.is_empty() {
                continue;
            }

            let mut new_centroid = vec![0.0; dimensions];
            for point in cluster {
                for (j, &val) in point.iter().enumerate() {
                    new_centroid[j] += val;
                }
            }

            for val in &mut new_centroid {
                *val /= cluster.len() as f32;
            }

            centroids[i] = new_centroid;
        }
    }

    centroids
}

/// Optimize traversal order of control points using greedy nearest neighbor.
fn optimize_traversal_order(control_points: &[Vec<f32>]) -> Vec<usize> {
    if control_points.is_empty() {
        return vec![];
    }

    let n = control_points.len();
    let mut order = Vec::with_capacity(n);
    let mut visited = vec![false; n];

    // Start from first point
    order.push(0);
    visited[0] = true;

    // Greedy: always go to nearest unvisited point
    for _ in 1..n {
        let current = order[order.len() - 1];
        let current_point = &control_points[current];

        let next = (0..n)
            .filter(|&i| !visited[i])
            .min_by(|&i, &j| {
                let dist_i: f32 = control_points[i]
                    .iter()
                    .zip(current_point.iter())
                    .map(|(a, b)| (a - b).powi(2))
                    .sum();

                let dist_j: f32 = control_points[j]
                    .iter()
                    .zip(current_point.iter())
                    .map(|(a, b)| (a - b).powi(2))
                    .sum();

                dist_i
                    .partial_cmp(&dist_j)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(0);

        order.push(next);
        visited[next] = true;
    }

    order
}

/// PCA-based clustering with AdaCurves (learned curve).
///
/// Superior clustering quality compared to Z-Order, but with higher build cost.
/// Best for read-heavy workloads where clustering quality matters.
///
/// AdaCurves still return u64 codes (48-bit segment + 16-bit offset).
pub fn cluster_blocks_pca_adacurves<B, I, F>(
    blocks: Vec<B>,
    index_entries: Vec<I>,
    get_centroid: F,
    target_dimensions: usize,
) -> (Vec<B>, Vec<I>, Vec<u64>)
where
    F: Fn(&I) -> &[f32],
{
    if blocks.is_empty() || blocks.len() != index_entries.len() {
        return (blocks, index_entries, vec![]);
    }

    let dimension = get_centroid(&index_entries[0]).len();

    // Use adaptive PCA configuration (AdaCurves use 64-bit codes with different encoding)
    let pca_config = AdaptivePcaConfig::with_target(dimension, target_dimensions);
    let n_components = pca_config.n_components;

    // Step 1: Compute PCA
    let mut pca = IncrementalPCA::new(dimension, n_components);
    for entry in &index_entries {
        pca.add_sample(get_centroid(entry));
    }
    pca.finalize();

    // Step 2: Project to PCA space
    let pca_coords: Vec<Vec<f32>> = index_entries
        .iter()
        .map(|entry| pca.transform(get_centroid(entry)))
        .collect();

    // Step 3: Train AdaCurve from PCA-transformed data
    let num_segments = (pca_coords.len() / 50).clamp(8, 256); // Adaptive segment count
    let adacurve = AdaCurve::train(&pca_coords, num_segments);

    // Step 4: Encode each point using learned curve
    let mut clustered: Vec<(u64, B, I)> = blocks
        .into_iter()
        .zip(index_entries)
        .enumerate()
        .map(|(i, (block, entry))| {
            let curve_code = adacurve.encode(&pca_coords[i]);
            (curve_code, block, entry)
        })
        .collect();

    // Step 5: Sort by curve position
    clustered.sort_by(|a, b| a.0.cmp(&b.0));

    // Step 6: Extract sorted blocks, entries, and codes
    let (codes, blocks, index_entries): (Vec<u64>, Vec<B>, Vec<I>) = clustered
        .into_iter()
        .map(|(code, block, entry)| (code, block, entry))
        .fold(
            (Vec::new(), Vec::new(), Vec::new()),
            |(mut codes, mut blocks, mut entries), (code, block, entry)| {
                codes.push(code);
                blocks.push(block);
                entries.push(entry);
                (codes, blocks, entries)
            },
        );

    (blocks, index_entries, codes)
}

/// Simple PCA-based clustering (PC1 score only).
///
/// Faster than Z-Order for cases where you don't need range pruning.
pub fn cluster_blocks_pca<B, I, F>(
    blocks: Vec<B>,
    index_entries: Vec<I>,
    get_centroid: F,
) -> (Vec<B>, Vec<I>)
where
    F: Fn(&I) -> &[f32],
{
    if blocks.is_empty() || blocks.len() != index_entries.len() {
        return (blocks, index_entries);
    }

    let dimension = get_centroid(&index_entries[0]).len();

    // Compute PCA (first component only)
    let mut pca = IncrementalPCA::new(dimension, 1);
    for entry in &index_entries {
        pca.add_sample(get_centroid(entry));
    }
    pca.finalize();

    // Sort by PC1 score
    let mut clustered: Vec<(f32, B, I)> = blocks
        .into_iter()
        .zip(index_entries)
        .map(|(block, entry)| {
            let score = pca.score(get_centroid(&entry));
            (score, block, entry)
        })
        .collect();

    clustered.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(Ordering::Equal));

    let (blocks, index_entries): (Vec<B>, Vec<I>) = clustered
        .into_iter()
        .map(|(_, block, entry)| (block, entry))
        .unzip();

    (blocks, index_entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_incremental_pca() {
        let mut pca = IncrementalPCA::new(3, 1);

        // Add samples along x-axis (PC1 should be [1, 0, 0])
        pca.add_sample(&[1.0, 0.0, 0.0]);
        pca.add_sample(&[2.0, 0.0, 0.0]);
        pca.add_sample(&[3.0, 0.0, 0.0]);
        pca.add_sample(&[4.0, 0.0, 0.0]);

        pca.finalize();

        // Transform should project onto PC1
        let score1 = pca.score(&[5.0, 0.0, 0.0]);
        let score2 = pca.score(&[1.0, 0.0, 0.0]);

        // Higher x should give higher score
        assert!(score1 > score2);
    }

    #[test]
    fn test_zorder_encoding() {
        let encoder = ZOrderEncoder::new(2, 8);

        // Test corner points
        let code00 = encoder.encode(&[0.0, 0.0]);
        let code01 = encoder.encode(&[0.0, 1.0]);
        let code10 = encoder.encode(&[1.0, 0.0]);
        let code11 = encoder.encode(&[1.0, 1.0]);

        // All should be different
        assert_ne!(code00, code01);
        assert_ne!(code00, code10);
        assert_ne!(code00, code11);

        // Decode should match
        let decoded = encoder.decode(&code11);
        assert!((decoded[0] - 1.0).abs() < 0.01);
        assert!((decoded[1] - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_zorder_locality() {
        let encoder = ZOrderEncoder::new(2, 10);

        // Nearby points should have close Z-order codes
        let code1 = encoder.encode(&[0.5, 0.5]);
        let code2 = encoder.encode(&[0.51, 0.51]); // Very close
        let code3 = encoder.encode(&[0.9, 0.9]); // Far away

        // For 64-bit codes, extract the u64 value for comparison
        let val1 = match code1 {
            SpatialCode::Code64(v) => v as i64,
            _ => 0,
        };
        let val2 = match code2 {
            SpatialCode::Code64(v) => v as i64,
            _ => 0,
        };
        let val3 = match code3 {
            SpatialCode::Code64(v) => v as i64,
            _ => 0,
        };

        let diff_nearby = (val1 - val2).abs();
        let diff_far = (val1 - val3).abs();

        // Nearby should have smaller code difference
        assert!(diff_nearby < diff_far);
    }

    #[test]
    fn test_zorder_high_dimensional() {
        // Test with 64 dimensions (max supported)
        let encoder = ZOrderEncoder::new(64, 8);

        let coords1: Vec<f32> = (0..64).map(|i| i as f32 / 64.0).collect();
        let coords2: Vec<f32> = (0..64).map(|i| (i + 1) as f32 / 64.0).collect();

        let code1 = encoder.encode(&coords1);
        let code2 = encoder.encode(&coords2);

        // Should be different
        assert_ne!(code1, code2);

        // Should be 512-bit codes
        assert!(matches!(code1, SpatialCode::Code512(_)));
        assert!(matches!(code2, SpatialCode::Code512(_)));

        // Decode should work
        let decoded1 = encoder.decode(&code1);
        assert_eq!(decoded1.len(), 64);
    }

    #[test]
    fn test_adaptive_pca_config() {
        // Test various vector dimensions
        let config_128 = AdaptivePcaConfig::for_vector_dim(128);
        assert!(config_128.n_components >= 16);
        assert!(config_128.n_components <= 24);

        let config_384 = AdaptivePcaConfig::for_vector_dim(384);
        assert!(config_384.n_components >= 32);
        assert!(config_384.n_components <= 48);

        let config_768 = AdaptivePcaConfig::for_vector_dim(768);
        assert!(config_768.n_components >= 48);
        assert!(config_768.n_components <= 64);

        let config_1536 = AdaptivePcaConfig::for_vector_dim(1536);
        assert_eq!(config_1536.n_components, 64); // Max

        // Test with_target
        let config_target = AdaptivePcaConfig::with_target(768, 32);
        assert_eq!(config_target.n_components, 32);
    }

    #[test]
    fn test_pca_clustering() {
        struct TestEntry {
            centroid: Vec<f32>,
        }

        // Create entries along diagonal
        let entries: Vec<TestEntry> = vec![
            TestEntry {
                centroid: vec![1.0, 1.0],
            },
            TestEntry {
                centroid: vec![5.0, 5.0],
            },
            TestEntry {
                centroid: vec![3.0, 3.0],
            },
            TestEntry {
                centroid: vec![7.0, 7.0],
            },
        ];

        let blocks = vec!["a", "b", "c", "d"];

        let (clustered_blocks, _) =
            cluster_blocks_pca(blocks, entries, |e: &TestEntry| &e.centroid);

        // Should be sorted by PC1 (diagonal)
        // Expected order: a (1,1), c (3,3), b (5,5), d (7,7)
        assert_eq!(clustered_blocks, vec!["a", "c", "b", "d"]);
    }
}
