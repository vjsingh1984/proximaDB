//! Collection management and fluent API for ProximaDB
//!
//! This module provides the `CollectionHandle` for fluent operations on
//! collections, and `CollectionBuilder` for creating collections.

use crate::error::{CollectionError, ProximaError, Result, VectorError};
use crate::search::SearchBuilder;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Storage engine types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum StorageEngine {
    /// SST - Write-optimized, real-time workloads
    #[default]
    Sst,
    /// HELIX - Locality-optimized with Hilbert curves
    Helix,
    /// VIPER - Columnar Parquet for analytics
    Viper,
    /// SWIFT - Ultra-low latency for small datasets
    Swift,
    /// NOVA - Progressive columnar for mixed workloads
    Nova,
    /// RAPTOR - Adaptive row-group for dynamic workloads
    Raptor,
}

impl StorageEngine {
    /// Convert to string representation
    pub fn as_str(&self) -> &'static str {
        match self {
            StorageEngine::Sst => "sst",
            StorageEngine::Helix => "helix",
            StorageEngine::Viper => "viper",
            StorageEngine::Swift => "swift",
            StorageEngine::Nova => "nova",
            StorageEngine::Raptor => "raptor",
        }
    }
}

impl std::str::FromStr for StorageEngine {
    type Err = ProximaError;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "sst" => Ok(StorageEngine::Sst),
            "helix" => Ok(StorageEngine::Helix),
            "viper" => Ok(StorageEngine::Viper),
            "swift" => Ok(StorageEngine::Swift),
            "nova" => Ok(StorageEngine::Nova),
            "raptor" => Ok(StorageEngine::Raptor),
            _ => Err(ProximaError::Collection(CollectionError::UnknownEngine {
                engine: s.to_string(),
            })),
        }
    }
}

/// Index type for collections
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum IndexType {
    /// HNSW - Hierarchical Navigable Small World
    #[default]
    Hnsw,
    /// IVF - Inverted File Index
    Ivf,
    /// LSH - Locality Sensitive Hashing
    Lsh,
    /// Flat - Brute force (no index)
    Flat,
}

impl IndexType {
    /// Convert to string representation
    pub fn as_str(&self) -> &'static str {
        match self {
            IndexType::Hnsw => "hnsw",
            IndexType::Ivf => "ivf",
            IndexType::Lsh => "lsh",
            IndexType::Flat => "flat",
        }
    }
}

/// Distance metric for similarity search
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DistanceMetric {
    /// L2 (Euclidean) distance
    L2,
    /// Cosine similarity
    #[default]
    Cosine,
    /// Dot product
    DotProduct,
    /// Inner product (alias for dot product)
    InnerProduct,
}

/// Builder for creating collections
///
/// # Example
///
/// ```rust,ignore
/// client.create_collection("embeddings")
///     .dimension(768)
///     .engine(StorageEngine::Sst)
///     .index(IndexType::Hnsw)
///     .metric(DistanceMetric::Cosine)
///     .execute()
///     .await?;
/// ```
pub struct CollectionBuilder<'a> {
    #[cfg(feature = "client")]
    client: Option<&'a crate::client::ProximaClient>,
    #[cfg(feature = "embedded")]
    db: Option<&'a crate::embedded::ProximaDB>,
    name: String,
    dimension: Option<u32>,
    engine: StorageEngine,
    index: IndexType,
    metric: DistanceMetric,
}

impl<'a> CollectionBuilder<'a> {
    /// Create a new collection builder (client mode)
    #[cfg(feature = "client")]
    pub fn new(client: &'a crate::client::ProximaClient, name: &str) -> Self {
        Self {
            client: Some(client),
            #[cfg(feature = "embedded")]
            db: None,
            name: name.to_string(),
            dimension: None,
            engine: StorageEngine::default(),
            index: IndexType::default(),
            metric: DistanceMetric::default(),
        }
    }

