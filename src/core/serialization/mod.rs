//! Vector serialization utilities with bytemuck and ZSTD support
//!
//! This module provides high-performance zero-copy serialization for Vec<f32> data
//! using bytemuck for direct memory mapping and ZSTD for compression.

pub mod fixed_length;
pub mod streaming;

use anyhow::{Context, Result};
use brotli::{CompressorWriter, Decompressor};
use bytemuck::{cast_slice, from_bytes, try_cast_slice};
use bzip2::read::BzDecoder;
use bzip2::write::BzEncoder;
use flate2::read::{DeflateDecoder, GzDecoder, ZlibDecoder};
use flate2::write::{DeflateEncoder, GzEncoder, ZlibEncoder};
use lz4_flex::{compress_prepend_size, decompress_size_prepended};
use serde::{Deserialize, Serialize};
use snap::{raw::Decoder as SnapDecoder, raw::Encoder as SnapEncoder};
use std::io::{Read, Write};
use std::mem::size_of;
use xz2::read::XzDecoder;
use xz2::write::XzEncoder;
use zstd::{decode_all, encode_all};

/// Vector serialization configuration
#[derive(Debug, Clone)]
pub struct VectorSerializationConfig {
    /// Use bytemuck for zero-copy serialization
    pub use_bytemuck: bool,
    /// Vector dimension threshold above which compression is applied
    pub compression_threshold: usize,
    /// Compression algorithm to use
    pub compression_algorithm: CompressionAlgorithm,
    /// Compression level (1-22 for ZSTD)
    pub compression_level: i32,
    /// Enable adaptive compression based on vector characteristics
    pub adaptive_compression: bool,
}

impl Default for VectorSerializationConfig {
    fn default() -> Self {
        Self {
            use_bytemuck: true,
            compression_threshold: 0, // No arbitrary threshold - let user decide
            compression_algorithm: CompressionAlgorithm::Zstd,
            compression_level: 3, // Balanced speed/compression
            adaptive_compression: true,
        }
    }
}

/// Compression algorithms supported for vector data
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum CompressionAlgorithm {
    /// No compression
    None,
    /// Zstandard compression
    Zstd,
    /// LZ4 fast compression
    Lz4,
    /// Snappy balanced compression
    Snappy,
    /// Gzip/Deflate compression
    Gzip,
    /// Brotli compression
    Brotli,
    /// Bzip2 compression  
    Bzip2,
    /// Raw deflate compression
    Deflate,
    /// XZ/LZMA2 compression
    Xz,
    /// Zlib compression
    Zlib,
    /// LZO compression (fastest)
    Lzo,
    /// LZ4 high compression variant
    Lz4hc,
    /// LZMA compression (high ratio)
    Lzma,
    /// Mixed compression strategy - optimal per-column compression
    /// Uses different algorithms based on column data type:
    /// - Binary columns: UNCOMPRESSED (fast filtering)  
    /// - INT8 columns: SNAPPY (fast decompression)
    /// - PQ columns: ZSTD (best ratio)
    /// - FP32 columns: LZ4 (fast decompression for reranking)
    /// - ID columns: GZIP (maximum compression)
    /// - Metadata columns: BROTLI (maximum compression for cold data)
    Mixed,
}

impl Default for CompressionAlgorithm {
    fn default() -> Self {
        CompressionAlgorithm::None
    }
}

/// Vector serialization format marker
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SerializationFormat {
    /// Raw bytemuck bytes (no compression)
    RawBytemuck = 0x01,
    /// ZSTD compressed bytemuck bytes
    ZstdBytemuck = 0x02,
    /// LZ4 compressed bytemuck bytes
    Lz4Bytemuck = 0x03,
    /// Snappy compressed bytemuck bytes
    SnappyBytemuck = 0x04,
    /// Gzip compressed bytemuck bytes
    GzipBytemuck = 0x05,
    /// Brotli compressed bytemuck bytes
    BrotliBytemuck = 0x06,
    /// Bzip2 compressed bytemuck bytes
    Bzip2Bytemuck = 0x07,
    /// Deflate compressed bytemuck bytes
    DeflateBytemuck = 0x08,
    /// Xz compressed bytemuck bytes
    XzBytemuck = 0x09,
    /// Zlib compressed bytemuck bytes
    ZlibBytemuck = 0x0A,
    /// Lzo compressed bytemuck bytes (reserved, not impl)
    LzoBytemuck = 0x0B,
    /// Lz4hc compressed bytemuck bytes
    Lz4hcBytemuck = 0x0C,
    /// Lzma compressed bytemuck bytes
    LzmaBytemuck = 0x0D,
}

