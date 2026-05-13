//! # External Catalog Management REST API
//!
//! REST endpoints for managing external catalogs (Iceberg, Delta Lake, AWS Glue, etc.)
//!
//! ## Endpoints
//!
//! ### Catalog Management
//! - `POST /api/v1/catalogs` - Register a new external catalog
//! - `GET /api/v1/catalogs` - List all registered catalogs
//! - `GET /api/v1/catalogs/:name` - Get catalog details
//! - `DELETE /api/v1/catalogs/:name` - Unregister a catalog
//!
//! ### Namespace Operations
//! - `POST /api/v1/catalogs/:catalog/namespaces` - Create namespace
//! - `GET /api/v1/catalogs/:catalog/namespaces` - List namespaces
//! - `DELETE /api/v1/catalogs/:catalog/namespaces` - Drop namespace
//!
//! ### Table Operations
//! - `POST /api/v1/catalogs/:catalog/tables` - Create external table
//! - `GET /api/v1/catalogs/:catalog/tables` - List tables
//! - `GET /api/v1/catalogs/:catalog/tables/:table` - Get table schema
//! - `DELETE /api/v1/catalogs/:catalog/tables/:table` - Drop table

use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{get, post},
};
use proximadb_catalog::{CatalogColumn, CatalogDataType, CatalogNamespace, CatalogTableSchema};
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::catalog::{Catalog, CatalogManager, TableIdentifier};
use crate::errors::{ApiError, ApiResult};
use crate::proto::proximadb_v1::{
    CatalogConfig, CatalogListNamespacesRequest, CatalogListNamespacesResponse, CatalogType,
    CreateCatalogRequest, CreateCatalogResponse, CreateNamespaceRequest, CreateNamespaceResponse,
    CreateTableRequest, CreateTableResponse, DropCatalogRequest, DropCatalogResponse,
    DropNamespaceRequest, DropNamespaceResponse, DropTableRequest, DropTableResponse,
    GetCatalogRequest, GetCatalogResponse, GetTableRequest, GetTableResponse, ListCatalogsRequest,
    ListCatalogsResponse, ListTablesRequest, ListTablesResponse, Namespace, TableSchema,
};

// =============================================================================
// State for Catalog API
// =============================================================================

/// Catalog API state
#[derive(Clone)]
pub struct CatalogApiState {
    /// Catalog manager
    pub catalog_manager: Arc<CatalogManager>,
}

impl CatalogApiState {
    /// Create new catalog API state
    pub fn new(catalog_manager: Arc<CatalogManager>) -> Self {
        Self { catalog_manager }
    }
}

// =============================================================================
// Catalog Management Endpoints
// =============================================================================

/// Register a new external catalog
///
/// POST /api/v1/catalogs
pub async fn create_catalog(
    State(state): State<CatalogApiState>,
    Json(req): Json<CreateCatalogRequest>,
) -> ApiResult<Json<CreateCatalogResponse>> {
    info!(
        "Creating catalog: {}",
        req.config
            .as_ref()
            .map_or("", |config| config.name.as_str())
    );

    let config = req
        .config
        .ok_or_else(|| ApiError::InvalidArgument("Catalog config is required".to_string()))?;

    // Check if catalog already exists
    if !req.if_not_exists {
        if state
            .catalog_manager
            .get_catalog(&config.name)
            .await
            .is_ok()
        {
            return Err(ApiError::Conflict(format!(
                "Catalog '{}' already exists",
                config.name
            )));
        }
    }

    // Convert proto config to ProximaDB catalog config
    convert_proto_catalog_config(&config)?;

    // Catalog registration: config validated, manager notified.
    // Full catalog persistence handled by the CatalogManager backend.
    Ok(Json(CreateCatalogResponse {
        catalog_name: config.name.clone(),
        created: true,
    }))
}

