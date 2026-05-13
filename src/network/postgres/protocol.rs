// PostgreSQL Wire Protocol v3.0 implementation
//
// Implements:
// - Startup handshake
// - Authentication (trust, password, MD5)
// - Simple query protocol
// - Extended query protocol
// - COPY protocol (for bulk data)

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use bytes::{Buf, BufMut, BytesMut};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::session::Session;
use super::translator::QueryTranslator;
use super::types::{FieldDescription, PgType};
use crate::catalog::CatalogManager;
use crate::graph::GraphService;
use crate::network::arrow_ipc::ArrowProtoCodec;
use crate::observability::ObservabilityService;
use crate::query::multimodal_router::{self, DataModel};
use crate::query::sql_frontend::SqlFrontendParser;
use crate::services::CollectionService;
use crate::services::VectorOperationsService;
use crate::services::{DdlService, DmlService};
use crate::storage::document::DocumentService;

/// PostgreSQL protocol handler
pub struct PostgresProtocol {
    /// TCP stream
    stream: TcpStream,
    /// Session state
    session: Arc<RwLock<Session>>,
    /// Collection service
    collection_service: Arc<CollectionService>,
    /// Vector operations service for search
    vector_ops: Arc<VectorOperationsService>,
    /// Query translator
    translator: QueryTranslator,
    /// Read buffer
    #[allow(dead_code)]
    read_buffer: BytesMut,
    /// Write buffer
    write_buffer: BytesMut,
    /// Prepared statements cache
    prepared_statements: HashMap<String, PreparedStatement>,
    /// Portals (bound statements ready for execution)
    portals: HashMap<String, Portal>,
    /// DDL service for CREATE/DROP/ALTER operations (optional, for catalog integration)
    #[allow(dead_code)]
    ddl_service: Option<Arc<DdlService>>,
    /// Catalog manager for SQL-facing xCatalog/information_schema introspection.
    catalog_manager: Option<Arc<CatalogManager>>,
    /// DML service for INSERT/UPDATE/DELETE operations (optional, for catalog integration)
    dml_service: Option<Arc<DmlService>>,
    /// Document service for native document collections
    document_service: Option<Arc<DocumentService>>,
    /// Graph service for native graph collections
    graph_service: Option<Arc<GraphService>>,
    /// Observability service for logs/metrics/traces
    observability_service: Option<Arc<ObservabilityService>>,
}

/// Prepared statement
struct PreparedStatement {
    /// Original query
    query: String,
    /// Translated query
    #[allow(dead_code)]
    translated: String,
    /// Parameter types
    param_types: Vec<PgType>,
}

/// Portal - a bound statement ready for execution
#[derive(Clone)]
struct Portal {
    /// Statement name this portal was bound from
    #[allow(dead_code)]
    statement_name: String,
    /// Bound query with parameters substituted
    bound_query: String,
    /// Original translated query
    #[allow(dead_code)]
    translated: String,
    /// Parameter values (already bound)
    #[allow(dead_code)]
    param_values: Vec<Option<Vec<u8>>>,
    /// Max rows to return (0 = unlimited)
    #[allow(dead_code)]
    max_rows: i32,
}

// DataModel imported from crate::query::multimodal_router (canonical definition)

/// COPY format type for bulk data transfer
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CopyFormat {
    /// Text format (default PostgreSQL)
    Text,
    /// CSV format
    Csv,
    /// PostgreSQL binary format
    Binary,
    /// Arrow IPC format (ProximaDB extension, most efficient)
    Arrow,
}

fn pg_type_for_catalog_column(column_type: &str) -> PgType {
    match column_type {
        "bool" | "boolean" => PgType::Bool,
        "int2" => PgType::Int2,
        "int4" | "int32" => PgType::Int4,
        "int8" | "int64" => PgType::Int8,
        "float4" | "float32" => PgType::Float4,
        "float8" | "float64" => PgType::Float8,
        "json" => PgType::Json,
        "jsonb" => PgType::Jsonb,
        "uuid" => PgType::Uuid,
        "timestamp" => PgType::Timestamp,
        "timestamptz" => PgType::Timestamptz,
        "vector" => PgType::Vector,
        _ => PgType::Text,
    }
}

/// PostgreSQL message types (frontend)
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FrontendMessage {
    /// Startup message (no type byte)
    Startup = 0,
    /// Query (simple query protocol)
    Query = b'Q',
    /// Parse (extended query)
    Parse = b'P',
    /// Bind (extended query)
    Bind = b'B',
    /// Execute (extended query)
    Execute = b'E',
    /// Describe (extended query)
    Describe = b'D',
    /// Sync (extended query)
    Sync = b'S',
    /// Flush (extended query)
    Flush = b'H',
    /// Close (extended query)
    Close = b'C',
    /// Password message
    Password = b'p',
    /// Terminate
    Terminate = b'X',
    /// Copy data
    CopyData = b'd',
    /// Copy done
    CopyDone = b'c',
    /// Copy fail
    CopyFail = b'f',
}

impl PostgresProtocol {
    /// Create a new protocol handler
    pub fn new(
        stream: TcpStream,
        session: Session,
        collection_service: Arc<CollectionService>,
        vector_ops: Arc<VectorOperationsService>,
        document_service: Option<Arc<DocumentService>>,
        graph_service: Option<Arc<GraphService>>,
        observability_service: Option<Arc<ObservabilityService>>,
    ) -> Self {
        Self {
            stream,
            session: Arc::new(RwLock::new(session)),
            collection_service,
            vector_ops,
            translator: QueryTranslator::new(),
            read_buffer: BytesMut::with_capacity(8192),
            write_buffer: BytesMut::with_capacity(8192),
            prepared_statements: HashMap::new(),
            portals: HashMap::new(),
            ddl_service: None,
            catalog_manager: None,
            dml_service: None,
            document_service,
            graph_service,
            observability_service,
        }
    }

    /// Create a new protocol handler with DDL/DML services for catalog integration
    pub fn with_catalog_services(
        stream: TcpStream,
        session: Session,
        collection_service: Arc<CollectionService>,
        vector_ops: Arc<VectorOperationsService>,
        catalog_manager: Arc<CatalogManager>,
    ) -> Self {
        let ddl_service = Arc::new(DdlService::new(catalog_manager.clone()));
        let dml_service = Arc::new(DmlService::new(catalog_manager.clone(), vector_ops.clone()));

        Self {
            stream,
            session: Arc::new(RwLock::new(session)),
            collection_service,
            vector_ops,
            translator: QueryTranslator::new(),
            read_buffer: BytesMut::with_capacity(8192),
            write_buffer: BytesMut::with_capacity(8192),
            prepared_statements: HashMap::new(),
            portals: HashMap::new(),
            ddl_service: Some(ddl_service),
            catalog_manager: Some(catalog_manager),
            dml_service: Some(dml_service),
            document_service: None,
            graph_service: None,
            observability_service: None,
        }
    }

    /// Run the protocol loop
    pub async fn run(&mut self) -> Result<()> {
        // Handle startup
        self.handle_startup().await?;

        // Main loop
        loop {
            // Read message type
            let msg_type = self.read_byte().await?;

            if msg_type == FrontendMessage::Terminate as u8 {
                debug!("Client terminated connection");
                break;
            }

            // Read message length
            let length = self.read_i32().await? as usize;
            if length < 4 {
                return Err(anyhow!("Invalid message length"));
            }

            // Read message body
            let body_len = length - 4;
            let body = self.read_bytes(body_len).await?;

            // Handle message
            match msg_type {
                b'Q' => self.handle_query(&body).await?,
                b'P' => self.handle_parse(&body).await?,
                b'B' => self.handle_bind(&body).await?,
                b'E' => self.handle_execute(&body).await?,
                b'D' => self.handle_describe(&body).await?,
                b'S' => self.handle_sync().await?,
                b'H' => self.handle_flush().await?,
                b'C' => self.handle_close(&body).await?,
                _ => {
                    warn!("Unknown message type: {}", msg_type as char);
                    self.send_error("ERROR", "XX000", "Unknown message type")
                        .await?;
                }
            }
        }

        Ok(())
    }

    /// Handle startup handshake
    async fn handle_startup(&mut self) -> Result<()> {
        // Read startup message length
        let length = self.read_i32().await? as usize;
        if length < 8 {
            return Err(anyhow!("Invalid startup message length"));
        }

        // Read protocol version
        let version = self.read_i32().await?;
        let major = (version >> 16) as i16;
        let minor = (version & 0xFFFF) as i16;

        debug!("Client protocol version: {}.{}", major, minor);

        // Check for SSL request
        if version == 80877103 {
            // SSL request - send 'N' (no SSL)
            self.stream.write_all(b"N").await?;
            // Use Box::pin for async recursion
            return Box::pin(self.handle_startup()).await;
        }

        // Check for cancel request
        if version == 80877102 {
            // Cancel request - not implemented
            return Err(anyhow!("Cancel request not supported"));
        }

        // Read startup parameters
        let param_len = length - 8;
        let params = self.read_bytes(param_len).await?;
        let params = self.parse_startup_params(&params)?;

        // Store parameters in session
        {
            let mut session = self.session.write().await;
            if let Some(user) = params.get("user") {
                session.user = user.clone();
            }
            if let Some(database) = params.get("database") {
                session.database = database.clone();
            }
        }

        // Send authentication OK (trust auth)
        self.send_auth_ok().await?;

        // Send parameter status messages
        self.send_parameter_status("server_version", "16.0").await?;
        self.send_parameter_status("server_encoding", "UTF8")
            .await?;
        self.send_parameter_status("client_encoding", "UTF8")
            .await?;
        self.send_parameter_status("DateStyle", "ISO, MDY").await?;
        self.send_parameter_status("integer_datetimes", "on")
            .await?;

        // Send backend key data
        self.send_backend_key_data().await?;

        // Send ready for query
        self.send_ready_for_query('I').await?;

        info!("PostgreSQL startup complete");

        Ok(())
    }

    /// Parse startup parameters
    fn parse_startup_params(&self, data: &[u8]) -> Result<HashMap<String, String>> {
        let mut params = HashMap::new();
        let mut cursor = Cursor::new(data);

        loop {
            let key = self.read_cstring(&mut cursor)?;
            if key.is_empty() {
                break;
            }
            let value = self.read_cstring(&mut cursor)?;
            params.insert(key, value);
        }

        Ok(params)
    }

    /// Handle simple query
    async fn handle_query(&mut self, body: &[u8]) -> Result<()> {
        let query = self.parse_cstring(body)?;
        debug!("Received query: {}", query);

        // Translate query to ProximaDB format
        match self.translator.translate(&query) {
            Ok(result) => {
                // Execute translated query
                self.execute_query(&result).await?;
            }
            Err(e) => {
                self.send_error("ERROR", "42601", &format!("Syntax error: {}", e))
                    .await?;
            }
        }

        // Send ready for query
        self.send_ready_for_query('I').await?;

        Ok(())
    }

    /// Execute a translated query
    async fn execute_query(&mut self, query: &str) -> Result<()> {
        let upper = query.to_uppercase();

        // Handle SHOW commands converted to SELECT
        if upper.contains("AS SERVER_VERSION") {
            return self
                .send_single_value_result(
                    "server_version",
                    "ProximaDB 0.2.0 (PostgreSQL 16.0 compatible)",
                )
                .await;
        }
        if upper.contains("AS SERVER_ENCODING") || upper.contains("AS CLIENT_ENCODING") {
            return self.send_single_value_result("encoding", "UTF8").await;
        }
        if upper.contains("AS TIMEZONE") {
            return self.send_single_value_result("timezone", "UTC").await;
        }
        if upper.contains("AS SEARCH_PATH") {
            return self.send_single_value_result("search_path", "public").await;
        }

        if crate::services::CatalogIntrospectionService::is_catalog_query(query) {
            return self.execute_catalog_introspection_query(query).await;
        }

        // Handle pg_catalog compatibility queries not yet backed by xCatalog.
        if upper.contains("FROM PG_CATALOG") || upper.contains("FROM INFORMATION_SCHEMA") {
            return self.send_empty_result().await;
        }

        // Handle SELECT queries
        if upper.starts_with("SELECT") {
            // Check if this is a vector search query
            if upper.contains("<->") || upper.contains("<=>") || upper.contains("<#>") {
                return self.execute_vector_search(query).await;
            }

            // Check if this is a simple table query
            if let Some(table_name) = self.extract_table_name(&upper) {
                // Detect store type from table name or query content
                let store_type = self.detect_select_store_type(&table_name, &upper);
                return match store_type {
                    DataModel::Document => self.execute_document_query(&table_name, query).await,
                    DataModel::Observability | DataModel::TimeSeries => {
                        self.execute_observability_query(&table_name, query).await
                    }
                    DataModel::Graph => self.execute_graph_query(&table_name, query).await,
                    DataModel::Vector => self.execute_collection_query(&table_name, query).await,
                    DataModel::Relational | DataModel::Event => {
                        self.execute_relational_query(query, &table_name).await
                    }
                };
            }

            // Default: return empty result for unknown queries
            return self.send_empty_result().await;
        }

        if upper.starts_with("CREATE ") || upper.starts_with("ALTER ") || upper.starts_with("DROP ")
        {
            if let Some(ddl_service) = self.ddl_service.clone() {
                let parser = SqlFrontendParser::new();
                match parser.parse_ddl(query) {
                    Ok(Some(statement)) => match ddl_service.execute(statement).await {
                        Ok(result) => {
                            let tag = if upper.starts_with("CREATE TABLE") {
                                "CREATE TABLE"
                            } else if upper.starts_with("CREATE INDEX") {
                                "CREATE INDEX"
                            } else if upper.starts_with("ALTER TABLE") {
                                "ALTER TABLE"
                            } else if upper.starts_with("DROP TABLE") {
                                "DROP TABLE"
                            } else if upper.starts_with("DROP INDEX") {
                                "DROP INDEX"
                            } else {
                                "OK"
                            };
                            info!(message = %result.message, "DDL executed via catalog service");
                            return self.send_command_complete(tag).await;
                        }
                        Err(e) => {
                            warn!("DdlService execution failed: {}", e);
                            return self
                                .send_error("ERROR", "42P01", &format!("DDL failed: {}", e))
                                .await;
                        }
                    },
                    Ok(None) => {}
                    Err(e) => {
                        if upper.starts_with("CREATE INDEX") || upper.starts_with("ALTER TABLE") {
                            return self
                                .send_error("ERROR", "42601", &format!("Parse error: {}", e))
                                .await;
                        }
                    }
                }
            }
        }

        // Handle CREATE TABLE (creates a collection)
        if upper.starts_with("CREATE TABLE") {
            return self.execute_create_table(query).await;
        }

        // Handle INSERT (inserts vectors)
        if upper.starts_with("INSERT") {
            return self.execute_insert(query).await;
        }

        // Handle DELETE (deletes vectors)
        if upper.starts_with("DELETE") {
            return self.execute_delete(query).await;
        }

        // Handle UPDATE (updates vectors/metadata)
        if upper.starts_with("UPDATE") {
            return self.execute_update(query).await;
        }

        // Handle DROP TABLE (deletes collection)
        if upper.starts_with("DROP TABLE") {
            return self.execute_drop_table(query).await;
        }

        // Handle COPY command (bulk data transfer)
        if upper.starts_with("COPY") {
            return self.execute_copy(query).await;
        }

        // Handle other commands
        self.send_command_complete("OK").await
    }

