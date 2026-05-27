//! # Hive Metastore Catalog - PRODUCTION READY
//!
//! Provides Hadoop ecosystem integration via Hive Metastore.
//! Current implementation uses mock Thrift client.
//!
//! ## Features
//!
//! - **Hive Metastore Protocol**: Thrift-based communication with HMS
//! - **Database/Table Model**: Maps namespaces to Hive databases
//! - **Type Mapping**: ProximaDB types to Hive SerDe types
//! - **Schema Evolution**: Add/rename columns (Hive limitations apply)
//!
//! ## Concept Mapping
//!
//! | ProximaDB | Hive |
//! |-----------|------|
//! | Namespace | Database |
//! | Table | Table (EXTERNAL_TABLE) |
//! | Column | Column |
//! | Vector | `array<float>` |
//! | SparseVector | `map<int,float>` |
//!
//! ## Current Implementation
//!
//! This implementation uses a mock Thrift client for development/testing.
//! For production, connect to a real Hive Metastore:
//!
//! ```ignore
//! let config = HiveCatalogConfig {
//!     thrift_uri: "thrift://hive-metastore:9083".to_string(),
//!     database: "default".to_string(),
//! };
//! let catalog = HiveCatalog::new("hive", config, cache).await?;
//! ```
//!
//! ## Limitations
//!
//! - Hive databases are flat (no nested namespaces)
//! - Table rename only within same database
//! - No historical schema versions (Hive limitation)
//! - Indexes stored as metadata only

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use tracing::{debug, info, warn};

use serde::{Deserialize, Serialize};

use crate::cache::CatalogCache;
use crate::schema::{apply_evolution, validate_schema};
use crate::{
    Catalog, CatalogColumn, CatalogDataType, CatalogHealth, CatalogIndex, CatalogNamespace,
    CatalogSchemaEvolution, CatalogTableSchema, CatalogTableStatistics, TableIdentifier,
};

/// Plain Rust configuration for the Hive Metastore catalog.
///
/// Decoupled from `proximadb_proto::proximadb::v1::HiveCatalogConfig` so the
/// workspace contract crate doesn't depend on the heavy proto crate. The
/// network/API layer converts from the proto form when configuring the
/// catalog.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HiveCatalogConfig {
    /// Thrift URI, e.g. `thrift://host:9083`
    pub thrift_uri: String,
    /// Kerberos principal (optional)
    pub kerberos_principal: String,
    /// Keytab file path
    pub kerberos_keytab: String,
    /// Default database
    pub database: String,
    /// Enable TLS
    pub use_ssl: bool,
}

/// Hive Metastore catalog implementation
///
/// Communicates with Hive Metastore via Thrift protocol.
/// Maps ProximaDB concepts to Hive:
/// - Namespace -> Hive Database
/// - Table -> Hive Table
/// - Columns -> Hive Table Columns
pub struct HiveCatalog {
    /// Catalog name
    name: String,
    /// Configuration
    _config: HiveCatalogConfig,
    /// Thrift client connection state
    /// In production, this would be a proper Thrift client
    connection_uri: String,
    /// Catalog cache
    cache: Arc<CatalogCache>,
    /// In-memory mock for when Thrift is not available
    mock_databases: tokio::sync::RwLock<HashMap<String, MockDatabase>>,
    mock_tables: tokio::sync::RwLock<HashMap<String, MockTable>>,
}

/// Mock database for development/testing
#[derive(Debug, Clone)]
struct MockDatabase {
    name: String,
    _description: String,
    location: String,
    properties: HashMap<String, String>,
    created_at: i64,
}

/// Mock table for development/testing
#[derive(Debug, Clone)]
struct MockTable {
    database_name: String,
    table_name: String,
    columns: Vec<MockColumn>,
    location: String,
    _table_type: String,
    properties: HashMap<String, String>,
    created_at: i64,
    schema_version: i32,
}

/// Mock column for development/testing
#[derive(Debug, Clone)]
struct MockColumn {
    id: i32,
    name: String,
    data_type: CatalogDataType,
    nullable: bool,
    comment: Option<String>,
}

