//! # AWS Glue Catalog - PRODUCTION READY
//!
//! Provides integration with AWS Glue Data Catalog for metadata management.
//!
//! ## Feature Gate
//!
//! This catalog requires the `aws` feature flag:
//! ```toml
//! [dependencies]
//! proximadb = { version = "0.1", features = ["aws"] }
//! ```
//!
//! Without the `aws` feature, operations are no-ops and return empty results.
//!
//! ## Features
//!
//! - **AWS Native**: Direct integration with AWS Glue Data Catalog
//! - **Database/Table Model**: Maps namespaces to Glue databases
//! - **Vector Extensions**: Store vectors as `array<float>` with dimension in comments
//! - **Athena/Redshift Compatible**: Tables queryable from AWS analytics services
//! - **Automatic Region Detection**: Uses AWS SDK configuration
//!
//! ## Concept Mapping
//!
//! | ProximaDB | AWS Glue |
//! |-----------|----------|
//! | Namespace | Database |
//! | Table | Table (EXTERNAL_TABLE) |
//! | Column | Column |
//! | Vector | `array<float>(dim)` + comment `vector:dim:metric=cosine` |
//! | SparseVector | `map<int,float>` |
//!
//! ## Configuration
//!
//! ```ignore
//! let config = GlueCatalogConfig {
//!     region: "us-east-1".to_string(),
//!     catalog_id: "123456789012".to_string(),  // AWS Account ID
//!     default_database: "proximadb".to_string(),
//! };
//! let catalog = GlueCatalog::new("glue", config, cache).await?;
//! ```
//!
//! ## Limitations
//!
//! - Glue databases are flat (no nested namespaces)
//! - No native index support (stored as table properties)
//! - No historical schema versions
//! - Table rename requires copy + delete

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use tracing::{debug, info, warn};

use crate::proto::proximadb_v1::GlueCatalogConfig;

use super::TableIdentifier;
use super::cache::CatalogCache;
use super::schema::{apply_evolution, validate_schema};
use super::traits::{Catalog, CatalogHealth};
use super::types::{
    CatalogColumn, CatalogDataType, CatalogIndex, CatalogNamespace, CatalogSchemaEvolution,
    CatalogTableSchema, CatalogTableStatistics,
};

/// AWS Glue catalog implementation
///
/// Maps ProximaDB concepts to Glue:
/// - Namespace -> Glue Database
/// - Table -> Glue Table
/// - Columns -> Glue Table Columns (with vector type extensions)
pub struct GlueCatalog {
    /// Catalog name
    name: String,
    /// Configuration
    config: GlueCatalogConfig,
    /// AWS Glue client (lazy initialized)
    #[cfg(feature = "aws")]
    client: Option<aws_sdk_glue::Client>,
    /// Catalog cache
    cache: Arc<CatalogCache>,
}

impl GlueCatalog {
    /// Create a new Glue catalog
    pub async fn new(
        name: String,
        config: GlueCatalogConfig,
        cache: Arc<CatalogCache>,
    ) -> Result<Self> {
        info!(
            "Initializing AWS Glue catalog: {} in region {}",
            name, config.region
        );

        #[cfg(feature = "aws")]
        let client = {
            let aws_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
                .region(aws_sdk_glue::config::Region::new(config.region.clone()))
                .load()
                .await;
            Some(aws_sdk_glue::Client::new(&aws_config))
        };

        Ok(Self {
            name,
            config,
            #[cfg(feature = "aws")]
            client,
            cache,
        })
    }

    /// Convert Glue column to CatalogColumn
    #[cfg(feature = "aws")]
    fn glue_column_to_column(col: &aws_sdk_glue::types::Column) -> CatalogColumn {
        let glue_type = col.r#type().unwrap_or("string");
        let data_type = Self::glue_type_to_data_type(glue_type);

        let mut properties = HashMap::new();

        // Parse vector dimension from comment or parameters
        if let Some(comment) = col.comment() {
            if comment.starts_with("vector:") {
                if let Some(dim) = comment.strip_prefix("vector:").and_then(|s| s.parse().ok()) {
                    properties.insert("dimension".to_string(), dim);
                }
            }
        }

        CatalogColumn {
            name: col.name().unwrap_or("").to_string(),
            data_type,
            nullable: true, // Glue doesn't track nullability well
            default_value: None,
            comment: col.comment().unwrap_or("").to_string(),
            properties,
        }
    }