    /// Create a new collection builder (embedded mode)
    #[cfg(feature = "embedded")]
    pub fn new_embedded(db: &'a crate::embedded::ProximaDB, name: &str) -> Self {
        Self {
            #[cfg(feature = "client")]
            client: None,
            db: Some(db),
            name: name.to_string(),
            dimension: None,
            engine: StorageEngine::default(),
            index: IndexType::default(),
            metric: DistanceMetric::default(),
        }
    }

    /// Set the vector dimension (required)
    pub fn dimension(mut self, dim: u32) -> Self {
        self.dimension = Some(dim);
        self
    }

    /// Set the storage engine
    pub fn engine(mut self, engine: StorageEngine) -> Self {
        self.engine = engine;
        self
    }

    /// Set the storage engine from string
    pub fn engine_str(mut self, engine: &str) -> Result<Self> {
        self.engine = engine.parse()?;
        Ok(self)
    }

    /// Set the index type
    pub fn index(mut self, index: IndexType) -> Self {
        self.index = index;
        self
    }

    /// Set the distance metric
    pub fn metric(mut self, metric: DistanceMetric) -> Self {
        self.metric = metric;
        self
    }

    /// Execute the collection creation (async, client mode)
    #[cfg(feature = "client")]
    pub async fn execute(self) -> Result<()> {
        let client = self
            .client
            .ok_or_else(|| ProximaError::Internal("No client reference".to_string()))?;

        let dimension = self.dimension.ok_or_else(|| {
            ProximaError::Collection(CollectionError::InvalidConfig {
                reason: "dimension is required".to_string(),
            })
        })?;

        let request = CreateCollectionRequest {
            name: self.name,
            dimension,
            engine: Some(self.engine.as_str().to_string()),
            index_type: Some(self.index.as_str().to_string()),
        };

        let url = format!("{}/api/v1/collections", client.url());
        let _response: CreateCollectionResponse = client.post(&url, &request).await?;
        Ok(())
    }

    /// Execute the collection creation (sync, embedded mode)
    #[cfg(feature = "embedded")]
    pub fn execute_sync(self) -> Result<()> {
        let db = self
            .db
            .ok_or_else(|| ProximaError::Internal("No embedded database reference".to_string()))?;

        let dimension = self.dimension.ok_or_else(|| {
            ProximaError::Collection(CollectionError::InvalidConfig {
                reason: "dimension is required".to_string(),
            })
        })?;

        db.create_collection_internal(&self.name, dimension, &self.engine, &self.index)
    }
}

/// Handle to a collection for fluent operations
///
/// # Example
///
/// ```rust,ignore
/// let results = client.collection("embeddings")
///     .search()
///     .vector(&query)
///     .top_k(10)
///     .filter("category = 'tech'")
///     .execute()
///     .await?;
/// ```
pub struct CollectionHandle<'a> {
    #[cfg(feature = "client")]
    client: Option<&'a crate::client::ProximaClient>,
    #[cfg(feature = "embedded")]
    db: Option<&'a crate::embedded::ProximaDB>,
    name: String,
}

impl<'a> CollectionHandle<'a> {
    /// Create a new collection handle (client mode)
    #[cfg(feature = "client")]
    pub fn new(client: &'a crate::client::ProximaClient, name: &str) -> Self {
        Self {
            client: Some(client),
            #[cfg(feature = "embedded")]
            db: None,
            name: name.to_string(),
        }
    }

    /// Create a new collection handle (embedded mode)
    #[cfg(feature = "embedded")]
    pub fn new_embedded(db: &'a crate::embedded::ProximaDB, name: &str) -> Self {
        Self {
            #[cfg(feature = "client")]
            client: None,
            db: Some(db),
            name: name.to_string(),
        }
    }

