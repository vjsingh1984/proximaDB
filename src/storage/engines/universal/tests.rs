//! Tests for Universal Distance Adapter
//!
//! This module contains comprehensive tests for the universal adapter system.

#[cfg(test)]
mod tests {
    use crate::storage::engines::universal::*;
    use crate::storage::engines::universal::adapter::*;
    use crate::compute::distance_computation::DistanceMetric;
    use std::collections::HashMap;
    use crate::utils::uuid::Uuid;

    #[tokio::test]
    async fn test_universal_adapter_creation() {
        let adapter = UniversalDistanceAdapter::new().await;
        assert!(
            adapter.is_ok(),
            "Failed to create universal adapter: {:?}",
            adapter.err()
        );
    }

    #[tokio::test]
    async fn test_progressive_refinement_pipeline() {
        let adapter = UniversalDistanceAdapter::new().await.unwrap();

        let query_vector = vec![1.0, 2.0, 3.0, 4.0];
        let candidates = vec![CandidateVector {
            id: Uuid::new_v4(),
            data: vec![1, 2, 3, 4, 5, 6, 7, 8],
            original_vector: Some(vec![1.1, 2.1, 3.1, 4.1]),
            metadata: None,
            quality_score: None,
        }];

        let request = DistanceComputationRequest {
            query_vector,
            candidates,
            distance_metric: DistanceMetric::Euclidean,
            storage_format: StorageFormat::FP32,
            refinement_config: None,
            max_results: 10,
            enable_acceleration: true,
            // quality_threshold removed -  None,
            collection_id: Uuid::new_v4(),
            engine_type: EngineType::PRISM,
        };

        let result = adapter.compute_progressive_distance(request).await;
        assert!(
            result.is_ok(),
            "Progressive distance computation failed: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    async fn test_storage_engine_adapters() {
        use crate::storage::engines::universal::config::StorageEngineConfig;
        use crate::storage::engines::universal::storage_integration::*;

        // Test PRISM adapter
        let prism_config = StorageEngineConfig::prism_default();
        let prism_adapter = PRISMAdapter::new(&prism_config).await;
        assert!(prism_adapter.is_ok());

        // Test NOVA adapter
        let nova_config = StorageEngineConfig::nova_default();
        let nova_adapter = NOVAAdapter::new(&nova_config).await;
        assert!(nova_adapter.is_ok());

        // Test format optimization
        let adapter = prism_adapter.unwrap();
        let optimal_format = adapter.optimal_format(128, 100_000, 0.9).await.unwrap();
        assert!(matches!(
            optimal_format,
            StorageFormat::QuantizedINT8 { .. }
        ));
    }

    #[tokio::test]
    async fn test_format_conversion() {
        use crate::storage::engines::universal::conversion::*;

        let converter = FormatConverter::new().await.unwrap();

        // Test FP32 to INT8 conversion
        let fp32_data = vec![1.0f32, 2.0, 3.0, 4.0];
        let fp32_bytes = fp32_data
            .iter()
            .flat_map(|&f| f.to_le_bytes().to_vec())
            .collect::<Vec<u8>>();

        let int8_result = converter.to_int8(&fp32_bytes).await;
        assert!(int8_result.is_ok());

        let int8_data = int8_result.unwrap();
        assert_eq!(int8_data.len(), 4);
    }

    #[tokio::test]
    async fn test_quantized_calculator() {
        use crate::storage::engines::universal::config::UniversalAdapterConfig;
        use crate::storage::engines::universal::quantized_calculator::*;
        use crate::core::hardware_capabilities::HardwareCapabilities;

        let config = UniversalAdapterConfig::default();
        let capabilities = crate::core::hardware_capabilities::get_hardware_capabilities();

        let calculator = UniversalQuantizedCalculator::new(&config, &capabilities).await;
        assert!(
            calculator.is_ok(),
            "Failed to create quantized calculator: {:?}",
            calculator.err()
        );
    }

    #[tokio::test]
    async fn test_hardware_acceleration_manager() {
        use crate::storage::engines::universal::config::HardwareAccelerationConfig;
        use crate::storage::engines::universal::hardware_manager::*;
        use crate::core::hardware_capabilities::HardwareCapabilities;

        let config = HardwareAccelerationConfig::default();
        let capabilities = crate::core::hardware_capabilities::get_hardware_capabilities();

        let manager = HardwareAccelerationManager::new(&config, &capabilities).await;
        assert!(
            manager.is_ok(),
            "Failed to create hardware acceleration manager: {:?}",
            manager.err()
        );

        let manager = manager.unwrap();
        let strategy = manager.get_optimization_strategy();
        assert!(matches!(
            strategy,
            OptimizationStrategy::SIMD | OptimizationStrategy::Scalar
        ));
    }

    #[test]
    fn test_storage_format_properties() {
        use crate::storage::engines::universal::conversion::StorageFormat;

        let fp32_format = StorageFormat::FP32;
        assert_eq!(fp32_format.data_size_per_vector(128), 512);
        assert!(fp32_format.supports_hardware_acceleration());

        let int8_format = StorageFormat::QuantizedINT8 {
            scale: 1.0,
            zero_point: 0,
        };
        assert_eq!(int8_format.data_size_per_vector(128), 128);
        assert!(int8_format.supports_hardware_acceleration());

        let pq_format = StorageFormat::QuantizedPQ {
            segments: 8,
            bits: 8,
        };
        assert_eq!(pq_format.data_size_per_vector(128), 8);
        assert!(!pq_format.supports_hardware_acceleration());
    }

    #[test]
    fn test_engine_type_serialization() {
        use crate::storage::engines::universal::storage_integration::EngineType;

        let engine_types = vec![
            EngineType::PRISM,
            EngineType::NOVA,
            EngineType::SWIFT,
            EngineType::VIPER,
            EngineType::SST,
        ];

        for engine_type in engine_types {
            let serialized = serde_json::to_string(&engine_type).unwrap();
            let deserialized: EngineType = serde_json::from_str(&serialized).unwrap();
            assert_eq!(engine_type, deserialized);
        }
    }

    // Helper function to create test vector records
    fn create_test_vectors(count: usize, dimension: usize) -> Vec<crate::core::VectorRecord> {
        let mut vectors = Vec::new();
        for i in 0..count {
            vectors.push(crate::core::VectorRecord {
                id: Uuid::new_v4(),
                vector: (0..dimension).map(|j| (i + j) as f32 * 0.1).collect(),
                metadata: HashMap::new(),
                version: 1,
                timestamp: chrono::Utc::now(),
                updated_at: Some(chrono::Utc::now()),
            });
        }
        vectors
    }

    #[tokio::test]
    async fn test_engine_adapter_vector_conversion() {
        use crate::storage::engines::universal::config::StorageEngineConfig;
        use crate::storage::engines::universal::storage_integration::*;

        let config = StorageEngineConfig::prism_default();
        let adapter = PRISMAdapter::new(&config).await.unwrap();

        let test_vectors = create_test_vectors(10, 64);

        // Test FP32 conversion
        let fp32_result = adapter
            .convert_vectors(&test_vectors, &StorageFormat::FP32)
            .await;
        assert!(fp32_result.is_ok());
        let fp32_data = fp32_result.unwrap();
        assert_eq!(fp32_data.len(), 10 * 64 * 4); // 10 vectors * 64 dims * 4 bytes

        // Test INT8 conversion
        let int8_format = StorageFormat::QuantizedINT8 {
            scale: 1.0,
            zero_point: 0,
        };
        let int8_result = adapter.convert_vectors(&test_vectors, &int8_format).await;
        assert!(int8_result.is_ok());
        let int8_data = int8_result.unwrap();
        assert_eq!(int8_data.len(), 10 * 64); // 10 vectors * 64 dims * 1 byte
    }

    #[tokio::test]
    async fn test_memory_usage_estimation() {
        use crate::storage::engines::universal::config::StorageEngineConfig;
        use crate::storage::engines::universal::storage_integration::*;

        let config = StorageEngineConfig::nova_default();
        let adapter = NOVAAdapter::new(&config).await.unwrap();

        let memory_usage = adapter
            .estimate_memory_usage(
                1000, // vector count
                256,  // vector dimension
                &StorageFormat::FP32,
            )
            .await
            .unwrap();

        // Expected: 1000 * 256 * 4 = 1,024,000 bytes + 10% overhead
        let expected_min = 1_024_000;
        let expected_max = 1_200_000;

        assert!(
            memory_usage >= expected_min && memory_usage <= expected_max,
            "Memory usage {} not in expected range [{}, {}]",
            memory_usage,
            expected_min,
            expected_max
        );
    }
}

// Re-export commonly used types for tests
pub use super::{
    adapter::*, config::*, conversion::*, hardware_manager::*, progressive_refinement::*,
    quantized_calculator::*, storage_integration::*,
};

// Test utilities
#[cfg(test)]
pub mod test_utils {
    use super::*;
    use std::collections::HashMap;

    pub fn create_test_candidate_vector(id: uuid::Uuid, dimension: usize) -> CandidateVector {
        let data: Vec<u8> = (0..dimension * 4).map(|i| (i % 256) as u8).collect();

        CandidateVector {
            id,
            data,
            original_vector: Some((0..dimension).map(|i| i as f32 * 0.1).collect()),
            metadata: Some(HashMap::new()),
            quality_score: Some(0.8),
        }
    }

    pub fn create_test_distance_request(
        query_dimension: usize,
        candidate_count: usize,
    ) -> DistanceComputationRequest {
        let query_vector = (0..query_dimension).map(|i| i as f32 * 0.1).collect();
        let candidates = (0..candidate_count)
            .map(|_| create_test_candidate_vector(crate::utils::uuid::Uuid::new_v4(), query_dimension))
            .collect();

        DistanceComputationRequest {
            query_vector,
            candidates,
            distance_metric: DistanceMetric::Euclidean,
            storage_format: StorageFormat::FP32,
            refinement_config: None,
            max_results: 10,
            enable_acceleration: true,
            // quality_threshold removed -  Some(0.8),
            collection_id: crate::utils::uuid::Uuid::new_v4(),
            engine_type: EngineType::PRISM,
        }
    }
}
