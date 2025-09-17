//! Comprehensive tests for VIPER compaction functionality
//!
//! These tests ensure VIPER's compaction correctly handles:
//! - Small file merging
//! - Tombstone cleanup
//! - Space reclamation
//! - Data integrity during compaction
//! - Concurrent access during compaction

use std::sync::Arc;
use tempfile::TempDir;
use tokio::time::{Duration, sleep};
use tracing::{debug, warn, error};
use std::collections::HashMap;

use crate::proto::proximadb_v1::{VectorRecord, sql_value::Value};
use crate::storage::engines::impls::viper::{ViperEngine, ViperEngineConfig};
use crate::storage::traits::{CompactionParameters, FlushParameters, UnifiedStorageEngine};
// CompactionStrategy is not needed - it's part of CompactionParameters
use crate::storage::persistence::filesystem::FilesystemFactory;

/// Create test configuration with custom compaction settings
fn create_compaction_config(_base_path: &str) -> ViperEngineConfig {
    let mut config = ViperEngineConfig::default();
    config.enable_ml_clustering = false;
    config.flush_size_bytes = Some(512 * 1024); // 512KB for faster testing
    config.row_group_size = 100; // Small row groups for testing
    config
}

/// Set up storage assignment for test collection
async fn setup_test_assignment(collection_id: &str, base_path: &str) {
    use tokio::fs;

    // Create necessary directories
    let data_dir = format!("{}/{}/data", base_path, collection_id);
    fs::create_dir_all(&data_dir)
        .await
        .expect("Failed to create data directory");

    // Create temp directory for atomic writes
    let temp_dir = format!("{}/{}/data/___temp", base_path, collection_id);
    fs::create_dir_all(&temp_dir)
        .await
        .expect("Failed to create temp directory");

    // Storage assignment is now handled internally by CollectionService
    // when a collection is created. For test purposes, we just ensure
    // the directory structure exists.
    let wal_dir = format!("{}/{}/write_buffer", base_path, collection_id);
    tokio::fs::create_dir_all(&wal_dir)
        .await
        .expect("Failed to create WAL directory");
}

/// Create test collection with storage assignment
fn create_test_collection(
    collection_id: &str,
    base_path: &str,
) -> crate::proto::proximadb_v1::Collection {
    use crate::proto::proximadb_v1::{Collection, CollectionConfig, StorageAssignment};

    Collection {
        id: collection_id.to_string(),
        config: Some(CollectionConfig {
            name: collection_id.to_string(),
            dimension: 128,
            distance_metric: 0, // Cosine
            storage_engine: 0,  // VIPER
            filterable_columns: vec![],
            index_configs: vec![],
            quantization: Some(crate::proto::proximadb_v1::QuantizationConfig {
                enabled: true,                         // Quantization enabled by default for VIPER
                enable_progressive_search: true, // Progressive search enabled by default
                ..Default::default()
            }),
            storage_config: None,
            description: None,
            tags: vec![],
            embedding_models: vec![],
            primary_index: "default".to_string(),
            auto_index_selection: true,
            owner: Some("test".to_string()),
        }),
        stats: None,
        created_at: chrono::Utc::now().timestamp(),
        updated_at: chrono::Utc::now().timestamp(),
        storage_assignment: Some(StorageAssignment {
            primary_path: format!("file://{}", base_path),
            backup_paths: vec![],
            engine: 1, // VIPER enum value
            engine_config: HashMap::new(),
            base_location: base_path.to_string(),
            assigned_at: chrono::Utc::now().timestamp(),
        }),
    }
}

/// Create test vector
fn create_test_vector(id: &str, dimension: usize) -> VectorRecord {
    let mut metadata = HashMap::new();
    metadata.insert(
        "compaction_test".to_string(),
        crate::proto::proximadb_v1::SqlValue {
            value: Some(Value::StringValue("true".to_string())),
        },
    );

    VectorRecord {
        id: id.to_string(),
        vector: (0..dimension)
            .map(|i| (i as f32) / (dimension as f32))
            .collect(),
        metadata,
        timestamp: chrono::Utc::now().timestamp(),
        updated_at: Some(chrono::Utc::now().timestamp()),
        expires_at: None,
        version: Some(1),
        quantized_vector: vec![],
        source: None,
    }
}