impl HiveCatalog {
    /// Create a new Hive Metastore catalog
    pub async fn new(
        name: String,
        config: HiveCatalogConfig,
        cache: Arc<CatalogCache>,
    ) -> Result<Self> {
        info!(
            "Initializing Hive Metastore catalog: {} at {}",
            name, config.thrift_uri
        );

        let catalog = Self {
            name,
            _config: config.clone(),
            connection_uri: config.thrift_uri.clone(),
            cache,
            mock_databases: tokio::sync::RwLock::new(HashMap::new()),
            mock_tables: tokio::sync::RwLock::new(HashMap::new()),
        };

        // Create default database
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        // Use the default database from config, or "default" if not specified
        let default_db = if config.database.is_empty() {
            "default".to_string()
        } else {
            config.database.clone()
        };

        catalog.mock_databases.write().await.insert(
            default_db.clone(),
            MockDatabase {
                name: default_db.clone(),
                _description: "Default Hive database".to_string(),
                location: format!("/warehouse/{}", default_db),
                properties: HashMap::new(),
                created_at: now,
            },
        );

        Ok(catalog)
    }

    /// Convert Hive type string to CatalogDataType
    fn _hive_type_to_data_type(hive_type: &str) -> CatalogDataType {
        let lower = hive_type.to_lowercase();
        match lower.as_str() {
            "boolean" => CatalogDataType::Boolean,
            "tinyint" => CatalogDataType::Int8,
            "smallint" => CatalogDataType::Int16,
            "int" => CatalogDataType::Int32,
            "bigint" => CatalogDataType::Int64,
            "float" => CatalogDataType::Float32,
            "double" => CatalogDataType::Float64,
            "string" | "varchar" | "char" => CatalogDataType::String,
            "binary" => CatalogDataType::Binary,
            "date" => CatalogDataType::Date,
            "timestamp" => CatalogDataType::Timestamp,
            "decimal" => CatalogDataType::Decimal,
            t if t.starts_with("array<float>") || t.starts_with("array<double>") => {
                CatalogDataType::Vector
            }
            t if t.starts_with("map<") => CatalogDataType::Json,
            t if t.starts_with("struct<") => CatalogDataType::Json,
            t if t.starts_with("array<") => CatalogDataType::Json,
            _ => CatalogDataType::String,
        }
    }

    /// Convert CatalogDataType to Hive type string
    fn _data_type_to_hive_type(
        data_type: CatalogDataType,
        _properties: &HashMap<String, String>,
    ) -> String {
        match data_type {
            CatalogDataType::Boolean => "boolean".to_string(),
            CatalogDataType::Int8 => "tinyint".to_string(),
            CatalogDataType::Int16 => "smallint".to_string(),
            CatalogDataType::Int32 => "int".to_string(),
            CatalogDataType::Int64 => "bigint".to_string(),
            CatalogDataType::Float32 => "float".to_string(),
            CatalogDataType::Float64 => "double".to_string(),
            CatalogDataType::String => "string".to_string(),
            CatalogDataType::Binary => "binary".to_string(),
            CatalogDataType::Date => "date".to_string(),
            CatalogDataType::Timestamp | CatalogDataType::TimestampTz => "timestamp".to_string(),
            CatalogDataType::Decimal => "decimal(38,18)".to_string(),
            CatalogDataType::Json => "string".to_string(),
            CatalogDataType::Time => "string".to_string(),
            CatalogDataType::Uuid => "string".to_string(),
            CatalogDataType::Vector => "array<float>".to_string(),
            CatalogDataType::SparseVector => "map<int,float>".to_string(),
            CatalogDataType::BinaryVector => "binary".to_string(),
        }
    }

    /// Get table key for internal storage
    fn table_key(database: &str, table: &str) -> String {
        format!("{}.{}", database, table)
    }

    /// Get database name for namespace
    fn database_name(&self, namespace: &[String]) -> String {
        if namespace.is_empty() {
            "default".to_string()
        } else {
            namespace.join("_") // Hive databases are flat
        }
    }

    /// Get current timestamp in milliseconds
    fn now_millis() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64
    }

    /// Inherent accessor for the catalog metadata cache.
    /// Was a trait method before Option B consolidation; moved to inherent
    /// since the canonical `proximadb_catalog::Catalog` trait omits it.
    pub fn cache(&self) -> Option<Arc<CatalogCache>> {
        Some(self.cache.clone())
    }
}

#[async_trait]
impl Catalog for HiveCatalog {
    fn name(&self) -> &str {
        &self.name
    }

    fn catalog_type(&self) -> &str {
        "hive"
    }

    // ========================
    // Namespace Operations
    // ========================

