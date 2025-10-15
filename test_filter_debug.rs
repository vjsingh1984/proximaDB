use proximadb::core::search::{ComparisonOperator, FilterExpression, SearchParams};
use proximadb::proto::proximadb_v1::{Collection, CollectionConfig, StorageAssignment, StorageConfig, VectorRecord, sql_value::Value as SqlValueEnum};
use proximadb::storage::engines::factory::StorageEngineFactory;
use proximadb::storage::traits::{FlushParameters, StorageQueryContext, StorageQueryMetadata};
use std::collections::HashMap;
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_target(false)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false)
        .init();

    println!("\n=== Testing ProximaBlock Filter ===\n");

    let base_path = "/tmp/test_filter_debug";
    // Clean up
    let _ = std::fs::remove_dir_all(base_path);

    // Create test vectors with metadata
    let mut records = Vec::new();
    for i in 0..10 {
        let mut metadata = HashMap::new();
        metadata.insert(
            "category".to_string(),
            proximadb::proto::proximadb_v1::SqlValue {
                value: Some(SqlValueEnum::StringValue(format!("cat_{}", i % 3))),
            },
        );
        metadata.insert(
            "price".to_string(),
            proximadb::proto::proximadb_v1::SqlValue {
                value: Some(SqlValueEnum::NumberValue((i as f64) * 100.0)),
            },
        );

        records.push(VectorRecord {
            id: format!("vec_{}", i),
            vector: vec![i as f32 / 10.0; 384],
            metadata,
            ..Default::default()
        });
    }

    println!("Created {} test vectors", records.len());
    println!("Sample metadata: {:?}", records[0].metadata);

    // Test SST engine
    println!("\n--- Testing SST Engine ---");
    let sst_engine = StorageEngineFactory::create_sst()?;
    let collection_id = "test-sst".to_string();

    let filterable_columns = vec![
        proximadb::proto::proximadb_v1::FilterableColumnSpec {
            name: "category".to_string(),
            data_type: proximadb::proto::proximadb_v1::FilterableDataType::FilterableString as i32,
            indexed: true,
            supports_range: false,
            estimated_cardinality: Some(3),
        },
        proximadb::proto::proximadb_v1::FilterableColumnSpec {
            name: "price".to_string(),
            data_type: proximadb::proto::proximadb_v1::FilterableDataType::FilterableFloat as i32,
            indexed: true,
            supports_range: true,
            estimated_cardinality: None,
        },
    ];

    let collection = Collection {
        id: collection_id.clone(),
        config: Some(CollectionConfig {
            name: collection_id.clone(),
            dimension: 384,
            filterable_columns: filterable_columns.clone(),
            ..Default::default()
        }),
        storage_assignment: Some(StorageAssignment {
            primary_path: base_path.to_string(),
            base_location: base_path.to_string(),
            ..Default::default()
        }),
        ..Default::default()
    };

    // Flush
    let flush_params = FlushParameters {
        collection_id: Some(collection_id.clone()),
        vector_records: records.clone(),
        force: true,
        synchronous: true,
        collection_config: Some(collection.clone()),
        ..Default::default()
    };

    println!("Flushing {} vectors...", records.len());
    let flush_result = sst_engine.flush(flush_params).await?;
    println!("Flushed {} vectors, {} bytes", flush_result.entries_flushed.unwrap_or(0), flush_result.bytes_written.unwrap_or(0));

    // Search without filter
    println!("\n--- Search without filter ---");
    let query_vector = vec![0.0; 384];
    let search_params = Arc::new(SearchParams {
        vector: Some(query_vector.clone()),
        top_k: Some(5),
        filters: None,
        filter_expression: None,
        ..Default::default()
    });

    let mut metadata = StorageQueryMetadata::default();
    metadata.collection_id = collection_id.clone();
    metadata.dimension = 384;
    metadata.storage_path = base_path.to_string();

    let ctx = StorageQueryContext {
        search_params: search_params.clone(),
        collection: Arc::new(collection.clone()),
        metadata,
    };

    let results = sst_engine.search_vectors_unified(&ctx).await?;
    println!("Found {} results without filter", results.len());
    for (i, r) in results.iter().take(3).enumerate() {
        println!("  [{}] ID={}, metadata={:?}", i, r.id, r.metadata);
    }

    // Search with filter: category="cat_1"
    println!("\n--- Search with filter: category='cat_1' ---");
    let filter_expr = FilterExpression::Comparison {
        field: "category".to_string(),
        operator: ComparisonOperator::Equals,
        value: serde_json::Value::String("cat_1".to_string()),
    };

    let search_params_filtered = Arc::new(SearchParams {
        vector: Some(query_vector.clone()),
        top_k: Some(5),
        filter_expression: Some(filter_expr),
        filters: None,
        ..Default::default()
    });

    let mut metadata = StorageQueryMetadata::default();
    metadata.collection_id = collection_id.clone();
    metadata.dimension = 384;
    metadata.storage_path = base_path.to_string();

    let ctx_filtered = StorageQueryContext {
        search_params: search_params_filtered,
        collection: Arc::new(collection.clone()),
        metadata,
    };

    let filtered_results = sst_engine.search_vectors_unified(&ctx_filtered).await?;
    println!("Found {} results with filter", filtered_results.len());
    for (i, r) in filtered_results.iter().take(3).enumerate() {
        println!("  [{}] ID={}, metadata={:?}", i, r.id, r.metadata);
    }

    // Expected: vec_1, vec_4, vec_7 (indices 1, 4, 7 have category=cat_1)
    println!("\nExpected IDs with category='cat_1': vec_1, vec_4, vec_7");

    if filtered_results.is_empty() {
        println!("\n❌ FAILURE: No results returned with filter!");
    } else {
        println!("\n✅ SUCCESS: Filter returned {} results", filtered_results.len());
    }

    Ok(())
}