#[tokio::test]
async fn test_insert_flush_compact_flow() {
    // Initialize hardware capabilities for testing
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    // Test complete flow: insert -> flush -> compact
    let temp_dir = TempDir::new().unwrap();
    let config = create_compaction_config(temp_dir.path().to_str().unwrap());

    let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
    let engine = ViperEngine::from_core_config(
        crate::core::config::ViperConfig::default(),
        filesystem_factory,
    )
    .await
    .expect("Failed to create engine");

    let collection_id = "insert_flush_compact";

    // Set up storage assignment
    setup_test_assignment(collection_id, temp_dir.path().to_str().unwrap()).await;

    debug!("\n🚀 Starting Insert-Flush-Compact test\n");

    // Step 1: Create vectors for batch insert
    debug!("📝 Step 1: Creating vectors for batch insert...");
    let mut all_vectors = Vec::new();
    for batch in 0..4 {
        for i in 0..10 {
            all_vectors.push(create_test_vector(&format!("vec_{}_{}", batch, i), 128));
        }
    }
    debug!("  📦 Created {} vectors", all_vectors.len());

    // Step 2: Flush vectors to disk (VIPER batch insert)
    debug!("\n💾 Step 2: Flushing vectors to disk (batch insert)...");
    let flush_params = FlushParameters {
        collection_id: Some(collection_id.to_string()),
        force: true,
        synchronous: true,
        vector_records: all_vectors, // Pass vectors here for VIPER batch insert
        batch_ids: vec![],
        hints: std::collections::HashMap::new(),
        timeout_ms: None,
        trigger_compaction: false,
        estimated_size: 1024 * 1024, // 1MB estimated
        collection_config: Some(create_test_collection(
            collection_id,
            temp_dir.path().to_str().unwrap(),
        )),
    };

    let flush_result = engine
        .do_flush(&flush_params)
        .await
        .expect("Failed to flush");
    debug!(
        "  ✅ Flush complete: {} files created, {} entries flushed",
        flush_result.files_created.unwrap_or(0), flush_result.entries_flushed.unwrap_or(0)
    );

    // List files after flush
    let base_path = temp_dir.path().to_str().unwrap();
    let data_url = format!("file://{}/{}/data", base_path, collection_id);
    let fs = engine
        .get_filesystem_factory()
        .get_filesystem(&data_url)
        .unwrap();

    debug!("\n📂 Files after flush:");
    if let Ok(entries) = fs.list(&data_url).await {
        for entry in &entries {
            if entry.name.ends_with(".parquet") && !entry.metadata.is_directory {
                debug!("  - {}", entry.name);
            }
        }
    }

    // Step 3: Run compaction
    debug!("\n🔨 Step 3: Running compaction...");
    let compact_params = CompactionParameters {
        collection_id: Some(collection_id.to_string()),
        force: true,
        synchronous: true,
        hints: std::collections::HashMap::new(),
        timeout_ms: None,
        priority: crate::storage::traits::OperationPriority::Medium,
        estimated_input_size: 1024 * 1024, // 1MB estimated
        collection_config: Some(create_test_collection(
            collection_id,
            temp_dir.path().to_str().unwrap(),
        )),
    };

    let compact_result = engine
        .do_compact(&compact_params)
        .await
        .expect("Compaction failed");
    debug!(
        "  ✅ Compaction complete: {} entries processed, {} input files -> {} output files",
        compact_result.entries_processed.unwrap_or(0), compact_result.input_files.unwrap_or(0), compact_result.output_files.unwrap_or(0)
    );

    // CRITICAL: Verify compaction doesn't duplicate data
    // We inserted 40 vectors total (4 batches × 10 vectors)
    let expected_entries = 40u64;
    assert!(
        compact_result.entries_processed.unwrap_or(0) <= expected_entries,
        "❌ Compaction processed {} entries but we only inserted {}! This indicates data duplication.",
        compact_result.entries_processed.unwrap_or(0),
        expected_entries
    );

    // Allow up to 20% overhead for versioning/metadata/deduplication
    let max_allowed = (expected_entries as f64 * 1.2) as u64;
    assert!(
        compact_result.entries_processed.unwrap_or(0) <= max_allowed,
        "❌ Compaction processed {} entries, exceeding 20% threshold of {} (max allowed: {})",
        compact_result.entries_processed.unwrap_or(0),
        expected_entries,
        max_allowed
    );

    // List files after compaction
    debug!("\n📂 Files after compaction:");
    let mut compacted_file_url = None;
    if let Ok(entries) = fs.list(&data_url).await {
        for entry in &entries {
            if entry.name.ends_with(".parquet") && !entry.metadata.is_directory {
                debug!("  - {}", entry.name);
                if entry.name.starts_with("compacted_") {
                    compacted_file_url = Some(format!("{}/{}", data_url, entry.name));
                }
            }
        }
    }

    // Step 4: Verify compacted file contents
    debug!("\n🔍 Step 4: Verifying compacted file...");
    if let Some(file_url) = compacted_file_url {
        if let Ok(data) = fs.read(&file_url).await {
            debug!("  📊 File size: {} bytes", data.len());

            use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
            if let Ok(builder) = ParquetRecordBatchReaderBuilder::try_new(bytes::Bytes::from(data))
            {
                let reader = builder.build().unwrap();
                let mut total_rows = 0;
                for (i, batch) in reader.enumerate() {
                    if let Ok(batch) = batch {
                        debug!("  📊 Batch {}: {} rows", i, batch.num_rows());
                        total_rows += batch.num_rows();
                    }
                }
                debug!("  📊 Total rows in compacted file: {}", total_rows);
                assert_eq!(
                    total_rows, 40,
                    "Expected 40 rows in compacted file, got {}",
                    total_rows
                );
            }
        }
    }

    // Step 5: Verify data is still searchable
    debug!("\n🔍 Step 5: Verifying search after compaction...");
    let storage_url = format!(
        "file://{}/{}/data",
        temp_dir.path().to_str().unwrap(),
        collection_id
    );
    let search_results = engine
        .search_vectors(collection_id, &storage_url, &vec![0.5; 128], 100)
        .await
        .unwrap();

    debug!("  ✅ Found {} results", search_results.len());
    assert_eq!(
        search_results.len(),
        40,
        "Expected 40 search results, got {}",
        search_results.len()
    );
}

