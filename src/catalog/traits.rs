//! Catalog Trait Definitions
//!
//! Defines the core trait that all catalog backends must implement.
//! Uses internal catalog types for Serde compatibility.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use super::cache::CatalogCache;
use super::types::{
    CatalogIndex, CatalogNamespace, CatalogPartitionSpec, CatalogSchemaEvolution,
    CatalogSortOrder, CatalogTableSchema, CatalogTableStatistics,
};
use super::TableIdentifier;

/// Core catalog trait - all catalog backends implement this
#[async_trait]
pub trait Catalog: Send + Sync {
    /// Get the catalog name
    fn name(&self) -> &str;

    /// Get the catalog type identifier
    fn catalog_type(&self) -> &str;

    // ========================
    // Namespace Operations
    // ========================

    /// Create a new namespace
    async fn create_namespace(
        &self,
        namespace: &[String],
        properties: HashMap<String, String>,
    ) -> Result<CatalogNamespace>;

    /// Drop a namespace (must be empty unless cascade=true)
    async fn drop_namespace(&self, namespace: &[String], cascade: bool) -> Result<bool>;

    /// List namespaces under a parent namespace
    async fn list_namespaces(&self, parent: Option<&[String]>) -> Result<Vec<CatalogNamespace>>;

    /// Check if a namespace exists
    async fn namespace_exists(&self, namespace: &[String]) -> Result<bool>;

    /// Get namespace metadata
    async fn get_namespace(&self, namespace: &[String]) -> Result<CatalogNamespace>;

    /// Update namespace properties
    async fn update_namespace_properties(
        &self,
        namespace: &[String],
        updates: HashMap<String, String>,
        removals: Vec<String>,
    ) -> Result<()>;

    // ========================
    // Table Operations
    // ========================

    /// Create a new table
    async fn create_table(
        &self,
        identifier: &TableIdentifier,
        schema: CatalogTableSchema,
    ) -> Result<CatalogTableSchema>;

    /// Drop a table
    async fn drop_table(&self, identifier: &TableIdentifier, purge: bool) -> Result<bool>;

    /// List tables in a namespace
    async fn list_tables(&self, namespace: &[String]) -> Result<Vec<TableIdentifier>>;

    /// Check if a table exists
    async fn table_exists(&self, identifier: &TableIdentifier) -> Result<bool>;

    /// Get table schema
    async fn get_table(&self, identifier: &TableIdentifier) -> Result<CatalogTableSchema>;

    /// Rename a table
    async fn rename_table(&self, from: &TableIdentifier, to: &TableIdentifier) -> Result<()>;

    // ========================
    // Schema Evolution
    // ========================

    /// Evolve table schema (add/drop/rename columns, change types)
    async fn evolve_schema(
        &self,
        identifier: &TableIdentifier,
        evolution: CatalogSchemaEvolution,
    ) -> Result<CatalogTableSchema>;

    /// Get table's current schema version
    async fn get_schema_version(&self, identifier: &TableIdentifier) -> Result<i32>;

    /// Get historical schema by version
    async fn get_schema_by_version(
        &self,
        identifier: &TableIdentifier,
        version: i32,
    ) -> Result<CatalogTableSchema>;

    // ========================
    // Index Operations
    // ========================

    /// Create an index on a table
    async fn create_index(
        &self,
        identifier: &TableIdentifier,
        index: CatalogIndex,
    ) -> Result<CatalogIndex>;

    /// Drop an index
    async fn drop_index(&self, identifier: &TableIdentifier, index_name: &str) -> Result<bool>;

    /// List indexes on a table
    async fn list_indexes(&self, identifier: &TableIdentifier) -> Result<Vec<CatalogIndex>>;

    // ========================
    // Statistics
    // ========================

    /// Get table statistics
    async fn get_statistics(&self, identifier: &TableIdentifier) -> Result<CatalogTableStatistics>;

    /// Update table statistics
    async fn update_statistics(
        &self,
        identifier: &TableIdentifier,
        stats: CatalogTableStatistics,
    ) -> Result<()>;

    // ========================
    // Partitioning (for Iceberg-compatible catalogs)
    // ========================