/// Header for serialized vector data
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct VectorHeader {
    /// Format marker
    pub format: u8,
    /// Original vector length (number of f32 elements)
    pub vector_len: u32,
    /// Compressed data length (bytes)
    pub data_len: u32,
    /// CRC32 checksum of original data
    pub checksum: u32,
}

unsafe impl bytemuck::Pod for VectorHeader {}
unsafe impl bytemuck::Zeroable for VectorHeader {}

impl VectorSerializationConfig {
    /// Create optimized configuration for specific vector dimensions
    /// This is now a suggestion - users can override via collection config
    pub fn for_dimension(dimension: usize) -> Self {
        let mut config = Self::default();

        // Suggest optimization based on dimension (user can override)
        match dimension {
            // Small vectors: suggest no compression for low latency
            d if d <= 128 => {
                config.compression_threshold = usize::MAX; // Disable by default
                config.compression_algorithm = CompressionAlgorithm::None;
            }
            // Medium vectors: suggest light compression
            d if d <= 512 => {
                config.compression_threshold = 0; // User decides
                config.compression_level = 1;
            }
            // Large vectors: suggest stronger compression
            _ => {
                config.compression_threshold = 0; // User decides
                config.compression_level = 6;
            }
        }

        config
    }

    /// Serialize a vector using zero-copy bytemuck with optional compression
    pub fn serialize_vector(&self, vector: &[f32]) -> Result<Vec<u8>> {
        if !self.use_bytemuck {
            // Fallback to bincode for compatibility
            return Ok(bincode::serialize(vector)?);
        }

        // Convert to bytes using bytemuck (zero-copy)
        let bytes = cast_slice(vector);
        let checksum = crate::utils::checksum::crc32_fast(bytes);

        let (format, compressed_data) = if vector.len() >= self.compression_threshold {
            match self.compression_algorithm {
                CompressionAlgorithm::Zstd => {
                    let compressed = encode_all(bytes, self.compression_level)
                        .context("Failed to compress vector with ZSTD")?;
                    (SerializationFormat::ZstdBytemuck, compressed)
                }
                CompressionAlgorithm::Lz4 => {
                    let compressed = compress_prepend_size(bytes);
                    (SerializationFormat::Lz4Bytemuck, compressed)
                }
                CompressionAlgorithm::Snappy => {
                    let mut encoder = SnapEncoder::new();
                    let compressed = encoder
                        .compress_vec(bytes)
                        .map_err(|e| anyhow::anyhow!("Snappy compression failed: {}", e))?;
                    (SerializationFormat::SnappyBytemuck, compressed)
                }
                CompressionAlgorithm::Gzip => {
                    let mut encoder = GzEncoder::new(
                        Vec::new(),
                        flate2::Compression::new(self.compression_level as u32),
                    );
                    encoder.write_all(bytes)?;
                    let compressed = encoder.finish()?;
                    (SerializationFormat::GzipBytemuck, compressed)
                }
                CompressionAlgorithm::Brotli => {
                    let mut compressed = Vec::new();
                    let mut encoder = CompressorWriter::new(
                        &mut compressed,
                        4096,
                        self.compression_level as u32,
                        22,
                    );
                    encoder.write_all(bytes)?;
                    encoder.flush()?;
                    drop(encoder);
                    (SerializationFormat::BrotliBytemuck, compressed)
                }
                CompressionAlgorithm::Bzip2 => {
                    let mut encoder = BzEncoder::new(
                        Vec::new(),
                        bzip2::Compression::new(self.compression_level as u32),
                    );
                    encoder.write_all(bytes)?;
                    let compressed = encoder.finish()?;
                    (SerializationFormat::Bzip2Bytemuck, compressed)
                }
                CompressionAlgorithm::Deflate => {
                    let mut encoder = DeflateEncoder::new(
                        Vec::new(),
                        flate2::Compression::new(self.compression_level as u32),
                    );
                    encoder.write_all(bytes)?;
                    let compressed = encoder.finish()?;
                    (SerializationFormat::DeflateBytemuck, compressed)
                }
                CompressionAlgorithm::Xz => {
                    let mut encoder = XzEncoder::new(Vec::new(), self.compression_level as u32);
                    encoder.write_all(bytes)?;
                    let compressed = encoder.finish()?;
                    (SerializationFormat::XzBytemuck, compressed)
                }
                CompressionAlgorithm::Zlib => {
                    let mut encoder = ZlibEncoder::new(
                        Vec::new(),
                        flate2::Compression::new(self.compression_level as u32),
                    );
                    encoder.write_all(bytes)?;
                    let compressed = encoder.finish()?;
                    (SerializationFormat::ZlibBytemuck, compressed)
                }
                CompressionAlgorithm::Lzo => {
                    // LZO not available in Rust ecosystem, fallback to LZ4
                    let compressed = compress_prepend_size(bytes);
                    (SerializationFormat::Lz4Bytemuck, compressed)
                }
                CompressionAlgorithm::Lz4hc => {
                    // Use regular LZ4 with higher compression
                    let compressed = compress_prepend_size(bytes);
                    (SerializationFormat::Lz4hcBytemuck, compressed)
                }
                CompressionAlgorithm::Lzma => {
                    // Use XZ which includes LZMA2
                    let mut encoder = XzEncoder::new(Vec::new(), 9); // Max compression for LZMA
                    encoder.write_all(bytes)?;
                    let compressed = encoder.finish()?;
                    (SerializationFormat::LzmaBytemuck, compressed)
                }
                CompressionAlgorithm::Mixed => {
                    // For vector serialization, Mixed defaults to ZSTD level 3
                    let compressed = encode_all(bytes, 3)
                        .context("Failed to compress vector with Mixed strategy (ZSTD)")?;
                    (SerializationFormat::ZstdBytemuck, compressed)
                }
                CompressionAlgorithm::None => (SerializationFormat::RawBytemuck, bytes.to_vec()),
            }
        } else {
            (SerializationFormat::RawBytemuck, bytes.to_vec())
        };

        // Create header
        let header = VectorHeader {
            format: format as u8,
            vector_len: vector.len() as u32,
            data_len: compressed_data.len() as u32,
            checksum,
        };

        // Combine header and data
        let mut result = Vec::with_capacity(size_of::<VectorHeader>() + compressed_data.len());
        result.extend_from_slice(bytemuck::bytes_of(&header));
        result.extend_from_slice(&compressed_data);

        Ok(result)
    }