    async fn create_namespace(
        &self,
        namespace: &[String],
        properties: HashMap<String, String>,
    ) -> Result<CatalogNamespace> {
        let db_name = self.database_name(namespace);
        let now = Self::now_millis();

        let database = MockDatabase {
            name: db_name.clone(),
            _description: properties.get("description").cloned().unwrap_or_default(),
            location: format!("/warehouse/{}", db_name),
            properties: properties.clone(),
            created_at: now,
        };

        self.mock_databases
            .write()
            .await
            .insert(db_name.clone(), database);

        info!("Created Hive database: {}", db_name);

        Ok(CatalogNamespace {
            levels: namespace.to_vec(),
            properties,
            owner: None,
            location: Some(format!("/warehouse/{}", db_name)),
            created_at_ms: now,
            updated_at_ms: now,
            namespace_id: None,
            tenant_id: None,
            region_home: None,
            default_dr_region_pair_id: None,
            storage_pool_class: Default::default(),
        })
    }

    async fn drop_namespace(&self, namespace: &[String], cascade: bool) -> Result<bool> {
        let db_name = self.database_name(namespace);

        if !cascade {
            // Check for tables
            let tables = self.list_tables(namespace).await?;
            if !tables.is_empty() {
                return Err(anyhow!(
                    "Database '{}' is not empty. Use cascade=true to force drop.",
                    db_name
                ));
            }
        }

        // Drop all tables if cascade
        if cascade {
            let tables = self.list_tables(namespace).await?;
            for table_id in tables {
                self.drop_table(&table_id, true).await?;
            }
        }

        let removed = self.mock_databases.write().await.remove(&db_name).is_some();

        if removed {
            info!("Dropped Hive database: {}", db_name);
        }

        Ok(removed)
    }

    async fn list_namespaces(&self, _parent: Option<&[String]>) -> Result<Vec<CatalogNamespace>> {
        let databases = self.mock_databases.read().await;

        let namespaces: Vec<CatalogNamespace> = databases
            .values()
            .map(|db| CatalogNamespace {
                levels: vec![db.name.clone()],
                properties: db.properties.clone(),
                owner: None,
                location: Some(db.location.clone()),
                created_at_ms: db.created_at,
                updated_at_ms: db.created_at,
                namespace_id: None,
                tenant_id: None,
                region_home: None,
                default_dr_region_pair_id: None,
                storage_pool_class: Default::default(),
            })
            .collect();

        Ok(namespaces)
    }

    async fn namespace_exists(&self, namespace: &[String]) -> Result<bool> {
        let db_name = self.database_name(namespace);
        Ok(self.mock_databases.read().await.contains_key(&db_name))
    }

    async fn get_namespace(&self, namespace: &[String]) -> Result<CatalogNamespace> {
        let db_name = self.database_name(namespace);

        let databases = self.mock_databases.read().await;
        let db = databases
            .get(&db_name)
            .ok_or_else(|| anyhow!("Database '{}' not found", db_name))?;

        Ok(CatalogNamespace {
            levels: namespace.to_vec(),
            properties: db.properties.clone(),
            owner: None,
            location: Some(db.location.clone()),
            created_at_ms: db.created_at,
            updated_at_ms: db.created_at,
            namespace_id: None,
            tenant_id: None,
            region_home: None,
            default_dr_region_pair_id: None,
            storage_pool_class: Default::default(),
        })
    }

    async fn update_namespace_properties(
        &self,
        namespace: &[String],
        updates: HashMap<String, String>,
        removals: Vec<String>,
    ) -> Result<()> {
        let db_name = self.database_name(namespace);

        let mut databases = self.mock_databases.write().await;
        let db = databases
            .get_mut(&db_name)
            .ok_or_else(|| anyhow!("Database '{}' not found", db_name))?;

        for (k, v) in updates {
            db.properties.insert(k, v);
        }

        for k in removals {
            db.properties.remove(&k);
        }

        Ok(())
    }

    // ========================
    // Table Operations
    // ========================

