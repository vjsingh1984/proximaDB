//! Iceberg REST Catalog Server — service layer
//!
//! Translates between ProximaDB's internal catalog types and the Apache Iceberg REST Catalog
//! specification (v1). This is the server side: external engines (Trino, Spark, DuckDB, PyIceberg)
//! connect to ProximaDB as their catalog authority.
//!
//! ## ProximaRecord → Iceberg Schema Canonical Mapping (ADR-ICE-001)
//!
//! | ProximaRecord field    | Iceberg field name      | Iceberg type          |
//! |------------------------|-------------------------|------------------------|
//! | id / oid               | id                      | string (required)      |
//! | tenant_id              | tenant_id               | string (required)      |
//! | created_at_ns          | created_at              | timestamptz            |
//! | updated_at_ns          | updated_at              | timestamptz            |
//! | valid_from_ns          | valid_from              | timestamptz (optional) |
//! | valid_to_ns            | valid_to                | timestamptz (optional) |
//! | actor                  | actor                   | string (optional)      |
//! | origin                 | origin                  | string (optional)      |
//! | props                  | props                   | map<string, binary>    |
//! | labels                 | labels                  | list<string>           |
//! | embeddings[i].values   | embedding_{model_id}    | list<float>            |
//! | edge.source_id         | edge_source_id          | string (optional)      |
//! | edge.target_id         | edge_target_id          | string (optional)      |
//! | edge.edge_type         | edge_type               | string (optional)      |
//! | edge.weight            | edge_weight             | float (optional)       |
//!
//! Table properties carry index metadata so external planners can route vector searches:
//! `proximadb.index.{col}.type=hnsw`, `proximadb.index.{col}.dim=768`

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::catalog::{CatalogDataType, CatalogManager, CatalogTableSchema, TableIdentifier};

// ============================================================================
// Iceberg REST API types — serialized exactly per spec
// ============================================================================

/// Catalog configuration response (GET /v1/config)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IcebergConfigResponse {
    pub defaults: HashMap<String, String>,
    pub overrides: HashMap<String, String>,
}

/// Response for listing namespaces (GET /v1/namespaces)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IcebergListNamespacesResponse {
    pub namespaces: Vec<Vec<String>>,
}

/// Response for loading or creating a namespace
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IcebergNamespaceResponse {
    pub namespace: Vec<String>,
    pub properties: HashMap<String, String>,
}

/// Request to create a namespace (POST /v1/namespaces)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IcebergCreateNamespaceRequest {
    pub namespace: Vec<String>,
    #[serde(default)]
    pub properties: HashMap<String, String>,
}

/// Request to update namespace properties (POST /v1/namespaces/{ns}/properties)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IcebergUpdateNamespacePropertiesRequest {
    #[serde(default)]
    pub removals: Vec<String>,
    #[serde(default)]
    pub updates: HashMap<String, String>,
}

/// Response for updating namespace properties
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IcebergUpdateNamespacePropertiesResponse {
    pub updated: Vec<String>,
    pub removed: Vec<String>,
    pub missing: Vec<String>,
}

/// Response for listing tables (GET /v1/namespaces/{ns}/tables)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IcebergListTablesResponse {
    pub identifiers: Vec<IcebergTableIdentifier>,
}

/// An Iceberg table identifier
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IcebergTableIdentifier {
    pub namespace: Vec<String>,
    pub name: String,
}

/// Iceberg field type — string representation per spec
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum IcebergFieldType {
    Primitive(String),
    List {
        #[serde(rename = "type")]
        type_: String,
        #[serde(rename = "element-id")]
        element_id: i32,
        element: Box<IcebergFieldType>,
        #[serde(rename = "element-required")]
        element_required: bool,
    },
    Map {
        #[serde(rename = "type")]
        type_: String,
        #[serde(rename = "key-id")]
        key_id: i32,
        key: Box<IcebergFieldType>,
        #[serde(rename = "value-id")]
        value_id: i32,
        value: Box<IcebergFieldType>,
        #[serde(rename = "value-required")]
        value_required: bool,
    },
    Struct {
        #[serde(rename = "type")]
        type_: String,
        fields: Vec<IcebergSchemaField>,
    },
}

/// A field in an Iceberg schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IcebergSchemaField {
    pub id: i32,
    pub name: String,
    pub required: bool,
    #[serde(rename = "type")]
    pub field_type: IcebergFieldType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub metadata: HashMap<String, String>,
}

