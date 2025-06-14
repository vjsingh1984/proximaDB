# ProximaDB Rust Client Examples

Complete examples for using ProximaDB with Rust clients for both REST and gRPC APIs.

## Dependencies

Add these to your `Cargo.toml`:

```toml
[dependencies]
# Official ProximaDB Rust Client
proximadb-client = "0.1.0"

# For manual REST integration
reqwest = { version = "0.11", features = ["json"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
tokio = { version = "1.0", features = ["full"] }

# For gRPC integration
tonic = "0.10"
prost = "0.12"
tonic-build = "0.10"

# Utilities
uuid = { version = "1.0", features = ["v4", "serde"] }
thiserror = "1.0"
anyhow = "1.0"
```

## REST API Examples

### Basic Setup

```rust
use reqwest::{Client, Response};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use thiserror::Error;
use uuid::Uuid;

#[derive(Error, Debug)]
pub enum ProximaDBError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("API error {code}: {message}")]
    Api { code: String, message: String },
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

pub struct ProximaDBRestClient {
    client: Client,
    base_url: String,
    api_key: Option<String>,
}

impl ProximaDBRestClient {
    pub fn new(base_url: impl Into<String>, api_key: Option<String>) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key,
        }
    }

    pub async fn request(
        &self,
        method: reqwest::Method,
        endpoint: &str,
        data: Option<Value>,
    ) -> Result<Value, ProximaDBError> {
        let url = format!("{}{}", self.base_url, endpoint);
        let mut request_builder = self.client.request(method, &url)
            .header("Content-Type", "application/json");

        if let Some(ref api_key) = self.api_key {
            request_builder = request_builder.bearer_auth(api_key);
        }

        if let Some(data) = data {
            request_builder = request_builder.json(&data);
        }

        let response = request_builder.send().await?;
        
        if !response.status().is_success() {
            let error_body: Value = response.json().await?;
            if let Some(error) = error_body.get("error") {
                return Err(ProximaDBError::Api {
                    code: error.get("code")
                        .and_then(|c| c.as_str())
                        .unwrap_or("UNKNOWN_ERROR")
                        .to_string(),
                    message: error.get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("Unknown error")
                        .to_string(),
                });
            }
        }

        Ok(response.json().await?)
    }
}

// Initialize client
async fn create_client() -> ProximaDBRestClient {
    ProximaDBRestClient::new(
        "http://localhost:5678",
        Some("pk_live_1234567890abcdef".to_string()),
    )
}
```

### Collection Management

