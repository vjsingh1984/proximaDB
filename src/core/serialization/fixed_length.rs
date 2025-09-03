//! Fixed-length vector optimizations for known dimensions
//!
//! Provides highly optimized serialization for vectors with known, fixed dimensions.
//! Eliminates length prefixes and enables direct memory mapping for maximum performance.

use anyhow::{Context, Result};
use bytemuck::{Pod, Zeroable, cast_slice, try_cast_slice};
use std::marker::PhantomData;
use std::mem::size_of;
use tracing::{debug, trace};
use zstd::{decode_all, encode_all};

/// Marker trait for fixed-length vector dimensions
pub trait FixedDimension: Send + Sync + 'static {
    const DIMENSION: usize;
    const BYTE_SIZE: usize = Self::DIMENSION * size_of::<f32>();
}

/// Common fixed dimensions
pub struct Dim64;
impl FixedDimension for Dim64 {
    const DIMENSION: usize = 64;
}

pub struct Dim128;
impl FixedDimension for Dim128 {
    const DIMENSION: usize = 128;
}

pub struct Dim256;
impl FixedDimension for Dim256 {
    const DIMENSION: usize = 256;
}

pub struct Dim512;
impl FixedDimension for Dim512 {
    const DIMENSION: usize = 512;
}

pub struct Dim768;
impl FixedDimension for Dim768 {
    const DIMENSION: usize = 768;
}

pub struct Dim1024;
impl FixedDimension for Dim1024 {
    const DIMENSION: usize = 1024;
}

pub struct Dim1536;
impl FixedDimension for Dim1536 {
    const DIMENSION: usize = 1536;
}

pub struct Dim2048;
impl FixedDimension for Dim2048 {
    const DIMENSION: usize = 2048;
}

/// Fixed-length vector wrapper for zero-copy operations
#[repr(C)]
#[derive(Debug, Clone, PartialEq)]
pub struct FixedVector<D: FixedDimension> {
    data: Vec<f32>,
    _dimension: PhantomData<D>,
}

impl<D: FixedDimension> FixedVector<D> {
    /// Create a new fixed vector from a Vec<f32>
    pub fn new(data: Vec<f32>) -> Result<Self> {
        if data.len() != D::DIMENSION {
            return Err(anyhow::anyhow!(
                "Vector dimension mismatch: expected {}, got {}",
                D::DIMENSION,
                data.len()
            ));
        }

        Ok(Self {
            data,
            _dimension: PhantomData,
        })
    }

    /// Create from slice
    pub fn from_slice(slice: &[f32]) -> Result<Self> {
        Self::new(slice.to_vec())
    }

    /// Get the vector data
    pub fn data(&self) -> &[f32] {
        &self.data
    }

    /// Get the dimension of this fixed vector
    pub fn dimension(&self) -> usize {
        D::DIMENSION
    }

    /// Get mutable vector data
    pub fn data_mut(&mut self) -> &mut [f32] {
        &mut self.data
    }

    /// Convert to Vec<f32>
    pub fn into_vec(self) -> Vec<f32> {
        self.data
    }

    /// Get dimension at compile time - static version
    pub const fn dimension_const() -> usize {
        D::DIMENSION
    }

    /// Get byte size at compile time
    pub const fn byte_size() -> usize {
        D::BYTE_SIZE
    }
}

impl<D: FixedDimension> From<Vec<f32>> for FixedVector<D> {
    fn from(data: Vec<f32>) -> Self {
        Self::new(data).expect("Vector dimension must match")
    }
}

impl<D: FixedDimension> AsRef<[f32]> for FixedVector<D> {
    fn as_ref(&self) -> &[f32] {
        &self.data
    }
}

impl<D: FixedDimension> std::ops::Index<usize> for FixedVector<D> {
    type Output = f32;

    fn index(&self, index: usize) -> &Self::Output {
        &self.data[index]
    }
}