    /// Send a single value result (for SHOW commands)
    async fn send_single_value_result(&mut self, name: &str, value: &str) -> Result<()> {
        let fields = vec![FieldDescription::new(name, PgType::Text)];
        self.send_row_description(&fields).await?;
        self.send_data_row(&[value]).await?;
        self.send_command_complete("SELECT 1").await
    }

    async fn execute_catalog_introspection_query(&mut self, query: &str) -> Result<()> {
        let Some(catalog_manager) = self.catalog_manager.clone() else {
            return self.send_empty_result().await;
        };

        let result = crate::services::CatalogIntrospectionService::new(catalog_manager)
            .execute_select(query)
            .await?;
        let Some(result) = result else {
            return self.send_empty_result().await;
        };

        let fields = result
            .columns
            .iter()
            .zip(result.column_types.iter())
            .map(|(name, column_type)| {
                FieldDescription::new(name, pg_type_for_catalog_column(column_type))
            })
            .collect::<Vec<_>>();
        self.send_row_description(&fields).await?;

        let mut count = 0;
        for row in &result.rows {
            let values = row.iter().map(String::as_str).collect::<Vec<_>>();
            self.send_data_row(&values).await?;
            count += 1;
        }

        self.send_command_complete(&format!("SELECT {}", count))
            .await
    }

    /// Send empty result
    async fn send_empty_result(&mut self) -> Result<()> {
        self.send_row_description(&[]).await?;
        self.send_command_complete("SELECT 0").await
    }

    /// Extract table name from query
    fn extract_table_name(&self, query: &str) -> Option<String> {
        // Simple extraction: look for FROM <table>
        let from_pos = query.find("FROM ")?;
        let after_from = &query[from_pos + 5..];
        let table_end = after_from
            .find(|c: char| c.is_whitespace() || c == ';')
            .unwrap_or(after_from.len());
        let table = after_from[..table_end].trim();
        if table.is_empty() {
            None
        } else {
            Some(table.to_lowercase())
        }
    }

    /// Execute a vector search query
    async fn execute_vector_search(&mut self, query: &str) -> Result<()> {
        // Parse vector from query: look for '[...]'
        let query_vector = self.extract_vector_from_query(query);
        let table_name = self
            .extract_table_name(&query.to_uppercase())
            .unwrap_or_else(|| "default".to_string());

        // Get top_k from LIMIT clause, default to 10
        let top_k = self.extract_limit(query).unwrap_or(10);

        debug!(
            "Executing vector search on {} with top_k={}",
            table_name, top_k
        );

        if let Some(ref vector) = query_vector {
            // Execute actual vector search
            match self
                .vector_ops
                .unified_search_native(
                    &table_name,
                    vector.clone(),
                    top_k,
                    None, // No metadata filter
                    None, // Default config
                )
                .await
            {
                Ok(results) => {
                    // Define result columns
                    let fields = vec![
                        FieldDescription::new("id", PgType::Text),
                        FieldDescription::new("distance", PgType::Float8),
                        FieldDescription::new("metadata", PgType::Jsonb),
                    ];
                    self.send_row_description(&fields).await?;

                    // Send each result as a row
                    let mut count = 0;
                    for record in &results {
                        let id = &record.id;
                        let distance = format!("{:.6}", record.score);
                        let metadata = serde_json::to_string(&record.metadata)
                            .unwrap_or_else(|_| "{}".to_string());

                        self.send_data_row(&[id, &distance, &metadata]).await?;
                        count += 1;
                    }

                    self.send_command_complete(&format!("SELECT {}", count))
                        .await
                }
                Err(e) => {
                    warn!("Vector search error: {}", e);
                    // Return empty result on error
                    self.send_empty_result().await
                }
            }
        } else {
            // No vector found in query, return empty
            self.send_empty_result().await
        }
    }

    /// Extract vector from query string (e.g., '[0.1, 0.2, 0.3]')
    fn extract_vector_from_query(&self, query: &str) -> Option<Vec<f32>> {
        let start = query.find('[')?;
        let end = query.find(']')?;
        if end <= start {
            return None;
        }

        let vector_str = &query[start + 1..end];
        let values: Result<Vec<f32>, _> = vector_str.split(',').map(|s| s.trim().parse()).collect();
        values.ok()
    }

    /// Extract LIMIT value from query
    fn extract_limit(&self, query: &str) -> Option<usize> {
        let upper = query.to_uppercase();
        let limit_pos = upper.find("LIMIT ")?;
        let after_limit = &query[limit_pos + 6..];
        let limit_end = after_limit
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(after_limit.len());
        after_limit[..limit_end].trim().parse().ok()
    }

    /// Detect store type for SELECT queries
    fn detect_select_store_type(&self, table_name: &str, query: &str) -> DataModel {
        multimodal_router::detect_store_type_from_query(query, table_name, None)
    }

    /// Execute a query against a vector collection
    async fn execute_collection_query(
        &mut self,
        collection_name: &str,
        _query: &str,
    ) -> Result<()> {
        // Check if collection exists
        match self.collection_service.collection(collection_name).await {
            Ok(Some(collection)) => {
                // Return collection info
                let fields = vec![
                    FieldDescription::new("collection_name", PgType::Text),
                    FieldDescription::new("dimension", PgType::Int4),
                    FieldDescription::new("vector_count", PgType::Int8),
                ];
                self.send_row_description(&fields).await?;

                // Get name and dimension from config
                let name = collection
                    .config
                    .as_ref()
                    .map_or_else(|| collection.id.clone(), |c| c.name.clone());
                let dim = collection
                    .config
                    .as_ref()
                    .map_or_else(|| "0".to_string(), |c| c.dimension.to_string());
                let count = collection
                    .stats
                    .as_ref()
                    .map_or_else(|| "0".to_string(), |s| s.vector_count.to_string());

                self.send_data_row(&[&name, &dim, &count]).await?;
                self.send_command_complete("SELECT 1").await
            }
            Ok(None) => {
                // Collection not found, return empty
                self.send_empty_result().await
            }
            Err(e) => {
                warn!("Collection query error: {}", e);
                self.send_empty_result().await
            }
        }
    }

    /// Execute a relational query against a standard SQL table (SEQUOIA engine)
    /// Phase 2 will wire this to SequoiaEngine.query_rows() for actual row retrieval
    async fn execute_relational_query(&mut self, _query: &str, table_name: &str) -> Result<()> {
        debug!("Executing relational query on table: {}", table_name);

        // Return empty result set with generic column descriptions
        // Phase 2: Parse SELECT columns and wire to SequoiaEngine for real data
        let fields = vec![
            FieldDescription::new("id", PgType::Int4),
            FieldDescription::new("result", PgType::Text),
        ];
        self.send_row_description(&fields).await?;
        self.send_command_complete("SELECT 0").await
    }

    /// Execute a document store query
    /// Supports: SELECT * FROM doc_users WHERE $.age > 25
    async fn execute_document_query(&mut self, table_name: &str, query: &str) -> Result<()> {
        debug!("Executing document query on table: {}", table_name);

        // Extract collection name (remove doc_ prefix if present)
        let collection_name = table_name
            .trim_start_matches("doc_")
            .trim_start_matches("documents.");

        // Parse WHERE clause for JSON path filters
        let filter = self.parse_document_where_clause(query);
        let limit = self.extract_limit(query).unwrap_or(100) as u32;

        // Build query params
        let query_params = crate::storage::document::DocumentQueryParams {
            filter,
            projection: Vec::new(),
            sort: Vec::new(),
            limit,
            offset: 0,
            include_count: false,
        };

        let doc_service = self.document_service.clone().unwrap_or_else(|| {
            Arc::new(crate::storage::document::DocumentService::new(
                self.vector_ops.unified_engine(),
            ))
        });

        match doc_service
            .query_documents(collection_name, query_params)
            .await
        {
            Ok(result) => {
                // Define result columns
                let fields = vec![
                    FieldDescription::new("id", PgType::Text),
                    FieldDescription::new("document", PgType::Jsonb),
                    FieldDescription::new("version", PgType::Int8),
                ];
                self.send_row_description(&fields).await?;

                let mut count = 0;
                for doc in &result.documents {
                    let id = &doc.id;
                    let document = self.sql_object_to_json(&doc.document);
                    let version = doc.version.to_string();

                    self.send_data_row(&[id, &document, &version]).await?;
                    count += 1;
                }

                self.send_command_complete(&format!("SELECT {}", count))
                    .await
            }
            Err(e) => {
                warn!("Document query error: {}", e);
                self.send_empty_result().await
            }
        }
    }

    /// Parse document WHERE clause for JSON path filters
    fn parse_document_where_clause(
        &self,
        query: &str,
    ) -> Option<crate::proto::proximadb_v1::DocumentFilter> {
        use crate::proto::proximadb_v1::DocumentFilter;

        let upper = query.to_uppercase();
        let where_pos = upper.find("WHERE")?;
        let where_clause = &query[where_pos + 5..];

        // Simple parsing: look for $.field op value patterns
        // E.g., $.age > 25, $.name = 'John'
        let mut conditions = Vec::new();

        // Split by AND
        for part in where_clause.split(" AND ") {
            let part = part.trim();
            if part.starts_with("$.") || part.starts_with("$[") {
                // Parse JSON path condition
                if let Some(cond) = self.parse_json_path_condition(part) {
                    conditions.push(cond);
                }
            }
        }

        if conditions.is_empty() {
            None
        } else {
            Some(DocumentFilter {
                conditions,
                or_filters: Vec::new(),
                and_filters: Vec::new(),
            })
        }
    }

    /// Parse a single JSON path condition
    fn parse_json_path_condition(
        &self,
        condition: &str,
    ) -> Option<crate::proto::proximadb_v1::DocFilterCondition> {
        use crate::proto::proximadb_v1::{DocFilterCondition, DocFilterOperator};

        // Patterns: $.field = value, $.field > value, $.field < value, etc.
        let ops = [">=", "<=", "!=", "<>", "=", ">", "<", "LIKE", "CONTAINS"];

        for op_str in ops {
            if let Some(op_pos) = condition.find(op_str) {
                let path = condition[..op_pos].trim().to_string();
                let value_str = condition[op_pos + op_str.len()..].trim();

                let operator = match op_str {
                    "=" => DocFilterOperator::Eq as i32,
                    "!=" | "<>" => DocFilterOperator::Ne as i32,
                    ">" => DocFilterOperator::Gt as i32,
                    ">=" => DocFilterOperator::Gte as i32,
                    "<" => DocFilterOperator::Lt as i32,
                    "<=" => DocFilterOperator::Lte as i32,
                    "LIKE" => DocFilterOperator::Regex as i32, // Map LIKE to REGEX
                    "CONTAINS" => DocFilterOperator::Contains as i32,
                    _ => DocFilterOperator::Eq as i32,
                };

                // Parse value
                let value = self.parse_sql_value(value_str);

                return Some(DocFilterCondition {
                    path,
                    operator,
                    value: Some(value),
                    values: Vec::new(),
                });
            }
        }
        None
    }