```rust
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct CreateCollectionRequest {
    pub collection_id: String,
    pub dimension: u32,
    pub distance_metric: String,
    pub description: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Collection {
    pub id: String,
    pub name: String,
    pub dimension: u32,
    pub distance_metric: String,
    pub vector_count: u64,
    pub created_at: String,
    pub updated_at: String,
}

pub struct CollectionExamples {
    client: ProximaDBRestClient,
}

impl CollectionExamples {
    pub fn new(client: ProximaDBRestClient) -> Self {
        Self { client }
    }

    // Create a collection
    pub async fn create_collection_example(&self) -> Result<Value, ProximaDBError> {
        let collection_data = json!({
            "collection_id": "rust_embeddings",
            "dimension": 384,
            "distance_metric": "cosine",
            "description": "Rust document embeddings"
        });

        match self.client.request(
            reqwest::Method::POST,
            "/collections",
            Some(collection_data),
        ).await {
            Ok(result) => {
                println!("Collection created: {}", 
                    result.get("collection_id").unwrap().as_str().unwrap());
                Ok(result)
            }
            Err(ProximaDBError::Api { code, .. }) if code == "COLLECTION_ALREADY_EXISTS" => {
                println!("Collection already exists, continuing...");
                Ok(json!({"collection_id": "rust_embeddings"}))
            }
            Err(e) => Err(e),
        }
    }

    // List all collections
    pub async fn list_collections_example(&self) -> Result<Value, ProximaDBError> {
        let collections = self.client.request(
            reqwest::Method::GET,
            "/collections",
            None,
        ).await?;

        if let Some(collection_list) = collections.get("collections").and_then(|c| c.as_array()) {
            println!("Found {} collections:", collection_list.len());
            for collection in collection_list {
                println!("- {}: {} vectors",
                    collection.get("id").unwrap().as_str().unwrap(),
                    collection.get("vector_count").unwrap().as_u64().unwrap()
                );
            }
        }

        Ok(collections)
    }

    // Get collection details
    pub async fn get_collection_example(&self, collection_id: &str) -> Result<Value, ProximaDBError> {
        let endpoint = format!("/collections/{}", collection_id);
        let collection = self.client.request(
            reqwest::Method::GET,
            &endpoint,
            None,
        ).await?;

        println!("Collection {}:", collection.get("id").unwrap().as_str().unwrap());
        println!("  Dimension: {}", collection.get("dimension").unwrap().as_u64().unwrap());
        println!("  Vectors: {}", collection.get("vector_count").unwrap().as_u64().unwrap());
        println!("  Size: {} bytes", collection.get("total_size_bytes").unwrap().as_u64().unwrap());

        Ok(collection)
    }

    // Delete a collection
    pub async fn delete_collection_example(&self, collection_id: &str) -> Result<Value, ProximaDBError> {
        let endpoint = format!("/collections/{}", collection_id);
        let result = self.client.request(
            reqwest::Method::DELETE,
            &endpoint,
            None,
        ).await?;

        println!("Collection deleted: {}", result.get("message").unwrap().as_str().unwrap());
        Ok(result)
    }
}
```

### Vector Operations

```rust
use rand::Rng;

#[derive(Serialize, Deserialize, Debug)]
pub struct VectorData {
    pub id: Option<String>,
    pub vector: Vec<f64>,
    pub metadata: HashMap<String, Value>,
}

pub struct VectorExamples {
    client: ProximaDBRestClient,
}

impl VectorExamples {
    pub fn new(client: ProximaDBRestClient) -> Self {
        Self { client }
    }

    // Generate random vector for testing
    fn generate_random_vector(&self, dimension: usize) -> Vec<f64> {
        let mut rng = rand::thread_rng();
        (0..dimension).map(|_| rng.gen::<f64>()).collect()
    }

    // Insert a single vector
    pub async fn insert_vector_example(&self) -> Result<Value, ProximaDBError> {
        let mut metadata = HashMap::new();
        metadata.insert("document_id".to_string(), json!("rust_doc_001"));
        metadata.insert("title".to_string(), json!("Rust Example Document"));
        metadata.insert("category".to_string(), json!("programming"));
        metadata.insert("tags".to_string(), json!(["rust", "systems", "api"]));

        let vector_data = json!({
            "id": Uuid::new_v4().to_string(),
            "vector": self.generate_random_vector(384),
            "metadata": metadata
        });

        let result = self.client.request(
            reqwest::Method::POST,
            "/collections/rust_embeddings/vectors",
            Some(vector_data),
        ).await?;

        println!("Vector inserted: {}", result.get("vector_id").unwrap().as_str().unwrap());
        Ok(result)
    }

    // Get a vector by ID
    pub async fn get_vector_example(&self, collection_id: &str, vector_id: &str) -> Result<Value, ProximaDBError> {
        let endpoint = format!("/collections/{}/vectors/{}", collection_id, vector_id);
        let vector = self.client.request(
            reqwest::Method::GET,
            &endpoint,
            None,
        ).await?;

        println!("Vector {}:", vector.get("id").unwrap().as_str().unwrap());
        println!("  Dimension: {}", vector.get("vector").unwrap().as_array().unwrap().len());
        println!("  Metadata: {}", vector.get("metadata").unwrap());

        Ok(vector)
    }

    // Search for similar vectors
    pub async fn search_vectors_example(&self) -> Result<Value, ProximaDBError> {
        let query_vector = self.generate_random_vector(384);

        let search_data = json!({
            "vector": query_vector,
            "k": 10,
            "filter": {
                "category": "programming"
            },
            "threshold": 0.7
        });

        let results = self.client.request(
            reqwest::Method::POST,
            "/collections/rust_embeddings/search",
            Some(search_data),
        ).await?;

        println!("Found {} similar vectors:", results.get("total_count").unwrap().as_u64().unwrap());
        if let Some(result_list) = results.get("results").and_then(|r| r.as_array()) {
            for result in result_list {
                println!("- {}: score={:.3}",
                    result.get("id").unwrap().as_str().unwrap(),
                    result.get("score").unwrap().as_f64().unwrap()
                );
                if let Some(metadata) = result.get("metadata") {
                    if let Some(title) = metadata.get("title") {
                        println!("  Title: {}", title.as_str().unwrap_or("N/A"));
                    }
                }
            }
        }

        Ok(results)
    }

    // Delete a vector
    pub async fn delete_vector_example(&self, collection_id: &str, vector_id: &str) -> Result<Value, ProximaDBError> {
        let endpoint = format!("/collections/{}/vectors/{}", collection_id, vector_id);
        let result = self.client.request(
            reqwest::Method::DELETE,
            &endpoint,
            None,
        ).await?;

        println!("Vector deleted: {}", result.get("message").unwrap().as_str().unwrap());
        Ok(result)
    }
}
```

