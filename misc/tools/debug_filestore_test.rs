// Debug test for filestore backend file creation issue

use std::sync::Arc;
use tempfile::TempDir;
use tokio;
use anyhow::Result;

use proximadb::storage::metadata::backends::filestore_backend::{
    FilestoreMetadataBackend, FilestoreMetadataConfig, CollectionRecord
};
use proximadb::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};
use proximadb::proto::proximadb::CollectionConfig;

fn create_test_collection_config(name: &str) -> CollectionConfig {
    CollectionConfig {
        name: name.to_string(),
        dimension: 128,
        distance_metric: 1, // COSINE
        indexing_algorithm: 1, // HNSW
        storage_engine: 1, // VIPER
        filterable_metadata_fields: vec![],
        indexing_config: std::collections::HashMap::new(),
    }
}

fn count_files_recursive(dir: &std::path::Path) -> Result<usize> {
    let mut count = 0;
    if dir.exists() {
        println!("Directory exists: {}", dir.display());
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            println!("  Found: {}", path.display());
            if path.is_dir() {
                count += count_files_recursive(&path)?;
            } else {
                count += 1;
            }
        }
    } else {
        println!("Directory does not exist: {}", dir.display());
    }
    Ok(count)
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("🧪 Debug test for FilestoreMetadataBackend file creation");
    
    // Create temporary directory for test
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();
    
    println!("📁 Test directory: {}", temp_path.display());
    
    // Configure filestore to use temp directory
    let filestore_config = FilestoreMetadataConfig {
        filestore_url: format!("file://{}", temp_path.display()),
        enable_compression: true,
        enable_backup: true,
        enable_snapshot_archival: true,
        max_archived_snapshots: 5,
    };
    
    println!("🔧 Filestore URL: {}", filestore_config.filestore_url);
    
    // Create filesystem factory
    let filesystem_config = FilesystemConfig::default();
    let filesystem_factory = Arc::new(
        FilesystemFactory::new(filesystem_config).await?
    );
    
    // Create filestore backend
    println!("🔧 Creating FilestoreMetadataBackend...");
    let filestore_backend = Arc::new(
        FilestoreMetadataBackend::new(filestore_config, filesystem_factory).await?
    );
    
    println!("✅ FilestoreMetadataBackend created successfully");
    
    // Check initial directory structure
    println!("\n📂 Initial directory structure:");
    let metadata_dir = temp_path.join("metadata");
    if metadata_dir.exists() {
        println!("  metadata/ directory exists");
        for entry in std::fs::read_dir(&metadata_dir)? {
            let entry = entry?;
            println!("    {}", entry.file_name().to_string_lossy());
        }
    } else {
        println!("  metadata/ directory does NOT exist");
    }
    
    let initial_files = count_files_recursive(&metadata_dir)?;
    println!("📄 Initial files: {}", initial_files);
    
    // Test upsert operation
    println!("\n1️⃣ Testing upsert_collection_record...");
    
    let test_config = create_test_collection_config("test_collection");
    let record = CollectionRecord::from_grpc_config("test_collection".to_string(), &test_config)?;
    
    println!("   📝 Collection record created: {} (UUID: {})", record.name, record.uuid);
    
    // Upsert the record
    println!("   💾 Calling upsert_collection_record...");
    filestore_backend.upsert_collection_record(record.clone()).await?;
    println!("   ✅ Upsert completed successfully");
    
    // Check files after upsert
    println!("\n📂 Directory structure after upsert:");
    if metadata_dir.exists() {
        println!("  metadata/ directory exists");
        for entry in std::fs::read_dir(&metadata_dir)? {
            let entry = entry?;
            let path = entry.path();
            println!("    {} ({})", entry.file_name().to_string_lossy(), 
                if path.is_dir() { "DIR" } else { "FILE" });
            
            if path.is_dir() {
                for sub_entry in std::fs::read_dir(&path)? {
                    let sub_entry = sub_entry?;
                    let sub_path = sub_entry.path();
                    let size = if sub_path.is_file() {
                        format!(" ({} bytes)", std::fs::metadata(&sub_path)?.len())
                    } else { " (DIR)".to_string() };
                    println!("      {}{}", sub_entry.file_name().to_string_lossy(), size);
                }
            }
        }
    } else {
        println!("  metadata/ directory still does NOT exist");
    }
    
    let after_upsert_files = count_files_recursive(&metadata_dir)?;
    println!("📄 Files after upsert: {} (delta: +{})", after_upsert_files, after_upsert_files - initial_files);
    
    // Test get operation to verify in-memory operation
    println!("\n2️⃣ Testing get_collection_record_by_name...");
    let retrieved = filestore_backend.get_collection_record_by_name("test_collection").await?;
    match retrieved {
        Some(retrieved_record) => {
            println!("   ✅ Collection retrieved from memory: {} (UUID: {})", retrieved_record.name, retrieved_record.uuid);
        }
        None => {
            println!("   ❌ Collection NOT found in memory!");
        }
    }
    
    // Test list operation
    println!("\n3️⃣ Testing list_collections...");
    let collections = filestore_backend.list_collections(None).await?;
    println!("   📋 Collections in memory: {}", collections.len());
    for col in &collections {
        println!("      - {} (UUID: {})", col.name, col.uuid);
    }
    
    println!("\n🔍 Summary:");
    println!("   - Memory operations work: {}", collections.len() > 0);
    println!("   - Files created: {}", after_upsert_files > initial_files);
    println!("   - Final file count: {}", after_upsert_files);
    
    if after_upsert_files == initial_files {
        println!("\n❌ ISSUE: No files were created on disk!");
        println!("   This confirms the bug - operations work in memory but files aren't written.");
    } else {
        println!("\n✅ Files were created successfully!");
    }
    
    Ok(())
}