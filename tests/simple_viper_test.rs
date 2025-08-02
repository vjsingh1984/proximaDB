//! Simple test to verify VIPER metadata filtering behavior using gRPC client

use proximadb::proto::proximadb::proxima_db_client::ProximaDbClient;
use proximadb::proto::proximadb::*;
use proximadb::proto::proximadb::vector_operation_response::ResultPayload;

#[tokio::test]
#[ignore] // Requires server to be running
async fn test_viper_metadata_filtering() -> anyhow::Result<()> {
    println!("🔍 Starting VIPER metadata filter test");
    
    // Connect to running server
    let mut client = ProximaDbClient::connect("http://localhost:5679").await?;
    
    // Create test collection with VIPER engine
    let collection_name = "simple_viper_test";
    
    // Delete collection if exists
    let _ = client.collection_operation(tonic::Request::new(CollectionRequest {
        operation: CollectionOperation::CollectionDelete as i32,
        collection_id: Some(collection_name.to_string()),
        collection_config: None,
        query_params: Default::default(),
        options: Default::default(),
        migration_config: Default::default(),
    })).await;
    
    // Create with VIPER engine
    let create_response = client.collection_operation(tonic::Request::new(CollectionRequest {
        operation: CollectionOperation::CollectionCreate as i32,
        collection_id: Some(collection_name.to_string()),
        collection_config: Some(CollectionConfig {
            name: collection_name.to_string(),
            dimension: 128,
            distance_metric: DistanceMetric::Cosine as i32,
            storage_engine: StorageEngine::Viper as i32,
            primary_indexing_algorithm: IndexingAlgorithm::Hnsw as i32,
            filterable_columns: vec![
                FilterableColumnSpec {
                    name: "category".to_string(),
                    data_type: FilterableDataType::FilterableString as i32,
                    indexed: true,
                    supports_range: false,
                    estimated_cardinality: None,
                },
                FilterableColumnSpec {
                    name: "brand".to_string(),
                    data_type: FilterableDataType::FilterableString as i32,
                    indexed: true,
                    supports_range: false,
                    estimated_cardinality: None,
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
    
    println!("✅ Created collection");
    
    // Insert test data - 3 electronics, 2 books
    let mut vectors = Vec::new();
    
    // Electronics
    vectors.push(VectorRecord {
        id: Some("elec_1".to_string()),
        vector: vec![0.1; 128],
        metadata: vec![
            MetadataItem {
                key: "category".to_string(),
                value: Some(metadata_item::Value::StringValue("electronics".to_string())),
            },
            MetadataItem {
                key: "brand".to_string(),
                value: Some(metadata_item::Value::StringValue("Apple".to_string())),
            },
        ],
        ..Default::default()
    });
    
    vectors.push(VectorRecord {
        id: Some("elec_2".to_string()),
        vector: vec![0.2; 128],
        metadata: vec![
            MetadataItem {
                key: "category".to_string(),
                value: Some(metadata_item::Value::StringValue("electronics".to_string())),
            },
            MetadataItem {
                key: "brand".to_string(),
                value: Some(metadata_item::Value::StringValue("Samsung".to_string())),
            },
        ],
        ..Default::default()
    });
    
    vectors.push(VectorRecord {
        id: Some("elec_3".to_string()),
        vector: vec![0.3; 128],
        metadata: vec![
            MetadataItem {
                key: "category".to_string(),
                value: Some(metadata_item::Value::StringValue("electronics".to_string())),
            },
            MetadataItem {
                key: "brand".to_string(),
                value: Some(metadata_item::Value::StringValue("Sony".to_string())),
            },
        ],
        ..Default::default()
    });
    
    // Books
    vectors.push(VectorRecord {
        id: Some("book_1".to_string()),
        vector: vec![0.4; 128],
        metadata: vec![
            MetadataItem {
                key: "category".to_string(),
                value: Some(metadata_item::Value::StringValue("books".to_string())),
            },
            MetadataItem {
                key: "brand".to_string(),
                value: Some(metadata_item::Value::StringValue("Penguin".to_string())),
            },
        ],
        ..Default::default()
    });
    
    vectors.push(VectorRecord {
        id: Some("book_2".to_string()),
        vector: vec![0.5; 128],
        metadata: vec![
            MetadataItem {
                key: "category".to_string(),
                value: Some(metadata_item::Value::StringValue("books".to_string())),
            },
            MetadataItem {
                key: "brand".to_string(),
                value: Some(metadata_item::Value::StringValue("OReilly".to_string())),
            },
        ],
        ..Default::default()
    });
    
    let insert_request = VectorBatchRequest {
        collection_id: collection_name.to_string(),
        vectors,
        batch_timeout_ms: None,
        request_id: None,
    };
    
    let response = client.vector_batch(tonic::Request::new(insert_request)).await?;
    println!("✅ Inserted {} vectors", response.get_ref().vector_ids.len());
    
    // Wait for indexing and flush
    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
    
    // Test 1: Filter by category = 'electronics' (should return 3)
    println!("\n🧪 Test 1: Filter by category = 'electronics'");
    
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
        println!("⚠️  No compact results found");
        return Ok(());
    };
    
    println!("📊 Found {} results (expected: 3)", search_results.len());
    
    let mut electronics_count = 0;
    let mut wrong_category_results = Vec::new();
    
    for result in search_results {
        let mut category_value = "unknown";
        
        for metadata_item in &result.metadata {
            if metadata_item.key == "category" {
                if let Some(value) = &metadata_item.value {
                    if let metadata_item::Value::StringValue(s) = value {
                        category_value = s.as_str();
                    }
                }
            }
        }
        
        let id = result.id.as_ref().map(|s| s.as_str()).unwrap_or("?");
        
        println!("  - ID: {}, Category: {}, Score: {}", id, category_value, result.score);
        
        if category_value == "electronics" {
            electronics_count += 1;
        } else {
            wrong_category_results.push(format!("ID: {}, Category: {}", id, category_value));
        }
    }
    
    println!("\n📈 Results Summary:");
    println!("  - Correct (electronics): {}", electronics_count);
    println!("  - Wrong category: {}", wrong_category_results.len());
    
    if !wrong_category_results.is_empty() {
        println!("❌ Wrong results found:");
        for wrong in &wrong_category_results {
            println!("    - {}", wrong);
        }
    }
    
    // Test 2: Filter by brand = 'Apple' (should return 1)
    println!("\n🧪 Test 2: Filter by brand = 'Apple'");
    
    let search_request = VectorSearchRequest {
        collection_id: collection_name.to_string(),
        queries: vec![SearchQuery {
            vector: vec![0.1; 128],
            id: None,
            metadata_filter: Some(MetadataFilter {
                conditions: vec![FilterCondition {
                    field_name: "brand".to_string(),
                    operation: FilterOperation::Equals as i32,
                    value: Some(MetadataValue {
                        value: Some(metadata_value::Value::StringValue("Apple".to_string())),
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
        println!("⚠️  No compact results found");
        return Ok(());
    };
    
    println!("📊 Found {} results (expected: 1)", search_results.len());
    
    let mut apple_count = 0;
    let mut wrong_brand_results = Vec::new();
    
    for result in search_results {
        let mut brand_value = "unknown";
        
        for metadata_item in &result.metadata {
            if metadata_item.key == "brand" {
                if let Some(value) = &metadata_item.value {
                    if let metadata_item::Value::StringValue(s) = value {
                        brand_value = s.as_str();
                    }
                }
            }
        }
        
        let id = result.id.as_ref().map(|s| s.as_str()).unwrap_or("?");
        
        println!("  - ID: {}, Brand: {}", id, brand_value);
        
        if brand_value == "Apple" {
            apple_count += 1;
        } else {
            wrong_brand_results.push(format!("ID: {}, Brand: {}", id, brand_value));
        }
    }
    
    println!("\n📈 Results Summary:");
    println!("  - Correct (Apple): {}", apple_count);
    println!("  - Wrong brand: {}", wrong_brand_results.len());
    
    if !wrong_brand_results.is_empty() {
        println!("❌ Wrong results found:");
        for wrong in &wrong_brand_results {
            println!("    - {}", wrong);
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
    
    // Report results
    println!("\n🔍 DIAGNOSIS COMPLETE:");
    println!("Electronics filter: {} correct, {} wrong", electronics_count, wrong_category_results.len());
    println!("Apple filter: {} correct, {} wrong", apple_count, wrong_brand_results.len());
    
    if wrong_category_results.is_empty() && wrong_brand_results.is_empty() {
        println!("✅ VIPER metadata filtering is working correctly!");
    } else {
        println!("❌ VIPER metadata filtering has issues - returning incorrect results");
        println!("This confirms the P0 issue: VIPER metadata filters return incorrect results");
    }
    
    Ok(())
}