### Batch Operations

```rust
pub struct BatchExamples {
    client: ProximaDBRestClient,
}

impl BatchExamples {
    pub fn new(client: ProximaDBRestClient) -> Self {
        Self { client }
    }

    fn generate_random_vector(&self, dimension: usize) -> Vec<f64> {
        let mut rng = rand::thread_rng();
        (0..dimension).map(|_| rng.gen::<f64>()).collect()
    }

    // Batch insert vectors
    pub async fn batch_insert_example(&self) -> Result<Value, ProximaDBError> {
        let mut vectors = Vec::new();

        for i in 0..100 {
            let mut metadata = HashMap::new();
            metadata.insert("document_id".to_string(), json!(format!("rust_doc_{:03}", i)));
            metadata.insert("batch".to_string(), json!("rust_example_batch"));
            metadata.insert("index".to_string(), json!(i));

            let vector_data = json!({
                "id": Uuid::new_v4().to_string(),
                "vector": self.generate_random_vector(384),
                "metadata": metadata
            });

            vectors.push(vector_data);
        }

        let batch_data = json!({
            "vectors": vectors
        });

        let result = self.client.request(
            reqwest::Method::POST,
            "/collections/rust_embeddings/vectors/batch",
            Some(batch_data),
        ).await?;

        println!("Batch inserted {} vectors", result.get("inserted_count").unwrap().as_u64().unwrap());
        if let Some(errors) = result.get("errors").and_then(|e| e.as_array()) {
            if !errors.is_empty() {
                println!("Errors: {:?}", errors);
            }
        }

        Ok(result)
    }

    // Batch search
    pub async fn batch_search_example(&self) -> Result<Value, ProximaDBError> {
        let mut searches = Vec::new();

        for _ in 0..5 {
            let search = json!({
                "collection_id": "rust_embeddings",
                "vector": self.generate_random_vector(384),
                "k": 5,
                "filter": {
                    "batch": "rust_example_batch"
                }
            });
            searches.push(search);
        }

        let batch_data = json!({
            "searches": searches
        });

        let results = self.client.request(
            reqwest::Method::POST,
            "/batch/search",
            Some(batch_data),
        ).await?;

        println!("Performed {} searches:", results.get("results").unwrap().as_array().unwrap().len());
        if let Some(result_list) = results.get("results").and_then(|r| r.as_array()) {
            for (i, search_result) in result_list.iter().enumerate() {
                println!("  Search {}: {} results", 
                    i, search_result.get("total_count").unwrap().as_u64().unwrap());
            }
        }

        Ok(results)
    }
}
```

