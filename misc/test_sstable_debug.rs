use proximadb::storage::engines::sst::readers::unified_sstable_reader::UnifiedSstableReader;
use proximadb::storage::persistence::filesystem::FilesystemFactory;
use proximadb::core::search::{SearchParams, CollectionContext};
use proximadb::compute::distance::DistanceMetric;
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter("proximadb=debug")
        .init();

    // Find SSTable file from test
    let test_dir = std::fs::read_dir("/tmp")?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry.file_name().to_string_lossy().starts_with(".tmp") &&
            entry.file_type().map(|t| t.is_dir()).unwrap_or(false)
        })
        .max_by_key(|entry| entry.metadata().and_then(|m| m.modified()).ok())?;
    
    let base_path = test_dir.path();
    println!("Using test directory: {:?}", base_path);
    
    // Find SSTable file
    let sstable_path = std::fs::read_dir(base_path.join("test_collection_0/data"))?
        .filter_map(|entry| entry.ok())
        .find(|entry| entry.file_name().to_string_lossy().ends_with(".sst"))
        .map(|entry| entry.path())?;
    
    let sstable_url = format!("file://{}", sstable_path.display());
    println!("Found SSTable: {}", sstable_url);
    
    // Check file size
    let metadata = std::fs::metadata(&sstable_path)?;
    println!("SSTable file size: {} bytes", metadata.len());
    
    // Create reader
    let filesystem = Arc::new(FilesystemFactory::new(Default::default()).await?);
    let reader = UnifiedSstableReader::new(filesystem);
    
    // Load metadata
    println!("\nLoading SSTable metadata...");
    reader.load_metadata(&sstable_url).await?;
    
    // Create search params
    let params = SearchParams {
        query_vector: Some(vec![1.0, 0.0, 0.0]),
        top_k: Some(3),
        distance_metric: Some(DistanceMetric::Cosine),
        ..Default::default()
    };
    
    // Create context
    let context = CollectionContext {
        collection_id: "test_collection_0".to_string(),
        file_path: sstable_url.clone(),
        sstable_files: vec![sstable_url.clone()],
        total_vectors: 3,
        metadata_columns: vec!["category".to_string()],
        level: 0,
        creation_time: chrono::Utc::now(),
    };
    
    // Search
    println!("\nSearching vectors...");
    let results = reader.search_vectors(&params, &context).await?;
    
    println!("\nSearch results: {} found", results.len());
    for (i, result) in results.iter().enumerate() {
        println!("  {}: id={}, score={}, distance={:?}", 
                 i, result.id, result.score, result.distance);
    }
    
    Ok(())
}