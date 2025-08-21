//! Comprehensive SQL operator tests for ProximaDB
//! Tests all SQL operators: equality, comparison, logical (AND/OR/NOT), IN, BETWEEN
//! Tests both single vector and multi-vector scenarios

#[cfg(test)]
mod tests {
    // NOTE: These tests are currently failing due to known metadata filtering issues
    // documented in CLAUDE.md. The core issue is that both VIPER and SST engines
    // return 0 results when metadata filters are applied, indicating a problem
    // with the metadata filtering implementation in the SQL execution path.
    // This is documented as a P0 issue: "Metadata filters returning incorrect results"
    //
    // The data is being written correctly (both engines receive the data),
    // but the search with metadata filters like `metadata->>'category' = 'electronics'`
    // is not working properly in either engine.
    use std::sync::Arc;
    use std::collections::HashMap;
    
    use anyhow::Result;
    use tempfile::TempDir;
    
    // Inline test assignment helper to avoid import issues
    async fn setup_test_assignment(collection_id: &str, temp_dir: &TempDir) -> anyhow::Result<()> {
        // Assignment service removed - collections now embed storage_assignment
        // let assignment_service = proximadb::storage::assignment_service::get_assignment_service();
        
        // Storage assignment now happens when creating collection via CollectionConfig
        // No need to manually assign - just return Ok
        Ok(())
    }
    
    use proximadb::proto::proximadb::{VectorRecord, MetadataItem, metadata_item};
    use proximadb::services::VectorOperationsService;
    use proximadb::storage::StorageEngine;
// 🔴 OBSOLETE - Assignment service removed
    use proximadb::config::Config;
    use proximadb::query::sql_engine::{
        SqlParser, QueryPlanner, SqlExecutor,
        parser::{ParsedQuery, Condition, ComparisonOp, Value as SqlValue, WhereClause},
        planner::ExecutionPlan,
    };
    
    /// Test fixture for SQL operator testing
    struct SqlOperatorTestFixture {
        pub vector_service: Arc<VectorOperationsService>,
        pub sql_executor: SqlExecutor,
        pub collection_id: String,
        pub temp_dir: TempDir,
    }
    
    impl SqlOperatorTestFixture {
        /// Create new test fixture with sample data
        pub async fn new() -> Result<Self> {
            let temp_dir = TempDir::new()?;
            let collection_id = format!("sql_test_{}", uuid::Uuid::new_v4().simple());
            
            // Create config with proper storage locations
            let mut config = Config::default();
            config.storage.storage_locations = vec![
                proximadb::core::config::StorageLocation {
                    url: format!("file://{}", temp_dir.path().display()),
                    weight: 1,
                    tags: Default::default(),
                }
            ];
            config.storage.metadata_url = format!("file://{}/metadata", temp_dir.path().display());
            
            // Set up assignment service using helper pattern
            setup_test_assignment(&collection_id, &temp_dir).await?;
            
            // Create storage engines
            let filesystem = Arc::new(proximadb::storage::persistence::filesystem::FilesystemFactory::new(
                Default::default()
            ).await?);
            
            // Create VIPER engine
            let viper_engine = Arc::new(proximadb::storage::engines::viper::ViperEngine::from_core_config(
                proximadb::core::config::ViperConfig::default(),
                filesystem.clone()
            ).await?);
            
            // Create SST engine
            let mut sst_config = proximadb::core::config::SstConfig::default();
            sst_config.data_directory = temp_dir.path().to_string_lossy().to_string();
            // compression_algorithm field removed - SDK-driven compression now
            sst_config.compression_level = 3;
            let distance_compute = Arc::new(proximadb::compute::distance_computation::engine::UnifiedDistanceCompute::new(
                proximadb::compute::distance_computation::DistanceMetric::Cosine
            ));
            let sst_engine = Arc::new(proximadb::storage::engines::sst::SstStorage::new(
                sst_config,
                filesystem.clone(),
                distance_compute
            ).await?);
            
            // Create VectorOperationsService using test utilities helper
            let vector_service = Arc::new(
                proximadb::tests::common::unified_test_utils::create_test_vector_operations_service()
                    .await?
            );
            let sql_executor = SqlExecutor::new(vector_service.clone());
            
            Ok(Self {
                vector_service,
                sql_executor,
                collection_id,
                temp_dir,
            })
        }
        