### Complete Example Application

```rust
/**
 * ProximaDB REST API Example Application
 * Demonstrates document similarity search using embeddings.
 */

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct Document {
    pub title: String,
    pub content: String,
    pub author: Option<String>,
    pub category: Option<String>,
}

#[derive(Debug)]
pub struct SearchResult {
    pub id: String,
    pub score: f64,
    pub title: String,
    pub author: String,
    pub content: String,
}

pub struct DocumentSearchApp {
    client: ProximaDBRestClient,
    collection_id: String,
}

impl DocumentSearchApp {
    pub fn new(api_key: Option<String>) -> Self {
        Self {
            client: ProximaDBRestClient::new("http://localhost:5678", api_key),
            collection_id: "rust_document_search".to_string(),
        }
    }

    pub async fn setup(&self) -> Result<(), ProximaDBError> {
        let collection_data = json!({
            "collection_id": &self.collection_id,
            "dimension": 384,
            "distance_metric": "cosine",
            "description": "Rust document similarity search"
        });

        match self.client.request(
            reqwest::Method::POST,
            "/collections",
            Some(collection_data),
        ).await {
            Ok(_) => println!("Collection created successfully"),
            Err(ProximaDBError::Api { code, .. }) if code == "COLLECTION_ALREADY_EXISTS" => {
                println!("Collection already exists");
            }
            Err(e) => return Err(e),
        }

        Ok(())
    }

    // Simulate text embeddings (in practice, use a real embedding model)
    fn generate_simulated_embedding(&self, text: &str) -> Vec<f64> {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        // Simple hash-based simulation for demo purposes
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        let seed = hasher.finish();
        
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
        use rand::Rng;
        (0..384).map(|_| rng.gen_range(-1.0..1.0)).collect()
    }

    pub async fn add_documents(&self, documents: Vec<Document>) -> Result<Value, ProximaDBError> {
        let mut vectors = Vec::new();

        for doc in documents {
            // Generate embedding (simulate with hash-based random)
            let embedding = self.generate_simulated_embedding(&doc.content);

            let mut metadata = HashMap::new();
            metadata.insert("title".to_string(), json!(doc.title));
            metadata.insert("content".to_string(), json!(
                if doc.content.len() > 200 {
                    &doc.content[..200]
                } else {
                    &doc.content
                }
            ));
            metadata.insert("author".to_string(), json!(doc.author.unwrap_or_else(|| "Unknown".to_string())));
            metadata.insert("category".to_string(), json!(doc.category.unwrap_or_else(|| "general".to_string())));

            let vector_data = json!({
                "vector": embedding,
                "metadata": metadata
            });

            vectors.push(vector_data);
        }

        // Batch insert
        let batch_data = json!({
            "vectors": vectors
        });

        let result = self.client.request(
            reqwest::Method::POST,
            &format!("/collections/{}/vectors/batch", self.collection_id),
            Some(batch_data),
        ).await?;

        println!("Added {} documents to search index", 
            result.get("inserted_count").unwrap().as_u64().unwrap());
        Ok(result)
    }

    pub async fn search_documents(
        &self,
        query: &str,
        k: usize,
        category_filter: Option<&str>,
    ) -> Result<Vec<SearchResult>, ProximaDBError> {
        // Generate query embedding
        let query_embedding = self.generate_simulated_embedding(query);

        let mut search_data = json!({
            "vector": query_embedding,
            "k": k,
            "threshold": 0.3
        });

        // Add category filter if specified
        if let Some(category) = category_filter {
            search_data["filter"] = json!({
                "category": category
            });
        }

        let results = self.client.request(
            reqwest::Method::POST,
            &format!("/collections/{}/search", self.collection_id),
            Some(search_data),
        ).await?;

        let mut documents = Vec::new();
        if let Some(result_list) = results.get("results").and_then(|r| r.as_array()) {
            for result in result_list {
                let doc = SearchResult {
                    id: result.get("id").unwrap().as_str().unwrap().to_string(),
                    score: result.get("score").unwrap().as_f64().unwrap(),
                    title: result.get("metadata").unwrap().get("title").unwrap().as_str().unwrap().to_string(),
                    author: result.get("metadata").unwrap().get("author").unwrap().as_str().unwrap().to_string(),
                    content: result.get("metadata").unwrap().get("content").unwrap().as_str().unwrap().to_string(),
                };
                documents.push(doc);
            }
        }

        Ok(documents)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize the application
    let app = DocumentSearchApp::new(Some("pk_test_example123".to_string()));
    app.setup().await?;

    // Sample documents
    let documents = vec![
        Document {
            title: "Introduction to Machine Learning".to_string(),
            content: "Machine learning is a subset of artificial intelligence that focuses on algorithms that can learn from data.".to_string(),
            author: Some("Dr. Smith".to_string()),
            category: Some("education".to_string()),
        },
        Document {
            title: "Deep Learning Fundamentals".to_string(),
            content: "Deep learning uses neural networks with multiple layers to model complex patterns in data.".to_string(),
            author: Some("Prof. Johnson".to_string()),
            category: Some("education".to_string()),
        },
        Document {
            title: "Rust Programming Guide".to_string(),
            content: "Rust is a systems programming language that focuses on safety, speed, and concurrency.".to_string(),
            author: Some("Rust Expert".to_string()),
            category: Some("programming".to_string()),
        },
        Document {
            title: "WebAssembly with Rust".to_string(),
            content: "WebAssembly allows Rust code to run in web browsers with near-native performance.".to_string(),
            author: Some("WASM Guru".to_string()),
            category: Some("programming".to_string()),
        },
    ];

    // Add documents to the index
    app.add_documents(documents).await?;

    // Search for documents
    println!("\n--- Search Results ---");
    let queries = [
        "artificial intelligence algorithms",
        "neural networks and deep learning",
        "systems programming languages",
        "web assembly performance",
    ];

    for query in &queries {
        println!("\nQuery: '{}'", query);
        let results = app.search_documents(query, 3, None).await?;

        for (i, doc) in results.iter().enumerate() {
            println!("{}. {} (score: {:.3})", i + 1, doc.title, doc.score);
            println!("   Author: {}", doc.author);
            println!("   Content: {}...", doc.content);
        }
    }

    Ok(())
}
```