impl<D: FixedDimension> std::ops::IndexMut<usize> for FixedVector<D> {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.data[index]
    }
}

/// Configuration for fixed-length vector serialization
#[derive(Debug, Clone)]
pub struct FixedLengthConfig {
    /// Enable compression for vectors above this sparsity threshold
    pub compression_sparsity_threshold: f32,
    /// ZSTD compression level (1-22)
    pub compression_level: i32,
    /// Enable checksum validation
    pub enable_checksum: bool,
    /// Alignment for memory operations (must be power of 2)
    pub alignment: usize,
}

impl Default for FixedLengthConfig {
    fn default() -> Self {
        Self {
            compression_sparsity_threshold: 0.7, // Compress if >70% sparse
            compression_level: 3,                // Balanced speed/compression
            enable_checksum: true,
            alignment: 32, // 32-byte alignment for SIMD
        }
    }
}

/// Fixed-length vector serializer for maximum performance
pub struct FixedLengthSerializer<D: FixedDimension> {
    config: FixedLengthConfig,
    _dimension: PhantomData<D>,
}

/// Serialization format markers
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FixedSerializationFormat {
    /// Raw bytes, no compression
    Raw = 0x10,
    /// ZSTD compressed bytes
    ZstdCompressed = 0x11,
    /// Sparse representation (value + index pairs)
    Sparse = 0x12,
}

/// Header for fixed-length serialized data
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct FixedHeader {
    /// Format marker
    pub format: u8,
    /// Checksum (CRC32)
    pub checksum: u32,
    /// Data length in bytes
    pub data_len: u32,
    /// Reserved for future use
    pub reserved: u8,
}

unsafe impl Pod for FixedHeader {}
unsafe impl Zeroable for FixedHeader {}

impl<D: FixedDimension> FixedLengthSerializer<D> {
    /// Create new serializer
    pub fn new(config: FixedLengthConfig) -> Self {
        Self {
            config,
            _dimension: PhantomData,
        }
    }

    /// Create with default configuration
    pub fn default() -> Self {
        Self::new(FixedLengthConfig::default())
    }

    /// Serialize fixed vector with maximum performance
    pub fn serialize(&self, vector: &FixedVector<D>) -> Result<Vec<u8>> {
        let data = vector.data();

        // Analyze vector characteristics
        let sparsity = self.calculate_sparsity(data);
        let should_compress = sparsity >= self.config.compression_sparsity_threshold;

        // Convert to bytes using bytemuck (zero-copy)
        let raw_bytes = cast_slice(data);
        let checksum = if self.config.enable_checksum {
            crc32fast::hash(raw_bytes)
        } else {
            0
        };

        let (format, serialized_data) = if should_compress {
            // Use ZSTD compression for sparse vectors
            let compressed = encode_all(raw_bytes, self.config.compression_level)
                .context("Failed to compress fixed vector")?;
            (FixedSerializationFormat::ZstdCompressed, compressed)
        } else {
            // Raw format for dense vectors
            (FixedSerializationFormat::Raw, raw_bytes.to_vec())
        };

        // Create header
        let header = FixedHeader {
            format: format as u8,
            checksum,
            data_len: serialized_data.len() as u32,
            reserved: 0,
        };

        // Combine header and data
        let mut result = Vec::with_capacity(size_of::<FixedHeader>() + serialized_data.len());
        result.extend_from_slice(bytemuck::bytes_of(&header));
        result.extend_from_slice(&serialized_data);

        trace!(
            "📦 Fixed-length serialized: {}D vector, {} bytes, format={:?}, sparsity={:.3}",
            D::DIMENSION,
            result.len(),
            format,
            sparsity
        );

        Ok(result)
    }

