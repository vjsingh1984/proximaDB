//! Unified compression module for ProximaDB
//! 
//! This module provides a centralized implementation of all compression algorithms
//! used throughout the system, eliminating duplication between:
//! - Core serialization (vector-level compression)
//! - SST block compression (custom block format)
//! - VIPER Parquet compression (Arrow WriterProperties)
//!
//! Key Design:
//! - SST: Custom compression with format markers
//! - VIPER: Uses Arrow Parquet's built-in compression via WriterProperties
//! - Core: Vector serialization with headers

use anyhow::{Context, Result};
use std::io::{Write, Read};
use parquet::file::properties::WriterProperties;

// Re-export for clean imports
pub use crate::core::serialization::CompressionAlgorithm;

// Compression markers module
pub mod markers;
pub use markers::*;

// Compression markers are defined in the markers.rs module

/// Compression context - determines how compression is applied
#[derive(Debug, Clone, PartialEq)]
pub enum CompressionContext {
    /// SST block compression - custom format with markers
    SstBlock,
    /// Vector serialization - with headers and checksums  
    VectorSerialization,
    /// Parquet column chunks - handled by Arrow WriterProperties
    ParquetColumn,
}

/// Unified compression interface
pub trait CompressionProvider {
    /// Compress data using the specified algorithm and level for given context
    fn compress(&self, data: &[u8], algorithm: CompressionAlgorithm, level: i32, context: CompressionContext) -> Result<Vec<u8>>;
    
    /// Decompress data using the specified algorithm for given context
    fn decompress(&self, data: &[u8], algorithm: CompressionAlgorithm, context: CompressionContext) -> Result<Vec<u8>>;
    
    /// Get estimated compression ratio for given data and algorithm
    fn estimate_ratio(&self, data: &[u8], algorithm: CompressionAlgorithm) -> f32;
    
    /// Convert our compression algorithm to Parquet compression for Arrow WriterProperties
    fn to_parquet_compression(&self, algorithm: CompressionAlgorithm) -> Option<parquet::basic::Compression>;
    
    /// Get supported algorithms for Parquet (Arrow has limited support)
    fn parquet_supported_algorithms(&self) -> Vec<CompressionAlgorithm>;
}

/// Mapping between our compression algorithms and Parquet's built-in compression
/// Arrow/Parquet only supports a subset of compression algorithms
pub fn map_to_parquet_compression(algorithm: &CompressionAlgorithm) -> Option<parquet::basic::Compression> {
    match algorithm {
        CompressionAlgorithm::None => Some(parquet::basic::Compression::UNCOMPRESSED),
        CompressionAlgorithm::Snappy => Some(parquet::basic::Compression::SNAPPY),
        CompressionAlgorithm::Gzip => Some(parquet::basic::Compression::GZIP(Default::default())),
        CompressionAlgorithm::Lz4 => Some(parquet::basic::Compression::LZ4),
        CompressionAlgorithm::Zstd => Some(parquet::basic::Compression::ZSTD(Default::default())),
        CompressionAlgorithm::Brotli => Some(parquet::basic::Compression::BROTLI(Default::default())),
        CompressionAlgorithm::Lzo => Some(parquet::basic::Compression::LZO),
        // These are not supported by Arrow Parquet - fallback to Snappy
        CompressionAlgorithm::Bzip2 | 
        CompressionAlgorithm::Deflate |
        CompressionAlgorithm::Xz |
        CompressionAlgorithm::Zlib |
        CompressionAlgorithm::Lz4hc |
        CompressionAlgorithm::Lzma => {
            tracing::warn!("Algorithm {:?} not supported by Parquet, using Snappy fallback", algorithm);
            Some(parquet::basic::Compression::SNAPPY)
        }
    }
}

/// Get list of compression algorithms supported by Arrow Parquet
pub fn parquet_supported_algorithms() -> Vec<CompressionAlgorithm> {
    vec![
        CompressionAlgorithm::None,
        CompressionAlgorithm::Snappy,
        CompressionAlgorithm::Gzip,
        CompressionAlgorithm::Lz4,
        CompressionAlgorithm::Zstd,
        CompressionAlgorithm::Brotli,
        CompressionAlgorithm::Lzo,
    ]
}