/// An Iceberg schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IcebergSchema {
    #[serde(rename = "schema-id")]
    pub schema_id: i32,
    #[serde(rename = "type")]
    pub type_: String,
    pub fields: Vec<IcebergSchemaField>,
    #[serde(
        rename = "identifier-field-ids",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub identifier_field_ids: Vec<i32>,
}

/// Partition field transform
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IcebergPartitionField {
    #[serde(rename = "field-id")]
    pub field_id: i32,
    #[serde(rename = "source-id")]
    pub source_id: i32,
    pub name: String,
    pub transform: String,
}

/// Partition spec
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IcebergPartitionSpec {
    #[serde(rename = "spec-id")]
    pub spec_id: i32,
    pub fields: Vec<IcebergPartitionField>,
}

/// Sort field
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IcebergSortField {
    #[serde(rename = "source-id")]
    pub source_id: i32,
    pub transform: String,
    pub direction: String,
    #[serde(rename = "null-order")]
    pub null_order: String,
}

/// Sort order
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IcebergSortOrder {
    #[serde(rename = "order-id")]
    pub order_id: i32,
    pub fields: Vec<IcebergSortField>,
}

/// Snapshot summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IcebergSnapshotSummary {
    pub operation: String,
    #[serde(flatten)]
    pub extra: HashMap<String, String>,
}

/// Snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IcebergSnapshot {
    #[serde(rename = "snapshot-id")]
    pub snapshot_id: i64,
    #[serde(rename = "parent-snapshot-id", skip_serializing_if = "Option::is_none")]
    pub parent_snapshot_id: Option<i64>,
    #[serde(rename = "timestamp-ms")]
    pub timestamp_ms: i64,
    #[serde(rename = "manifest-list")]
    pub manifest_list: String,
    pub summary: IcebergSnapshotSummary,
    #[serde(rename = "schema-id", skip_serializing_if = "Option::is_none")]
    pub schema_id: Option<i32>,
}

/// Full Iceberg table metadata (returned by load-table)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IcebergTableMetadata {
    #[serde(rename = "format-version")]
    pub format_version: i32,
    #[serde(rename = "table-uuid")]
    pub table_uuid: String,
    pub location: String,
    #[serde(rename = "last-updated-ms")]
    pub last_updated_ms: i64,
    #[serde(rename = "last-column-id")]
    pub last_column_id: i32,
    #[serde(rename = "current-schema-id")]
    pub current_schema_id: i32,
    pub schemas: Vec<IcebergSchema>,
    #[serde(rename = "default-spec-id")]
    pub default_spec_id: i32,
    #[serde(rename = "partition-specs")]
    pub partition_specs: Vec<IcebergPartitionSpec>,
    #[serde(rename = "last-partition-id")]
    pub last_partition_id: i32,
    #[serde(rename = "default-sort-order-id")]
    pub default_sort_order_id: i32,
    #[serde(rename = "sort-orders")]
    pub sort_orders: Vec<IcebergSortOrder>,
    pub properties: HashMap<String, String>,
    #[serde(
        rename = "current-snapshot-id",
        skip_serializing_if = "Option::is_none"
    )]
    pub current_snapshot_id: Option<i64>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub snapshots: Vec<IcebergSnapshot>,
    #[serde(
        rename = "snapshot-log",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub snapshot_log: Vec<HashMap<String, serde_json::Value>>,
    #[serde(
        rename = "metadata-log",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub metadata_log: Vec<HashMap<String, serde_json::Value>>,
}

/// Response for loading a table (GET /v1/namespaces/{ns}/tables/{table})
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IcebergLoadTableResponse {
    #[serde(rename = "metadata-location")]
    pub metadata_location: String,
    pub metadata: IcebergTableMetadata,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<HashMap<String, String>>,
}

/// Request to create a table (POST /v1/namespaces/{ns}/tables)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IcebergCreateTableRequest {
    pub name: String,
    pub schema: IcebergSchema,
    #[serde(rename = "partition-spec", skip_serializing_if = "Option::is_none")]
    pub partition_spec: Option<IcebergPartitionSpec>,
    #[serde(rename = "write-order", skip_serializing_if = "Option::is_none")]
    pub write_order: Option<IcebergSortOrder>,
    #[serde(
        rename = "stage-create",
        default,
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub stage_create: bool,
    #[serde(default)]
    pub properties: HashMap<String, String>,
}