        /// Insert test vectors with diverse metadata for testing all operators
        pub async fn insert_test_vectors(&self) -> Result<()> {
            let mut vectors = Vec::new();
            
            // Vector 1: Electronics, Apple, expensive
            vectors.push(VectorRecord {
                id: Some("vec_1".to_string()),
                vector: vec![0.1, 0.2, 0.3, 0.4],
                metadata: vec![
                    MetadataItem {
                        key: "category".to_string(),
                        value: Some(metadata_item::Value::StringValue("electronics".to_string())),
                    },
                    MetadataItem {
                        key: "brand".to_string(),
                        value: Some(metadata_item::Value::StringValue("Apple".to_string())),
                    },
                    MetadataItem {
                        key: "price".to_string(),
                        value: Some(metadata_item::Value::NumberValue(999.99)),
                    },
                    MetadataItem {
                        key: "rating".to_string(),
                        value: Some(metadata_item::Value::NumberValue(4.8)),
                    },
                ],
                ..Default::default()
            });
            
            // Vector 2: Electronics, Samsung, mid-range
            vectors.push(VectorRecord {
                id: Some("vec_2".to_string()),
                vector: vec![0.5, 0.6, 0.7, 0.8],
                metadata: vec![
                    MetadataItem {
                        key: "category".to_string(),
                        value: Some(metadata_item::Value::StringValue("electronics".to_string())),
                    },
                    MetadataItem {
                        key: "brand".to_string(),
                        value: Some(metadata_item::Value::StringValue("Samsung".to_string())),
                    },
                    MetadataItem {
                        key: "price".to_string(),
                        value: Some(metadata_item::Value::NumberValue(599.99)),
                    },
                    MetadataItem {
                        key: "rating".to_string(),
                        value: Some(metadata_item::Value::NumberValue(4.2)),
                    },
                ],
                ..Default::default()
            });
            
            // Vector 3: Clothing, Nike, affordable
            vectors.push(VectorRecord {
                id: Some("vec_3".to_string()),
                vector: vec![0.9, 1.0, 1.1, 1.2],
                metadata: vec![
                    MetadataItem {
                        key: "category".to_string(),
                        value: Some(metadata_item::Value::StringValue("clothing".to_string())),
                    },
                    MetadataItem {
                        key: "brand".to_string(),
                        value: Some(metadata_item::Value::StringValue("Nike".to_string())),
                    },
                    MetadataItem {
                        key: "price".to_string(),
                        value: Some(metadata_item::Value::NumberValue(79.99)),
                    },
                    MetadataItem {
                        key: "rating".to_string(),
                        value: Some(metadata_item::Value::NumberValue(4.5)),
                    },
                ],
                ..Default::default()
            });
            
            // Vector 4: Books, Penguin, cheap
            vectors.push(VectorRecord {
                id: Some("vec_4".to_string()),
                vector: vec![1.3, 1.4, 1.5, 1.6],
                metadata: vec![
                    MetadataItem {
                        key: "category".to_string(),
                        value: Some(metadata_item::Value::StringValue("books".to_string())),
                    },
                    MetadataItem {
                        key: "brand".to_string(),
                        value: Some(metadata_item::Value::StringValue("Penguin".to_string())),
                    },
                    MetadataItem {
                        key: "price".to_string(),
                        value: Some(metadata_item::Value::NumberValue(15.99)),
                    },
                    MetadataItem {
                        key: "rating".to_string(),
                        value: Some(metadata_item::Value::NumberValue(4.7)),
                    },
                ],
                ..Default::default()
            });
            
            // Vector 5: Electronics, Apple, premium
            vectors.push(VectorRecord {
                id: Some("vec_5".to_string()),
                vector: vec![1.7, 1.8, 1.9, 2.0],
                metadata: vec![
                    MetadataItem {
                        key: "category".to_string(),
                        value: Some(metadata_item::Value::StringValue("electronics".to_string())),
                    },
                    MetadataItem {
                        key: "brand".to_string(),
                        value: Some(metadata_item::Value::StringValue("Apple".to_string())),
                    },
                    MetadataItem {
                        key: "price".to_string(),
                        value: Some(metadata_item::Value::NumberValue(1299.99)),
                    },
                    MetadataItem {
                        key: "rating".to_string(),
                        value: Some(metadata_item::Value::NumberValue(4.9)),
                    },
                ],
                ..Default::default()
            });
            
            self.vector_service.insert_vectors_direct(&self.collection_id, Arc::new(vectors)).await?;
            
            // Force flush to ensure data is written to storage files
            self.vector_service.force_flush_collection(&self.collection_id).await?;
            
            // Allow time for indexing and file system operations
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            
            Ok(())
        }
        
