//! SST Compression Integration Example
//! 
//! This file demonstrates how SST engine SHOULD integrate with the
//! UniversalCompressionAdapter to benefit from adaptive compression
//! and eliminate potential code duplication.

use anyhow::Result;
use std::sync::Arc;
use tracing::{debug, info};

use crate::storage::engines::common::{
    UniversalCompressionAdapter,
    UniversalCompressionConfig,
    compression_common::{
        AdaptiveCompressionSettings, AdaptiveStrategy,
        ContextAwareCompressionConfig, CompressionDataType,
    },
};
use crate::core::compression::CompressionAlgorithm;
use super::{DataBlock, SstableWriter};

/// Enhanced SSTable Writer with Universal Compression Integration
pub struct EnhancedSstableWriter {
    /// Universal compression adapter (replaces direct compression)
    compression_adapter: Arc<UniversalCompressionAdapter>,
    
    /// Base writer functionality
    base_writer: SstableWriter,
}

impl EnhancedSstableWriter {
    /// Create new writer with universal compression
    pub fn new(path: &std::path::Path) -> Result<Self> {
        let compression_adapter = Arc::new(UniversalCompressionAdapter::new()?);
        let base_writer = SstableWriter::new(path)?;
        
        Ok(Self {
            compression_adapter,
            base_writer,
        })
    }
    
    /// Compress a data block using universal adapter with SST-specific optimization
    pub fn compress_block(&mut self, block: &DataBlock) -> Result<Vec<u8>> {
        // Serialize the block first
        let serialized = block.serialize()?;
        
        // Create SST-specific universal configuration
        let config = self.create_sst_compression_config(&serialized);
        
        // Use universal adapter which will:
        // 1. Analyze data characteristics
        // 2. Select optimal algorithm (adaptive)
        // 3. Apply context-aware compression
        // 4. Track performance statistics
        let compressed = self.compression_adapter
            .compress_with_universal_config(&serialized, &config)?;
        
        info!(
            "Block compressed: {} -> {} bytes (ratio: {:.2}:1, algorithm: {:?})",
            serialized.len(),
            compressed.compressed_size,
            compressed.original_size as f64 / compressed.compressed_size as f64,
            compressed.algorithm
        );
        
        // Return compressed data
        Ok(compressed.data)
    }
    
    /// Create SST-specific compression configuration
    fn create_sst_compression_config(&self, data: &[u8]) -> UniversalCompressionConfig {
        UniversalCompressionConfig {
            enabled: true,
            
            // Default algorithm (will be overridden by adaptive if enabled)
            primary_algorithm: CompressionAlgorithm::Zstd,
            fallback_algorithms: vec![
                CompressionAlgorithm::Lz4,
                CompressionAlgorithm::Snappy,
            ],
            compression_level: 6,
            
            // Adaptive compression - KEY BENEFIT!
            adaptive_settings: AdaptiveCompressionSettings {
                enabled: true,
                strategy: AdaptiveStrategy::DataDriven, // Analyze data patterns
                fallback_algorithms: vec![
                    CompressionAlgorithm::Zstd,  // High compression
                    CompressionAlgorithm::Lz4,   // Fast compression
                    CompressionAlgorithm::None,  // Skip if not compressible
                ],
                performance_target: Some(10), // 10ms target
            },
            
            // Context-aware compression - SST SPECIFIC!
            context_aware: ContextAwareCompressionConfig {
                data_type: CompressionDataType::SstBlock,
                size_hint: Some(data.len()),
                access_pattern: Some(AccessPattern::Sequential), // SST is sequential
            },
            
            // Hardware optimizations
            hardware_optimizations: CompressionHardwareConfig {
                enable_simd: true,
                enable_parallel: data.len() > 1_000_000, // Parallel for large blocks
                thread_count: None, // Auto-detect
            },
            
            // Performance configuration
            performance_config: CompressionPerformanceConfig {
                max_compression_time_ms: Some(50),
                min_compression_ratio: Some(1.2), // Skip if < 20% savings
                enable_statistics: true,
            },
            
            // Quality settings
            quality_settings: CompressionQualityConfig {
                priority: CompressionPriority::Balanced,
                error_tolerance: 0.0, // Lossless
            },
        }
    }
    