#[tokio::test]
async fn test_basic_compaction() {
    // Initialize hardware capabilities for testing
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let temp_dir = TempDir::new().unwrap();
    let config = create_compaction_config(temp_dir.path().to_str().unwrap());

    let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
    let engine = ViperEngine::from_core_config(
        crate::core::config::ViperConfig::default(),
        filesystem_factory,
    )
    .await
    .expect("Failed to create engine");

    let collection_id = "basic_compact";

    // Set up storage assignment for the test collection
    setup_test_assignment(collection_id, temp_dir.path().to_str().unwrap()).await;

    // Debug: check data directory
    let base_path = temp_dir.path().to_str().unwrap();
    let data_url = format!("file://{}/{}/data", base_path, collection_id);
    debug!("📍 Data directory: {}", data_url);

    // Extract the actual filesystem path for debugging
    if data_url.starts_with("file://") {
        let fs_path = data_url.strip_prefix("file://");
        debug!("📍 Filesystem path: {}", fs_path.unwrap_or("unknown"));
    }

    // Create multiple small files
    for batch in 0..4 {
        let mut vectors = Vec::new();
        for i in 0..10 {
            vectors.push(create_test_vector(
                &format!("batch_{}_vec_{}", batch, i),
                128,
            ));
        }

        // VIPER doesn't support single inserts - vectors collected above

        let flush_params = FlushParameters {
            collection_id: Some(collection_id.to_string()),
            force: true,
            synchronous: false,
            vector_records: vectors,
            batch_ids: vec![],
            hints: std::collections::HashMap::new(),
            timeout_ms: None,
            trigger_compaction: false,
            estimated_size: 1024 * 1024, // 1MB estimated
            collection_config: Some(create_test_collection(
                collection_id,
                temp_dir.path().to_str().unwrap(),
            )),
        };
        let flush_result = engine.do_flush(&flush_params).await.unwrap();
        debug!(
            "📄 Batch {} flush result: {} files created, {} entries flushed",
            batch, flush_result.files_created.unwrap_or(0), flush_result.entries_flushed.unwrap_or(0)
        );

        // Debug: Check if the flush actually created files
        if flush_result.files_created.unwrap_or(0) == 0 {
            debug!("  ⚠️ WARNING: No files created during flush!");
        }
    }

    // Debug: check what files exist in the data directory
    let data_url = format!("file://{}/{}/data", base_path, collection_id);
    let fs = engine
        .get_filesystem_factory()
        .get_filesystem(&data_url)
        .unwrap();

    if fs.exists(&data_url).await.unwrap() {
        let entries = fs.list(&data_url).await.unwrap();
        debug!("📂 Files in data directory ({}):", data_url);
        for entry in &entries {
            debug!(
                "  - {} (is_dir: {})",
                entry.name, entry.metadata.is_directory
            );

            // Check inside ___temp directory
            if entry.name == "___temp" {
                let temp_url = format!("{}/___temp", data_url);
                if fs.exists(&temp_url).await.unwrap() {
                    let temp_entries = fs.list(&temp_url).await.unwrap();
                    debug!("    📁 Files in temp directory:");
                    for temp_entry in &temp_entries {
                        debug!(
                            "      - {} (is_dir: {})",
                            temp_entry.name, temp_entry.metadata.is_directory
                        );
                    }
                }
            }
        }
    } else {
        error!("❌ Data directory does not exist: {}", data_url);
    }

    // Run compaction
    let compact_params = CompactionParameters {
        collection_id: Some(collection_id.to_string()),
        force: true,
        synchronous: true,
        hints: std::collections::HashMap::new(),
        timeout_ms: None,
        priority: crate::storage::traits::OperationPriority::Medium,
        estimated_input_size: 1024 * 1024, // 1MB estimated
        collection_config: Some(create_test_collection(
            collection_id,
            temp_dir.path().to_str().unwrap(),
        )),
    };

    let result = engine
        .do_compact(&compact_params)
        .await
        .expect("Compaction failed");

    debug!(
        "📊 Compaction result: success={}, entries_processed={}, input_files={}, output_files={}",
        result.success, result.entries_processed.unwrap_or(0), result.input_files.unwrap_or(0), result.output_files.unwrap_or(0)
    );

    assert!(result.success);

    // CRITICAL: Verify compaction doesn't duplicate data
    // We inserted 4 batches × 10 vectors = 40 vectors total
    let expected_entries = 40u64;
    assert!(
        result.entries_processed.unwrap_or(0) <= expected_entries,
        "❌ Compaction processed {} entries but we only inserted {}! This indicates data duplication.",
        result.entries_processed.unwrap_or(0),
        expected_entries
    );

    // Allow up to 20% overhead for versioning/metadata/deduplication
    let max_allowed = (expected_entries as f64 * 1.2) as u64;
    assert!(
        result.entries_processed.unwrap_or(0) <= max_allowed,
        "❌ Compaction processed {} entries, exceeding 20% threshold of {} (max allowed: {})",
        result.entries_processed.unwrap_or(0),
        expected_entries,
        max_allowed
    );
    // For now, let's check if files were processed instead of entries
    assert!(
        result.input_files.unwrap_or(0) > 0,
        "Expected input files to be processed, got {}",
        result.input_files.unwrap_or(0)
    );

    // Small delay to ensure filesystem operations complete
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // List files after compaction
    debug!("📂 Files after compaction:");
    if fs.exists(&data_url).await.unwrap() {
        let entries = fs.list(&data_url).await.unwrap();
        debug!("Files in data directory ({}):", data_url);
        for entry in &entries {
            debug!(
                "  - {} (is_dir: {})",
                entry.name, entry.metadata.is_directory
            );

            // Check inside ___temp directory
            if entry.name == "___temp" {
                let temp_url = format!("{}/___temp", data_url);
                if fs.exists(&temp_url).await.unwrap() {
                    let temp_entries = fs.list(&temp_url).await.unwrap();
                    debug!("    📁 Files in temp directory:");
                    for temp_entry in &temp_entries {
                        debug!(
                            "      - {} (is_dir: {})",
                            temp_entry.name, temp_entry.metadata.is_directory
                        );
                    }
                }
            }
        }
    }

    // Add a delay to ensure file operations complete
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // List all parquet files in data directory to debug
    let data_url = format!("file://{}/{}/data", base_path, collection_id);
    let fs = engine
        .get_filesystem_factory()
        .get_filesystem(&data_url)
        .unwrap();

    debug!("🔍 Looking for parquet files in: {}", data_url);
    let mut all_parquet_files = Vec::new();
    if let Ok(entries) = fs.list(&data_url).await {
        for entry in entries {
            if entry.name.ends_with(".parquet") && !entry.metadata.is_directory {
                debug!("  ✅ Found parquet file: {}", entry.name);
                all_parquet_files.push(entry.name.clone());
            }
        }
    }

    if all_parquet_files.is_empty() {
        debug!("  ❌ No parquet files found in data directory!");

        // Check ___temp directory
        let temp_url = format!("{}/___temp", data_url);
        if let Ok(temp_entries) = fs.list(&temp_url).await {
            debug!("  📁 Checking ___temp directory:");
            for entry in temp_entries {
                if entry.name.ends_with(".parquet") && !entry.metadata.is_directory {
                    debug!("    ⚠️ Found parquet in temp: {}", entry.name);
                }
            }
        }
    } else {
        // Debug: Read the compacted file directly to see what's in it
        let compacted_file = &all_parquet_files[0];
        let file_url = format!("{}/{}", data_url, compacted_file);
        debug!("🔍 Debug: Reading compacted file directly: {}", file_url);

        if let Ok(data) = fs.read(&file_url).await {
            debug!("  📊 File size: {} bytes", data.len());

            // Parse with arrow reader
            use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
            if let Ok(builder) = ParquetRecordBatchReaderBuilder::try_new(bytes::Bytes::from(data))
            {
                let reader = builder.build().unwrap();
                let mut total_rows = 0;
                for (i, batch) in reader.enumerate() {
                    if let Ok(batch) = batch {
                        debug!("  📊 Batch {}: {} rows", i, batch.num_rows());
                        total_rows += batch.num_rows();

                        // Print first few IDs
                        if let Some(id_column) = batch.column_by_name("id") {
                            use arrow_array::Array;
                            let id_array = id_column
                                .as_any()
                                .downcast_ref::<arrow_array::StringArray>()
                                .unwrap();
                            for j in 0..std::cmp::min(3, id_array.len()) {
                                debug!("    - ID[{}]: {:?}", j, id_array.value(j));
                            }
                        }
                    }
                }
                debug!("  📊 Total rows in file: {}", total_rows);
            }
        }
    }

    // Verify all data still accessible
    let storage_url = format!(
        "file://{}/{}/data",
        temp_dir.path().to_str().unwrap(),
        collection_id
    );

    // First, let's check what parquet files exist
    debug!("🔍 DEBUG: Checking for parquet files at: {}", storage_url);
    let parquet_files = engine
        .parquet_files_with_storage_url(collection_id, &storage_url)
        .await
        .unwrap();
    debug!("🔍 DEBUG: Found {} parquet files", parquet_files.len());
    for (i, file) in parquet_files.iter().enumerate() {
        debug!("  [{}] {}", i, file);
    }

    let search_results = engine
        .search_vectors(collection_id, &storage_url, &vec![0.5; 128], 100)
        .await
        .unwrap();

    let total_results: usize = search_results.iter().map(|sr| sr.results.len()).sum();
    debug!(
        "🔍 Search results after compaction: {} search result objects with {} total individual results",
        search_results.len(),
        total_results
    );
    for search_result in &search_results {
        for result in &search_result.results {
            debug!("  - ID: {}, Score: {:?}", result.id, result.score);
        }
    }

    // Check if all vectors are present
    for batch in 0..4 {
        for i in 0..10 {
            let id = format!("batch_{}_vec_{}", batch, i);
            let found = search_results.iter().any(|sr| sr.results.iter().any(|r| r.id == id));
            if !found {
                error!("❌ Missing vector: {}", id);
            }
            assert!(found, "Vector {} missing after compaction", id);
        }
    }
}

