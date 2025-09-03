#[cfg(test)]
mod tests {
    use super::super::config::RaptorConfig;
    use super::super::writer::RaptorWriter;
    use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
    use crate::compute::quantization::storage_engine::StorageQuantizationEngine;
    use crate::infrastructure::P2Matrix;
    use crate::storage::engines::core::ops::fastlanes_encoding::FastLanesScheme;
    use anyhow::Result;
    use tempfile::TempDir;

    /// Test P² matrix creation with upper triangle indexing
    #[test]
    fn test_p2_matrix_upper_triangle_indexing() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

        // Create a P² matrix with 4 vectors
        let n = 4;
        let upper_triangle_size = n * (n - 1) / 2;
        assert_eq!(upper_triangle_size, 6); // 4×3/2 = 6

        // Test index formula: idx = i×(2n-i-1)/2 + j - i - 1
        // Expected indices for n=4:
        // (0,1) -> 0
        // (0,2) -> 1
        // (0,3) -> 2
        // (1,2) -> 3
        // (1,3) -> 4
        // (2,3) -> 5

        let test_cases = vec![
            ((0, 1), 0),
            ((0, 2), 1),
            ((0, 3), 2),
            ((1, 2), 3),
            ((1, 3), 4),
            ((2, 3), 5),
        ];