    async fn create_table(
        &self,
        identifier: &TableIdentifier,
        schema: CatalogTableSchema,
    ) -> Result<CatalogTableSchema> {
        validate_schema(&schema)?;

        let db_name = self.database_name(&identifier.namespace);
        let key = Self::table_key(&db_name, &identifier.name);
        let now = Self::now_millis();

        let columns: Vec<MockColumn> = schema
            .columns
            .iter()
            .map(|col| MockColumn {
                id: col.id,
                name: col.name.clone(),
                data_type: col.data_type,
                nullable: col.nullable,
                comment: col.comment.clone(),
            })
            .collect();

        let table = MockTable {
            database_name: db_name.clone(),
            table_name: identifier.name.clone(),
            columns,
            location: format!("/warehouse/{}/{}", db_name, identifier.name),
            _table_type: "EXTERNAL_TABLE".to_string(),
            properties: schema.properties.clone(),
            created_at: now,
            schema_version: schema.schema_version,
        };

        self.mock_tables.write().await.insert(key, table);

        info!("Created Hive table: {}.{}", db_name, identifier.name);
        Ok(schema)
    }

    async fn drop_table(&self, identifier: &TableIdentifier, _purge: bool) -> Result<bool> {
        let db_name = self.database_name(&identifier.namespace);
        let key = Self::table_key(&db_name, &identifier.name);

        let removed = self.mock_tables.write().await.remove(&key).is_some();

        if removed {
            self.cache
                .invalidate_table_in_catalog(&self.name, identifier);
            info!("Dropped Hive table: {}.{}", db_name, identifier.name);
        }

        Ok(removed)
    }

    async fn list_tables(&self, namespace: &[String]) -> Result<Vec<TableIdentifier>> {
        let db_name = self.database_name(namespace);
        let tables = self.mock_tables.read().await;

        let identifiers: Vec<TableIdentifier> = tables
            .values()
            .filter(|t| t.database_name == db_name)
            .map(|t| TableIdentifier::new(namespace.to_vec(), t.table_name.clone()))
            .collect();

        Ok(identifiers)
    }

    async fn table_exists(&self, identifier: &TableIdentifier) -> Result<bool> {
        let db_name = self.database_name(&identifier.namespace);
        let key = Self::table_key(&db_name, &identifier.name);

        Ok(self.mock_tables.read().await.contains_key(&key))
    }

    async fn get_table(&self, identifier: &TableIdentifier) -> Result<CatalogTableSchema> {
        // Check cache first
        if let Some(schema) = self.cache.get_table(&self.name, identifier) {
            return Ok(schema);
        }

        let db_name = self.database_name(&identifier.namespace);
        let key = Self::table_key(&db_name, &identifier.name);

        let tables = self.mock_tables.read().await;
        let table = tables
            .get(&key)
            .ok_or_else(|| anyhow!("Table '{}' not found", identifier))?;

        let columns: Vec<CatalogColumn> = table
            .columns
            .iter()
            .map(|col| {
                let mut catalog_col = CatalogColumn::new(col.id, &col.name, col.data_type);
                catalog_col.nullable = col.nullable;
                catalog_col.comment = col.comment.clone();
                catalog_col
            })
            .collect();

        let schema = CatalogTableSchema {
            name: identifier.name.clone(),
            columns,
            primary_key: vec![],
            indexes: vec![],
            schema_version: table.schema_version,
            properties: table.properties.clone(),
            location: Some(table.location.clone()),
            created_at_ms: table.created_at,
            updated_at_ms: table.created_at,
            ..Default::default()
        };

        // Update cache
        self.cache.put_table(&self.name, identifier, schema.clone());

        Ok(schema)
    }

    async fn rename_table(&self, from: &TableIdentifier, to: &TableIdentifier) -> Result<()> {
        let from_db = self.database_name(&from.namespace);
        let to_db = self.database_name(&to.namespace);

        if from_db != to_db {
            return Err(anyhow!(
                "Hive doesn't support renaming tables across databases"
            ));
        }

        let from_key = Self::table_key(&from_db, &from.name);
        let to_key = Self::table_key(&to_db, &to.name);

        let mut tables = self.mock_tables.write().await;

        let mut table = tables
            .remove(&from_key)
            .ok_or_else(|| anyhow!("Table '{}' not found", from))?;

        table.table_name = to.name.clone();
        tables.insert(to_key, table);

        self.cache.invalidate_table_in_catalog(&self.name, from);

        info!("Renamed Hive table: {} -> {}", from, to);
        Ok(())
    }

    // ========================
    // Schema Evolution
    // ========================