## gRPC API Examples

### Basic Setup

```rust
use tonic::{transport::Channel, Request};
use proximadb_proto::vector_db_client::VectorDbClient;
use proximadb_proto::*;

pub mod proximadb_proto {
    tonic::include_proto!("proximadb");
}

pub struct ProximaDBGrpcClient {
    client: VectorDbClient<Channel>,
}

impl ProximaDBGrpcClient {
    pub async fn new(endpoint: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let channel = Channel::from_static(endpoint).connect().await?;
        let client = VectorDbClient::new(channel);
        Ok(Self { client })
    }

    pub async fn with_auth(endpoint: &str, api_key: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let channel = Channel::from_static(endpoint).connect().await?;
        let client = VectorDbClient::with_interceptor(channel, move |mut req: Request<()>| {
            req.metadata_mut().insert(
                "authorization",
                format!("Bearer {}", api_key).parse().unwrap(),
            );
            Ok(req)
        });
        Ok(Self { client })
    }
}

// Initialize client
async fn create_grpc_client() -> Result<ProximaDBGrpcClient, Box<dyn std::error::Error>> {
    ProximaDBGrpcClient::with_auth(
        "http://localhost:9090",
        "pk_live_1234567890abcdef"
    ).await
}
```

### Collection Management with gRPC

