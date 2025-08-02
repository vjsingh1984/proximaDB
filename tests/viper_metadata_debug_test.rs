//! Diagnostic test to debug VIPER metadata filtering issues using gRPC
//! 
//! This test isolates the metadata filtering problem where filters return incorrect results

use proximadb::proto::proximadb::proxima_db_client::ProximaDbClient;
use proximadb::proto::proximadb::*;
use proximadb::proto::proximadb::vector_operation_response::ResultPayload;

#[tokio::test]
#[ignore] // Requires server to be running
async fn test_viper_metadata_filter_diagnosis() -> anyhow::Result<()> {
    println!("🔍 Starting VIPER metadata filter diagnosis test");
    
    // Connect to running server
    let mut client = ProximaDbClient::connect("http://localhost:5679").await?;
    
    // Create test collection with VIPER engine
    let collection_name = "viper_metadata_debug_test";
    
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
                FilterableColumnSpec {
                    name: "price".to_string(),
                    data_type: FilterableDataType::FilterableFloat as i32,
                    indexed: true,
                    supports_range: true,
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
    
    // Insert test data - mix of electronics and books
    let mut vectors = Vec::new();
    
    // Electronics
    for i in 0..5 {
        vectors.push(VectorRecord {
            id: Some(format!("elec_{}", i)),
            vector: vec![0.1 + (i as f32 * 0.01); 128],
            metadata: vec![
                MetadataItem {
                    key: "category".to_string(),
                    value: Some(metadata_item::Value::StringValue("electronics".to_string())),
                },
                MetadataItem {
                    key: "brand".to_string(),
                    value: Some(metadata_item::Value::StringValue(
                        match i {
                            0 => "Apple",
                            1 => "Samsung",
                            2 => "Sony",
                            3 => "LG",
                            _ => "Generic",
                        }.to_string()
                    )),
                },
                MetadataItem {
                    key: "price".to_string(),
                    value: Some(metadata_item::Value::NumberValue((100 + i * 50) as f64)),
                },
            ],
            ..Default::default()
        });
    }
    
    // Books
    for i in 0..3 {
        vectors.push(VectorRecord {
            id: Some(format!("book_{}", i)),
            vector: vec![0.5 + (i as f32 * 0.01); 128],
            metadata: vec![
                MetadataItem {
                    key: "category".to_string(),
                    value: Some(metadata_item::Value::StringValue("books".to_string())),
                },
                MetadataItem {
                    key: "brand".to_string(),
                    value: Some(metadata_item::Value::StringValue(
                        match i {
                            0 => "Penguin",
                            1 => "OReilly",
                            _ => "Wiley",
                        }.to_string()
                    )),
                },
                MetadataItem {
                    key: "price".to_string(),
                    value: Some(metadata_item::Value::NumberValue((20 + i * 10) as f64)),
                },
            ],
            ..Default::default()
        });
    }
    
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
    
    // Test 1: Filter by category = 'electronics' (should return 5)
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
    
    println!("📊 Found {} results (expected: 5)", search_results.len());
    
    let mut electronics_count = 0;
    let mut books_count = 0;
    let mut wrong_results = Vec::new();
    
    for result in search_results {
        let mut category_value = "unknown";
        let mut brand_value = "unknown";
        
        for metadata_item in &result.metadata {
            match metadata_item.key.as_str() {
                "category" => {
                    if let Some(metadata_item::Value::StringValue(s)) = &metadata_item.value {
                        category_value = s.as_str();
                    }
                }
                "brand" => {
                    if let Some(metadata_item::Value::StringValue(s)) = &metadata_item.value {
                        brand_value = s.as_str();
                    }
                }
                _ => {}
            }
        }
        
        let id = result.id.as_ref().map(|s| s.as_str()).unwrap_or("?");
        
        println!("  - ID: {}, Category: {}, Brand: {}", id, category_value, brand_value);
        
        if category_value == "electronics" {
            electronics_count += 1;
        } else if category_value == "books" {
            books_count += 1;
            wrong_results.push(format!("ID: {}, Category: {}", id, category_value));
        }
    }
    
    println!("\n📈 Results Summary:");
    println!("  - Correct (electronics): {}", electronics_count);
    println!("  - Wrong (books): {}", books_count);
    println!("  - Total: {}", search_results.len());
    
    if !wrong_results.is_empty() {
        println!("❌ FILTER FAILURE: Found {} wrong results:", wrong_results.len());
        for wrong in &wrong_results {
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
    let mut non_apple_results = Vec::new();
    
    for result in search_results {
        let mut brand_value = "unknown";
        
        for metadata_item in &result.metadata {
            if metadata_item.key == "brand" {
                if let Some(metadata_item::Value::StringValue(s)) = &metadata_item.value {
                    brand_value = s.as_str();
                }
            }
        }
        
        let id = result.id.as_ref().map(|s| s.as_str()).unwrap_or("?");
        
        println!("  - ID: {}, Brand: {}", id, brand_value);
        
        if brand_value == "Apple" {
            apple_count += 1;
        } else {
            non_apple_results.push(format!("ID: {}, Brand: {}", id, brand_value));
        }
    }
    
    println!("\n📈 Results Summary:");
    println!("  - Correct (Apple): {}", apple_count);
    println!("  - Wrong (non-Apple): {}", non_apple_results.len());
    
    if !non_apple_results.is_empty() {
        println!("❌ FILTER FAILURE: Found {} wrong results:", non_apple_results.len());
        for wrong in &non_apple_results {
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
    if wrong_results.is_empty() && non_apple_results.is_empty() {
        println!("✅ VIPER metadata filtering is working correctly!");
    } else {
        println!("❌ VIPER metadata filtering has critical issues:");
        println!("  - Category filter returned {} wrong results", wrong_results.len());
        println!("  - Brand filter returned {} wrong results", non_apple_results.len());
        println!("This confirms the P0 issue: VIPER metadata filters return incorrect results");
    }
    
    Ok(())
}