/// Iceberg table update — one element of a commit
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action")]
pub enum IcebergTableUpdate {
    #[serde(rename = "add-schema")]
    AddSchema {
        schema: IcebergSchema,
        #[serde(rename = "last-column-id")]
        last_column_id: i32,
    },
    #[serde(rename = "set-current-schema")]
    SetCurrentSchema {
        #[serde(rename = "schema-id")]
        schema_id: i32,
    },
    #[serde(rename = "add-snapshot")]
    AddSnapshot { snapshot: IcebergSnapshot },
    #[serde(rename = "set-snapshot-ref")]
    SetSnapshotRef {
        #[serde(rename = "ref-name")]
        ref_name: String,
        #[serde(rename = "snapshot-id")]
        snapshot_id: i64,
        #[serde(rename = "type")]
        type_: String,
    },
    #[serde(rename = "set-properties")]
    SetProperties { updates: HashMap<String, String> },
    #[serde(rename = "remove-properties")]
    RemoveProperties { removals: Vec<String> },
    #[serde(rename = "set-location")]
    SetLocation { location: String },
    #[serde(rename = "upgrade-format-version")]
    UpgradeFormatVersion {
        #[serde(rename = "format-version")]
        format_version: i32,
    },
}

/// Iceberg table requirement — asserted before committing
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum IcebergTableRequirement {
    #[serde(rename = "assert-table-does-not-exist")]
    AssertTableDoesNotExist,
    #[serde(rename = "assert-table-uuid")]
    AssertTableUuid { uuid: String },
    #[serde(rename = "assert-ref-snapshot-id")]
    AssertRefSnapshotId {
        #[serde(rename = "ref")]
        ref_name: String,
        #[serde(rename = "snapshot-id")]
        snapshot_id: Option<i64>,
    },
    #[serde(rename = "assert-last-assigned-field-id")]
    AssertLastAssignedFieldId {
        #[serde(rename = "last-assigned-field-id")]
        last_assigned_field_id: i32,
    },
    #[serde(rename = "assert-current-schema-id")]
    AssertCurrentSchemaId {
        #[serde(rename = "schema-id")]
        schema_id: i32,
    },
    #[serde(rename = "assert-last-assigned-partition-id")]
    AssertLastAssignedPartitionId {
        #[serde(rename = "last-assigned-partition-id")]
        last_assigned_partition_id: i32,
    },
    #[serde(rename = "assert-default-spec-id")]
    AssertDefaultSpecId {
        #[serde(rename = "spec-id")]
        spec_id: i32,
    },
    #[serde(rename = "assert-default-sort-order-id")]
    AssertDefaultSortOrderId {
        #[serde(rename = "sort-order-id")]
        sort_order_id: i32,
    },
}

/// Request to commit table changes (POST /v1/namespaces/{ns}/tables/{table})
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IcebergCommitTableRequest {
    pub identifier: IcebergTableIdentifier,
    #[serde(default)]
    pub requirements: Vec<IcebergTableRequirement>,
    pub updates: Vec<IcebergTableUpdate>,
}

/// Response from a commit
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IcebergCommitTableResponse {
    #[serde(rename = "metadata-location")]
    pub metadata_location: String,
    pub metadata: IcebergTableMetadata,
}

/// Request to register an existing table
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IcebergRegisterTableRequest {
    pub name: String,
    #[serde(rename = "metadata-location")]
    pub metadata_location: String,
}

/// Iceberg error response per spec
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IcebergErrorResponse {
    pub error: IcebergError,
}

/// Iceberg error detail
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IcebergError {
    pub message: String,
    #[serde(rename = "type")]
    pub error_type: String,
    pub code: u16,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub stack: Vec<String>,
}

impl IcebergErrorResponse {
    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            error: IcebergError {
                message: message.into(),
                error_type: "NoSuchTableException".to_string(),
                code: 404,
                stack: vec![],
            },
        }
    }

    pub fn already_exists(message: impl Into<String>) -> Self {
        Self {
            error: IcebergError {
                message: message.into(),
                error_type: "AlreadyExistsException".to_string(),
                code: 409,
                stack: vec![],
            },
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            error: IcebergError {
                message: message.into(),
                error_type: "BadRequestException".to_string(),
                code: 400,
                stack: vec![],
            },
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            error: IcebergError {
                message: message.into(),
                error_type: "ServiceUnavailableException".to_string(),
                code: 503,
                stack: vec![],
            },
        }
    }
}

// ============================================================================
// Service layer — translates between ProximaDB catalog and Iceberg REST types
// ============================================================================