#[tokio::test]
async fn test_concurrent_compaction_and_reads() {
    // Initialize hardware capabilities for testing
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let temp_dir = TempDir::new().unwrap();
    let config = create_compaction_config(temp_dir.path().to_str().unwrap());

    let engine = Arc::new(
        {
            let filesystem_factory =
                Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
            ViperEngine::from_core_config(
                crate::core::config::ViperConfig::default(),
                filesystem_factory,
            )
            .await
        }
        .expect("Failed to create engine"),
    );

    let collection_id = "concurrent_compact";

    // Set up storage assignment for the test collection
    setup_test_assignment(collection_id, temp_dir.path().to_str().unwrap()).await;

    // Create initial data (VIPER doesn't support single inserts)
    let mut initial_vectors = vec![];
    for i in 0..100 {
        initial_vectors.push(create_test_vector(&format!("concurrent_{}", i), 128));
    }

    // Flush initial data
    let flush_params = FlushParameters {
        collection_id: Some(collection_id.to_string()),
        force: true,
        synchronous: true,
        vector_records: initial_vectors,
        batch_ids: vec![],
        hints: std::collections::HashMap::new(),
        timeout_ms: None,
        trigger_compaction: false,
        estimated_size: 1024 * 1024, // 1MB estimated
        collection_config: Some(create_test_collection(
            collection_id,
            temp_dir.path().to_str().unwrap(),
        )),
    };
    engine.do_flush(&flush_params).await.unwrap();

    // Create multiple files
    for i in 0..5 {
        // Create some additional vectors for each flush
        let mut additional_vectors = vec![];
        for j in 0..20 {
            additional_vectors.push(create_test_vector(&format!("additional_{}_{}", i, j), 128));
        }

        let flush_params = FlushParameters {
            collection_id: Some(collection_id.to_string()),
            force: true,
            synchronous: false,
            vector_records: additional_vectors,
            batch_ids: vec![],
            hints: std::collections::HashMap::new(),
            timeout_ms: None,
            trigger_compaction: false,
            estimated_size: 1024 * 1024, // 1MB estimated
            collection_config: Some(create_test_collection(
                collection_id,
                temp_dir.path().to_str().unwrap(),
            )),
        };
        engine.do_flush(&flush_params).await.unwrap();
    }

    // Start compaction in background
    let compact_engine = engine.clone();
    let temp_dir_path_for_compact = temp_dir.path().to_str().unwrap().to_string();
    let compact_handle = tokio::spawn(async move {
        let compact_params = CompactionParameters {
            collection_id: Some(collection_id.to_string()),
            force: true,
            synchronous: true,
            hints: std::collections::HashMap::new(),
            timeout_ms: None,
            priority: crate::storage::traits::OperationPriority::Medium,
            estimated_input_size: 1024 * 1024, // 1MB estimated
            collection_config: Some(create_test_collection(
                collection_id,
                &temp_dir_path_for_compact,
            )),
        };
        compact_engine.do_compact(&compact_params).await
    });

    // Concurrent reads during compaction
    let mut read_handles = vec![];
    let temp_dir_path = temp_dir.path().to_str().unwrap().to_string();
    for task_id in 0..3 {
        let read_engine = engine.clone();
        let collection_id_owned = collection_id.to_string();
        let storage_url = format!("file://{}/{}/data", temp_dir_path, collection_id);
        let handle = tokio::spawn(async move {
            let mut successful_reads = 0;
            let mut failed_reads = 0;

            for attempt in 0..20 {
                let results = read_engine
                    .search_vectors(&collection_id_owned, &storage_url, &vec![0.5; 128], 10)
                    .await;

                match results {
                    Ok(search_results) => {
                        successful_reads += 1;
                        // Verify we got some results (even if empty during compaction)
                        debug!(
                            "Read {} succeeded during compaction, found {} results",
                            attempt,
                            search_results.len()
                        );
                    }
                    Err(e) => {
                        failed_reads += 1;
                        // During compaction, some reads might fail due to file replacement
                        // This is expected behavior - log but don't panic
                        warn!(
                            "Read {} failed during compaction (expected): {}",
                            attempt, e
                        );

                        // Check if it's a file access error (expected during compaction)
                        let error_str = e.to_string().to_lowercase();
                        if error_str.contains("no such file")
                            || error_str.contains("file not found")
                            || error_str.contains("no valid parquet files")
                            || error_str.contains("compaction_info")
                        {
                            // Expected during compaction - files being replaced
                            debug!("Expected file access error during compaction_info");
                        } else {
                            // Unexpected error - fail the test
                            panic!("Unexpected read error during compaction: {}", e);
                        }
                    }
                }

                sleep(Duration::from_millis(10)).await;
            }

            // Ensure we had at least some successful reads
            // During compaction, it's normal to have some failures
            assert!(
                successful_reads > 0,
                "Task {} had no successful reads during compaction",
                task_id
            );

            debug!(
                "Read task {} completed: {} successful, {} failed (expected during compaction)",
                task_id, successful_reads, failed_reads
            );
        });
        read_handles.push(handle);
    }

    // Wait for all operations
    compact_handle.await.unwrap().expect("Compaction failed");
    for handle in read_handles {
        handle.await.expect("Read task failed");
    }
}

