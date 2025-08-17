/// Integration test for SST SSTable writer and reader
/// Tests the full cycle of writing and reading SSTable files

use std::sync::Arc;
use tracing::{debug, error, info, warn};
use std::collections::HashMap;
use tempfile::TempDir;
use proximadb::core::{VectorRecord, MetadataItem};
use proximadb::storage::engines::sst::sstable_writer::SstableWriter;
use proximadb::storage::engines::sst::readers::unified_sstable_reader::{
    UnifiedSstableReader, CollectionContext, ReaderConfig
};
use proximadb::storage::engines::sst::SstRecord;
use proximadb::core::bloom::BloomFilterConfig;
use proximadb::storage::persistence::filesystem::FilesystemFactory;
use proximadb::compute::distance_computation::DistanceMetric;
use proximadb::core::search::{SearchParams};
use anyhow::Result;

/// Test SSTable write and read integration
#[tokio::test]
async fn test_sstable_write_read_integration() -> Result<()> {
    // Create temp directory
    let temp_dir = TempDir::new()?;
    let base_path = temp_dir.path().to_str().unwrap();
    
    // Create filesystem
    let filesystem = Arc::new(FilesystemFactory::new(HashMap::new()));
    
    // Create test vectors
    let vectors = vec![
        VectorRecord {
            id: Some("vec1".to_string())),
            vector: vec![1.0, 0.0, 0.0],
            metadata: vec![
                MetadataItem {
                    key: "category".to_string(),
                    value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue("A".to_string())),
            timestamp: 0,
            updated_at: None,
            expires_at: None,
            distance: None,
            rank: None,
            score: None,
        }
            ],
            ..Default::default()
        },
        VectorRecord {
            id: Some("vec2".to_string())),
            vector: vec![0.0, 1.0, 0.0],
            metadata: vec![
                MetadataItem {
                    key: "category".to_string(),
                    value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue("B".to_string())),
            timestamp: 0,
            updated_at: None,
            expires_at: None,
            distance: None,
            rank: None,
            score: None,
        }
            ],
            ..Default::default()
        },
        VectorRecord {
            id: Some("vec3".to_string())),
            vector: vec![0.0, 0.0, 1.0],
            metadata: vec![
                MetadataItem {
                    key: "category".to_string(),
                    value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue("A".to_string())),
            timestamp: 0,
            updated_at: None,
            expires_at: None,
            distance: None,
            rank: None,
            score: None,
        }
            ],
            ..Default::default()
        },
    ];
    
    // Write SSTable
    let sst_path = format!("file://{}/test.sstable", base_path);
    let writer = SstableWriter::new(&sst_path, 4096, filesystem.clone())
        .with_bloom_config(BloomFilterConfig {
            false_positive_rate: 0.01,
            min_elements: 100,
        });
    
    // Convert to SST records
    let mut entries = std::collections::BTreeMap::new();
    for (i, vec) in vectors.iter().enumerate() {
        let mut lsm_record = SstRecord::from_vector_record(vec.clone(), "test_collection");
        lsm_record.sequence_number = i as u64;
        lsm_0 /* TODO: VectorRecord no longer has level field */ = 0;
        entries.insert(vec.id.as_ref().unwrap().clone(), lsm_record);
    }
    
    // Write records using streaming approach for production consistency
    let record_count = entries.len();
    let sorted_records_iter = entries.into_iter(); // BTreeMap already sorted by key
    writer.write_sorted_records(sorted_records_iter, record_count).await?;
    debug!("Wrote {} vectors to SSTable: {}", vectors.len(), sst_path);
    
    // Create reader
    let reader = UnifiedSstableReader::new(filesystem.clone());
    
    // Load metadata
    reader.load_metadata(&sst_path).await?;
    debug!("Loaded SSTable metadata");
    
    // Test bloom filter
    let might_contain_vec1 = reader.might_contain_key(&sst_path, "vec1").await;
    let might_contain_vec4 = reader.might_contain_key(&sst_path, "vec4").await;
    debug!("Bloom filter test - vec1: {}, vec4 (shouldn't exist): {}", 
             might_contain_vec1, might_contain_vec4);
    
    assert!(might_contain_vec1, "Bloom filter should indicate vec1 might exist");
    
    // Create collection context
    let context = CollectionContext {
        collection_id: "test_collection".to_string(),
        file_path: sst_path.clone(),
        sstable_files: vec![sst_path.clone()],
        total_vectors: 0,
        metadata_columns: vec!["category".to_string()],
        level: 0,
        creation_time: chrono::Utc::now(),
    };
    
    // Test search
    let search_params = SearchParams {
        query_vectors: Some(vec![vec![1.0, 0.0, 0.0]]), // Close to vec1
        top_k: Some(10),
        distance_metric: Some(DistanceMetric::Cosine),
        filters: None,
        ..Default::default()
    };
    
    let results = reader.search_vectors(&search_params, &context).await?;
    debug!("Search returned {} results", results.len());
    
    // Verify results
    assert!(!results.is_empty(), "Should find results");
    assert_eq!(results[0].id, "vec1", "First result should be vec1");
    
    // Test with metadata filter
    let mut filters = HashMap::new();
    filters.insert("category".to_string(), serde_json::Value::String("A".to_string()));
    
    let filtered_params = SearchParams {
        query_vectors: Some(vec![vec![0.0, 1.0, 0.0]]), // Close to vec2 (category B)
        top_k: Some(10),
        distance_metric: Some(DistanceMetric::Cosine),
        filters: Some(filters),
        ..Default::default()
    };
    
    let filtered_results = reader.search_vectors(&filtered_params, &context).await?;
    debug!("Filtered search (category=A) returned {} results", filtered_results.len());
    
    // Should return vec1 and vec3 (category A), not vec2 (category B)
    assert_eq!(filtered_results.len(), 2, "Should find 2 results with category A");
    assert!(filtered_results.iter().all(|r| r.id != "vec2"), "Should not include vec2");
    
    Ok(())
}

/// Test empty SSTable handling
#[tokio::test]
async fn test_empty_sstable() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let base_path = temp_dir.path().to_str().unwrap();
    let filesystem = Arc::new(FilesystemFactory::new(HashMap::new()));
    
    // Write empty SSTable
    let sst_path = format!("file://{}/empty.sstable", base_path);
    let writer = SstableWriter::new(&sst_path, 4096, filesystem.clone());
    let entries = std::collections::BTreeMap::new();
    // Write records using streaming approach for production consistency
    let record_count = entries.len();
    let sorted_records_iter = entries.into_iter(); // BTreeMap already sorted by key
    writer.write_sorted_records(sorted_records_iter, record_count).await?;
    
    // Create reader
    let reader = UnifiedSstableReader::new(filesystem.clone());
    reader.load_metadata(&sst_path).await?;
    
    // Search should return empty results
    let context = CollectionContext {
        collection_id: "test_collection".to_string(),
        file_path: sst_path.clone(),
        sstable_files: vec![sst_path.clone()],
        total_vectors: 0,
        metadata_columns: vec![],
        level: 0,
        creation_time: chrono::Utc::now(),
    };
    
    let search_params = SearchParams {
        query_vectors: Some(vec![vec![1.0, 0.0, 0.0]]),
        top_k: Some(10),
        distance_metric: Some(DistanceMetric::Cosine),
        ..Default::default()
    };
    
    let results = reader.search_vectors(&search_params, &context).await?;
    assert_eq!(results.len(), 0, "Empty SSTable should return no results");
    
    Ok(())
}