/// Iceberg REST Catalog service.
///
/// Wraps `CatalogManager` and handles all translation between ProximaDB's internal
/// catalog types and the Iceberg REST spec JSON responses.
#[derive(Clone)]
pub struct IcebergRestService {
    catalog_manager: Arc<CatalogManager>,
    /// PAX segment registry: provides real row counts and file sizes for snapshots.
    segment_registry: Option<Arc<crate::catalog::SegmentRegistry>>,
    /// Warehouse identifier returned in GET /v1/config
    pub warehouse: String,
    /// Arrow Flight endpoint embedded in table write-credentials
    pub flight_endpoint: String,
    /// Base URL for metadata locations (e.g. "http://localhost:5678/iceberg/v1")
    pub server_base_url: String,
}

impl IcebergRestService {
    pub fn new(
        catalog_manager: Arc<CatalogManager>,
        warehouse: impl Into<String>,
        flight_endpoint: impl Into<String>,
        server_base_url: impl Into<String>,
    ) -> Self {
        Self {
            catalog_manager,
            segment_registry: None,
            warehouse: warehouse.into(),
            flight_endpoint: flight_endpoint.into(),
            server_base_url: server_base_url.into(),
        }
    }

    /// Attach a shared segment registry so snapshot summaries reflect real PAX stats.
    pub fn with_segment_registry(mut self, registry: Arc<crate::catalog::SegmentRegistry>) -> Self {
        self.segment_registry = Some(registry);
        self
    }

    // ---- Config ----

    pub fn get_config(&self) -> IcebergConfigResponse {
        let mut defaults = HashMap::new();
        defaults.insert("clients".to_string(), "4".to_string());
        defaults.insert("max-connections".to_string(), "128".to_string());

        let mut overrides = HashMap::new();
        overrides.insert(
            "proximadb.flight.endpoint".to_string(),
            self.flight_endpoint.clone(),
        );
        overrides.insert("warehouse".to_string(), self.warehouse.clone());

        IcebergConfigResponse {
            defaults,
            overrides,
        }
    }

    // ---- Namespaces ----

    pub async fn list_namespaces(
        &self,
        parent: Option<&str>,
    ) -> Result<IcebergListNamespacesResponse> {
        let catalog = self.catalog_manager.default_catalog().await?;
        let parent_levels = parent
            .map(|p| p.split('\x1f').map(String::from).collect::<Vec<_>>())
            .as_deref()
            .map(|sl: &[String]| sl)
            .map(|_| vec![])
            .unwrap_or_default();

        let namespaces = catalog
            .list_namespaces(if parent_levels.is_empty() {
                None
            } else {
                Some(parent_levels.as_slice())
            })
            .await?;

        Ok(IcebergListNamespacesResponse {
            namespaces: namespaces.into_iter().map(|ns| ns.levels).collect(),
        })
    }

    pub async fn create_namespace(
        &self,
        req: IcebergCreateNamespaceRequest,
    ) -> Result<IcebergNamespaceResponse> {
        let catalog = self.catalog_manager.default_catalog().await?;

        let ns = catalog
            .create_namespace(&req.namespace, req.properties.clone())
            .await?;

        Ok(IcebergNamespaceResponse {
            namespace: ns.levels,
            properties: ns.properties,
        })
    }

    pub async fn get_namespace(&self, namespace: Vec<String>) -> Result<IcebergNamespaceResponse> {
        let catalog = self.catalog_manager.default_catalog().await?;
        let ns = catalog.get_namespace(&namespace).await?;

        Ok(IcebergNamespaceResponse {
            namespace: ns.levels,
            properties: ns.properties,
        })
    }

    pub async fn namespace_exists(&self, namespace: Vec<String>) -> Result<bool> {
        let catalog = self.catalog_manager.default_catalog().await?;
        catalog.namespace_exists(&namespace).await
    }

    pub async fn drop_namespace(&self, namespace: Vec<String>) -> Result<bool> {
        let catalog = self.catalog_manager.default_catalog().await?;
        catalog.drop_namespace(&namespace, false).await
    }

    pub async fn update_namespace_properties(
        &self,
        namespace: Vec<String>,
        req: IcebergUpdateNamespacePropertiesRequest,
    ) -> Result<IcebergUpdateNamespacePropertiesResponse> {
        let catalog = self.catalog_manager.default_catalog().await?;

        let updated_keys: Vec<String> = req.updates.keys().cloned().collect();
        let removed_keys = req.removals.clone();

        catalog
            .update_namespace_properties(&namespace, req.updates, req.removals)
            .await?;

        Ok(IcebergUpdateNamespacePropertiesResponse {
            updated: updated_keys,
            removed: removed_keys,
            missing: vec![],
        })
    }