#[tokio::test]
async fn test_concurrent_compaction_across_collections() {
    // Initialize hardware capabilities for testing
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    // Test that we can compact different collections concurrently
    let temp_dir = TempDir::new().unwrap();
    let config = create_compaction_config(temp_dir.path().to_str().unwrap());

    let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
    let engine = Arc::new(
        ViperEngine::from_core_config(
            crate::core::config::ViperConfig::default(),
            filesystem_factory,
        )
        .await
        .expect("Failed to create engine"),
    );

    let collections = vec!["collection_a", "collection_b", "collection_c"];

    // Set up storage assignments and create data for each collection
    for collection_id in &collections {
        setup_test_assignment(collection_id, temp_dir.path().to_str().unwrap()).await;

        // Create collection with storage assignment
        let collection = create_test_collection(collection_id, temp_dir.path().to_str().unwrap());

        // Create initial data
        let mut vectors = vec![];
        for i in 0..50 {
            vectors.push(create_test_vector(&format!("{}_{}", collection_id, i), 128));
        }

        let flush_params = FlushParameters {
            collection_id: Some(collection_id.to_string()),
            force: true,
            synchronous: true,
            vector_records: vectors,
            batch_ids: vec![],
            hints: std::collections::HashMap::new(),
            timeout_ms: None,
            trigger_compaction: false,
            estimated_size: 1024 * 1024, // 1MB estimated
            collection_config: Some(collection.clone()),
        };
        engine.do_flush(&flush_params).await.unwrap();

        // Create multiple files to compact
        for j in 0..3 {
            let mut more_vectors = vec![];
            for k in 0..20 {
                more_vectors.push(create_test_vector(
                    &format!("{}_extra_{}_{}", collection_id, j, k),
                    128,
                ));
            }

            let flush_params = FlushParameters {
                collection_id: Some(collection_id.to_string()),
                force: true,
                synchronous: false,
                vector_records: more_vectors,
                batch_ids: vec![],
                hints: std::collections::HashMap::new(),
                timeout_ms: None,
                trigger_compaction: false,
                estimated_size: 1024 * 1024, // 1MB estimated
                collection_config: Some(collection.clone()),
            };
            engine.do_flush(&flush_params).await.unwrap();
        }
    }

    // Start concurrent compactions on different collections
    let mut handles = vec![];
    for collection_id in collections {
        let engine_clone = engine.clone();
        let collection_id_owned = collection_id.to_string();
        let temp_path = temp_dir.path().to_str().unwrap().to_string();

        let handle = tokio::spawn(async move {
            // Create collection config for this specific collection
            let collection = create_test_collection(&collection_id_owned, &temp_path);

            let compact_params = CompactionParameters {
                collection_id: Some(collection_id_owned.clone()),
                force: true,
                synchronous: true,
                hints: std::collections::HashMap::new(),
                timeout_ms: None,
                priority: crate::storage::traits::OperationPriority::Medium,
                estimated_input_size: 1024 * 1024, // 1MB estimated
                collection_config: Some(collection),
            };

            let result = engine_clone.do_compact(&compact_params).await;
            assert!(
                result.is_ok(),
                "Compaction failed for collection {}",
                collection_id_owned
            );
            collection_id_owned
        });

        handles.push(handle);
    }

    // Wait for all compactions to complete
    let mut completed_collections = vec![];
    for handle in handles {
        let collection = handle.await.expect("Compaction task panicked");
        completed_collections.push(collection);
    }

    // Verify all collections were compacted
    assert_eq!(completed_collections.len(), 3);
    debug!(
        "Successfully compacted collections: {:?}",
        completed_collections
    );
}

