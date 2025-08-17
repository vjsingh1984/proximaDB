/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Engine Capabilities Module
//! 
//! Centralized module for checking storage engine capabilities and feature support.
//! This module provides static methods to check what features each engine supports
//! without needing to instantiate engine instances.

use crate::proto::proximadb::{CompressionAlgorithm, StorageEngine};
use std::collections::HashSet;

/// Engine capabilities checker - provides static methods for feature support queries
pub struct EngineCapabilities;

impl EngineCapabilities {
    /// Check if a compression algorithm is supported by a given storage engine
    pub fn is_compression_supported(engine: StorageEngine, algorithm: CompressionAlgorithm) -> bool {
        let supported = Self::get_supported_compression_algorithms(engine);
        supported.contains_hash(&algorithm)
    }
    
    /// Get all supported compression algorithms for a storage engine
    pub fn get_supported_compression_algorithms(engine: StorageEngine) -> HashSet<CompressionAlgorithm> {
        match engine {
            StorageEngine::Sst => {
                // SST supports all algorithms except LZO (no Rust implementation)
                let mut supported = HashSet::new();
                supported.insert(CompressionAlgorithm::CompressionNone);
                supported.insert(CompressionAlgorithm::CompressionZstd);
                supported.insert(CompressionAlgorithm::CompressionLz4);
                supported.insert(CompressionAlgorithm::CompressionSnappy);
                supported.insert(CompressionAlgorithm::CompressionGzip);
                supported.insert(CompressionAlgorithm::CompressionBrotli);
                supported.insert(CompressionAlgorithm::CompressionBzip2);
                supported.insert(CompressionAlgorithm::CompressionDeflate);
                supported.insert(CompressionAlgorithm::CompressionXz);
                supported.insert(CompressionAlgorithm::CompressionZlib);
                supported.insert(CompressionAlgorithm::CompressionLz4hc);
                supported.insert(CompressionAlgorithm::CompressionLzma);
                // LZO not supported
                supported
            }
            StorageEngine::Viper => {
                // VIPER uses Parquet which has limited compression support
                let mut supported = HashSet::new();
                supported.insert(CompressionAlgorithm::CompressionNone);
                supported.insert(CompressionAlgorithm::CompressionZstd);
                supported.insert(CompressionAlgorithm::CompressionLz4);
                supported.insert(CompressionAlgorithm::CompressionSnappy);
                supported.insert(CompressionAlgorithm::CompressionGzip);
                supported.insert(CompressionAlgorithm::CompressionBrotli);
                supported
            }
            _ => {
                // Unknown engine - return empty set (no support)
                HashSet::new()
            }
        }
    }
    
    /// Get unsupported compression algorithms for a storage engine
    pub fn get_unsupported_compression_algorithms(engine: StorageEngine) -> Vec<CompressionAlgorithm> {
        let supported = Self::get_supported_compression_algorithms(engine);
        let mut unsupported = Vec::new();
        
        // Check all possible algorithms
        let all_algorithms = vec![
            CompressionAlgorithm::CompressionNone,
            CompressionAlgorithm::CompressionZstd,
            CompressionAlgorithm::CompressionLz4,
            CompressionAlgorithm::CompressionSnappy,
            CompressionAlgorithm::CompressionGzip,
            CompressionAlgorithm::CompressionBrotli,
            CompressionAlgorithm::CompressionBzip2,
            CompressionAlgorithm::CompressionDeflate,
            CompressionAlgorithm::CompressionXz,
            CompressionAlgorithm::CompressionZlib,
            CompressionAlgorithm::CompressionLzo,
            CompressionAlgorithm::CompressionLz4hc,
            CompressionAlgorithm::CompressionLzma,
        ];
        
        for algo in all_algorithms {
            if !supported.contains_hash(&algo) {
                unsupported.push(algo);
            }
        }
        
        unsupported
    }
    
    /// Get recommended compression algorithm for a use case
    pub fn get_recommended_compression(
        engine: StorageEngine, 
        priority: CompressionPriority
    ) -> CompressionAlgorithm {
        match (engine, priority) {
            // SST engine recommendations
            (StorageEngine::Sst, CompressionPriority::Speed) => CompressionAlgorithm::CompressionLz4,
            (StorageEngine::Sst, CompressionPriority::Balanced) => CompressionAlgorithm::CompressionZstd,
            (StorageEngine::Sst, CompressionPriority::Ratio) => CompressionAlgorithm::CompressionBrotli,
            
            // VIPER engine recommendations
            (StorageEngine::Viper, CompressionPriority::Speed) => CompressionAlgorithm::CompressionSnappy,
            (StorageEngine::Viper, CompressionPriority::Balanced) => CompressionAlgorithm::CompressionZstd,
            (StorageEngine::Viper, CompressionPriority::Ratio) => CompressionAlgorithm::CompressionBrotli,
            
            // Default
            _ => CompressionAlgorithm::CompressionNone,
        }
    }
    