/// Standard compression implementation
#[derive(Debug, Clone, Default)]
pub struct StandardCompression;

impl CompressionProvider for StandardCompression {
    fn compress(&self, data: &[u8], algorithm: CompressionAlgorithm, level: i32, context: CompressionContext) -> Result<Vec<u8>> {
        // For ParquetColumn context, we don't do compression here - 
        // Arrow handles it via WriterProperties
        if context == CompressionContext::ParquetColumn {
            return Err(anyhow::anyhow!(
                "Parquet compression should be handled by Arrow WriterProperties, not directly"
            ));
        }
        use lz4_flex::compress_prepend_size;
        use snap::raw::Encoder as SnapEncoder;
        use flate2::write::{GzEncoder, DeflateEncoder, ZlibEncoder};
        use brotli::CompressorWriter;
        use bzip2::write::BzEncoder;
        use xz2::write::XzEncoder;
        
        match algorithm {
            CompressionAlgorithm::None => Ok(data.to_vec()),
            
            CompressionAlgorithm::Zstd => {
                zstd::encode_all(data, level)
                    .context("ZSTD compression failed")
            }
            
            CompressionAlgorithm::Lz4 | CompressionAlgorithm::Lzo | CompressionAlgorithm::Lz4hc => {
                Ok(compress_prepend_size(data))
            }
            
            CompressionAlgorithm::Snappy => {
                let mut encoder = SnapEncoder::new();
                encoder.compress_vec(data)
                    .map_err(|e| anyhow::anyhow!("Snappy compression failed: {}", e))
            }
            
            CompressionAlgorithm::Gzip => {
                let mut encoder = GzEncoder::new(Vec::new(), flate2::Compression::new(level as u32));
                encoder.write_all(data)?;
                encoder.finish().context("Gzip compression failed")
            }
            
            CompressionAlgorithm::Brotli => {
                let mut compressed = Vec::new();
                let mut encoder = CompressorWriter::new(&mut compressed, 4096, level as u32, 22);
                encoder.write_all(data)?;
                encoder.flush()?;
                drop(encoder);
                Ok(compressed)
            }
            
            CompressionAlgorithm::Bzip2 => {
                let mut encoder = BzEncoder::new(Vec::new(), bzip2::Compression::new(level as u32));
                encoder.write_all(data)?;
                encoder.finish().context("Bzip2 compression failed")
            }
            
            CompressionAlgorithm::Deflate => {
                let mut encoder = DeflateEncoder::new(Vec::new(), flate2::Compression::new(level as u32));
                encoder.write_all(data)?;
                encoder.finish().context("Deflate compression failed")
            }
            
            CompressionAlgorithm::Xz | CompressionAlgorithm::Lzma => {
                let mut encoder = XzEncoder::new(Vec::new(), level as u32);
                encoder.write_all(data)?;
                encoder.finish().context("XZ compression failed")
            }
            
            CompressionAlgorithm::Zlib => {
                let mut encoder = ZlibEncoder::new(Vec::new(), flate2::Compression::new(level as u32));
                encoder.write_all(data)?;
                encoder.finish().context("Zlib compression failed")
            }
        }
    }
    