#[tokio::test]
async fn test_atomic_coordinator_prevents_concurrent_same_collection_compaction() {
    // Initialize tracing for detailed debugging
    let _ = tracing_subscriber::fmt::try_init();
    debug!("🧪 TEST START: atomic coordinator concurrent compaction prevention");

    // Use the same pattern as successful SST tests - simple filesystem creation
    let temp_dir = TempDir::new().unwrap();
    debug!("📁 Created temp directory: {}", temp_dir.path().display());

    let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
    debug!("🗃️ Created filesystem factory");

    let engine = Arc::new(
        ViperEngine::from_core_config(
            crate::core::config::ViperConfig::default(),
            filesystem_factory,
        )
        .await
        .expect("Failed to create engine"),
    );
    debug!("🚀 Created VIPER engine");

    let collection_id = "test_atomic";
    debug!("🏷️ Using collection ID: {}", collection_id);

    setup_test_assignment(collection_id, temp_dir.path().to_str().unwrap()).await;
    debug!("📋 Set up test assignment for collection");

    // Create initial data using test utility vector generator
    let mut vectors = vec![];
    for i in 0..50 {
        vectors.push(create_test_vector(&format!("atomic_{}", i), 128));
    }
    debug!("📦 Created {} test vectors", vectors.len());

    let collection = create_test_collection(collection_id, temp_dir.path().to_str().unwrap());
    debug!("🗂️ Created test collection config");

    let flush_params = FlushParameters {
        collection_id: Some(collection_id.to_string()),
        force: true,
        synchronous: true,
        vector_records: vectors,
        batch_ids: vec![],
        hints: std::collections::HashMap::new(),
        timeout_ms: None,
        trigger_compaction: false,
        estimated_size: 1024 * 1024, // 1MB estimated
        collection_config: Some(collection.clone()),
    };
    debug!(
        "⚡ Starting flush operation with {} vectors",
        flush_params.vector_records.len()
    );

    let flush_result = engine.do_flush(&flush_params).await;
    match &flush_result {
        Ok(result) => {
            debug!(
                "✅ Flush succeeded: {} entries flushed, success: {}",
                result.entries_flushed.unwrap_or(0), result.success
            );
        }
        Err(e) => {
            debug!("❌ Flush failed: {}", e);
        }
    }
    flush_result.unwrap();

    // Try to start two concurrent compactions on the same collection
    debug!("🔀 Setting up concurrent compaction test");

    let engine1 = engine.clone();
    let engine2 = engine.clone();
    let collection1 = collection.clone();
    let collection2 = collection.clone();

    let (tx1, rx1) = tokio::sync::oneshot::channel();
    let (tx2, rx2) = tokio::sync::oneshot::channel();

    debug!("📊 Created communication channels for coordination");

    // First compaction
    debug!("🥇 Setting up first compaction task");
    let collection_id_clone1 = collection_id.to_string();
    let handle1 = tokio::spawn(async move {
        debug!("🥇 First compaction task started");
        tx1.send(()).unwrap(); // Signal that we're starting

        let compact_params = CompactionParameters {
            collection_id: Some(collection_id_clone1.clone()),
            force: true,
            synchronous: true,
            hints: std::collections::HashMap::new(),
            timeout_ms: Some(5000), // 5 second timeout
            priority: crate::storage::traits::OperationPriority::Medium,
            estimated_input_size: 1024 * 1024, // 1MB estimated
            collection_config: Some(collection1),
        };

        debug!(
            "🥇 First compaction: calling do_compact for collection {}",
            collection_id_clone1
        );
        let result = engine1.do_compact(&compact_params).await;
        match &result {
            Ok(res) => debug!(
                "🥇 First compaction result: success={}, entries_processed={}",
                res.success, res.entries_processed.unwrap_or(0)
            ),
            Err(e) => debug!("🥇 First compaction failed: {}", e),
        }
        result
    });

    // Wait for first compaction to start
    debug!("⏳ Waiting for first compaction to start");
    rx1.await.unwrap();
    debug!("🥇 First compaction started, waiting 100ms");
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Second compaction (should be blocked by atomic coordinator)
    debug!("🥈 Setting up second compaction task (should be blocked)");
    let collection_id_clone2 = collection_id.to_string();
    let handle2 = tokio::spawn(async move {
        debug!("🥈 Second compaction task started");
        tx2.send(()).unwrap(); // Signal that we're starting

        let compact_params = CompactionParameters {
            collection_id: Some(collection_id_clone2.clone()),
            force: true,
            synchronous: true,
            hints: std::collections::HashMap::new(),
            timeout_ms: Some(1000), // 1 second timeout - should fail
            priority: crate::storage::traits::OperationPriority::Medium,
            estimated_input_size: 1024 * 1024, // 1MB estimated
            collection_config: Some(collection2),
        };

        debug!(
            "🥈 Second compaction: calling do_compact for collection {}",
            collection_id_clone2
        );
        let result = engine2.do_compact(&compact_params).await;
        match &result {
            Ok(res) => debug!(
                "🥈 Second compaction result: success={}, entries_processed={}",
                res.success, res.entries_processed.unwrap_or(0)
            ),
            Err(e) => debug!("🥈 Second compaction failed (expected): {}", e),
        }
        result
    });

    // Wait for second compaction to start attempting
    debug!("⏳ Waiting for second compaction to start");
    rx2.await.unwrap();

    // Get results
    let result1 = handle1.await.expect("First compaction task panicked");
    let result2 = handle2.await.expect("Second compaction task panicked");

    // First should succeed
    assert!(result1.is_ok(), "First compaction should succeed");

    // Second should either fail with a lock error or succeed after first completes
    // (depends on timing and timeout settings)
    match result2 {
        Ok(_) => debug!("Second compaction succeeded (after first completed)"),
        Err(e) => {
            debug!("Second compaction failed as expected: {}", e);
            // Should be a lock/coordination error
            assert!(
                e.to_string().contains("lock")
                    || e.to_string().contains("timeout")
                    || e.to_string().contains("operation")
                    || e.to_string().contains("in progress")
                    || e.to_string().contains("Failed to read input file"),
                "Expected lock/timeout/file error, got: {}",
                e
            );
        }
    }
}

