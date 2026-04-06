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

// Partition pruning for query optimization
pub mod partition_pruning;

// Always-available catalog implementations
pub mod hive;
pub mod iceberg;
pub mod native;

// Internal schema registry (multi-model unified catalog)
pub mod internal;

// Catalog federation (unified view across internal and external catalogs)
pub mod federation;

// Feature-gated implementations
#[cfg(feature = "delta-lake")]
pub mod delta;
#[cfg(feature = "aws")]
pub mod glue;
#[cfg(feature = "polaris-catalog")]
pub mod polaris;
#[cfg(feature = "unity-catalog")]
pub mod unity;

// Re-exports for feature-gated catalogs
#[cfg(feature = "delta-lake")]
pub use delta::{DeltaCatalog, DeltaCatalogConfig};
#[cfg(feature = "aws")]
pub use glue::GlueCatalog;
#[cfg(feature = "polaris-catalog")]
pub use polaris::PolarisCatalog;
#[cfg(feature = "unity-catalog")]
pub use unity::UnityCatalog;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use tokio::sync::RwLock;
use tracing::info;

pub use self::cache::CatalogCache;
pub use self::partition_pruning::{
    PartitionInfo, PartitionPruner, PruningResult, parse_partition_path,
};
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
        info!(
            "Registering catalog: {} (type: {})",
            name,
            catalog.catalog_type()
        );

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

        let catalog = hive::HiveCatalog::new(name.to_string(), config, self.cache.clone()).await?;

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

    /// Create and register an AWS Glue catalog
    ///
    /// Requires the `aws` feature flag to be enabled.
    ///
    /// # Arguments
    /// * `name` - Catalog name
    /// * `region` - AWS region (e.g., "us-east-1")
    /// * `catalog_id` - AWS account ID (optional, uses default account if empty)
    ///
    /// # Example
    /// ```ignore
    /// let catalog = manager.create_glue_catalog("glue", "us-east-1", "123456789012").await?;
    /// ```
    #[cfg(feature = "aws")]
    pub async fn create_glue_catalog(
        &self,
        name: &str,
        region: &str,
        catalog_id: &str,
    ) -> Result<Arc<dyn Catalog>> {
        use crate::proto::proximadb_v1::GlueCatalogConfig;

        let config = GlueCatalogConfig {
            region: region.to_string(),
            catalog_id: catalog_id.to_string(),
            ..Default::default()
        };

        let catalog = glue::GlueCatalog::new(name.to_string(), config, self.cache.clone()).await?;

        let catalog: Arc<dyn Catalog> = Arc::new(catalog);
        self.register(catalog.clone()).await?;
        Ok(catalog)
    }

    /// Create and register an AWS Glue catalog (stub for non-AWS builds)
    #[cfg(not(feature = "aws"))]
    pub async fn create_glue_catalog(
        &self,
        _name: &str,
        _region: &str,
        _catalog_id: &str,
    ) -> Result<Arc<dyn Catalog>> {
        Err(anyhow!(
            "AWS Glue catalog requires the 'aws' feature flag. \
             Build with: cargo build --features aws"
        ))
    }

    /// Create and register a Databricks Unity catalog
    ///
    /// Requires the `unity-catalog` feature flag to be enabled.
    ///
    /// # Arguments
    /// * `name` - Catalog name
    /// * `workspace_url` - Databricks workspace URL (e.g., "https://xxx.cloud.databricks.com")
    /// * `token` - Personal access token or OAuth token
    /// * `catalog_name` - Unity catalog name within the workspace
    ///
    /// # Example
    /// ```ignore
    /// let catalog = manager.create_unity_catalog(
    ///     "unity",
    ///     "https://my-workspace.cloud.databricks.com",
    ///     "dapi123...",
    ///     "main"
    /// ).await?;
    /// ```
    #[cfg(feature = "unity-catalog")]
    pub async fn create_unity_catalog(
        &self,
        name: &str,
        workspace_url: &str,
        token: &str,
        catalog_name: &str,
    ) -> Result<Arc<dyn Catalog>> {
        use crate::proto::proximadb_v1::UnityCatalogConfig;

        let config = UnityCatalogConfig {
            workspace_url: workspace_url.to_string(),
            token: token.to_string(),
            catalog_name: catalog_name.to_string(),
            ..Default::default()
        };

        let catalog =
            unity::UnityCatalog::new(name.to_string(), config, self.cache.clone()).await?;

        let catalog: Arc<dyn Catalog> = Arc::new(catalog);
        self.register(catalog.clone()).await?;
        Ok(catalog)
    }

    /// Create and register a Unity catalog (stub for non-Unity builds)
    #[cfg(not(feature = "unity-catalog"))]
    pub async fn create_unity_catalog(
        &self,
        _name: &str,
        _workspace_url: &str,
        _token: &str,
        _catalog_name: &str,
    ) -> Result<Arc<dyn Catalog>> {
        Err(anyhow!(
            "Databricks Unity catalog requires the 'unity-catalog' feature flag. \
             Build with: cargo build --features unity-catalog"
        ))
    }

    /// Create and register an Apache Polaris catalog (Iceberg REST)
    ///
    /// Requires the `polaris-catalog` feature flag to be enabled.
    ///
    /// # Arguments
    /// * `name` - Catalog name
    /// * `uri` - Polaris server URI
    /// * `warehouse` - Warehouse name
    /// * `credential` - OAuth credential (client_id:client_secret format)
    ///
    /// # Example
    /// ```ignore
    /// let catalog = manager.create_polaris_catalog(
    ///     "polaris",
    ///     "https://polaris.example.com",
    ///     "my_warehouse",
    ///     "client_id:client_secret"
    /// ).await?;
    /// ```
    #[cfg(feature = "polaris-catalog")]
    pub async fn create_polaris_catalog(
        &self,
        name: &str,
        uri: &str,
        warehouse: &str,
        credential: &str,
    ) -> Result<Arc<dyn Catalog>> {
        use crate::proto::proximadb_v1::PolarisCatalogConfig;

        let config = PolarisCatalogConfig {
            uri: uri.to_string(),
            warehouse: warehouse.to_string(),
            credential: credential.to_string(),
            ..Default::default()
        };

        let catalog =
            polaris::PolarisCatalog::new(name.to_string(), config, self.cache.clone()).await?;

        let catalog: Arc<dyn Catalog> = Arc::new(catalog);
        self.register(catalog.clone()).await?;
        Ok(catalog)
    }

    /// Create and register a Polaris catalog (stub for non-Polaris builds)
    #[cfg(not(feature = "polaris-catalog"))]
    pub async fn create_polaris_catalog(
        &self,
        _name: &str,
        _uri: &str,
        _warehouse: &str,
        _credential: &str,
    ) -> Result<Arc<dyn Catalog>> {
        Err(anyhow!(
            "Apache Polaris catalog requires the 'polaris-catalog' feature flag. \
             Build with: cargo build --features polaris-catalog"
        ))
    }

    /// Create and register a Delta Lake catalog
    ///
    /// Requires the `delta-lake` feature flag to be enabled.
    ///
    /// # Arguments
    /// * `name` - Catalog name
    /// * `storage_url` - Storage URL (s3://bucket/path, file:///path, etc.)
    ///
    /// # Example
    /// ```ignore
    /// let catalog = manager.create_delta_catalog(
    ///     "delta",
    ///     "s3://my-bucket/delta-tables"
    /// ).await?;
    /// ```
    #[cfg(feature = "delta-lake")]
    pub async fn create_delta_catalog(
        &self,
        name: &str,
        storage_url: &str,
    ) -> Result<Arc<dyn Catalog>> {
        use delta::DeltaCatalogConfig;

        let config = DeltaCatalogConfig {
            storage_url: storage_url.to_string(),
            ..Default::default()
        };

        let catalog =
            delta::DeltaCatalog::new(name.to_string(), config, self.cache.clone()).await?;

        let catalog: Arc<dyn Catalog> = Arc::new(catalog);
        self.register(catalog.clone()).await?;
        Ok(catalog)
    }

    /// Create and register a Delta Lake catalog (stub for non-Delta builds)
    #[cfg(not(feature = "delta-lake"))]
    pub async fn create_delta_catalog(
        &self,
        _name: &str,
        _storage_url: &str,
    ) -> Result<Arc<dyn Catalog>> {
        Err(anyhow!(
            "Delta Lake catalog requires the 'delta-lake' feature flag. \
             Build with: cargo build --features delta-lake"
        ))
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
                let table = parts
                    .last()
                    .ok_or_else(|| anyhow!("Invalid table name: missing table component"))?
                    .to_string();
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
    /// Create a new table identifier with the given namespace path and table name
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
            let name = parts[parts.len() - 1].to_string();
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

    // ========================
    // TableIdentifier Tests
    // ========================

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

    #[test]
    fn test_table_identifier_single_namespace() {
        let id = TableIdentifier::parse("mydb.users");
        assert_eq!(id.namespace, vec!["mydb"]);
        assert_eq!(id.name, "users");
    }

    #[test]
    fn test_table_identifier_display() {
        let id = TableIdentifier::new(
            vec!["catalog".to_string(), "schema".to_string()],
            "table".to_string(),
        );
        assert_eq!(format!("{}", id), "catalog.schema.table");
    }

    #[test]
    fn test_table_identifier_empty_namespace_fqn() {
        let id = TableIdentifier::new(vec![], "users".to_string());
        assert_eq!(id.to_fqn(), "users");
    }

    #[test]
    fn test_table_identifier_equality() {
        let id1 = TableIdentifier::new(vec!["db".to_string()], "table".to_string());
        let id2 = TableIdentifier::new(vec!["db".to_string()], "table".to_string());
        let id3 = TableIdentifier::new(vec!["other".to_string()], "table".to_string());

        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
    }

    // ========================
    // CatalogManager Tests
    // ========================

    #[tokio::test]
    async fn test_catalog_manager_new() {
        let manager = CatalogManager::new();
        assert!(manager.list_catalogs().await.is_empty());
    }

    #[tokio::test]
    async fn test_catalog_manager_with_cache() {
        let manager = CatalogManager::with_cache(5000, 600);
        assert!(manager.list_catalogs().await.is_empty());
    }

    #[tokio::test]
    async fn test_catalog_manager_default() {
        let manager = CatalogManager::default();
        assert!(manager.list_catalogs().await.is_empty());
    }

    #[tokio::test]
    async fn test_catalog_manager_no_default_catalog() {
        let manager = CatalogManager::new();
        let result = manager.default_catalog().await;
        assert!(result.is_err());
        let err = result.err().expect("Expected error result");
        assert!(err.to_string().contains("No default catalog"));
    }

    #[tokio::test]
    async fn test_catalog_manager_get_nonexistent() {
        let manager = CatalogManager::new();
        let result = manager.get_catalog("nonexistent").await;
        assert!(result.is_err());
        let err = result.err().expect("Expected error result");
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_catalog_manager_set_default_nonexistent() {
        let manager = CatalogManager::new();
        let result = manager.set_default_catalog("nonexistent").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_catalog_manager_unregister_nonexistent() {
        let manager = CatalogManager::new();
        let result = manager.unregister_catalog("nonexistent").await;
        assert!(result.is_ok());
        assert!(!result.expect("Expected Ok result")); // Returns false for nonexistent
    }

    #[tokio::test]
    async fn test_catalog_manager_cache_access() {
        let manager = CatalogManager::new();
        let cache = manager.cache();
        // Cache should be valid
        assert!(Arc::strong_count(&cache) >= 1);
    }

    // ========================
    // Factory Methods Tests (Feature Stubs)
    // ========================

    #[tokio::test]
    #[cfg(not(feature = "aws"))]
    async fn test_create_glue_catalog_without_feature() {
        let manager = CatalogManager::new();
        let result = manager
            .create_glue_catalog("glue", "us-east-1", "123456789012")
            .await;
        assert!(result.is_err());
        let err = result.err().expect("Expected error result");
        assert!(err.to_string().contains("aws"));
    }

    #[tokio::test]
    #[cfg(not(feature = "unity-catalog"))]
    async fn test_create_unity_catalog_without_feature() {
        let manager = CatalogManager::new();
        let result = manager
            .create_unity_catalog(
                "unity",
                "https://example.cloud.databricks.com",
                "token",
                "main",
            )
            .await;
        assert!(result.is_err());
        let err = result.err().expect("Expected error result");
        assert!(err.to_string().contains("unity-catalog"));
    }

    #[tokio::test]
    #[cfg(not(feature = "polaris-catalog"))]
    async fn test_create_polaris_catalog_without_feature() {
        let manager = CatalogManager::new();
        let result = manager
            .create_polaris_catalog(
                "polaris",
                "https://polaris.example.com",
                "warehouse",
                "cred",
            )
            .await;
        assert!(result.is_err());
        let err = result.err().expect("Expected error result");
        assert!(err.to_string().contains("polaris-catalog"));
    }

    #[tokio::test]
    #[cfg(not(feature = "delta-lake"))]
    async fn test_create_delta_catalog_without_feature() {
        let manager = CatalogManager::new();
        let result = manager
            .create_delta_catalog("delta", "file:///tmp/delta")
            .await;
        assert!(result.is_err());
        let err = result.err().expect("Expected error result");
        assert!(err.to_string().contains("delta-lake"));
    }

    // ========================
    // Iceberg Catalog Tests
    // ========================

    #[tokio::test]
    async fn test_create_iceberg_catalog() {
        let manager = CatalogManager::new();
        let temp_dir = std::env::temp_dir().join("proximadb_test_iceberg_catalog");
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;

        let result = manager
            .create_iceberg_catalog(
                "iceberg",
                "memory://",
                &format!("file://{}", temp_dir.display()),
            )
            .await;

        assert!(result.is_ok());
        let catalogs = manager.list_catalogs().await;
        assert_eq!(catalogs.len(), 1);
        assert!(catalogs.contains(&"iceberg".to_string()));

        // Clean up
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[tokio::test]
    async fn test_iceberg_catalog_operations() {
        let manager = CatalogManager::new();
        let temp_dir = std::env::temp_dir().join("proximadb_test_iceberg_ops");
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;

        let catalog = manager
            .create_iceberg_catalog(
                "test_iceberg",
                "memory://",
                &format!("file://{}", temp_dir.display()),
            )
            .await
            .expect("Expected catalog creation to succeed");

        assert_eq!(catalog.name(), "test_iceberg");
        assert_eq!(catalog.catalog_type(), "iceberg");

        // Health check
        let health = catalog
            .health_check()
            .await
            .expect("Expected health check to succeed");
        assert!(health.is_healthy);

        // Clean up
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    // ========================
    // Native Catalog Tests
    // ========================

    #[tokio::test]
    async fn test_create_native_catalog() {
        let manager = CatalogManager::new();
        let temp_dir = std::env::temp_dir().join("proximadb_test_native_catalog");
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;

        let result = manager
            .create_native_catalog("native", &format!("file://{}", temp_dir.display()))
            .await;

        assert!(result.is_ok());
        let catalogs = manager.list_catalogs().await;
        assert!(catalogs.contains(&"native".to_string()));

        // Clean up
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[tokio::test]
    async fn test_native_catalog_first_is_default() {
        let manager = CatalogManager::new();
        let temp_dir = std::env::temp_dir().join("proximadb_test_native_default");
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;

        manager
            .create_native_catalog("first", &format!("file://{}", temp_dir.display()))
            .await
            .expect("Expected catalog creation to succeed");

        let default = manager
            .default_catalog()
            .await
            .expect("Expected default catalog to exist");
        assert_eq!(default.name(), "first");

        // Clean up
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    // ========================
    // Hive Catalog Tests
    // ========================

    #[tokio::test]
    async fn test_create_hive_catalog() {
        let manager = CatalogManager::new();

        // Hive catalog creation should work (even without a real Thrift server)
        let result = manager
            .create_hive_catalog("hive", "thrift://localhost:9083")
            .await;

        assert!(result.is_ok());
        assert!(manager.list_catalogs().await.contains(&"hive".to_string()));
    }

    // ========================
    // Multi-catalog Tests
    // ========================

    #[tokio::test]
    async fn test_multiple_catalogs() {
        let manager = CatalogManager::new();
        let temp_dir1 = std::env::temp_dir().join("proximadb_test_multi1");
        let temp_dir2 = std::env::temp_dir().join("proximadb_test_multi2");
        let _ = tokio::fs::remove_dir_all(&temp_dir1).await;
        let _ = tokio::fs::remove_dir_all(&temp_dir2).await;

        manager
            .create_native_catalog("catalog1", &format!("file://{}", temp_dir1.display()))
            .await
            .expect("Expected catalog creation to succeed");
        manager
            .create_iceberg_catalog(
                "catalog2",
                "memory://",
                &format!("file://{}", temp_dir2.display()),
            )
            .await
            .expect("Expected catalog creation to succeed");

        let catalogs = manager.list_catalogs().await;
        assert_eq!(catalogs.len(), 2);
        assert!(catalogs.contains(&"catalog1".to_string()));
        assert!(catalogs.contains(&"catalog2".to_string()));

        // Clean up
        let _ = tokio::fs::remove_dir_all(&temp_dir1).await;
        let _ = tokio::fs::remove_dir_all(&temp_dir2).await;
    }

    #[tokio::test]
    async fn test_set_and_get_default_catalog() {
        let manager = CatalogManager::new();
        let temp_dir1 = std::env::temp_dir().join("proximadb_test_default1");
        let temp_dir2 = std::env::temp_dir().join("proximadb_test_default2");
        let _ = tokio::fs::remove_dir_all(&temp_dir1).await;
        let _ = tokio::fs::remove_dir_all(&temp_dir2).await;

        manager
            .create_native_catalog("cat1", &format!("file://{}", temp_dir1.display()))
            .await
            .expect("Expected catalog creation to succeed");
        manager
            .create_native_catalog("cat2", &format!("file://{}", temp_dir2.display()))
            .await
            .expect("Expected catalog creation to succeed");

        // First catalog should be default
        let default = manager
            .default_catalog()
            .await
            .expect("Expected default catalog to exist");
        assert_eq!(default.name(), "cat1");

        // Change default
        manager
            .set_default_catalog("cat2")
            .await
            .expect("Expected set_default_catalog to succeed");
        let new_default = manager
            .default_catalog()
            .await
            .expect("Expected default catalog to exist");
        assert_eq!(new_default.name(), "cat2");

        // Clean up
        let _ = tokio::fs::remove_dir_all(&temp_dir1).await;
        let _ = tokio::fs::remove_dir_all(&temp_dir2).await;
    }

    #[tokio::test]
    async fn test_unregister_catalog() {
        let manager = CatalogManager::new();
        let temp_dir = std::env::temp_dir().join("proximadb_test_unregister");
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;

        manager
            .create_native_catalog("to_remove", &format!("file://{}", temp_dir.display()))
            .await
            .expect("Expected catalog creation to succeed");

        assert_eq!(manager.list_catalogs().await.len(), 1);

        let removed = manager
            .unregister_catalog("to_remove")
            .await
            .expect("Expected unregister to succeed");
        assert!(removed);
        assert!(manager.list_catalogs().await.is_empty());

        // Clean up
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[tokio::test]
    async fn test_unregister_default_catalog() {
        let manager = CatalogManager::new();
        let temp_dir1 = std::env::temp_dir().join("proximadb_test_unreg_def1");
        let temp_dir2 = std::env::temp_dir().join("proximadb_test_unreg_def2");
        let _ = tokio::fs::remove_dir_all(&temp_dir1).await;
        let _ = tokio::fs::remove_dir_all(&temp_dir2).await;

        manager
            .create_native_catalog("cat1", &format!("file://{}", temp_dir1.display()))
            .await
            .expect("Expected catalog creation to succeed");
        manager
            .create_native_catalog("cat2", &format!("file://{}", temp_dir2.display()))
            .await
            .expect("Expected catalog creation to succeed");

        // Remove default catalog
        manager
            .unregister_catalog("cat1")
            .await
            .expect("Expected unregister to succeed");

        // cat2 should become the new default
        let default = manager
            .default_catalog()
            .await
            .expect("Expected default catalog to exist");
        assert_eq!(default.name(), "cat2");

        // Clean up
        let _ = tokio::fs::remove_dir_all(&temp_dir1).await;
        let _ = tokio::fs::remove_dir_all(&temp_dir2).await;
    }

    // ========================
    // Resolve Table Tests
    // ========================

    #[tokio::test]
    async fn test_resolve_table_simple() {
        let manager = CatalogManager::new();
        let temp_dir = std::env::temp_dir().join("proximadb_test_resolve");
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;

        manager
            .create_native_catalog("default", &format!("file://{}", temp_dir.display()))
            .await
            .expect("Expected catalog creation to succeed");

        // Simple table name
        let (catalog, id) = manager
            .resolve_table("users")
            .await
            .expect("Expected resolve_table to succeed");
        assert_eq!(catalog.name(), "default");
        assert_eq!(id.name, "users");
        assert_eq!(id.namespace, vec!["default"]);

        // Clean up
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[tokio::test]
    async fn test_resolve_table_with_namespace() {
        let manager = CatalogManager::new();
        let temp_dir = std::env::temp_dir().join("proximadb_test_resolve_ns");
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;

        manager
            .create_native_catalog("default", &format!("file://{}", temp_dir.display()))
            .await
            .expect("Expected catalog creation to succeed");

        // namespace.table
        let (catalog, id) = manager
            .resolve_table("mydb.users")
            .await
            .expect("Expected resolve_table to succeed");
        assert_eq!(catalog.name(), "default");
        assert_eq!(id.name, "users");
        assert_eq!(id.namespace, vec!["mydb"]);

        // Clean up
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[tokio::test]
    async fn test_resolve_table_fully_qualified() {
        let manager = CatalogManager::new();
        let temp_dir = std::env::temp_dir().join("proximadb_test_resolve_fqn");
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;

        manager
            .create_native_catalog("mycat", &format!("file://{}", temp_dir.display()))
            .await
            .expect("Expected catalog creation to succeed");

        // catalog.namespace.table
        let (catalog, id) = manager
            .resolve_table("mycat.mydb.users")
            .await
            .expect("Expected resolve_table to succeed");
        assert_eq!(catalog.name(), "mycat");
        assert_eq!(id.name, "users");
        assert_eq!(id.namespace, vec!["mydb"]);

        // Clean up
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[tokio::test]
    async fn test_resolve_table_multi_level_namespace() {
        let manager = CatalogManager::new();
        let temp_dir = std::env::temp_dir().join("proximadb_test_resolve_multi");
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;

        manager
            .create_native_catalog("catalog", &format!("file://{}", temp_dir.display()))
            .await
            .expect("Expected catalog creation to succeed");

        // catalog.ns1.ns2.table
        let (catalog, id) = manager
            .resolve_table("catalog.db.schema.users")
            .await
            .expect("Expected resolve_table to succeed");
        assert_eq!(catalog.name(), "catalog");
        assert_eq!(id.name, "users");
        assert_eq!(id.namespace, vec!["db", "schema"]);

        // Clean up
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    // ========================
    // CatalogManager Creation Tests
    // ========================

    #[tokio::test]
    async fn test_catalog_manager_creation() {
        // Default construction
        let manager = CatalogManager::new();
        assert!(manager.list_catalogs().await.is_empty());

        // No default catalog should be set
        let default_result = manager.default_catalog().await;
        assert!(default_result.is_err());

        // With custom cache settings
        let manager_custom = CatalogManager::with_cache(50000, 600);
        assert!(manager_custom.list_catalogs().await.is_empty());

        // Default trait implementation
        let manager_default = CatalogManager::default();
        assert!(manager_default.list_catalogs().await.is_empty());

        // Cache should always be accessible
        let cache = manager.cache();
        assert!(std::sync::Arc::strong_count(&cache) >= 1);
    }

    // ========================
    // Catalog Namespace Operations Tests
    // ========================

    #[tokio::test]
    async fn test_catalog_namespace_operations() {
        let manager = CatalogManager::new();
        let temp_dir = std::env::temp_dir().join("proximadb_test_ns_ops");
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;

        // Create a native catalog so we have something to operate against
        let catalog = manager
            .create_native_catalog("test_ns", &format!("file://{}", temp_dir.display()))
            .await
            .expect("Expected native catalog creation to succeed");

        // Catalog should be registered
        let catalogs = manager.list_catalogs().await;
        assert!(catalogs.contains(&"test_ns".to_string()));

        // It should be the default (first registered)
        let default = manager
            .default_catalog()
            .await
            .expect("Expected default catalog");
        assert_eq!(default.name(), "test_ns");
        assert_eq!(default.catalog_type(), "native");

        // Create a namespace via the catalog trait
        let ns = catalog
            .create_namespace(
                &["analytics".to_string()],
                {
                    let mut props = std::collections::HashMap::new();
                    props.insert("owner".to_string(), "data_team".to_string());
                    props
                },
            )
            .await
            .expect("Expected namespace creation to succeed");

        assert_eq!(ns.levels, vec!["analytics"]);
        assert_eq!(
            ns.properties.get("owner"),
            Some(&"data_team".to_string())
        );

        // Check namespace exists
        let exists = catalog
            .namespace_exists(&["analytics".to_string()])
            .await
            .expect("Expected namespace_exists to succeed");
        assert!(exists);

        // List namespaces
        let namespaces = catalog
            .list_namespaces(None)
            .await
            .expect("Expected list_namespaces to succeed");
        assert!(!namespaces.is_empty());

        // Drop namespace
        let dropped = catalog
            .drop_namespace(&["analytics".to_string()], false)
            .await
            .expect("Expected drop_namespace to succeed");
        assert!(dropped);

        // Clean up
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    // ========================
    // Catalog Table Registration Tests
    // ========================

    #[tokio::test]
    async fn test_catalog_table_registration() {
        let manager = CatalogManager::new();
        let temp_dir = std::env::temp_dir().join("proximadb_test_table_reg");
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;

        let catalog = manager
            .create_native_catalog("test_tbl", &format!("file://{}", temp_dir.display()))
            .await
            .expect("Expected native catalog creation to succeed");

        // Create namespace first
        catalog
            .create_namespace(
                &["default".to_string()],
                std::collections::HashMap::new(),
            )
            .await
            .expect("Expected namespace creation to succeed");

        // Create a table with schema
        let table_id = TableIdentifier::new(vec!["default".to_string()], "vectors".to_string());

        let schema = types::CatalogTableSchema::new("vectors")
            .with_column(types::CatalogColumn::new(
                1,
                "id",
                types::CatalogDataType::String,
            ).nullable(false))
            .with_column(types::CatalogColumn::new(
                2,
                "embedding",
                types::CatalogDataType::Vector,
            ))
            .with_column(types::CatalogColumn::new(
                3,
                "category",
                types::CatalogDataType::String,
            ))
            .with_primary_key(vec!["id".to_string()]);

        let created_schema = catalog
            .create_table(&table_id, schema)
            .await
            .expect("Expected table creation to succeed");

        assert_eq!(created_schema.name, "vectors");
        assert_eq!(created_schema.columns.len(), 3);
        assert_eq!(created_schema.primary_key, vec!["id"]);

        // Verify table exists
        let exists = catalog
            .table_exists(&table_id)
            .await
            .expect("Expected table_exists to succeed");
        assert!(exists);

        // List tables in namespace
        let tables = catalog
            .list_tables(&["default".to_string()])
            .await
            .expect("Expected list_tables to succeed");
        assert!(!tables.is_empty());
        assert!(tables.iter().any(|t| t.name == "vectors"));

        // Resolve the table through the manager
        let (resolved_catalog, resolved_id) = manager
            .resolve_table("test_tbl.default.vectors")
            .await
            .expect("Expected resolve_table to succeed");
        assert_eq!(resolved_catalog.name(), "test_tbl");
        assert_eq!(resolved_id.name, "vectors");

        // Clean up
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    // ========================
    // Catalog Schema Introspection Tests
    // ========================

    #[tokio::test]
    async fn test_catalog_schema_introspection() {
        let manager = CatalogManager::new();
        let temp_dir = std::env::temp_dir().join("proximadb_test_schema_intro");
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;

        let catalog = manager
            .create_native_catalog("test_intro", &format!("file://{}", temp_dir.display()))
            .await
            .expect("Expected native catalog creation to succeed");

        // Create namespace
        catalog
            .create_namespace(
                &["mydb".to_string()],
                std::collections::HashMap::new(),
            )
            .await
            .expect("Expected namespace creation to succeed");

        // Create a table with a rich schema
        let table_id = TableIdentifier::new(vec!["mydb".to_string()], "products".to_string());

        let schema = types::CatalogTableSchema::new("products")
            .with_column(
                types::CatalogColumn::new(1, "product_id", types::CatalogDataType::Uuid)
                    .nullable(false)
                    .with_comment("Primary key UUID"),
            )
            .with_column(
                types::CatalogColumn::new(2, "name", types::CatalogDataType::String)
                    .nullable(false),
            )
            .with_column(
                types::CatalogColumn::new(3, "price", types::CatalogDataType::Float64)
                    .with_default("0.0"),
            )
            .with_column(
                types::CatalogColumn::new(4, "created_at", types::CatalogDataType::TimestampTz),
            )
            .with_column(
                types::CatalogColumn::new(5, "embedding", types::CatalogDataType::Vector),
            )
            .with_primary_key(vec!["product_id".to_string()])
            .with_index(types::CatalogIndex::new(
                "idx_name",
                vec!["name".to_string()],
                types::CatalogIndexType::BTree,
            ));

        catalog
            .create_table(&table_id, schema)
            .await
            .expect("Expected table creation to succeed");

        // Retrieve the schema and introspect it
        let retrieved = catalog
            .get_table(&table_id)
            .await
            .expect("Expected get_table to succeed");

        assert_eq!(retrieved.name, "products");
        assert_eq!(retrieved.columns.len(), 5);
        assert_eq!(retrieved.schema_version, 1);

        // Verify individual columns
        let id_col = retrieved.columns.iter().find(|c| c.name == "product_id")
            .expect("product_id column should exist");
        assert!(!id_col.nullable);
        assert_eq!(id_col.data_type, types::CatalogDataType::Uuid);
        assert_eq!(id_col.comment.as_deref(), Some("Primary key UUID"));

        let price_col = retrieved.columns.iter().find(|c| c.name == "price")
            .expect("price column should exist");
        assert_eq!(price_col.data_type, types::CatalogDataType::Float64);
        assert_eq!(price_col.default_value.as_deref(), Some("0.0"));
        assert!(price_col.nullable); // Default is true

        let embed_col = retrieved.columns.iter().find(|c| c.name == "embedding")
            .expect("embedding column should exist");
        assert_eq!(embed_col.data_type, types::CatalogDataType::Vector);

        // Verify primary key
        assert_eq!(retrieved.primary_key, vec!["product_id"]);

        // Verify index
        assert!(!retrieved.indexes.is_empty());
        let idx = &retrieved.indexes[0];
        assert_eq!(idx.name, "idx_name");
        assert_eq!(idx.columns, vec!["name"]);
        assert_eq!(idx.index_type, types::CatalogIndexType::BTree);

        // Check schema version
        let version = catalog
            .get_schema_version(&table_id)
            .await
            .expect("Expected get_schema_version to succeed");
        assert_eq!(version, 1);

        // Clean up
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }
}