    /// Get partition spec for a table
    async fn get_partition_spec(
        &self,
        identifier: &TableIdentifier,
    ) -> Result<Option<CatalogPartitionSpec>> {
        // Default: no partitioning
        let _ = identifier;
        Ok(None)
    }

    /// Update partition spec
    async fn update_partition_spec(
        &self,
        identifier: &TableIdentifier,
        spec: CatalogPartitionSpec,
    ) -> Result<()> {
        // Default: not supported
        let _ = (identifier, spec);
        Err(anyhow::anyhow!(
            "Partition spec updates not supported by this catalog"
        ))
    }

    // ========================
    // Sort Order (for Iceberg-compatible catalogs)
    // ========================

    /// Get sort order for a table
    async fn get_sort_order(
        &self,
        identifier: &TableIdentifier,
    ) -> Result<Option<CatalogSortOrder>> {
        // Default: no sort order
        let _ = identifier;
        Ok(None)
    }

    /// Update sort order
    async fn update_sort_order(
        &self,
        identifier: &TableIdentifier,
        order: CatalogSortOrder,
    ) -> Result<()> {
        // Default: not supported
        let _ = (identifier, order);
        Err(anyhow::anyhow!(
            "Sort order updates not supported by this catalog"
        ))
    }

    // ========================
    // Transaction Support
    // ========================

    /// Begin a catalog transaction (for atomic multi-table operations)
    async fn begin_transaction(&self) -> Result<CatalogTransaction> {
        // Default: no transaction support
        Ok(CatalogTransaction::NoOp)
    }

    /// Commit a catalog transaction
    async fn commit_transaction(&self, _txn: CatalogTransaction) -> Result<()> {
        Ok(())
    }

    /// Rollback a catalog transaction
    async fn rollback_transaction(&self, _txn: CatalogTransaction) -> Result<()> {
        Ok(())
    }

    // ========================
    // Cache Integration
    // ========================

    /// Get the cache instance (if available)
    fn cache(&self) -> Option<Arc<CatalogCache>> {
        None
    }

    /// Invalidate cached entries for a table
    async fn invalidate_cache(&self, identifier: &TableIdentifier) -> Result<()> {
        if let Some(cache) = self.cache() {
            cache.invalidate_table(identifier).await;
        }
        Ok(())
    }

    // ========================
    // Health & Connectivity
    // ========================

    /// Check if the catalog backend is healthy/reachable
    async fn health_check(&self) -> Result<CatalogHealth>;

    /// Close the catalog connection
    async fn close(&self) -> Result<()>;
}

/// Catalog transaction handle
#[derive(Debug)]
pub enum CatalogTransaction {
    /// No-op transaction (for catalogs without transaction support)
    NoOp,
    /// Transaction ID for catalogs with transaction support
    Active {
        txn_id: String,
        started_at: std::time::Instant,
    },
}

/// Catalog health status
#[derive(Debug, Clone)]
pub struct CatalogHealth {
    /// Is the catalog reachable?
    pub is_healthy: bool,
    /// Latency to the catalog backend
    pub latency_ms: u64,
    /// Error message if unhealthy
    pub error: Option<String>,
    /// Additional health details
    pub details: HashMap<String, String>,
}

impl CatalogHealth {
    pub fn healthy(latency_ms: u64) -> Self {
        Self {
            is_healthy: true,
            latency_ms,
            error: None,
            details: HashMap::new(),
        }
    }

    pub fn unhealthy(error: impl Into<String>) -> Self {
        Self {
            is_healthy: false,
            latency_ms: 0,
            error: Some(error.into()),
            details: HashMap::new(),
        }
    }

    pub fn with_detail(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.details.insert(key.into(), value.into());
        self
    }
}

/// Extension trait for vector-specific catalog operations
#[async_trait]
pub trait VectorCatalogExtension: Catalog {
    /// Create a vector index (HNSW, IVF, PQ, etc.)
    async fn create_vector_index(
        &self,
        identifier: &TableIdentifier,
        column: &str,
        index_type: VectorIndexType,
        params: VectorIndexParams,
    ) -> Result<CatalogIndex>;

    /// Get vector index statistics
    async fn get_vector_index_stats(
        &self,
        identifier: &TableIdentifier,
        index_name: &str,
    ) -> Result<VectorIndexStats>;
}

