//! # Schema Registry - Schema Versioning and Management
//!
//! Provides schema versioning, fingerprint-based lookup, and persistence.
//! Supports both in-memory (development) and persistent (production) backends.

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

use super::proxima_schema::ProximaSchema;

/// Schema registry for managing schema versions.
#[async_trait]
pub trait SchemaRegistry: Send + Sync {
    /// Register a new schema version.
    async fn register_schema(&self, collection_id: &str, schema: ProximaSchema) -> Result<()>;

    /// Get schema by version.
    async fn get_schema(&self, collection_id: &str, version: u32) -> Result<Option<ProximaSchema>>;

    /// Get latest schema for collection.
    async fn get_latest_schema(&self, collection_id: &str) -> Result<Option<ProximaSchema>>;

    /// Get schema by fingerprint (fast lookup).
    async fn get_schema_by_fingerprint(&self, fingerprint: u64) -> Result<Option<ProximaSchema>>;

    /// List all schema versions for collection.
    async fn list_versions(&self, collection_id: &str) -> Result<Vec<SchemaVersionInfo>>;

    /// Get schema lineage (parent chain).
    async fn get_lineage(&self, schema_id: &str) -> Result<Vec<ProximaSchema>>;

    /// Delete old schema versions (keep recent N).
    async fn prune_versions(&self, collection_id: &str, keep_versions: u32) -> Result<u32>;

    /// Check if a schema exists for the collection.
    async fn has_schema(&self, collection_id: &str) -> Result<bool>;
}

/// Schema version metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaVersionInfo {
    pub schema_id: String,
    pub version: u32,
    pub fingerprint: u64,
    pub created_at_ms: i64,
    pub is_current: bool,
    pub column_count: usize,
}

/// In-memory schema registry (for development/testing).
pub struct InMemorySchemaRegistry {
    /// schemas[collection_id][version] = schema
    schemas: RwLock<HashMap<String, HashMap<u32, ProximaSchema>>>,
    /// fingerprint -> (collection_id, version)
    fingerprint_index: RwLock<HashMap<u64, (String, u32)>>,
}

