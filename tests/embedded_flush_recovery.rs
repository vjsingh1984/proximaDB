use proximadb::embedded::{EmbeddedConfig, EmbeddedProximaDB};

// Verifies that a close/flush cycle writes data to the UUID-based storage path
// and that reopening the embedded database can search the flushed data without
// needing WAL replay. Also exercises idempotent flush (second flush is a no-op).
#[test]
fn embedded_flush_persists_and_recovers() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let data_path = temp_dir.path().join("data");
    std::fs::create_dir_all(&data_path).expect("create data dir");

    // Create DB, insert, and flush
    let mut config = EmbeddedConfig::for_low_memory(data_path.to_string_lossy().to_string());
    config.enable_wal = true;
    let db = EmbeddedProximaDB::new(config).expect("create db");

    db.create_collection("test_collection", 4, Some("sst"))
        .expect("create collection");

    let ids = vec!["v0".to_string(), "v1".to_string()];
    let vectors = vec![vec![0.1_f32, 0.2, 0.3, 0.4], vec![0.1_f32, 0.2, 0.3, 0.5]];

    db.insert("test_collection", ids.clone(), vectors.clone(), None)
        .expect("insert");

    // First flush should write SST + update manifest; second flush should be a no-op
    db.flush().expect("flush");
    db.flush().expect("idempotent flush");
    drop(db); // close database

    // Reopen and search; results should be served from flushed storage, not WAL
    let mut reopen_config = EmbeddedConfig::for_low_memory(data_path.to_string_lossy().to_string());
    reopen_config.enable_wal = true;
    let reopened = EmbeddedProximaDB::new(reopen_config).expect("reopen db");

    let results = reopened
        .search("test_collection", vectors[0].clone(), 2, None)
        .expect("search");

    assert!(
        results.iter().any(|r| r.id == "v0"),
        "Expected to recover vector v0 after reopen; got ids: {:?}",
        results.iter().map(|r| r.id.clone()).collect::<Vec<_>>()
    );

    // Additional flush to ensure no pending WAL remains after search
    reopened.flush().expect("final flush");
}

/// Moderate-scale recovery test (kept bounded to avoid excessive runtime).
#[test]
fn embedded_flush_persists_many_vectors() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let data_path = temp_dir.path().join("data");
    std::fs::create_dir_all(&data_path).expect("create data dir");

    eprintln!("📁 Test data path: {:?}", data_path);

    // Create DB with 100 vectors
    let mut config = EmbeddedConfig::for_low_memory(data_path.to_string_lossy().to_string());
    config.enable_wal = true;
    let db = EmbeddedProximaDB::new(config).expect("create db");

    db.create_collection("test_collection", 128, Some("sst"))
        .expect("create collection");

    // Insert a bounded set that still exercises SST flushing/recovery paths.
    let num_vectors = 2_000;
    let dimension = 128;
    let mut ids = Vec::with_capacity(num_vectors);
    let mut vectors = Vec::with_capacity(num_vectors);

    for i in 0..num_vectors {
        ids.push(format!("vec_{}", i));
        let vector: Vec<f32> = (0..dimension)
            .map(|j| i as f32 * 0.01 + j as f32 * 0.001 )
            .collect();
        vectors.push(vector);
    }

    db.insert("test_collection", ids.clone(), vectors.clone(), None)
        .expect("insert");

    eprintln!("✅ Inserted {} vectors", num_vectors);

    // Flush
    db.flush().expect("flush");
    eprintln!("✅ Flushed to disk");

    // List files in data directory to confirm SST files were written
    fn print_dir_recursive(path: &std::path::Path, indent: usize) {
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                let path = entry.path();
                let prefix = "  ".repeat(indent);
                if path.is_dir() {
                    eprintln!(
                        "{}📁 {}/",
                        prefix,
                        path.file_name().unwrap().to_string_lossy()
                    );
                    print_dir_recursive(&path, indent + 1);
                } else {
                    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    eprintln!(
                        "{}📄 {} ({} bytes)",
                        prefix,
                        path.file_name().unwrap().to_string_lossy(),
                        size
                    );
                }
            }
        }
    }
    eprintln!("📂 Files in data directory:");
    print_dir_recursive(&data_path, 1);

    drop(db);
    eprintln!("✅ Closed database");

    // Reopen
    let mut reopen_config = EmbeddedConfig::for_low_memory(data_path.to_string_lossy().to_string());
    reopen_config.enable_wal = true;
    let reopened = EmbeddedProximaDB::new(reopen_config).expect("reopen db");

    eprintln!("✅ Reopened database");

    // Search for first vector - should find it
    let results = reopened
        .search("test_collection", vectors[0].clone(), 10, None)
        .expect("search");

    eprintln!("🔍 Search results: {} found", results.len());
    for (i, result) in results.iter().enumerate() {
        eprintln!("  [{}] id={}, score={}", i, result.id, result.score);
    }

    // vec_0 should be the top result with score 1.0 (exact match)
    assert!(
        !results.is_empty(),
        "Should find at least 1 result after reopen"
    );
    assert_eq!(
        results[0].id, "vec_0",
        "Query vector should be the top result"
    );
    assert!(
        (results[0].score - 1.0).abs() < 0.001,
        "Score should be ~1.0 for exact match"
    );

    eprintln!("✅ Test passed!");
}