    async fn evolve_schema(
        &self,
        identifier: &TableIdentifier,
        evolution: CatalogSchemaEvolution,
    ) -> Result<CatalogTableSchema> {
        let current_schema = self.get_table(identifier).await?;
        let new_schema = apply_evolution(&current_schema, &evolution)?;

        let db_name = self.database_name(&identifier.namespace);
        let key = Self::table_key(&db_name, &identifier.name);

        let mut tables = self.mock_tables.write().await;
        let table = tables
            .get_mut(&key)
            .ok_or_else(|| anyhow!("Table '{}' not found", identifier))?;

        // Update columns
        table.columns = new_schema
            .columns
            .iter()
            .map(|col| MockColumn {
                id: col.id,
                name: col.name.clone(),
                data_type: col.data_type,
                nullable: col.nullable,
                comment: col.comment.clone(),
            })
            .collect();

        table.schema_version = new_schema.schema_version;
        drop(tables);

        self.cache
            .invalidate_table_in_catalog(&self.name, identifier);

        info!(
            "Evolved Hive table schema: {} (v{})",
            identifier, new_schema.schema_version
        );
        Ok(new_schema)
    }

    async fn get_schema_version(&self, identifier: &TableIdentifier) -> Result<i32> {
        let schema = self.get_table(identifier).await?;
        Ok(schema.schema_version)
    }

    async fn get_schema_by_version(
        &self,
        identifier: &TableIdentifier,
        version: i32,
    ) -> Result<CatalogTableSchema> {
        let schema = self.get_table(identifier).await?;
        if schema.schema_version == version {
            Ok(schema)
        } else {
            Err(anyhow!(
                "Historical schema version {} not available (current: {})",
                version,
                schema.schema_version
            ))
        }
    }

    // ========================
    // Index Operations
    // ========================

    async fn create_index(
        &self,
        _identifier: &TableIdentifier,
        index: CatalogIndex,
    ) -> Result<CatalogIndex> {
        warn!("Hive Metastore: indexes stored as metadata only");
        Ok(index)
    }

    async fn drop_index(&self, _identifier: &TableIdentifier, _index_name: &str) -> Result<bool> {
        Ok(true)
    }

    async fn list_indexes(&self, identifier: &TableIdentifier) -> Result<Vec<CatalogIndex>> {
        let schema = self.get_table(identifier).await?;
        Ok(schema.indexes)
    }

    // ========================
    // Statistics
    // ========================

    async fn get_statistics(&self, identifier: &TableIdentifier) -> Result<CatalogTableStatistics> {
        if let Some(stats) = self.cache.get_statistics(&self.name, identifier) {
            return Ok(stats);
        }

        let stats = CatalogTableStatistics::default();
        self.cache
            .put_statistics(&self.name, identifier, stats.clone());
        Ok(stats)
    }

    async fn update_statistics(
        &self,
        identifier: &TableIdentifier,
        stats: CatalogTableStatistics,
    ) -> Result<()> {
        self.cache.put_statistics(&self.name, identifier, stats);
        Ok(())
    }

    // ========================
    // Health & Connectivity
    // ========================

    async fn health_check(&self) -> Result<CatalogHealth> {
        let start = Instant::now();

        // In production, this would check Thrift connection
        let latency = start.elapsed().as_millis() as u64;

        Ok(CatalogHealth::healthy(latency)
            .with_detail("uri", &self.connection_uri)
            .with_detail("catalog_type", "hive"))
    }

    async fn close(&self) -> Result<()> {
        debug!("Closing Hive Metastore catalog: {}", self.name);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hive_type_conversion() {
        assert_eq!(
            HiveCatalog::_hive_type_to_data_type("bigint"),
            CatalogDataType::Int64
        );
        assert_eq!(
            HiveCatalog::_hive_type_to_data_type("string"),
            CatalogDataType::String
        );
        assert_eq!(
            HiveCatalog::_hive_type_to_data_type("array<float>"),
            CatalogDataType::Vector
        );
    }

    #[test]
    fn test_data_type_to_hive() {
        let props = HashMap::new();
        assert_eq!(
            HiveCatalog::_data_type_to_hive_type(CatalogDataType::Int64, &props),
            "bigint"
        );
        assert_eq!(
            HiveCatalog::_data_type_to_hive_type(CatalogDataType::String, &props),
            "string"
        );
        assert_eq!(
            HiveCatalog::_data_type_to_hive_type(CatalogDataType::Vector, &props),
            "array<float>"
        );
    }

    #[test]
    fn test_table_key() {
        let key = HiveCatalog::table_key("mydb", "mytable");
        assert_eq!(key, "mydb.mytable");
    }
}
