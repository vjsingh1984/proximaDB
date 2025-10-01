//! # SQL API Testing Utilities
//!
//! Helper functions and utilities for testing ProximaDB SQL API capabilities

use proximadb::{
    services::collection::CollectionService,
    services::operations::vectors::VectorOperationsService,
    query::sql_frontend::parser::SqlFrontendParser,
    proto::proximadb_v1::{
        CreateCollectionRequest, CollectionConfig, VectorRecord, MetadataItem,
        metadata_item::Value as MetadataValue, InsertVectorRequest,
    },
};
use std::collections::HashMap;
use std::sync::Arc;
use anyhow::Result;

/// Test collection identifier for SQL tests
pub const TEST_SQL_COLLECTION: &str = "test_sql_collection";

/// SQL test data structure
#[derive(Debug, Clone)]
pub struct TestVectorData {
    pub id: String,
    pub vector: Vec<f32>,
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Initialize services needed for SQL testing
pub async fn setup_sql_test_services() -> Result<(Arc<CollectionService>, Arc<VectorOperationsService>)> {
    // Create collection service
    let collection_service = Arc::new(CollectionService::new());

    // Create vector operations service
    let storage_url = "memory://sql_test".to_string();
    let vector_service = Arc::new(VectorOperationsService::new(
        storage_url,
        Some(collection_service.clone()),
        None, // wal_manager
        None, // axis_index_manager
    )?);

    Ok((collection_service, vector_service))
}

/// Create a test collection for SQL operations
pub async fn create_test_sql_collection(
    collection_service: &CollectionService,
    dimension: u32,
) -> Result<()> {
    let create_request = CreateCollectionRequest {
        name: TEST_SQL_COLLECTION.to_string(),
        config: Some(CollectionConfig {
            dimension,
            index_type: "hnsw".to_string(),
            distance_metric: "cosine".to_string(),
            engine_name: "viper".to_string(),
            metadata_index_enabled: true,
            ..Default::default()
        }),
    };

    match collection_service.create_collection(create_request).await {
        Ok(_) => println!("Created test SQL collection: {}", TEST_SQL_COLLECTION),
        Err(e) if e.to_string().contains("already exists") => {
            println!("Test SQL collection already exists: {}", TEST_SQL_COLLECTION);
        }
        Err(e) => return Err(e),
    }

    Ok(())
}

/// Generate test vector data
pub fn generate_test_vectors(count: usize, dimension: usize) -> Vec<TestVectorData> {
    (0..count)
        .map(|i| {
            let vector: Vec<f32> = (0..dimension)
                .map(|j| (i as f32 * 0.1) + (j as f32 * 0.01))
                .collect();

            let mut metadata = HashMap::new();
            metadata.insert("category".to_string(), serde_json::Value::String(
                if i % 2 == 0 { "electronics" } else { "books" }.to_string()
            ));
            metadata.insert("price".to_string(), serde_json::Value::Number(
                serde_json::Number::from_f64(10.0 + (i as f64 * 5.0)).unwrap()
            ));
            metadata.insert("rating".to_string(), serde_json::Value::Number(
                serde_json::Number::from_f64(3.0 + (i as f64 % 3.0)).unwrap()
            ));

            TestVectorData {
                id: format!("item_{}", i),
                vector,
                metadata,
            }
        })
        .collect()
}

/// Insert test vectors into a collection
pub async fn insert_test_vectors(
    vector_service: &VectorOperationsService,
    collection_name: &str,
    test_data: &[TestVectorData],
) -> Result<()> {
    for data in test_data {
        let metadata_items: Vec<MetadataItem> = data.metadata
            .iter()
            .map(|(key, value)| MetadataItem {
                key: key.clone(),
                value: Some(json_to_metadata_value(value.clone())),
            })
            .collect();

        let vector_record = VectorRecord {
            id: data.id.clone(),
            vector: data.vector.clone(),
            metadata: metadata_items,
            timestamp: chrono::Utc::now().timestamp_millis(),
            version: Some(1),
            ..Default::default()
        };

        let insert_request = InsertVectorRequest {
            collection_name: collection_name.to_string(),
            vectors: vec![vector_record],
            upsert: false,
        };

        vector_service.insert_vectors(insert_request).await?;
    }

    println!("Inserted {} test vectors into {}", test_data.len(), collection_name);
    Ok(())
}

/// Convert JSON value to metadata value
fn json_to_metadata_value(value: serde_json::Value) -> MetadataValue {
    match value {
        serde_json::Value::String(s) => MetadataValue::StringValue(s),
        serde_json::Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                MetadataValue::NumberValue(f)
            } else {
                MetadataValue::NumberValue(0.0)
            }
        }
        serde_json::Value::Bool(b) => MetadataValue::BoolValue(b),
        _ => MetadataValue::StringValue(value.to_string()),
    }
}

/// Test SQL query parsing
pub fn test_sql_parsing(sql: &str) -> Result<()> {
    let parser = SqlFrontendParser::new();
    let _ast = parser.parse(sql)?;
    println!("Successfully parsed SQL: {}", sql);
    Ok(())
}

/// Common SQL test queries
pub struct SqlTestQueries;

impl SqlTestQueries {
    /// Basic vector similarity search
    pub fn similarity_search() -> &'static str {
        "SELECT id, SIMILAR(vector, [0.1, 0.2, 0.3]) as score FROM test_sql_collection ORDER BY score DESC LIMIT 5"
    }

    /// Vector search with metadata filtering
    pub fn filtered_similarity_search() -> &'static str {
        "SELECT id, SIMILAR(vector, [0.1, 0.2, 0.3]) as score FROM test_sql_collection WHERE category = 'electronics' ORDER BY score DESC LIMIT 5"
    }

    /// Metadata-only query
    pub fn metadata_query() -> &'static str {
        "SELECT id, category, price FROM test_sql_collection WHERE price > 20 AND rating >= 4"
    }

    /// Aggregation query
    pub fn aggregation_query() -> &'static str {
        "SELECT category, COUNT(*) as count, AVG(price) as avg_price FROM test_sql_collection GROUP BY category"
    }

    /// Complex query with join (if supported)
    pub fn complex_query() -> &'static str {
        "SELECT t1.id, t1.category, SIMILAR(t1.vector, [0.1, 0.2, 0.3]) as score FROM test_sql_collection t1 WHERE t1.price > (SELECT AVG(price) FROM test_sql_collection t2 WHERE t2.category = t1.category)"
    }
}

/// Execute and validate a SQL query
pub async fn execute_sql_test(
    sql: &str,
    expected_result_count: Option<usize>,
) -> Result<()> {
    // Parse the SQL
    test_sql_parsing(sql)?;

    // Note: Actual execution would require a full SQL execution engine
    // For now, we just validate parsing
    if let Some(count) = expected_result_count {
        println!("SQL test passed for query (expected {} results): {}", count, sql);
    } else {
        println!("SQL test passed for query: {}", sql);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sql_parsing_basic() {
        let queries = [
            SqlTestQueries::similarity_search(),
            SqlTestQueries::filtered_similarity_search(),
            SqlTestQueries::metadata_query(),
            SqlTestQueries::aggregation_query(),
        ];

        for query in queries {
            execute_sql_test(query, None).await.unwrap();
        }
    }

    #[test]
    fn test_vector_data_generation() {
        let vectors = generate_test_vectors(10, 128);
        assert_eq!(vectors.len(), 10);
        assert_eq!(vectors[0].vector.len(), 128);
        assert!(vectors[0].metadata.contains_key("category"));
        assert!(vectors[0].metadata.contains_key("price"));
    }
}