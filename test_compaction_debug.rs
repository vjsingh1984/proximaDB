use std::sync::Arc;
use tempfile::TempDir;
use tracing::{debug, error, info};

#[tokio::main]
async fn main() {
    // Initialize
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    
    let temp_dir = TempDir::new().unwrap();
    let filesystem_factory = Arc::new(
        proximadb::storage::persistence::filesystem::FilesystemFactory::new(Default::default())
            .await
            .unwrap()
    );
    
    let engine = proximadb::storage::engines::viper::engine::ViperEngine::from_core_config(
        proximadb::core::config::ViperConfig::default(),
        filesystem_factory.clone()
    ).await.expect("Failed to create engine");
    
    let collection_id = "test_debug";
    let base_path = temp_dir.path().to_str().unwrap();
    
    // Create a simple vector record
    let vector_record = proximadb::storage::engines::viper::VectorRecord {
        id: Some("test_vec_1".to_string()),
        version: Some(1),
        timestamp: 0,
        vector: vec![1.0, 2.0, 3.0, 4.0],
        metadata: vec![],
        updated_at: None,
        expires_at: None,
        rank: None,
        score: None,
        distance: None,
    };
    
    // Create collection config
    let collection = proximadb::proto::proximadb::Collection {
        id: collection_id.to_string(),
        config: Some(proximadb::proto::proximadb::CollectionConfig {
            name: collection_id.to_string(),
            dimension: 4,
            distance_metric: 0,
            storage_engine: 0,
            primary_indexing_algorithm: 0,
            filterable_columns: vec![],
            index_configs: vec![],
            quantization: None,
            primary_index: String::new(),
            auto_index_selection: false,
            description: None,
            tags: vec![],
            owner: None,
            compression: None,
            storage_location: None,
            optimization_hints: None,
        }),
        stats: None,
        created_at: chrono::Utc::now().timestamp(),
        updated_at: chrono::Utc::now().timestamp(),
        storage_assignment: Some(proximadb::proto::proximadb::StorageAssignment {
            base_location: format!("file://{}", base_path),
            assigned_at: chrono::Utc::now().timestamp(),
        }),
    };
    
    // Flush the vector
    let flush_params = proximadb::storage::traits::FlushParameters {
        collection_id: Some(collection_id.to_string()),
        force: true,
        synchronous: true,
        vector_records: vec![vector_record.clone().into()],
        batch_ids: vec![],
        hints: std::collections::HashMap::new(),
        timeout_ms: None,
        trigger_compaction: false,
        collection_config: Some(collection.clone()),
    };
    
    let flush_result = engine.do_flush(&flush_params).await.unwrap();
    info!("Flush result: success={}, files_created={}", flush_result.success, flush_result.files_created);
    
    // Search for the vector
    let storage_url = format!("file://{}/{}/data", base_path, collection_id);
    debug!("Searching at: {}", storage_url);
    
    let search_results = engine.search_vectors(
        collection_id,
        &storage_url,
        &vec![1.0, 2.0, 3.0, 4.0],
        10,
    ).await.unwrap();
    
    debug!("Search results: {} found", search_results.len());
    for result in &search_results {
        debug!("  - ID: {}, Score: {:?}", result.id, result.score);
    }
    
    if search_results.is_empty() {
        error!("ERROR: Vector not found after flush!");
    } else {
        info!("SUCCESS: Vector found!");
    }
}