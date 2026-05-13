//! Integration tests for codebook metadata storage
//!
//! Verifies that codebook metadata works correctly across different storage engines

#[cfg(test)]
mod tests {
    use crate::storage::engines::core::formats::codebook_metadata::{
        BinaryCodebook, CodebookSerializer, Int8Codebook, PqCodebook, PqTrainingConfig,
        QuantizationCodebookMetadata,
    };
    use crate::storage::engines::core::formats::columnar::constants::*;
    use crate::storage::engines::core::formats::common_quantization::{
        ProductQuantizationBits, QuantizedVectorData, ScalarQuantizationBits,
        StorageQuantizationFormat,
    };
    use std::collections::HashMap;

    #[test]
    fn test_codebook_metadata_with_quantized_vectors() {
        // Create quantized vector data
        let mut quantized_data = QuantizedVectorData::empty();
        quantized_data.vector_count = 100;
        quantized_data.dimension = 128;
        quantized_data.q_binary = Some(vec![vec![0u8; 16]; 100]); // 128 bits = 16 bytes
        quantized_data.q_int8 = Some(vec![vec![0i8; 128]; 100]);

        // Create corresponding codebook metadata
        let mut metadata = QuantizationCodebookMetadata {
            collection_id: "test_collection".to_string(),
            binary_codebook: Some(BinaryCodebook {
                threshold: 0.5,
                mean: Some(vec![0.0; 128]),
                dimension: 128,
            }),
            int8_codebook: Some(Int8Codebook {
                scale: 0.01,
                zero_point: 0,
                min_value: -1.0,
                max_value: 1.0,
                dimension: 128,
            }),
            pq_codebooks: HashMap::new(),
            created_at: 1234567890,
            training_samples: 100,
            schema_version: 1,
        };

        // Add PQ8 codebook
        metadata.pq_codebooks.insert(
            "pq8_16".to_string(),
            PqCodebook {
                num_subvectors: 16,
                bits_per_code: 8,
                centroids: vec![vec![vec![0.0; 8]; 256]; 16], // 16 subvectors, 256 centroids each
                dimension: 128,
                subvector_dim: 8,
                num_centroids: 256,
                training_config: PqTrainingConfig {
                    num_iterations: 100,
                    seed: Some(42),
                    distance_metric: "euclidean".to_string(),
                },
            },
        );

        // Verify serialization/deserialization
        let serializer = CodebookSerializer::new();

        // Test footer serialization (for ProximaBlock engines)
        let footer_bytes = serializer.serialize_for_footer(&metadata).unwrap();
        let deserialized_footer = serializer.deserialize_from_footer(&footer_bytes).unwrap();

        assert_eq!(deserialized_footer.collection_id, metadata.collection_id);
        assert_eq!(deserialized_footer.binary_codebook.is_some(), true);
        assert_eq!(deserialized_footer.int8_codebook.is_some(), true);
        assert_eq!(deserialized_footer.pq_codebooks.len(), 1);

        // Test sidecar serialization (for Parquet engines)
        let sidecar_json = serializer.serialize_for_sidecar(&metadata).unwrap();
        let deserialized_sidecar = serializer.deserialize_from_sidecar(&sidecar_json).unwrap();

        assert_eq!(deserialized_sidecar.collection_id, metadata.collection_id);
        assert_eq!(
            deserialized_sidecar.training_samples,
            metadata.training_samples
        );
    }

    #[test]
    fn test_column_name_constants() {
        // Verify all quantization column names use constants
        assert_eq!(FIELD_Q_BINARY, "q_binary");
        assert_eq!(FIELD_Q_INT8, "q_int8");
        assert_eq!(FIELD_Q_PQ4, "q_pq4");
        assert_eq!(FIELD_Q_PQ8, "q_pq8");
        assert_eq!(FIELD_Q_PQ16, "q_pq16");
        assert_eq!(FIELD_Q_PQ32, "q_pq32");

        // Verify parameter columns
        assert_eq!(FIELD_QP_BINARY_THRESHOLD, "qp_binary_threshold");
        assert_eq!(FIELD_QP_INT8_MIN, "qp_int8_min");
        assert_eq!(FIELD_QP_INT8_MAX, "qp_int8_max");
        assert_eq!(FIELD_QP_INT8_SCALE, "qp_int8_scale");
        assert_eq!(FIELD_QP_PQ_SUBQUANTIZERS, "qp_pq_subquantizers");
        assert_eq!(FIELD_QP_PQ_CENTROIDS, "qp_pq_centroids");
    }

    #[test]
    fn test_quantization_level_compatibility() {
        // Verify storage quantization formats work with the schema constants.
        let levels = vec![
            StorageQuantizationFormat::BinaryFormat,
            StorageQuantizationFormat::ScalarFormat(ScalarQuantizationBits::Int8),
            StorageQuantizationFormat::ProductFormat(ProductQuantizationBits::PQ4),
            StorageQuantizationFormat::ProductFormat(ProductQuantizationBits::PQ8),
            StorageQuantizationFormat::ProductFormat(ProductQuantizationBits::PQ16),
            StorageQuantizationFormat::ProductFormat(ProductQuantizationBits::PQ32),
        ];

        for level in levels {
            let field_name = match level {
                StorageQuantizationFormat::BinaryFormat => FIELD_Q_BINARY,
                StorageQuantizationFormat::ScalarFormat(ScalarQuantizationBits::Int8) => {
                    FIELD_Q_INT8
                }
                StorageQuantizationFormat::ProductFormat(ProductQuantizationBits::PQ4) => {
                    FIELD_Q_PQ4
                }
                StorageQuantizationFormat::ProductFormat(ProductQuantizationBits::PQ8) => {
                    FIELD_Q_PQ8
                }
                StorageQuantizationFormat::ProductFormat(ProductQuantizationBits::PQ16) => {
                    FIELD_Q_PQ16
                }
                StorageQuantizationFormat::ProductFormat(ProductQuantizationBits::PQ32) => {
                    FIELD_Q_PQ32
                }
                StorageQuantizationFormat::ScalarFormat(
                    ScalarQuantizationBits::Int4 | ScalarQuantizationBits::UInt8,
                ) => FIELD_Q_INT8,
            };

            assert!(field_name.starts_with("q_"));
        }
    }
}