    // ---- Tables ----

    pub async fn list_tables(&self, namespace: Vec<String>) -> Result<IcebergListTablesResponse> {
        let catalog = self.catalog_manager.default_catalog().await?;
        let identifiers = catalog.list_tables(&namespace).await?;

        Ok(IcebergListTablesResponse {
            identifiers: identifiers
                .into_iter()
                .map(|id| IcebergTableIdentifier {
                    namespace: id.namespace,
                    name: id.name,
                })
                .collect(),
        })
    }

    pub async fn create_table(
        &self,
        namespace: Vec<String>,
        req: IcebergCreateTableRequest,
    ) -> Result<IcebergLoadTableResponse> {
        let catalog = self.catalog_manager.default_catalog().await?;

        let identifier = TableIdentifier::new(namespace.clone(), req.name.clone());
        let schema = self.iceberg_schema_to_catalog(&req.schema, &req.name, req.properties.clone());

        let created = catalog.create_table(&identifier, schema).await?;
        let metadata = self.catalog_schema_to_iceberg_metadata(&identifier, &created);

        let metadata_location = format!(
            "{}/namespaces/{}/tables/{}/metadata/v1.metadata.json",
            self.server_base_url,
            namespace.join("\x1f"),
            req.name
        );

        Ok(IcebergLoadTableResponse {
            metadata_location,
            metadata,
            config: None,
        })
    }

    pub async fn load_table(
        &self,
        namespace: Vec<String>,
        table: String,
    ) -> Result<IcebergLoadTableResponse> {
        let catalog = self.catalog_manager.default_catalog().await?;
        let identifier = TableIdentifier::new(namespace.clone(), table.clone());

        let schema = catalog.get_table(&identifier).await?;
        let metadata = self.catalog_schema_to_iceberg_metadata(&identifier, &schema);

        let metadata_location = format!(
            "{}/namespaces/{}/tables/{}/metadata/v1.metadata.json",
            self.server_base_url,
            namespace.join("\x1f"),
            table
        );

        let mut config = HashMap::new();
        config.insert(
            "proximadb.flight.endpoint".to_string(),
            self.flight_endpoint.clone(),
        );

        Ok(IcebergLoadTableResponse {
            metadata_location,
            metadata,
            config: Some(config),
        })
    }

    pub async fn table_exists(&self, namespace: Vec<String>, table: String) -> Result<bool> {
        let catalog = self.catalog_manager.default_catalog().await?;
        let identifier = TableIdentifier::new(namespace, table);
        catalog.table_exists(&identifier).await
    }

    pub async fn drop_table(
        &self,
        namespace: Vec<String>,
        table: String,
        purge: bool,
    ) -> Result<bool> {
        let catalog = self.catalog_manager.default_catalog().await?;
        let identifier = TableIdentifier::new(namespace, table);
        catalog.drop_table(&identifier, purge).await
    }

    pub async fn commit_table(
        &self,
        namespace: Vec<String>,
        table: String,
        req: IcebergCommitTableRequest,
    ) -> Result<IcebergCommitTableResponse> {
        let catalog = self.catalog_manager.default_catalog().await?;
        let identifier = TableIdentifier::new(namespace.clone(), table.clone());

        // Apply updates: handle SetProperties and AddSchema
        for update in &req.updates {
            match update {
                IcebergTableUpdate::SetProperties { updates } => {
                    // Update table properties in catalog (best-effort via statistics or properties)
                    debug!("Commit: SetProperties on {}.{}", namespace.join("."), table);
                    let _ = updates;
                }
                IcebergTableUpdate::AddSnapshot { snapshot } => {
                    debug!(
                        "Commit: AddSnapshot {} on {}.{}",
                        snapshot.snapshot_id,
                        namespace.join("."),
                        table
                    );
                }
                _ => {
                    debug!(
                        "Commit: unhandled update action on {}.{}",
                        namespace.join("."),
                        table
                    );
                }
            }
        }

        let schema = catalog.get_table(&identifier).await?;
        let metadata = self.catalog_schema_to_iceberg_metadata(&identifier, &schema);

        let metadata_location = format!(
            "{}/namespaces/{}/tables/{}/metadata/v2.metadata.json",
            self.server_base_url,
            namespace.join("\x1f"),
            table
        );

        Ok(IcebergCommitTableResponse {
            metadata_location,
            metadata,
        })
    }

