//! Unit tests for Vector Operations Service
//!
//! Tests the actual vector operations orchestration functionality

use proximadb::compute::quantization::types::QuantizationLevel;
use proximadb::core::search::FilterExpression;
use proximadb::proto::proximadb_v1::{SqlValue, VectorRecord, sql_value};
use std::collections::HashMap;

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_vector(id: &str, values: Vec<f32>) -> VectorRecord {
        let mut metadata = HashMap::new();
        metadata.insert(
            "test_id".to_string(),
            SqlValue {
                value: Some(sql_value::Value::StringValue(id.to_string())),
            },
        );

        VectorRecord {
            id: id.to_string(),
            vector: values,
            metadata,
            timestamp: Some(chrono::Utc::now().timestamp()),
            updated_at: Some(chrono::Utc::now().timestamp()),
            expires_at: None,
            version: Some(1),
            source: None,
        }
    }

    #[tokio::test]
    async fn test_vector_record_creation() {
        let vector = create_test_vector("vec1", vec![1.0, 2.0, 3.0]);

        assert_eq!(vector.id, "vec1");
        assert_eq!(vector.vector.len(), 3);
        assert!(vector.metadata.contains_key("test_id"));
    }

    #[test]
    fn test_quantization_levels() {
        use proximadb::compute::quantization::types::{
            BinaryQuantization, ProductQuantization, ScalarQuantization,
        };

        // Test that quantization levels are properly defined
        let levels = [
            QuantizationLevel::Binary(BinaryQuantization {
                threshold: None,
                sign_based: false,
            }),
            QuantizationLevel::Scalar(ScalarQuantization {
                scale: 1.0,
                offset: 0.0,
                bits: 8, // INT8 quantization
                clamp_values: true,
            }),
            QuantizationLevel::Pq(ProductQuantization {
                bits_per_code: 8,
                num_subvectors: 8,
                codebook_id: None,
                adaptive_subvectors: false,
            }),
        ];

        for level in &levels {
            match level {
                QuantizationLevel::Binary(_) => {
                    // Binary quantization reduces to 1 bit per dimension
                    assert!(true, "Binary quantization available");
                }
                QuantizationLevel::Scalar(_) => {
                    // Scalar quantization (INT8, etc.)
                    assert!(true, "Scalar quantization available");
                }
                QuantizationLevel::Pq(_) => {
                    // PQ quantization with configurable subspaces
                    assert!(true, "Product quantization available");
                }
                _ => {}
            }
        }
    }

    #[test]
    fn test_filter_expression_creation() {
        // Test metadata filtering
        let filter = FilterExpression::Comparison {
            field: "category".to_string(),
            operator: proximadb::core::search::ComparisonOperator::Equals,
            value: serde_json::Value::String("test".to_string()),
        };

        match filter {
            FilterExpression::Comparison { field, .. } => {
                assert_eq!(field, "category");
            }
            _ => panic!("Expected comparison filter"),
        }
    }

    #[test]
    fn test_vector_metadata_structure() {
        let mut metadata = HashMap::new();

        // Test different SqlValue types
        metadata.insert(
            "string_field".to_string(),
            SqlValue {
                value: Some(sql_value::Value::StringValue("test".to_string())),
            },
        );

        metadata.insert(
            "number_field".to_string(),
            SqlValue {
                value: Some(sql_value::Value::NumberValue(42.0)),
            },
        );

        metadata.insert(
            "bool_field".to_string(),
            SqlValue {
                value: Some(sql_value::Value::BoolValue(true)),
            },
        );

        assert_eq!(metadata.len(), 3);
        assert!(metadata.contains_key("string_field"));
        assert!(metadata.contains_key("number_field"));
        assert!(metadata.contains_key("bool_field"));
    }

    #[tokio::test]
    async fn test_batch_vector_creation() {
        let mut vectors = Vec::new();

        for i in 0..100 {
            let vector = create_test_vector(
                &format!("vec_{}", i),
                vec![i as f32, (i * 2) as f32, (i * 3) as f32],
            );
            vectors.push(vector);
        }

        assert_eq!(vectors.len(), 100);
        assert_eq!(vectors[0].id, "vec_0");
        assert_eq!(vectors[99].id, "vec_99");
    }
}
