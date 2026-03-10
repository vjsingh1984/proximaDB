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
    extract::{Extension, Path, State},
    http::StatusCode,
    response::{IntoResponse, Json as JsonResponse},
    routing::get,
    Router,
};
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::catalog::{
    Catalog, CatalogManager, TableIdentifier,
    types::{
        CatalogNamespace, CatalogTableSchema, CatalogDataType, CatalogColumn,
        CatalogConfig as ProximaCatalogConfig,
    },
};
use crate::errors::{ApiError, ApiResult};
use crate::network::rest::v1::handlers::AppState;
use crate::proto::proximadb_v1::{
    CatalogConfig, CatalogType,
    CreateCatalogRequest, CreateCatalogResponse,
    GetCatalogRequest, GetCatalogResponse,
    ListCatalogsRequest, ListCatalogsResponse,
    DropCatalogRequest, DropCatalogResponse,
    CreateNamespaceRequest, CreateNamespaceResponse,
    CatalogListNamespacesRequest, CatalogListNamespacesResponse,
    DropNamespaceRequest, DropNamespaceResponse,
    CreateTableRequest, CreateTableResponse,
    ListTablesRequest, ListTablesResponse,
    GetTableRequest, GetTableResponse,
    DropTableRequest, DropTableResponse,
    Namespace, TableSchema,
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
    info!("Creating catalog: {}", req.config.as_ref().map(|c| &c.name).unwrap_or(""));

    let config = req.config.ok_or_else(|| {
        ApiError::bad_request("Catalog config is required")
    })?;

    // Check if catalog already exists
    if !req.if_not_exists {
        if state.catalog_manager.get_catalog(&config.name).await.is_ok() {
            return Err(ApiError::conflict(format!(
                "Catalog '{}' already exists",
                config.name
            )));
        }
    }

    // Convert proto config to ProximaDB catalog config
    let _proxima_config = convert_proto_catalog_config(&config)?;

    // TODO: Create and register the catalog instance
    // For now, just return success
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
        .map_err(|e| ApiError::not_found(format!("Catalog '{}': {}", name, e)))?;

    // TODO: Convert catalog to proto config
    Ok(Json(GetCatalogResponse {
        config: None, // TODO: Convert from catalog
    }))
}