    /// Get optimal compression level for an algorithm
    pub fn get_optimal_compression_level(
        algorithm: CompressionAlgorithm,
        priority: CompressionPriority
    ) -> i32 {
        match (algorithm, priority) {
            // ZSTD levels (1-22)
            (CompressionAlgorithm::CompressionZstd, CompressionPriority::Speed) => 1,
            (CompressionAlgorithm::CompressionZstd, CompressionPriority::Balanced) => 3,
            (CompressionAlgorithm::CompressionZstd, CompressionPriority::Ratio) => 9,
            
            // LZ4 doesn't have levels in lz4_flex
            (CompressionAlgorithm::CompressionLz4, _) => 0,
            (CompressionAlgorithm::CompressionLz4hc, _) => 9,
            
            // Gzip/Deflate/Zlib levels (0-9)
            (CompressionAlgorithm::CompressionGzip, CompressionPriority::Speed) => 1,
            (CompressionAlgorithm::CompressionGzip, CompressionPriority::Balanced) => 6,
            (CompressionAlgorithm::CompressionGzip, CompressionPriority::Ratio) => 9,
            
            (CompressionAlgorithm::CompressionDeflate, CompressionPriority::Speed) => 1,
            (CompressionAlgorithm::CompressionDeflate, CompressionPriority::Balanced) => 6,
            (CompressionAlgorithm::CompressionDeflate, CompressionPriority::Ratio) => 9,
            
            (CompressionAlgorithm::CompressionZlib, CompressionPriority::Speed) => 1,
            (CompressionAlgorithm::CompressionZlib, CompressionPriority::Balanced) => 6,
            (CompressionAlgorithm::CompressionZlib, CompressionPriority::Ratio) => 9,
            
            // Brotli levels (0-11)
            (CompressionAlgorithm::CompressionBrotli, CompressionPriority::Speed) => 1,
            (CompressionAlgorithm::CompressionBrotli, CompressionPriority::Balanced) => 4,
            (CompressionAlgorithm::CompressionBrotli, CompressionPriority::Ratio) => 11,
            
            // Bzip2 levels (1-9)
            (CompressionAlgorithm::CompressionBzip2, CompressionPriority::Speed) => 1,
            (CompressionAlgorithm::CompressionBzip2, CompressionPriority::Balanced) => 5,
            (CompressionAlgorithm::CompressionBzip2, CompressionPriority::Ratio) => 9,
            
            // XZ/LZMA levels (0-9)
            (CompressionAlgorithm::CompressionXz, CompressionPriority::Speed) => 0,
            (CompressionAlgorithm::CompressionXz, CompressionPriority::Balanced) => 6,
            (CompressionAlgorithm::CompressionXz, CompressionPriority::Ratio) => 9,
            
            (CompressionAlgorithm::CompressionLzma, CompressionPriority::Speed) => 0,
            (CompressionAlgorithm::CompressionLzma, CompressionPriority::Balanced) => 6,
            (CompressionAlgorithm::CompressionLzma, CompressionPriority::Ratio) => 9,
            
            // Snappy doesn't have levels
            (CompressionAlgorithm::CompressionSnappy, _) => 0,
            
            // No compression
            (CompressionAlgorithm::CompressionNone, _) => 0,
            
            // LZO not supported
            (CompressionAlgorithm::CompressionLzo, _) => 0,
        }
    }
    
    /// Check if an engine supports a specific storage feature
    pub fn is_feature_supported(engine: StorageEngine, feature: StorageFeature) -> bool {
        match feature {
            StorageFeature::Quantization => matches!(engine, StorageEngine::Viper),
            StorageFeature::FilterPushdown => matches!(engine, StorageEngine::Viper),
            StorageFeature::ColumnProjection => matches!(engine, StorageEngine::Viper),
            StorageFeature::BloomFilter => matches!(engine, StorageEngine::Sst),
            StorageFeature::AtomicFlush => true, // All engines support atomic flush
            StorageFeature::Compaction => true,  // All engines support compaction
            StorageFeature::TieredStorage => matches!(engine, StorageEngine::Sst),
            StorageFeature::CacheOptimized => matches!(engine, StorageEngine::Sst),
        }
    }
    