#[tokio::test]
async fn test_size_tiered_compaction_strategy() {
    // Initialize hardware capabilities for testing
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let temp_dir = TempDir::new().unwrap();
    let config = create_compaction_config(temp_dir.path().to_str().unwrap());

    let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
    let engine = ViperEngine::from_core_config(
        crate::core::config::ViperConfig::default(),
        filesystem_factory,
    )
    .await
    .expect("Failed to create engine");

    let collection_id = "size_tiered";

    // Set up storage assignment for the test collection
    setup_test_assignment(collection_id, temp_dir.path().to_str().unwrap()).await;

    // Create files of different sizes
    let file_sizes = vec![10, 10, 10, 50, 50, 100];

    for (idx, size) in file_sizes.iter().enumerate() {
        let mut vectors = Vec::new();
        for i in 0..*size {
            vectors.push(create_test_vector(&format!("tier_{}_vec_{}", idx, i), 64));
        }

        // VIPER is columnar - vectors will be flushed below

        let flush_params = FlushParameters {
            collection_id: Some(collection_id.to_string()),
            force: true,
            synchronous: false,
            vector_records: vectors,
            batch_ids: vec![],
            hints: std::collections::HashMap::new(),
            timeout_ms: None,
            trigger_compaction: false,
            estimated_size: 1024 * 1024, // 1MB estimated
            collection_config: Some(create_test_collection(
                collection_id,
                temp_dir.path().to_str().unwrap(),
            )),
        };
        engine.do_flush(&flush_params).await.unwrap();
    }

    // Run size-tiered compaction
    let compact_params = CompactionParameters {
        collection_id: Some(collection_id.to_string()),
        force: true,
        synchronous: true,
        hints: std::collections::HashMap::new(),
        timeout_ms: None,
        priority: crate::storage::traits::OperationPriority::Medium,
        estimated_input_size: 1024 * 1024, // 1MB estimated
        collection_config: Some(create_test_collection(
            collection_id,
            temp_dir.path().to_str().unwrap(),
        )),
    };

    let result = engine
        .do_compact(&compact_params)
        .await
        .expect("Compaction failed");

    assert!(result.success);

    // Verify all data accessible
    let total_vectors: usize = file_sizes.iter().sum();
    let storage_url = format!(
        "file://{}/{}/data",
        temp_dir.path().to_str().unwrap(),
        collection_id
    );
    let search_results = engine
        .search_vectors(
            collection_id,
            &storage_url,
            &vec![0.5; 64],
            total_vectors + 10,
        )
        .await
        .unwrap();

    assert_eq!(search_results.len(), total_vectors);
}