```rust
use prost_types::Struct;
use std::collections::HashMap;

pub struct GrpcCollectionExamples {
    client: ProximaDBGrpcClient,
}

impl GrpcCollectionExamples {
    pub fn new(client: ProximaDBGrpcClient) -> Self {
        Self { client }
    }

    pub async fn create_collection_grpc(&mut self) -> Result<CreateCollectionResponse, Box<dyn std::error::Error>> {
        let request = Request::new(CreateCollectionRequest {
            collection_id: "grpc_rust_embeddings".to_string(),
            name: "gRPC Rust Document Embeddings".to_string(),
            dimension: 384,
            schema_type: SchemaType::Document as i32,
            schema: None,
        });

        let response = self.client.client.create_collection(request).await?;
        println!("Collection created: {}", response.get_ref().success);
        Ok(response.into_inner())
    }

    pub async fn list_collections_grpc(&mut self) -> Result<ListCollectionsResponse, Box<dyn std::error::Error>> {
        let request = Request::new(ListCollectionsRequest {});
        let response = self.client.client.list_collections(request).await?;
        
        let collections = &response.get_ref().collections;
        println!("Found {} collections:", collections.len());
        for collection in collections {
            println!("- {}: {} vectors", collection.id, collection.vector_count);
        }

        Ok(response.into_inner())
    }

    pub async fn delete_collection_grpc(&mut self, collection_id: &str) -> Result<DeleteCollectionResponse, Box<dyn std::error::Error>> {
        let request = Request::new(DeleteCollectionRequest {
            collection_id: collection_id.to_string(),
        });

        let response = self.client.client.delete_collection(request).await?;
        println!("Collection deleted: {}", response.get_ref().success);
        Ok(response.into_inner())
    }
}
```

### Vector Operations with gRPC

