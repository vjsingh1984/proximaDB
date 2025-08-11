//! Simple test to verify metadata filtering works correctly using gRPC

use proximadb::proto::proximadb::proxima_db_client::ProximaDbClient;
use proximadb::proto::proximadb::*;
use proximadb::proto::proximadb::vector_operation_response::ResultPayload;
use tracing::{debug, error, info, warn};

#[tokio::test]
#[ignore] // Requires server to be running
async fn test_metadata_filtering_simple() -> anyhow::Result<()> {
    info!("🔍 Starting simple metadata filter test");
    
    // Connect to running server
    let mut client = ProximaDbClient::connect("http://localhost:5679").await?;
    
    // Create test collection
    let collection_name = "simple_metadata_test";
    
    // Delete collection if exists
    let _ = client.collection_operation(tonic::Request::new(CollectionRequest {
        operation: CollectionOperation::CollectionDelete as i32,
        collection_id: Some(collection_name.to_string()),
        collection_config: None,
        query_params: Default::default(),
        options: Default::default(),
        migration_config: Default::default(),
    })).await;
    
    // Create collection
    let create_response = client.collection_operation(tonic::Request::new(CollectionRequest {
        operation: CollectionOperation::CollectionCreate as i32,
        collection_id: Some(collection_name.to_string()),
        collection_config: Some(CollectionConfig {
            name: collection_name.to_string(),
            dimension: 128,
            distance_metric: DistanceMetric::Cosine as i32,
            storage_engine: StorageEngine::Viper as i32,
            primary_indexing_algorithm: IndexingAlgorithm::Hnsw as i32,
            storage_location: None,
            compression: None,
            optimization_hints: None,
            filterable_columns: vec![
                FilterableColumnSpec {
                    name: "category".to_string(),
                    data_type: FilterableDataType::FilterableString as i32,
                    indexed: true,
                    supports_range: false,
                    estimated_cardinality: None,
                    encoding_hint: None,

                    },
            ],
            index_configs: vec![],
            quantization_config: None,
            primary_index_name: "default".to_string(),
            enable_automatic_index_selection: false,
            description: None,
            owner: None,
            tags: Default::default(),
        }),
        query_params: Default::default(),
        options: Default::default(),
        migration_config: Default::default(),
    })).await?;
    
    info!("✅ Created collection");
    
    // Insert test data
    let vectors = vec![
        VectorRecord {
            id: Some("vec_1".to_string()),
            vector: vec![0.1; 128],
            metadata: vec![
                MetadataItem {
                    key: "category".to_string(),
                    value: Some(metadata_item::Value::StringValue("electronics".to_string())),
                },
            ],
            timestamp: 0,
            updated_at: None,
            expires_at: None,
            distance: None,
            rank: None,
            score: None,
            ..Default::default()
        },
        VectorRecord {
            id: Some("vec_2".to_string()),
            vector: vec![0.2; 128],
            metadata: vec![
                MetadataItem {
                    key: "category".to_string(),
                    value: Some(metadata_item::Value::StringValue("books".to_string())),
                },
            ],
            timestamp: 0,
            updated_at: None,
            expires_at: None,
            distance: None,
            rank: None,
            score: None,
            ..Default::default()
        },
    ];
    
    let insert_request = VectorBatchRequest {
        collection_id: collection_name.to_string(),
        vectors,
        batch_timeout_ms: None,
        request_id: None,
    };
    
    let response = client.vector_batch(tonic::Request::new(insert_request)).await?;
    info!("✅ Inserted {} vectors", response.get_ref().vector_ids.len());
    
    // Wait for indexing
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
    
    // Search with filter
    debug!("\n🧪 Testing filter: category = 'electronics'");
    
    let search_request = VectorSearchRequest {
        collection_id: collection_name.to_string(),
        queries: vec![SearchQuery {
            vector: vec![0.1; 128],
            id: None,
            metadata_filter: Some(MetadataFilter {
                conditions: vec![FilterCondition {
                    field_name: "category".to_string(),
                    operation: FilterOperation::Equals as i32,
                    value: Some(MetadataValue {
                        value: Some(metadata_value::Value::StringValue("electronics".to_string())),
                    }),
                }],
                operator: FilterOperator::And as i32,
            }),
        }],
        top_k: 10,
        distance_metric_override: None,
        search_params: None,
        include_fields: Some(IncludeFields {
            vector: false,
            metadata: true,
            score: true,
            rank: true,
        }),
        search_optimization: None,
    };
    
    let search_response = client.vector_search(tonic::Request::new(search_request)).await?;
    let search_results = if let Some(ResultPayload::CompactResults(ref compact)) = search_response.get_ref().result_payload {
        &compact.results
    } else {
        warn!("⚠️  No compact results found");
        return Ok(());
    };
    
    info!("📊 Found {} results", search_results.len());
    
    for result in search_results {
        let mut category_value = "unknown";
        
        for metadata_item in &result.metadata {
            if metadata_item.key == "category" {
                if let Some(metadata_item::Value::StringValue(s)) = &metadata_item.value {
                    category_value = s;
                }
            }
        }
        
        let id = result.id.as_ref().map(|s| s.as_str()).unwrap_or("?");
        debug!("  - ID: {}, Category: {}", id, category_value);
        
        if category_value != "electronics" {
            error!("❌ ERROR: Expected 'electronics' but got '{}'", category_value);
        }
    }
    
    // Cleanup
    client.collection_operation(tonic::Request::new(CollectionRequest {
        operation: CollectionOperation::CollectionDelete as i32,
        collection_id: Some(collection_name.to_string()),
        collection_config: None,
        query_params: Default::default(),
        options: Default::default(),
        migration_config: Default::default(),
    })).await?;
    
    info!("\n✅ Test completed");
    Ok(())
}