    /// Get the collection name
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Start building a search query
    #[cfg(feature = "client")]
    pub fn search(&self) -> SearchBuilder<'a> {
        SearchBuilder::new_client(self.client.expect("Client reference required"), &self.name)
    }

    /// Start building a search query (embedded mode)
    #[cfg(feature = "embedded")]
    pub fn search_embedded(&self) -> SearchBuilder<'a> {
        SearchBuilder::new_embedded(self.db.expect("Embedded DB reference required"), &self.name)
    }

    /// Start building an insert operation
    pub fn insert(&self) -> InsertBuilder<'a> {
        #[cfg(feature = "client")]
        {
            InsertBuilder::new_client(self.client.expect("Client reference required"), &self.name)
        }
        #[cfg(all(feature = "embedded", not(feature = "client")))]
        {
            InsertBuilder::new_embedded(
                self.db.expect("Embedded DB reference required"),
                &self.name,
            )
        }
    }

    /// Get collection info
    #[cfg(feature = "client")]
    pub async fn info(&self) -> Result<CollectionInfo> {
        let client = self.client.expect("Client reference required");
        let url = format!("{}/api/v1/collections/{}", client.url(), self.name);
        client.get(&url).await
    }

    /// Get vector count
    #[cfg(feature = "client")]
    pub async fn count(&self) -> Result<u64> {
        let info = self.info().await?;
        Ok(info.vector_count)
    }

    /// Delete the collection
    #[cfg(feature = "client")]
    pub async fn delete(self) -> Result<()> {
        let client = self.client.expect("Client reference required");
        client.delete_collection(&self.name).await
    }

    /// Start building an update operation for a vector
    pub fn update(&self, id: &str) -> UpdateBuilder<'a> {
        #[cfg(feature = "client")]
        {
            UpdateBuilder::new_client(
                self.client.expect("Client reference required"),
                &self.name,
                id,
            )
        }
        #[cfg(all(feature = "embedded", not(feature = "client")))]
        {
            UpdateBuilder::new_embedded(
                self.db.expect("Embedded DB reference required"),
                &self.name,
                id,
            )
        }
    }

    /// Delete a vector by ID
    #[cfg(feature = "client")]
    pub async fn delete_vector(&self, id: &str) -> Result<()> {
        let client = self.client.expect("Client reference required");
        let url = format!(
            "{}/api/v1/collections/{}/vectors/{}",
            client.url(),
            self.name,
            id
        );
        client.delete::<serde_json::Value>(&url).await?;
        Ok(())
    }

    /// Delete multiple vectors by IDs
    #[cfg(feature = "client")]
    pub async fn delete_vectors(&self, ids: Vec<String>) -> Result<usize> {
        let client = self.client.expect("Client reference required");
        let request = DeleteVectorsRequest {
            collection: self.name.clone(),
            ids,
        };
        let url = format!(
            "{}/api/v1/collections/{}/vectors/delete",
            client.url(),
            self.name
        );
        let response: DeleteVectorsResponse = client.post(&url, &request).await?;
        Ok(response.deleted_count)
    }

    /// Get a vector by ID
    #[cfg(feature = "client")]
    pub async fn get_vector(&self, id: &str) -> Result<Option<VectorRecord>> {
        let client = self.client.expect("Client reference required");
        let url = format!(
            "{}/api/v1/collections/{}/vectors/{}",
            client.url(),
            self.name,
            id
        );
        match client.get::<VectorRecord>(&url).await {
            Ok(record) => Ok(Some(record)),
            Err(ProximaError::Network(crate::error::NetworkError::HttpError {
                status: 404,
                ..
            })) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Check if a vector exists
    #[cfg(feature = "client")]
    pub async fn exists(&self, id: &str) -> Result<bool> {
        Ok(self.get_vector(id).await?.is_some())
    }
}

/// Builder for update operations
///
/// # Example
///
/// ```rust,ignore
/// collection.update("vec_123")
///     .vector(&new_embedding)
///     .meta("updated_at", "2024-01-15")
///     .execute()
///     .await?;
/// ```
pub struct UpdateBuilder<'a> {
    #[cfg(feature = "client")]
    client: Option<&'a crate::client::ProximaClient>,
    #[cfg(feature = "embedded")]
    db: Option<&'a crate::embedded::ProximaDB>,
    collection: String,
    id: String,
    vector: Option<Vec<f32>>,
    metadata: HashMap<String, serde_json::Value>,
    replace_metadata: bool,
}