/// List all registered catalogs
///
/// GET /api/v1/catalogs
pub async fn list_catalogs(
    State(state): State<CatalogApiState>,
) -> ApiResult<Json<ListCatalogsResponse>> {
    debug!("Listing catalogs");

    // TODO: Get actual catalog list
    Ok(Json(ListCatalogsResponse {
        catalogs: vec![],
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
        .map_err(|e| ApiError::internal_error(format!("Failed to drop catalog: {}", e)))?;

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
        .map_err(|e| ApiError::not_found(format!("Catalog '{}': {}", catalog, e)))?;

    // Create namespace
    let namespace = catalog
        .create_namespace(&req.namespace, req.properties)
        .await
        .map_err(|e| ApiError::internal_error(format!("Failed to create namespace: {}", e)))?;

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
        .map_err(|e| ApiError::not_found(format!("Catalog '{}': {}", catalog, e)))?;

    let parent = vec![]; // TODO: from query params
    let namespaces = catalog
        .list_namespaces(if parent.is_empty() { None } else { Some(&parent) }))
        .await
        .map_err(|e| ApiError::internal_error(format!("Failed to list namespaces: {}", e)))?;

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
    info!(
        "Dropping namespace: {}.{}",
        catalog,
        namespace_str
    );

    let namespace: Vec<String> = namespace_str.split('.').map(String::from).collect();

    let catalog = state
        .catalog_manager
        .get_catalog(&catalog)
        .await
        .map_err(|e| ApiError::not_found(format!("Catalog '{}': {}", catalog, e)))?;

    let dropped = catalog
        .drop_namespace(&namespace, true)
        .await
        .map_err(|e| ApiError::internal_error(format!("Failed to drop namespace: {}", e)))?;

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
        .map_err(|e| ApiError::not_found(format!("Catalog '{}': {}", catalog, e)))?;

    let schema = convert_table_schema_from_proto(&req.schema)?;
    let identifier = TableIdentifier::new(req.namespace.clone(), req.name.clone());

    let created_schema = catalog
        .create_table(&identifier, schema)
        .await
        .map_err(|e| ApiError::internal_error(format!("Failed to create table: {}", e)))?;

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
        .map_err(|e| ApiError::not_found(format!("Catalog '{}': {}", catalog, e)))?;

    // TODO: Get namespace from query params
    let namespace = vec![];
    let tables = catalog
        .list_tables(&namespace)
        .await
        .map_err(|e| ApiError::internal_error(format!("Failed to list tables: {}", e)))?;

    // TODO: Convert TableIdentifiers to TableSchema
    Ok(Json(ListTablesResponse {
        tables: vec![],
    }))
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
    let parts: Vec<String> = table_str.split('.').collect();
    let (namespace, table_name) = if parts.len() > 1 {
        (parts[..parts.len() - 1].to_vec(), parts[parts.len() - 1].clone())
    } else {
        (vec![], table_str.clone())
    };

    let catalog = state
        .catalog_manager
        .get_catalog(&catalog)
        .await
        .map_err(|e| ApiError::not_found(format!("Catalog '{}': {}", catalog, e)))?;

    let identifier = TableIdentifier::new(namespace, table_name);
    let schema = catalog
        .get_table(&identifier)
        .await
        .map_err(|e| ApiError::not_found(format!("Table '{}': {}", table_str, e)))?;

    Ok(Json(GetTableResponse {
        table: Some(convert_table_schema_to_proto(schema)),
        statistics: None, // TODO: Include if requested
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
    let parts: Vec<String> = table_str.split('.').collect();
    let (namespace, table_name) = if parts.len() > 1 {
        (parts[..parts.len() - 1].to_vec(), parts[parts.len() - 1].clone())
    } else {
        (vec![], table_str.clone())
    };

    let catalog = state
        .catalog_manager
        .get_catalog(&catalog)
        .await
        .map_err(|e| ApiError::not_found(format!("Catalog '{}': {}", catalog, e)))?;

    let identifier = TableIdentifier::new(namespace, table_name);
    let dropped = catalog
        .drop_table(&identifier, false)
        .await
        .map_err(|e| ApiError::internal_error(format!("Failed to drop table: {}", e)))?;

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
fn convert_proto_catalog_config(
    proto_config: &CatalogConfig,
) -> ApiResult<ProximaCatalogConfig> {
    // TODO: Implement conversion based on catalog type
    Err(ApiError::not_implemented("Catalog config conversion not yet implemented"))
}

/// Convert ProximaDB namespace to proto namespace
fn convert_namespace_to_proto(ns: CatalogNamespace) -> Namespace {
    Namespace {
        catalog: "".to_string(), // TODO: from context
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
        let catalog_col = CatalogColumn::new(
            col.id,
            col.name.clone(),
            convert_data_type(col.data_type()),
        )
        .nullable(col.nullable)
        .comment(col.comment.clone());

        schema = schema.with_column(catalog_col);
    }

    // TODO: Add indexes, partitions, primary key, etc.
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
        primary_key: None, // TODO
        indexes: vec![],
        format: 0, // TODO: Map from schema.format
        table_type: 0, // TODO: Map from schema
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
        DataType::DATA_TYPE_BOOLEAN => CatalogDataType::Boolean,
        DataType::DATA_TYPE_INT8 => CatalogDataType::Int8,
        DataType::DATA_TYPE_INT16 => CatalogDataType::Int16,
        DataType::DATA_TYPE_INT32 => CatalogDataType::Int32,
        DataType::DATA_TYPE_INT64 => CatalogDataType::Int64,
        DataType::DATA_TYPE_FLOAT32 => CatalogDataType::Float32,
        DataType::DATA_TYPE_FLOAT64 => CatalogDataType::Float64,
        DataType::DATA_TYPE_DECIMAL => CatalogDataType::Decimal,
        DataType::DATA_TYPE_STRING => CatalogDataType::String,
        DataType::DATA_TYPE_BINARY => CatalogDataType::Binary,
        DataType::DATA_TYPE_DATE => CatalogDataType::Date,
        DataType::DATA_TYPE_TIME => CatalogDataType::Time,
        DataType::DATA_TYPE_TIMESTAMP => CatalogDataType::Timestamp,
        DataType::DATA_TYPE_TIMESTAMPTZ => CatalogDataType::TimestampTz,
        DataType::DATA_TYPE_UUID => CatalogDataType::Uuid,
        DataType::DATA_TYPE_JSON => CatalogDataType::Json,
        DataType::DATA_TYPE_VECTOR => CatalogDataType::Vector,
        _ => CatalogDataType::String, // Default fallback
    }
}

/// Convert ProximaDB data type to proto data type
fn convert_data_type_to_proto(data_type: CatalogDataType) -> crate::proto::proximadb_v1::DataType {
    use crate::proto::proximadb_v1::DataType;
    match data_type {
        CatalogDataType::Boolean => DataType::DATA_TYPE_BOOLEAN,
        CatalogDataType::Int8 => DataType::DATA_TYPE_INT8,
        CatalogDataType::Int16 => DataType::DATA_TYPE_INT16,
        CatalogDataType::Int32 => DataType::DATA_TYPE_INT32,
        CatalogDataType::Int64 => DataType::DATA_TYPE_INT64,
        CatalogDataType::Float32 => DataType::DATA_TYPE_FLOAT32,
        CatalogDataType::Float64 => DataType::DATA_TYPE_FLOAT64,
        CatalogDataType::Decimal => DataType::DATA_TYPE_DECIMAL,
        CatalogDataType::String => DataType::DATA_TYPE_STRING,
        CatalogDataType::Binary => DataType::DATA_TYPE_BINARY,
        CatalogDataType::Date => DataType::DATA_TYPE_DATE,
        CatalogDataType::Time => DataType::DATA_TYPE_TIME,
        CatalogDataType::Timestamp => DataType::DATA_TYPE_TIMESTAMP,
        CatalogDataType::TimestampTz => DataType::DATA_TYPE_TIMESTAMPTZ,
        CatalogDataType::Uuid => DataType::DATA_TYPE_UUID,
        CatalogDataType::Json => DataType::DATA_TYPE_JSON,
        CatalogDataType::Vector => DataType::DATA_TYPE_VECTOR,
        _ => DataType::DATA_TYPE_STRING, // Default fallback
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
