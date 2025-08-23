//! RAPTOR Engine Constants
//! 
//! This module defines all magic numbers and configuration constants used across
//! the RAPTOR storage engine to ensure consistency and maintainability.

/// RAPTOR file magic constant for backward compatibility
pub const RAPTOR_MAGIC: &[u8] = b"RPTR";

/// File format and version constants
pub mod file_format {
    /// RAPTOR file magic constant (4 bytes)
    pub const MAGIC: [u8; 4] = *b"RPTR";
    
    /// Current RAPTOR file format version
    pub const VERSION: u32 = 1;
    
    /// Default footer size for metadata storage (bytes)
    pub const DEFAULT_FOOTER_SIZE: usize = 1024;
    
    /// Default encoding marker for FastLanes tensor encoding
    pub const FASTLANES_ENCODING_MARKER: u8 = 0xA1;
    
    /// Sparse tensor encoding marker
    pub const SPARSE_ENCODING_MARKER: u8 = 0xA2;
    
    /// Quantized tensor encoding marker
    pub const QUANTIZED_ENCODING_MARKER: u8 = 0xA3;
    
    /// Raw tensor encoding marker (no compression)
    pub const RAW_ENCODING_MARKER: u8 = 0xA0;
}

/// Row group and clustering optimization constants
pub mod clustering {
    /// Default row group size optimized for k<10 queries
    /// Balances I/O efficiency vs wasted reads
    pub const DEFAULT_ROWGROUP_SIZE: usize = 1000;
    
    /// Minimum row group size for meaningful clustering benefit
    pub const MIN_ROWGROUP_SIZE: usize = 1024;
    
    /// Minimum dataset size before applying optimization (vectors)
    pub const MIN_OPTIMIZATION_DATASET_SIZE: usize = 10_000;
    
    /// Target percentage of L3 cache to use for row groups
    pub const L3_CACHE_UTILIZATION_PERCENT: f64 = 0.45;
    
    /// Default L3 cache size assumption when detection fails (bytes)
    pub const DEFAULT_L3_CACHE_SIZE: usize = 8 * 1024 * 1024; // 8MB
    
    /// Vector metadata overhead estimate per vector (bytes)
    pub const METADATA_OVERHEAD_PER_VECTOR: usize = 512;
    
    /// Bytes per f32 dimension
    pub const BYTES_PER_F32_DIMENSION: usize = 4;
    
    /// Default number of clusters for k-means when not calculated
    pub const DEFAULT_CLUSTER_COUNT: usize = 32;
    
    /// Maximum allowed clusters to prevent memory explosion
    pub const MAX_CLUSTER_COUNT: usize = 1000;
    
    /// k-means convergence tolerance
    pub const KMEANS_TOLERANCE: f64 = 1e-4;
    
    /// Maximum k-means iterations
    pub const KMEANS_MAX_ITERATIONS: usize = 10;
    
    /// Number of k-means initializations for best result
    pub const KMEANS_INIT_ATTEMPTS: usize = 3;
}

/// Matrix Trinity constants (replaces HNSW)
pub mod matrix {
    /// Default P² matrix dimension (vectors per rowgroup)
    pub const DEFAULT_P_DIMENSION: usize = 1024;
    
    /// Maximum K² matrix dimension (number of centroids)
    pub const MAX_K_DIMENSION: usize = 10000;
    
    /// P×K matrix compression threshold
    pub const PXK_COMPRESSION_THRESHOLD: usize = 100;
    
    /// Matrix overhead bytes per vector
    pub const MATRIX_OVERHEAD_PER_VECTOR: usize = 5;
}

/// Component boosting default weights
pub mod boosting {
    /// α₁: Vector-to-own-centroid weight (intra-cluster cohesion)
    pub const ALPHA_OWN_DEFAULT: f32 = 0.5;
    
    /// α₂: Inter-centroid weight (boundary penalty)  
    pub const ALPHA_INTER_DEFAULT: f32 = 1.0;
    
    /// α₃: Cluster variance weight (compactness measure)
    pub const ALPHA_VARIANCE_DEFAULT: f32 = 0.3;
    
    /// β₁: Minimum inter-centroid distance weight (cluster separation)
    pub const BETA_MIN_DEFAULT: f32 = 1.0;
    
    /// β₂: Maximum inter-centroid distance weight (global structure)
    pub const BETA_MAX_DEFAULT: f32 = 1.0;
    
    /// β: Cross-centroid exponential decay weight
    pub const BETA_CROSS_DEFAULT: f32 = 1.0;
    
    /// Boundary detection threshold in standard deviations
    pub const BOUNDARY_THRESHOLD_DEFAULT: f32 = 1.0;
    
    /// Number of distance components in boosting formula
    pub const BOOSTING_COMPONENTS: usize = 5;
    
    /// Boosting calculations per vector (α₁, α₂, α₃, β₁, β₂ components)
    pub const BOOSTING_CALCS_PER_VECTOR: usize = 42;
}

/// Memory and caching constants
pub mod memory {
    /// Default SIMD lane width for vectorized operations
    pub const DEFAULT_SIMD_LANES: usize = 16;
    
    /// Default cache size for row groups (MB)
    pub const DEFAULT_CACHE_SIZE_MB: usize = 1024;
    