impl<'a> UpdateBuilder<'a> {
    /// Create a new update builder (client mode)
    #[cfg(feature = "client")]
    pub fn new_client(
        client: &'a crate::client::ProximaClient,
        collection: &str,
        id: &str,
    ) -> Self {
        Self {
            client: Some(client),
            #[cfg(feature = "embedded")]
            db: None,
            collection: collection.to_string(),
            id: id.to_string(),
            vector: None,
            metadata: HashMap::new(),
            replace_metadata: false,
        }
    }

    /// Create a new update builder (embedded mode)
    #[cfg(feature = "embedded")]
    pub fn new_embedded(db: &'a crate::embedded::ProximaDB, collection: &str, id: &str) -> Self {
        Self {
            #[cfg(feature = "client")]
            client: None,
            db: Some(db),
            collection: collection.to_string(),
            id: id.to_string(),
            vector: None,
            metadata: HashMap::new(),
            replace_metadata: false,
        }
    }

    /// Set a new vector
    pub fn vector(mut self, vector: &[f32]) -> Self {
        self.vector = Some(vector.to_vec());
        self
    }

    /// Set metadata from JSON value (merges with existing)
    pub fn metadata(mut self, metadata: serde_json::Value) -> Self {
        if let serde_json::Value::Object(map) = metadata {
            for (k, v) in map {
                self.metadata.insert(k, v);
            }
        }
        self
    }

    /// Set a single metadata field
    pub fn meta(mut self, key: impl Into<String>, value: impl Into<serde_json::Value>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Replace all metadata instead of merging
    pub fn replace_metadata(mut self, replace: bool) -> Self {
        self.replace_metadata = replace;
        self
    }

    /// Execute the update (async, client mode)
    #[cfg(feature = "client")]
    pub async fn execute(self) -> Result<()> {
        let client = self
            .client
            .ok_or_else(|| ProximaError::Internal("No client reference".to_string()))?;

        let request = UpdateVectorRequest {
            collection: self.collection.clone(),
            id: self.id,
            vector: self.vector,
            metadata: if self.metadata.is_empty() {
                None
            } else {
                Some(self.metadata)
            },
            replace_metadata: self.replace_metadata,
        };

        let url = format!(
            "{}/api/v1/collections/{}/vectors/update",
            client.url(),
            self.collection
        );
        let _response: UpdateVectorResponse = client.post(&url, &request).await?;
        Ok(())
    }

    /// Execute the update (sync, embedded mode)
    #[cfg(feature = "embedded")]
    pub fn execute_sync(self) -> Result<()> {
        let db = self
            .db
            .ok_or_else(|| ProximaError::Internal("No embedded database reference".to_string()))?;

        db.update_internal(
            &self.collection,
            self.id,
            self.vector,
            self.metadata,
            self.replace_metadata,
        )
    }
}

/// Collection information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionInfo {
    /// Collection name
    pub name: String,
    /// Vector dimension
    pub dimension: u32,
    /// Number of vectors
    #[serde(default)]
    pub vector_count: u64,
    /// Storage engine
    #[serde(default)]
    pub engine: Option<String>,
    /// Disk usage in bytes
    #[serde(default)]
    pub disk_usage_bytes: Option<u64>,
}

/// Builder for insert operations
///
/// # Example
///
/// ```rust,ignore
/// collection.insert()
///     .id("vec_123")
///     .vector(&embedding)
///     .metadata(json!({"type": "article"}))
///     .execute()
///     .await?;
/// ```
pub struct InsertBuilder<'a> {
    #[cfg(feature = "client")]
    client: Option<&'a crate::client::ProximaClient>,
    #[cfg(feature = "embedded")]
    db: Option<&'a crate::embedded::ProximaDB>,
    collection: String,
    records: Vec<InsertRecord>,
}