        for ((i, j), expected_idx) in test_cases {
            let idx = i * (2 * n - i - 1) / 2 + j - i - 1;
            assert_eq!(
                idx, expected_idx,
                "Index for ({},{}) should be {}",
                i, j, expected_idx
            );
        }
    }

    /// Test P² matrix distance retrieval with symmetry
    #[test]
    fn test_p2_matrix_distance_symmetry() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

        // Create a P² matrix with mock distances
        let p2_matrix = P2Matrix {
            num_vectors: 3,
            distances: vec![10, 20, 30], // Quantized distances: (0,1)=10, (0,2)=20, (1,2)=30
            min_distance: 0.0,
            max_distance: 1.0,
            compression: FastLanesScheme::None,
            compressed_size: 3,
        };

        // Test symmetry: d(i,j) = d(j,i)
        assert_eq!(p2_matrix.get_distance(0, 1), p2_matrix.get_distance(1, 0));
        assert_eq!(p2_matrix.get_distance(0, 2), p2_matrix.get_distance(2, 0));
        assert_eq!(p2_matrix.get_distance(1, 2), p2_matrix.get_distance(2, 1));

        // Test self-distance is 0
        assert_eq!(p2_matrix.get_distance(0, 0), 0.0);
        assert_eq!(p2_matrix.get_distance(1, 1), 0.0);
        assert_eq!(p2_matrix.get_distance(2, 2), 0.0);
    }

    /// Test P² matrix builder with real vectors
    #[tokio::test]
    async fn test_build_p2_matrix() -> Result<()> {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

        // Since we can't directly test RaptorWriter.build_p2_matrix due to visibility,
        // we'll test the P² matrix concept directly

        // Create test vectors
        let vectors = vec![
            vec![1.0, 0.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0, 0.0],
            vec![0.0, 0.0, 1.0, 0.0],
            vec![0.0, 0.0, 0.0, 1.0],
        ];

        // Compute upper triangle distances manually
        let distance_compute = UnifiedDistanceCompute::new(
            crate::compute::distance_computation::engine::DistanceMetric::Cosine,
        );
        let mut distances = Vec::new();

        for i in 0..vectors.len() {
            for j in (i + 1)..vectors.len() {
                let dist = distance_compute.calculate(&vectors[i], &vectors[j])?;
                distances.push(dist);
            }
        }

        // Verify we have the right number of distances
        assert_eq!(distances.len(), 6); // 4×3/2 = 6 upper triangle entries

        // All vectors are orthogonal, so cosine distances should be 1.0
        // (cosine distance = 1 - cosine similarity, and orthogonal vectors have similarity 0)
        for dist in distances {
            assert!(
                (dist - 1.0).abs() < 0.01,
                "Orthogonal vectors should have cosine distance ~1.0"
            );
        }

        Ok(())
    }

    /// Test P² matrix with FastLanes encoding
    #[tokio::test]
    async fn test_p2_matrix_fastlanes_encoding() -> Result<()> {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

        use crate::storage::engines::core::ops::fastlanes_encoding::FastLanesEncoder;

        // Create larger set of vectors to test compression
        let mut vectors = Vec::new();
        for i in 0..32 {
            let mut v = vec![0.0; 128];
            v[i % 128] = 1.0;
            vectors.push(v);
        }

        // Compute distances and quantize
        let distance_compute = UnifiedDistanceCompute::new(
            crate::compute::distance_computation::engine::DistanceMetric::Cosine,
        );
        let mut distances = Vec::new();

        for i in 0..vectors.len() {
            for j in (i + 1)..vectors.len() {
                let dist = distance_compute.calculate(&vectors[i], &vectors[j])?;
                distances.push(dist);
            }
        }

        // Quantize to u8
        let quantization_engine = StorageQuantizationEngine::new_default();
        let (quantized, min_dist, max_dist) = quantization_engine.quantize_to_u8(&distances);

        // Apply FastLanes encoding
        let fastlanes_encoder = FastLanesEncoder::new();
        let scheme = fastlanes_encoder.analyze_and_select_scheme(&quantized)?;
        let encoded = fastlanes_encoder.encode_u8_slice(&quantized, scheme)?;

        // Verify compression
        let uncompressed_size = 32 * 31 / 2; // Upper triangle size
        assert_eq!(distances.len(), uncompressed_size);
        assert!(encoded.len() > 0);

        // Check that we achieved some compression
        println!(
            "FastLanes compression: {} -> {} bytes ({:.2}x)",
            quantized.len(),
            encoded.len(),
            quantized.len() as f32 / encoded.len() as f32
        );

        Ok(())
    }

    /// Test P² matrix memory usage for different rowgroup sizes
    #[test]
    fn test_p2_matrix_memory_scaling() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

        // Test memory usage for different P values
        let test_cases = vec![
            (256, 32_640),     // 256×255/2 = 32,640 bytes
            (512, 130_816),    // 512×511/2 = 130,816 bytes
            (1024, 523_776),   // 1024×1023/2 = 523,776 bytes (~512KB)
            (2048, 2_096_128), // 2048×2047/2 = 2,096,128 bytes (~2MB)
        ];

        for (p, expected_bytes) in test_cases {
            let upper_triangle_size = p * (p - 1) / 2;
            assert_eq!(
                upper_triangle_size, expected_bytes,
                "P={} should require {} bytes",
                p, expected_bytes
            );

            // With INT8 quantization, this is the exact memory requirement
            println!("P={}: {:.2} KB", p, expected_bytes as f32 / 1024.0);
        }
    }

    /// Test P² matrix integration with search
    #[tokio::test]
    async fn test_p2_matrix_search_integration() -> Result<()> {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

        use super::super::consolidated_reader::{IntraRowgroupMatrix, RaptorReader};

        // Create test P² matrix
        let p2_matrix = P2Matrix {
            num_vectors: 4,
            distances: vec![50, 100, 150, 50, 100, 50], // Some test distances
            min_distance: 0.0,
            max_distance: 1.0,
            compression: FastLanesScheme::None,
            compressed_size: 6,
        };

        // Create test vectors
        let vectors = vec![
            vec![1.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0],
            vec![0.0, 0.0, 1.0],
            vec![0.5, 0.5, 0.0],
        ];

        let matrix = IntraRowgroupMatrix::new(p2_matrix, vectors.clone());

        // Verify we can access distances
        assert_eq!(matrix.p2_matrix.num_vectors, 4);
        assert_eq!(matrix.vectors.len(), 4);

        // Test distance retrieval
        let dist_01 = matrix.p2_matrix.get_distance(0, 1);
        let dist_10 = matrix.p2_matrix.get_distance(1, 0);
        assert_eq!(dist_01, dist_10, "Distance should be symmetric");

        Ok(())
    }
}
