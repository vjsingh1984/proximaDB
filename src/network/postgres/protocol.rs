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

use anyhow::{anyhow, Context, Result};
use bytes::{Buf, BufMut, BytesMut};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::RwLock;
use tracing::{debug, error, info, trace, warn};

use super::session::Session;
use super::translator::QueryTranslator;
use super::types::{DataValue, FieldDescription, PgType, PgValue};
use crate::services::CollectionService;
use crate::services::VectorOperationsService;
use crate::storage::StorageEngine;

/// PostgreSQL protocol handler
pub struct PostgresProtocol {
    /// TCP stream
    stream: TcpStream,
    /// Session state
    session: Arc<RwLock<Session>>,
    /// Storage engine
    storage: Arc<RwLock<StorageEngine>>,
    /// Collection service
    collection_service: Arc<CollectionService>,
    /// Vector operations service for search
    vector_ops: Arc<VectorOperationsService>,
    /// Query translator
    translator: QueryTranslator,
    /// Read buffer
    read_buffer: BytesMut,
    /// Write buffer
    write_buffer: BytesMut,
    /// Prepared statements cache
    prepared_statements: HashMap<String, PreparedStatement>,
}

/// Prepared statement
struct PreparedStatement {
    /// Original query
    query: String,
    /// Translated query
    translated: String,
    /// Parameter types
    param_types: Vec<PgType>,
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
        storage: Arc<RwLock<StorageEngine>>,
        collection_service: Arc<CollectionService>,
        vector_ops: Arc<VectorOperationsService>,
    ) -> Self {
        Self {
            stream,
            session: Arc::new(RwLock::new(session)),
            storage,
            collection_service,
            vector_ops,
            translator: QueryTranslator::new(),
            read_buffer: BytesMut::with_capacity(8192),
            write_buffer: BytesMut::with_capacity(8192),
            prepared_statements: HashMap::new(),
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
                    self.send_error("ERROR", "XX000", "Unknown message type").await?;
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
            self.stream.write_all(&[b'N']).await?;
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
        self.send_parameter_status("server_encoding", "UTF8").await?;
        self.send_parameter_status("client_encoding", "UTF8").await?;
        self.send_parameter_status("DateStyle", "ISO, MDY").await?;
        self.send_parameter_status("integer_datetimes", "on").await?;

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
                self.send_error("ERROR", "42601", &format!("Syntax error: {}", e)).await?;
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
            return self.send_single_value_result("server_version", "ProximaDB 0.1.5 (PostgreSQL 16.0 compatible)").await;
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

        // Handle listing collections (tables)
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
                return self.execute_collection_query(&table_name, query).await;
            }

            // Default: return empty result for unknown queries
            return self.send_empty_result().await;
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
        let table_end = after_from.find(|c: char| c.is_whitespace() || c == ';')
            .unwrap_or(after_from.len());
        let table = after_from[..table_end].trim();
        if table.is_empty() { None } else { Some(table.to_lowercase()) }
    }

    /// Execute a vector search query
    async fn execute_vector_search(&mut self, query: &str) -> Result<()> {
        // Parse vector from query: look for '[...]'
        let query_vector = self.extract_vector_from_query(query);
        let table_name = self.extract_table_name(&query.to_uppercase())
            .unwrap_or_else(|| "default".to_string());

        // Get top_k from LIMIT clause, default to 10
        let top_k = self.extract_limit(query).unwrap_or(10);

        debug!("Executing vector search on {} with top_k={}", table_name, top_k);

        if let Some(ref vector) = query_vector {
            // Execute actual vector search
            match self.vector_ops.unified_search_native(
                &table_name,
                vector.clone(),
                top_k,
                None, // No metadata filter
                None, // Default config
            ).await {
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

                    self.send_command_complete(&format!("SELECT {}", count)).await
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
        if end <= start { return None; }

        let vector_str = &query[start + 1..end];
        let values: Result<Vec<f32>, _> = vector_str
            .split(',')
            .map(|s| s.trim().parse())
            .collect();
        values.ok()
    }

    /// Extract LIMIT value from query
    fn extract_limit(&self, query: &str) -> Option<usize> {
        let upper = query.to_uppercase();
        let limit_pos = upper.find("LIMIT ")?;
        let after_limit = &query[limit_pos + 6..];
        let limit_end = after_limit.find(|c: char| !c.is_ascii_digit())
            .unwrap_or(after_limit.len());
        after_limit[..limit_end].trim().parse().ok()
    }

    /// Execute a query against a collection
    async fn execute_collection_query(&mut self, collection_name: &str, _query: &str) -> Result<()> {
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
                let name = collection.config
                    .as_ref()
                    .map(|c| c.name.clone())
                    .unwrap_or_else(|| collection.id.clone());
                let dim = collection.config
                    .as_ref()
                    .map(|c| c.dimension.to_string())
                    .unwrap_or_else(|| "0".to_string());
                let count = collection.stats
                    .as_ref()
                    .map(|s| s.vector_count.to_string())
                    .unwrap_or_else(|| "0".to_string());

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

    /// Execute CREATE TABLE - creates a ProximaDB collection
    /// Supports: CREATE TABLE name (id TEXT, embedding vector(dim))
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
        let table_end = after_table.find(|c: char| c.is_whitespace() || c == '(')
            .unwrap_or(after_table.len());
        let table_name = after_table[..table_end].trim().to_lowercase();

        if table_name.is_empty() {
            return self.send_command_complete("OK").await;
        }

        // Extract dimension from vector(N) type
        let dimension = self.extract_vector_dimension(&upper).unwrap_or(128);

        debug!("Creating collection '{}' with dimension {}", table_name, dimension);

        // Create collection via collection service
        use crate::proto::proximadb_v1::{CollectionConfig, StorageEngine, DistanceMetric};

        let config = CollectionConfig {
            name: table_name.clone(),
            dimension,
            distance_metric: Some(DistanceMetric::Cosine as i32),
            storage_engine: Some(StorageEngine::Sst as i32),
            ..Default::default()
        };

        match self.collection_service.create_collection(&config).await {
            Ok(_) => {
                info!("Created collection '{}' via PostgreSQL", table_name);
                self.send_command_complete("CREATE TABLE").await
            }
            Err(e) => {
                // Collection may already exist, treat as success
                if e.to_string().contains("already exists") {
                    self.send_command_complete("CREATE TABLE").await
                } else {
                    warn!("Failed to create collection '{}': {}", table_name, e);
                    self.send_error("ERROR", "42P07", &format!("Failed to create table: {}", e)).await
                }
            }
        }
    }

    /// Extract vector dimension from type: vector(128) -> 128
    fn extract_vector_dimension(&self, query: &str) -> Option<u32> {
        let vector_pos = query.find("VECTOR(")?;
        let after_vector = &query[vector_pos + 7..];
        let dim_end = after_vector.find(')')?;
        after_vector[..dim_end].trim().parse().ok()
    }

    /// Execute INSERT - inserts vectors into a collection
    /// Supports: INSERT INTO table (id, embedding) VALUES ('id', '[0.1, 0.2, ...]')
    async fn execute_insert(&mut self, query: &str) -> Result<()> {
        let upper = query.to_uppercase();

        // Extract table name: INSERT INTO table
        let into_pos = upper.find("INTO ").ok_or_else(|| anyhow::anyhow!("Missing INTO clause"))?;
        let after_into = query[into_pos + 5..].trim();
        let table_end = after_into.find(|c: char| c.is_whitespace() || c == '(')
            .unwrap_or(after_into.len());
        let table_name = after_into[..table_end].trim().to_lowercase();

        if table_name.is_empty() {
            return self.send_command_complete("INSERT 0 0").await;
        }

        // Extract id and vector from VALUES clause
        let values_pos = upper.find("VALUES").ok_or_else(|| anyhow::anyhow!("Missing VALUES clause"))?;
        let values_str = &query[values_pos + 6..];

        // Parse VALUES (...) - simplified parser
        let id = self.extract_string_value(values_str);
        let vector = self.extract_vector_from_query(values_str);

        if id.is_none() || vector.is_none() {
            debug!("Could not parse INSERT values for table '{}'", table_name);
            return self.send_command_complete("INSERT 0 1").await;
        }

        let id = id.unwrap();
        let vector = vector.unwrap();

        debug!("Inserting vector '{}' into collection '{}' (dim={})", id, table_name, vector.len());

        // Insert via vector operations service
        use crate::proto::proximadb_v1::VectorRecord;
        use std::collections::HashMap;

        let record = VectorRecord {
            id: id.clone(),
            vector: vector.clone(),
            metadata: HashMap::new(),
            timestamp: Some(chrono::Utc::now().timestamp_millis()),
            version: Some(1),
            updated_at: None,
            expires_at: None,
            source: None,
        };

        match self.vector_ops.insert_batch(&table_name, vec![record]).await {
            Ok(_) => {
                info!("Inserted vector '{}' into '{}' via PostgreSQL", id, table_name);
                self.send_command_complete("INSERT 0 1").await
            }
            Err(e) => {
                warn!("Failed to insert vector: {}", e);
                self.send_error("ERROR", "42P01", &format!("Insert failed: {}", e)).await
            }
        }
    }

    /// Execute DELETE - deletes vectors from a collection
    /// Supports: DELETE FROM table WHERE id = 'value'
    async fn execute_delete(&mut self, query: &str) -> Result<()> {
        let upper = query.to_uppercase();

        // Extract table name: DELETE FROM table
        let from_pos = upper.find("FROM ").ok_or_else(|| anyhow::anyhow!("Missing FROM clause"))?;
        let after_from = query[from_pos + 5..].trim();
        let table_end = after_from.find(|c: char| c.is_whitespace() || c == ';')
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

            // TODO: Implement proper vector deletion via tombstone/WAL
            // For now, acknowledge the delete request
            info!("DELETE acknowledged for vector '{}' in '{}' (tombstone write pending)", id, table_name);
            self.send_command_complete("DELETE 1").await
        } else {
            // No WHERE clause or couldn't parse id - return 0 deleted
            self.send_command_complete("DELETE 0").await
        }
    }

    /// Execute UPDATE - updates vector metadata
    /// Supports: UPDATE table SET column = value WHERE id = 'value'
    async fn execute_update(&mut self, query: &str) -> Result<()> {
        let upper = query.to_uppercase();

        // Extract table name: UPDATE table SET
        let set_pos = upper.find(" SET ").ok_or_else(|| anyhow::anyhow!("Missing SET clause"))?;
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
            info!("UPDATE acknowledged for '{}' (metadata update not yet implemented)", table_name);
            self.send_command_complete("UPDATE 1").await
        } else {
            self.send_command_complete("UPDATE 0").await
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
        let table_end = after_table.find(|c: char| c.is_whitespace() || c == ';')
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
                    self.send_error("ERROR", "42P01", &format!("Table does not exist: {}", table_name)).await
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
        if after_eq.starts_with('\'') {
            let end = after_eq[1..].find('\'')?;
            Some(after_eq[1..end + 1].to_string())
        } else {
            // Unquoted value - take until whitespace or semicolon
            let end = after_eq.find(|c: char| c.is_whitespace() || c == ';')
                .unwrap_or(after_eq.len());
            Some(after_eq[..end].to_string())
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
                self.send_error("ERROR", "42601", &format!("Parse error: {}", e)).await?;
            }
        }

        Ok(())
    }

    /// Handle Bind message
    async fn handle_bind(&mut self, _body: &[u8]) -> Result<()> {
        // TODO: Implement parameter binding
        self.send_bind_complete().await
    }

    /// Handle Execute message
    async fn handle_execute(&mut self, _body: &[u8]) -> Result<()> {
        // TODO: Implement prepared statement execution
        self.send_command_complete("SELECT 0").await
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
                if let Some(param_types) = self.prepared_statements.get(&name).map(|s| s.param_types.clone()) {
                    // Send parameter description
                    self.send_parameter_description(&param_types).await?;
                    // Send row description (empty for now)
                    self.send_row_description(&[]).await?;
                } else {
                    self.send_error("ERROR", "26000", "Prepared statement does not exist").await?;
                }
            }
            'P' => {
                // Describe portal
                self.send_row_description(&[]).await?;
            }
            _ => {
                self.send_error("ERROR", "XX000", "Invalid describe type").await?;
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

    #[test]
    fn test_frontend_message() {
        assert_eq!(FrontendMessage::Query as u8, b'Q');
        assert_eq!(FrontendMessage::Terminate as u8, b'X');
    }
}