/// A single record to insert
#[derive(Debug, Clone)]
struct InsertRecord {
    id: String,
    vector: Vec<f32>,
    metadata: HashMap<String, serde_json::Value>,
}

impl<'a> InsertBuilder<'a> {
    /// Create a new insert builder (client mode)
    #[cfg(feature = "client")]
    pub fn new_client(client: &'a crate::client::ProximaClient, collection: &str) -> Self {
        Self {
            client: Some(client),
            #[cfg(feature = "embedded")]
            db: None,
            collection: collection.to_string(),
            records: Vec::new(),
        }
    }

    /// Create a new insert builder (embedded mode)
    #[cfg(feature = "embedded")]
    pub fn new_embedded(db: &'a crate::embedded::ProximaDB, collection: &str) -> Self {
        Self {
            #[cfg(feature = "client")]
            client: None,
            db: Some(db),
            collection: collection.to_string(),
            records: Vec::new(),
        }
    }

    /// Add a single vector with ID
    pub fn id(self, id: impl Into<String>) -> InsertBuilderWithId<'a> {
        InsertBuilderWithId {
            builder: self,
            id: id.into(),
            vector: None,
            metadata: HashMap::new(),
        }
    }

    /// Add multiple vectors in batch
    pub fn batch(
        mut self,
        ids: Vec<String>,
        vectors: Vec<Vec<f32>>,
    ) -> Result<InsertBuilderBatch<'a>> {
        if ids.len() != vectors.len() {
            return Err(ProximaError::Vector(VectorError::BatchSizeMismatch {
                ids: ids.len(),
                vectors: vectors.len(),
            }));
        }

        for (id, vector) in ids.into_iter().zip(vectors.into_iter()) {
            self.records.push(InsertRecord {
                id,
                vector,
                metadata: HashMap::new(),
            });
        }

        Ok(InsertBuilderBatch { builder: self })
    }

    /// Execute the insert (internal)
    #[cfg(feature = "client")]
    async fn execute_internal(self) -> Result<InsertResponse> {
        let client = self
            .client
            .ok_or_else(|| ProximaError::Internal("No client reference".to_string()))?;

        if self.records.is_empty() {
            return Ok(InsertResponse { inserted_count: 0 });
        }

        let request = InsertRequest {
            collection: self.collection,
            vectors: self
                .records
                .into_iter()
                .map(|r| VectorRecord {
                    id: r.id,
                    vector: r.vector,
                    metadata: r.metadata,
                })
                .collect(),
        };

        let url = format!("{}/api/v1/vectors/insert", client.url());
        client.post(&url, &request).await
    }
}

/// Insert builder with ID set
pub struct InsertBuilderWithId<'a> {
    builder: InsertBuilder<'a>,
    id: String,
    vector: Option<Vec<f32>>,
    metadata: HashMap<String, serde_json::Value>,
}

impl<'a> InsertBuilderWithId<'a> {
    /// Set the vector
    pub fn vector(mut self, vector: &[f32]) -> Self {
        self.vector = Some(vector.to_vec());
        self
    }

    /// Set metadata from JSON value
    pub fn metadata(mut self, metadata: serde_json::Value) -> Self {
        if let serde_json::Value::Object(map) = metadata {
            for (k, v) in map {
                self.metadata.insert(k, v);
            }
        }
        self
    }

    /// Set a single metadata field
    pub fn meta(mut self, key: impl Into<String>, value: impl Into<serde_json::Value>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Execute the insert (async, client mode)
    #[cfg(feature = "client")]
    pub async fn execute(mut self) -> Result<()> {
        let vector = self.vector.ok_or_else(|| {
            ProximaError::Vector(VectorError::InvalidFormat {
                reason: "vector is required".to_string(),
            })
        })?;

        self.builder.records.push(InsertRecord {
            id: self.id,
            vector,
            metadata: self.metadata,
        });

        self.builder.execute_internal().await?;
        Ok(())
    }

    /// Execute the insert (sync, embedded mode)
    #[cfg(feature = "embedded")]
    pub fn execute_sync(self) -> Result<()> {
        let vector = self.vector.ok_or_else(|| {
            ProximaError::Vector(VectorError::InvalidFormat {
                reason: "vector is required".to_string(),
            })
        })?;

        let db = self
            .builder
            .db
            .ok_or_else(|| ProximaError::Internal("No embedded database reference".to_string()))?;

        db.insert_internal(&self.builder.collection, self.id, vector, self.metadata)
    }
}

/// Insert builder for batch operations
pub struct InsertBuilderBatch<'a> {
    builder: InsertBuilder<'a>,
}

