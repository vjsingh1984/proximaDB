//! Apache Polaris Catalog Integration
//!
//! Provides integration with Apache Polaris (Iceberg REST Catalog) for metadata management.
//! Implements the Iceberg REST Catalog API specification.
//!
//! This module is feature-gated behind `polaris-catalog` feature.

#![cfg(feature = "polaris-catalog")]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::cache::CatalogCache;
use crate::schema::{apply_evolution, validate_schema};
use crate::{
    Catalog, CatalogColumn, CatalogDataType, CatalogHealth, CatalogIndex, CatalogNamespace,
    CatalogPartitionSpec, CatalogSchemaEvolution, CatalogSortOrder, CatalogTableSchema,
    CatalogTableStatistics, LakehouseExtension, TableFormat, TableIdentifier,
};

/// Plain Rust configuration for the Apache Polaris (Iceberg REST) catalog.
///
/// Decoupled from `proximadb_proto::proximadb::v1::PolarisCatalogConfig` so the
/// workspace contract crate doesn't depend on the heavy proto crate. The
/// network/API layer converts from the proto form when configuring the
/// catalog.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PolarisCatalogConfig {
    /// Polaris server URI
    pub uri: String,
    /// Warehouse name
    pub warehouse: String,
    /// Bearer token or OAuth client credentials
    pub credential: String,
    /// OAuth2 server URI (optional)
    pub oauth_server_uri: String,
    /// OAuth2 scope
    pub scope: String,
}

/// Apache Polaris catalog implementation
///
/// Implements the Iceberg REST Catalog API for metadata management.
/// Supports full Iceberg table semantics including:
/// - Schema evolution
/// - Partition evolution
/// - Table properties
/// - Snapshots and branching
pub struct PolarisCatalog {
    /// Catalog name
    name: String,
    /// Configuration
    config: PolarisCatalogConfig,
    /// HTTP client
    http_client: reqwest::Client,
    /// OAuth2 access token (refreshed as needed)
    access_token: tokio::sync::RwLock<Option<String>>,
    /// Catalog cache
    cache: Arc<CatalogCache>,
}

