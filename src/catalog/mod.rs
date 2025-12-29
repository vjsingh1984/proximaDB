//! Unified Catalog System for ProximaDB
//!
//! Provides a pluggable catalog abstraction supporting multiple backends:
//! - Native: Cloud-first ProximaDB catalog with object storage
//! - AWS Glue: AWS Glue Data Catalog integration (feature-gated)
//! - Unity: Databricks Unity Catalog integration (feature-gated)
//! - Polaris: Apache Polaris (Iceberg REST Catalog) (feature-gated)
//! - Hive: Apache Hive Metastore (Thrift)
//! - Iceberg: Generic Iceberg catalogs (REST, JDBC, Hadoop)
//!
//! Design Principles:
//! - Cloud-first: Object storage as primary, local as cache
//! - Serverless-friendly: Stateless operations, external state
//! - Lakehouse-native: Iceberg/Delta/Hudi table format support
//! - Multi-tenant: Namespace isolation with RBAC

// Internal catalog types (Serde-compatible)
pub mod types;

// Core traits
pub mod traits;

// Metadata cache
pub mod cache;

// Schema utilities (builders, evolution, validation)
pub mod schema;

// Always-available catalog implementations
pub mod native;
pub mod hive;
pub mod iceberg;

// Internal schema registry (multi-model unified catalog)
pub mod internal;

// Feature-gated implementations
#[cfg(feature = "aws")]
pub mod glue;
#[cfg(feature = "unity-catalog")]
pub mod unity;
#[cfg(feature = "polaris-catalog")]
pub mod polaris;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use tokio::sync::RwLock;
use tracing::info;

pub use self::cache::CatalogCache;
pub use self::traits::*;
pub use self::types::*;

/// Catalog manager - manages multiple catalog instances
pub struct CatalogManager {
    /// Registered catalogs by name
    catalogs: RwLock<HashMap<String, Arc<dyn Catalog>>>,
    /// Default catalog name
    default_catalog: RwLock<Option<String>>,
    /// Catalog cache for metadata
    cache: Arc<CatalogCache>,
}

impl CatalogManager {
    /// Create a new catalog manager
    pub fn new() -> Self {
        Self {
            catalogs: RwLock::new(HashMap::new()),
            default_catalog: RwLock::new(None),
            cache: Arc::new(CatalogCache::new(10000, 300)), // 10K entries, 5min TTL
        }
    }

    /// Create a new catalog manager with custom cache settings
    pub fn with_cache(max_entries: usize, ttl_seconds: u64) -> Self {
        Self {
            catalogs: RwLock::new(HashMap::new()),
            default_catalog: RwLock::new(None),
            cache: Arc::new(CatalogCache::new(max_entries, ttl_seconds)),
        }
    }

    /// Register a pre-created catalog
    pub async fn register(&self, catalog: Arc<dyn Catalog>) -> Result<()> {
        let name = catalog.name().to_string();
        info!("Registering catalog: {} (type: {})", name, catalog.catalog_type());

        let mut catalogs = self.catalogs.write().await;
        catalogs.insert(name.clone(), catalog);

        // Set as default if first catalog
        let mut default = self.default_catalog.write().await;
        if default.is_none() {
            *default = Some(name);
        }

        Ok(())
    }

    /// Create and register a native catalog
    pub async fn create_native_catalog(
        &self,
        name: &str,
        storage_url: &str,
    ) -> Result<Arc<dyn Catalog>> {
        use crate::proto::proximadb_v1::NativeCatalogConfig;

        let config = NativeCatalogConfig {
            storage_url: storage_url.to_string(),
            ..Default::default()
        };

        let catalog =
            native::NativeCatalog::new(name.to_string(), config, self.cache.clone()).await?;

        let catalog: Arc<dyn Catalog> = Arc::new(catalog);
        self.register(catalog.clone()).await?;
        Ok(catalog)
    }

    /// Create and register a Hive Metastore catalog
    pub async fn create_hive_catalog(
        &self,
        name: &str,
        thrift_uri: &str,
    ) -> Result<Arc<dyn Catalog>> {
        use crate::proto::proximadb_v1::HiveCatalogConfig;

        let config = HiveCatalogConfig {
            thrift_uri: thrift_uri.to_string(),
            ..Default::default()
        };

        let catalog =
            hive::HiveCatalog::new(name.to_string(), config, self.cache.clone()).await?;

        let catalog: Arc<dyn Catalog> = Arc::new(catalog);
        self.register(catalog.clone()).await?;
        Ok(catalog)
    }

    /// Create and register an Iceberg catalog
    pub async fn create_iceberg_catalog(
        &self,
        name: &str,
        uri: &str,
        warehouse: &str,
    ) -> Result<Arc<dyn Catalog>> {
        use crate::proto::proximadb_v1::IcebergCatalogConfig;

        let config = IcebergCatalogConfig {
            uri: uri.to_string(),
            warehouse: warehouse.to_string(),
            ..Default::default()
        };

        let catalog =
            iceberg::IcebergCatalog::new(name.to_string(), config, self.cache.clone()).await?;

        let catalog: Arc<dyn Catalog> = Arc::new(catalog);
        self.register(catalog.clone()).await?;
        Ok(catalog)
    }