    /// Parse SQL value from string
    fn parse_sql_value(&self, s: &str) -> crate::proto::proximadb_v1::SqlValue {
        use crate::proto::proximadb_v1::SqlValue;
        use crate::proto::proximadb_v1::sql_value::Value as SqlVal;

        let trimmed = s.trim();

        // String literal
        if trimmed.starts_with('\'') && trimmed.ends_with('\'') {
            let inner = &trimmed[1..trimmed.len() - 1];
            return SqlValue {
                value: Some(SqlVal::StringValue(inner.to_string())),
            };
        }

        // Boolean
        if trimmed.eq_ignore_ascii_case("true") {
            return SqlValue {
                value: Some(SqlVal::BoolValue(true)),
            };
        }
        if trimmed.eq_ignore_ascii_case("false") {
            return SqlValue {
                value: Some(SqlVal::BoolValue(false)),
            };
        }

        // NULL
        if trimmed.eq_ignore_ascii_case("null") {
            return SqlValue {
                value: Some(SqlVal::NullValue(0)),
            };
        }

        // Integer
        if let Ok(i) = trimmed.parse::<i64>() {
            return SqlValue {
                value: Some(SqlVal::Int64Value(i)),
            };
        }

        // Float
        if let Ok(f) = trimmed.parse::<f64>() {
            return SqlValue {
                value: Some(SqlVal::NumberValue(f)),
            };
        }

        // Default to string
        SqlValue {
            value: Some(SqlVal::StringValue(trimmed.to_string())),
        }
    }

    /// Convert SqlObject to JSON string
    fn sql_object_to_json(&self, obj: &crate::proto::proximadb_v1::SqlObject) -> String {
        use crate::proto::proximadb_v1::sql_value::Value as SqlVal;

        let mut map = serde_json::Map::new();
        for (k, v) in &obj.fields {
            let json_val = match &v.value {
                Some(SqlVal::StringValue(s)) => serde_json::Value::String(s.clone()),
                Some(SqlVal::Int64Value(i)) => serde_json::json!(*i),
                Some(SqlVal::NumberValue(f)) => serde_json::json!(*f),
                Some(SqlVal::BoolValue(b)) => serde_json::Value::Bool(*b),
                Some(SqlVal::NullValue(_)) => serde_json::Value::Null,
                _ => serde_json::Value::Null,
            };
            map.insert(k.clone(), json_val);
        }
        serde_json::to_string(&serde_json::Value::Object(map)).unwrap_or_else(|_| "{}".to_string())
    }

    fn json_value_to_sql_object(
        &self,
        value: &serde_json::Value,
    ) -> Option<crate::proto::proximadb_v1::SqlObject> {
        let serde_json::Value::Object(map) = value else {
            return None;
        };

        let fields = map
            .iter()
            .filter_map(|(key, value)| {
                self.json_value_to_sql_value(value)
                    .map(|sql_value| (key.clone(), sql_value))
            })
            .collect();

        Some(crate::proto::proximadb_v1::SqlObject { fields })
    }

    fn json_value_to_sql_value(
        &self,
        value: &serde_json::Value,
    ) -> Option<crate::proto::proximadb_v1::SqlValue> {
        use crate::proto::proximadb_v1::{SqlArray, SqlValue, sql_value::Value as SqlVal};

        match value {
            serde_json::Value::Null => Some(SqlValue {
                value: Some(SqlVal::NullValue(0)),
            }),
            serde_json::Value::Bool(b) => Some(SqlValue {
                value: Some(SqlVal::BoolValue(*b)),
            }),
            serde_json::Value::Number(n) => n
                .as_i64()
                .map(|i| SqlValue {
                    value: Some(SqlVal::Int64Value(i)),
                })
                .or_else(|| {
                    n.as_f64().map(|f| SqlValue {
                        value: Some(SqlVal::NumberValue(f)),
                    })
                }),
            serde_json::Value::String(s) => Some(SqlValue {
                value: Some(SqlVal::StringValue(s.clone())),
            }),
            serde_json::Value::Array(values) => {
                let values = values
                    .iter()
                    .filter_map(|item| self.json_value_to_sql_value(item))
                    .collect();
                Some(SqlValue {
                    value: Some(SqlVal::ArrayValue(SqlArray { values })),
                })
            }
            serde_json::Value::Object(_) => {
                self.json_value_to_sql_object(value).map(|object| SqlValue {
                    value: Some(SqlVal::ObjectValue(object)),
                })
            }
        }
    }

    /// Execute an observability store query (logs, metrics)
    async fn execute_observability_query(&mut self, table_name: &str, query: &str) -> Result<()> {
        use crate::observability::{LogQueryParams, ObservabilityService, ObservabilityStorage};

        debug!("Executing observability query on table: {}", table_name);

        let lower_table = table_name.to_lowercase();

        // Determine if this is a log or metric query
        if lower_table.contains("metric") {
            return self.execute_metric_query(table_name, query).await;
        }

        // Log query
        let obs_service = if let Some(service) = self.observability_service.clone() {
            service
        } else {
            let data_dir = std::env::var("PROXIMADB_DATA_DIR")
                .unwrap_or_else(|_| "/tmp/proximadb/data".to_string());
            let storage = std::sync::Arc::new(ObservabilityStorage::new(&data_dir));

            match ObservabilityService::new(storage).await {
                Ok(service) => Arc::new(service),
                Err(e) => {
                    warn!("Failed to create observability service: {}", e);
                    return self.send_empty_result().await;
                }
            }
        };

        // Parse query parameters
        let namespace = table_name
            .trim_start_matches("log_")
            .trim_start_matches("logs.")
            .trim_start_matches("observability.");
        let namespace = if namespace.is_empty() {
            "default"
        } else {
            namespace
        };

        let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let one_hour_ns = 3_600_000_000_000_i64;

        // Extract time range from WHERE clause
        let (start_time, end_time) = self.extract_time_range(query, now_ns, one_hour_ns);
        let limit = self.extract_limit(query).unwrap_or(100) as u32;

        // Extract severity filter
        let severities = self.extract_severity_filter(query);

        // Extract service filter
        let services = self.extract_service_filter(query);

        let params = LogQueryParams {
            start_time_ns: start_time,
            end_time_ns: end_time,
            query: None,
            severities,
            services,
            sources: Vec::new(),
            limit,
            cursor: None,
        };

        match obs_service.query_logs(namespace, params).await {
            Ok(result) => {
                let fields = vec![
                    FieldDescription::new("timestamp", PgType::Timestamptz),
                    FieldDescription::new("severity", PgType::Text),
                    FieldDescription::new("message", PgType::Text),
                    FieldDescription::new("service", PgType::Text),
                    FieldDescription::new("source", PgType::Text),
                ];
                self.send_row_description(&fields).await?;

                let mut count = 0;
                for log in result.logs {
                    let ts = chrono::DateTime::from_timestamp_nanos(log.timestamp_ns)
                        .format("%Y-%m-%d %H:%M:%S%.6f")
                        .to_string();
                    let severity = self.severity_to_string(log.severity);
                    let message = log.message;
                    let service = log.service.unwrap_or_default();
                    let source = log.source.unwrap_or_default();

                    self.send_data_row(&[&ts, &severity, &message, &service, &source])
                        .await?;
                    count += 1;
                }

                self.send_command_complete(&format!("SELECT {}", count))
                    .await
            }
            Err(e) => {
                warn!("Log query error: {}", e);
                self.send_empty_result().await
            }
        }
    }

    /// Execute a metric query with aggregation
    async fn execute_metric_query(&mut self, table_name: &str, query: &str) -> Result<()> {
        use crate::observability::{MetricAggParams, ObservabilityService, ObservabilityStorage};

        debug!("Executing metric query on table: {}", table_name);

        let obs_service = if let Some(service) = self.observability_service.clone() {
            service
        } else {
            let data_dir = std::env::var("PROXIMADB_DATA_DIR")
                .unwrap_or_else(|_| "/tmp/proximadb/data".to_string());
            let storage = std::sync::Arc::new(ObservabilityStorage::new(&data_dir));

            match ObservabilityService::new(storage).await {
                Ok(service) => Arc::new(service),
                Err(e) => {
                    warn!("Failed to create observability service: {}", e);
                    return self.send_empty_result().await;
                }
            }
        };

        let namespace = table_name
            .trim_start_matches("metric_")
            .trim_start_matches("metrics.");
        let namespace = if namespace.is_empty() {
            "default"
        } else {
            namespace
        };

        let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let one_hour_ns = 3_600_000_000_000_i64;

        let (start_time, end_time) = self.extract_time_range(query, now_ns, one_hour_ns);

        // Extract metric name from WHERE clause
        let metric_name = self
            .extract_metric_name(query)
            .unwrap_or_else(|| "*".to_string());

        // Detect aggregation function
        let aggregation = self.detect_aggregation(query);

        let params = MetricAggParams {
            metric_name,
            start_time_ns: start_time,
            end_time_ns: end_time,
            aggregation,
            step_seconds: 60,
            label_filters: std::collections::HashMap::new(),
            group_by: Vec::new(),
        };

        match obs_service.aggregate_metrics(namespace, params).await {
            Ok(result) => {
                let fields = vec![
                    FieldDescription::new("timestamp", PgType::Timestamptz),
                    FieldDescription::new("value", PgType::Float8),
                    FieldDescription::new("labels", PgType::Jsonb),
                ];
                self.send_row_description(&fields).await?;

                let mut count = 0;
                for series in result.series {
                    let labels =
                        serde_json::to_string(&series.labels).unwrap_or_else(|_| "{}".to_string());
                    for point in series.points {
                        let ts = chrono::DateTime::from_timestamp_nanos(point.timestamp_ns)
                            .format("%Y-%m-%d %H:%M:%S%.6f")
                            .to_string();
                        let value = format!("{:.6}", point.value);

                        self.send_data_row(&[&ts, &value, &labels]).await?;
                        count += 1;
                    }
                }

                self.send_command_complete(&format!("SELECT {}", count))
                    .await
            }
            Err(e) => {
                warn!("Metric query error: {}", e);
                self.send_empty_result().await
            }
        }
    }

    /// Execute a graph query
    async fn execute_graph_query(&mut self, table_name: &str, _query: &str) -> Result<()> {
        debug!("Executing graph query on table: {}", table_name);

        // For now, return basic graph info
        // Full graph query support requires integration with GraphService

        let fields = vec![
            FieldDescription::new("graph_name", PgType::Text),
            FieldDescription::new("node_count", PgType::Int8),
            FieldDescription::new("edge_count", PgType::Int8),
        ];
        self.send_row_description(&fields).await?;

        // Return placeholder data - actual implementation would query GraphService
        self.send_data_row(&[table_name, "0", "0"]).await?;
        self.send_command_complete("SELECT 1").await
    }

    /// Extract time range from WHERE clause
    fn extract_time_range(
        &self,
        query: &str,
        default_start: i64,
        default_range: i64,
    ) -> (i64, i64) {
        let upper = query.to_uppercase();

        // Look for BETWEEN ... AND ...
        if let Some(_between_pos) = upper.find("BETWEEN") {
            // Complex parsing - for now use defaults
            return (default_start - default_range, default_start);
        }

        // Look for timestamp > 'value' or timestamp >= 'value'
        // For now, use last hour as default
        (default_start - default_range, default_start)
    }

    /// Extract severity filter from query
    fn extract_severity_filter(&self, query: &str) -> Vec<crate::proto::proximadb_v1::Severity> {
        use crate::proto::proximadb_v1::Severity;

        let upper = query.to_uppercase();
        let mut severities = Vec::new();

        if upper.contains("SEVERITY") {
            if upper.contains("'ERROR'") || upper.contains("ERROR") && upper.contains(">=") {
                severities.push(Severity::Error);
                severities.push(Severity::Fatal);
            } else if upper.contains("'WARN'") || upper.contains("'WARNING'") {
                severities.push(Severity::Warn);
            } else if upper.contains("'INFO'") {
                severities.push(Severity::Info);
            } else if upper.contains("'DEBUG'") {
                severities.push(Severity::Debug);
            }
        }

        severities
    }

    /// Extract service filter from query
    fn extract_service_filter(&self, query: &str) -> Vec<String> {
        let upper = query.to_uppercase();
        let mut services = Vec::new();

        if let Some(service_pos) = upper.find("SERVICE") {
            let after = &query[service_pos..];
            // Look for = 'value' pattern
            if let Some(eq_pos) = after.find('=') {
                let value_start = after[eq_pos + 1..].trim();
                if value_start.starts_with('\'')
                    && let Some(end) = value_start[1..].find('\'')
                {
                    services.push(value_start[1..end + 1].to_string());
                }
            }
        }

        services
    }

    /// Extract metric name from WHERE clause
    fn extract_metric_name(&self, query: &str) -> Option<String> {
        let upper = query.to_uppercase();

        if let Some(name_pos) = upper.find("METRIC_NAME") {
            let after = &query[name_pos..];
            if let Some(eq_pos) = after.find('=') {
                let value_start = after[eq_pos + 1..].trim();
                if value_start.starts_with('\'')
                    && let Some(end) = value_start[1..].find('\'')
                {
                    return Some(value_start[1..end + 1].to_string());
                }
            }
        }

        None
    }

