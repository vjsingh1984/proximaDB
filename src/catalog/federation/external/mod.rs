//! # External Catalog Interface
//!
//! Defines the interface for external catalogs (Iceberg, Delta, Hive, etc.)
//! that can be registered with the federated catalog.

use std::collections::HashMap;

use anyhow::Result;
use arrow_schema::Schema as ArrowSchema;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::ConstraintSupport;

// ============================================================================
// External Catalog Types
// ============================================================================

/// Types of external catalogs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ExternalCatalogType {
    /// Apache Iceberg catalog (REST, JDBC, Hadoop)
    Iceberg,
    /// Delta Lake catalog
    DeltaLake,
    /// Apache Hudi catalog
    Hudi,
    /// Apache Hive Metastore
    Hive,
    /// AWS Glue Data Catalog
    Glue,
    /// Databricks Unity Catalog
    Unity,
    /// Apache Polaris (Iceberg REST)
    Polaris,
    /// LanceDB
    LanceDb,
    /// DuckDB
    DuckDb,
    /// Custom/Other
    Custom,
}

impl ExternalCatalogType {
    /// Get default constraint support for this catalog type
    pub fn default_constraint_support(&self) -> ConstraintSupport {
        match self {
            ExternalCatalogType::Iceberg => ConstraintSupport::iceberg(),
            ExternalCatalogType::DeltaLake => ConstraintSupport::delta_lake(),
            ExternalCatalogType::LanceDb => ConstraintSupport::lancedb(),
            _ => ConstraintSupport::parquet(), // Most others have minimal support
        }
    }
}

// ============================================================================
// External Catalog Configuration
// ============================================================================

/// Configuration for external catalogs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalCatalogConfig {
    /// Catalog name
    pub name: String,
    /// Catalog type
    pub catalog_type: ExternalCatalogType,
    /// Connection URI (REST endpoint, JDBC URL, etc.)
    pub uri: String,
    /// Warehouse location
    pub warehouse: Option<String>,
    /// Authentication credentials
    pub credentials: Option<CatalogCredentials>,
    /// Additional properties
    pub properties: HashMap<String, String>,
}

/// Catalog credentials
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogCredentials {
    /// Authentication type
    pub auth_type: AuthType,
    /// Credentials (tokens, keys, etc.)
    #[serde(skip_serializing)] // Don't serialize sensitive data
    pub credentials: HashMap<String, String>,
}

/// Authentication types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuthType {
    /// No authentication
    None,
    /// Bearer token
    Bearer,
    /// OAuth2
    OAuth2,
    /// AWS SigV4
    AwsSigV4,
    /// Basic auth (username/password)
    Basic,
    /// API key
    ApiKey,
}

// ============================================================================
// External Catalog Trait
// ============================================================================

/// Trait for external catalog implementations
#[async_trait]
pub trait ExternalCatalog: Send + Sync {
    /// Get catalog name
    fn catalog_name(&self) -> &str;

    /// Get catalog type
    fn catalog_type(&self) -> ExternalCatalogType;

    /// Get constraint support level
    fn constraint_support(&self) -> ConstraintSupport {
        self.catalog_type().default_constraint_support()
    }

    // ========================================================================
    // Namespace Operations
    // ========================================================================

    /// List all namespaces
    async fn list_namespaces(&self) -> Result<Vec<String>>;

    /// List namespaces under a parent namespace
    async fn list_namespaces_under(&self, parent: &str) -> Result<Vec<String>> {
        // Default: filter list_namespaces() by prefix
        let all = self.list_namespaces().await?;
        let prefix = format!("{}.", parent);
        Ok(all
            .into_iter()
            .filter(|ns| ns.starts_with(&prefix))
            .collect())
    }

    /// Create a namespace
    async fn create_namespace(
        &self,
        namespace: &str,
        properties: HashMap<String, String>,
    ) -> Result<()>;