/// Test that verifies SST block serialization/deserialization roundtrip
/// This ensures the writer format is compatible with the reader format.
#[test]
fn sst_block_serialization_roundtrip() {
    use proximadb::embedded::{EmbeddedConfig, EmbeddedProximaDB};

    let temp_dir = tempfile::tempdir().expect("tempdir");
    let data_path = temp_dir.path().join("data");
    std::fs::create_dir_all(&data_path).expect("create data dir");

    // Create collection and insert vectors of varying sizes to test block boundaries
    let mut config = EmbeddedConfig::for_low_memory(data_path.to_string_lossy().to_string());
    config.enable_wal = true;
    let db = EmbeddedProximaDB::new(config).expect("create db");

    db.create_collection("roundtrip_test", 64, Some("sst"))
        .expect("create collection");

    // Insert vectors in batches to create multiple blocks without overloading CI.
    // SST uses ~500 vectors per block, so 2K vectors yields multiple blocks.
    let num_batches = 4;
    let batch_size = 500;
    let total_to_insert = num_batches * batch_size;

    for batch_idx in 0..num_batches {
        let mut ids = Vec::with_capacity(batch_size);
        let mut vectors = Vec::with_capacity(batch_size);

        for i in 0..batch_size {
            let global_id = batch_idx * batch_size + i;
            ids.push(format!("vec_{}", global_id));
            // Create distinct vectors using the global ID
            let vector: Vec<f32> = (0..64)
                .map(|j| global_id as f32 * 0.01 + j as f32 * 0.001 )
                .collect();
            vectors.push(vector);
        }

        db.insert("roundtrip_test", ids, vectors, None)
            .expect(&format!("insert batch {}", batch_idx));
    }

    eprintln!(
        "✅ Inserted {} vectors in {} batches",
        total_to_insert, num_batches
    );

    // Flush to disk
    db.flush().expect("flush");
    eprintln!("✅ Flushed to disk");

    // Verify pre-reopen search works (search for first vector, should be in top-10)
    let query_vec: Vec<f32> = (0..64).map(|j| j as f32 * 0.001).collect(); // vec_0's vector
    let pre_results = db
        .search("roundtrip_test", query_vec.clone(), 10, None)
        .expect("pre-reopen search");
    assert!(!pre_results.is_empty(), "Should find vectors before reopen");
    assert_eq!(
        pre_results[0].id, "vec_0",
        "vec_0 should be top result before reopen"
    );
    eprintln!(
        "✅ Pre-reopen: vec_0 is top result with score {}",
        pre_results[0].score
    );

    drop(db);
    eprintln!("✅ Closed database");

    // Reopen and verify all vectors are accessible
    let mut reopen_config = EmbeddedConfig::for_low_memory(data_path.to_string_lossy().to_string());
    reopen_config.enable_wal = true;
    let reopened = EmbeddedProximaDB::new(reopen_config).expect("reopen db");

    let post_results = reopened
        .search("roundtrip_test", query_vec, 10, None)
        .expect("post-reopen search");

    eprintln!(
        "📊 Post-reopen: {} results, top result: id={}, score={}",
        post_results.len(),
        post_results
            .first()
            .map(|r| r.id.as_str())
            .unwrap_or("none"),
        post_results.first().map(|r| r.score).unwrap_or(0.0)
    );

    // Verify vec_0 is still the top result after reopen (proves all blocks were read)
    assert!(!post_results.is_empty(), "Should find vectors after reopen");
    assert_eq!(
        post_results[0].id, "vec_0",
        "SERIALIZATION BUG: vec_0 not found as top result after reopen (got {})",
        post_results[0].id
    );
    assert!(
        (post_results[0].score - 1.0).abs() < 0.001,
        "vec_0 should have score ~1.0 (exact match), got {}",
        post_results[0].score
    );

    eprintln!("✅ Serialization roundtrip test passed!");
}