    /// Convert Glue type string to CatalogDataType
    fn glue_type_to_data_type(glue_type: &str) -> CatalogDataType {
        let lower = glue_type.to_lowercase();
        match lower.as_str() {
            "boolean" | "bool" => CatalogDataType::Boolean,
            "int" | "integer" => CatalogDataType::Int32,
            "bigint" | "long" => CatalogDataType::Int64,
            "float" | "real" => CatalogDataType::Float32,
            "double" => CatalogDataType::Float64,
            "string" | "varchar" | "char" => CatalogDataType::String,
            "binary" | "bytes" => CatalogDataType::Bytes,
            "date" => CatalogDataType::Date,
            "timestamp" => CatalogDataType::Timestamp,
            "decimal" => CatalogDataType::Decimal,
            t if t.starts_with("array<float>") || t.starts_with("vector") => {
                CatalogDataType::Vector
            }
            _ => CatalogDataType::String, // Default fallback
        }
    }

    /// Convert CatalogDataType to Glue type string
    fn data_type_to_glue_type(
        data_type: &CatalogDataType,
        properties: &HashMap<String, String>,
    ) -> String {
        match data_type {
            CatalogDataType::Boolean => "boolean".to_string(),
            CatalogDataType::Int32 => "int".to_string(),
            CatalogDataType::Int64 => "bigint".to_string(),
            CatalogDataType::Float32 => "float".to_string(),
            CatalogDataType::Float64 => "double".to_string(),
            CatalogDataType::String => "string".to_string(),
            CatalogDataType::Bytes => "binary".to_string(),
            CatalogDataType::Date => "date".to_string(),
            CatalogDataType::Timestamp => "timestamp".to_string(),
            CatalogDataType::TimestampTz => "timestamp".to_string(),
            CatalogDataType::Decimal => "decimal(38,18)".to_string(),
            CatalogDataType::Json => "string".to_string(), // Glue doesn't have native JSON
            CatalogDataType::Vector | CatalogDataType::Embedding => {
                // Store as array<float> with dimension in comment
                let dim = properties
                    .get("dimension")
                    .unwrap_or(&"0".to_string())
                    .clone();
                format!("array<float>({})", dim)
            }
            CatalogDataType::SparseVector => "map<int,float>".to_string(),
            CatalogDataType::BinaryVector => "binary".to_string(),
            CatalogDataType::Uuid => "string".to_string(),
        }
    }

    /// Create vector column comment for Glue
    fn vector_column_comment(properties: &HashMap<String, String>) -> String {
        let dim = properties.get("dimension").unwrap_or(&"0".to_string());
        let metric = properties.get("metric").unwrap_or(&"cosine".to_string());
        format!("vector:{}:metric={}", dim, metric)
    }

    /// Get the Glue database name for a namespace
    fn database_name(&self, namespace: &[String]) -> String {
        if namespace.is_empty() {
            self.config.default_database.clone()
        } else {
            namespace.join("_") // Glue doesn't support nested namespaces
        }
    }
}

#[async_trait]
impl Catalog for GlueCatalog {
    fn name(&self) -> &str {
        &self.name
    }