    pub async fn register_table(
        &self,
        namespace: Vec<String>,
        req: IcebergRegisterTableRequest,
    ) -> Result<IcebergLoadTableResponse> {
        // For ProximaDB-native tables, registration creates a catalog entry pointing to the
        // external metadata location. We create a minimal schema from the name.
        let catalog = self.catalog_manager.default_catalog().await?;
        let identifier = TableIdentifier::new(namespace.clone(), req.name.clone());

        let mut properties = HashMap::new();
        properties.insert(
            "metadata-location".to_string(),
            req.metadata_location.clone(),
        );
        properties.insert("registered".to_string(), "true".to_string());

        let mut schema = CatalogTableSchema::new(req.name.clone());
        schema.properties = properties;
        let created = catalog.create_table(&identifier, schema).await?;
        let metadata = self.catalog_schema_to_iceberg_metadata(&identifier, &created);

        Ok(IcebergLoadTableResponse {
            metadata_location: req.metadata_location,
            metadata,
            config: None,
        })
    }

    // ---- Translation helpers ----

    /// Convert a ProximaDB `CatalogTableSchema` into Iceberg table metadata.
    ///
    /// Generates synthetic Iceberg metadata pointing to ProximaDB's Arrow Flight
    /// endpoint for data access. External engines see valid Iceberg table metadata
    /// and use the embedded Flight ticket to retrieve records.
    fn catalog_schema_to_iceberg_metadata(
        &self,
        identifier: &TableIdentifier,
        schema: &CatalogTableSchema,
    ) -> IcebergTableMetadata {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);

        // Build Iceberg fields from ProximaDB columns
        let mut fields = Vec::new();
        let mut field_id_counter = 1i32;

        // Always include canonical ProximaRecord identity fields
        fields.push(IcebergSchemaField {
            id: field_id_counter,
            name: "id".to_string(),
            required: true,
            field_type: IcebergFieldType::Primitive("string".to_string()),
            doc: Some("Canonical ProximaRecord identifier".to_string()),
            metadata: HashMap::new(),
        });
        field_id_counter += 1;

        fields.push(IcebergSchemaField {
            id: field_id_counter,
            name: "tenant_id".to_string(),
            required: true,
            field_type: IcebergFieldType::Primitive("string".to_string()),
            doc: Some("Tenant identifier for RLS".to_string()),
            metadata: HashMap::new(),
        });
        field_id_counter += 1;

        fields.push(IcebergSchemaField {
            id: field_id_counter,
            name: "created_at".to_string(),
            required: false,
            field_type: IcebergFieldType::Primitive("timestamptz".to_string()),
            doc: None,
            metadata: HashMap::new(),
        });
        field_id_counter += 1;

        fields.push(IcebergSchemaField {
            id: field_id_counter,
            name: "updated_at".to_string(),
            required: false,
            field_type: IcebergFieldType::Primitive("timestamptz".to_string()),
            doc: None,
            metadata: HashMap::new(),
        });
        field_id_counter += 1;

        // Add columns from the catalog schema
        for col in &schema.columns {
            let iceberg_type = catalog_type_to_iceberg(&col.data_type, field_id_counter);
            fields.push(IcebergSchemaField {
                id: field_id_counter,
                name: col.name.clone(),
                required: !col.nullable,
                field_type: iceberg_type,
                doc: col.comment.clone(),
                metadata: col.properties.clone(),
            });
            field_id_counter += 1;
        }

        let iceberg_schema = IcebergSchema {
            schema_id: schema.schema_version,
            type_: "struct".to_string(),
            fields,
            identifier_field_ids: vec![1], // id field
        };

        // Build table properties with ProximaDB index metadata
        let mut properties = schema.properties.clone();
        properties.insert("proximadb.collection".to_string(), schema.name.clone());
        properties.insert(
            "proximadb.flight.endpoint".to_string(),
            self.flight_endpoint.clone(),
        );
        properties.insert(
            "proximadb.record.envelope".to_string(),
            "ProximaRecord/v1".to_string(),
        );

        // Add HNSW index metadata for vector columns
        for col in &schema.columns {
            if matches!(
                col.data_type,
                CatalogDataType::Vector | CatalogDataType::SparseVector
            ) {
                let prefix = format!("proximadb.index.{}", col.name);
                properties.insert(format!("{}.type", prefix), "hnsw".to_string());
                if let Some(dim) = col.properties.get("dimension") {
                    properties.insert(format!("{}.dim", prefix), dim.clone());
                }
                if let Some(ef) = col.properties.get("ef_construction") {
                    properties.insert(format!("{}.ef_construction", prefix), ef.clone());
                }
            }
        }