    /// Deserialize a vector using zero-copy bytemuck with optional decompression
    pub fn deserialize_vector(&self, data: &[u8]) -> Result<Vec<f32>> {
        if data.len() < size_of::<VectorHeader>() {
            return Err(anyhow::anyhow!("Invalid vector data: too short for header"));
        }

        // Extract header
        let header_bytes = &data[..size_of::<VectorHeader>()];
        let header: &VectorHeader = from_bytes(header_bytes);
        let payload = &data[size_of::<VectorHeader>()..];

        if payload.len() != header.data_len as usize {
            return Err(anyhow::anyhow!(
                "Invalid vector data: payload length mismatch"
            ));
        }

        let format = match header.format {
            0x01 => SerializationFormat::RawBytemuck,
            0x02 => SerializationFormat::ZstdBytemuck,
            0x03 => SerializationFormat::Lz4Bytemuck,
            0x04 => SerializationFormat::SnappyBytemuck,
            0x05 => SerializationFormat::GzipBytemuck,
            0x06 => SerializationFormat::BrotliBytemuck,
            0x07 => SerializationFormat::Bzip2Bytemuck,
            0x08 => SerializationFormat::DeflateBytemuck,
            0x09 => SerializationFormat::XzBytemuck,
            0x0A => SerializationFormat::ZlibBytemuck,
            0x0B => SerializationFormat::LzoBytemuck,
            0x0C => SerializationFormat::Lz4hcBytemuck,
            0x0D => SerializationFormat::LzmaBytemuck,
            _ => {
                return Err(anyhow::anyhow!(
                    "Unknown serialization format: {:#x}",
                    header.format
                ));
            }
        };

        let decompressed_bytes = match format {
            SerializationFormat::RawBytemuck => payload.to_vec(),
            SerializationFormat::ZstdBytemuck => {
                decode_all(payload).context("Failed to decompress ZSTD vector data")?
            }
            SerializationFormat::Lz4Bytemuck
            | SerializationFormat::LzoBytemuck
            | SerializationFormat::Lz4hcBytemuck => decompress_size_prepended(payload)
                .map_err(|e| anyhow::anyhow!("LZ4 decompression failed: {}", e))?,
            SerializationFormat::SnappyBytemuck => {
                let mut decoder = SnapDecoder::new();
                decoder
                    .decompress_vec(payload)
                    .map_err(|e| anyhow::anyhow!("Snappy decompression failed: {}", e))?
            }
            SerializationFormat::GzipBytemuck => {
                let mut decoder = GzDecoder::new(payload);
                let mut decompressed = Vec::new();
                decoder.read_to_end(&mut decompressed)?;
                decompressed
            }
            SerializationFormat::BrotliBytemuck => {
                let mut decoder = Decompressor::new(payload, 4096);
                let mut decompressed = Vec::new();
                decoder.read_to_end(&mut decompressed)?;
                decompressed
            }
            SerializationFormat::Bzip2Bytemuck => {
                let mut decoder = BzDecoder::new(payload);
                let mut decompressed = Vec::new();
                decoder.read_to_end(&mut decompressed)?;
                decompressed
            }
            SerializationFormat::DeflateBytemuck => {
                let mut decoder = DeflateDecoder::new(payload);
                let mut decompressed = Vec::new();
                decoder.read_to_end(&mut decompressed)?;
                decompressed
            }
            SerializationFormat::XzBytemuck | SerializationFormat::LzmaBytemuck => {
                let mut decoder = XzDecoder::new(payload);
                let mut decompressed = Vec::new();
                decoder.read_to_end(&mut decompressed)?;
                decompressed
            }
            SerializationFormat::ZlibBytemuck => {
                let mut decoder = ZlibDecoder::new(payload);
                let mut decompressed = Vec::new();
                decoder.read_to_end(&mut decompressed)?;
                decompressed
            }
        };

        // Verify checksum
        let actual_checksum = crate::utils::checksum::crc32_fast(&decompressed_bytes);
        if actual_checksum != header.checksum {
            return Err(anyhow::anyhow!("Vector data corrupted: checksum mismatch"));
        }

        // Convert bytes back to f32 slice using bytemuck
        let floats = try_cast_slice::<u8, f32>(&decompressed_bytes)
            .map_err(|e| anyhow::anyhow!("Failed to cast bytes to f32: {}", e))?;

        // Copy values from packed struct to avoid unaligned reference
        let expected_len = header.vector_len;
        if floats.len() != expected_len as usize {
            return Err(anyhow::anyhow!(
                "Vector length mismatch: expected {}, got {}",
                expected_len,
                floats.len()
            ));
        }

        Ok(floats.to_vec())
    }