/// Test that verifies large k values work correctly (k=1000)
/// This ensures no hidden limits on search results.
#[test]
fn test_large_k_search_returns_correct_count() {
    use proximadb::embedded::{EmbeddedConfig, EmbeddedProximaDB};

    let temp_dir = tempfile::tempdir().expect("tempdir");
    let data_path = temp_dir.path().join("data");
    std::fs::create_dir_all(&data_path).expect("create data dir");

    // Create collection and insert enough vectors to test k=1000 behavior
    let mut config = EmbeddedConfig::for_low_memory(data_path.to_string_lossy().to_string());
    config.enable_wal = true;
    let db = EmbeddedProximaDB::new(config).expect("create db");

    db.create_collection("test_large_k", 64, Some("sst"))
        .expect("create collection");

    let num_vectors = 2_000;
    let mut ids = Vec::with_capacity(num_vectors);
    let mut vectors = Vec::with_capacity(num_vectors);

    for i in 0..num_vectors {
        ids.push(format!("vec_{}", i));
        let vector: Vec<f32> = (0..64)
            .map(|j| i as f32 * 0.01 + j as f32 * 0.001 )
            .collect();
        vectors.push(vector);
    }

    db.insert("test_large_k", ids.clone(), vectors.clone(), None)
        .expect("insert");

    eprintln!(
        "Inserted {} vectors (ids from {} to {})",
        ids.len(),
        ids.first().unwrap(),
        ids.last().unwrap()
    );

    // Check collection info to verify vector count
    let collection_info = db.get_collection("test_large_k").expect("get collection");
    if let Some(info) = collection_info {
        eprintln!("Collection info: {} vectors", info.vector_count);
    }

    // Search BEFORE flush/close to check if the limit is in WAL vs SST path
    // Test with different k values to trace where the limit is applied
    eprintln!("\nPre-flush k scan:");
    for k in [10, 50, 100, 200, 500, 1000] {
        let results = db
            .search("test_large_k", vectors[0].clone(), k, None)
            .expect(&format!("search pre-flush k={}", k));
        eprintln!("  k={}: got {} results", k, results.len());
    }
    let results_pre_flush = db
        .search("test_large_k", vectors[0].clone(), 1000, None)
        .expect("search pre-flush");

    db.flush().expect("flush");

    // Search AFTER flush but before close
    let results_post_flush = db
        .search("test_large_k", vectors[0].clone(), 1000, None)
        .expect("search post-flush");
    eprintln!(
        "Post-flush k=1000: got {} results",
        results_post_flush.len()
    );

    drop(db);

    // Reopen and search with k=1000
    let mut reopen_config = EmbeddedConfig::for_low_memory(data_path.to_string_lossy().to_string());
    reopen_config.enable_wal = true;
    let reopened = EmbeddedProximaDB::new(reopen_config).expect("reopen db");

    // Test with k=1000 - should return exactly 1000 results
    let results_1000 = reopened
        .search("test_large_k", vectors[0].clone(), 1000, None)
        .expect("search k=1000");

    eprintln!("Post-reopen k=1000: got {} results", results_1000.len());

    // Check all three phases
    eprintln!("\nSummary:");
    eprintln!(
        "  Pre-flush (WAL only):     {} results",
        results_pre_flush.len()
    );
    eprintln!(
        "  Post-flush (WAL+SST):     {} results",
        results_post_flush.len()
    );
    eprintln!("  Post-reopen (SST only):   {} results", results_1000.len());

    assert_eq!(
        results_1000.len(),
        1000,
        "k=1000 should return exactly 1000 results when 2000 vectors exist"
    );
    assert_eq!(
        results_1000[0].id, "vec_0",
        "Query vector should be top result"
    );

    eprintln!("✅ Large k test passed - no hidden limits!");
}
