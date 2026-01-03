//! Example: Using ProximaDB Rust SDK in embedded mode
//!
//! This example demonstrates using ProximaDB as an in-process database
//! without any network overhead - ideal for agent memory systems.
//!
//! Prerequisites:
//! 1. Build with embedded feature: cargo build --features embedded
//! 2. Run: cargo run --example embedded_mode --features embedded

use proximadb_sdk::{ProximaDB, SearchMode, StorageEngine};
use std::collections::HashMap;

fn main() -> proximadb_sdk::Result<()> {
    println!("ProximaDB Rust SDK - Embedded Mode Example");
    println!("==========================================\n");

    // Create a temporary directory for this example
    let temp_dir = std::env::temp_dir().join("proximadb_rust_example");
    let data_dir = temp_dir.to_string_lossy().to_string();

    // Clean up any previous run
    let _ = std::fs::remove_dir_all(&temp_dir);
    std::fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    println!("Data directory: {}\n", data_dir);

    // Open an embedded database
    println!("1. Opening embedded database...");
    let db = ProximaDB::embedded()
        .data_dir(&data_dir)
        .cache_size_mb(256)
        .default_engine(StorageEngine::Sst)
        .enable_wal(true)
        .enable_rl_planner(false) // Disable for faster startup in example
        .open()?;
    println!("   Database opened successfully!");

    // Create a collection for agent memories
    let collection_name = "agent_memories";
    println!("\n2. Creating collection '{}'...", collection_name);
    db.create_collection(collection_name)
        .dimension(768)
        .engine(StorageEngine::Sst)
        .execute_sync()?;
    println!("   Collection created!");

    // Insert some memory vectors
    println!("\n3. Inserting agent memories...");

    // Simulate embedding vectors for different memory types
    let conversation_embedding = generate_embedding(768, "conversation");
    let task_embedding = generate_embedding(768, "task");
    let knowledge_embedding = generate_embedding(768, "knowledge");

    // Insert individual memories
    db.collection(collection_name)
        .insert()
        .id("mem_conv_001")
        .vector(&conversation_embedding)
        .meta("type", "conversation")
        .meta("timestamp", "1704067200")
        .meta("summary", "User asked about vector databases")
        .execute_sync()?;

    db.collection(collection_name)
        .insert()
        .id("mem_task_001")
        .vector(&task_embedding)
        .meta("type", "task")
        .meta("status", "completed")
        .meta("summary", "Analyzed codebase structure")
        .execute_sync()?;

    db.collection(collection_name)
        .insert()
        .id("mem_know_001")
        .vector(&knowledge_embedding)
        .meta("type", "knowledge")
        .meta("topic", "rust")
        .meta("summary", "Rust ownership model explanation")
        .execute_sync()?;

    println!("   Inserted 3 memories");

    // Batch insert more memories
    println!("\n4. Batch inserting memories...");
    let batch_ids: Vec<String> = (1..=5).map(|i| format!("mem_batch_{:03}", i)).collect();
    let batch_vectors: Vec<Vec<f32>> = (1..=5)
        .map(|i| generate_embedding(768, &format!("batch_{}", i)))
        .collect();

    db.collection(collection_name)
        .insert()
        .batch(batch_ids, batch_vectors)?
        .execute_sync()?;
    println!("   Batch inserted 5 memories");

    // Search for relevant memories
    println!("\n5. Searching for relevant memories...");
    let query_embedding = generate_embedding(768, "conversation");

    let results = db
        .collection(collection_name)
        .search_embedded()
        .vector(&query_embedding)
        .top_k(5)
        .execute_sync()?;

    println!("   Found {} relevant memories:", results.len());
    for result in &results {
        let memory_type = result.get_meta("type").unwrap_or("unknown");
        let summary = result.get_meta("summary").unwrap_or("");
        println!(
            "     - {} (score: {:.4}): {}",
            result.id, result.score, summary
        );
    }

    // Search with different modes
    println!("\n6. Comparing search modes...");

    // Exact search
    let exact_results = db
        .collection(collection_name)
        .search_embedded()
        .vector(&query_embedding)
        .top_k(3)
        .mode(SearchMode::Exact)
        .execute_sync()?;
    println!("   Exact search: {} results", exact_results.len());

    // Approximate search
    let approx_results = db
        .collection(collection_name)
        .search_embedded()
        .vector(&query_embedding)
        .top_k(3)
        .approximate()
        .execute_sync()?;
    println!("   Approximate search: {} results", approx_results.len());

    // Get storage stats
    println!("\n7. Storage statistics...");
    let stats = db.storage_stats()?;
    println!("   Total vectors: {}", stats.total_vectors);
    println!("   Total collections: {}", stats.total_collections);
    println!("   Disk usage: {} bytes", stats.disk_usage_bytes);
    println!("   Cache hit rate: {:.2}%", stats.cache_hit_rate * 100.0);

    // Flush and close
    println!("\n8. Flushing and closing database...");
    db.flush()?;
    println!("   Data flushed to disk");
    db.close()?;
    println!("   Database closed");

    // Cleanup temp directory
    let _ = std::fs::remove_dir_all(&temp_dir);

    println!("\nExample completed successfully!");
    Ok(())
}

/// Generate a pseudo-embedding vector for demonstration
fn generate_embedding(dimension: usize, seed: &str) -> Vec<f32> {
    let seed_hash: u32 = seed.bytes().fold(0u32, |acc, b| acc.wrapping_add(b as u32));
    (0..dimension)
        .map(|i| {
            let x = (seed_hash as f32 + i as f32) * 0.01;
            (x.sin() * 0.5 + 0.5).abs()
        })
        .collect()
}