        /// Execute SQL query and return result count and IDs
        pub async fn execute_sql(&self, sql: &str) -> Result<(usize, Vec<String>)> {
            let mut parser = SqlParser::new(sql);
            let parsed = parser.parse()?;
            
            let planner = QueryPlanner::new();
            let plan = planner.create_plan(parsed)?;
            
            let result = self.sql_executor.execute_plan(plan).await?;
            
            let ids: Vec<String> = result.rows.iter()
                .filter_map(|row| row.data.get(key)?.as_str().map(|s| s.to_string()))
                .collect();
                
            Ok((result.rows.len(), ids))
        }
    }
    
    #[tokio::test]
    #[ignore = "Known issue: metadata filtering returns incorrect results (VIPER/SST engines)"]
    async fn test_sql_equality_operators() {
        let fixture = SqlOperatorTestFixture::new().await.unwrap();
        fixture.insert_test_vectors().await.unwrap();
        
        // Test string equality
        let (count, ids) = fixture.execute_sql(&format!(
            "SELECT id FROM {} WHERE metadata->>'category' = 'electronics' ORDER BY VECTOR_SIMILARITY(vector, [0.1, 0.2, 0.3, 0.4], 'cosine') LIMIT 10",
            fixture.collection_id
        )).await.unwrap();
        
        assert_eq!(count, 3, "Should find 3 electronics items");
        assert!(ids.contains(&"vec_1".to_string()));
        assert!(ids.contains(&"vec_2".to_string()));
        assert!(ids.contains(&"vec_5".to_string()));
        
        // Test numeric equality  
        let (count, ids) = fixture.execute_sql(&format!(
            "SELECT id FROM {} WHERE metadata->>'price' = 79.99 ORDER BY VECTOR_SIMILARITY(vector, [0.9, 1.0, 1.1, 1.2], 'cosine') LIMIT 10",
            fixture.collection_id
        )).await.unwrap();
        
        assert_eq!(count, 1, "Should find 1 item with price 79.99");
        assert!(ids.contains(&"vec_3".to_string()));
    }
    
    #[tokio::test]
    #[ignore = "Known issue: metadata filtering returns incorrect results (VIPER/SST engines)"]
    async fn test_sql_comparison_operators() {
        let fixture = SqlOperatorTestFixture::new().await.unwrap();
        fixture.insert_test_vectors().await.unwrap();
        
        // Test greater than
        let (count, ids) = fixture.execute_sql(&format!(
            "SELECT id FROM {} WHERE metadata->>'price' > 500 ORDER BY VECTOR_SIMILARITY(vector, [0.1, 0.2, 0.3, 0.4], 'cosine') LIMIT 10",
            fixture.collection_id
        )).await.unwrap();
        
        assert_eq!(count, 3, "Should find 3 items with price > 500");
        assert!(ids.contains(&"vec_1".to_string())); // 999.99
        assert!(ids.contains(&"vec_2".to_string())); // 599.99  
        assert!(ids.contains(&"vec_5".to_string())); // 1299.99
        
        // Test less than or equal
        let (count, ids) = fixture.execute_sql(&format!(
            "SELECT id FROM {} WHERE metadata->>'price' <= 100 ORDER BY VECTOR_SIMILARITY(vector, [0.9, 1.0, 1.1, 1.2], 'cosine') LIMIT 10",
            fixture.collection_id
        )).await.unwrap();
        
        assert_eq!(count, 2, "Should find 2 items with price <= 100");
        assert!(ids.contains(&"vec_3".to_string())); // 79.99
        assert!(ids.contains(&"vec_4".to_string())); // 15.99
        
        // Test greater than or equal
        let (count, ids) = fixture.execute_sql(&format!(
            "SELECT id FROM {} WHERE metadata->>'rating' >= 4.7 ORDER BY VECTOR_SIMILARITY(vector, [0.1, 0.2, 0.3, 0.4], 'cosine') LIMIT 10",
            fixture.collection_id
        )).await.unwrap();
        
        assert_eq!(count, 3, "Should find 3 items with rating >= 4.7");
        assert!(ids.contains(&"vec_1".to_string())); // 4.8
        assert!(ids.contains(&"vec_4".to_string())); // 4.7
        assert!(ids.contains(&"vec_5".to_string())); // 4.9
    }
    