    /// Deserialize fixed vector with validation
    pub fn deserialize(&self, data: &[u8]) -> Result<FixedVector<D>> {
        if data.len() < size_of::<FixedHeader>() {
            return Err(anyhow::anyhow!("Invalid fixed vector data: too short"));
        }

        // Extract header
        let header_bytes = &data[..size_of::<FixedHeader>()];
        let header: &FixedHeader = bytemuck::from_bytes(header_bytes);
        let payload = &data[size_of::<FixedHeader>()..];

        if payload.len() != header.data_len as usize {
            return Err(anyhow::anyhow!("Fixed vector data length mismatch"));
        }

        // Parse format
        let format = match header.format {
            0x10 => FixedSerializationFormat::Raw,
            0x11 => FixedSerializationFormat::ZstdCompressed,
            0x12 => FixedSerializationFormat::Sparse,
            _ => {
                return Err(anyhow::anyhow!(
                    "Unknown fixed serialization format: {}",
                    header.format
                ));
            }
        };

        // Decompress if needed
        let raw_bytes = match format {
            FixedSerializationFormat::Raw => payload.to_vec(),
            FixedSerializationFormat::ZstdCompressed => {
                decode_all(payload).context("Failed to decompress fixed vector")?
            }
            FixedSerializationFormat::Sparse => {
                return Err(anyhow::anyhow!("Sparse format not yet implemented"));
            }
        };

        // Validate checksum
        if self.config.enable_checksum && header.checksum != 0 {
            let actual_checksum = crc32fast::hash(&raw_bytes);
            if actual_checksum != header.checksum {
                return Err(anyhow::anyhow!("Fixed vector checksum mismatch"));
            }
        }

        // Convert bytes back to f32 slice
        if raw_bytes.len() != D::BYTE_SIZE {
            return Err(anyhow::anyhow!(
                "Fixed vector size mismatch: expected {} bytes, got {}",
                D::BYTE_SIZE,
                raw_bytes.len()
            ));
        }

        let floats = try_cast_slice::<u8, f32>(&raw_bytes)
            .map_err(|e| anyhow::anyhow!("Failed to cast bytes to f32: {}", e))?;

        Ok(FixedVector::new(floats.to_vec())?)
    }

    /// Serialize batch of fixed vectors efficiently  
    pub fn serialize_batch(&self, vectors: &[FixedVector<D>]) -> Result<Vec<u8>> {
        if vectors.is_empty() {
            return Ok(vec![]);
        }

        // Pre-allocate buffer
        let estimated_size = vectors.len() * (D::BYTE_SIZE + size_of::<FixedHeader>() + 16);
        let mut result = Vec::with_capacity(estimated_size);

        // Write batch header
        result.extend_from_slice(&(vectors.len() as u32).to_le_bytes());

        for vector in vectors {
            let serialized = self.serialize(vector)?;
            result.extend_from_slice(&(serialized.len() as u32).to_le_bytes());
            result.extend_from_slice(&serialized);
        }

        debug!(
            "📦 Fixed-length batch serialized: {} vectors, {} bytes",
            vectors.len(),
            result.len()
        );

        Ok(result)
    }

    /// Deserialize batch of fixed vectors
    pub fn deserialize_batch(&self, data: &[u8]) -> Result<Vec<FixedVector<D>>> {
        if data.len() < 4 {
            return Ok(vec![]);
        }

        let mut cursor = 0;

        // Read vector count
        let count_bytes = &data[cursor..cursor + 4];
        let count = u32::from_le_bytes([
            count_bytes[0],
            count_bytes[1],
            count_bytes[2],
            count_bytes[3],
        ]) as usize;
        cursor += 4;

        let mut vectors = Vec::with_capacity(count);

        for _ in 0..count {
            if cursor + 4 > data.len() {
                return Err(anyhow::anyhow!("Invalid batch data: truncated length"));
            }

            // Read vector data length
            let len_bytes = &data[cursor..cursor + 4];
            let len = u32::from_le_bytes([len_bytes[0], len_bytes[1], len_bytes[2], len_bytes[3]])
                as usize;
            cursor += 4;

            if cursor + len > data.len() {
                return Err(anyhow::anyhow!("Invalid batch data: truncated vector"));
            }

            // Deserialize vector
            let vector_data = &data[cursor..cursor + len];
            let vector = self.deserialize(vector_data)?;
            vectors.push(vector);
            cursor += len;
        }

        debug!(
            "📦 Fixed-length batch deserialized: {} vectors",
            vectors.len()
        );

        Ok(vectors)
    }