    /// Get a catalog by name
    pub async fn get_catalog(&self, name: &str) -> Result<Arc<dyn Catalog>> {
        let catalogs = self.catalogs.read().await;
        catalogs
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow!("Catalog '{}' not found", name))
    }

    /// Get the default catalog
    pub async fn default_catalog(&self) -> Result<Arc<dyn Catalog>> {
        let default = self.default_catalog.read().await;
        let name = default
            .as_ref()
            .ok_or_else(|| anyhow!("No default catalog configured"))?;

        self.get_catalog(name).await
    }

    /// Set the default catalog
    pub async fn set_default_catalog(&self, name: &str) -> Result<()> {
        // Verify catalog exists
        let catalogs = self.catalogs.read().await;
        if !catalogs.contains_key(name) {
            return Err(anyhow!("Catalog '{}' not found", name));
        }
        drop(catalogs);

        let mut default = self.default_catalog.write().await;
        *default = Some(name.to_string());
        Ok(())
    }

    /// List all registered catalogs
    pub async fn list_catalogs(&self) -> Vec<String> {
        let catalogs = self.catalogs.read().await;
        catalogs.keys().cloned().collect()
    }

    /// Unregister a catalog
    pub async fn unregister_catalog(&self, name: &str) -> Result<bool> {
        let mut catalogs = self.catalogs.write().await;
        let removed = catalogs.remove(name).is_some();

        // Clear default if it was this catalog
        if removed {
            let mut default = self.default_catalog.write().await;
            if default.as_deref() == Some(name) {
                *default = catalogs.keys().next().cloned();
            }
        }

        Ok(removed)
    }

    /// Resolve a fully-qualified table name (catalog.namespace.table)
    pub async fn resolve_table(&self, fqn: &str) -> Result<(Arc<dyn Catalog>, TableIdentifier)> {
        let parts: Vec<&str> = fqn.split('.').collect();

        match parts.len() {
            1 => {
                // Just table name - use default catalog and namespace
                let catalog = self.default_catalog().await?;
                let id = TableIdentifier::new(vec!["default".to_string()], parts[0].to_string());
                Ok((catalog, id))
            }
            2 => {
                // namespace.table - use default catalog
                let catalog = self.default_catalog().await?;
                let id = TableIdentifier::new(vec![parts[0].to_string()], parts[1].to_string());
                Ok((catalog, id))
            }
            3 => {
                // catalog.namespace.table
                let catalog = self.get_catalog(parts[0]).await?;
                let id = TableIdentifier::new(vec![parts[1].to_string()], parts[2].to_string());
                Ok((catalog, id))
            }
            _ => {
                // catalog.ns1.ns2...nsN.table
                let catalog = self.get_catalog(parts[0]).await?;
                let namespace: Vec<String> = parts[1..parts.len() - 1]
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
                let table = parts.last().unwrap().to_string();
                let id = TableIdentifier::new(namespace, table);
                Ok((catalog, id))
            }
        }
    }

    /// Get cache reference for direct access
    pub fn cache(&self) -> Arc<CatalogCache> {
        self.cache.clone()
    }
}

impl Default for CatalogManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Table identifier with namespace path
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TableIdentifier {
    /// Namespace path (e.g., ["db", "schema"])
    pub namespace: Vec<String>,
    /// Table name
    pub name: String,
}

impl TableIdentifier {
    pub fn new(namespace: Vec<String>, name: String) -> Self {
        Self { namespace, name }
    }

    /// Parse from string (e.g., "db.schema.table")
    pub fn parse(s: &str) -> Self {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() == 1 {
            Self::new(vec![], parts[0].to_string())
        } else {
            let namespace: Vec<String> = parts[..parts.len() - 1]
                .iter()
                .map(|s| s.to_string())
                .collect();
            let name = parts.last().unwrap().to_string();
            Self::new(namespace, name)
        }
    }

    /// Convert to fully-qualified name
    pub fn to_fqn(&self) -> String {
        if self.namespace.is_empty() {
            self.name.clone()
        } else {
            format!("{}.{}", self.namespace.join("."), self.name)
        }
    }
}

impl std::fmt::Display for TableIdentifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_fqn())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_table_identifier_parse() {
        let id = TableIdentifier::parse("db.schema.users");
        assert_eq!(id.namespace, vec!["db", "schema"]);
        assert_eq!(id.name, "users");
    }

    #[test]
    fn test_table_identifier_simple() {
        let id = TableIdentifier::parse("users");
        assert!(id.namespace.is_empty());
        assert_eq!(id.name, "users");
    }

    #[test]
    fn test_table_identifier_to_fqn() {
        let id = TableIdentifier::new(
            vec!["db".to_string(), "schema".to_string()],
            "users".to_string(),
        );
        assert_eq!(id.to_fqn(), "db.schema.users");
    }

    #[tokio::test]
    async fn test_catalog_manager_new() {
        let manager = CatalogManager::new();
        assert!(manager.list_catalogs().await.is_empty());
    }
}