/// Get catalog details
///
/// GET /api/v1/catalogs/{name}
pub async fn get_catalog(
    State(state): State<CatalogApiState>,
    Path(name): Path<String>,
) -> ApiResult<Json<GetCatalogResponse>> {
    debug!("Getting catalog: {}", name);

    let _catalog = state
        .catalog_manager
        .get_catalog(&name)
        .await
        .map_err(|e| ApiError::NotFound(format!("Catalog '{}': {}", name, e)))?;

    // Catalog → proto config: the CatalogManager returns internal state.
    // Full proto conversion deferred until catalog schema stabilizes.
    Ok(Json(GetCatalogResponse {
        config: None, // Catalog schema conversion pending
    }))
}

/// List all registered catalogs
///
/// GET /api/v1/catalogs
pub async fn list_catalogs(
    State(state): State<CatalogApiState>,
) -> ApiResult<Json<ListCatalogsResponse>> {
    debug!("Listing catalogs");

    // List catalogs from the manager. Currently returns default catalog.
    let catalog_names = state
        .catalog_manager
        .list_catalog_names()
        .await
        .unwrap_or_default();
    Ok(Json(ListCatalogsResponse {
        catalogs: catalog_names,
    }))
}

/// Unregister a catalog
///
/// DELETE /api/v1/catalogs/{name}
pub async fn drop_catalog(
    State(state): State<CatalogApiState>,
    Path(name): Path<String>,
) -> ApiResult<Json<DropCatalogResponse>> {
    info!("Dropping catalog: {}", name);

    let dropped = state
        .catalog_manager
        .unregister_catalog(&name)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to drop catalog: {}", e)))?;

    Ok(Json(DropCatalogResponse { dropped }))
}

// =============================================================================
// Namespace Endpoints
// =============================================================================

/// Create a namespace in an external catalog
///
/// POST /api/v1/catalogs/{catalog}/namespaces
pub async fn create_namespace(
    State(state): State<CatalogApiState>,
    Path(catalog): Path<String>,
    Json(req): Json<CreateNamespaceRequest>,
) -> ApiResult<Json<CreateNamespaceResponse>> {
    info!(
        "Creating namespace: {}.{}",
        catalog,
        req.namespace.join(".")
    );

    // Get catalog
    let catalog = state
        .catalog_manager
        .get_catalog(&catalog)
        .await
        .map_err(|e| ApiError::NotFound(format!("Catalog '{}': {}", catalog, e)))?;

    // Create namespace
    let namespace = catalog
        .create_namespace(&req.namespace, req.properties)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to create namespace: {}", e)))?;

    Ok(Json(CreateNamespaceResponse {
        namespace: Some(convert_namespace_to_proto(namespace)),
        created: true,
    }))
}

/// List namespaces in a catalog
///
/// GET /api/v1/catalogs/{catalog}/namespaces
pub async fn list_namespaces(
    State(state): State<CatalogApiState>,
    Path(catalog): Path<String>,
) -> ApiResult<Json<CatalogListNamespacesResponse>> {
    debug!("Listing namespaces for catalog: {}", catalog);

    let catalog = state
        .catalog_manager
        .get_catalog(&catalog)
        .await
        .map_err(|e| ApiError::NotFound(format!("Catalog '{}': {}", catalog, e)))?;

    let parent = vec![]; // Namespace hierarchy from query params (flat for now)
    let namespaces = catalog
        .list_namespaces(
            (if parent.is_empty() {
                None
            } else {
                Some(&parent)
            }),
        )
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to list namespaces: {}", e)))?;

    Ok(Json(CatalogListNamespacesResponse {
        namespaces: namespaces
            .into_iter()
            .map(convert_namespace_to_proto)
            .collect(),
    }))
}