```rust
use prost_types::{Struct, Value};
use uuid::Uuid;
use rand::Rng;

pub struct GrpcVectorExamples {
    client: ProximaDBGrpcClient,
}

impl GrpcVectorExamples {
    pub fn new(client: ProximaDBGrpcClient) -> Self {
        Self { client }
    }

    fn create_metadata_struct(&self, metadata_map: HashMap<String, Value>) -> Struct {
        Struct {
            fields: metadata_map,
        }
    }

    fn generate_random_vector(&self, dimension: usize) -> Vec<f32> {
        let mut rng = rand::thread_rng();
        (0..dimension).map(|_| rng.gen::<f32>()).collect()
    }

    pub async fn insert_vector_grpc(&mut self) -> Result<InsertResponse, Box<dyn std::error::Error>> {
        // Create metadata
        let mut metadata_fields = HashMap::new();
        metadata_fields.insert("document_id".to_string(), Value {
            kind: Some(prost_types::value::Kind::StringValue("grpc_rust_doc_001".to_string())),
        });
        metadata_fields.insert("title".to_string(), Value {
            kind: Some(prost_types::value::Kind::StringValue("gRPC Rust Example Document".to_string())),
        });
        metadata_fields.insert("category".to_string(), Value {
            kind: Some(prost_types::value::Kind::StringValue("technical".to_string())),
        });

        let metadata = self.create_metadata_struct(metadata_fields);

        // Create vector record
        let record = VectorRecord {
            id: Uuid::new_v4().to_string(),
            collection_id: "grpc_rust_embeddings".to_string(),
            vector: self.generate_random_vector(384),
            metadata: Some(metadata),
            timestamp: None,
        };

        let request = Request::new(InsertRequest {
            collection_id: "grpc_rust_embeddings".to_string(),
            record: Some(record),
        });

        let response = self.client.client.insert(request).await?;
        println!("Vector inserted: {}", response.get_ref().vector_id);
        Ok(response.into_inner())
    }

    pub async fn search_vectors_grpc(&mut self) -> Result<SearchResponse, Box<dyn std::error::Error>> {
        let query_vector = self.generate_random_vector(384);

        // Create filter
        let mut filter_fields = HashMap::new();
        filter_fields.insert("category".to_string(), Value {
            kind: Some(prost_types::value::Kind::StringValue("technical".to_string())),
        });
        let filters = self.create_metadata_struct(filter_fields);

        let request = Request::new(SearchRequest {
            collection_id: "grpc_rust_embeddings".to_string(),
            vector: query_vector,
            k: 10,
            filters: Some(filters),
            threshold: Some(0.7),
        });

        let response = self.client.client.search(request).await?;
        
        println!("Found {} similar vectors:", response.get_ref().total_count);
        for result in &response.get_ref().results {
            println!("- {}: score={:.3}", result.id, result.score);
            if let Some(metadata) = &result.metadata {
                if let Some(title_value) = metadata.fields.get("title") {
                    if let Some(prost_types::value::Kind::StringValue(title)) = &title_value.kind {
                        println!("  Title: {}", title);
                    }
                }
            }
        }

        Ok(response.into_inner())
    }

    pub async fn batch_insert_grpc(&mut self) -> Result<BatchInsertResponse, Box<dyn std::error::Error>> {
        let mut records = Vec::new();

        for i in 0..50 {
            let mut metadata_fields = HashMap::new();
            metadata_fields.insert("document_id".to_string(), Value {
                kind: Some(prost_types::value::Kind::StringValue(format!("grpc_rust_batch_{:03}", i))),
            });
            metadata_fields.insert("index".to_string(), Value {
                kind: Some(prost_types::value::Kind::NumberValue(i as f64)),
            });
            metadata_fields.insert("batch".to_string(), Value {
                kind: Some(prost_types::value::Kind::StringValue("grpc_rust_example".to_string())),
            });

            let metadata = self.create_metadata_struct(metadata_fields);

            let record = VectorRecord {
                collection_id: "grpc_rust_embeddings".to_string(),
                id: Uuid::new_v4().to_string(),
                vector: self.generate_random_vector(384),
                metadata: Some(metadata),
                timestamp: None,
            };

            records.push(record);
        }

        let request = Request::new(BatchInsertRequest {
            collection_id: "grpc_rust_embeddings".to_string(),
            records,
        });

        let response = self.client.client.batch_insert(request).await?;
        println!("Batch inserted {} vectors", response.get_ref().inserted_count);

        if !response.get_ref().errors.is_empty() {
            println!("Errors: {:?}", response.get_ref().errors);
        }

        Ok(response.into_inner())
    }
}
```

### Health and Monitoring

```rust
pub struct GrpcHealthExamples {
    client: ProximaDBGrpcClient,
}

impl GrpcHealthExamples {
    pub fn new(client: ProximaDBGrpcClient) -> Self {
        Self { client }
    }

    pub async fn health_check_grpc(&mut self) -> Result<HealthResponse, Box<dyn std::error::Error>> {
        let request = Request::new(HealthRequest {});
        let response = self.client.client.health(request).await?;

        println!("Health status: {}", response.get_ref().status);
        if let Some(timestamp) = &response.get_ref().timestamp {
            println!("Timestamp: {:?}", timestamp);
        }

        Ok(response.into_inner())
    }

    pub async fn get_status_grpc(&mut self) -> Result<StatusResponse, Box<dyn std::error::Error>> {
        let request = Request::new(StatusRequest {});
        let response = self.client.client.status(request).await?;

        let status = response.get_ref();
        println!("Node ID: {}", status.node_id);
        println!("Version: {}", status.version);
        println!("Role: {:?}", status.role);

        if let Some(storage) = &status.storage {
            println!("Total vectors: {}", storage.total_vectors);
            println!("Total size: {} bytes", storage.total_size_bytes);
        }

        Ok(response.into_inner())
    }
}
```

## Error Handling and Best Practices

