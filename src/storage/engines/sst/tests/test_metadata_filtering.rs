//! Test metadata filtering with unified SSTable reader
//!
//! This test module validates metadata filtering capabilities using
//! the CompositeBloomFilter implementation which supports both key
//! and metadata bloom filters.

use crate::core::search::SearchParams;
use crate::proto::proximadb_v1::{SqlValue, VectorRecord, sql_value};
use crate::storage::engines::sst::SstConfig;
use crate::storage::engines::sst::SstableWriter;
use crate::storage::engines::sst::readers::{CollectionContext, UnifiedSstableReader};
use crate::storage::persistence::filesystem::caching_filesystem::UnifiedCachingFilesystem;
use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
use proximadb_distance_kernel::DistanceMetric;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tempfile::TempDir;

fn create_test_config() -> SstConfig {
    SstConfig {
        block_size_kb: 4, // Use small 4KB blocks for tests
        decompression_cache_config: None,
        ..SstConfig::default()
    }
}
use serde_json::json;
use tracing::info;

// Helper function to compare SqlValue with JSON
fn sql_value_matches_json(sql_value: &SqlValue, json_value: &serde_json::Value) -> bool {
    match (&sql_value.value, json_value) {
        (Some(sql_value::Value::StringValue(s)), serde_json::Value::String(json_s)) => s == json_s,
        (Some(sql_value::Value::Int64Value(i)), serde_json::Value::Number(n)) => {
            *i == n.as_i64().unwrap_or(0)
        }
        (Some(sql_value::Value::NumberValue(f)), serde_json::Value::Number(n)) => {
            (*f - n.as_f64().unwrap_or(0.0)).abs() < f64::EPSILON
        }
        (Some(sql_value::Value::BoolValue(b)), serde_json::Value::Bool(json_b)) => b == json_b,
        _ => false,
    }
}