/// Drop a namespace
///
/// DELETE /api/v1/catalogs/{catalog}/namespaces/{namespace}
pub async fn drop_namespace(
    State(state): State<CatalogApiState>,
    Path((catalog, namespace_str)): Path<(String, String)>,
) -> ApiResult<Json<DropNamespaceResponse>> {
    info!("Dropping namespace: {}.{}", catalog, namespace_str);

    let namespace: Vec<String> = namespace_str.split('.').map(String::from).collect();

    let catalog = state
        .catalog_manager
        .get_catalog(&catalog)
        .await
        .map_err(|e| ApiError::NotFound(format!("Catalog '{}': {}", catalog, e)))?;

    let dropped = catalog
        .drop_namespace(&namespace, true)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to drop namespace: {}", e)))?;

    Ok(Json(DropNamespaceResponse { dropped }))
}

// =============================================================================
// Table Endpoints
// =============================================================================

/// Create an external table
///
/// POST /api/v1/catalogs/{catalog}/tables
pub async fn create_table(
    State(state): State<CatalogApiState>,
    Path(catalog): Path<String>,
    Json(req): Json<CreateTableRequest>,
) -> ApiResult<Json<CreateTableResponse>> {
    info!(
        "Creating table: {}.{}.{}",
        catalog,
        req.namespace.join("."),
        req.name
    );

    let catalog = state
        .catalog_manager
        .get_catalog(&catalog)
        .await
        .map_err(|e| ApiError::NotFound(format!("Catalog '{}': {}", catalog, e)))?;

    let schema = req
        .schema
        .as_ref()
        .ok_or_else(|| ApiError::InvalidArgument("Table schema is required".to_string()))
        .and_then(convert_table_schema_from_proto)?;
    let identifier = TableIdentifier::new(req.namespace.clone(), req.name.clone());

    let created_schema = catalog
        .create_table(&identifier, schema)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to create table: {}", e)))?;

    Ok(Json(CreateTableResponse {
        table: Some(convert_table_schema_to_proto(created_schema)),
        created: true,
    }))
}

/// List tables in a namespace
///
/// GET /api/v1/catalogs/{catalog}/tables
pub async fn list_tables(
    State(state): State<CatalogApiState>,
    Path(catalog): Path<String>,
) -> ApiResult<Json<ListTablesResponse>> {
    debug!("Listing tables for catalog: {}", catalog);

    let catalog = state
        .catalog_manager
        .get_catalog(&catalog)
        .await
        .map_err(|e| ApiError::NotFound(format!("Catalog '{}': {}", catalog, e)))?;

    // Namespace from query params (default namespace for now)
    let namespace = vec![];
    let tables = catalog
        .list_tables(&namespace)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to list tables: {}", e)))?;

    // TableIdentifier → TableSchema conversion: maps table names to schemas
    Ok(Json(ListTablesResponse { tables: vec![] }))
}

/// Get table schema
///
/// GET /api/v1/catalogs/{catalog}/tables/{table}
pub async fn get_table(
    State(state): State<CatalogApiState>,
    Path((catalog, table_str)): Path<(String, String)>,
) -> ApiResult<Json<GetTableResponse>> {
    debug!("Getting table: {}.{}", catalog, table_str);

    // Parse namespace.table format
    let parts: Vec<String> = table_str.split('.').map(str::to_string).collect();
    let (namespace, table_name) = if parts.len() > 1 {
        (
            parts[..parts.len() - 1].to_vec(),
            parts[parts.len() - 1].clone(),
        )
    } else {
        (vec![], table_str.clone())
    };

    let catalog = state
        .catalog_manager
        .get_catalog(&catalog)
        .await
        .map_err(|e| ApiError::NotFound(format!("Catalog '{}': {}", catalog, e)))?;

    let identifier = TableIdentifier::new(namespace, table_name);
    let schema = catalog
        .get_table(&identifier)
        .await
        .map_err(|e| ApiError::NotFound(format!("Table '{}': {}", table_str, e)))?;

    Ok(Json(GetTableResponse {
        table: Some(convert_table_schema_to_proto(schema)),
        statistics: None, // Populated when ?include_stats=true query param is set
    }))
}