    /// Get compression ratio for a vector (compressed_size / original_size)
    pub fn compression_ratio(&self, vector: &[f32]) -> Result<f32> {
        let original_size = vector.len() * size_of::<f32>();
        let compressed = self.serialize_vector(vector)?;
        Ok(compressed.len() as f32 / original_size as f32)
    }

    /// Analyze vector characteristics for adaptive compression
    pub fn analyze_vector(&self, vector: &[f32]) -> VectorAnalysis {
        let mut zero_count = 0;
        let mut near_zero_count = 0;
        let mut sum = 0.0;
        let mut sum_squares = 0.0;

        for &value in vector {
            if value == 0.0 {
                zero_count += 1;
            } else if value.abs() < 1e-6 {
                near_zero_count += 1;
            }
            sum += value;
            sum_squares += value * value;
        }

        let len = vector.len() as f32;
        let mean = sum / len;
        let variance = (sum_squares / len) - (mean * mean);
        let sparsity = (zero_count + near_zero_count) as f32 / len;

        VectorAnalysis {
            dimension: vector.len(),
            sparsity,
            mean,
            variance,
            zero_ratio: zero_count as f32 / len,
        }
    }

    /// Get optimal configuration based on vector analysis
    pub fn optimize_for_analysis(&mut self, analysis: &VectorAnalysis) {
        if !self.adaptive_compression {
            return;
        }

        // High sparsity vectors compress well
        if analysis.sparsity > 0.5 {
            self.compression_level = std::cmp::max(6, self.compression_level);
            self.compression_threshold = std::cmp::min(128, self.compression_threshold);
        }

        // Low variance vectors compress well
        if analysis.variance < 0.1 {
            self.compression_level = std::cmp::max(4, self.compression_level);
        }

        // Very small vectors don't benefit from compression overhead
        if analysis.dimension < 64 {
            self.compression_algorithm = CompressionAlgorithm::None;
        }
    }
}