impl Default for InMemorySchemaRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemorySchemaRegistry {
    pub fn new() -> Self {
        Self {
            schemas: RwLock::new(HashMap::new()),
            fingerprint_index: RwLock::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl SchemaRegistry for InMemorySchemaRegistry {
    async fn register_schema(&self, collection_id: &str, schema: ProximaSchema) -> Result<()> {
        let mut schemas = self.schemas.write().await;
        let collection_schemas = schemas
            .entry(collection_id.to_string())
            .or_insert_with(HashMap::new);

        // Check for duplicate version
        if collection_schemas.contains_key(&schema.version) {
            return Err(anyhow::anyhow!(
                "Schema version {} already exists for collection {}",
                schema.version,
                collection_id
            ));
        }

        // Index by fingerprint
        let mut index = self.fingerprint_index.write().await;
        index.insert(
            schema.fingerprint,
            (collection_id.to_string(), schema.version),
        );

        info!(
            "Registered schema v{} for collection {} (fingerprint: {})",
            schema.version, collection_id, schema.fingerprint
        );

        collection_schemas.insert(schema.version, schema);

        Ok(())
    }

    async fn get_schema(&self, collection_id: &str, version: u32) -> Result<Option<ProximaSchema>> {
        let schemas = self.schemas.read().await;
        Ok(schemas
            .get(collection_id)
            .and_then(|c| c.get(&version))
            .cloned())
    }

    async fn get_latest_schema(&self, collection_id: &str) -> Result<Option<ProximaSchema>> {
        let schemas = self.schemas.read().await;
        Ok(schemas
            .get(collection_id)
            .and_then(|c| c.values().max_by_key(|s| s.version))
            .cloned())
    }

    async fn get_schema_by_fingerprint(&self, fingerprint: u64) -> Result<Option<ProximaSchema>> {
        let index = self.fingerprint_index.read().await;
        if let Some((collection_id, version)) = index.get(&fingerprint) {
            return self.get_schema(collection_id, *version).await;
        }
        Ok(None)
    }

    async fn list_versions(&self, collection_id: &str) -> Result<Vec<SchemaVersionInfo>> {
        let schemas = self.schemas.read().await;
        let collection_schemas = schemas.get(collection_id);

        let mut versions: Vec<SchemaVersionInfo> = collection_schemas
            .map(|c| {
                let max_version = c.values().map(|s| s.version).max().unwrap_or(0);
                c.values()
                    .map(|s| SchemaVersionInfo {
                        schema_id: s.schema_id.clone(),
                        version: s.version,
                        fingerprint: s.fingerprint,
                        created_at_ms: s.created_at_ms,
                        is_current: s.version == max_version,
                        column_count: s.columns.iter().filter(|c| !c.is_deleted).count(),
                    })
                    .collect()
            })
            .unwrap_or_default();

        versions.sort_by_key(|v| v.version);
        Ok(versions)
    }

    async fn get_lineage(&self, schema_id: &str) -> Result<Vec<ProximaSchema>> {
        let schemas = self.schemas.read().await;
        let mut lineage = Vec::new();
        let mut current_id = Some(schema_id.to_string());

        while let Some(id) = current_id.take() {
            let mut found = false;
            for collection_schemas in schemas.values() {
                for schema in collection_schemas.values() {
                    if schema.schema_id == id {
                        current_id = schema.parent_schema_id.clone();
                        lineage.push(schema.clone());
                        found = true;
                        break;
                    }
                }
                if found {
                    break;
                }
            }
            if !found {
                break;
            }
        }

        Ok(lineage)
    }

    async fn prune_versions(&self, collection_id: &str, keep_versions: u32) -> Result<u32> {
        let mut schemas = self.schemas.write().await;
        let mut index = self.fingerprint_index.write().await;

        if let Some(collection_schemas) = schemas.get_mut(collection_id) {
            let mut versions: Vec<u32> = collection_schemas.keys().copied().collect();
            versions.sort();

            let to_delete: Vec<u32> = versions
                .iter()
                .rev()
                .skip(keep_versions as usize)
                .copied()
                .collect();

            for version in &to_delete {
                if let Some(schema) = collection_schemas.remove(version) {
                    index.remove(&schema.fingerprint);
                }
            }

            info!(
                "Pruned {} schema versions for collection {}",
                to_delete.len(),
                collection_id
            );

            return Ok(to_delete.len() as u32);
        }

        Ok(0)
    }

    async fn has_schema(&self, collection_id: &str) -> Result<bool> {
        let schemas = self.schemas.read().await;
        Ok(schemas.contains_key(collection_id))
    }
}

/// Persistent schema registry backed by storage.
pub struct PersistentSchemaRegistry {
    /// Base storage URL for schema files
    storage_url: String,
    /// Filesystem for persistence
    filesystem: Arc<dyn crate::storage::persistence::filesystem::FileSystem>,
    /// In-memory cache
    cache: InMemorySchemaRegistry,
}

impl PersistentSchemaRegistry {
    pub fn new(
        storage_url: String,
        filesystem: Arc<dyn crate::storage::persistence::filesystem::FileSystem>,
    ) -> Self {
        Self {
            storage_url,
            filesystem,
            cache: InMemorySchemaRegistry::new(),
        }
    }

    fn schema_path(&self, collection_id: &str, version: u32) -> String {
        format!(
            "{}/schemas/{}/v{}.json",
            self.storage_url, collection_id, version
        )
    }

    async fn persist_schema(&self, collection_id: &str, schema: &ProximaSchema) -> Result<()> {
        let path = self.schema_path(collection_id, schema.version);
        let json = serde_json::to_vec_pretty(schema)?;
        self.filesystem.write(&path, &json, None).await?;
        debug!("Persisted schema to {}", path);
        Ok(())
    }

    async fn load_schema(
        &self,
        collection_id: &str,
        version: u32,
    ) -> Result<Option<ProximaSchema>> {
        let path = self.schema_path(collection_id, version);
        match self.filesystem.read(&path).await {
            Ok(data) => {
                let schema: ProximaSchema = serde_json::from_slice(&data)
                    .with_context(|| format!("Failed to parse schema at {}", path))?;
                Ok(Some(schema))
            }
            Err(_) => Ok(None),
        }
    }

    async fn list_schema_files(&self, collection_id: &str) -> Result<Vec<u32>> {
        let prefix = format!("{}/schemas/{}/", self.storage_url, collection_id);
        let files = self.filesystem.list(&prefix).await.unwrap_or_default();

        let versions: Vec<u32> = files
            .iter()
            .filter_map(|entry| {
                // Use entry.name (filename) not the full URL
                let filename = entry.name.as_str();
                filename
                    .strip_prefix('v')
                    .and_then(|s| s.strip_suffix(".json"))
                    .and_then(|s| s.parse::<u32>().ok())
            })
            .collect();

        Ok(versions)
    }
}

#[async_trait]
impl SchemaRegistry for PersistentSchemaRegistry {
    async fn register_schema(&self, collection_id: &str, schema: ProximaSchema) -> Result<()> {
        // Persist first
        self.persist_schema(collection_id, &schema).await?;
        // Then cache
        self.cache.register_schema(collection_id, schema).await
    }

    async fn get_schema(&self, collection_id: &str, version: u32) -> Result<Option<ProximaSchema>> {
        // Check cache first
        if let Some(schema) = self.cache.get_schema(collection_id, version).await? {
            return Ok(Some(schema));
        }

        // Load from storage
        if let Some(schema) = self.load_schema(collection_id, version).await? {
            // Populate cache
            self.cache
                .register_schema(collection_id, schema.clone())
                .await
                .ok();
            return Ok(Some(schema));
        }

        Ok(None)
    }

    async fn get_latest_schema(&self, collection_id: &str) -> Result<Option<ProximaSchema>> {
        // Try cache first
        if let Some(schema) = self.cache.get_latest_schema(collection_id).await? {
            return Ok(Some(schema));
        }

        // Scan storage for versions
        let versions = self.list_schema_files(collection_id).await?;
        if let Some(max_version) = versions.into_iter().max() {
            return self.get_schema(collection_id, max_version).await;
        }

        Ok(None)
    }

    async fn get_schema_by_fingerprint(&self, fingerprint: u64) -> Result<Option<ProximaSchema>> {
        self.cache.get_schema_by_fingerprint(fingerprint).await
    }

    async fn list_versions(&self, collection_id: &str) -> Result<Vec<SchemaVersionInfo>> {
        // Ensure all versions are loaded into cache
        let versions = self.list_schema_files(collection_id).await?;
        for version in versions {
            if self.cache.get_schema(collection_id, version).await?.is_none() {
                if let Some(schema) = self.load_schema(collection_id, version).await? {
                    self.cache
                        .register_schema(collection_id, schema)
                        .await
                        .ok();
                }
            }
        }

        self.cache.list_versions(collection_id).await
    }

    async fn get_lineage(&self, schema_id: &str) -> Result<Vec<ProximaSchema>> {
        self.cache.get_lineage(schema_id).await
    }

    async fn prune_versions(&self, collection_id: &str, keep_versions: u32) -> Result<u32> {
        // Get versions to delete
        let versions = self.cache.list_versions(collection_id).await?;
        let to_delete: Vec<u32> = versions
            .iter()
            .rev()
            .skip(keep_versions as usize)
            .map(|v| v.version)
            .collect();

        // Delete from storage
        for version in &to_delete {
            let path = self.schema_path(collection_id, *version);
            self.filesystem.delete(&path).await.ok();
        }

        // Prune cache
        self.cache.prune_versions(collection_id, keep_versions).await
    }

    async fn has_schema(&self, collection_id: &str) -> Result<bool> {
        if self.cache.has_schema(collection_id).await? {
            return Ok(true);
        }
        let versions = self.list_schema_files(collection_id).await?;
        Ok(!versions.is_empty())
    }
}

/// Global schema registry singleton.
static GLOBAL_SCHEMA_REGISTRY: std::sync::OnceLock<Arc<dyn SchemaRegistry>> =
    std::sync::OnceLock::new();

/// Get the global schema registry.
pub fn global_schema_registry() -> Option<Arc<dyn SchemaRegistry>> {
    GLOBAL_SCHEMA_REGISTRY.get().cloned()
}

/// Initialize the global schema registry.
pub fn init_global_schema_registry(registry: Arc<dyn SchemaRegistry>) -> Result<()> {
    GLOBAL_SCHEMA_REGISTRY
        .set(registry)
        .map_err(|_| anyhow::anyhow!("Global schema registry already initialized"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_in_memory_registry() {
        let registry = InMemorySchemaRegistry::new();
        let schema = ProximaSchema::vector_record_schema(512);

        registry
            .register_schema("test_collection", schema.clone())
            .await
            .unwrap();

        let retrieved = registry
            .get_schema("test_collection", 0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retrieved.fingerprint, schema.fingerprint);

        let latest = registry
            .get_latest_schema("test_collection")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(latest.version, 0);

        let versions = registry.list_versions("test_collection").await.unwrap();
        assert_eq!(versions.len(), 1);
        assert!(versions[0].is_current);
    }

    #[tokio::test]
    async fn test_fingerprint_lookup() {
        let registry = InMemorySchemaRegistry::new();
        let schema = ProximaSchema::vector_record_schema(768);
        let fingerprint = schema.fingerprint;

        registry
            .register_schema("lookup_test", schema)
            .await
            .unwrap();

        let found = registry
            .get_schema_by_fingerprint(fingerprint)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.vector_dimension(), Some(768));
    }

    #[tokio::test]
    async fn test_prune_versions() {
        let registry = InMemorySchemaRegistry::new();

        // Register multiple versions
        for i in 0..5 {
            let mut schema = ProximaSchema::vector_record_schema(512);
            schema.version = i;
            schema.schema_id = format!("schema_v{}", i);
            registry
                .register_schema("prune_test", schema)
                .await
                .unwrap();
        }

        let versions_before = registry.list_versions("prune_test").await.unwrap();
        assert_eq!(versions_before.len(), 5);

        let pruned = registry.prune_versions("prune_test", 2).await.unwrap();
        assert_eq!(pruned, 3);

        let versions_after = registry.list_versions("prune_test").await.unwrap();
        assert_eq!(versions_after.len(), 2);
    }
}