    /// Get the engine name as a string
    pub fn get_engine_name(engine: StorageEngine) -> &'static str {
        match engine {
            StorageEngine::Sst => "SST",
            StorageEngine::Viper => "VIPER",
            _ => "Unknown",
        }
    }
    
    /// Convert integer engine type to StorageEngine enum
    pub fn engine_from_int(engine_type: i32) -> StorageEngine {
        match engine_type {
            1 => StorageEngine::Sst,
            2 => StorageEngine::Viper,
            _ => StorageEngine::Unspecified,
        }
    }
}

/// Compression priority for choosing algorithms and levels
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionPriority {
    /// Optimize for compression/decompression speed
    Speed,
    /// Balance between speed and compression ratio
    Balanced,
    /// Optimize for best compression ratio
    Ratio,
}

/// Storage features that may or may not be supported by engines
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageFeature {
    /// Vector quantization support
    Quantization,
    /// Predicate pushdown for filtering
    FilterPushdown,
    /// Column projection for selective reads
    ColumnProjection,
    /// Bloom filter support
    BloomFilter,
    /// Atomic flush operations
    AtomicFlush,
    /// Background compaction
    Compaction,
    /// Tiered storage with hot/cold separation
    TieredStorage,
    /// Cache-optimized storage format
    CacheOptimized,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_sst_compression_support() {
        // SST should support all algorithms except LZO
        assert!(EngineCapabilities::is_compression_supported(
            StorageEngine::Sst,
            CompressionAlgorithm::CompressionZstd
        ));
        assert!(EngineCapabilities::is_compression_supported(
            StorageEngine::Sst,
            CompressionAlgorithm::CompressionBrotli
        ));
        assert!(!EngineCapabilities::is_compression_supported(
            StorageEngine::Sst,
            CompressionAlgorithm::CompressionLzo
        ));
    }
    
    #[test]
    fn test_viper_compression_support() {
        // VIPER should support limited set
        assert!(EngineCapabilities::is_compression_supported(
            StorageEngine::Viper,
            CompressionAlgorithm::CompressionZstd
        ));
        assert!(EngineCapabilities::is_compression_supported(
            StorageEngine::Viper,
            CompressionAlgorithm::CompressionSnappy
        ));
        assert!(!EngineCapabilities::is_compression_supported(
            StorageEngine::Viper,
            CompressionAlgorithm::CompressionBzip2
        ));
        assert!(!EngineCapabilities::is_compression_supported(
            StorageEngine::Viper,
            CompressionAlgorithm::CompressionLzma
        ));
    }
    
    #[test]
    fn test_compression_recommendations() {
        // Test speed optimized recommendations
        assert_eq!(
            EngineCapabilities::get_recommended_compression(
                StorageEngine::Sst,
                CompressionPriority::Speed
            ),
            CompressionAlgorithm::CompressionLz4
        );
        
        assert_eq!(
            EngineCapabilities::get_recommended_compression(
                StorageEngine::Viper,
                CompressionPriority::Speed
            ),
            CompressionAlgorithm::CompressionSnappy
        );
        
        // Test ratio optimized recommendations
        assert_eq!(
            EngineCapabilities::get_recommended_compression(
                StorageEngine::Sst,
                CompressionPriority::Ratio
            ),
            CompressionAlgorithm::CompressionBrotli
        );
    }
    
    #[test]
    fn test_compression_levels() {
        // Test ZSTD levels
        assert_eq!(
            EngineCapabilities::get_optimal_compression_level(
                CompressionAlgorithm::CompressionZstd,
                CompressionPriority::Speed
            ),
            1
        );
        assert_eq!(
            EngineCapabilities::get_optimal_compression_level(
                CompressionAlgorithm::CompressionZstd,
                CompressionPriority::Ratio
            ),
            9
        );
        
        // Test Brotli levels
        assert_eq!(
            EngineCapabilities::get_optimal_compression_level(
                CompressionAlgorithm::CompressionBrotli,
                CompressionPriority::Ratio
            ),
            11
        );
    }
    
    #[test]
    fn test_feature_support() {
        // VIPER should support quantization
        assert!(EngineCapabilities::is_feature_supported(
            StorageEngine::Viper,
            StorageFeature::Quantization
        ));
        
        // SST should support bloom filters
        assert!(EngineCapabilities::is_feature_supported(
            StorageEngine::Sst,
            StorageFeature::BloomFilter
        ));
        
        // SST should not support quantization
        assert!(!EngineCapabilities::is_feature_supported(
            StorageEngine::Sst,
            StorageFeature::Quantization
        ));
    }
}