#[tokio::test]
async fn test_metadata_filtering_basic() {
    let _ = tracing_subscriber::fmt::try_init();

    // Create temp directory and filesystem
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path();
    let config = FilesystemConfig::default();
    let filesystem = Arc::new(FilesystemFactory::create(config).await.unwrap());

    // Create SSTable with records having different metadata
    let sstable_path = temp_path.join("metadata_test.sstable");
    let test_config = create_test_config();
    let block_size = (test_config.block_size_kb * 1024) as usize;
    let writer = SstableWriter::new(&sstable_path, block_size, filesystem.clone())
        .with_bloom_config(crate::core::bloom::BloomFilterConfig {
            strategy: crate::core::bloom::BloomStrategy::ByteAligned,
            bits_per_key: 10,
            false_positive_rate: Some(0.01),
            expected_items: 1000,
            enabled: true,
            hash_algorithm: crate::core::bloom::HashAlgorithm::XXHash,
        });

    // Create test records with various metadata
    let mut records = BTreeMap::new();

    // Category A records
    for i in 0..5 {
        let mut metadata = HashMap::new();
        metadata.insert(
            "category".to_string(),
            SqlValue {
                value: Some(sql_value::Value::StringValue("A".to_string())),
            },
        );
        metadata.insert(
            "score".to_string(),
            SqlValue {
                value: Some(sql_value::Value::NumberValue((i * 10) as f64)),
            },
        );
        metadata.insert(
            "type".to_string(),
            SqlValue {
                value: Some(sql_value::Value::StringValue("document".to_string())),
            },
        );

        let record = VectorRecord {
            id: format!("vec_a_{}", i),
            vector: vec![i as f32; 3],
            metadata,
            timestamp: Some(chrono::Utc::now().timestamp()),
            updated_at: Some(chrono::Utc::now().timestamp()),
            expires_at: None,
            version: Some(1),
            source: None,
        };
        records.insert(record.id.clone(), record);
    }

    // Category B records
    for i in 0..5 {
        let mut metadata = HashMap::new();
        metadata.insert(
            "category".to_string(),
            SqlValue {
                value: Some(sql_value::Value::StringValue("B".to_string())),
            },
        );
        metadata.insert(
            "score".to_string(),
            SqlValue {
                value: Some(sql_value::Value::NumberValue((i * 10 + 5) as f64)),
            },
        );
        metadata.insert(
            "type".to_string(),
            SqlValue {
                value: Some(sql_value::Value::StringValue("image".to_string())),
            },
        );

        let record = VectorRecord {
            id: format!("vec_b_{}", i),
            vector: vec![(i + 10) as f32; 3],
            metadata,
            timestamp: Some(chrono::Utc::now().timestamp()),
            updated_at: Some(chrono::Utc::now().timestamp()),
            expires_at: None,
            version: Some(1),
            source: None,
        };
        records.insert(record.id.clone(), record);
    }

    // Write records
    // Write records using streaming approach for production consistency
    let record_count = records.len();
    let sorted_records_iter = records.into_iter(); // BTreeMap already sorted by key
    writer
        .write_sorted_vector_records(sorted_records_iter, record_count)
        .await
        .unwrap();
    info!("Wrote SSTable with 10 records (5 category A, 5 category B)");

    // Create reader and load metadata
    let base_fs = filesystem.get_filesystem("file://").unwrap();
    let unified_fs = Arc::new(UnifiedCachingFilesystem::new(
        base_fs,
        "test_collection".to_string(),
        "sst".to_string(),
    ));
    let reader = UnifiedSstableReader::new(
        filesystem.clone(),
        unified_fs,
        "test_collection".to_string(),
    );
    let file_url = format!("file://{}", sstable_path.display());
    reader.load_metadata(&file_url).await.unwrap();

    // Create collection context
    let context = CollectionContext {
        file_path: file_url.clone(),
        sstable_files: vec![file_url.clone()],
        total_vectors: 10,
        metadata_columns: vec![
            "category".to_string(),
            "score".to_string(),
            "type".to_string(),
        ],
        level: 0,
        creation_time: chrono::Utc::now(),
        io_optimization_hints: None,
        collection: None,
    };

    // Test 1: Filter by category = A
    info!("\nTest 1: Filter by category = A");
    let mut filters = HashMap::new();
    filters.insert("category".to_string(), json!("A"));

    let search_params = SearchParams {
        query_vectors: Some(vec![vec![0.0, 0.0, 0.0]]),
        top_k: Some(10),
        distance_metric: Some(DistanceMetric::Euclidean),
        ..Default::default()
    }
    .with_simple_filters(filters);

    let results = reader
        .search_vectors(&search_params, &context)
        .await
        .unwrap();
    info!("Results with category=A filter: {}", results.len());
    assert_eq!(results.len(), 5, "Should find 5 records with category A");
    for result in &results {
        assert!(
            result.id.starts_with("vec_a_"),
            "All results should be category A"
        );
        let category = result.metadata.get("category").unwrap();
        assert!(
            sql_value_matches_json(category, &json!("A")),
            "Category should be A"
        );
    }

    // Test 2: Filter by type = image
    info!("\nTest 2: Filter by type = image");
    let mut filters = HashMap::new();
    filters.insert("type".to_string(), json!("image"));

    let search_params = SearchParams {
        query_vectors: Some(vec![vec![10.0, 10.0, 10.0]]),
        top_k: Some(10),
        distance_metric: Some(DistanceMetric::Euclidean),
        ..Default::default()
    }
    .with_simple_filters(filters);

    let results = reader
        .search_vectors(&search_params, &context)
        .await
        .unwrap();
    info!("Results with type=image filter: {}", results.len());
    assert_eq!(results.len(), 5, "Should find 5 records with type image");
    for result in &results {
        assert!(
            result.id.starts_with("vec_b_"),
            "All image results should be category B"
        );
        let type_val = result.metadata.get("type").unwrap();
        assert!(
            sql_value_matches_json(type_val, &json!("image")),
            "Type should be image"
        );
    }

    // Test 3: Filter by score > 25 (numeric filter)
    info!("\nTest 3: Numeric filter - score > 25");
    // Note: For now we'll test exact match. Range queries need filter expressions
    let mut filters = HashMap::new();
    filters.insert("score".to_string(), json!(30)); // vec_a_3 has score 30

    let search_params = SearchParams {
        query_vectors: Some(vec![vec![3.0, 3.0, 3.0]]),
        top_k: Some(10),
        distance_metric: Some(DistanceMetric::Euclidean),
        ..Default::default()
    }
    .with_simple_filters(filters);

    let results = reader
        .search_vectors(&search_params, &context)
        .await
        .unwrap();
    info!("Results with score=30 filter: {}", results.len());
    assert_eq!(results.len(), 1, "Should find 1 record with score 30");
    assert_eq!(results[0].id, "vec_a_3", "Should be vec_a_3");

    // Test 4: Multiple filters (category = B AND type = image)
    info!("\nTest 4: Multiple filters - category=B AND type=image");
    let mut filters = HashMap::new();
    filters.insert("category".to_string(), json!("B"));
    filters.insert("type".to_string(), json!("image"));

    let search_params = SearchParams {
        query_vectors: Some(vec![vec![12.0, 12.0, 12.0]]),
        top_k: Some(10),
        distance_metric: Some(DistanceMetric::Euclidean),
        ..Default::default()
    }
    .with_simple_filters(filters);

    let results = reader
        .search_vectors(&search_params, &context)
        .await
        .unwrap();
    info!("Results with category=B AND type=image: {}", results.len());
    assert_eq!(
        results.len(),
        5,
        "Should find 5 records matching both filters"
    );
    for result in &results {
        let category = result.metadata.get("category").unwrap();
        let type_val = result.metadata.get("type").unwrap();
        assert!(
            sql_value_matches_json(category, &json!("B")),
            "Category should be B"
        );
        assert!(
            sql_value_matches_json(type_val, &json!("image")),
            "Type should be image"
        );
    }

    // Test 5: No results filter
    info!("\nTest 5: Filter that matches no records");
    let mut filters = HashMap::new();
    filters.insert("category".to_string(), json!("Z")); // Doesn't exist

    let search_params = SearchParams {
        query_vectors: Some(vec![vec![0.0, 0.0, 0.0]]),
        top_k: Some(10),
        distance_metric: Some(DistanceMetric::Euclidean),
        ..Default::default()
    }
    .with_simple_filters(filters);

    let results = reader
        .search_vectors(&search_params, &context)
        .await
        .unwrap();
    info!("Results with category=Z filter: {}", results.len());
    assert_eq!(results.len(), 0, "Should find no records with category Z");
}