    /// Calculate sparsity (ratio of zero/near-zero elements)
    fn calculate_sparsity(&self, data: &[f32]) -> f32 {
        let zero_count = data.iter().filter(|&&x| x.abs() < 1e-6).count();

        zero_count as f32 / data.len() as f32
    }

    /// Get compression ratio for a vector
    pub fn compression_ratio(&self, vector: &FixedVector<D>) -> Result<f32> {
        let original_size = D::BYTE_SIZE;
        let compressed = self.serialize(vector)?;
        Ok(compressed.len() as f32 / original_size as f32)
    }

    /// Analyze performance characteristics
    pub fn analyze_vector(&self, vector: &FixedVector<D>) -> FixedVectorAnalysis {
        let data = vector.data();
        let sparsity = self.calculate_sparsity(data);

        let mut min_val = f32::INFINITY;
        let mut max_val = f32::NEG_INFINITY;
        let mut sum = 0.0;
        let mut sum_squares = 0.0;

        for &val in data {
            min_val = min_val.min(val);
            max_val = max_val.max(val);
            sum += val;
            sum_squares += val * val;
        }

        let mean = sum / data.len() as f32;
        let variance = (sum_squares / data.len() as f32) - (mean * mean);

        FixedVectorAnalysis {
            dimension: D::DIMENSION,
            sparsity,
            min_value: min_val,
            max_value: max_val,
            mean,
            variance,
            l2_norm: sum_squares.sqrt(),
        }
    }
}

/// Analysis results for fixed-length vectors
#[derive(Debug, Clone)]
pub struct FixedVectorAnalysis {
    pub dimension: usize,
    pub sparsity: f32,
    pub min_value: f32,
    pub max_value: f32,
    pub mean: f32,
    pub variance: f32,
    pub l2_norm: f32,
}

impl FixedVectorAnalysis {
    pub fn print_summary(&self) {
        debug!("📊 Fixed Vector Analysis ({}D):", self.dimension);
        debug!("   Sparsity: {:.3}", self.sparsity);
        debug!("   Range: [{:.6}, {:.6}]", self.min_value, self.max_value);
        debug!("   Mean: {:.6}, Variance: {:.6}", self.mean, self.variance);
        debug!("   L2 norm: {:.6}", self.l2_norm);
    }
}

/// Type aliases for common dimensions
pub type Vector64 = FixedVector<Dim64>;
pub type Vector128 = FixedVector<Dim128>;
pub type Vector256 = FixedVector<Dim256>;
pub type Vector512 = FixedVector<Dim512>;
pub type Vector768 = FixedVector<Dim768>;
pub type Vector1024 = FixedVector<Dim1024>;
pub type Vector1536 = FixedVector<Dim1536>;
pub type Vector2048 = FixedVector<Dim2048>;