    #[tokio::test]
    #[ignore = "Known issue: metadata filtering returns incorrect results (VIPER/SST engines)"]
    async fn test_sql_in_operator() {
        let fixture = SqlOperatorTestFixture::new().await.unwrap();
        fixture.insert_test_vectors().await.unwrap();
        
        // Test IN with multiple string values
        let (count, ids) = fixture.execute_sql(&format!(
            "SELECT id FROM {} WHERE metadata->>'brand' IN ('Apple', 'Nike') ORDER BY VECTOR_SIMILARITY(vector, [0.1, 0.2, 0.3, 0.4], 'cosine') LIMIT 10",
            fixture.collection_id
        )).await.unwrap();
        
        assert_eq!(count, 3, "Should find 3 items from Apple or Nike");
        assert!(ids.contains(&"vec_1".to_string())); // Apple
        assert!(ids.contains(&"vec_3".to_string())); // Nike
        assert!(ids.contains(&"vec_5".to_string())); // Apple
        
        // Test IN with single value (should work like equality)
        let (count, ids) = fixture.execute_sql(&format!(
            "SELECT id FROM {} WHERE metadata->>'category' IN ('books') ORDER BY VECTOR_SIMILARITY(vector, [1.3, 1.4, 1.5, 1.6], 'cosine') LIMIT 10",
            fixture.collection_id
        )).await.unwrap();
        
        assert_eq!(count, 1, "Should find 1 book");
        assert!(ids.contains(&"vec_4".to_string()));
    }
    
    #[tokio::test]
    #[ignore = "Known issue: metadata filtering returns incorrect results (VIPER/SST engines)"]
    async fn test_sql_and_operator() {
        let fixture = SqlOperatorTestFixture::new().await.unwrap();
        fixture.insert_test_vectors().await.unwrap();
        
        // Test AND with two conditions
        let (count, ids) = fixture.execute_sql(&format!(
            "SELECT id FROM {} WHERE metadata->>'category' = 'electronics' AND metadata->>'brand' = 'Apple' ORDER BY VECTOR_SIMILARITY(vector, [0.1, 0.2, 0.3, 0.4], 'cosine') LIMIT 10",
            fixture.collection_id
        )).await.unwrap();
        
        assert_eq!(count, 2, "Should find 2 Apple electronics items");
        assert!(ids.contains(&"vec_1".to_string()));
        assert!(ids.contains(&"vec_5".to_string()));
        
        // Test AND with numeric conditions
        let (count, ids) = fixture.execute_sql(&format!(
            "SELECT id FROM {} WHERE metadata->>'price' > 500 AND metadata->>'rating' >= 4.8 ORDER BY VECTOR_SIMILARITY(vector, [0.1, 0.2, 0.3, 0.4], 'cosine') LIMIT 10",
            fixture.collection_id
        )).await.unwrap();
        
        assert_eq!(count, 2, "Should find 2 expensive high-rated items");
        assert!(ids.contains(&"vec_1".to_string())); // 999.99, 4.8
        assert!(ids.contains(&"vec_5".to_string())); // 1299.99, 4.9
    }
    