    /// Drop a namespace
    async fn drop_namespace(&self, namespace: &str) -> Result<()>;

    /// Check if namespace exists
    async fn namespace_exists(&self, namespace: &str) -> Result<bool> {
        let namespaces = self.list_namespaces().await?;
        Ok(namespaces.contains(&namespace.to_string()))
    }

    // ========================================================================
    // Table Operations
    // ========================================================================

    /// List tables in a namespace
    async fn list_tables(&self, namespace: &str) -> Result<Vec<String>>;

    /// Get table schema
    async fn get_table_schema(&self, namespace: &str, table: &str) -> Result<ArrowSchema>;

    /// Get table properties
    async fn get_table_properties(
        &self,
        namespace: &str,
        table: &str,
    ) -> Result<HashMap<String, String>>;

    /// Get table location (storage path)
    async fn get_table_location(&self, namespace: &str, table: &str) -> Result<String>;

    /// Check if table exists
    async fn table_exists(&self, namespace: &str, table: &str) -> Result<bool> {
        let tables = self.list_tables(namespace).await?;
        Ok(tables.contains(&table.to_string()))
    }

    /// Create a table
    async fn create_table(
        &self,
        namespace: &str,
        table: &str,
        schema: &ArrowSchema,
        location: Option<&str>,
        properties: HashMap<String, String>,
    ) -> Result<()>;

    /// Drop a table
    async fn drop_table(&self, namespace: &str, table: &str, purge: bool) -> Result<()>;

    /// Rename a table
    async fn rename_table(
        &self,
        namespace: &str,
        old_name: &str,
        new_namespace: Option<&str>,
        new_name: &str,
    ) -> Result<()>;

    // ========================================================================
    // Metadata Operations
    // ========================================================================

    /// Get table statistics (if available)
    async fn get_table_statistics(
        &self,
        _namespace: &str,
        _table: &str,
    ) -> Result<Option<TableStatistics>> {
        Ok(None)
    }

    /// Get partition information (if partitioned)
    async fn get_partition_spec(
        &self,
        _namespace: &str,
        _table: &str,
    ) -> Result<Option<Vec<String>>> {
        Ok(None)
    }

    /// Refresh table metadata from source
    async fn refresh_table(&self, namespace: &str, table: &str) -> Result<()>;
}

/// Table statistics from external catalog
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableStatistics {
    /// Row count
    pub row_count: Option<u64>,
    /// Total size in bytes
    pub size_bytes: Option<u64>,
    /// File count
    pub file_count: Option<u64>,
    /// Column statistics
    pub column_stats: HashMap<String, ColumnStatistics>,
}

/// Column statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnStatistics {
    /// Null count
    pub null_count: Option<u64>,
    /// Distinct count
    pub distinct_count: Option<u64>,
    /// Min value (JSON encoded)
    pub min: Option<serde_json::Value>,
    /// Max value (JSON encoded)
    pub max: Option<serde_json::Value>,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::federation::federated_catalog::ConstraintLevel;

    #[test]
    fn test_external_catalog_type_constraint_support() {
        let iceberg = ExternalCatalogType::Iceberg;
        let support = iceberg.default_constraint_support();
        assert_eq!(support.not_null, ConstraintLevel::Full);

        let delta = ExternalCatalogType::DeltaLake;
        let support = delta.default_constraint_support();
        assert_eq!(support.foreign_key, ConstraintLevel::None);
    }

    #[test]
    fn test_catalog_config_serialization() {
        let config = ExternalCatalogConfig {
            name: "my_iceberg".to_string(),
            catalog_type: ExternalCatalogType::Iceberg,
            uri: "http://localhost:8181".to_string(),
            warehouse: Some("s3://my-bucket/warehouse".to_string()),
            credentials: None,
            properties: HashMap::new(),
        };

        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("Iceberg"));
        assert!(json.contains("my_iceberg"));
    }
}