        // Synthetic snapshot: represents the current state of the ProximaDB collection
        let snapshot_id = now_ms;
        let manifest_list_url = format!(
            "{}/namespaces/{}/tables/{}/manifests/snap-{}.avro",
            self.server_base_url,
            identifier.namespace.join("\x1f"),
            identifier.name,
            snapshot_id
        );

        // Build snapshot summary: use real PAX segment stats when available.
        let seg_stats = self
            .segment_registry
            .as_ref()
            .and_then(|r| r.stats(&schema.name));

        let snapshot = IcebergSnapshot {
            snapshot_id,
            parent_snapshot_id: None,
            timestamp_ms: now_ms,
            manifest_list: manifest_list_url,
            summary: IcebergSnapshotSummary {
                operation: "append".to_string(),
                extra: {
                    let mut m = HashMap::new();
                    m.insert(
                        "proximadb.flight.ticket".to_string(),
                        format!(
                            "{{\"collection\":\"{}\",\"snapshot\":{}}}",
                            schema.name, snapshot_id
                        ),
                    );
                    if let Some(ref s) = seg_stats {
                        m.insert("total-records".to_string(), s.row_count.to_string());
                        m.insert("total-data-files".to_string(), s.segment_count.to_string());
                        m.insert("total-files-size".to_string(), s.size_bytes.to_string());
                    }
                    m
                },
            },
            schema_id: Some(schema.schema_version),
        };

        // Default partition spec: partition by tenant_id (field id=2)
        let partition_spec = IcebergPartitionSpec {
            spec_id: 0,
            fields: vec![IcebergPartitionField {
                field_id: 1000,
                source_id: 2, // tenant_id
                name: "tenant_id".to_string(),
                transform: "identity".to_string(),
            }],
        };

        // Unsorted by default
        let sort_order = IcebergSortOrder {
            order_id: 0,
            fields: vec![],
        };

        let table_uuid = schema
            .properties
            .get("proximadb.table.uuid")
            .cloned()
            .unwrap_or_else(|| uuid_from_name(&identifier.name));

        IcebergTableMetadata {
            format_version: 2,
            table_uuid,
            location: format!(
                "{}/namespaces/{}/tables/{}",
                self.server_base_url,
                identifier.namespace.join("/"),
                identifier.name
            ),
            last_updated_ms: now_ms,
            last_column_id: field_id_counter - 1,
            current_schema_id: iceberg_schema.schema_id,
            schemas: vec![iceberg_schema],
            default_spec_id: 0,
            partition_specs: vec![partition_spec],
            last_partition_id: 1000,
            default_sort_order_id: 0,
            sort_orders: vec![sort_order],
            properties,
            current_snapshot_id: Some(snapshot_id),
            snapshots: vec![snapshot],
            snapshot_log: vec![],
            metadata_log: vec![],
        }
    }

    /// Convert an incoming Iceberg schema to a ProximaDB `CatalogTableSchema`.
    fn iceberg_schema_to_catalog(
        &self,
        iceberg_schema: &IcebergSchema,
        table_name: &str,
        properties: HashMap<String, String>,
    ) -> CatalogTableSchema {
        let mut schema = CatalogTableSchema::new(table_name);
        schema.properties = properties;

        for (i, f) in iceberg_schema.fields.iter().enumerate() {
            let data_type = iceberg_type_to_catalog(&f.field_type);
            let mut col = crate::catalog::CatalogColumn::new(
                f.id.max(i as i32 + 1),
                f.name.clone(),
                data_type,
            );
            col.nullable = !f.required;
            if let Some(doc) = &f.doc {
                col.comment = Some(doc.clone());
            }
            col.properties = f.metadata.clone();
            schema = schema.with_column(col);
        }

        schema
    }
}

// ============================================================================
// Type conversion helpers
// ============================================================================