    /// Default prefetch size for cloud I/O (MB)
    pub const DEFAULT_PREFETCH_SIZE_MB: usize = 32;
    
    /// Default buffer pool size (MB)
    pub const DEFAULT_BUFFER_POOL_SIZE_MB: usize = 512;
    
    /// Maximum parallel read operations
    pub const DEFAULT_MAX_PARALLEL_READS: usize = 8;
    
    /// Cache eviction threshold (number of entries before cleanup)
    pub const CACHE_EVICTION_THRESHOLD: usize = 2;
}

/// Compression and encoding constants
pub mod compression {
    /// Default ZSTD compression level for balanced performance
    pub const DEFAULT_ZSTD_LEVEL: i32 = 3;
    
    /// Default FastLanes bit-packing bits
    pub const DEFAULT_FASTLANES_BITS: u8 = 16;
    
    /// Default bloom filter false positive rate
    pub const DEFAULT_BLOOM_FPP: f64 = 0.01;
}

/// File size and I/O optimization constants
pub mod io {
    /// Compaction trigger when this many files exist
    pub const COMPACTION_THRESHOLD_FILES: usize = 2;
    
    /// Minimum file size before compaction (MB)
    pub const COMPACTION_MIN_SIZE_MB: usize = 1;
    
    /// Target single file size (unlimited for RAPTOR)
    pub const TARGET_FILE_SIZE: usize = usize::MAX;
    
    /// LSM-style level configuration (RAPTOR uses L0 only)
    pub const MAX_LSM_LEVEL: usize = 0;
}

/// Complexity calculation constants for performance analysis
pub mod complexity {
    /// Matrix Trinity complexity: k² + p×(k+p)
    
    /// RAPTOR complexity components: k² + p×(k+p) where:
    /// - k = number of clusters (typically √n)
    /// - p = rowgroup size (typically √n/5)
    /// - n = total number of vectors
    /// 
    /// Breakdown:
    /// - k² = cluster centroid comparisons during search
    /// - p×k = vectors per rowgroup × clusters to check
    /// - p×p = intra-rowgroup edge computations
    /// 
    /// Example for 1M vectors:
    /// - k = 1000 clusters, p = 200 per rowgroup
    /// - k² = 1,000,000 centroid comparisons
    /// - p×(k+p) = 200×1200 = 240,000 edge computations
    /// - Total: 1,240,000 operations vs 200M for HNSW
    pub const K_SQUARED_FACTOR: f32 = 1.0;
    pub const P_K_FACTOR: f32 = 1.0;
    pub const P_SQUARED_FACTOR: f32 = 1.0;
    
    /// HNSW complexity factors (for comparison)
    pub const HNSW_M_FACTOR: f32 = 16.0;  // M parameter (edges per node)
    pub const HNSW_EF_FACTOR: f32 = 200.0; // ef construction parameter
}

/// Common vector dimensions for optimization defaults
pub mod dimensions {
    /// OpenAI text-embedding-ada-002 dimension
    pub const OPENAI_ADA_002: usize = 1536;
    
    /// OpenAI text-embedding-3-small dimension  
    pub const OPENAI_3_SMALL: usize = 1536;
    
    /// OpenAI text-embedding-3-large dimension
    pub const OPENAI_3_LARGE: usize = 3072;
    
    /// Sentence transformers all-MiniLM-L6-v2 dimension
    pub const SENTENCE_TRANSFORMERS_MINI: usize = 384;
    
    /// BERT base model dimension
    pub const BERT_BASE: usize = 768;
    
    /// Default dimension when not specified
    pub const DEFAULT: usize = SENTENCE_TRANSFORMERS_MINI;
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_constants_consistency() {
        // Ensure minimum rowgroup size is reasonable for clustering
        assert!(clustering::MIN_ROWGROUP_SIZE >= 512);
        assert!(clustering::DEFAULT_ROWGROUP_SIZE >= clustering::MIN_ROWGROUP_SIZE);
        
        // Ensure Matrix Trinity parameters are reasonable
        assert!(matrix::DEFAULT_P_DIMENSION >= 100 && matrix::DEFAULT_P_DIMENSION <= 10000);
        assert!(matrix::MAX_K_DIMENSION >= matrix::DEFAULT_P_DIMENSION);
        
        // Ensure cache utilization is reasonable
        assert!(clustering::L3_CACHE_UTILIZATION_PERCENT > 0.0);
        assert!(clustering::L3_CACHE_UTILIZATION_PERCENT < 1.0);
        
        // Ensure boosting weights are positive
        assert!(boosting::ALPHA_OWN_DEFAULT > 0.0);
        assert!(boosting::BETA_CROSS_DEFAULT > 0.0);
    }
    
    #[test]
    fn test_file_format_constants() {
        assert_eq!(file_format::MAGIC, *b"RPTR");
        assert_eq!(file_format::VERSION, 1);
        
        // Ensure encoding markers are in the correct range (0xA0-0xAF)
        assert_eq!(file_format::RAW_ENCODING_MARKER, 0xA0);
        assert_eq!(file_format::FASTLANES_ENCODING_MARKER, 0xA1);
        assert_eq!(file_format::SPARSE_ENCODING_MARKER, 0xA2);
        assert_eq!(file_format::QUANTIZED_ENCODING_MARKER, 0xA3);
    }
}