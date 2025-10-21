use proximadb::core::compression::CompressionAlgorithm;
use proximadb::storage::engines::core::formats::proximablocks::{
    BlockCompressionConfig, VectorEncodingLayout,
};

/// Example encoding configurations for different workload patterns
/// Implements the recommendations from docs/ENCODING_PERFORMANCE.adoc

/// WORM (Write Once, Read Many) Optimized Configuration
///
/// **Characteristics:**
/// * Data written once, read frequently
/// * Storage efficiency critical
/// * Can tolerate higher write latency
/// * Examples: Log archives, historical data, training datasets
///
/// **Performance Expectations:**
/// * Storage Reduction: 94% (17x compression)
/// * Write Latency: 40-80ms per batch
/// * Read Performance: 200-400ms for full reconstruction
/// * Cost Savings: ~$850/TB/year on S3 storage
pub fn create_worm_config() -> BlockCompressionConfig {
    BlockCompressionConfig {
        // Force columnar for maximum compression
        vector_layout: VectorEncodingLayout::TransposeFieldEncodedAndCompressedVector,

        // Use Zstd for best compression ratio
        algorithm: CompressionAlgorithm::Zstd,
        compression_level: 6, // Higher level for better compression

        // Enable all compression features
        enable_vector_compression: true,
        enable_metadata_compression: true,
        compression_threshold_bytes: 4096, // Lower threshold
        dictionary_compression: true,
        metadata_algorithm: Some(CompressionAlgorithm::Zstd),
    }
}

/// Real-time Query Workload Configuration
///
/// **Characteristics:**
/// * Sub-10ms query latency requirements
/// * Frequent random access patterns
/// * Write and read latency critical
/// * Examples: Recommendation systems, search engines, chat applications
///
/// **Performance Expectations:**
/// * Write Latency: <1ms per vector
/// * Read Latency: <1ms direct access
/// * Query Response: <10ms p99
/// * Storage Overhead: No compression (1x size)
pub fn create_realtime_config() -> BlockCompressionConfig {
    BlockCompressionConfig {
        // Force row-wise for minimum latency
        vector_layout: VectorEncodingLayout::FullVector,

        // Use LZ4 for fast compression/decompression
        algorithm: CompressionAlgorithm::Lz4,
        compression_level: 1, // Fastest compression

        // Minimize compression overhead
        enable_vector_compression: false, // Skip vector compression
        enable_metadata_compression: true,
        compression_threshold_bytes: 16384, // Higher threshold
        dictionary_compression: false,
        metadata_algorithm: Some(CompressionAlgorithm::Lz4),
    }
}

/// Balanced Mixed Workload Configuration
///
/// **Characteristics:**
/// * Mix of batch and real-time operations
/// * Moderate latency requirements (10-100ms)
/// * Cost-conscious but performance-aware
/// * Examples: Analytics platforms, ML serving, data pipelines
///
/// **Auto-selection Logic:**
/// * Dimensions ≤ 512: TransposeVector (better compression)
/// * Dimensions > 512: Row-wise (better latency)
///
/// **Performance Expectations:**
/// * Write Latency: 1-40ms (dimension-dependent)
/// * Compression Ratio: 1-17x (layout-dependent)
/// * Query Performance: 10-100ms p95
/// * Storage Efficiency: 40-90% reduction for low dimensions
pub fn create_balanced_config() -> BlockCompressionConfig {
    BlockCompressionConfig {
        // Auto-select based on dimension
        vector_layout: VectorEncodingLayout::Auto,

        // Balanced compression algorithm
        algorithm: CompressionAlgorithm::Snappy,
        compression_level: 3, // Moderate compression

        // Selective compression
        enable_vector_compression: true,
        enable_metadata_compression: true,
        compression_threshold_bytes: 8192, // Standard threshold
        dictionary_compression: false,     // Skip for speed
        metadata_algorithm: Some(CompressionAlgorithm::Snappy),
    }
}

/// AWS S3 + CloudFront Optimized Configuration
///
/// Optimized for AWS S3 storage costs with CloudFront distribution.
/// Prioritizes maximum compression to reduce storage and transfer costs.
pub fn create_aws_config() -> BlockCompressionConfig {
    BlockCompressionConfig {
        vector_layout: VectorEncodingLayout::TransposeFieldEncodedAndCompressedVector,
        algorithm: CompressionAlgorithm::Zstd,
        compression_level: 5,
        enable_vector_compression: true,
        enable_metadata_compression: true,
        compression_threshold_bytes: 4096,
        dictionary_compression: true,
        metadata_algorithm: Some(CompressionAlgorithm::Zstd),
    }
}

/// Azure Blob Storage Hot Tier Configuration
///
/// Optimized for Azure Blob Storage hot tier access patterns.
/// Balances compression with access speed for frequently accessed data.
pub fn create_azure_config() -> BlockCompressionConfig {
    BlockCompressionConfig {
        vector_layout: VectorEncodingLayout::Auto,
        algorithm: CompressionAlgorithm::Snappy,
        compression_level: 3,
        enable_vector_compression: true,
        enable_metadata_compression: true,
        compression_threshold_bytes: 8192,
        dictionary_compression: false,
        metadata_algorithm: Some(CompressionAlgorithm::Snappy),
    }
}