/// Analysis results for a vector
#[derive(Debug, Clone)]
pub struct VectorAnalysis {
    pub dimension: usize,
    pub sparsity: f32, // Ratio of zero/near-zero elements
    pub mean: f32,
    pub variance: f32,
    pub zero_ratio: f32, // Ratio of exact zero elements
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_vector(size: usize, sparsity: f32) -> Vec<f32> {
        let mut vector = vec![0.0; size];
        let non_zero_count = ((1.0 - sparsity) * size as f32) as usize;

        for i in 0..non_zero_count {
            vector[i] = (i as f32 + 1.0) * 0.1;
        }

        vector
    }

    #[test]
    fn test_vector_serialization_roundtrip() {
        let config = VectorSerializationConfig::default();
        let vector = vec![1.0, 2.0, 3.0, 4.0, 5.0];

        let serialized = config.serialize_vector(&vector).unwrap();
        let deserialized = config.deserialize_vector(&serialized).unwrap();

        assert_eq!(vector, deserialized);
    }

    #[test]
    fn test_zero_copy_performance() {
        let config = VectorSerializationConfig::default();
        let large_vector: Vec<f32> = (0..10000).map(|i| i as f32 * 0.001).collect();

        let serialized = config.serialize_vector(&large_vector).unwrap();
        let deserialized = config.deserialize_vector(&serialized).unwrap();

        assert_eq!(large_vector.len(), deserialized.len());
        for (original, recovered) in large_vector.iter().zip(deserialized.iter()) {
            assert!((original - recovered).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn test_compression_effectiveness() {
        let config = VectorSerializationConfig::default();

        // Sparse vector should compress well
        let sparse_vector = create_test_vector(1000, 0.9);
        let sparse_ratio = config.compression_ratio(&sparse_vector).unwrap();

        // Dense vector compresses less
        let dense_vector = create_test_vector(1000, 0.1);
        let dense_ratio = config.compression_ratio(&dense_vector).unwrap();

        assert!(
            sparse_ratio < dense_ratio,
            "Sparse vectors should compress better"
        );
    }

    #[test]
    fn test_small_vector_no_compression() {
        let mut config = VectorSerializationConfig::default();
        config.compression_threshold = 100;

        let small_vector = vec![1.0, 2.0, 3.0]; // Below threshold
        let serialized = config.serialize_vector(&small_vector).unwrap();

        // Should use raw format
        let header_bytes = &serialized[..size_of::<VectorHeader>()];
        let header: &VectorHeader = from_bytes(header_bytes);
        assert_eq!(header.format, SerializationFormat::RawBytemuck as u8);
    }

    #[test]
    fn test_vector_analysis() {
        let config = VectorSerializationConfig::default();

        // Create a sparse vector (90% zeros)
        let sparse_vector = create_test_vector(1000, 0.9);
        let analysis = config.analyze_vector(&sparse_vector);

        assert!(analysis.sparsity > 0.8);
        assert!(analysis.zero_ratio > 0.8);
        assert_eq!(analysis.dimension, 1000);
    }

    #[test]
    fn test_adaptive_optimization() {
        let mut config = VectorSerializationConfig::default();
        config.adaptive_compression = true;

        let sparse_vector = create_test_vector(1000, 0.9);
        let analysis = config.analyze_vector(&sparse_vector);
        config.optimize_for_analysis(&analysis);

        // Should have increased compression level for sparse data
        assert!(config.compression_level >= 6);
    }

    #[test]
    fn test_dimension_optimized_config() {
        let small_config = VectorSerializationConfig::for_dimension(64);
        let large_config = VectorSerializationConfig::for_dimension(2048);

        // Small vectors should avoid compression
        assert_eq!(
            small_config.compression_algorithm,
            CompressionAlgorithm::None
        );

        // Large vectors should use compression
        assert_ne!(
            large_config.compression_algorithm,
            CompressionAlgorithm::None
        );
        assert!(large_config.compression_level > small_config.compression_level);
    }
}