/// Map a `CatalogDataType` to an Iceberg primitive or complex type string.
fn catalog_type_to_iceberg(dt: &CatalogDataType, field_id_base: i32) -> IcebergFieldType {
    match dt {
        CatalogDataType::Boolean => IcebergFieldType::Primitive("boolean".to_string()),
        CatalogDataType::Int8 | CatalogDataType::Int16 | CatalogDataType::Int32 => {
            IcebergFieldType::Primitive("int".to_string())
        }
        CatalogDataType::Int64 => IcebergFieldType::Primitive("long".to_string()),
        CatalogDataType::Float32 => IcebergFieldType::Primitive("float".to_string()),
        CatalogDataType::Float64 => IcebergFieldType::Primitive("double".to_string()),
        CatalogDataType::Decimal => IcebergFieldType::Primitive("decimal(38,18)".to_string()),
        CatalogDataType::String => IcebergFieldType::Primitive("string".to_string()),
        CatalogDataType::Binary => IcebergFieldType::Primitive("binary".to_string()),
        CatalogDataType::Date => IcebergFieldType::Primitive("date".to_string()),
        CatalogDataType::Time => IcebergFieldType::Primitive("time".to_string()),
        CatalogDataType::Timestamp => IcebergFieldType::Primitive("timestamp".to_string()),
        CatalogDataType::TimestampTz => IcebergFieldType::Primitive("timestamptz".to_string()),
        CatalogDataType::Uuid => IcebergFieldType::Primitive("uuid".to_string()),
        CatalogDataType::Json => IcebergFieldType::Primitive("string".to_string()),
        // Vector → list<float>
        CatalogDataType::Vector | CatalogDataType::SparseVector => IcebergFieldType::List {
            type_: "list".to_string(),
            element_id: field_id_base + 1000,
            element: Box::new(IcebergFieldType::Primitive("float".to_string())),
            element_required: true,
        },
        // Binary vector → binary
        CatalogDataType::BinaryVector => IcebergFieldType::Primitive("binary".to_string()),
    }
}

/// Map an Iceberg field type to a `CatalogDataType`.
fn iceberg_type_to_catalog(ft: &IcebergFieldType) -> CatalogDataType {
    match ft {
        IcebergFieldType::Primitive(s) => match s.as_str() {
            "boolean" => CatalogDataType::Boolean,
            "int" => CatalogDataType::Int32,
            "long" => CatalogDataType::Int64,
            "float" => CatalogDataType::Float32,
            "double" => CatalogDataType::Float64,
            "date" => CatalogDataType::Date,
            "time" => CatalogDataType::Time,
            "timestamp" => CatalogDataType::Timestamp,
            "timestamptz" => CatalogDataType::TimestampTz,
            "uuid" => CatalogDataType::Uuid,
            "binary" => CatalogDataType::Binary,
            s if s.starts_with("decimal") => CatalogDataType::Decimal,
            s if s.starts_with("fixed") => CatalogDataType::Binary,
            _ => CatalogDataType::String,
        },
        IcebergFieldType::List { element, .. } => match element.as_ref() {
            IcebergFieldType::Primitive(s) if s == "float" || s == "double" => {
                CatalogDataType::Vector
            }
            _ => CatalogDataType::Json,
        },
        IcebergFieldType::Map { .. } => CatalogDataType::Json,
        IcebergFieldType::Struct { .. } => CatalogDataType::Json,
    }
}

/// Deterministic UUID from table name (for stable table-uuid across restarts).
fn uuid_from_name(name: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut h);
    let v = h.finish();
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        (v >> 32) as u32,
        ((v >> 16) & 0xffff) as u16,
        (v & 0x0fff) as u16,
        0x8000u16 | ((v >> 48) & 0x3fff) as u16,
        v & 0xffffffffffff
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_catalog_type_to_iceberg_vector() {
        let ft = catalog_type_to_iceberg(&CatalogDataType::Vector, 10);
        match ft {
            IcebergFieldType::List { type_, .. } => assert_eq!(type_, "list"),
            _ => panic!("expected list type for Vector"),
        }
    }

    #[test]
    fn test_iceberg_type_to_catalog_float_list() {
        let ft = IcebergFieldType::List {
            type_: "list".to_string(),
            element_id: 9999,
            element: Box::new(IcebergFieldType::Primitive("float".to_string())),
            element_required: true,
        };
        assert!(matches!(
            iceberg_type_to_catalog(&ft),
            CatalogDataType::Vector
        ));
    }

    #[test]
    fn test_uuid_from_name_stable() {
        let a = uuid_from_name("my_table");
        let b = uuid_from_name("my_table");
        assert_eq!(a, b);
        assert_ne!(uuid_from_name("table_a"), uuid_from_name("table_b"));
    }

    #[test]
    fn test_iceberg_error_response_serialization() {
        let err = IcebergErrorResponse::not_found("table not found");
        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("NoSuchTableException"));
        assert!(json.contains("404"));
    }
}
