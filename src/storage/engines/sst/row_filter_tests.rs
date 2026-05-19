#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::search::{ComparisonOperator, FilterExpression};
    use crate::proto::proximadb_v1::VectorRecord;
    use crate::storage::engines::sst::row_filter::{
        SSTBatchFilterEvaluator, SSTRowFilterEvaluator,
    };
    use proximadb_records::ProximaRecord;

    #[tokio::test]
    async fn test_sst_row_filter_performance() {
        let mut evaluator = SSTRowFilterEvaluator::new();

        // Create test records with metadata
        let records = create_test_vector_records(1000);

        // Simple equality filter
        let filter = FilterExpression::Comparison {
            field: "category".to_string(),
            operator: ComparisonOperator::Equals,
            value: serde_json::json!("electronics"),
        };

        let indices = evaluator
            .filter_vector_records_fast(&records, &filter)
            .unwrap();

        assert!(!indices.is_empty(), "Should find some matching records");
        println!(
            "SST Row Filter found {} matches out of {} records",
            indices.len(),
            records.len()
        );
    }

    #[tokio::test]
    async fn test_parallel_and_or_evaluation() {
        let mut evaluator = SSTBatchFilterEvaluator::new();
        let records = create_test_vector_records(1000);

        // Complex AND/OR filter
        let filter = FilterExpression::Or(vec![
            FilterExpression::And(vec![
                FilterExpression::Comparison {
                    field: "category".to_string(),
                    operator: ComparisonOperator::Equals,
                    value: serde_json::json!("electronics"),
                },
                FilterExpression::Comparison {
                    field: "price".to_string(),
                    operator: ComparisonOperator::GreaterThan,
                    value: serde_json::json!(100),
                },
            ]),
            FilterExpression::Comparison {
                field: "brand".to_string(),
                operator: ComparisonOperator::Equals,
                value: serde_json::json!("Apple"),
            },
        ]);

        let indices = evaluator
            .evaluate_parallel_filters(&records, &filter)
            .await
            .unwrap();

        assert!(!indices.is_empty(), "Should find some matching records");
        println!("Parallel filter found {} matches", indices.len());
    }

    fn create_test_vector_records(count: usize) -> Vec<ProximaRecord> {
        let mut records = Vec::new();

        for i in 0..count {
            let mut metadata = Vec::new();

            // Add test metadata
            metadata.push(crate::proto::proximadb_v1::MetadataItem {
                key: "category".to_string(),
                value: Some(
                    crate::proto::proximadb_v1::metadata_item::Value::StringValue(if i % 3 == 0 {
                        "electronics".to_string()
                    } else {
                        "books".to_string()
                    }),
                ),
            });

            metadata.push(crate::proto::proximadb_v1::MetadataItem {
                key: "price".to_string(),
                value: Some(
                    crate::proto::proximadb_v1::metadata_item::Value::NumberValue(
                        (50 + (i * 10) % 200) as f64,
                    ),
                ),
            });

            metadata.push(crate::proto::proximadb_v1::MetadataItem {
                key: "brand".to_string(),
                value: Some(
                    crate::proto::proximadb_v1::metadata_item::Value::StringValue(if i % 7 == 0 {
                        "Apple".to_string()
                    } else {
                        "Samsung".to_string()
                    }),
                ),
            });

            let mut map_metadata = std::collections::HashMap::new();
            for item in metadata {
                if let Some(value) = item.value {
                    // Convert metadata_item::Value to sql_value::Value
                    let sql_value = match value {
                        crate::proto::proximadb_v1::metadata_item::Value::StringValue(s) => {
                            crate::proto::proximadb_v1::sql_value::Value::StringValue(s)
                        }
                        crate::proto::proximadb_v1::metadata_item::Value::NumberValue(n) => {
                            crate::proto::proximadb_v1::sql_value::Value::NumberValue(n)
                        }
                        crate::proto::proximadb_v1::metadata_item::Value::BoolValue(b) => {
                            crate::proto::proximadb_v1::sql_value::Value::BoolValue(b)
                        }
                    };
                    map_metadata.insert(
                        item.key,
                        crate::proto::proximadb_v1::SqlValue {
                            value: Some(sql_value),
                        },
                    );
                }
            }

            records.push(ProximaRecord::from(VectorRecord {
                id: format!("vec_{}", i),
                vector: vec![0.1; 128], // Dummy vector
                metadata: map_metadata,
                timestamp: Some(1000000 + i as i64),
                updated_at: None,
                expires_at: None,
                version: Some(1),
                source: None,
            }));
        }

        records
    }
}
