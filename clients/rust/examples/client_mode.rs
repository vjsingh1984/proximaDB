//! Example: Using ProximaDB Rust SDK in client mode
//!
//! This example demonstrates connecting to a remote ProximaDB server
//! and performing vector database operations.
//!
//! Prerequisites:
//! 1. Start a ProximaDB server: cargo run --release --bin proximadb-server
//! 2. Run this example: cargo run --example client_mode --features client

use proximadb_sdk::{StorageEngine, connect};

#[tokio::main]
async fn main() -> proximadb_sdk::Result<()> {
    println!("ProximaDB Rust SDK - Client Mode Example");
    println!("=========================================\n");

    // Connect to the server
    let client = connect("http://localhost:5678")?;
    println!("Connected to: {}\n", client.url());

    // Check server health
    match client.health().await {
        Ok(health) => {
            println!("Server Status: {}", health.status);
            if let Some(version) = health.version {
                println!("Server Version: {}", version);
            }
        }
        Err(e) => {
            println!("Warning: Could not check server health: {}", e);
        }
    }

    // Collection name for this example
    let collection_name = "rust_sdk_example";

    // Try to delete existing collection (ignore errors)
    let _ = client.delete_collection(collection_name).await;

    // Create a collection
    println!("\n1. Creating collection '{}'...", collection_name);
    client
        .create_collection(collection_name)
        .dimension(128)
        .engine(StorageEngine::Sst)
        .execute()
        .await?;
    println!("   Collection created successfully!");

    // Insert some vectors
    println!("\n2. Inserting vectors...");

    // Single vector insert with metadata
    client
        .collection(collection_name)
        .insert()
        .id("vec_001")
        .vector(&generate_random_vector(128, 0.1))
        .meta("category", "tech")
        .meta("source", "api")
        .execute()
        .await?;

    client
        .collection(collection_name)
        .insert()
        .id("vec_002")
        .vector(&generate_random_vector(128, 0.2))
        .meta("category", "science")
        .meta("source", "web")
        .execute()
        .await?;

    client
        .collection(collection_name)
        .insert()
        .id("vec_003")
        .vector(&generate_random_vector(128, 0.3))
        .meta("category", "tech")
        .meta("source", "api")
        .execute()
        .await?;

    println!("   Inserted 3 vectors");

    // Batch insert
    println!("\n3. Batch inserting vectors...");
    let batch_ids: Vec<String> = (4..=10).map(|i| format!("vec_{:03}", i)).collect();
    let batch_vectors: Vec<Vec<f32>> = (4..=10)
        .map(|i| generate_random_vector(128, i as f32 * 0.1))
        .collect();

    client
        .collection(collection_name)
        .insert()
        .batch(batch_ids, batch_vectors)?
        .execute()
        .await?;
    println!("   Batch inserted 7 more vectors");

    // Search for similar vectors
    println!("\n4. Searching for similar vectors...");
    let query_vector = generate_random_vector(128, 0.15);

    let results = client
        .collection(collection_name)
        .search()
        .vector(&query_vector)
        .top_k(5)
        .execute()
        .await?;

    println!("   Found {} results:", results.len());
    for result in &results {
        println!(
            "     - ID: {}, Score: {:.4}, Metadata: {:?}",
            result.id, result.score, result.metadata
        );
    }

    // Search with filter
    println!("\n5. Searching with filter (category = 'tech')...");
    let results = client
        .collection(collection_name)
        .search()
        .vector(&query_vector)
        .top_k(5)
        .filter("category = 'tech'")
        .execute()
        .await?;

    println!("   Found {} results:", results.len());
    for result in &results {
        println!("     - ID: {}, Score: {:.4}", result.id, result.score);
    }

    // Search with approximate mode
    println!("\n6. Approximate search (faster, ~95% recall)...");
    let results = client
        .collection(collection_name)
        .search()
        .vector(&query_vector)
        .top_k(5)
        .approximate()
        .execute()
        .await?;

    println!("   Found {} results:", results.len());

    // Get collection info
    println!("\n7. Getting collection info...");
    let info = client.collection(collection_name).info().await?;
    println!("   Name: {}", info.name);
    println!("   Dimension: {}", info.dimension);
    println!("   Vector Count: {}", info.vector_count);

    // Cleanup
    println!("\n8. Cleaning up...");
    client.delete_collection(collection_name).await?;
    println!("   Collection deleted");

    println!("\nExample completed successfully!");
    Ok(())
}

/// Generate a pseudo-random vector for demonstration
fn generate_random_vector(dimension: usize, seed: f32) -> Vec<f32> {
    (0..dimension)
        .map(|i| {
            let x = seed + (i as f32 * 0.1);
            (x.sin() * 0.5 + 0.5).abs()
        })
        .collect()
}