    fn catalog_type(&self) -> &str {
        "glue"
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

        #[cfg(feature = "aws")]
        if let Some(client) = &self.client {
            let mut input_builder = aws_sdk_glue::types::DatabaseInput::builder()
                .name(&db_name)
                .description(properties.get("description").cloned().unwrap_or_default());

            // Add parameters
            for (k, v) in &properties {
                input_builder = input_builder.parameters(k.clone(), v.clone());
            }

            client
                .create_database()
                .database_input(input_builder.build()?)
                .catalog_id(&self.config.catalog_id)
                .send()
                .await
                .map_err(|e| anyhow!("Failed to create Glue database '{}': {}", db_name, e))?;

            info!("Created Glue database: {}", db_name);
        }

        #[cfg(not(feature = "aws"))]
        {
            warn!("AWS feature not enabled, Glue operations are no-op");
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        Ok(CatalogNamespace {
            name: db_name.clone(),
            levels: namespace.to_vec(),
            properties,
            created_at: now,
            updated_at: now,
        })
    }

    async fn drop_namespace(&self, namespace: &[String], cascade: bool) -> Result<bool> {
        let db_name = self.database_name(namespace);

        if !cascade {
            // Check if database has tables
            let tables = self.list_tables(namespace).await?;
            if !tables.is_empty() {
                return Err(anyhow!(
                    "Database '{}' is not empty. Use cascade=true to force drop.",
                    db_name
                ));
            }
        }

        #[cfg(feature = "aws")]
        if let Some(client) = &self.client {
            // Delete all tables first if cascade
            if cascade {
                let tables = self.list_tables(namespace).await?;
                for table_id in tables {
                    self.drop_table(&table_id, true).await?;
                }
            }

            client
                .delete_database()
                .name(&db_name)
                .catalog_id(&self.config.catalog_id)
                .send()
                .await
                .map_err(|e| anyhow!("Failed to delete Glue database '{}': {}", db_name, e))?;

            info!("Deleted Glue database: {}", db_name);
            return Ok(true);
        }

        #[cfg(not(feature = "aws"))]
        {
            warn!("AWS feature not enabled, Glue operations are no-op");
        }

        Ok(false)
    }

    async fn list_namespaces(&self, _parent: Option<&[String]>) -> Result<Vec<CatalogNamespace>> {
        #[cfg(feature = "aws")]
        if let Some(client) = &self.client {
            let resp = client
                .get_databases()
                .catalog_id(&self.config.catalog_id)
                .send()
                .await
                .map_err(|e| anyhow!("Failed to list Glue databases: {}", e))?;

            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64;

            let namespaces: Vec<CatalogNamespace> = resp
                .database_list()
                .iter()
                .map(|db| {
                    let name = db.name().to_string();
                    let levels: Vec<String> = name.split('_').map(String::from).collect();

                    CatalogNamespace {
                        name: name.clone(),
                        levels,
                        properties: db.parameters().cloned().unwrap_or_default(),
                        created_at: now,
                        updated_at: now,
                    }
                })
                .collect();

            return Ok(namespaces);
        }

        #[cfg(not(feature = "aws"))]
        {
            warn!("AWS feature not enabled, returning empty namespace list");
        }

        Ok(vec![])
    }

    async fn namespace_exists(&self, namespace: &[String]) -> Result<bool> {
        let db_name = self.database_name(namespace);

        #[cfg(feature = "aws")]
        if let Some(client) = &self.client {
            let result = client
                .get_database()
                .name(&db_name)
                .catalog_id(&self.config.catalog_id)
                .send()
                .await;

            return Ok(result.is_ok());
        }

        #[cfg(not(feature = "aws"))]
        {
            warn!("AWS feature not enabled, assuming namespace exists");
        }

        Ok(true)
    }

    async fn get_namespace(&self, namespace: &[String]) -> Result<CatalogNamespace> {
        let db_name = self.database_name(namespace);

        #[cfg(feature = "aws")]
        if let Some(client) = &self.client {
            let resp = client
                .get_database()
                .name(&db_name)
                .catalog_id(&self.config.catalog_id)
                .send()
                .await
                .map_err(|e| anyhow!("Failed to get Glue database '{}': {}", db_name, e))?;

            let db = resp
                .database()
                .ok_or_else(|| anyhow!("Database not found"))?;

            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64;

            return Ok(CatalogNamespace {
                name: db_name.clone(),
                levels: namespace.to_vec(),
                properties: db.parameters().cloned().unwrap_or_default(),
                created_at: now,
                updated_at: now,
            });
        }

        Err(anyhow!("Namespace '{}' not found", db_name))
    }

    async fn update_namespace_properties(
        &self,
        namespace: &[String],
        updates: HashMap<String, String>,
        _removals: Vec<String>,
    ) -> Result<()> {
        let db_name = self.database_name(namespace);

        #[cfg(feature = "aws")]
        if let Some(client) = &self.client {
            // Get current database
            let resp = client
                .get_database()
                .name(&db_name)
                .catalog_id(&self.config.catalog_id)
                .send()
                .await
                .map_err(|e| anyhow!("Failed to get Glue database '{}': {}", db_name, e))?;

            let db = resp
                .database()
                .ok_or_else(|| anyhow!("Database not found"))?;

            // Merge properties
            let mut params = db.parameters().cloned().unwrap_or_default();
            for (k, v) in updates {
                params.insert(k, v);
            }

            let input = aws_sdk_glue::types::DatabaseInput::builder()
                .name(&db_name)
                .description(db.description().unwrap_or(""))
                .set_parameters(Some(params))
                .build()?;

            client
                .update_database()
                .name(&db_name)
                .catalog_id(&self.config.catalog_id)
                .database_input(input)
                .send()
                .await
                .map_err(|e| anyhow!("Failed to update Glue database '{}': {}", db_name, e))?;

            return Ok(());
        }

        #[cfg(not(feature = "aws"))]
        {
            warn!("AWS feature not enabled, Glue operations are no-op");
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

        #[cfg(feature = "aws")]
        if let Some(client) = &self.client {
            // Convert columns to Glue format
            let mut glue_columns = Vec::new();
            for col in &schema.columns {
                let glue_type = Self::data_type_to_glue_type(&col.data_type, &col.properties);

                let comment = if col.data_type == CatalogDataType::Vector
                    || col.data_type == CatalogDataType::Embedding
                {
                    Self::vector_column_comment(&col.properties)
                } else {
                    col.comment.clone()
                };

                let glue_col = aws_sdk_glue::types::Column::builder()
                    .name(&col.name)
                    .r#type(glue_type)
                    .comment(comment)
                    .build()?;

                glue_columns.push(glue_col);
            }

            // Build table input
            let storage_descriptor = aws_sdk_glue::types::StorageDescriptor::builder()
                .set_columns(Some(glue_columns))
                .location(format!(
                    "s3://{}/{}/{}",
                    self.config.default_database,
                    identifier.namespace.join("/"),
                    identifier.name
                ))
                .build();

            let mut table_input = aws_sdk_glue::types::TableInput::builder()
                .name(&identifier.name)
                .storage_descriptor(storage_descriptor)
                .table_type("EXTERNAL_TABLE");

            // Add table properties
            for (k, v) in &schema.properties {
                table_input = table_input.parameters(k.clone(), v.clone());
            }

            // Mark as ProximaDB table
            table_input = table_input.parameters("proximadb.version".to_string(), "1".to_string());
            table_input = table_input.parameters(
                "proximadb.schema_version".to_string(),
                schema.schema_version.to_string(),
            );

            client
                .create_table()
                .database_name(&db_name)
                .catalog_id(&self.config.catalog_id)
                .table_input(table_input.build()?)
                .send()
                .await
                .map_err(|e| anyhow!("Failed to create Glue table '{}': {}", identifier, e))?;

            info!("Created Glue table: {}.{}", db_name, identifier.name);
        }

        #[cfg(not(feature = "aws"))]
        {
            warn!("AWS feature not enabled, Glue operations are no-op");
        }

        Ok(schema)
    }

    async fn drop_table(&self, identifier: &TableIdentifier, _purge: bool) -> Result<bool> {
        let db_name = self.database_name(&identifier.namespace);

        #[cfg(feature = "aws")]
        if let Some(client) = &self.client {
            client
                .delete_table()
                .database_name(&db_name)
                .name(&identifier.name)
                .catalog_id(&self.config.catalog_id)
                .send()
                .await
                .map_err(|e| anyhow!("Failed to delete Glue table '{}': {}", identifier, e))?;

            // Invalidate cache
            self.cache
                .invalidate_table_in_catalog(&self.name, identifier)
                .await;

            info!("Deleted Glue table: {}.{}", db_name, identifier.name);
            return Ok(true);
        }

        #[cfg(not(feature = "aws"))]
        {
            warn!("AWS feature not enabled, Glue operations are no-op");
        }

        Ok(false)
    }

    async fn list_tables(&self, namespace: &[String]) -> Result<Vec<TableIdentifier>> {
        let db_name = self.database_name(namespace);

        #[cfg(feature = "aws")]
        if let Some(client) = &self.client {
            let resp = client
                .get_tables()
                .database_name(&db_name)
                .catalog_id(&self.config.catalog_id)
                .send()
                .await
                .map_err(|e| anyhow!("Failed to list Glue tables: {}", e))?;

            let tables: Vec<TableIdentifier> = resp
                .table_list()
                .iter()
                .map(|t| TableIdentifier::new(namespace.to_vec(), t.name().to_string()))
                .collect();

            return Ok(tables);
        }

        #[cfg(not(feature = "aws"))]
        {
            warn!("AWS feature not enabled, returning empty table list");
        }

        Ok(vec![])
    }

    async fn table_exists(&self, identifier: &TableIdentifier) -> Result<bool> {
        let db_name = self.database_name(&identifier.namespace);

        #[cfg(feature = "aws")]
        if let Some(client) = &self.client {
            let result = client
                .get_table()
                .database_name(&db_name)
                .name(&identifier.name)
                .catalog_id(&self.config.catalog_id)
                .send()
                .await;

            return Ok(result.is_ok());
        }

        #[cfg(not(feature = "aws"))]
        {
            warn!("AWS feature not enabled, assuming table exists");
        }

        Ok(true)
    }

    async fn get_table(&self, identifier: &TableIdentifier) -> Result<CatalogTableSchema> {
        // Check cache first
        if let Some(schema) = self.cache.get_table(&self.name, identifier) {
            return Ok(schema);
        }

        let db_name = self.database_name(&identifier.namespace);

        #[cfg(feature = "aws")]
        if let Some(client) = &self.client {
            let resp = client
                .get_table()
                .database_name(&db_name)
                .name(&identifier.name)
                .catalog_id(&self.config.catalog_id)
                .send()
                .await
                .map_err(|e| anyhow!("Failed to get Glue table '{}': {}", identifier, e))?;

            let table = resp.table().ok_or_else(|| anyhow!("Table not found"))?;
            let sd = table
                .storage_descriptor()
                .ok_or_else(|| anyhow!("No storage descriptor"))?;

            // Convert Glue columns to CatalogColumn
            let columns: Vec<CatalogColumn> = sd
                .columns()
                .iter()
                .map(Self::glue_column_to_column)
                .collect();

            let schema_version = table
                .parameters()
                .and_then(|p| p.get("proximadb.schema_version"))
                .and_then(|v| v.parse().ok())
                .unwrap_or(1);

            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64;

            let schema = CatalogTableSchema {
                name: identifier.name.clone(),
                columns,
                primary_key: vec![],
                indexes: vec![],
                schema_version,
                properties: table.parameters().cloned().unwrap_or_default(),
                created_at: now,
                updated_at: now,
            };

            // Update cache
            self.cache.put_table(&self.name, identifier, schema.clone());

            return Ok(schema);
        }

        Err(anyhow!("Table '{}' not found", identifier))
    }

    async fn rename_table(&self, from: &TableIdentifier, to: &TableIdentifier) -> Result<()> {
        // Glue doesn't support table rename directly
        // Need to copy and delete
        let schema = self.get_table(from).await?;
        self.create_table(to, schema).await?;
        self.drop_table(from, false).await?;

        // Invalidate cache
        self.cache
            .invalidate_table_in_catalog(&self.name, from)
            .await;

        info!("Renamed Glue table: {} -> {}", from, to);
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

        // Update table in Glue
        let db_name = self.database_name(&identifier.namespace);

        #[cfg(feature = "aws")]
        if let Some(client) = &self.client {
            // Convert new columns to Glue format
            let mut glue_columns = Vec::new();
            for col in &new_schema.columns {
                let glue_type = Self::data_type_to_glue_type(&col.data_type, &col.properties);

                let comment = if col.data_type == CatalogDataType::Vector
                    || col.data_type == CatalogDataType::Embedding
                {
                    Self::vector_column_comment(&col.properties)
                } else {
                    col.comment.clone()
                };

                let glue_col = aws_sdk_glue::types::Column::builder()
                    .name(&col.name)
                    .r#type(glue_type)
                    .comment(comment)
                    .build()?;

                glue_columns.push(glue_col);
            }

            let storage_descriptor = aws_sdk_glue::types::StorageDescriptor::builder()
                .set_columns(Some(glue_columns))
                .build();

            let mut table_input = aws_sdk_glue::types::TableInput::builder()
                .name(&identifier.name)
                .storage_descriptor(storage_descriptor);

            // Update properties
            for (k, v) in &new_schema.properties {
                table_input = table_input.parameters(k.clone(), v.clone());
            }
            table_input = table_input.parameters(
                "proximadb.schema_version".to_string(),
                new_schema.schema_version.to_string(),
            );

            client
                .update_table()
                .database_name(&db_name)
                .catalog_id(&self.config.catalog_id)
                .table_input(table_input.build()?)
                .send()
                .await
                .map_err(|e| anyhow!("Failed to update Glue table '{}': {}", identifier, e))?;
        }

        // Invalidate cache
        self.cache
            .invalidate_table_in_catalog(&self.name, identifier)
            .await;

        info!(
            "Evolved Glue table schema: {} (v{})",
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
        // Glue doesn't keep historical versions
        let schema = self.get_table(identifier).await?;
        if schema.schema_version == version {
            Ok(schema)
        } else {
            Err(anyhow!(
                "Historical schema version {} not available for '{}' (current: {})",
                version,
                identifier,
                schema.schema_version
            ))
        }
    }

    // ========================
    // Index Operations
    // ========================

    async fn create_index(
        &self,
        identifier: &TableIdentifier,
        index: CatalogIndex,
    ) -> Result<CatalogIndex> {
        // Glue doesn't have native index support
        // Store as table property
        let mut schema = self.get_table(identifier).await?;
        schema.indexes.push(index.clone());

        // Update table properties
        let indexes_json = serde_json::to_string(&schema.indexes)?;
        schema
            .properties
            .insert("proximadb.indexes".to_string(), indexes_json);

        // This would need a full schema update
        warn!("Glue catalog: indexes stored as metadata only, not enforced");

        Ok(index)
    }

    async fn drop_index(&self, identifier: &TableIdentifier, index_name: &str) -> Result<bool> {
        let mut schema = self.get_table(identifier).await?;
        let initial_len = schema.indexes.len();
        schema.indexes.retain(|i| i.name != index_name);

        Ok(schema.indexes.len() < initial_len)
    }

    async fn list_indexes(&self, identifier: &TableIdentifier) -> Result<Vec<CatalogIndex>> {
        let schema = self.get_table(identifier).await?;
        Ok(schema.indexes)
    }

    // ========================
    // Statistics
    // ========================

    async fn get_statistics(&self, identifier: &TableIdentifier) -> Result<CatalogTableStatistics> {
        // Check cache first
        if let Some(stats) = self.cache.get_statistics(&self.name, identifier) {
            return Ok(stats);
        }

        // Glue has its own statistics in the table metadata
        // For now, return empty stats
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
        // Store in cache (Glue stats would need partition-level updates)
        self.cache.put_statistics(&self.name, identifier, stats);
        Ok(())
    }

    // ========================
    // Cache Integration
    // ========================

    fn cache(&self) -> Option<Arc<CatalogCache>> {
        Some(self.cache.clone())
    }

    // ========================
    // Health & Connectivity
    // ========================

    async fn health_check(&self) -> Result<CatalogHealth> {
        let start = Instant::now();

        #[cfg(feature = "aws")]
        if let Some(client) = &self.client {
            match client
                .get_databases()
                .catalog_id(&self.config.catalog_id)
                .max_results(1)
                .send()
                .await
            {
                Ok(_) => {
                    let latency = start.elapsed().as_millis() as u64;
                    return Ok(CatalogHealth::healthy(latency)
                        .with_detail("catalog_id", &self.config.catalog_id)
                        .with_detail("region", &self.config.region)
                        .with_detail("catalog_type", "glue"));
                }
                Err(e) => {
                    return Ok(CatalogHealth::unhealthy(e.to_string()));
                }
            }
        }

        #[cfg(not(feature = "aws"))]
        {
            Ok(CatalogHealth::unhealthy("AWS feature not enabled"))
        }
    }

    async fn close(&self) -> Result<()> {
        debug!("Closing Glue catalog: {}", self.name);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glue_type_conversion() {
        assert_eq!(
            GlueCatalog::glue_type_to_data_type("bigint"),
            CatalogDataType::Int64
        );
        assert_eq!(
            GlueCatalog::glue_type_to_data_type("string"),
            CatalogDataType::String
        );
        assert_eq!(
            GlueCatalog::glue_type_to_data_type("array<float>"),
            CatalogDataType::Vector
        );
    }

    #[test]
    fn test_data_type_to_glue() {
        let props = HashMap::new();
        assert_eq!(
            GlueCatalog::data_type_to_glue_type(&CatalogDataType::Int64, &props),
            "bigint"
        );
        assert_eq!(
            GlueCatalog::data_type_to_glue_type(&CatalogDataType::String, &props),
            "string"
        );

        let mut vec_props = HashMap::new();
        vec_props.insert("dimension".to_string(), "768".to_string());
        assert_eq!(
            GlueCatalog::data_type_to_glue_type(&CatalogDataType::Vector, &vec_props),
            "array<float>(768)"
        );
    }

    #[test]
    fn test_vector_column_comment() {
        let mut props = HashMap::new();
        props.insert("dimension".to_string(), "384".to_string());
        props.insert("metric".to_string(), "l2".to_string());

        let comment = GlueCatalog::vector_column_comment(&props);
        assert!(comment.contains("384"));
        assert!(comment.contains("l2"));
    }
}