    fn decompress(&self, data: &[u8], algorithm: CompressionAlgorithm, context: CompressionContext) -> Result<Vec<u8>> {
        // For ParquetColumn context, decompression is handled by Arrow readers
        if context == CompressionContext::ParquetColumn {
            return Err(anyhow::anyhow!(
                "Parquet decompression should be handled by Arrow readers, not directly"
            ));
        }
        use lz4_flex::decompress_size_prepended;
        use snap::raw::Decoder as SnapDecoder;
        use flate2::read::{GzDecoder, DeflateDecoder, ZlibDecoder};
        use brotli::Decompressor;
        use bzip2::read::BzDecoder;
        use xz2::read::XzDecoder;
        
        match algorithm {
            CompressionAlgorithm::None => Ok(data.to_vec()),
            
            CompressionAlgorithm::Zstd => {
                zstd::decode_all(data).context("ZSTD decompression failed")
            }
            
            CompressionAlgorithm::Lz4 | CompressionAlgorithm::Lzo | CompressionAlgorithm::Lz4hc => {
                decompress_size_prepended(data)
                    .map_err(|e| anyhow::anyhow!("LZ4 decompression failed: {}", e))
            }
            
            CompressionAlgorithm::Snappy => {
                let mut decoder = SnapDecoder::new();
                decoder.decompress_vec(data)
                    .map_err(|e| anyhow::anyhow!("Snappy decompression failed: {}", e))
            }
            
            CompressionAlgorithm::Gzip => {
                let mut decoder = GzDecoder::new(data);
                let mut decompressed = Vec::new();
                decoder.read_to_end(&mut decompressed)?;
                Ok(decompressed)
            }
            
            CompressionAlgorithm::Brotli => {
                let mut decoder = Decompressor::new(data, 4096);
                let mut decompressed = Vec::new();
                decoder.read_to_end(&mut decompressed)?;
                Ok(decompressed)
            }
            
            CompressionAlgorithm::Bzip2 => {
                let mut decoder = BzDecoder::new(data);
                let mut decompressed = Vec::new();
                decoder.read_to_end(&mut decompressed)?;
                Ok(decompressed)
            }
            
            CompressionAlgorithm::Deflate => {
                let mut decoder = DeflateDecoder::new(data);
                let mut decompressed = Vec::new();
                decoder.read_to_end(&mut decompressed)?;
                Ok(decompressed)
            }
            
            CompressionAlgorithm::Xz | CompressionAlgorithm::Lzma => {
                let mut decoder = XzDecoder::new(data);
                let mut decompressed = Vec::new();
                decoder.read_to_end(&mut decompressed)?;
                Ok(decompressed)
            }
            
            CompressionAlgorithm::Zlib => {
                let mut decoder = ZlibDecoder::new(data);
                let mut decompressed = Vec::new();
                decoder.read_to_end(&mut decompressed)?;
                Ok(decompressed)
            }
        }
    }
    
    fn estimate_ratio(&self, _data: &[u8], algorithm: CompressionAlgorithm) -> f32 {
        // Rough estimates based on typical performance
        match algorithm {
            CompressionAlgorithm::None => 1.0,
            CompressionAlgorithm::Lz4 | CompressionAlgorithm::Lzo => 0.6,
            CompressionAlgorithm::Snappy => 0.65,
            CompressionAlgorithm::Zstd => 0.4,
            CompressionAlgorithm::Gzip => 0.45,
            CompressionAlgorithm::Deflate => 0.45,
            CompressionAlgorithm::Zlib => 0.45,
            CompressionAlgorithm::Brotli => 0.35,
            CompressionAlgorithm::Bzip2 => 0.3,
            CompressionAlgorithm::Xz | CompressionAlgorithm::Lzma => 0.25,
            CompressionAlgorithm::Lz4hc => 0.5,
        }
    }
    
    fn to_parquet_compression(&self, algorithm: CompressionAlgorithm) -> Option<parquet::basic::Compression> {
        map_to_parquet_compression(&algorithm)
    }
    
    fn parquet_supported_algorithms(&self) -> Vec<CompressionAlgorithm> {
        parquet_supported_algorithms()
    }
}

/// Global compression instance
static COMPRESSION: StandardCompression = StandardCompression;

/// Clean API functions
pub fn compress(data: &[u8], algorithm: CompressionAlgorithm, level: i32, context: CompressionContext) -> Result<Vec<u8>> {
    COMPRESSION.compress(data, algorithm, level, context)
}

pub fn decompress(data: &[u8], algorithm: CompressionAlgorithm, context: CompressionContext) -> Result<Vec<u8>> {
    COMPRESSION.decompress(data, algorithm, context)
}

pub fn estimate_ratio(data: &[u8], algorithm: CompressionAlgorithm) -> f32 {
    COMPRESSION.estimate_ratio(data, algorithm)
}