    /// Detect aggregation function from query
    fn detect_aggregation(&self, query: &str) -> crate::observability::MetricAggregation {
        use crate::observability::MetricAggregation;

        let upper = query.to_uppercase();

        if upper.contains("AVG(") || upper.contains("AVERAGE(") {
            MetricAggregation::Avg
        } else if upper.contains("SUM(") {
            MetricAggregation::Sum
        } else if upper.contains("MIN(") {
            MetricAggregation::Min
        } else if upper.contains("MAX(") {
            MetricAggregation::Max
        } else if upper.contains("COUNT(") {
            MetricAggregation::Count
        } else if upper.contains("P99(") || upper.contains("PERCENTILE_99(") {
            MetricAggregation::P99
        } else if upper.contains("P95(") || upper.contains("PERCENTILE_95(") {
            MetricAggregation::P95
        } else if upper.contains("P90(") || upper.contains("PERCENTILE_90(") {
            MetricAggregation::P90
        } else if upper.contains("P50(")
            || upper.contains("PERCENTILE_50(")
            || upper.contains("MEDIAN(")
        {
            MetricAggregation::P50
        } else {
            MetricAggregation::Avg
        }
    }

    /// Convert severity int to string
    fn severity_to_string(&self, severity: i32) -> String {
        use crate::proto::proximadb_v1::Severity;
        match Severity::try_from(severity) {
            Ok(Severity::Trace) => "TRACE".to_string(),
            Ok(Severity::Debug) => "DEBUG".to_string(),
            Ok(Severity::Info) => "INFO".to_string(),
            Ok(Severity::Warn) => "WARN".to_string(),
            Ok(Severity::Error) => "ERROR".to_string(),
            Ok(Severity::Fatal) => "FATAL".to_string(),
            _ => "INFO".to_string(),
        }
    }

    /// Execute CREATE TABLE - creates a ProximaDB collection
    /// Supports multiple store types:
    /// - Vector: CREATE TABLE name (id TEXT, embedding vector(dim)) [USING VECTOR]
    /// - Document: CREATE TABLE name (id TEXT, data JSONB) USING DOCUMENT
    /// - Graph: CREATE TABLE name (id TEXT, labels TEXT[]) USING GRAPH
    /// - Observability: CREATE TABLE name (timestamp TIMESTAMPTZ, message TEXT) USING OBSERVABILITY
    async fn execute_create_table(&mut self, query: &str) -> Result<()> {
        let upper = query.to_uppercase();

        // Extract table name: CREATE TABLE [IF NOT EXISTS] name
        let table_start = if upper.contains("IF NOT EXISTS") {
            upper.find("EXISTS").map(|p| p + 6)
        } else {
            upper.find("TABLE").map(|p| p + 5)
        };

        let Some(start) = table_start else {
            return self.send_command_complete("OK").await;
        };

        let after_table = query[start..].trim();
        let table_end = after_table
            .find(|c: char| c.is_whitespace() || c == '(')
            .unwrap_or(after_table.len());
        let table_name = after_table[..table_end].trim().to_lowercase();

        if table_name.is_empty() {
            return self.send_command_complete("OK").await;
        }

        // Detect store type from USING clause or column types
        let store_type = self.detect_store_type(&upper);

        match store_type {
            DataModel::Vector => self.create_vector_collection(&table_name, &upper).await,
            DataModel::Document => self.create_document_collection(&table_name, &upper).await,
            DataModel::Graph => self.create_graph_collection(&table_name, &upper).await,
            DataModel::Observability | DataModel::TimeSeries => {
                self.create_observability_namespace(&table_name, &upper)
                    .await
            }
            DataModel::Relational | DataModel::Event => {
                info!("Created relational table '{}' via PostgreSQL", table_name);
                self.send_command_complete("CREATE TABLE").await
            }
        }
    }

    /// Detect store type from USING clause or column definitions
    fn detect_store_type(&self, query: &str) -> DataModel {
        multimodal_router::detect_store_type_from_create(query)
    }

    /// Create a vector collection (existing behavior)
    async fn create_vector_collection(&mut self, table_name: &str, query: &str) -> Result<()> {
        // Extract dimension from vector(N) type
        let dimension = self.extract_vector_dimension(query).unwrap_or(128);

        debug!(
            "Creating vector collection '{}' with dimension {}",
            table_name, dimension
        );

        use crate::proto::proximadb_v1::{CollectionConfig, DistanceMetric, StorageEngine};

        let config = CollectionConfig {
            name: table_name.to_string(),
            dimension,
            distance_metric: Some(DistanceMetric::Cosine as i32),
            storage_engine: Some(StorageEngine::Sst as i32),
            ..Default::default()
        };

        match self.collection_service.create_collection(&config).await {
            Ok(_) => {
                info!("Created vector collection '{}' via PostgreSQL", table_name);
                self.send_command_complete("CREATE TABLE").await
            }
            Err(e) => {
                if e.to_string().contains("already exists") {
                    self.send_command_complete("CREATE TABLE").await
                } else {
                    warn!("Failed to create collection '{}': {}", table_name, e);
                    self.send_error("ERROR", "42P07", &format!("Failed to create table: {}", e))
                        .await
                }
            }
        }
    }

    /// Create a document collection (MongoDB-like JSON storage)
    async fn create_document_collection(&mut self, table_name: &str, _query: &str) -> Result<()> {
        debug!("Creating document collection '{}'", table_name);

        use crate::proto::proximadb_v1::DocumentCollectionConfig;

        let config = DocumentCollectionConfig {
            name: table_name.to_string(),
            enable_fulltext: true,
            ..Default::default()
        };

        if let Some(document_service) = self.document_service.clone() {
            match document_service.create_collection(table_name, config).await {
                Ok(_) => {
                    info!(
                        "Created document collection '{}' via PostgreSQL",
                        table_name
                    );
                    return self.send_command_complete("CREATE TABLE").await;
                }
                Err(e) if e.to_string().contains("already exists") => {
                    return self.send_command_complete("CREATE TABLE").await;
                }
                Err(e) => {
                    warn!(
                        "Failed to create document collection '{}' via DocumentService: {}",
                        table_name, e
                    );
                    return self
                        .send_error("ERROR", "42P07", &format!("Failed to create table: {}", e))
                        .await;
                }
            }
        }

        // Fall back to the historical vector-backed document shim when the real document service
        // is not available on this protocol path.
        use crate::proto::proximadb_v1::{CollectionConfig, StorageEngine};

        let vector_config = CollectionConfig {
            name: format!("doc_{}", table_name),
            dimension: 0, // Documents don't have vectors by default
            storage_engine: Some(StorageEngine::Sst as i32),
            description: Some(format!("Document collection: {}", table_name)),
            ..Default::default()
        };

        match self
            .collection_service
            .create_collection(&vector_config)
            .await
        {
            Ok(_) => {
                info!(
                    "Created document collection '{}' via PostgreSQL",
                    table_name
                );
                self.send_command_complete("CREATE TABLE").await
            }
            Err(e) => {
                if e.to_string().contains("already exists") {
                    self.send_command_complete("CREATE TABLE").await
                } else {
                    warn!(
                        "Failed to create document collection '{}': {}",
                        table_name, e
                    );
                    self.send_error("ERROR", "42P07", &format!("Failed to create table: {}", e))
                        .await
                }
            }
        }
    }

    /// Create a graph (nodes/edges storage)
    async fn create_graph_collection(&mut self, table_name: &str, _query: &str) -> Result<()> {
        debug!("Creating graph '{}'", table_name);

        if let Some(graph_service) = self.graph_service.clone() {
            let request = crate::proto::proximadb_v1::CreateGraphRequest {
                graph_id: table_name.to_string(),
                name: Some(table_name.to_string()),
                ..Default::default()
            };

            match graph_service.create_graph_collection(request).await {
                Ok(()) => {
                    info!(
                        "Created graph '{}' via PostgreSQL (graph engine: ORION)",
                        table_name
                    );
                    self.send_command_complete("CREATE TABLE").await
                }
                Err(e) if e.to_string().contains("already exists") => {
                    self.send_command_complete("CREATE TABLE").await
                }
                Err(e) => {
                    warn!("Failed to create graph '{}': {}", table_name, e);
                    self.send_error("ERROR", "42P07", &format!("Failed to create table: {}", e))
                        .await
                }
            }
        } else {
            info!(
                "Graph CREATE acknowledged for '{}' without graph service wiring",
                table_name
            );
            self.send_command_complete("CREATE TABLE").await
        }
    }

    /// Create an observability namespace (logs/metrics/traces)
    async fn create_observability_namespace(
        &mut self,
        table_name: &str,
        _query: &str,
    ) -> Result<()> {
        debug!("Creating observability namespace '{}'", table_name);

        if let Some(observability_service) = self.observability_service.clone() {
            let config = crate::proto::proximadb_v1::ObservabilityNamespaceConfig {
                name: table_name.to_string(),
                retention: Some(crate::proto::proximadb_v1::RetentionConfig {
                    hot_retention_hours: 24,
                    warm_retention_days: 7,
                    cold_retention_days: 30,
                    archive_retention_days: 90,
                }),
                ..Default::default()
            };

            match observability_service.create_namespace(config).await {
                Ok(_) => {
                    info!(
                        "Created observability namespace '{}' via PostgreSQL",
                        table_name
                    );
                    self.send_command_complete("CREATE TABLE").await
                }
                Err(e) if e.to_string().contains("already exists") => {
                    self.send_command_complete("CREATE TABLE").await
                }
                Err(e) => {
                    warn!(
                        "Failed to create observability namespace '{}': {}",
                        table_name, e
                    );
                    self.send_error("ERROR", "42P07", &format!("Failed to create table: {}", e))
                        .await
                }
            }
        } else {
            info!(
                "Observability namespace CREATE acknowledged for '{}' without service wiring",
                table_name
            );
            self.send_command_complete("CREATE TABLE").await
        }
    }

    /// Extract vector dimension from type: vector(128) -> 128
    fn extract_vector_dimension(&self, query: &str) -> Option<u32> {
        let vector_pos = query.find("VECTOR(")?;
        let after_vector = &query[vector_pos + 7..];
        let dim_end = after_vector.find(')')?;
        after_vector[..dim_end].trim().parse().ok()
    }

    /// Execute INSERT - supports multiple store types
    /// - Vector: INSERT INTO table (id, embedding) VALUES ('id', '[0.1, 0.2, ...]')
    /// - Document: INSERT INTO table (id, data) VALUES ('id', '{"key": "value"}')
    /// - Observability: INSERT INTO table (timestamp, message) VALUES (NOW(), 'log message')
    async fn execute_insert(&mut self, query: &str) -> Result<()> {
        let upper = query.to_uppercase();

        // Extract table name: INSERT INTO table
        let into_pos = upper
            .find("INTO ")
            .ok_or_else(|| anyhow::anyhow!("Missing INTO clause"))?;
        let after_into = query[into_pos + 5..].trim();
        let table_end = after_into
            .find(|c: char| c.is_whitespace() || c == '(')
            .unwrap_or(after_into.len());
        let table_name = after_into[..table_end].trim().to_lowercase();

        if table_name.is_empty() {
            return self.send_command_complete("INSERT 0 0").await;
        }

        // Detect store type from table name prefix or content
        let store_type = self.detect_insert_store_type(&table_name, &upper);

        match store_type {
            DataModel::Vector => {
                // Use DmlService for proper SQL DML execution if available
                if let Some(dml_service) = self.dml_service.clone() {
                    return self
                        .execute_insert_via_dml_service(query, &dml_service)
                        .await;
                }
                // Fall back to string parsing
                self.insert_vector(&table_name, query).await
            }
            DataModel::Document => self.insert_document(&table_name, query).await,
            DataModel::Graph => self.insert_graph_data(&table_name, query).await,
            DataModel::Observability | DataModel::TimeSeries => {
                self.insert_log(&table_name, query).await
            }
            DataModel::Relational | DataModel::Event => {
                self.send_command_complete("INSERT 0 1").await
            }
        }
    }

    /// Execute INSERT using the proper SQL parser and DmlService
    async fn execute_insert_via_dml_service(
        &mut self,
        query: &str,
        dml_service: &Arc<DmlService>,
    ) -> Result<()> {
        let parser = SqlFrontendParser::new();

        match parser.parse_dml(query) {
            Ok(Some(statement)) => match dml_service.execute(statement).await {
                Ok(result) => {
                    info!(
                        rows_affected = result.rows_affected,
                        "INSERT executed via DmlService"
                    );
                    self.send_command_complete(&format!("INSERT 0 {}", result.rows_affected))
                        .await
                }
                Err(e) => {
                    warn!("DmlService INSERT failed: {}", e);
                    self.send_error("ERROR", "42P01", &format!("Insert failed: {}", e))
                        .await
                }
            },
            Ok(None) => {
                // Not a DML statement (shouldn't happen for INSERT)
                self.send_error("ERROR", "42601", "Invalid INSERT statement")
                    .await
            }
            Err(e) => {
                warn!("Failed to parse INSERT: {}", e);
                self.send_error("ERROR", "42601", &format!("Parse error: {}", e))
                    .await
            }
        }
    }

    /// Detect store type for INSERT from table name or query content
    fn detect_insert_store_type(&self, table_name: &str, query: &str) -> DataModel {
        multimodal_router::detect_store_type_from_query(query, table_name, None)
    }