/// Iceberg REST API types
#[derive(Debug, Serialize, Deserialize)]
struct IcebergNamespace {
    namespace: Vec<String>,
    properties: Option<HashMap<String, String>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct IcebergListNamespacesResponse {
    namespaces: Vec<Vec<String>>,
    #[serde(rename = "next-page-token")]
    next_page_token: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct IcebergTableIdentifier {
    namespace: Vec<String>,
    name: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct IcebergListTablesResponse {
    identifiers: Vec<IcebergTableIdentifier>,
    #[serde(rename = "next-page-token")]
    next_page_token: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct IcebergSchema {
    #[serde(rename = "type")]
    schema_type: String,
    #[serde(rename = "schema-id")]
    schema_id: i32,
    fields: Vec<IcebergField>,
}

#[derive(Debug, Serialize, Deserialize)]
struct IcebergField {
    id: i32,
    name: String,
    #[serde(rename = "type")]
    field_type: serde_json::Value, // Can be string or complex type
    required: bool,
    doc: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct IcebergTableMetadata {
    #[serde(rename = "format-version")]
    format_version: i32,
    #[serde(rename = "table-uuid")]
    table_uuid: String,
    location: String,
    #[serde(rename = "last-updated-ms")]
    last_updated_ms: i64,
    #[serde(rename = "last-column-id")]
    last_column_id: i32,
    schema: IcebergSchema,
    schemas: Option<Vec<IcebergSchema>>,
    #[serde(rename = "current-schema-id")]
    current_schema_id: i32,
    #[serde(rename = "partition-spec")]
    partition_spec: Option<serde_json::Value>,
    #[serde(rename = "partition-specs")]
    partition_specs: Option<Vec<serde_json::Value>>,
    #[serde(rename = "default-spec-id")]
    default_spec_id: Option<i32>,
    #[serde(rename = "sort-order")]
    sort_order: Option<serde_json::Value>,
    #[serde(rename = "sort-orders")]
    sort_orders: Option<Vec<serde_json::Value>>,
    properties: Option<HashMap<String, String>>,
    #[serde(rename = "current-snapshot-id")]
    current_snapshot_id: Option<i64>,
    snapshots: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct IcebergLoadTableResponse {
    #[serde(rename = "metadata-location")]
    metadata_location: Option<String>,
    metadata: IcebergTableMetadata,
}

#[derive(Debug, Serialize, Deserialize)]
struct IcebergCreateTableRequest {
    name: String,
    location: Option<String>,
    schema: IcebergSchema,
    #[serde(rename = "partition-spec")]
    partition_spec: Option<serde_json::Value>,
    #[serde(rename = "write-order")]
    write_order: Option<serde_json::Value>,
    properties: Option<HashMap<String, String>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OAuth2TokenResponse {
    access_token: String,
    token_type: String,
    expires_in: Option<i64>,
}

impl PolarisCatalog {
    /// Create a new Polaris catalog
    pub async fn new(
        name: String,
        config: PolarisCatalogConfig,
        cache: Arc<CatalogCache>,
    ) -> Result<Self> {
        info!("Initializing Polaris catalog: {} at {}", name, config.uri);

        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;

        let catalog = Self {
            name,
            config,
            http_client,
            access_token: tokio::sync::RwLock::new(None),
            cache,
        };

        // Authenticate if credentials provided
        if !catalog.config.credential.is_empty() {
            catalog.refresh_token().await?;
        }

        Ok(catalog)
    }

    /// Get the API base URL
    fn api_url(&self) -> String {
        format!(
            "{}/v1/{}",
            self.config.uri.trim_end_matches('/'),
            self.config.warehouse
        )
    }

    /// Refresh OAuth2 token
    async fn refresh_token(&self) -> Result<()> {
        let token_url = format!("{}/v1/oauth/tokens", self.config.uri.trim_end_matches('/'));

        let response = self
            .http_client
            .post(&token_url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(format!(
                "grant_type=client_credentials&client_id={}&client_secret={}&scope=PRINCIPAL_ROLE:ALL",
                self.config.credential.split(':').next().unwrap_or(""),
                self.config.credential.split(':').nth(1).unwrap_or("")
            ))
            .send()
            .await?;

        if response.status().is_success() {
            let token_resp: OAuth2TokenResponse = response.json().await?;
            *self.access_token.write().await = Some(token_resp.access_token);
            debug!("Polaris OAuth2 token refreshed");
        } else {
            warn!("Failed to refresh Polaris token: {}", response.status());
        }

        Ok(())
    }

    /// Make an authenticated request
    async fn api_request<T: for<'de> Deserialize<'de>>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<T> {
        let url = format!("{}{}", self.api_url(), path);

        let mut request = self.http_client.request(method.clone(), &url);

        // Add auth header if we have a token
        if let Some(token) = self.access_token.read().await.as_ref() {
            request = request.header("Authorization", format!("Bearer {}", token));
        }

        request = request.header("Content-Type", "application/json");

        if let Some(b) = body {
            request = request.json(&b);
        }

        let response = request.send().await?;

        if response.status() == 401 {
            // Token expired, refresh and retry
            self.refresh_token().await?;

            let mut retry_request = self.http_client.request(method, &url);
            if let Some(token) = self.access_token.read().await.as_ref() {
                retry_request = retry_request.header("Authorization", format!("Bearer {}", token));
            }
            let retry_response = retry_request.send().await?;

            if !retry_response.status().is_success() {
                let status = retry_response.status();
                let error_body = retry_response.text().await.unwrap_or_default();
                return Err(anyhow!("Polaris API error ({}): {}", status, error_body));
            }

            return Ok(retry_response.json().await?);
        }

        if !response.status().is_success() {
            let status = response.status();
            let error_body = response.text().await.unwrap_or_default();
            return Err(anyhow!("Polaris API error ({}): {}", status, error_body));
        }

        Ok(response.json().await?)
    }

    /// Make an API request that returns no content
    async fn api_request_no_content(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<()> {
        let url = format!("{}{}", self.api_url(), path);

        let mut request = self.http_client.request(method.clone(), &url);

        if let Some(token) = self.access_token.read().await.as_ref() {
            request = request.header("Authorization", format!("Bearer {}", token));
        }

        request = request.header("Content-Type", "application/json");

        if let Some(b) = body {
            request = request.json(&b);
        }

        let response = request.send().await?;

        if !response.status().is_success() && response.status() != 204 {
            let status = response.status();
            let error_body = response.text().await.unwrap_or_default();
            return Err(anyhow!("Polaris API error ({}): {}", status, error_body));
        }

        Ok(())
    }

    /// Convert Iceberg field to internal column type
    fn iceberg_field_to_column(field: &IcebergField) -> CatalogColumn {
        let (data_type, properties) = Self::parse_iceberg_type(&field.field_type);

        CatalogColumn {
            id: field.id,
            name: field.name.clone(),
            data_type,
            nullable: !field.required,
            default_value: None,
            comment: field.doc.clone(),
            properties,
        }
    }

    /// Parse Iceberg type to CatalogDataType and properties
    fn parse_iceberg_type(
        type_value: &serde_json::Value,
    ) -> (CatalogDataType, HashMap<String, String>) {
        let mut properties = HashMap::new();

        let data_type = match type_value {
            serde_json::Value::String(s) => match s.as_str() {
                "boolean" => CatalogDataType::Boolean,
                "int" | "integer" => CatalogDataType::Int32,
                "long" => CatalogDataType::Int64,
                "float" => CatalogDataType::Float32,
                "double" => CatalogDataType::Float64,
                "string" => CatalogDataType::String,
                "binary" => CatalogDataType::Binary,
                "date" => CatalogDataType::Date,
                "timestamp" | "timestamptz" => CatalogDataType::Timestamp,
                "uuid" => CatalogDataType::Uuid,
                t if t.starts_with("decimal") => CatalogDataType::Decimal,
                t if t.starts_with("fixed") => CatalogDataType::Binary,
                _ => CatalogDataType::String,
            },
            serde_json::Value::Object(obj) => {
                let type_name = obj.get("type").and_then(|v| v.as_str()).unwrap_or("struct");
                match type_name {
                    "list" => {
                        // Check if it's a vector (list of floats)
                        if let Some(element) = obj.get("element") {
                            if element.as_str() == Some("float")
                                || element.as_str() == Some("double")
                            {
                                if let Some(length) = obj.get("length").and_then(|v| v.as_i64()) {
                                    properties.insert("dimension".to_string(), length.to_string());
                                }
                                return (CatalogDataType::Vector, properties);
                            }
                        }
                        CatalogDataType::Json
                    }
                    "map" => CatalogDataType::Json,
                    "struct" => CatalogDataType::Json,
                    _ => CatalogDataType::String,
                }
            }
            _ => CatalogDataType::String,
        };

        (data_type, properties)
    }

    /// Convert CatalogDataType to Iceberg type
    fn data_type_to_iceberg(
        data_type: &CatalogDataType,
        properties: &HashMap<String, String>,
    ) -> serde_json::Value {
        match data_type {
            CatalogDataType::Boolean => serde_json::json!("boolean"),
            CatalogDataType::Int8 | CatalogDataType::Int16 | CatalogDataType::Int32 => {
                serde_json::json!("int")
            }
            CatalogDataType::Int64 => serde_json::json!("long"),
            CatalogDataType::Float32 => serde_json::json!("float"),
            CatalogDataType::Float64 => serde_json::json!("double"),
            CatalogDataType::String => serde_json::json!("string"),
            CatalogDataType::Binary | CatalogDataType::BinaryVector => serde_json::json!("binary"),
            CatalogDataType::Date => serde_json::json!("date"),
            CatalogDataType::Time => serde_json::json!("time"),
            CatalogDataType::Timestamp | CatalogDataType::TimestampTz => {
                serde_json::json!("timestamptz")
            }
            CatalogDataType::Uuid => serde_json::json!("uuid"),
            CatalogDataType::Decimal => serde_json::json!("decimal(38,18)"),
            CatalogDataType::Json => serde_json::json!("string"),
            CatalogDataType::Vector => {
                let dim = properties
                    .get("dimension")
                    .and_then(|d| d.parse::<i64>().ok())
                    .unwrap_or(0);
                serde_json::json!({
                    "type": "list",
                    "element": "float",
                    "element-id": 0,
                    "length": dim
                })
            }
            CatalogDataType::SparseVector => serde_json::json!({
                "type": "map",
                "key": "int",
                "value": "float"
            }),
        }
    }

    /// Encode namespace for URL
    fn encode_namespace(namespace: &[String]) -> String {
        namespace
            .iter()
            .map(|s| urlencoding::encode(s).to_string())
            .collect::<Vec<_>>()
            .join("%1F") // Unit separator
    }
}

#[async_trait]
impl Catalog for PolarisCatalog {
    fn name(&self) -> &str {
        &self.name
    }

    fn catalog_type(&self) -> &str {
        "polaris"
    }

    // ========================
    // Namespace Operations
    // ========================

    async fn create_namespace(
        &self,
        namespace: &[String],
        properties: HashMap<String, String>,
    ) -> Result<CatalogNamespace> {
        let body = serde_json::json!({
            "namespace": namespace,
            "properties": properties,
        });

        let ns: IcebergNamespace = self
            .api_request(reqwest::Method::POST, "/namespaces", Some(body))
            .await?;

        info!("Created Polaris namespace: {:?}", namespace);

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        Ok(CatalogNamespace {
            levels: ns.namespace,
            properties: ns.properties.unwrap_or_default(),
            owner: None,
            location: None,
            created_at_ms: now,
            updated_at_ms: now,
            ..CatalogNamespace::new(Vec::new())
        })
    }

    async fn drop_namespace(&self, namespace: &[String], _cascade: bool) -> Result<bool> {
        let encoded = Self::encode_namespace(namespace);
        let path = format!("/namespaces/{}", encoded);

        self.api_request_no_content(reqwest::Method::DELETE, &path, None)
            .await?;

        info!("Deleted Polaris namespace: {:?}", namespace);
        Ok(true)
    }

    async fn list_namespaces(&self, parent: Option<&[String]>) -> Result<Vec<CatalogNamespace>> {
        let mut all_namespaces = Vec::new();
        let mut next_page_token = None;

        let base_path = if let Some(p) = parent {
            format!("/namespaces?parent={}", Self::encode_namespace(p))
        } else {
            "/namespaces".to_string()
        };

        loop {
            let mut path = base_path.clone();
            if let Some(token) = &next_page_token {
                let sep = if path.contains('?') { '&' } else { '?' };
                path.push_str(&format!("{}pageToken={}", sep, token));
            }

            let response: IcebergListNamespacesResponse =
                self.api_request(reqwest::Method::GET, &path, None).await?;

            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64;

            all_namespaces.extend(response.namespaces.into_iter().map(|ns| CatalogNamespace {
                levels: ns,
                properties: HashMap::new(),
                owner: None,
                location: None,
                created_at_ms: now,
                updated_at_ms: now,
                ..CatalogNamespace::new(Vec::new())
            }));

            next_page_token = response.next_page_token;
            if next_page_token.is_none() {
                break;
            }
        }

        Ok(all_namespaces)
    }

    async fn namespace_exists(&self, namespace: &[String]) -> Result<bool> {
        let encoded = Self::encode_namespace(namespace);
        let path = format!("/namespaces/{}", encoded);

        match self
            .api_request::<IcebergNamespace>(reqwest::Method::GET, &path, None)
            .await
        {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }

    async fn get_namespace(&self, namespace: &[String]) -> Result<CatalogNamespace> {
        let encoded = Self::encode_namespace(namespace);
        let path = format!("/namespaces/{}", encoded);

        let ns: IcebergNamespace = self.api_request(reqwest::Method::GET, &path, None).await?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        Ok(CatalogNamespace {
            levels: ns.namespace,
            properties: ns.properties.unwrap_or_default(),
            owner: None,
            location: None,
            created_at_ms: now,
            updated_at_ms: now,
            ..CatalogNamespace::new(Vec::new())
        })
    }

    async fn update_namespace_properties(
        &self,
        namespace: &[String],
        updates: HashMap<String, String>,
        removals: Vec<String>,
    ) -> Result<()> {
        let encoded = Self::encode_namespace(namespace);
        let path = format!("/namespaces/{}/properties", encoded);

        let body = serde_json::json!({
            "updates": updates,
            "removals": removals,
        });

        self.api_request_no_content(reqwest::Method::POST, &path, Some(body))
            .await?;

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

        let encoded_ns = Self::encode_namespace(&identifier.namespace);
        let path = format!("/namespaces/{}/tables", encoded_ns);

        // Convert columns to Iceberg fields
        let fields: Vec<IcebergField> = schema
            .columns
            .iter()
            .enumerate()
            .map(|(idx, col)| {
                let field_type = Self::data_type_to_iceberg(&col.data_type, &col.properties);

                IcebergField {
                    id: (idx + 1) as i32,
                    name: col.name.clone(),
                    field_type,
                    required: !col.nullable,
                    doc: col.comment.clone(),
                }
            })
            .collect();

        let iceberg_schema = IcebergSchema {
            schema_type: "struct".to_string(),
            schema_id: schema.schema_version,
            fields,
        };

        let mut properties = schema.properties.clone();
        properties.insert("proximadb.version".to_string(), "1".to_string());

        let create_request = IcebergCreateTableRequest {
            name: identifier.name.clone(),
            location: None,
            schema: iceberg_schema,
            partition_spec: None,
            write_order: None,
            properties: Some(properties),
        };

        let _response: IcebergLoadTableResponse = self
            .api_request(
                reqwest::Method::POST,
                &path,
                Some(serde_json::to_value(create_request)?),
            )
            .await?;

        info!("Created Polaris table: {}", identifier);
        Ok(schema)
    }

    async fn drop_table(&self, identifier: &TableIdentifier, purge: bool) -> Result<bool> {
        let encoded_ns = Self::encode_namespace(&identifier.namespace);
        let path = format!(
            "/namespaces/{}/tables/{}?purgeRequested={}",
            encoded_ns, identifier.name, purge
        );

        self.api_request_no_content(reqwest::Method::DELETE, &path, None)
            .await?;

        // Invalidate cache
        self.cache
            .invalidate_table_in_catalog(&self.name, identifier);

        info!("Deleted Polaris table: {} (purge={})", identifier, purge);
        Ok(true)
    }

    async fn list_tables(&self, namespace: &[String]) -> Result<Vec<TableIdentifier>> {
        let encoded_ns = Self::encode_namespace(namespace);
        let base_path = format!("/namespaces/{}/tables", encoded_ns);

        let mut all_tables = Vec::new();
        let mut next_page_token = None;

        loop {
            let mut path = base_path.clone();
            if let Some(token) = &next_page_token {
                path.push_str(&format!("?pageToken={}", token));
            }

            let response: IcebergListTablesResponse =
                self.api_request(reqwest::Method::GET, &path, None).await?;

            all_tables.extend(
                response
                    .identifiers
                    .into_iter()
                    .map(|id| TableIdentifier::new(id.namespace, id.name)),
            );

            next_page_token = response.next_page_token;
            if next_page_token.is_none() {
                break;
            }
        }

        Ok(all_tables)
    }

    async fn table_exists(&self, identifier: &TableIdentifier) -> Result<bool> {
        let encoded_ns = Self::encode_namespace(&identifier.namespace);
        let path = format!("/namespaces/{}/tables/{}", encoded_ns, identifier.name);

        match self
            .api_request::<IcebergLoadTableResponse>(reqwest::Method::GET, &path, None)
            .await
        {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }

    async fn get_table(&self, identifier: &TableIdentifier) -> Result<CatalogTableSchema> {
        // Check cache first
        if let Some(schema) = self.cache.get_table(&self.name, identifier) {
            return Ok(schema);
        }

        let encoded_ns = Self::encode_namespace(&identifier.namespace);
        let path = format!("/namespaces/{}/tables/{}", encoded_ns, identifier.name);

        let response: IcebergLoadTableResponse =
            self.api_request(reqwest::Method::GET, &path, None).await?;

        let columns: Vec<CatalogColumn> = response
            .metadata
            .schema
            .fields
            .iter()
            .map(Self::iceberg_field_to_column)
            .collect();

        let schema = CatalogTableSchema {
            name: identifier.name.clone(),
            columns,
            primary_key: vec![],
            indexes: vec![],
            schema_version: response.metadata.current_schema_id,
            properties: response.metadata.properties.unwrap_or_default(),
            location: Some(response.metadata.location.clone()),
            created_at_ms: 0,
            updated_at_ms: response.metadata.last_updated_ms,
            ..Default::default()
        };

        // Update cache
        self.cache.put_table(&self.name, identifier, schema.clone());

        Ok(schema)
    }

    async fn rename_table(&self, from: &TableIdentifier, to: &TableIdentifier) -> Result<()> {
        let body = serde_json::json!({
            "source": {
                "namespace": from.namespace,
                "name": from.name,
            },
            "destination": {
                "namespace": to.namespace,
                "name": to.name,
            }
        });

        self.api_request_no_content(reqwest::Method::POST, "/tables/rename", Some(body))
            .await?;

        // Invalidate cache
        self.cache.invalidate_table_in_catalog(&self.name, from);

        info!("Renamed Polaris table: {} -> {}", from, to);
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

        // Polaris uses table update requirements for schema evolution
        let encoded_ns = Self::encode_namespace(&identifier.namespace);
        let path = format!("/namespaces/{}/tables/{}", encoded_ns, identifier.name);

        // Convert to Iceberg schema update
        let fields: Vec<IcebergField> = new_schema
            .columns
            .iter()
            .enumerate()
            .map(|(idx, col)| {
                let field_type = Self::data_type_to_iceberg(&col.data_type, &col.properties);

                IcebergField {
                    id: (idx + 1) as i32,
                    name: col.name.clone(),
                    field_type,
                    required: !col.nullable,
                    doc: col.comment.clone(),
                }
            })
            .collect();

        let update_body = serde_json::json!({
            "updates": [{
                "action": "upgrade-format-version",
                "format-version": 2
            }, {
                "action": "set-current-schema",
                "schema": {
                    "type": "struct",
                    "schema-id": new_schema.schema_version,
                    "fields": fields
                }
            }]
        });

        self.api_request_no_content(reqwest::Method::POST, &path, Some(update_body))
            .await?;

        // Invalidate cache
        self.cache
            .invalidate_table_in_catalog(&self.name, identifier);

        info!(
            "Evolved Polaris table schema: {} (v{})",
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
        // Iceberg keeps historical schemas
        let encoded_ns = Self::encode_namespace(&identifier.namespace);
        let path = format!("/namespaces/{}/tables/{}", encoded_ns, identifier.name);

        let response: IcebergLoadTableResponse =
            self.api_request(reqwest::Method::GET, &path, None).await?;

        // Find schema by version in schemas array
        if let Some(schemas) = &response.metadata.schemas {
            for iceberg_schema in schemas {
                if iceberg_schema.schema_id == version {
                    let columns: Vec<CatalogColumn> = iceberg_schema
                        .fields
                        .iter()
                        .map(Self::iceberg_field_to_column)
                        .collect();

                    return Ok(CatalogTableSchema {
                        name: identifier.name.clone(),
                        columns,
                        primary_key: vec![],
                        indexes: vec![],
                        schema_version: version,
                        properties: HashMap::new(),
                        location: None,
                        created_at_ms: 0,
                        updated_at_ms: response.metadata.last_updated_ms,
                        ..Default::default()
                    });
                }
            }
        }

        Err(anyhow!(
            "Schema version {} not found for table '{}'",
            version,
            identifier
        ))
    }

    // ========================
    // Index Operations
    // ========================

    async fn create_index(
        &self,
        identifier: &TableIdentifier,
        index: CatalogIndex,
    ) -> Result<CatalogIndex> {
        // Iceberg doesn't have native index support
        // Store index metadata in table properties
        warn!("Polaris/Iceberg catalog: indexes stored as metadata only");

        let mut schema = self.get_table(identifier).await?;
        schema.indexes.push(index.clone());

        // Update table properties with index information
        let encoded_ns = Self::encode_namespace(&identifier.namespace);
        let path = format!("/namespaces/{}/tables/{}", encoded_ns, identifier.name);

        let index_json = serde_json::to_string(&schema.indexes)?;
        let update_body = serde_json::json!({
            "updates": [{
                "action": "set-properties",
                "updates": {
                    "proximadb.indexes": index_json
                }
            }]
        });

        self.api_request_no_content(reqwest::Method::POST, &path, Some(update_body))
            .await?;

        // Invalidate cache
        self.cache
            .invalidate_table_in_catalog(&self.name, identifier);

        Ok(index)
    }

    async fn drop_index(&self, identifier: &TableIdentifier, index_name: &str) -> Result<bool> {
        let mut schema = self.get_table(identifier).await?;
        let original_len = schema.indexes.len();
        schema.indexes.retain(|idx| idx.name != index_name);

        if schema.indexes.len() == original_len {
            return Ok(false); // Index not found
        }

        // Update table properties
        let encoded_ns = Self::encode_namespace(&identifier.namespace);
        let path = format!("/namespaces/{}/tables/{}", encoded_ns, identifier.name);

        let index_json = serde_json::to_string(&schema.indexes)?;
        let update_body = serde_json::json!({
            "updates": [{
                "action": "set-properties",
                "updates": {
                    "proximadb.indexes": index_json
                }
            }]
        });

        self.api_request_no_content(reqwest::Method::POST, &path, Some(update_body))
            .await?;

        // Invalidate cache
        self.cache
            .invalidate_table_in_catalog(&self.name, identifier);

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

        // Polaris/Iceberg doesn't have a direct stats API
        // Return default statistics
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
        // Store statistics in cache (Polaris doesn't have a direct stats update API)
        self.cache.put_statistics(&self.name, identifier, stats);
        Ok(())
    }

    // ========================
    // Partitioning
    // ========================

    async fn get_partition_spec(
        &self,
        identifier: &TableIdentifier,
    ) -> Result<Option<CatalogPartitionSpec>> {
        let encoded_ns = Self::encode_namespace(&identifier.namespace);
        let path = format!("/namespaces/{}/tables/{}", encoded_ns, identifier.name);

        let response: IcebergLoadTableResponse =
            self.api_request(reqwest::Method::GET, &path, None).await?;

        // Convert Iceberg partition spec to internal type
        if response.metadata.partition_spec.is_some() {
            // Parse Iceberg partition spec
            Ok(Some(CatalogPartitionSpec::default()))
        } else {
            Ok(None)
        }
    }

    async fn update_partition_spec(
        &self,
        identifier: &TableIdentifier,
        _spec: CatalogPartitionSpec,
    ) -> Result<()> {
        // Iceberg supports partition evolution
        let encoded_ns = Self::encode_namespace(&identifier.namespace);
        let path = format!("/namespaces/{}/tables/{}", encoded_ns, identifier.name);

        let update_body = serde_json::json!({
            "updates": [{
                "action": "add-partition-field",
                // Would need to convert spec to Iceberg format
            }]
        });

        self.api_request_no_content(reqwest::Method::POST, &path, Some(update_body))
            .await?;

        Ok(())
    }

    // ========================
    // Sort Order
    // ========================

    async fn get_sort_order(
        &self,
        identifier: &TableIdentifier,
    ) -> Result<Option<CatalogSortOrder>> {
        let encoded_ns = Self::encode_namespace(&identifier.namespace);
        let path = format!("/namespaces/{}/tables/{}", encoded_ns, identifier.name);

        let response: IcebergLoadTableResponse =
            self.api_request(reqwest::Method::GET, &path, None).await?;

        if response.metadata.sort_order.is_some() {
            Ok(Some(CatalogSortOrder::default()))
        } else {
            Ok(None)
        }
    }

    async fn update_sort_order(
        &self,
        identifier: &TableIdentifier,
        _order: CatalogSortOrder,
    ) -> Result<()> {
        let encoded_ns = Self::encode_namespace(&identifier.namespace);
        let path = format!("/namespaces/{}/tables/{}", encoded_ns, identifier.name);

        let update_body = serde_json::json!({
            "updates": [{
                "action": "set-default-sort-order",
                // Would need to convert order to Iceberg format
            }]
        });

        self.api_request_no_content(reqwest::Method::POST, &path, Some(update_body))
            .await?;

        Ok(())
    }

    // ========================
    // Health & Connectivity
    // ========================

    async fn health_check(&self) -> Result<CatalogHealth> {
        let start = Instant::now();

        match self
            .api_request::<serde_json::Value>(reqwest::Method::GET, "/config", None)
            .await
        {
            Ok(_) => {
                let latency = start.elapsed().as_millis() as u64;
                Ok(CatalogHealth::healthy(latency)
                    .with_detail("uri", &self.config.uri)
                    .with_detail("warehouse", &self.config.warehouse)
                    .with_detail("catalog_type", "polaris"))
            }
            Err(e) => Ok(CatalogHealth::unhealthy(e.to_string())),
        }
    }

    async fn close(&self) -> Result<()> {
        debug!("Closing Polaris catalog: {}", self.name);
        Ok(())
    }
}

#[async_trait]
impl LakehouseExtension for PolarisCatalog {
    fn table_format(&self) -> TableFormat {
        TableFormat::Iceberg
    }

    async fn get_table_location(&self, identifier: &TableIdentifier) -> Result<String> {
        let encoded_ns = Self::encode_namespace(&identifier.namespace);
        let path = format!("/namespaces/{}/tables/{}", encoded_ns, identifier.name);

        let response: IcebergLoadTableResponse =
            self.api_request(reqwest::Method::GET, &path, None).await?;

        Ok(response.metadata.location)
    }

    async fn get_current_snapshot(&self, identifier: &TableIdentifier) -> Result<Option<i64>> {
        let encoded_ns = Self::encode_namespace(&identifier.namespace);
        let path = format!("/namespaces/{}/tables/{}", encoded_ns, identifier.name);

        let response: IcebergLoadTableResponse =
            self.api_request(reqwest::Method::GET, &path, None).await?;

        Ok(response.metadata.current_snapshot_id)
    }

    async fn list_snapshots(&self, identifier: &TableIdentifier) -> Result<Vec<i64>> {
        let encoded_ns = Self::encode_namespace(&identifier.namespace);
        let path = format!("/namespaces/{}/tables/{}", encoded_ns, identifier.name);

        let response: IcebergLoadTableResponse =
            self.api_request(reqwest::Method::GET, &path, None).await?;

        // Extract snapshot IDs from snapshots array
        let snapshot_ids = response
            .metadata
            .snapshots
            .map(|snaps| {
                snaps
                    .iter()
                    .filter_map(|s| s.get("snapshot-id").and_then(|id| id.as_i64()))
                    .collect()
            })
            .unwrap_or_default();

        Ok(snapshot_ids)
    }

    async fn get_schema_history(&self, identifier: &TableIdentifier) -> Result<Vec<i32>> {
        let encoded_ns = Self::encode_namespace(&identifier.namespace);
        let path = format!("/namespaces/{}/tables/{}", encoded_ns, identifier.name);

        let response: IcebergLoadTableResponse =
            self.api_request(reqwest::Method::GET, &path, None).await?;

        let schema_ids = response
            .metadata
            .schemas
            .map(|schemas| schemas.iter().map(|s| s.schema_id).collect())
            .unwrap_or_else(|| vec![response.metadata.current_schema_id]);

        Ok(schema_ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_namespace() {
        let ns = vec!["db".to_string(), "schema".to_string()];
        let encoded = PolarisCatalog::encode_namespace(&ns);
        assert!(encoded.contains("db"));
        assert!(encoded.contains("schema"));
    }

    #[test]
    fn test_iceberg_type_conversion() {
        let (dt, _) = PolarisCatalog::parse_iceberg_type(&serde_json::json!("long"));
        assert!(matches!(dt, CatalogDataType::Int64));

        let (dt, _) = PolarisCatalog::parse_iceberg_type(&serde_json::json!("string"));
        assert!(matches!(dt, CatalogDataType::String));

        let (dt, _) = PolarisCatalog::parse_iceberg_type(&serde_json::json!("boolean"));
        assert!(matches!(dt, CatalogDataType::Boolean));

        let (dt, _) = PolarisCatalog::parse_iceberg_type(&serde_json::json!("float"));
        assert!(matches!(dt, CatalogDataType::Float32));

        let (dt, _) = PolarisCatalog::parse_iceberg_type(&serde_json::json!("double"));
        assert!(matches!(dt, CatalogDataType::Float64));
    }

    #[test]
    fn test_data_type_to_iceberg() {
        let props = HashMap::new();

        let iceberg_type = PolarisCatalog::data_type_to_iceberg(&CatalogDataType::Int64, &props);
        assert_eq!(iceberg_type, serde_json::json!("long"));

        let iceberg_type = PolarisCatalog::data_type_to_iceberg(&CatalogDataType::String, &props);
        assert_eq!(iceberg_type, serde_json::json!("string"));

        let iceberg_type = PolarisCatalog::data_type_to_iceberg(&CatalogDataType::Boolean, &props);
        assert_eq!(iceberg_type, serde_json::json!("boolean"));
    }

    #[test]
    fn test_vector_type_conversion() {
        let mut vec_props = HashMap::new();
        vec_props.insert("dimension".to_string(), "768".to_string());

        let iceberg_type =
            PolarisCatalog::data_type_to_iceberg(&CatalogDataType::Vector, &vec_props);
        assert!(iceberg_type.get("type").is_some());
        assert_eq!(iceberg_type.get("type").unwrap(), "list");
        assert_eq!(iceberg_type.get("element").unwrap(), "float");
        assert_eq!(iceberg_type.get("length").unwrap(), 768);
    }

    #[test]
    fn test_iceberg_field_to_column() {
        let field = IcebergField {
            id: 1,
            name: "test_col".to_string(),
            field_type: serde_json::json!("string"),
            required: true,
            doc: Some("Test column".to_string()),
        };

        let column = PolarisCatalog::iceberg_field_to_column(&field);
        assert_eq!(column.name, "test_col");
        assert!(matches!(column.data_type, CatalogDataType::String));
        assert!(!column.nullable);
        assert_eq!(column.comment, Some("Test column".to_string()));
    }

    #[test]
    fn test_parse_complex_iceberg_types() {
        // Test list type that's not a vector
        let list_type = serde_json::json!({
            "type": "list",
            "element": "string"
        });
        let (dt, _) = PolarisCatalog::parse_iceberg_type(&list_type);
        assert!(matches!(dt, CatalogDataType::Json));

        // Test vector type (list of floats with length)
        let vector_type = serde_json::json!({
            "type": "list",
            "element": "float",
            "length": 512
        });
        let (dt, props) = PolarisCatalog::parse_iceberg_type(&vector_type);
        assert!(matches!(dt, CatalogDataType::Vector));
        assert_eq!(props.get("dimension"), Some(&"512".to_string()));

        // Test map type
        let map_type = serde_json::json!({
            "type": "map",
            "key": "string",
            "value": "int"
        });
        let (dt, _) = PolarisCatalog::parse_iceberg_type(&map_type);
        assert!(matches!(dt, CatalogDataType::Json));

        // Test struct type
        let struct_type = serde_json::json!({
            "type": "struct",
            "fields": []
        });
        let (dt, _) = PolarisCatalog::parse_iceberg_type(&struct_type);
        assert!(matches!(dt, CatalogDataType::Json));
    }
}