/// Vector index types
#[derive(Debug, Clone)]
pub enum VectorIndexType {
    /// Hierarchical Navigable Small World
    Hnsw,
    /// Inverted File Index
    Ivf,
    /// Product Quantization
    Pq,
    /// IVF with Product Quantization
    IvfPq,
    /// Flat (brute-force)
    Flat,
    /// Disk-based ANN
    DiskAnn,
}

/// Vector index parameters
#[derive(Debug, Clone, Default)]
pub struct VectorIndexParams {
    /// Dimension of vectors
    pub dimension: Option<u32>,
    /// Distance metric (l2, cosine, dot_product)
    pub metric: Option<String>,
    /// HNSW: M parameter
    pub hnsw_m: Option<u32>,
    /// HNSW: ef_construction parameter
    pub hnsw_ef_construction: Option<u32>,
    /// IVF: number of clusters
    pub ivf_nlist: Option<u32>,
    /// PQ: number of subquantizers
    pub pq_m: Option<u32>,
    /// PQ: bits per subquantizer
    pub pq_nbits: Option<u32>,
    /// Custom parameters
    pub custom: HashMap<String, String>,
}

/// Vector index statistics
#[derive(Debug, Clone, Default)]
pub struct VectorIndexStats {
    /// Number of indexed vectors
    pub vector_count: u64,
    /// Index size in bytes
    pub size_bytes: u64,
    /// Build progress (0.0 - 1.0)
    pub build_progress: f32,
    /// Is the index ready for queries?
    pub is_ready: bool,
    /// Last updated timestamp
    pub last_updated: Option<i64>,
}

/// Extension trait for lakehouse table format operations
#[async_trait]
pub trait LakehouseExtension: Catalog {
    /// Get the table format
    fn table_format(&self) -> TableFormat;

    /// Get table storage location
    async fn get_table_location(&self, identifier: &TableIdentifier) -> Result<String>;

    /// Get current snapshot ID (for Iceberg tables)
    async fn get_current_snapshot(&self, identifier: &TableIdentifier) -> Result<Option<i64>>;

    /// List all snapshots
    async fn list_snapshots(&self, identifier: &TableIdentifier) -> Result<Vec<i64>>;

    /// Get schema history (list of schema version IDs)
    async fn get_schema_history(&self, identifier: &TableIdentifier) -> Result<Vec<i32>>;
}

/// Table format for lakehouse tables
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableFormat {
    ProximaDB,
    Iceberg,
    Delta,
    Hudi,
    Parquet,
}

impl std::fmt::Display for TableFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TableFormat::ProximaDB => write!(f, "proximadb"),
            TableFormat::Iceberg => write!(f, "iceberg"),
            TableFormat::Delta => write!(f, "delta"),
            TableFormat::Hudi => write!(f, "hudi"),
            TableFormat::Parquet => write!(f, "parquet"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_catalog_health_healthy() {
        let health = CatalogHealth::healthy(5);
        assert!(health.is_healthy);
        assert_eq!(health.latency_ms, 5);
        assert!(health.error.is_none());
    }

    #[test]
    fn test_catalog_health_unhealthy() {
        let health = CatalogHealth::unhealthy("Connection refused");
        assert!(!health.is_healthy);
        assert_eq!(health.error, Some("Connection refused".to_string()));
    }

    #[test]
    fn test_catalog_health_with_details() {
        let health = CatalogHealth::healthy(10)
            .with_detail("version", "3.1.0")
            .with_detail("backend", "glue");

        assert_eq!(health.details.get("version"), Some(&"3.1.0".to_string()));
        assert_eq!(health.details.get("backend"), Some(&"glue".to_string()));
    }

    #[test]
    fn test_table_format_display() {
        assert_eq!(TableFormat::Iceberg.to_string(), "iceberg");
        assert_eq!(TableFormat::Delta.to_string(), "delta");
        assert_eq!(TableFormat::ProximaDB.to_string(), "proximadb");
    }

    #[test]
    fn test_vector_index_params_default() {
        let params = VectorIndexParams::default();
        assert!(params.dimension.is_none());
        assert!(params.metric.is_none());
        assert!(params.custom.is_empty());
    }
}