/// Google Cloud Storage Nearline Configuration
///
/// Optimized for GCS nearline storage with balanced access patterns.
/// Uses Gzip for good compression with reasonable decompression speed.
pub fn create_gcs_config() -> BlockCompressionConfig {
    BlockCompressionConfig {
        vector_layout: VectorEncodingLayout::TransposeFieldEncodedAndCompressedVector,
        algorithm: CompressionAlgorithm::Gzip,
        compression_level: 6,
        enable_vector_compression: true,
        enable_metadata_compression: true,
        compression_threshold_bytes: 4096,
        dictionary_compression: true,
        metadata_algorithm: Some(CompressionAlgorithm::Gzip),
    }
}

/// Configuration Validation Function
///
/// Validates configuration before deployment to ensure optimal performance
/// and warn about potential issues.
pub fn validate_encoding_config(
    config: &BlockCompressionConfig,
    expected_dimensions: usize,
    latency_budget_ms: f64,
) -> Result<(), String> {
    match config.vector_layout {
        VectorEncodingLayout::TransposeFieldEncodedAndCompressedVector
            if expected_dimensions > 1536 =>
        {
            if latency_budget_ms < 100.0 {
                return Err(
                    "TransposeField encoding may exceed latency budget for high dimensions".into(),
                );
            }
        }
        VectorEncodingLayout::FullVector if expected_dimensions < 256 => {
            println!("Warning: Missing compression opportunity for low dimensions");
        }
        _ => {}
    }
    Ok(())
}

/// Example usage and configuration selection
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workload_configurations() {
        // Test WORM configuration
        let worm_config = create_worm_config();
        assert_eq!(
            worm_config.vector_layout,
            VectorEncodingLayout::TransposeFieldEncodedAndCompressedVector
        );
        assert_eq!(worm_config.algorithm, CompressionAlgorithm::Zstd);
        assert!(worm_config.enable_vector_compression);
        assert!(worm_config.dictionary_compression);

        // Test real-time configuration
        let realtime_config = create_realtime_config();
        assert_eq!(
            realtime_config.vector_layout,
            VectorEncodingLayout::FullVector
        );
        assert_eq!(realtime_config.algorithm, CompressionAlgorithm::Lz4);
        assert!(!realtime_config.enable_vector_compression);
        assert!(!realtime_config.dictionary_compression);

        // Test balanced configuration
        let balanced_config = create_balanced_config();
        assert_eq!(balanced_config.vector_layout, VectorEncodingLayout::Auto);
        assert_eq!(balanced_config.algorithm, CompressionAlgorithm::Snappy);
        assert!(balanced_config.enable_vector_compression);
        assert!(!balanced_config.dictionary_compression);
    }

    #[test]
    fn test_configuration_validation() {
        let columnar_config = create_worm_config();

        // Should warn about high dimensions with low latency budget
        let result = validate_encoding_config(&columnar_config, 2048, 50.0);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("latency budget"));

        // Should pass with appropriate settings
        let result = validate_encoding_config(&columnar_config, 768, 200.0);
        assert!(result.is_ok());

        // Test row-wise with low dimensions (should warn but not fail)
        let rowwise_config = create_realtime_config();
        let result = validate_encoding_config(&rowwise_config, 128, 10.0);
        assert!(result.is_ok()); // Warning printed but doesn't fail
    }

    #[test]
    fn test_cloud_provider_configs() {
        let aws_config = create_aws_config();
        assert_eq!(
            aws_config.vector_layout,
            VectorEncodingLayout::TransposeFieldEncodedAndCompressedVector
        );
        assert_eq!(aws_config.algorithm, CompressionAlgorithm::Zstd);

        let azure_config = create_azure_config();
        assert_eq!(azure_config.vector_layout, VectorEncodingLayout::Auto);
        assert_eq!(azure_config.algorithm, CompressionAlgorithm::Snappy);

        let gcs_config = create_gcs_config();
        assert_eq!(
            gcs_config.vector_layout,
            VectorEncodingLayout::TransposeFieldEncodedAndCompressedVector
        );
        assert_eq!(gcs_config.algorithm, CompressionAlgorithm::Gzip);
    }
}

/// Example usage in application code
pub fn example_configuration_selection() {
    println!("ProximaDB Encoding Configuration Examples");
    println!("=========================================");

    // Example 1: WORM workload for data archival
    let worm_config = create_worm_config();
    println!("WORM Configuration: {:?}", worm_config);

    // Example 2: Real-time recommendation system
    let realtime_config = create_realtime_config();
    println!("Real-time Configuration: {:?}", realtime_config);

    // Example 3: Analytics platform with mixed workloads
    let balanced_config = create_balanced_config();
    println!("Balanced Configuration: {:?}", balanced_config);

    // Example 4: Validate configuration for specific use case
    let dimension = 768;
    let latency_budget = 100.0; // 100ms budget

    match validate_encoding_config(&balanced_config, dimension, latency_budget) {
        Ok(_) => println!(
            "✓ Configuration validated for {}D vectors with {}ms budget",
            dimension, latency_budget
        ),
        Err(e) => println!("✗ Configuration validation failed: {}", e),
    }
}

/// Main function to run the encoding configuration examples
fn main() {
    example_configuration_selection();
}
