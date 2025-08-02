use proximadb::storage::engines::viper::{ViperEngine, ViperConfig};
use proximadb::storage::traits::{UnifiedStorageEngine, FlushParameters};
use proximadb::storage::persistence::filesystem::FilesystemFactory;
use proximadb::core::VectorRecord;
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("[TEST] Starting simple VIPER flush test");
    
    // Create temp dir
    let temp_dir = tempfile::TempDir::new()?;
    let base_path = temp_dir.path().to_str().unwrap();
    println!("[TEST] Base path: {}", base_path);
    
    // Create engine
    let config = ViperConfig::default();
    let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await?);
    println!("[TEST] Creating VIPER engine...");
    let engine = ViperEngine::new(config, filesystem_factory).await?;
    println!("[TEST] Engine created");
    
    // Setup collection
    let collection_id = "test_flush";
    tokio::fs::create_dir_all(format!("{}/{}/data", base_path, collection_id)).await?;
    
    let assignment_service = proximadb::storage::assignment_service::get_assignment_service();
    let storage_location = proximadb::core::config::StorageLocation {
        url: format!("file://{}", base_path),
        weight: 1,
        tags: Default::default(),
    };
    println!("[TEST] Assigning storage location...");
    assignment_service
        .assign_collection(collection_id, &[storage_location], "hash")
        .await?;
    println!("[TEST] Storage assigned");
    
    // Create one vector
    let vector = VectorRecord {
        id: Some("test_vec".to_string()),
        vector: vec![0.1, 0.2, 0.3],
        metadata: vec![],
        timestamp: 1000,
        created_at: 1000,
        updated_at: Some(1000),
        expires_at: None,
        version: Some(1),
        rank: None,
        score: None,
        distance: None,
    };
    
    // Flush
    println!("[TEST] Calling do_flush...");
    let params = FlushParameters {
        collection_id: Some(collection_id.to_string()),
        force: true,
        synchronous: true,
        vector_records: vec![vector],
        batch_ids: vec![],
        hints: Default::default(),
        timeout_ms: None,
        trigger_compaction: false,
    };
    
    let result = engine.do_flush(&params).await?;
    println!("[TEST] Flush result: {:?}", result);
    
    Ok(())
}