    /// Write vectors with automatic adaptive compression
    pub async fn write_vectors(&mut self, vectors: Vec<VectorRecord>) -> Result<()> {
        debug!("Writing {} vectors with universal compression", vectors.len());
        
        // Organize into blocks
        let blocks = self.organize_into_blocks(vectors)?;
        
        // Compress each block with adaptive algorithm selection
        for block in blocks {
            let compressed = self.compress_block(&block)?;
            
            // Write compressed block
            self.base_writer.write_compressed_block(compressed)?;
        }
        
        // Get compression statistics
        let stats = self.compression_adapter.get_performance_stats();
        info!(
            "Compression statistics: {} blocks, avg ratio: {:.2}, throughput: {:.2} MB/s",
            stats.total_compressions,
            stats.average_compression_ratio(),
            stats.compression_throughput_mbps()
        );
        
        Ok(())
    }
}

/// Example: Demonstrating the benefits of universal compression
#[cfg(test)]
mod integration_tests {
    use super::*;
    
    #[tokio::test]
    async fn test_adaptive_compression_benefits() {
        let mut writer = EnhancedSstableWriter::new("/tmp/test.sst").unwrap();
        
        // Test 1: Highly compressible data (should select Zstd)
        let repetitive_block = DataBlock {
            vectors: vec![create_repetitive_vector(); 100],
            // ... other fields
        };
        
        let compressed1 = writer.compress_block(&repetitive_block).unwrap();
        // Adapter should automatically select Zstd for high compression
        
        // Test 2: Random data (should select Lz4 or None)
        let random_block = DataBlock {
            vectors: vec![create_random_vector(); 100],
            // ... other fields
        };
        
        let compressed2 = writer.compress_block(&random_block).unwrap();
        // Adapter should select Lz4 for speed or None if not compressible
        
        // Test 3: Mixed data (should adapt per block)
        let mixed_block = DataBlock {
            vectors: vec![
                create_repetitive_vector(),
                create_random_vector(),
                create_sparse_vector(),
            ],
            // ... other fields
        };
        
        let compressed3 = writer.compress_block(&mixed_block).unwrap();
        // Adapter makes intelligent choice based on overall characteristics
        
        // The key benefit: Each block gets optimal compression automatically!
        // No need for SST to implement its own adaptive logic
    }
}

// ================================================================================
// MIGRATION GUIDE: How to integrate this into existing SST
// ================================================================================

/// Step 1: Add compression adapter to SstableWriter
impl SstableWriter {
    pub fn with_universal_compression(mut self) -> Result<Self> {
        self.compression_adapter = Some(Arc::new(UniversalCompressionAdapter::new()?));
        Ok(self)
    }
}

/// Step 2: Update compress_block to use adapter when available
impl SstableWriter {
    fn compress_block_internal(&mut self, block: &DataBlock) -> Result<Vec<u8>> {
        if let Some(adapter) = &self.compression_adapter {
            // Use universal adapter with all its benefits
            let config = create_universal_config(block);
            let compressed = adapter.compress_with_universal_config(
                &block.serialize()?,
                &config
            )?;
            Ok(compressed.data)
        } else {
            // Fallback to existing direct compression
            self.compress_block_direct(block)
        }
    }
}

/// Step 3: Gradually migrate all compression calls to use the adapter

// ================================================================================
// BENEFITS OF INTEGRATION
// ================================================================================
//
// 1. ADAPTIVE ALGORITHM SELECTION
//    - Automatically chooses best algorithm per block
//    - No manual tuning required
//    - Adapts to data characteristics
//
// 2. PERFORMANCE OPTIMIZATION
//    - Hardware acceleration (SIMD, parallel)
//    - Performance targets (time budgets)
//    - Skip compression if not beneficial
//
// 3. UNIFIED STATISTICS
//    - Consistent metrics across all engines
//    - Performance tracking built-in
//    - Compression ratio monitoring
//
// 4. FUTURE ENHANCEMENTS
//    - New algorithms automatically available
//    - ML-based algorithm selection (future)
//    - Cloud-specific optimizations
//
// 5. CODE SIMPLIFICATION
//    - No need for engine-specific compression logic
//    - Centralized configuration
//    - Reduced maintenance burden