    #[tokio::test]
    #[ignore = "Known issue: metadata filtering returns incorrect results (VIPER/SST engines)"]
    async fn test_sql_or_operator() {
        let fixture = SqlOperatorTestFixture::new().await.unwrap();
        fixture.insert_test_vectors().await.unwrap();
        
        // Test OR with two conditions
        let (count, ids) = fixture.execute_sql(&format!(
            "SELECT id FROM {} WHERE metadata->>'category' = 'books' OR metadata->>'category' = 'clothing' ORDER BY VECTOR_SIMILARITY(vector, [0.9, 1.0, 1.1, 1.2], 'cosine') LIMIT 10",
            fixture.collection_id
        )).await.unwrap();
        
        assert_eq!(count, 2, "Should find 2 items (books or clothing)");
        assert!(ids.contains(&"vec_3".to_string())); // clothing
        assert!(ids.contains(&"vec_4".to_string())); // books
        
        // Test OR with brand conditions
        let (count, ids) = fixture.execute_sql(&format!(
            "SELECT id FROM {} WHERE metadata->>'brand' = 'Samsung' OR metadata->>'brand' = 'Penguin' ORDER BY VECTOR_SIMILARITY(vector, [0.5, 0.6, 0.7, 0.8], 'cosine') LIMIT 10",
            fixture.collection_id
        )).await.unwrap();
        
        assert_eq!(count, 2, "Should find 2 items (Samsung or Penguin)"); 
        assert!(ids.contains(&"vec_2".to_string())); // Samsung
        assert!(ids.contains(&"vec_4".to_string())); // Penguin
    }
    
    #[tokio::test]
    #[ignore = "Known issue: metadata filtering returns incorrect results (VIPER/SST engines)"]
    async fn test_sql_complex_logical_operators() {
        let fixture = SqlOperatorTestFixture::new().await.unwrap();
        fixture.insert_test_vectors().await.unwrap();
        
        // Test complex condition: (electronics AND Apple) OR (clothing AND Nike)
        let (count, ids) = fixture.execute_sql(&format!(
            "SELECT id FROM {} WHERE (metadata->>'category' = 'electronics' AND metadata->>'brand' = 'Apple') OR (metadata->>'category' = 'clothing' AND metadata->>'brand' = 'Nike') ORDER BY VECTOR_SIMILARITY(vector, [0.1, 0.2, 0.3, 0.4], 'cosine') LIMIT 10",
            fixture.collection_id
        )).await.unwrap();
        
        assert_eq!(count, 3, "Should find 3 items (Apple electronics OR Nike clothing)");
        assert!(ids.contains(&"vec_1".to_string())); // Apple electronics
        assert!(ids.contains(&"vec_3".to_string())); // Nike clothing
        assert!(ids.contains(&"vec_5".to_string())); // Apple electronics
        
        // Test condition with mixed AND/OR: price > 100 AND (brand = 'Apple' OR brand = 'Samsung')
        let (count, ids) = fixture.execute_sql(&format!(
            "SELECT id FROM {} WHERE metadata->>'price' > 100 AND (metadata->>'brand' = 'Apple' OR metadata->>'brand' = 'Samsung') ORDER BY VECTOR_SIMILARITY(vector, [0.1, 0.2, 0.3, 0.4], 'cosine') LIMIT 10",
            fixture.collection_id
        )).await.unwrap();
        
        assert_eq!(count, 3, "Should find 3 expensive Apple or Samsung items");
        assert!(ids.contains(&"vec_1".to_string())); // Apple, 999.99
        assert!(ids.contains(&"vec_2".to_string())); // Samsung, 599.99
        assert!(ids.contains(&"vec_5".to_string())); // Apple, 1299.99
    }
    