/// Serializer aliases for common dimensions
pub type Serializer64 = FixedLengthSerializer<Dim64>;
pub type Serializer128 = FixedLengthSerializer<Dim128>;
pub type Serializer256 = FixedLengthSerializer<Dim256>;
pub type Serializer512 = FixedLengthSerializer<Dim512>;
pub type Serializer768 = FixedLengthSerializer<Dim768>;
pub type Serializer1024 = FixedLengthSerializer<Dim1024>;
pub type Serializer1536 = FixedLengthSerializer<Dim1536>;
pub type Serializer2048 = FixedLengthSerializer<Dim2048>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fixed_vector_creation() {
        let data = vec![1.0, 2.0, 3.0, 4.0];
        let vector = Vector64::new(vec![0.0; 64]).unwrap();
        assert_eq!(vector.dimension(), 64);
        assert_eq!(vector.data().len(), 64);
    }

    #[test]
    fn test_dimension_mismatch() {
        let data = vec![1.0, 2.0, 3.0]; // Wrong size for 64D
        let result = Vector64::new(data);
        assert!(result.is_err());
    }

    #[test]
    fn test_fixed_serialization_roundtrip() {
        let serializer = Serializer128::default();

        let mut data = vec![0.0; 128];
        for i in 0..10 {
            data[i] = i as f32 * 0.1;
        }

        let vector = Vector128::new(data.clone()).unwrap();
        let serialized = serializer.serialize(&vector).unwrap();
        let deserialized = serializer.deserialize(&serialized).unwrap();

        assert_eq!(vector.data(), deserialized.data());
    }

    #[test]
    fn test_sparse_vector_compression() {
        let mut config = FixedLengthConfig::default();
        config.compression_sparsity_threshold = 0.5;
        let serializer = Serializer512::new(config);

        // Create sparse vector (90% zeros)
        let mut data = vec![0.0; 512];
        for i in (0..512).step_by(10) {
            data[i] = i as f32 * 0.001;
        }

        let vector = Vector512::new(data).unwrap();
        let analysis = serializer.analyze_vector(&vector);
        analysis.print_summary();

        assert!(analysis.sparsity > 0.8);

        let ratio = serializer.compression_ratio(&vector).unwrap();
        assert!(ratio < 0.8, "Sparse vector should compress well");
    }

    #[test]
    fn test_batch_serialization() {
        let serializer = Serializer256::default();

        let vectors = (0..5)
            .map(|i| {
                let mut data = vec![0.0; 256];
                for j in 0..10 {
                    data[j] = (i * 10 + j) as f32 * 0.01;
                }
                Vector256::new(data).unwrap()
            })
            .collect::<Vec<_>>();

        let serialized = serializer.serialize_batch(&vectors).unwrap();
        let deserialized = serializer.deserialize_batch(&serialized).unwrap();

        assert_eq!(vectors.len(), deserialized.len());

        for (original, recovered) in vectors.iter().zip(deserialized.iter()) {
            assert_eq!(original.data(), recovered.data());
        }
    }

    #[test]
    fn test_performance_characteristics() {
        let serializer = Serializer768::default();

        // Dense vector
        let dense_data: Vec<f32> = (0..768).map(|i| i as f32 * 0.001).collect();
        let dense_vector = Vector768::new(dense_data).unwrap();

        // Sparse vector
        let mut sparse_data = vec![0.0; 768];
        for i in (0..768).step_by(20) {
            sparse_data[i] = i as f32 * 0.001;
        }
        let sparse_vector = Vector768::new(sparse_data).unwrap();

        let dense_ratio = serializer.compression_ratio(&dense_vector).unwrap();
        let sparse_ratio = serializer.compression_ratio(&sparse_vector).unwrap();

        debug!(
            "Dense ratio: {:.3}, Sparse ratio: {:.3}",
            dense_ratio, sparse_ratio
        );

        assert!(sparse_ratio < dense_ratio, "Sparse should compress better");
    }

    #[test]
    fn test_checksum_validation() {
        let mut config = FixedLengthConfig::default();
        config.enable_checksum = true;
        let serializer = Serializer128::new(config);

        let vector = Vector128::new(vec![1.0; 128]).unwrap();
        let mut serialized = serializer.serialize(&vector).unwrap();

        // Corrupt the data
        if serialized.len() > 20 {
            serialized[20] = !serialized[20];
        }

        let result = serializer.deserialize(&serialized);
        assert!(result.is_err(), "Should fail checksum validation");
    }
}