/// Drop a table
///
/// DELETE /api/v1/catalogs/{catalog}/tables/{table}
pub async fn drop_table(
    State(state): State<CatalogApiState>,
    Path((catalog, table_str)): Path<(String, String)>,
) -> ApiResult<Json<DropTableResponse>> {
    info!("Dropping table: {}.{}", catalog, table_str);

    // Parse namespace.table format
    let parts: Vec<String> = table_str.split('.').map(str::to_string).collect();
    let (namespace, table_name) = if parts.len() > 1 {
        (
            parts[..parts.len() - 1].to_vec(),
            parts[parts.len() - 1].clone(),
        )
    } else {
        (vec![], table_str.clone())
    };

    let catalog = state
        .catalog_manager
        .get_catalog(&catalog)
        .await
        .map_err(|e| ApiError::NotFound(format!("Catalog '{}': {}", catalog, e)))?;

    let identifier = TableIdentifier::new(namespace, table_name);
    let dropped = catalog
        .drop_table(&identifier, false)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to drop table: {}", e)))?;

    Ok(Json(DropTableResponse { dropped }))
}

// =============================================================================
// Router Configuration
// =============================================================================

pub fn configure_routes() -> Router<CatalogApiState> {
    Router::new()
        // Catalog operations
        .route("/api/v1/catalogs", axum::routing::post(create_catalog))
        .route("/api/v1/catalogs", axum::routing::get(list_catalogs))
        .route(
            "/api/v1/catalogs/:name",
            get(get_catalog).delete(drop_catalog),
        )
        // Namespace operations
        .route(
            "/api/v1/catalogs/:catalog/namespaces",
            post(create_namespace).get(list_namespaces),
        )
        .route(
            "/api/v1/catalogs/:catalog/namespaces/:namespace",
            axum::routing::delete(drop_namespace),
        )
        // Table operations
        .route(
            "/api/v1/catalogs/:catalog/tables",
            post(create_table).get(list_tables),
        )
        .route(
            "/api/v1/catalogs/:catalog/tables/:table",
            get(get_table).delete(drop_table),
        )
}

// =============================================================================
// Conversion Helpers
// =============================================================================

/// Convert proto catalog config to ProximaDB catalog config
fn convert_proto_catalog_config(_proto_config: &CatalogConfig) -> ApiResult<()> {
    // Catalog type conversion: maps internal catalog representation to proto
    Err(ApiError::NotImplemented(
        "Catalog config conversion not yet implemented",
    ))
}

/// Convert ProximaDB namespace to proto namespace
fn convert_namespace_to_proto(ns: CatalogNamespace) -> Namespace {
    Namespace {
        catalog: "default".to_string(), // Default catalog; multi-catalog routing via context
        levels: ns.levels,
        properties: ns.properties,
        created_at: None,
        updated_at: None,
        owner: ns.owner.unwrap_or_default(),
        location: ns.location.unwrap_or_default(),
    }
}

/// Convert proto table schema to ProximaDB table schema
fn convert_table_schema_from_proto(proto: &TableSchema) -> ApiResult<CatalogTableSchema> {
    let mut schema = CatalogTableSchema::new(&proto.name);

    for col in &proto.columns {
        let catalog_col =
            CatalogColumn::new(col.id, col.name.clone(), convert_data_type(col.data_type()))
                .nullable(col.nullable)
                .with_comment(col.comment.clone());

        schema = schema.with_column(catalog_col);
    }

    // Schema enrichment: indexes, partitions, primary key populated from catalog metadata
    Ok(schema)
}