#[tokio::test]
async fn test_metadata_bloom_filter_optimization() {
    let _ = tracing_subscriber::fmt::try_init();

    // Create temp directory and filesystem
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path();
    let config = FilesystemConfig::default();
    let filesystem = Arc::new(FilesystemFactory::create(config).await.unwrap());

    // Create SSTable with metadata bloom filters
    let sstable_path = temp_path.join("bloom_metadata_test.sstable");
    let test_config = create_test_config();
    let block_size = (test_config.block_size_kb * 1024) as usize;
    let writer = SstableWriter::new(&sstable_path, block_size, filesystem.clone())
        .with_bloom_config(crate::core::bloom::BloomFilterConfig {
            strategy: crate::core::bloom::BloomStrategy::ByteAligned,
            bits_per_key: 10,
            false_positive_rate: Some(0.01),
            expected_items: 1000,
            enabled: true,
            hash_algorithm: crate::core::bloom::HashAlgorithm::XXHash,
        });

    // Create records
    let mut records = BTreeMap::new();
    for i in 0..20 {
        let mut metadata = HashMap::new();
        metadata.insert(
            "category".to_string(),
            SqlValue {
                value: Some(sql_value::Value::StringValue(
                    if i % 2 == 0 { "even" } else { "odd" }.to_string(),
                )),
            },
        );
        metadata.insert(
            "status".to_string(),
            SqlValue {
                value: Some(sql_value::Value::StringValue(
                    if i < 10 { "active" } else { "inactive" }.to_string(),
                )),
            },
        );

        let record = VectorRecord {
            id: format!("vec_{}", i),
            vector: vec![i as f32; 3],
            metadata,
            timestamp: Some(chrono::Utc::now().timestamp()),
            updated_at: Some(chrono::Utc::now().timestamp()),
            expires_at: None,
            version: Some(1),
            source: None,
        };
        records.insert(record.id.clone(), record);
    }

    // Write records using streaming approach for production consistency
    let record_count = records.len();
    let sorted_records_iter = records.into_iter(); // BTreeMap already sorted by key
    writer
        .write_sorted_vector_records(sorted_records_iter, record_count)
        .await
        .unwrap();
    info!("Wrote SSTable with metadata bloom filters");

    // Create reader
    let base_fs = filesystem.get_filesystem("file://").unwrap();
    let unified_fs = Arc::new(UnifiedCachingFilesystem::new(
        base_fs,
        "test_collection".to_string(),
        "sst".to_string(),
    ));
    let reader = UnifiedSstableReader::new(
        filesystem.clone(),
        unified_fs,
        "test_collection".to_string(),
    );
    let file_url = format!("file://{}", sstable_path.display());
    reader.load_metadata(&file_url).await.unwrap();

    // Test bloom filter for metadata values
    info!("\nTesting metadata bloom filter functionality");

    // Create collection context
    let context = CollectionContext {
        file_path: file_url.clone(),
        sstable_files: vec![file_url.clone()],
        total_vectors: 20,
        metadata_columns: vec!["category".to_string(), "status".to_string()],
        level: 0,
        creation_time: chrono::Utc::now(),
        io_optimization_hints: None,
        collection: None,
    };

    // Test 1: Filter by category = even (10 records)
    info!("\nTest 1: Filter by category = even");
    let mut filters = HashMap::new();
    filters.insert("category".to_string(), json!("even"));

    let search_params = SearchParams {
        query_vectors: Some(vec![vec![10.0, 10.0, 10.0]]),
        top_k: Some(20),
        distance_metric: Some(DistanceMetric::Euclidean),
        ..Default::default()
    }
    .with_simple_filters(filters);

    let results = reader
        .search_vectors(&search_params, &context)
        .await
        .unwrap();
    info!("Results with category=even filter: {}", results.len());
    assert_eq!(
        results.len(),
        10,
        "Should find 10 records with category even"
    );

    // Test 2: Filter by status = active (10 records)
    info!("\nTest 2: Filter by status = active");
    let mut filters = HashMap::new();
    filters.insert("status".to_string(), json!("active"));

    let search_params = SearchParams {
        query_vectors: Some(vec![vec![5.0, 5.0, 5.0]]),
        top_k: Some(20),
        distance_metric: Some(DistanceMetric::Euclidean),
        ..Default::default()
    }
    .with_simple_filters(filters);

    let results = reader
        .search_vectors(&search_params, &context)
        .await
        .unwrap();
    info!("Results with status=active filter: {}", results.len());
    assert_eq!(
        results.len(),
        10,
        "Should find 10 records with status active"
    );

    // Test 3: Combined filters (category = odd AND status = inactive)
    info!("\nTest 3: Combined filters - category=odd AND status=inactive");
    let mut filters = HashMap::new();
    filters.insert("category".to_string(), json!("odd"));
    filters.insert("status".to_string(), json!("inactive"));

    let search_params = SearchParams {
        query_vectors: Some(vec![vec![15.0, 15.0, 15.0]]),
        top_k: Some(20),
        distance_metric: Some(DistanceMetric::Euclidean),
        ..Default::default()
    }
    .with_simple_filters(filters);

    let results = reader
        .search_vectors(&search_params, &context)
        .await
        .unwrap();
    info!(
        "Results with category=odd AND status=inactive: {}",
        results.len()
    );
    assert_eq!(
        results.len(),
        5,
        "Should find 5 records matching both filters"
    );

    info!("✅ Metadata bloom filter tests completed successfully");
}