impl<'a> InsertBuilderBatch<'a> {
    /// Add metadata for all vectors
    pub fn with_metadata(
        mut self,
        metadata: Vec<HashMap<String, serde_json::Value>>,
    ) -> Result<Self> {
        if metadata.len() != self.builder.records.len() {
            return Err(ProximaError::Vector(VectorError::BatchSizeMismatch {
                ids: self.builder.records.len(),
                vectors: metadata.len(),
            }));
        }

        for (record, meta) in self.builder.records.iter_mut().zip(metadata.into_iter()) {
            record.metadata = meta;
        }

        Ok(self)
    }

    /// Execute the batch insert (async, client mode)
    #[cfg(feature = "client")]
    pub async fn execute(self) -> Result<usize> {
        let response = self.builder.execute_internal().await?;
        Ok(response.inserted_count)
    }

    /// Execute the batch insert (sync, embedded mode)
    #[cfg(feature = "embedded")]
    pub fn execute_sync(self) -> Result<usize> {
        let db = self
            .builder
            .db
            .ok_or_else(|| ProximaError::Internal("No embedded database reference".to_string()))?;

        let ids: Vec<String> = self.builder.records.iter().map(|r| r.id.clone()).collect();
        let vectors: Vec<Vec<f32>> = self
            .builder
            .records
            .iter()
            .map(|r| r.vector.clone())
            .collect();
        let metadata: Vec<HashMap<String, serde_json::Value>> = self
            .builder
            .records
            .iter()
            .map(|r| r.metadata.clone())
            .collect();

        db.insert_batch_internal(&self.builder.collection, ids, vectors, metadata)
    }
}

// Request/Response types for HTTP API

#[derive(Debug, Serialize)]
struct CreateCollectionRequest {
    name: String,
    dimension: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    engine: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    index_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateCollectionResponse {
    #[allow(dead_code)]
    success: bool,
}

#[derive(Debug, Serialize)]
struct InsertRequest {
    collection: String,
    vectors: Vec<VectorRecord>,
}

/// A vector record with ID, vector data, and metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorRecord {
    /// Vector ID
    pub id: String,
    /// Vector data
    pub vector: Vec<f32>,
    /// Associated metadata
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct InsertResponse {
    inserted_count: usize,
}

#[derive(Debug, Serialize)]
struct UpdateVectorRequest {
    collection: String,
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    vector: Option<Vec<f32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<HashMap<String, serde_json::Value>>,
    #[serde(default)]
    replace_metadata: bool,
}

#[derive(Debug, Deserialize)]
struct UpdateVectorResponse {
    #[allow(dead_code)]
    success: bool,
}

#[derive(Debug, Serialize)]
struct DeleteVectorsRequest {
    collection: String,
    ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct DeleteVectorsResponse {
    deleted_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_engine_parse() {
        assert_eq!("sst".parse::<StorageEngine>().unwrap(), StorageEngine::Sst);
        assert_eq!(
            "helix".parse::<StorageEngine>().unwrap(),
            StorageEngine::Helix
        );
        assert!("invalid".parse::<StorageEngine>().is_err());
    }

    #[test]
    fn test_storage_engine_as_str() {
        assert_eq!(StorageEngine::Sst.as_str(), "sst");
        assert_eq!(StorageEngine::Helix.as_str(), "helix");
    }
}