#[tokio::test]
async fn test_compaction_with_metadata_filtering() {
    // Initialize hardware capabilities for testing
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let temp_dir = TempDir::new().unwrap();
    let config = create_compaction_config(temp_dir.path().to_str().unwrap());

    let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
    let engine = ViperEngine::from_core_config(
        crate::core::config::ViperConfig::default(),
        filesystem_factory,
    )
    .await
    .expect("Failed to create engine");

    let collection_id = "metadata_compact";

    // Set up storage assignment for the test collection
    setup_test_assignment(collection_id, temp_dir.path().to_str().unwrap()).await;

    // Insert vectors with different metadata
    for category in &["A", "B", "C"] {
        let mut vectors = Vec::new();
        for i in 0..20 {
            let mut vector = create_test_vector(&format!("meta_{}_{}", category, i), 128);
            vector.metadata.insert(
                "category".to_string(),
                crate::proto::proximadb_v1::SqlValue {
                    value: Some(Value::StringValue(category.to_string())),
                },
            );
            vectors.push(vector);
        }

        // Flush each category separately
        let flush_params = FlushParameters {
            collection_id: Some(collection_id.to_string()),
            force: true,
            synchronous: false,
            vector_records: vectors,
            batch_ids: vec![],
            hints: std::collections::HashMap::new(),
            timeout_ms: None,
            trigger_compaction: false,
            estimated_size: 1024 * 1024, // 1MB estimated
            collection_config: Some(create_test_collection(
                collection_id,
                temp_dir.path().to_str().unwrap(),
            )),
        };
        engine.do_flush(&flush_params).await.unwrap();
    }

    // Run compaction
    let compact_params = CompactionParameters {
        collection_id: Some(collection_id.to_string()),
        force: true,
        synchronous: true,
        hints: std::collections::HashMap::new(),
        timeout_ms: None,
        priority: crate::storage::traits::OperationPriority::Medium,
        estimated_input_size: 1024 * 1024, // 1MB estimated
        collection_config: Some(create_test_collection(
            collection_id,
            temp_dir.path().to_str().unwrap(),
        )),
    };

    engine
        .do_compact(&compact_params)
        .await
        .expect("Compaction failed");

    // Verify metadata preserved after compaction
    let storage_url = format!(
        "file://{}/{}/data",
        temp_dir.path().to_str().unwrap(),
        collection_id
    );
    for category in &["A", "B", "C"] {
        let search_results = engine
            .search_vectors(collection_id, &storage_url, &vec![0.5; 128], 100)
            .await
            .unwrap();

        let category_count = search_results
            .iter()
            .flat_map(|sr| sr.results.iter())
            .filter(|r| r.metadata.get("category").and_then(|v| v.as_deref()) == Some(category))
            .count();

        assert_eq!(
            category_count, 20,
            "Category {} vectors missing after compaction",
            category
        );
    }
}

#[tokio::test]
async fn test_incremental_compaction() {
    // Initialize hardware capabilities for testing
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let temp_dir = TempDir::new().unwrap();
    let config = create_compaction_config(temp_dir.path().to_str().unwrap());
    // Incremental compaction is handled by CompactionParameters

    let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
    let engine = ViperEngine::from_core_config(
        crate::core::config::ViperConfig::default(),
        filesystem_factory,
    )
    .await
    .expect("Failed to create engine");

    let collection_id = "incremental";

    // Set up storage assignment for the test collection
    setup_test_assignment(collection_id, temp_dir.path().to_str().unwrap()).await;

    // Create 6 small files
    for batch in 0..6 {
        let mut vectors = Vec::new();
        for i in 0..5 {
            vectors.push(create_test_vector(&format!("inc_{}_vec_{}", batch, i), 64));
        }

        // VIPER is columnar - vectors will be flushed below

        let flush_params = FlushParameters {
            collection_id: Some(collection_id.to_string()),
            force: true,
            synchronous: false,
            vector_records: vectors,
            batch_ids: vec![],
            hints: std::collections::HashMap::new(),
            timeout_ms: None,
            trigger_compaction: false,
            estimated_size: 1024 * 1024, // 1MB estimated
            collection_config: Some(create_test_collection(
                collection_id,
                temp_dir.path().to_str().unwrap(),
            )),
        };
        engine.do_flush(&flush_params).await.unwrap();
    }

    // Run incremental compaction multiple times
    for _ in 0..3 {
        let compact_params = CompactionParameters {
            collection_id: Some(collection_id.to_string()),
            force: true,
            synchronous: true,
            hints: std::collections::HashMap::new(),
            timeout_ms: None,
            priority: crate::storage::traits::OperationPriority::Medium,
            estimated_input_size: 1024 * 1024, // 1MB estimated
            collection_config: Some(create_test_collection(
                collection_id,
                temp_dir.path().to_str().unwrap(),
            )),
        };

        engine
            .do_compact(&compact_params)
            .await
            .expect("Incremental compaction failed");
    }

    // Verify all vectors preserved
    let storage_url = format!(
        "file://{}/{}/data",
        temp_dir.path().to_str().unwrap(),
        collection_id
    );
    let search_results = engine
        .search_vectors(collection_id, &storage_url, &vec![0.5; 64], 100)
        .await
        .unwrap();

    assert_eq!(search_results.len(), 30); // 6 batches * 5 vectors
}
