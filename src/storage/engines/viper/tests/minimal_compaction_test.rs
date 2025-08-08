//! Minimal test to debug VIPER compaction

use std::sync::Arc;
use tempfile::TempDir;
use anyhow::Result;

use crate::core::VectorRecord;
use crate::proto::proximadb::MetadataItem;
use crate::storage::engines::viper::ViperEngine;
use crate::storage::traits::{UnifiedStorageEngine, FlushParameters, CompactionParameters};
use crate::storage::persistence::filesystem::FilesystemFactory;

/// Create test vector
fn create_test_vector(id: &str, dimension: usize) -> VectorRecord {
    VectorRecord {
        id: Some(id.to_string()),
        vector: (0..dimension).map(|i| (i as f32) / (dimension as f32)).collect(),
        metadata: vec![],
        timestamp: chrono::Utc::now().timestamp() as u32,
        updated_at: Some(chrono::Utc::now().timestamp() as u32),
        expires_at: None,
        version: Some(1),
        rank: None,
        score: None,
        distance: None,
    
        }
}

#[tokio::test]
async fn test_minimal_viper_compaction() -> Result<()> {
    println!("\n[TEST] Starting minimal VIPER compaction test");
    
    let temp_dir = TempDir::new()?;
    let base_path = temp_dir.path().to_str().unwrap();
    
    println!("[TEST] Test directory: {}", base_path);
    
    // Create config (using default core config for testing)
    let core_config = crate::core::config::ViperConfig::default();
    
    // Create engine
    let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await?);
    let engine = ViperEngine::from_core_config(core_config, filesystem_factory).await?;
    
    let collection_id = "minimal_test";
    
    // Set up storage assignment
    use tokio::fs;
    let data_dir = format!("{}/{}/data", base_path, collection_id);
    fs::create_dir_all(&data_dir).await?;
    
    // Storage assignment is now handled internally by CollectionService
    // when a collection is created. For test purposes, we just ensure
    // the directory structure exists.
    let wal_dir = format!("{}/{}/write_buffer", base_path, collection_id);
    fs::create_dir_all(&wal_dir).await?;
    
    // Create and flush just 3 vectors
    println!("\n[TEST] Creating and flushing 3 vectors");
    
    let vectors = vec![
        create_test_vector("vec_0", 128),
        create_test_vector("vec_1", 128),
        create_test_vector("vec_2", 128),
    ];
    
    let flush_params = FlushParameters {
        collection_id: Some(collection_id.to_string()),
        force: true,
        synchronous: true,
        vector_records: vectors,
        batch_ids: vec![],
        hints: std::collections::HashMap::new(),
        timeout_ms: None,
        trigger_compaction: false,
    
        collection_config: None,};
    
    let flush_result = engine.do_flush(&flush_params).await?;
    println!("[TEST] Flush complete: {} files created, {} entries flushed", 
             flush_result.files_created, flush_result.entries_flushed);
    
    // Run compaction
    println!("\n[TEST] Running compaction");
    
    let compact_params = CompactionParameters {
        collection_id: Some(collection_id.to_string()),
        force: true,
        synchronous: true,
        hints: std::collections::HashMap::new(),
        timeout_ms: None,
        priority: crate::storage::traits::OperationPriority::Medium,
    
        collection_config: None,};
    
    let compact_result = engine.do_compact(&compact_params).await?;
    println!("[TEST] Compaction complete: {} input files, {} output files, {} entries processed", 
             compact_result.input_files, compact_result.output_files, compact_result.entries_processed);
    
    assert!(compact_result.success, "Compaction should succeed");
    assert_eq!(compact_result.entries_processed, 3, "Should process 3 entries");
    
    Ok(())
}