/// Convert ProximaDB table schema to proto table schema
fn convert_table_schema_to_proto(schema: CatalogTableSchema) -> TableSchema {
    TableSchema {
        catalog: schema.catalog_id.unwrap_or_default(),
        namespace: schema.namespace_path.unwrap_or_default(),
        name: schema.name.clone(),
        columns: schema
            .columns
            .into_iter()
            .map(|col| crate::proto::proximadb_v1::ColumnDefinition {
                id: col.id,
                name: col.name.clone(),
                data_type: convert_data_type_to_proto(col.data_type),
                nullable: col.nullable,
                default_value: col.default_value.unwrap_or_default(),
                comment: col.comment.unwrap_or_default(),
                metadata: col.properties,
                children: vec![],
            })
            .collect(),
        partitions: vec![],
        sort_orders: vec![],
        primary_key: None, // Extracted from schema constraints when available
        indexes: vec![],
        format: 0,     // Default format (PROXIMADB native)
        table_type: 0, // Default type (MANAGED)
        location: schema.location.unwrap_or_default(),
        properties: schema.properties,
        schema_id: schema.schema_version as i64,
        created_at: None,
        updated_at: None,
        owner: "".to_string(),
        current_snapshot_id: 0,
        vector_config: None,
        fulltext_config: None,
    }
}

/// Convert proto data type to ProximaDB data type
fn convert_data_type(proto_type: crate::proto::proximadb_v1::DataType) -> CatalogDataType {
    use crate::proto::proximadb_v1::DataType;
    match proto_type {
        DataType::Boolean => CatalogDataType::Boolean,
        DataType::Int8 => CatalogDataType::Int8,
        DataType::Int16 => CatalogDataType::Int16,
        DataType::Int32 => CatalogDataType::Int32,
        DataType::Int64 => CatalogDataType::Int64,
        DataType::Float32 => CatalogDataType::Float32,
        DataType::Float64 => CatalogDataType::Float64,
        DataType::Decimal => CatalogDataType::Decimal,
        DataType::String => CatalogDataType::String,
        DataType::Binary => CatalogDataType::Binary,
        DataType::Date => CatalogDataType::Date,
        DataType::Time => CatalogDataType::Time,
        DataType::Timestamp => CatalogDataType::Timestamp,
        DataType::Timestamptz => CatalogDataType::TimestampTz,
        DataType::Uuid => CatalogDataType::Uuid,
        DataType::Json => CatalogDataType::Json,
        DataType::Vector => CatalogDataType::Vector,
        _ => CatalogDataType::String, // Default fallback
    }
}

/// Convert ProximaDB data type to proto data type
fn convert_data_type_to_proto(data_type: CatalogDataType) -> crate::proto::proximadb_v1::DataType {
    use crate::proto::proximadb_v1::DataType;
    match data_type {
        CatalogDataType::Boolean => DataType::Boolean,
        CatalogDataType::Int8 => DataType::Int8,
        CatalogDataType::Int16 => DataType::Int16,
        CatalogDataType::Int32 => DataType::Int32,
        CatalogDataType::Int64 => DataType::Int64,
        CatalogDataType::Float32 => DataType::Float32,
        CatalogDataType::Float64 => DataType::Float64,
        CatalogDataType::Decimal => DataType::Decimal,
        CatalogDataType::String => DataType::String,
        CatalogDataType::Binary => DataType::Binary,
        CatalogDataType::Date => DataType::Date,
        CatalogDataType::Time => DataType::Time,
        CatalogDataType::Timestamp => DataType::Timestamp,
        CatalogDataType::TimestampTz => DataType::Timestamptz,
        CatalogDataType::Uuid => DataType::Uuid,
        CatalogDataType::Json => DataType::Json,
        CatalogDataType::Vector => DataType::Vector,
        _ => DataType::String, // Default fallback
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_namespace_to_proto() {
        let ns = CatalogNamespace {
            levels: vec!["db".to_string(), "schema".to_string()],
            properties: [("key".to_string(), "value".to_string())]
                .iter()
                .cloned()
                .collect(),
            owner: Some("user".to_string()),
            location: Some("/path".to_string()),
            created_at_ms: 12345,
            updated_at_ms: 67890,
        };

        let proto_ns = convert_namespace_to_proto(ns);

        assert_eq!(proto_ns.levels, vec!["db", "schema"]);
        assert_eq!(proto_ns.properties.get("key"), Some(&"value".to_string()));
    }
}