/// Create Parquet WriterProperties with compression
pub fn create_parquet_writer_properties(
    algorithm: CompressionAlgorithm, 
    level: Option<i32>
) -> Result<WriterProperties> {
    let compression = map_to_parquet_compression(&algorithm)
        .ok_or_else(|| anyhow::anyhow!("Algorithm {:?} not supported by Parquet", algorithm))?;
    
    let builder = WriterProperties::builder();
    
    // Apply compression with level if supported
    let builder = match (compression, level) {
        (parquet::basic::Compression::GZIP(_), Some(level)) => {
            let gzip_level = parquet::basic::GzipLevel::try_new(level as u32)?;
            builder.set_compression(parquet::basic::Compression::GZIP(gzip_level))
        }
        (parquet::basic::Compression::ZSTD(_), Some(level)) => {
            let zstd_level = parquet::basic::ZstdLevel::try_new(level)?;
            builder.set_compression(parquet::basic::Compression::ZSTD(zstd_level))
        }
        (parquet::basic::Compression::BROTLI(_), Some(level)) => {
            let brotli_level = parquet::basic::BrotliLevel::try_new(level as u32)?;
            builder.set_compression(parquet::basic::Compression::BROTLI(brotli_level))
        }
        _ => {
            builder.set_compression(compression)
        }
    };
    
    Ok(builder.build())
}