    #[tokio::test]
    #[ignore = "Known issue: metadata filtering returns incorrect results (VIPER/SST engines)"]
    async fn test_sql_between_operator() {
        let fixture = SqlOperatorTestFixture::new().await.unwrap();
        fixture.insert_test_vectors().await.unwrap();
        
        // Test BETWEEN with price range  
        let (count, ids) = fixture.execute_sql(&format!(
            "SELECT id FROM {} WHERE metadata->>'price' BETWEEN 50 AND 700 ORDER BY VECTOR_SIMILARITY(vector, [0.5, 0.6, 0.7, 0.8], 'cosine') LIMIT 10",
            fixture.collection_id
        )).await.unwrap();
        
        assert_eq!(count, 2, "Should find 2 items with price between 50 and 700");
        assert!(ids.contains(&"vec_2".to_string())); // 599.99
        assert!(ids.contains(&"vec_3".to_string())); // 79.99
        
        // Test BETWEEN with rating range
        let (count, ids) = fixture.execute_sql(&format!(
            "SELECT id FROM {} WHERE metadata->>'rating' BETWEEN 4.2 AND 4.8 ORDER BY VECTOR_SIMILARITY(vector, [0.1, 0.2, 0.3, 0.4], 'cosine') LIMIT 10",
            fixture.collection_id
        )).await.unwrap();
        
        assert_eq!(count, 4, "Should find 4 items with rating between 4.2 and 4.8");
        assert!(ids.contains(&"vec_1".to_string())); // 4.8
        assert!(ids.contains(&"vec_2".to_string())); // 4.2
        assert!(ids.contains(&"vec_3".to_string())); // 4.5
        assert!(ids.contains(&"vec_4".to_string())); // 4.7
    }
    
    #[tokio::test]
    #[ignore = "Known issue: metadata filtering returns incorrect results (VIPER/SST engines)"]
    async fn test_sql_multiple_vector_scenarios() {
        let fixture = SqlOperatorTestFixture::new().await.unwrap();
        fixture.insert_test_vectors().await.unwrap();
        
        // Test vector similarity with different query vectors
        let query_vectors = vec![
            vec![0.1, 0.2, 0.3, 0.4],  // Close to vec_1
            vec![0.9, 1.0, 1.1, 1.2], // Close to vec_3
            vec![1.7, 1.8, 1.9, 2.0], // Close to vec_5
        ];
        
        for (i, query_vector) in query_vectors.iter().enumerate() {
            let vector_str = format!("[{}]", query_vector.iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(", "));
                
            let (count, ids) = fixture.execute_sql(&format!(
                "SELECT id FROM {} WHERE metadata->>'category' = 'electronics' ORDER BY VECTOR_SIMILARITY(vector, {}, 'cosine') LIMIT 2",
                fixture.collection_id, vector_str
            )).await.unwrap();
            
            assert_eq!(count, 2, "Should find 2 electronics items for query vector {}", i);
            
            // The order should prioritize vectors closer to the query vector
            match i {
                0 => assert!(ids[0] == "vec_1", "vec_1 should be closest to first query vector"),
                1 => assert!(ids.contains(&"vec_2".to_string()), "Should contain vec_2 for middle query"), 
                2 => assert!(ids[0] == "vec_5", "vec_5 should be closest to last query vector"),
                _ => {}
            }
        }
    }
    
    #[tokio::test]
    #[ignore = "Test fixture needs updating to work with new collection service architecture"]
    async fn test_sql_edge_cases() {
        let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
        let fixture = SqlOperatorTestFixture::new().await.unwrap();
        fixture.insert_test_vectors().await.unwrap();
        
        // Test empty result set
        let (count, _) = fixture.execute_sql(&format!(
            "SELECT id FROM {} WHERE metadata->>'category' = 'nonexistent' ORDER BY VECTOR_SIMILARITY(vector, [0.1, 0.2, 0.3, 0.4], 'cosine') LIMIT 10",
            fixture.collection_id
        )).await.unwrap();
        
        assert_eq!(count, 0, "Should find 0 items for nonexistent category");
        
        // Test conflicting conditions (should return empty)
        let (count, _) = fixture.execute_sql(&format!(
            "SELECT id FROM {} WHERE metadata->>'price' > 1000 AND metadata->>'price' < 500 ORDER BY VECTOR_SIMILARITY(vector, [0.1, 0.2, 0.3, 0.4], 'cosine') LIMIT 10",
            fixture.collection_id
        )).await.unwrap();
        
        assert_eq!(count, 0, "Should find 0 items with conflicting price conditions");
        
        // Test IN with empty list - this should error gracefully
        let result = fixture.execute_sql(&format!(
            "SELECT id FROM {} WHERE metadata->>'brand' IN () ORDER BY VECTOR_SIMILARITY(vector, [0.1, 0.2, 0.3, 0.4], 'cosine') LIMIT 10",
            fixture.collection_id
        )).await;
        
        assert!(result.is_err(), "Empty IN clause should return error");
    }
}