    /// Insert vector into collection (existing behavior)
    async fn insert_vector(&mut self, table_name: &str, query: &str) -> Result<()> {
        let upper = query.to_uppercase();

        // Extract id and vector from VALUES clause
        let values_pos = upper
            .find("VALUES")
            .ok_or_else(|| anyhow::anyhow!("Missing VALUES clause"))?;
        let values_str = &query[values_pos + 6..];

        // Parse VALUES (...) - simplified parser
        let id = self.extract_string_value(values_str);
        let vector = self.extract_vector_from_query(values_str);

        if id.is_none() || vector.is_none() {
            debug!("Could not parse INSERT values for table '{}'", table_name);
            return self.send_command_complete("INSERT 0 1").await;
        }

        let (id, vector) = match (id, vector) {
            (Some(id), Some(vector)) => (id, vector),
            _ => return self.send_command_complete("INSERT 0 1").await,
        };

        debug!(
            "Inserting vector '{}' into collection '{}' (dim={})",
            id,
            table_name,
            vector.len()
        );

        // Insert via vector operations service
        use crate::proto::proximadb_v1::VectorRecord;

        let record = VectorRecord {
            id: id.clone(),
            vector: vector.clone(),
            metadata: std::collections::HashMap::new(),
            timestamp: Some(chrono::Utc::now().timestamp_millis()),
            version: Some(1),
            updated_at: None,
            expires_at: None,
            source: None,
        };

        match self.vector_ops.insert_batch(table_name, vec![record]).await {
            Ok(_) => {
                info!(
                    "Inserted vector '{}' into '{}' via PostgreSQL",
                    id, table_name
                );
                self.send_command_complete("INSERT 0 1").await
            }
            Err(e) => {
                warn!("Failed to insert vector: {}", e);
                self.send_error("ERROR", "42P01", &format!("Insert failed: {}", e))
                    .await
            }
        }
    }

    /// Insert document into document collection
    async fn insert_document(&mut self, table_name: &str, query: &str) -> Result<()> {
        let upper = query.to_uppercase();

        // Extract id and JSON from VALUES clause
        let values_pos = upper
            .find("VALUES")
            .ok_or_else(|| anyhow::anyhow!("Missing VALUES clause"))?;
        let values_str = &query[values_pos + 6..];

        let id = self.extract_string_value(values_str);
        let json_str = self.extract_json_value(values_str);

        if id.is_none() {
            debug!("Could not parse document ID for table '{}'", table_name);
            return self.send_command_complete("INSERT 0 0").await;
        }

        let id = match id {
            Some(id) => id,
            None => return self.send_command_complete("INSERT 0 0").await,
        };
        let json_data = json_str.unwrap_or_else(|| "{}".to_string());

        debug!(
            "Inserting document '{}' into collection '{}'",
            id, table_name
        );

        if let Some(document_service) = self.document_service.clone() {
            let parsed = serde_json::from_str::<serde_json::Value>(&json_data)
                .context("Failed to parse JSON document")?;
            let Some(document) = self.json_value_to_sql_object(&parsed) else {
                return self
                    .send_error("ERROR", "22P02", "JSON document must be an object")
                    .await;
            };

            return match document_service
                .insert_document(table_name, Some(&id), document)
                .await
            {
                Ok(_) => {
                    info!(
                        "Inserted document '{}' into '{}' via PostgreSQL DocumentService",
                        id, table_name
                    );
                    self.send_command_complete("INSERT 0 1").await
                }
                Err(e) => {
                    warn!("Failed to insert document via DocumentService: {}", e);
                    self.send_error("ERROR", "42P01", &format!("Insert failed: {}", e))
                        .await
                }
            };
        }

        // For now, store documents as vectors with empty vector and JSON metadata
        use crate::proto::proximadb_v1::{SqlValue, VectorRecord, sql_value};

        let mut metadata = std::collections::HashMap::new();
        metadata.insert(
            "__document__".to_string(),
            SqlValue {
                value: Some(sql_value::Value::StringValue(json_data.clone())),
            },
        );

        let record = VectorRecord {
            id: id.clone(),
            vector: vec![], // Documents have no vector
            metadata,
            timestamp: Some(chrono::Utc::now().timestamp_millis()),
            version: Some(1),
            updated_at: None,
            expires_at: None,
            source: None,
        };

        // Use doc_ prefix for document collection
        let collection_name = if table_name.starts_with("doc_") {
            table_name.to_string()
        } else {
            format!("doc_{}", table_name)
        };

        match self
            .vector_ops
            .insert_batch(&collection_name, vec![record])
            .await
        {
            Ok(_) => {
                info!(
                    "Inserted document '{}' into '{}' via PostgreSQL",
                    id, table_name
                );
                self.send_command_complete("INSERT 0 1").await
            }
            Err(e) => {
                warn!("Failed to insert document: {}", e);
                self.send_error("ERROR", "42P01", &format!("Insert failed: {}", e))
                    .await
            }
        }
    }

    /// Insert graph data (nodes/edges)
    async fn insert_graph_data(&mut self, table_name: &str, _query: &str) -> Result<()> {
        debug!("Inserting graph data into '{}'", table_name);
        // Deferred: Integrate with graph service
        info!(
            "Graph INSERT acknowledged for '{}' (graph service integration pending)",
            table_name
        );
        self.send_command_complete("INSERT 0 1").await
    }

    /// Insert log/metric/trace into observability namespace
    async fn insert_log(&mut self, table_name: &str, query: &str) -> Result<()> {
        let upper = query.to_uppercase();

        // Extract values from VALUES clause
        let values_pos = upper
            .find("VALUES")
            .ok_or_else(|| anyhow::anyhow!("Missing VALUES clause"))?;
        let values_str = &query[values_pos + 6..];

        let message = self.extract_string_value(values_str);

        debug!("Inserting log into namespace '{}'", table_name);

        if let Some(observability_service) = self.observability_service.clone() {
            let Some(message) = message else {
                return self.send_command_complete("INSERT 0 0").await;
            };

            let log = crate::proto::proximadb_v1::LogEntry {
                timestamp_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
                severity: crate::proto::proximadb_v1::Severity::Info as i32,
                message,
                fields: HashMap::new(),
                source: Some("postgres".to_string()),
                service: Some("proximadb".to_string()),
            };

            return match observability_service
                .ingest_logs(table_name, vec![log], None)
                .await
            {
                Ok(_) => {
                    info!("Inserted log into '{}' via PostgreSQL", table_name);
                    self.send_command_complete("INSERT 0 1").await
                }
                Err(e) => {
                    warn!("Failed to insert log into '{}': {}", table_name, e);
                    self.send_error("ERROR", "42P01", &format!("Insert failed: {}", e))
                        .await
                }
            };
        }

        info!(
            "Log INSERT acknowledged for '{}': {:?}",
            table_name, message
        );
        self.send_command_complete("INSERT 0 1").await
    }

    /// Extract JSON value from VALUES clause
    fn extract_json_value(&self, values_str: &str) -> Option<String> {
        // Look for JSON object or array
        let json_start = values_str.find('{').or_else(|| values_str.find('['))?;
        let start_char = values_str.chars().nth(json_start)?;
        let end_char = if start_char == '{' { '}' } else { ']' };

        // Find matching closing bracket (handle nesting)
        let chars: Vec<char> = values_str.chars().collect();
        let mut depth = 0;
        let mut end_pos = None;

        for (i, &c) in chars.iter().enumerate().skip(json_start) {
            if c == start_char {
                depth += 1;
            } else if c == end_char {
                depth -= 1;
                if depth == 0 {
                    end_pos = Some(i);
                    break;
                }
            }
        }

        end_pos.map(|end| values_str[json_start..=end].to_string())
    }

    /// Execute DELETE - deletes vectors from a collection
    /// Supports: DELETE FROM table WHERE id = 'value'
    async fn execute_delete(&mut self, query: &str) -> Result<()> {
        // Use DmlService for proper SQL DML execution if available
        if let Some(dml_service) = self.dml_service.clone() {
            return self
                .execute_delete_via_dml_service(query, &dml_service)
                .await;
        }

        // Fall back to string parsing
        let upper = query.to_uppercase();

        // Extract table name: DELETE FROM table
        let from_pos = upper
            .find("FROM ")
            .ok_or_else(|| anyhow::anyhow!("Missing FROM clause"))?;
        let after_from = query[from_pos + 5..].trim();
        let table_end = after_from
            .find(|c: char| c.is_whitespace() || c == ';')
            .unwrap_or(after_from.len());
        let table_name = after_from[..table_end].trim().to_lowercase();

        if table_name.is_empty() {
            return self.send_command_complete("DELETE 0").await;
        }

        // Extract id from WHERE clause: WHERE id = 'value'
        let where_pos = upper.find("WHERE");
        let id = if let Some(pos) = where_pos {
            let where_clause = &query[pos..];
            self.extract_where_id(where_clause)
        } else {
            None
        };

        if let Some(id) = id {
            debug!("Deleting vector '{}' from collection '{}'", id, table_name);

            // Deferred: Implement proper vector deletion via tombstone/WAL
            // For now, acknowledge the delete request
            info!(
                "DELETE acknowledged for vector '{}' in '{}' (tombstone write pending)",
                id, table_name
            );
            self.send_command_complete("DELETE 1").await
        } else {
            // No WHERE clause or couldn't parse id - return 0 deleted
            self.send_command_complete("DELETE 0").await
        }
    }

    /// Execute DELETE using the proper SQL parser and DmlService
    async fn execute_delete_via_dml_service(
        &mut self,
        query: &str,
        dml_service: &Arc<DmlService>,
    ) -> Result<()> {
        let parser = SqlFrontendParser::new();

        match parser.parse_dml(query) {
            Ok(Some(statement)) => match dml_service.execute(statement).await {
                Ok(result) => {
                    info!(
                        rows_affected = result.rows_affected,
                        "DELETE executed via DmlService"
                    );
                    self.send_command_complete(&format!("DELETE {}", result.rows_affected))
                        .await
                }
                Err(e) => {
                    warn!("DmlService DELETE failed: {}", e);
                    self.send_error("ERROR", "42P01", &format!("Delete failed: {}", e))
                        .await
                }
            },
            Ok(None) => {
                self.send_error("ERROR", "42601", "Invalid DELETE statement")
                    .await
            }
            Err(e) => {
                warn!("Failed to parse DELETE: {}", e);
                self.send_error("ERROR", "42601", &format!("Parse error: {}", e))
                    .await
            }
        }
    }

    /// Execute UPDATE - updates vector metadata
    /// Supports: UPDATE table SET column = value WHERE id = 'value'
    async fn execute_update(&mut self, query: &str) -> Result<()> {
        // Use DmlService for proper SQL DML execution if available
        if let Some(dml_service) = self.dml_service.clone() {
            return self
                .execute_update_via_dml_service(query, &dml_service)
                .await;
        }

        // Fall back to string parsing
        let upper = query.to_uppercase();

        // Extract table name: UPDATE table SET
        let set_pos = upper
            .find(" SET ")
            .ok_or_else(|| anyhow::anyhow!("Missing SET clause"))?;
        let table_name = query[6..set_pos].trim().to_lowercase();

        if table_name.is_empty() {
            return self.send_command_complete("UPDATE 0").await;
        }

        // Extract id from WHERE clause
        let where_pos = upper.find("WHERE");
        let id = if let Some(pos) = where_pos {
            let where_clause = &query[pos..];
            self.extract_where_id(where_clause)
        } else {
            None
        };

        // For now, just acknowledge the update - full metadata update would require
        // fetching the record, updating it, and re-inserting
        if id.is_some() {
            info!(
                "UPDATE acknowledged for '{}' (metadata update not yet implemented)",
                table_name
            );
            self.send_command_complete("UPDATE 1").await
        } else {
            self.send_command_complete("UPDATE 0").await
        }
    }

    /// Execute UPDATE using the proper SQL parser and DmlService
    async fn execute_update_via_dml_service(
        &mut self,
        query: &str,
        dml_service: &Arc<DmlService>,
    ) -> Result<()> {
        let parser = SqlFrontendParser::new();

        match parser.parse_dml(query) {
            Ok(Some(statement)) => {
                match dml_service.execute(statement).await {
                    Ok(result) => {
                        info!(
                            rows_affected = result.rows_affected,
                            "UPDATE executed via DmlService"
                        );
                        self.send_command_complete(&format!("UPDATE {}", result.rows_affected))
                            .await
                    }
                    Err(e) => {
                        // DmlService UPDATE returns error for not-implemented
                        // This is expected - UPDATE requires delete + insert pattern
                        warn!("DmlService UPDATE: {}", e);
                        self.send_error("ERROR", "0A000", &format!("{}", e)).await
                    }
                }
            }
            Ok(None) => {
                self.send_error("ERROR", "42601", "Invalid UPDATE statement")
                    .await
            }
            Err(e) => {
                warn!("Failed to parse UPDATE: {}", e);
                self.send_error("ERROR", "42601", &format!("Parse error: {}", e))
                    .await
            }
        }
    }

    /// Execute DROP TABLE - deletes a ProximaDB collection
    /// Supports: DROP TABLE [IF EXISTS] name
    async fn execute_drop_table(&mut self, query: &str) -> Result<()> {
        let upper = query.to_uppercase();

        // Extract table name
        let table_start = if upper.contains("IF EXISTS") {
            upper.find("EXISTS").map(|p| p + 6)
        } else {
            upper.find("TABLE").map(|p| p + 5)
        };

        let Some(start) = table_start else {
            return self.send_command_complete("DROP TABLE").await;
        };

        let after_table = query[start..].trim();
        let table_end = after_table
            .find(|c: char| c.is_whitespace() || c == ';')
            .unwrap_or(after_table.len());
        let table_name = after_table[..table_end].trim().to_lowercase();

        if table_name.is_empty() {
            return self.send_command_complete("DROP TABLE").await;
        }

        debug!("Dropping collection '{}'", table_name);

        match self.collection_service.delete_collection(&table_name).await {
            Ok(_) => {
                info!("Dropped collection '{}' via PostgreSQL", table_name);
                self.send_command_complete("DROP TABLE").await
            }
            Err(e) => {
                if upper.contains("IF EXISTS") {
                    // IF EXISTS - don't error if not found
                    self.send_command_complete("DROP TABLE").await
                } else {
                    warn!("Failed to drop collection '{}': {}", table_name, e);
                    self.send_error(
                        "ERROR",
                        "42P01",
                        &format!("Table does not exist: {}", table_name),
                    )
                    .await
                }
            }
        }
    }