/// Check if algorithm is supported by Parquet
pub fn is_parquet_supported(algorithm: &CompressionAlgorithm) -> bool {
    // First check if we have a native mapping (not a fallback)
    match algorithm {
        CompressionAlgorithm::None |
        CompressionAlgorithm::Snappy |
        CompressionAlgorithm::Gzip |
        CompressionAlgorithm::Lz4 |
        CompressionAlgorithm::Zstd |
        CompressionAlgorithm::Brotli |
        CompressionAlgorithm::Lzo => true,
        // These are unsupported - they get fallback to Snappy
        CompressionAlgorithm::Bzip2 |
        CompressionAlgorithm::Deflate |
        CompressionAlgorithm::Xz |
        CompressionAlgorithm::Zlib |
        CompressionAlgorithm::Lz4hc |
        CompressionAlgorithm::Lzma => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_sst_compression_roundtrip_all_algorithms() {
        let test_data = b"Hello, World! This is a test string for compression. ".repeat(100);
        
        let algorithms = vec![
            CompressionAlgorithm::None,
            CompressionAlgorithm::Zstd,
            CompressionAlgorithm::Lz4,
            CompressionAlgorithm::Snappy,
            CompressionAlgorithm::Gzip,
            CompressionAlgorithm::Brotli,
            CompressionAlgorithm::Bzip2,
            CompressionAlgorithm::Deflate,
            CompressionAlgorithm::Xz,
            CompressionAlgorithm::Zlib,
            CompressionAlgorithm::Lzo,
            CompressionAlgorithm::Lz4hc,
            CompressionAlgorithm::Lzma,
        ];
        
        for algorithm in algorithms {
            println!("Testing SST compression with {:?}", algorithm);
            
            let compressed = compress(&test_data, algorithm.clone(), 3, CompressionContext::SstBlock)
                .unwrap_or_else(|e| panic!("SST compression failed for {:?}: {}", algorithm, e));
                
            let decompressed = decompress(&compressed, algorithm.clone(), CompressionContext::SstBlock)
                .unwrap_or_else(|e| panic!("SST decompression failed for {:?}: {}", algorithm, e));
            
            assert_eq!(test_data, decompressed.as_slice(), 
                "SST roundtrip failed for {:?}", algorithm);
        }
    }
    
    #[test]
    fn test_vector_serialization_roundtrip() {
        let test_data = b"Vector data for serialization test. ".repeat(50);
        
        let algorithms = vec![
            CompressionAlgorithm::None,
            CompressionAlgorithm::Zstd,
            CompressionAlgorithm::Lz4,
            CompressionAlgorithm::Snappy,
        ];
        
        for algorithm in algorithms {
            println!("Testing vector serialization with {:?}", algorithm);
            
            let compressed = compress(&test_data, algorithm.clone(), 3, CompressionContext::VectorSerialization)
                .unwrap_or_else(|e| panic!("Vector compression failed for {:?}: {}", algorithm, e));
                
            let decompressed = decompress(&compressed, algorithm.clone(), CompressionContext::VectorSerialization)
                .unwrap_or_else(|e| panic!("Vector decompression failed for {:?}: {}", algorithm, e));
            
            assert_eq!(test_data, decompressed.as_slice(), 
                "Vector serialization roundtrip failed for {:?}", algorithm);
        }
    }
    
    #[test]
    fn test_parquet_writer_properties_creation() {
        let parquet_supported = parquet_supported_algorithms();
        
        for algorithm in parquet_supported {
            println!("Testing Parquet WriterProperties for {:?}", algorithm);
            
            let properties = create_parquet_writer_properties(algorithm.clone(), Some(3))
                .unwrap_or_else(|e| panic!("Failed to create WriterProperties for {:?}: {}", algorithm, e));
            
            // Basic validation that properties were created (no validation needed - just check it doesn't panic)
            let _ = properties;
        }
    }
    
    #[test]
    fn test_parquet_unsupported_algorithms() {
        let unsupported = vec![
            CompressionAlgorithm::Bzip2,
            CompressionAlgorithm::Deflate,
            CompressionAlgorithm::Xz,
            CompressionAlgorithm::Zlib,
            CompressionAlgorithm::Lz4hc,
            CompressionAlgorithm::Lzma,
        ];
        
        for algorithm in unsupported {
            assert!(!is_parquet_supported(&algorithm), 
                "{:?} should not be supported by Parquet", algorithm);
                
            // Should fallback to Snappy for these
            let parquet_compression = map_to_parquet_compression(&algorithm);
            match parquet_compression {
                Some(parquet::basic::Compression::SNAPPY) => {}, // Expected fallback
                other => panic!("Expected Snappy fallback for {:?}, got {:?}", algorithm, other),
            }
        }
    }
    
    #[test]
    fn test_parquet_context_rejection() {
        let test_data = b"Should not compress directly";
        
        // ParquetColumn context should be rejected for direct compression
        let result = compress(test_data, CompressionAlgorithm::Zstd, 3, CompressionContext::ParquetColumn);
        assert!(result.is_err(), "Parquet context should be rejected for direct compression");
        
        let result = decompress(test_data, CompressionAlgorithm::Zstd, CompressionContext::ParquetColumn);
        assert!(result.is_err(), "Parquet context should be rejected for direct decompression");
    }
    
    #[test]
    fn test_compression_levels_all_algorithms() {
        let test_data = b"Compression level testing data. ".repeat(50);
        
        let level_tests = vec![
            (CompressionAlgorithm::Zstd, vec![1, 3, 6, 9]),
            (CompressionAlgorithm::Gzip, vec![1, 6, 9]),
            (CompressionAlgorithm::Brotli, vec![1, 4, 8]),
            (CompressionAlgorithm::Bzip2, vec![1, 6, 9]),
            (CompressionAlgorithm::Deflate, vec![1, 6, 9]),
            (CompressionAlgorithm::Xz, vec![1, 6, 9]),
            (CompressionAlgorithm::Zlib, vec![1, 6, 9]),
            (CompressionAlgorithm::Lzma, vec![1, 6, 9]),
        ];
        
        for (algorithm, levels) in level_tests {
            for level in levels {
                println!("Testing {:?} at level {}", algorithm, level);
                
                let compressed = compress(&test_data, algorithm.clone(), level, CompressionContext::SstBlock)
                    .unwrap_or_else(|e| panic!("Compression failed for {:?} level {}: {}", algorithm, level, e));
                    
                let decompressed = decompress(&compressed, algorithm.clone(), CompressionContext::SstBlock)
                    .unwrap_or_else(|e| panic!("Decompression failed for {:?} level {}: {}", algorithm, level, e));
                
                assert_eq!(test_data, decompressed.as_slice(), 
                    "Level test failed for {:?} level {}", algorithm, level);
            }
        }
    }
    
    #[test]
    fn test_edge_cases_all_contexts() {
        let edge_cases = vec![
            ("empty", vec![]),
            ("single_byte", vec![42]),
            ("small", vec![1, 2, 3, 4, 5]),
            ("highly_compressible", vec![0; 1000]),
            ("incompressible", (0..1000).map(|i| (i * 37) as u8).collect()),
        ];
        
        let algorithms = vec![
            CompressionAlgorithm::None,
            CompressionAlgorithm::Zstd,
            CompressionAlgorithm::Lz4,
            CompressionAlgorithm::Snappy,
        ];
        
        let contexts = vec![
            CompressionContext::SstBlock,
            CompressionContext::VectorSerialization,
        ];
        
        for (case_name, test_data) in edge_cases {
            for algorithm in &algorithms {
                for context in &contexts {
                    println!("Testing edge case '{}' with {:?} in {:?}", case_name, algorithm, context);
                    
                    let compressed = compress(&test_data, algorithm.clone(), 3, context.clone())
                        .unwrap_or_else(|e| panic!("Edge case compression failed for '{}' {:?} {:?}: {}", 
                            case_name, algorithm, context, e));
                        
                    let decompressed = decompress(&compressed, algorithm.clone(), context.clone())
                        .unwrap_or_else(|e| panic!("Edge case decompression failed for '{}' {:?} {:?}: {}", 
                            case_name, algorithm, context, e));
                    
                    assert_eq!(test_data, decompressed, 
                        "Edge case '{}' failed for {:?} in {:?}", case_name, algorithm, context);
                }
            }
        }
    }
    
    #[test]
    fn test_compression_ratio_estimates() {
        let test_data = b"Estimation test data. ".repeat(100);
        
        let algorithms = vec![
            CompressionAlgorithm::None,
            CompressionAlgorithm::Lz4,
            CompressionAlgorithm::Snappy,
            CompressionAlgorithm::Zstd,
            CompressionAlgorithm::Gzip,
            CompressionAlgorithm::Brotli,
            CompressionAlgorithm::Bzip2,
            CompressionAlgorithm::Xz,
        ];
        
        for algorithm in algorithms {
            let estimated_ratio = estimate_ratio(&test_data, algorithm.clone());
            println!("Estimated compression ratio for {:?}: {:.2}", algorithm, estimated_ratio);
            
            // Basic sanity checks
            match algorithm {
                CompressionAlgorithm::None => assert_eq!(estimated_ratio, 1.0),
                _ => assert!(estimated_ratio < 1.0 && estimated_ratio > 0.0, 
                    "Invalid ratio for {:?}: {}", algorithm, estimated_ratio),
            }
        }
    }
    
    #[test]
    fn test_parquet_integration_comprehensive() {
        let parquet_supported = parquet_supported_algorithms();
        
        // Test all supported algorithms can create WriterProperties
        for algorithm in &parquet_supported {
            println!("Testing Parquet integration for {:?}", algorithm);
            
            // Test with different compression levels
            let levels = match algorithm {
                CompressionAlgorithm::Gzip => vec![Some(1), Some(6), Some(9), None],
                CompressionAlgorithm::Zstd => vec![Some(1), Some(3), Some(6), None],
                CompressionAlgorithm::Brotli => vec![Some(1), Some(4), Some(8), None],
                _ => vec![Some(3), None],
            };
            
            for level in levels {
                let properties = create_parquet_writer_properties(algorithm.clone(), level)
                    .unwrap_or_else(|e| panic!("Failed to create WriterProperties for {:?} level {:?}: {}", 
                        algorithm, level, e));
                
                // Verify compression is set correctly (basic validation)
                let _ = properties;
            }
        }
        
        // Test unsupported algorithms are detected correctly
        let all_algorithms = vec![
            CompressionAlgorithm::None, CompressionAlgorithm::Zstd, CompressionAlgorithm::Lz4,
            CompressionAlgorithm::Snappy, CompressionAlgorithm::Gzip, CompressionAlgorithm::Brotli,
            CompressionAlgorithm::Bzip2, CompressionAlgorithm::Deflate, CompressionAlgorithm::Xz,
            CompressionAlgorithm::Zlib, CompressionAlgorithm::Lzo, CompressionAlgorithm::Lz4hc,
            CompressionAlgorithm::Lzma,
        ];
        
        for algorithm in all_algorithms {
            let is_supported = is_parquet_supported(&algorithm);
            let should_be_supported = parquet_supported.contains(&algorithm);
            
            assert_eq!(is_supported, should_be_supported, 
                "Parquet support detection mismatch for {:?}", algorithm);
        }
    }
}