```rust
use tonic::{Code, Status};
use tokio::time::{sleep, Duration};

#[derive(thiserror::Error, Debug)]
pub enum ProximaDBGrpcError {
    #[error("gRPC error {code:?}: {message}")]
    Grpc { code: Code, message: String },
    #[error("Transport error: {0}")]
    Transport(#[from] tonic::transport::Error),
}

pub fn handle_grpc_error(status: Status) -> ProximaDBGrpcError {
    let code = status.code();
    let message = status.message().to_string();

    let user_message = match code {
        Code::InvalidArgument => "Invalid request parameters",
        Code::NotFound => "Resource not found",
        Code::AlreadyExists => "Resource already exists",
        Code::Unauthenticated => "Authentication failed",
        Code::PermissionDenied => "Access denied",
        Code::ResourceExhausted => "Rate limited",
        Code::Internal => "Internal server error",
        Code::Unavailable => "Service unavailable",
        _ => "Unknown error",
    };

    ProximaDBGrpcError::Grpc {
        code,
        message: format!("{}: {}", user_message, message),
    }
}

// Retry with exponential backoff
pub async fn retry_with_backoff<F, Fut, T>(
    mut operation: F,
    max_retries: usize,
    base_delay: Duration,
) -> Result<T, ProximaDBGrpcError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, Status>>,
{
    for attempt in 0..max_retries {
        match operation().await {
            Ok(result) => return Ok(result),
            Err(status) if status.code() == Code::ResourceExhausted && attempt < max_retries - 1 => {
                let delay = base_delay * 2_u32.pow(attempt as u32);
                println!("Rate limited, retrying in {:?}...", delay);
                sleep(delay).await;
                continue;
            }
            Err(status) => return Err(handle_grpc_error(status)),
        }
    }
    unreachable!()
}

// Usage example
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = create_grpc_client().await?;

    // Example with retry
    let response = retry_with_backoff(
        || async {
            let request = Request::new(CreateCollectionRequest {
                collection_id: "test_collection".to_string(),
                name: "Test Collection".to_string(),
                dimension: 128,
                schema_type: SchemaType::Document as i32,
                schema: None,
            });
            client.client.create_collection(request).await
        },
        3,
        Duration::from_millis(1000),
    ).await;

    match response {
        Ok(_) => println!("Collection created successfully"),
        Err(ProximaDBGrpcError::Grpc { code: Code::AlreadyExists, .. }) => {
            println!("Collection already exists");
        }
        Err(ProximaDBGrpcError::Grpc { code: Code::ResourceExhausted, .. }) => {
            println!("Rate limited, try again later");
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            return Err(e.into());
        }
    }

    Ok(())
}
```

## Official SDK Usage

```rust
use proximadb_client::{ProximaDBClient, ClientConfig, VectorWithMetadata, SearchParams};
use std::collections::HashMap;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Using the official ProximaDB Rust client
    let config = ClientConfig::builder()
        .api_key("pk_live_1234567890abcdef")
        .host("localhost")
        .port(5678)
        .build();

    let client = ProximaDBClient::new(config).await?;

    // Create collection
    let collection = client.create_collection("sdk_vectors", 128).await?;
    println!("Collection created: {}", collection.id);

    // Insert vector
    let mut metadata = HashMap::new();
    metadata.insert("document".to_string(), "example.txt".to_string());

    let vector = VectorWithMetadata::builder()
        .vector(vec![0.1, 0.2, 0.3, 0.4])
        .metadata(metadata)
        .build();

    let result = client.insert_vector("sdk_vectors", vector).await?;
    println!("Vector inserted: {}", result.vector_id);

    // Search vectors
    let params = SearchParams::builder()
        .k(10)
        .threshold(0.8)
        .build();

    let search_response = client.search_vectors(
        "sdk_vectors",
        vec![0.1, 0.2, 0.3, 0.4],
        params,
    ).await?;

    println!("Found {} results", search_response.results.len());

    Ok(())
}
```

This comprehensive Rust guide covers both REST and gRPC APIs with practical examples for building production applications with ProximaDB.