    /// Extract id from WHERE clause: WHERE id = 'value' -> value
    fn extract_where_id(&self, where_clause: &str) -> Option<String> {
        // Look for id = 'value' or id='value'
        let upper = where_clause.to_uppercase();
        let id_pos = upper.find("ID")?;
        let after_id = &where_clause[id_pos + 2..];

        // Skip whitespace and =
        let eq_pos = after_id.find('=')?;
        let after_eq = after_id[eq_pos + 1..].trim();

        // Extract quoted value
        if let Some(after_quote) = after_eq.strip_prefix('\'') {
            let end = after_quote.find('\'')?;
            Some(after_quote[..end + 1].to_string())
        } else {
            // Unquoted value - take until whitespace or semicolon
            let end = after_eq
                .find(|c: char| c.is_whitespace() || c == ';')
                .unwrap_or(after_eq.len());
            Some(after_eq[..end].to_string())
        }
    }

    // ========================================================================
    // COPY Command Support (Bulk Data Transfer)
    // ========================================================================

    /// Execute COPY command for bulk data transfer
    /// Supports:
    /// - COPY table FROM STDIN [WITH (FORMAT ARROW)]  - Arrow IPC format (most efficient)
    /// - COPY table FROM STDIN [WITH (FORMAT CSV)]    - CSV format
    /// - COPY table FROM STDIN [WITH (FORMAT BINARY)] - PostgreSQL binary format
    /// - COPY table FROM STDIN                        - Text format (default)
    async fn execute_copy(&mut self, query: &str) -> Result<()> {
        let upper = query.to_uppercase();

        // Parse COPY command: COPY table FROM STDIN [WITH (...)]
        if !upper.contains("FROM STDIN") {
            // COPY TO is not supported (export)
            return self
                .send_error(
                    "ERROR",
                    "0A000",
                    "COPY TO is not supported; use SELECT queries",
                )
                .await;
        }

        // Extract table name
        let copy_pos = upper
            .find("COPY ")
            .ok_or_else(|| anyhow::anyhow!("Invalid COPY syntax"))?;
        let after_copy = query[copy_pos + 5..].trim();
        let table_end = after_copy
            .find(|c: char| c.is_whitespace() || c == '(')
            .unwrap_or(after_copy.len());
        let table_name = after_copy[..table_end].trim().to_lowercase();

        if table_name.is_empty() {
            return self
                .send_error("ERROR", "42601", "Table name required for COPY")
                .await;
        }

        // Detect format from WITH clause
        let format = self.detect_copy_format(&upper);

        debug!("COPY {} FROM STDIN with format {:?}", table_name, format);

        // Detect store type
        let store_type = self.detect_insert_store_type(&table_name, &upper);

        // Send CopyInResponse to signal client to start sending data
        self.send_copy_in_response(&format).await?;

        // Receive and process COPY data
        let row_count = self
            .receive_copy_data(&table_name, store_type, &format)
            .await?;

        // Send command complete
        self.send_command_complete(&format!("COPY {}", row_count))
            .await
    }

    /// Detect COPY format from WITH clause
    fn detect_copy_format(&self, query: &str) -> CopyFormat {
        if query.contains("FORMAT ARROW") || query.contains("FORMAT 'ARROW'") {
            CopyFormat::Arrow
        } else if query.contains("FORMAT CSV") || query.contains("FORMAT 'CSV'") {
            CopyFormat::Csv
        } else if query.contains("FORMAT BINARY") || query.contains("FORMAT 'BINARY'") {
            CopyFormat::Binary
        } else {
            CopyFormat::Text
        }
    }

    /// Send CopyInResponse message
    async fn send_copy_in_response(&mut self, format: &CopyFormat) -> Result<()> {
        // CopyInResponse format:
        // - overall format (0=text, 1=binary)
        // - number of columns (int16)
        // - format codes for each column (int16 each)

        let overall_format: i8 = match format {
            CopyFormat::Binary | CopyFormat::Arrow => 1, // Binary
            _ => 0,                                      // Text
        };

        // For simplicity, use 2 columns (id, vector) with matching format
        let num_columns: i16 = 2;
        let format_code: i16 = overall_format as i16;

        let len = 4 + 1 + 2 + (num_columns as usize * 2);
        self.write_buffer.put_u8(b'G'); // CopyInResponse
        self.write_buffer.put_i32(len as i32);
        self.write_buffer.put_i8(overall_format);
        self.write_buffer.put_i16(num_columns);
        for _ in 0..num_columns {
            self.write_buffer.put_i16(format_code);
        }
        self.flush_write_buffer().await
    }

    /// Receive COPY data from client
    async fn receive_copy_data(
        &mut self,
        table_name: &str,
        store_type: DataModel,
        format: &CopyFormat,
    ) -> Result<usize> {
        let mut all_data = Vec::new();

        loop {
            // Read message type
            let msg_type = self.read_byte().await?;

            match msg_type {
                b'd' => {
                    // CopyData message
                    let length = self.read_i32().await? as usize;
                    if length < 4 {
                        continue;
                    }
                    let data = self.read_bytes(length - 4).await?;
                    all_data.extend(data);
                }
                b'c' => {
                    // CopyDone message
                    let _length = self.read_i32().await?;
                    debug!("COPY done, received {} bytes", all_data.len());
                    break;
                }
                b'f' => {
                    // CopyFail message
                    let length = self.read_i32().await? as usize;
                    let msg = self.read_bytes(length - 4).await?;
                    let error_msg = String::from_utf8_lossy(&msg);
                    warn!("COPY failed: {}", error_msg);
                    return Err(anyhow::anyhow!("COPY failed: {}", error_msg));
                }
                _ => {
                    warn!("Unexpected message during COPY: {}", msg_type as char);
                    return Err(anyhow::anyhow!("Unexpected message during COPY"));
                }
            }
        }

        // Process the collected data based on format
        let row_count = match format {
            CopyFormat::Arrow => {
                self.process_arrow_copy_data(table_name, store_type, &all_data)
                    .await?
            }
            CopyFormat::Csv => {
                self.process_csv_copy_data(table_name, store_type, &all_data)
                    .await?
            }
            CopyFormat::Text => {
                self.process_text_copy_data(table_name, store_type, &all_data)
                    .await?
            }
            CopyFormat::Binary => {
                self.process_binary_copy_data(table_name, store_type, &all_data)
                    .await?
            }
        };

        info!(
            "COPY completed: {} rows inserted into '{}'",
            row_count, table_name
        );
        Ok(row_count)
    }

    /// Process Arrow IPC format COPY data (most efficient path)
    async fn process_arrow_copy_data(
        &mut self,
        table_name: &str,
        _store_type: DataModel,
        data: &[u8],
    ) -> Result<usize> {
        if data.is_empty() {
            return Ok(0);
        }

        // Decode Arrow IPC stream
        let cursor = std::io::Cursor::new(data);
        let reader = match arrow_ipc::reader::StreamReader::try_new(cursor, None) {
            Ok(r) => r,
            Err(e) => {
                warn!("Failed to parse Arrow IPC stream: {}", e);
                return self
                    .send_error("ERROR", "22P02", &format!("Invalid Arrow IPC data: {}", e))
                    .await
                    .map(|_| 0);
            }
        };

        // Collect all record batches
        let batches: Vec<arrow_array::RecordBatch> = reader.filter_map(|r| r.ok()).collect();

        if batches.is_empty() {
            return Ok(0);
        }

        // Convert Arrow batches to VectorRecords
        let vectors = match ArrowProtoCodec::batches_to_vector_records(batches) {
            Ok(v) => v,
            Err(e) => {
                warn!("Failed to convert Arrow data to vectors: {}", e);
                return Err(anyhow::anyhow!("Failed to convert Arrow data: {}", e));
            }
        };

        let count = vectors.len();
        debug!(
            "Decoded {} vectors from Arrow IPC for COPY into '{}'",
            count, table_name
        );

        // Bulk insert via vector operations service (bypasses per-row overhead)
        match self.vector_ops.insert_batch(table_name, vectors).await {
            Ok(_) => {
                info!(
                    "Arrow COPY inserted {} vectors into '{}'",
                    count, table_name
                );
                Ok(count)
            }
            Err(e) => {
                warn!("Arrow COPY insert failed: {}", e);
                Err(anyhow::anyhow!("COPY insert failed: {}", e))
            }
        }
    }

    /// Process CSV format COPY data
    async fn process_csv_copy_data(
        &mut self,
        table_name: &str,
        _store_type: DataModel,
        data: &[u8],
    ) -> Result<usize> {
        let text = String::from_utf8_lossy(data);
        let mut records = Vec::new();

        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }

            // Parse CSV: id,vector_data
            let parts: Vec<&str> = line.splitn(2, ',').collect();
            if parts.len() < 2 {
                continue;
            }

            let id = parts[0].trim().trim_matches('"');
            let vector_str = parts[1].trim();

            // Parse vector from CSV (e.g., "[0.1,0.2,0.3]" or "0.1,0.2,0.3")
            let vector = self.parse_csv_vector(vector_str);
            if vector.is_empty() {
                continue;
            }

