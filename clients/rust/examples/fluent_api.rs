//! Example: Fluent API patterns in ProximaDB Rust SDK
//!
//! This example showcases the fluent builder pattern APIs for
//! creating collections, inserting vectors, and searching.
//!
//! Run with: cargo run --example fluent_api --features client

use proximadb_sdk::{ClientBuilder, DistanceMetric, IndexType, ProximaClient, StorageEngine};

#[tokio::main]
async fn main() -> proximadb_sdk::Result<()> {
    println!("ProximaDB Rust SDK - Fluent API Examples");
    println!("========================================\n");

    // Example 1: Client Builder Pattern
    println!("1. Client Builder Pattern");
    println!("   -----------------------");

    // Simple connection
    let _client = ProximaClient::connect("http://localhost:5678")?;
    println!("   Simple: ProximaClient::connect(url)");

    // Builder with full configuration
    let client = ClientBuilder::new()
        .url("http://localhost:5678")
        .timeout_ms(5000)
        .max_retries(3)
        .pool_connections(true)
        .max_idle_connections(10)
        // .api_key("your-api-key")  // Optional authentication
        .build()?;
    println!("   Full builder: ClientBuilder::new().url().timeout_ms()...build()");

    // Example 2: Collection Builder Pattern
    println!("\n2. Collection Builder Pattern");
    println!("   ---------------------------");

    let collection_name = "fluent_api_demo";

    // Clean up from previous run
    let _ = client.delete_collection(collection_name).await;

    // Create collection with fluent API
    println!("   Creating collection with fluent API...");

    client
        .create_collection(collection_name)
        .dimension(512) // Vector dimension
        .engine(StorageEngine::Sst) // Storage engine (SST, HELIX, VIPER, etc.)
        .index(IndexType::Hnsw) // Index type (HNSW, IVF, LSH, Flat)
        .metric(DistanceMetric::Cosine) // Distance metric
        .execute()
        .await?;

    println!("   client.create_collection(\"{}\")", collection_name);
    println!("       .dimension(512)");
    println!("       .engine(StorageEngine::Sst)");
    println!("       .index(IndexType::Hnsw)");
    println!("       .metric(DistanceMetric::Cosine)");
    println!("       .execute().await?;");

    // Example 3: Insert Builder Pattern
    println!("\n3. Insert Builder Pattern");
    println!("   -----------------------");

    // Single vector insert with fluent metadata
    let embedding = vec![0.1f32; 512];

    client
        .collection(collection_name)
        .insert()
        .id("doc_001")
        .vector(&embedding)
        .meta("title", "Introduction to Rust")
        .meta("author", "Jane Doe")
        .meta("category", "programming")
        .meta("rating", "5")
        .execute()
        .await?;

    println!("   client.collection(\"{}\").insert()", collection_name);
    println!("       .id(\"doc_001\")");
    println!("       .vector(&embedding)");
    println!("       .meta(\"title\", \"Introduction to Rust\")");
    println!("       .meta(\"author\", \"Jane Doe\")");
    println!("       .execute().await?;");

    // Batch insert with fluent API
    let ids = vec!["doc_002".to_string(), "doc_003".to_string()];
    let vectors = vec![vec![0.2f32; 512], vec![0.3f32; 512]];

    client
        .collection(collection_name)
        .insert()
        .batch(ids, vectors)?
        .execute()
        .await?;

    println!("\n   Batch insert:");
    println!("   client.collection(\"{}\").insert()", collection_name);
    println!("       .batch(ids, vectors)?");
    println!("       .execute().await?;");

    // Example 4: Search Builder Pattern
    println!("\n4. Search Builder Pattern");
    println!("   ----------------------");

    let query = vec![0.15f32; 512];

    // Basic search
    let results = client
        .collection(collection_name)
        .search()
        .vector(&query)
        .top_k(10)
        .execute()
        .await?;

    println!("   Basic search found {} results", results.len());
    println!("   client.collection(\"{}\").search()", collection_name);
    println!("       .vector(&query)");
    println!("       .top_k(10)");
    println!("       .execute().await?;");

    // Search with filter
    let results = client
        .collection(collection_name)
        .search()
        .vector(&query)
        .top_k(5)
        .filter("category = 'programming'")
        .execute()
        .await?;

    println!("\n   Filtered search found {} results", results.len());
    println!("   client.collection(\"{}\").search()", collection_name);
    println!("       .vector(&query)");
    println!("       .top_k(5)");
    println!("       .filter(\"category = 'programming'\")");
    println!("       .execute().await?;");

    // Search with search mode
    let results = client
        .collection(collection_name)
        .search()
        .vector(&query)
        .top_k(5)
        .approximate_with_nprobe(5)
        .include_metadata(true)
        .min_score(0.5)
        .execute()
        .await?;

    println!("\n   Advanced search found {} results", results.len());
    println!("   client.collection(\"{}\").search()", collection_name);
    println!("       .vector(&query)");
    println!("       .top_k(5)");
    println!("       .approximate_with_nprobe(5)");
    println!("       .include_metadata(true)");
    println!("       .min_score(0.5)");
    println!("       .execute().await?;");

    // Example 5: Search Mode Variations
    println!("\n5. Search Mode Variations");
    println!("   ----------------------");

    // Exact search (100% recall)
    let _ = client
        .collection(collection_name)
        .search()
        .vector(&query)
        .exact()
        .execute()
        .await?;
    println!("   .exact() - 100% recall, searches all partitions");

    // Approximate search (faster, ~95% recall)
    let _ = client
        .collection(collection_name)
        .search()
        .vector(&query)
        .approximate()
        .execute()
        .await?;
    println!("   .approximate() - Faster with ~95% recall");

    // Approximate with custom nprobe
    let _ = client
        .collection(collection_name)
        .search()
        .vector(&query)
        .approximate_with_nprobe(10)
        .execute()
        .await?;
    println!("   .approximate_with_nprobe(10) - Custom partition count");

    // Adaptive search
    let _ = client
        .collection(collection_name)
        .search()
        .vector(&query)
        .adaptive(10000)
        .execute()
        .await?;
    println!("   .adaptive(10000) - Auto-selects based on dataset size");

    // Example 6: Collection Handle Operations
    println!("\n6. Collection Handle Operations");
    println!("   ----------------------------");

    let collection = client.collection(collection_name);
    println!(
        "   let collection = client.collection(\"{}\");",
        collection_name
    );

    let count = collection.count().await?;
    println!("   collection.count() -> {}", count);

    let info = collection.info().await?;
    println!("   collection.info() -> dimension: {}", info.dimension);

    // Cleanup
    println!("\n7. Cleanup");
    println!("   -------");
    client.delete_collection(collection_name).await?;
    println!("   Collection deleted");

    println!("\nFluent API examples completed!");
    Ok(())
}