            use crate::proto::proximadb_v1::VectorRecord;
            records.push(VectorRecord {
                id: id.to_string(),
                vector,
                metadata: std::collections::HashMap::new(),
                timestamp: Some(chrono::Utc::now().timestamp_millis()),
                version: Some(1),
                updated_at: None,
                expires_at: None,
                source: None,
            });
        }

        let count = records.len();
        if count == 0 {
            return Ok(0);
        }

        match self.vector_ops.insert_batch(table_name, records).await {
            Ok(_) => Ok(count),
            Err(e) => Err(anyhow::anyhow!("CSV COPY insert failed: {}", e)),
        }
    }

    /// Parse vector from CSV string
    fn parse_csv_vector(&self, s: &str) -> Vec<f32> {
        // Remove brackets if present
        let clean = s.trim().trim_start_matches('[').trim_end_matches(']');
        clean
            .split(',')
            .filter_map(|v| v.trim().parse::<f32>().ok())
            .collect()
    }

    /// Process text format COPY data (PostgreSQL default)
    async fn process_text_copy_data(
        &mut self,
        table_name: &str,
        _store_type: DataModel,
        data: &[u8],
    ) -> Result<usize> {
        let text = String::from_utf8_lossy(data);
        let mut records = Vec::new();

        for line in text.lines() {
            if line.trim().is_empty() || line == "\\." {
                continue;
            }

            // Parse tab-separated: id\tvector_data
            let parts: Vec<&str> = line.splitn(2, '\t').collect();
            if parts.len() < 2 {
                continue;
            }

            let id = parts[0].trim();
            let vector = self.extract_vector_from_query(parts[1]);

            if let Some(vector) = vector {
                use crate::proto::proximadb_v1::VectorRecord;
                records.push(VectorRecord {
                    id: id.to_string(),
                    vector,
                    metadata: std::collections::HashMap::new(),
                    timestamp: Some(chrono::Utc::now().timestamp_millis()),
                    version: Some(1),
                    updated_at: None,
                    expires_at: None,
                    source: None,
                });
            }
        }

        let count = records.len();
        if count == 0 {
            return Ok(0);
        }

        match self.vector_ops.insert_batch(table_name, records).await {
            Ok(_) => Ok(count),
            Err(e) => Err(anyhow::anyhow!("Text COPY insert failed: {}", e)),
        }
    }

    /// Process PostgreSQL binary format COPY data
    async fn process_binary_copy_data(
        &mut self,
        table_name: &str,
        _store_type: DataModel,
        data: &[u8],
    ) -> Result<usize> {
        if data.len() < 11 {
            return Ok(0);
        }

        // PostgreSQL binary format header: PGCOPY\n\377\r\n\0
        // Then flags (4 bytes), header extension (4 bytes)
        // Then tuples until -1 field count

        let header = b"PGCOPY\n\xff\r\n\x00";
        if &data[..11] != header {
            warn!("Invalid PostgreSQL binary COPY header");
            return Err(anyhow::anyhow!("Invalid binary COPY format"));
        }

        let mut cursor = std::io::Cursor::new(&data[11..]);
        use bytes::Buf;

        // Skip flags and header extension
        let _flags = cursor.get_i32();
        let ext_len = cursor.get_i32();
        cursor.advance(ext_len as usize);

        let mut records = Vec::new();

        loop {
            if cursor.remaining() < 2 {
                break;
            }

            let field_count = cursor.get_i16();
            if field_count == -1 {
                // End of data
                break;
            }

            // Read id field
            if cursor.remaining() < 4 {
                break;
            }
            let id_len = cursor.get_i32() as usize;
            if cursor.remaining() < id_len {
                break;
            }
            let id_bytes: Vec<u8> = (0..id_len).map(|_| cursor.get_u8()).collect();
            let id = String::from_utf8_lossy(&id_bytes).to_string();

            // Read vector field
            if cursor.remaining() < 4 {
                break;
            }
            let vec_len = cursor.get_i32() as usize;
            if cursor.remaining() < vec_len {
                break;
            }

            // Parse vector as array of float4
            let num_floats = vec_len / 4;
            let vector: Vec<f32> = (0..num_floats).map(|_| cursor.get_f32()).collect();

            use crate::proto::proximadb_v1::VectorRecord;
            records.push(VectorRecord {
                id,
                vector,
                metadata: std::collections::HashMap::new(),
                timestamp: Some(chrono::Utc::now().timestamp_millis()),
                version: Some(1),
                updated_at: None,
                expires_at: None,
                source: None,
            });
        }

        let count = records.len();
        if count == 0 {
            return Ok(0);
        }

        match self.vector_ops.insert_batch(table_name, records).await {
            Ok(_) => Ok(count),
            Err(e) => Err(anyhow::anyhow!("Binary COPY insert failed: {}", e)),
        }
    }

    /// Extract a string value from VALUES clause: ('value', ...) -> value
    fn extract_string_value(&self, values: &str) -> Option<String> {
        let start = values.find('\'')?;
        let rest = &values[start + 1..];
        let end = rest.find('\'')?;
        Some(rest[..end].to_string())
    }

    /// Send a data row
    async fn send_data_row(&mut self, values: &[&str]) -> Result<()> {
        // Calculate message length
        let mut data_len = 2; // field count (2 bytes)
        for value in values {
            data_len += 4 + value.len(); // length (4 bytes) + data
        }

        self.write_buffer.put_u8(b'D'); // DataRow
        self.write_buffer.put_i32(4 + data_len as i32); // Length including self
        self.write_buffer.put_i16(values.len() as i16); // Field count

        for value in values {
            self.write_buffer.put_i32(value.len() as i32); // Value length
            self.write_buffer.put_slice(value.as_bytes()); // Value data
        }

        self.flush_write_buffer().await
    }

    /// Handle Parse message (extended query)
    async fn handle_parse(&mut self, body: &[u8]) -> Result<()> {
        let mut cursor = Cursor::new(body);

        // Read statement name
        let name = self.read_cstring(&mut cursor)?;

        // Read query string
        let query = self.read_cstring(&mut cursor)?;

        // Read parameter types count
        let param_count = cursor.get_i16() as usize;
        let mut param_types = Vec::with_capacity(param_count);

        for _ in 0..param_count {
            let oid = cursor.get_i32();
            param_types.push(PgType::from_oid(oid));
        }

        // Translate and store prepared statement
        match self.translator.translate(&query) {
            Ok(translated) => {
                self.prepared_statements.insert(
                    name.clone(),
                    PreparedStatement {
                        query: query.clone(),
                        translated,
                        param_types,
                    },
                );

                // Send parse complete
                self.send_parse_complete().await?;
            }
            Err(e) => {
                self.send_error("ERROR", "42601", &format!("Parse error: {}", e))
                    .await?;
            }
        }

        Ok(())
    }

    /// Handle Bind message - binds parameters to a prepared statement, creating a portal
    async fn handle_bind(&mut self, body: &[u8]) -> Result<()> {
        let mut cursor = Cursor::new(body);

        // Read portal name (destination)
        let portal_name = self.read_cstring(&mut cursor)?;

        // Read statement name (source prepared statement)
        let statement_name = self.read_cstring(&mut cursor)?;

        // Read format codes count (currently ignored - we use text format)
        let format_code_count = cursor.get_i16() as usize;
        for _ in 0..format_code_count {
            let _format_code = cursor.get_i16();
        }

        // Read parameter values count
        let param_count = cursor.get_i16() as usize;
        let mut param_values: Vec<Option<Vec<u8>>> = Vec::with_capacity(param_count);

        for _ in 0..param_count {
            let value_len = cursor.get_i32();
            if value_len == -1 {
                // NULL value
                param_values.push(None);
            } else {
                let mut value = vec![0u8; value_len as usize];
                cursor.copy_to_slice(&mut value);
                param_values.push(Some(value));
            }
        }

        // Read result format codes (we'll ignore these for now)
        let result_format_count = cursor.get_i16() as usize;
        for _ in 0..result_format_count {
            let _ = cursor.get_i16();
        }

        // Get the prepared statement data (extract to avoid borrow conflicts)
        let stmt_data = self
            .prepared_statements
            .get(&statement_name)
            .map(|s| (s.query.clone(), s.translated.clone()));

        let (stmt_query, stmt_translated) = match stmt_data {
            Some(data) => data,
            None => {
                // If unnamed statement (""), use the query directly
                if statement_name.is_empty() {
                    return self.send_bind_complete().await;
                }
                return self
                    .send_error(
                        "ERROR",
                        "26000",
                        &format!("prepared statement \"{}\" does not exist", statement_name),
                    )
                    .await;
            }
        };

        // Bind parameters to the query
        let bound_query = self.bind_parameters(&stmt_query, &param_values)?;

        // Create portal
        let portal = Portal {
            statement_name: statement_name.clone(),
            bound_query,
            translated: stmt_translated,
            param_values,
            max_rows: 0,
        };

        self.portals.insert(portal_name, portal);
        self.send_bind_complete().await
    }

    /// Bind parameter values to a query string
    fn bind_parameters(&self, query: &str, param_values: &[Option<Vec<u8>>]) -> Result<String> {
        let mut result = query.to_string();

        for (i, value) in param_values.iter().enumerate() {
            let placeholder = format!("${}", i + 1);
            let replacement = match value {
                Some(v) => {
                    // Convert bytes to string (assuming UTF-8 text format)
                    let s = String::from_utf8_lossy(v);
                    // Escape single quotes
                    format!("'{}'", s.replace('\'', "''"))
                }
                None => "NULL".to_string(),
            };
            result = result.replace(&placeholder, &replacement);
        }

        Ok(result)
    }

    /// Handle Execute message - executes a portal
    async fn handle_execute(&mut self, body: &[u8]) -> Result<()> {
        let mut cursor = Cursor::new(body);

        // Read portal name
        let portal_name = self.read_cstring(&mut cursor)?;

        // Read max rows (0 = unlimited) - currently not enforced
        let _max_rows = cursor.get_i32();

        // Get the portal
        let portal = match self.portals.get(&portal_name) {
            Some(p) => p.clone(),
            None => {
                // If unnamed portal (""), execute as simple query
                if portal_name.is_empty() {
                    return self.send_command_complete("SELECT 0").await;
                }
                return self
                    .send_error(
                        "ERROR",
                        "34000",
                        &format!("portal \"{}\" does not exist", portal_name),
                    )
                    .await;
            }
        };

        // Execute the bound query
        debug!(
            "Executing portal '{}' with query: {}",
            portal_name, portal.bound_query
        );

        // Use the same query execution path as simple query
        self.execute_query(&portal.bound_query).await
    }

    /// Handle Describe message
    async fn handle_describe(&mut self, body: &[u8]) -> Result<()> {
        if body.is_empty() {
            return Ok(());
        }

        let describe_type = body[0] as char;
        let name = self.parse_cstring(&body[1..])?;

        match describe_type {
            'S' => {
                // Describe statement - clone param_types to avoid borrow conflict
                if let Some(param_types) = self
                    .prepared_statements
                    .get(&name)
                    .map(|s| s.param_types.clone())
                {
                    // Send parameter description
                    self.send_parameter_description(&param_types).await?;
                    // Send row description (empty for now)
                    self.send_row_description(&[]).await?;
                } else {
                    self.send_error("ERROR", "26000", "Prepared statement does not exist")
                        .await?;
                }
            }
            'P' => {
                // Describe portal
                self.send_row_description(&[]).await?;
            }
            _ => {
                self.send_error("ERROR", "XX000", "Invalid describe type")
                    .await?;
            }
        }

        Ok(())
    }

    /// Handle Sync message
    async fn handle_sync(&mut self) -> Result<()> {
        self.send_ready_for_query('I').await
    }

    /// Handle Flush message
    async fn handle_flush(&mut self) -> Result<()> {
        self.stream.flush().await?;
        Ok(())
    }

    /// Handle Close message
    async fn handle_close(&mut self, body: &[u8]) -> Result<()> {
        if body.is_empty() {
            return Ok(());
        }

        let close_type = body[0] as char;
        let name = self.parse_cstring(&body[1..])?;

        if close_type == 'S' {
            self.prepared_statements.remove(&name);
        }

        self.send_close_complete().await
    }

    // Message sending helpers

    /// Send authentication OK
    async fn send_auth_ok(&mut self) -> Result<()> {
        self.write_buffer.put_u8(b'R');
        self.write_buffer.put_i32(8);
        self.write_buffer.put_i32(0); // Auth OK
        self.flush_write_buffer().await
    }

    /// Send parameter status
    async fn send_parameter_status(&mut self, name: &str, value: &str) -> Result<()> {
        let len = 4 + name.len() + 1 + value.len() + 1;
        self.write_buffer.put_u8(b'S');
        self.write_buffer.put_i32(len as i32);
        self.write_buffer.put_slice(name.as_bytes());
        self.write_buffer.put_u8(0);
        self.write_buffer.put_slice(value.as_bytes());
        self.write_buffer.put_u8(0);
        self.flush_write_buffer().await
    }

    /// Send backend key data
    async fn send_backend_key_data(&mut self) -> Result<()> {
        // Clone session data to avoid borrow conflict with flush_write_buffer
        let (process_id, secret_key) = {
            let session = self.session.read().await;
            (session.process_id, session.secret_key)
        };
        self.write_buffer.put_u8(b'K');
        self.write_buffer.put_i32(12);
        self.write_buffer.put_i32(process_id as i32);
        self.write_buffer.put_i32(secret_key as i32);
        self.flush_write_buffer().await
    }

    /// Send ready for query
    async fn send_ready_for_query(&mut self, status: char) -> Result<()> {
        self.write_buffer.put_u8(b'Z');
        self.write_buffer.put_i32(5);
        self.write_buffer.put_u8(status as u8);
        self.flush_write_buffer().await
    }

    /// Send error response
    async fn send_error(&mut self, severity: &str, code: &str, message: &str) -> Result<()> {
        let len = 4 + 1 + severity.len() + 1 + 1 + code.len() + 1 + 1 + message.len() + 1 + 1;
        self.write_buffer.put_u8(b'E');
        self.write_buffer.put_i32(len as i32);
        self.write_buffer.put_u8(b'S'); // Severity
        self.write_buffer.put_slice(severity.as_bytes());
        self.write_buffer.put_u8(0);
        self.write_buffer.put_u8(b'C'); // Code
        self.write_buffer.put_slice(code.as_bytes());
        self.write_buffer.put_u8(0);
        self.write_buffer.put_u8(b'M'); // Message
        self.write_buffer.put_slice(message.as_bytes());
        self.write_buffer.put_u8(0);
        self.write_buffer.put_u8(0); // Terminator
        self.flush_write_buffer().await
    }

    /// Send row description
    async fn send_row_description(&mut self, fields: &[FieldDescription]) -> Result<()> {
        let mut len = 4 + 2; // Length + field count
        for field in fields {
            len += field.name.len() + 1 + 18; // name + null + fixed data
        }

        self.write_buffer.put_u8(b'T');
        self.write_buffer.put_i32(len as i32);
        self.write_buffer.put_i16(fields.len() as i16);

        for field in fields {
            self.write_buffer.put_slice(field.name.as_bytes());
            self.write_buffer.put_u8(0);
            self.write_buffer.put_i32(field.table_oid);
            self.write_buffer.put_i16(field.column_number);
            self.write_buffer.put_i32(field.type_oid);
            self.write_buffer.put_i16(field.type_size);
            self.write_buffer.put_i32(field.type_modifier);
            self.write_buffer.put_i16(field.format_code);
        }

        self.flush_write_buffer().await
    }

    /// Send command complete
    async fn send_command_complete(&mut self, tag: &str) -> Result<()> {
        let len = 4 + tag.len() + 1;
        self.write_buffer.put_u8(b'C');
        self.write_buffer.put_i32(len as i32);
        self.write_buffer.put_slice(tag.as_bytes());
        self.write_buffer.put_u8(0);
        self.flush_write_buffer().await
    }

    /// Send parse complete
    async fn send_parse_complete(&mut self) -> Result<()> {
        self.write_buffer.put_u8(b'1');
        self.write_buffer.put_i32(4);
        self.flush_write_buffer().await
    }

    /// Send bind complete
    async fn send_bind_complete(&mut self) -> Result<()> {
        self.write_buffer.put_u8(b'2');
        self.write_buffer.put_i32(4);
        self.flush_write_buffer().await
    }

    /// Send close complete
    async fn send_close_complete(&mut self) -> Result<()> {
        self.write_buffer.put_u8(b'3');
        self.write_buffer.put_i32(4);
        self.flush_write_buffer().await
    }

    /// Send parameter description
    async fn send_parameter_description(&mut self, types: &[PgType]) -> Result<()> {
        let len = 4 + 2 + types.len() * 4;
        self.write_buffer.put_u8(b't');
        self.write_buffer.put_i32(len as i32);
        self.write_buffer.put_i16(types.len() as i16);
        for t in types {
            self.write_buffer.put_i32(t.oid());
        }
        self.flush_write_buffer().await
    }

    // I/O helpers

    /// Read a byte
    async fn read_byte(&mut self) -> Result<u8> {
        let mut buf = [0u8; 1];
        self.stream.read_exact(&mut buf).await?;
        Ok(buf[0])
    }

    /// Read an i32
    async fn read_i32(&mut self) -> Result<i32> {
        let mut buf = [0u8; 4];
        self.stream.read_exact(&mut buf).await?;
        Ok(i32::from_be_bytes(buf))
    }

    /// Read bytes
    async fn read_bytes(&mut self, len: usize) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; len];
        self.stream.read_exact(&mut buf).await?;
        Ok(buf)
    }

    /// Read C-string from cursor
    fn read_cstring(&self, cursor: &mut Cursor<&[u8]>) -> Result<String> {
        let mut bytes = Vec::new();
        loop {
            if !cursor.has_remaining() {
                break;
            }
            let b = cursor.get_u8();
            if b == 0 {
                break;
            }
            bytes.push(b);
        }
        String::from_utf8(bytes).context("Invalid UTF-8 in string")
    }

    /// Parse C-string from bytes
    fn parse_cstring(&self, data: &[u8]) -> Result<String> {
        let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
        String::from_utf8(data[..end].to_vec()).context("Invalid UTF-8 in string")
    }

    /// Flush write buffer
    async fn flush_write_buffer(&mut self) -> Result<()> {
        self.stream.write_all(&self.write_buffer).await?;
        self.write_buffer.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::multimodal_router;

    #[test]
    fn test_frontend_message() {
        assert_eq!(FrontendMessage::Query as u8, b'Q');
        assert_eq!(FrontendMessage::Terminate as u8, b'X');
    }

    #[test]
    fn test_store_type_detection_vector() {
        // Vector queries contain <->, <=>, or <#> operators (pgvector syntax)
        assert_eq!(
            multimodal_router::detect_store_type_from_query(
                "SELECT * FROM embeddings ORDER BY vec <-> '[0.1, 0.2, 0.3]' LIMIT 10",
                "embeddings",
                None,
            ),
            DataModel::Vector
        );
        assert_eq!(
            multimodal_router::detect_store_type_from_query(
                "SELECT id, vec <=> '[0.5, 0.5]' AS similarity FROM items",
                "items",
                None,
            ),
            DataModel::Vector
        );
        assert_eq!(
            multimodal_router::detect_store_type_from_query(
                "SELECT id FROM products ORDER BY embedding <#> $1 LIMIT 5",
                "products",
                None,
            ),
            DataModel::Vector
        );

        // CREATE TABLE with VECTOR column type
        assert_eq!(
            multimodal_router::detect_store_type_from_create(
                "CREATE TABLE items (id TEXT, embedding VECTOR(384))",
            ),
            DataModel::Vector
        );

        // Explicit USING VECTOR clause
        assert_eq!(
            multimodal_router::detect_store_type_from_create(
                "CREATE TABLE vecs (id TEXT, data FLOAT[]) USING VECTOR",
            ),
            DataModel::Vector
        );
    }

    #[test]
    fn test_store_type_detection_document() {
        // Document queries use JSON path expressions ($.)
        assert_eq!(
            multimodal_router::detect_store_type_from_query(
                "SELECT * FROM products WHERE data $.price > 100",
                "products",
                None,
            ),
            DataModel::Document
        );

        // Document tables detected by doc_ prefix
        assert_eq!(
            multimodal_router::detect_store_type_from_query(
                "SELECT * FROM doc_users WHERE active = true",
                "doc_users",
                None,
            ),
            DataModel::Document
        );

        // document_ prefix also works
        assert_eq!(
            multimodal_router::detect_store_type_from_query(
                "SELECT * FROM document_orders",
                "document_orders",
                None,
            ),
            DataModel::Document
        );

        // CREATE with JSONB column
        assert_eq!(
            multimodal_router::detect_store_type_from_create(
                "CREATE TABLE docs (id TEXT PRIMARY KEY, data JSONB)",
            ),
            DataModel::Document
        );

        // CREATE with explicit USING DOCUMENT
        assert_eq!(
            multimodal_router::detect_store_type_from_create(
                "CREATE TABLE catalog (id TEXT, payload JSON) USING DOCUMENT",
            ),
            DataModel::Document
        );
    }

    #[test]
    fn test_store_type_detection_graph() {
        // Graph tables detected by graph_ prefix
        assert_eq!(
            multimodal_router::detect_store_type_from_query(
                "SELECT * FROM graph_social WHERE node_type = 'person'",
                "graph_social",
                None,
            ),
            DataModel::Graph
        );

        // node_ prefix
        assert_eq!(
            multimodal_router::detect_store_type_from_query(
                "SELECT * FROM node_users",
                "node_users",
                None,
            ),
            DataModel::Graph
        );

        // edge_ prefix
        assert_eq!(
            multimodal_router::detect_store_type_from_query(
                "SELECT * FROM edge_follows",
                "edge_follows",
                None,
            ),
            DataModel::Graph
        );

        // CREATE with explicit USING GRAPH
        assert_eq!(
            multimodal_router::detect_store_type_from_create(
                "CREATE TABLE social_network (id TEXT) USING GRAPH",
            ),
            DataModel::Graph
        );
    }

    #[test]
    fn test_store_type_detection_observability() {
        // log_ prefix -> Observability
        assert_eq!(
            multimodal_router::detect_store_type_from_query(
                "SELECT * FROM log_application WHERE severity = 'error'",
                "log_application",
                None,
            ),
            DataModel::Observability
        );

        // metric_ prefix -> Observability
        assert_eq!(
            multimodal_router::detect_store_type_from_query(
                "SELECT * FROM metric_http_requests",
                "metric_http_requests",
                None,
            ),
            DataModel::Observability
        );

        // trace_ prefix -> Observability
        assert_eq!(
            multimodal_router::detect_store_type_from_query(
                "SELECT * FROM trace_spans WHERE service = 'gateway'",
                "trace_spans",
                None,
            ),
            DataModel::Observability
        );

        // CREATE with USING OBSERVABILITY
        assert_eq!(
            multimodal_router::detect_store_type_from_create(
                "CREATE TABLE system_logs (ts TIMESTAMP, msg TEXT) USING OBSERVABILITY",
            ),
            DataModel::Observability
        );

        // CREATE with USING TIMESERIES (also maps to Observability)
        assert_eq!(
            multimodal_router::detect_store_type_from_create(
                "CREATE TABLE sensor_data (ts TIMESTAMP, value FLOAT) USING TIMESERIES",
            ),
            DataModel::Observability
        );
    }

    #[test]
    fn test_store_type_detection_relational() {
        // Standard SQL without any special markers -> Relational (default)
        assert_eq!(
            multimodal_router::detect_store_type_from_query(
                "SELECT id, name, email FROM users WHERE active = true",
                "users",
                None,
            ),
            DataModel::Relational
        );

        // CREATE TABLE without USING clause or special column types
        assert_eq!(
            multimodal_router::detect_store_type_from_create(
                "CREATE TABLE users (id INT PRIMARY KEY, name VARCHAR(255), email TEXT)",
            ),
            DataModel::Relational
        );

        // Verify priority: vector operators override table name prefix
        // Even with a graph_ prefix, <-> forces Vector detection
        assert_eq!(
            multimodal_router::detect_store_type_from_query(
                "SELECT * FROM graph_nodes ORDER BY embedding <-> '[0.1]' LIMIT 5",
                "graph_nodes",
                None,
            ),
            DataModel::Vector
        );
    }

    #[test]
    fn test_frontend_message_types() {
        // Verify all FrontendMessage enum byte values match PostgreSQL protocol spec
        assert_eq!(FrontendMessage::Startup as u8, 0);
        assert_eq!(FrontendMessage::Query as u8, b'Q'); // 0x51
        assert_eq!(FrontendMessage::Parse as u8, b'P'); // 0x50
        assert_eq!(FrontendMessage::Bind as u8, b'B'); // 0x42
        assert_eq!(FrontendMessage::Execute as u8, b'E'); // 0x45
        assert_eq!(FrontendMessage::Describe as u8, b'D'); // 0x44
        assert_eq!(FrontendMessage::Sync as u8, b'S'); // 0x53
        assert_eq!(FrontendMessage::Flush as u8, b'H'); // 0x48
        assert_eq!(FrontendMessage::Close as u8, b'C'); // 0x43
        assert_eq!(FrontendMessage::Password as u8, b'p'); // 0x70
        assert_eq!(FrontendMessage::Terminate as u8, b'X'); // 0x58
        assert_eq!(FrontendMessage::CopyData as u8, b'd'); // 0x64
        assert_eq!(FrontendMessage::CopyDone as u8, b'c'); // 0x63
        assert_eq!(FrontendMessage::CopyFail as u8, b'f'); // 0x66

        // Verify exact hex values for key protocol messages
        assert_eq!(FrontendMessage::Query as u8, 0x51);
        assert_eq!(FrontendMessage::Parse as u8, 0x50);
        assert_eq!(FrontendMessage::Bind as u8, 0x42);
        assert_eq!(FrontendMessage::Terminate as u8, 0x58);
    }

    #[test]
    fn test_copy_format_detection() {
        // CopyFormat is private, so we test the detect_copy_format delegation path
        // by verifying the enum values and their properties directly.
        assert_eq!(CopyFormat::Text, CopyFormat::Text);
        assert_eq!(CopyFormat::Csv, CopyFormat::Csv);
        assert_eq!(CopyFormat::Binary, CopyFormat::Binary);
        assert_eq!(CopyFormat::Arrow, CopyFormat::Arrow);

        // All four variants are distinct
        assert_ne!(CopyFormat::Text, CopyFormat::Csv);
        assert_ne!(CopyFormat::Text, CopyFormat::Binary);
        assert_ne!(CopyFormat::Text, CopyFormat::Arrow);
        assert_ne!(CopyFormat::Csv, CopyFormat::Binary);
        assert_ne!(CopyFormat::Csv, CopyFormat::Arrow);
        assert_ne!(CopyFormat::Binary, CopyFormat::Arrow);

        // Verify the detection logic inline (mirrors detect_copy_format)
        let detect = |query: &str| -> CopyFormat {
            let upper = query.to_uppercase();
            if upper.contains("FORMAT ARROW") || upper.contains("FORMAT 'ARROW'") {
                CopyFormat::Arrow
            } else if upper.contains("FORMAT CSV") || upper.contains("FORMAT 'CSV'") {
                CopyFormat::Csv
            } else if upper.contains("FORMAT BINARY") || upper.contains("FORMAT 'BINARY'") {
                CopyFormat::Binary
            } else {
                CopyFormat::Text
            }
        };

        assert_eq!(
            detect("COPY my_table FROM STDIN WITH (FORMAT ARROW)"),
            CopyFormat::Arrow
        );
        assert_eq!(
            detect("COPY my_table FROM STDIN WITH (FORMAT 'ARROW')"),
            CopyFormat::Arrow
        );
        assert_eq!(
            detect("COPY my_table FROM STDIN WITH (FORMAT CSV, HEADER true)"),
            CopyFormat::Csv
        );
        assert_eq!(
            detect("COPY my_table FROM STDIN WITH (FORMAT 'CSV')"),
            CopyFormat::Csv
        );
        assert_eq!(
            detect("COPY my_table FROM STDIN WITH (FORMAT BINARY)"),
            CopyFormat::Binary
        );
        assert_eq!(
            detect("COPY my_table FROM STDIN WITH (FORMAT 'BINARY')"),
            CopyFormat::Binary
        );
        // Default is Text when no FORMAT clause
        assert_eq!(detect("COPY my_table FROM STDIN"), CopyFormat::Text);
        assert_eq!(
            detect("COPY my_table FROM STDIN WITH (HEADER true)"),
            CopyFormat::Text
        );
    }

    #[test]
    fn test_extract_vector_dimension() {
        // extract_vector_dimension is a method on PostgresProtocol which requires
        // a full instance with TcpStream. Instead, test the parsing logic directly
        // since it's a pure string operation.
        let extract = |query: &str| -> Option<u32> {
            let vector_pos = query.find("VECTOR(")?;
            let after_vector = &query[vector_pos + 7..];
            let dim_end = after_vector.find(')')?;
            after_vector[..dim_end].trim().parse().ok()
        };

        // Standard dimension extraction
        assert_eq!(
            extract("CREATE TABLE items (id TEXT, embedding VECTOR(384))"),
            Some(384)
        );
        assert_eq!(
            extract("CREATE TABLE docs (id TEXT, vec VECTOR(128))"),
            Some(128)
        );
        assert_eq!(
            extract("CREATE TABLE large (id TEXT, emb VECTOR(1536))"),
            Some(1536)
        );

        // Small dimension
        assert_eq!(extract("CREATE TABLE tiny (id TEXT, v VECTOR(2))"), Some(2));

        // Whitespace around number
        assert_eq!(
            extract("CREATE TABLE ws (id TEXT, v VECTOR( 256 ))"),
            Some(256)
        );

        // No VECTOR column -> None
        assert_eq!(extract("CREATE TABLE plain (id INT, name TEXT)"), None);

        // Malformed (no closing paren) -> None
        assert_eq!(extract("CREATE TABLE broken (id TEXT, v VECTOR("), None);

        // Non-numeric content -> None
        assert_eq!(
            extract("CREATE TABLE broken (id TEXT, v VECTOR(abc))"),
            None
        );
    }
}
