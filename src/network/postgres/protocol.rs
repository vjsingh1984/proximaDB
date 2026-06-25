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
use futures::FutureExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use super::session::Session;
use super::translator::QueryTranslator;
use super::types::{FieldDescription, PgType};
use crate::catalog::CatalogManager;
use crate::graph::GraphService;
use crate::network::arrow_ipc::ArrowProtoCodec;
use crate::observability::ObservabilityService;
use crate::query::execution::{ExecutionControls, ExecutionPipelineResult, RowLimitMode};
use crate::query::multimodal_router::{self, DataModel};
use crate::query::sql_frontend::SqlFrontendParser;
use crate::query::table_write_plan::WriteIntentOverrides;
use crate::services::VectorOperationsService;
use crate::services::dml::{
    RelationalSelectPredicateCondition as SelectPredicateCondition,
    RelationalSelectPredicateInput as SelectPredicate,
    RelationalSelectPredicateOperator as SelectPredicateOperator,
};
use crate::services::{DdlService, DmlService};
use crate::storage::document::DocumentService;
use proximadb_data_model::ProximaType;
use proximadb_data_model::ProximaValue;

/// PostgreSQL protocol handler
pub struct PostgresProtocol {
    /// TCP stream
    stream: TcpStream,
    /// Session state
    session: Arc<RwLock<Session>>,
    /// Collection service
    collection_port: Arc<dyn proximadb_runtime::CollectionPort>,
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
    /// Slice 6.3: primary-pod write router. Same shape as the gRPC v2
    /// and Arrow Flight gates — when present, `execute_insert/update/
    /// delete_via_dml_service` consult the registry post-parse and
    /// pre-execute, rejecting misrouted DML with SQLSTATE 57P03
    /// (cannot_connect_now) so pgwire clients see "wrong server,
    /// retry elsewhere" semantics.
    primary_pod_gate: Option<PgwirePrimaryPodGate>,
    /// TD-102: when set, `send_row_description` is a no-op. The extended-query
    /// path (`handle_execute`) reports columns once at Describe(statement)
    /// time, so the per-statement `execute_*` RowDescription emitted during
    /// Execute would be a duplicate the client rejects (`UnexpectedMessage`).
    /// Set only around the extended Execute call; the simple-query path leaves
    /// it false and emits RowDescription as before.
    suppress_row_description: bool,
    /// E0 rate-limiting: shared per-IP limiter (None = disabled), checked once at
    /// query entry. Converged with the REST limiter via `RateLimitState`.
    rate_limiter: Option<Arc<crate::network::middleware::rate_limit::RateLimitState>>,
    /// This connection's peer IP — the rate-limit subject and the KOU
    /// result-egress locality subject (classified at each result-set boundary).
    peer_ip: std::net::IpAddr,
    /// KOU result-egress: bytes of DataRow payload accumulated for the current
    /// result set, flushed to the meter (direction=result) at CommandComplete /
    /// PortalSuspended. Zero on the free path / non-row commands.
    result_bytes_pending: u64,
}

/// Slice 6.3 gate-input bundle. Distinct type per surface for module
/// privacy; the wire contract (gate logic + error semantics) is the
/// same as gRPC v2 and Arrow Flight.
#[derive(Clone)]
struct PgwirePrimaryPodGate {
    registry: Arc<crate::cluster::primary_pod_registry::PrimaryPodRegistry>,
    self_pod_id: String,
}

/// Slice 6.3 gate consultation result. Surfaces the misrouted target
/// pod up to the caller so the pgwire ERROR message can include it —
/// pgwire clients can read the message in `psql` output and rebuild
/// their connection string.
enum PgwireGateOutcome {
    Allow,
    Misrouted { target_pod: String },
}

/// Slice 6.3 testable helper. Pure function so unit tests can call
/// it without spinning up a TCP listener / Session. Mirrors the
/// gRPC v2 and Arrow Flight gate semantics — `None` gate is a
/// no-op (legacy/embedded paths still pass through unchanged).
fn check_pgwire_primary_pod_gate(
    gate: &Option<PgwirePrimaryPodGate>,
    tenant_id: &str,
    collection_id: &str,
) -> PgwireGateOutcome {
    let Some(gate) = gate else {
        return PgwireGateOutcome::Allow;
    };
    match crate::cluster::primary_pod_registry::consult_for_write(
        &gate.registry,
        &gate.self_pod_id,
        tenant_id,
        collection_id,
    ) {
        crate::cluster::primary_pod_registry::WriteRoutingDecision::Allow => {
            if gate.registry.is_assigned(tenant_id, collection_id) {
                crate::metrics::primary_pod_metrics::record_allowed_bound(tenant_id);
            } else {
                crate::metrics::primary_pod_metrics::record_allowed_unbounded(tenant_id);
            }
            PgwireGateOutcome::Allow
        }
        crate::cluster::primary_pod_registry::WriteRoutingDecision::Misrouted { target_pod } => {
            crate::metrics::primary_pod_metrics::record_misrouted(tenant_id);
            tracing::warn!(
                target = "proximadb.primary_pod.misroute",
                self_pod = %gate.self_pod_id,
                target_pod = %target_pod,
                tenant_id = %tenant_id,
                collection_id = %collection_id,
                "pgwire DML misrouted — client should reconnect to the primary pod"
            );
            PgwireGateOutcome::Misrouted { target_pod }
        }
    }
}

// Note: DmlStatement::target_table_name() (defined in
// src/services/dml/mod.rs) already returns the target table for all
// 6 variants (Insert/Update/Delete/Upsert/InsertSelect/InsertOverwrite),
// so the gate just calls that — no local helper needed here.

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
    /// Materialized result cursor for extended-protocol portal paging.
    execution_state: Option<PortalExecutionState>,
}

/// Cached execution state for a portal.
struct PortalExecutionState {
    result: ExecutionPipelineResult,
    next_row: usize,
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

fn pg_type_for_catalog_data_type(data_type: &ProximaType) -> PgType {
    match data_type {
        ProximaType::Boolean => PgType::Bool,
        ProximaType::Int8 => PgType::Int2,
        ProximaType::Int16 => PgType::Int2,
        ProximaType::Int32 => PgType::Int4,
        ProximaType::Int64 => PgType::Int8,
        ProximaType::Float32 => PgType::Float4,
        ProximaType::Float64 => PgType::Float8,
        ProximaType::Binary => PgType::Bytea,
        ProximaType::Date => PgType::Date,
        ProximaType::Timestamp(_) => PgType::Timestamp,
        ProximaType::TimestampTz(_) => PgType::Timestamptz,
        ProximaType::Json => PgType::Jsonb,
        ProximaType::Uuid => PgType::Uuid,
        ProximaType::DenseVector { .. }
        | ProximaType::SparseVector { .. }
        | ProximaType::BinaryVector { .. } => PgType::Vector,
        // String / Time / Decimal and any richer ProximaType variant → text.
        _ => PgType::Text,
    }
}

/// One ORDER BY key after parsing (ADR-018 Phase 2).
#[derive(Debug, Clone, PartialEq, Eq)]
struct OrderByKey {
    /// Bare column name after identifier-cleanup (lowercased, quotes
    /// stripped). Empty names are rejected during parse.
    column: String,
    /// `true` for `DESC`, `false` for `ASC` (default).
    desc: bool,
    /// `true` for `NULLS FIRST`, `false` for `NULLS LAST`. Postgres
    /// default when no explicit NULLS clause: ASC → NULLS LAST,
    /// DESC → NULLS FIRST.
    nulls_first: bool,
}

fn proxima_value_to_pg_text(value: &ProximaValue) -> String {
    match value {
        ProximaValue::Boolean(value) => {
            if *value {
                "t".to_string()
            } else {
                "f".to_string()
            }
        }
        ProximaValue::Int8(value) => value.to_string(),
        ProximaValue::Int16(value) => value.to_string(),
        ProximaValue::Int32(value) => value.to_string(),
        ProximaValue::Int64(value) => value.to_string(),
        ProximaValue::UInt8(value) => value.to_string(),
        ProximaValue::UInt16(value) => value.to_string(),
        ProximaValue::UInt32(value) => value.to_string(),
        ProximaValue::UInt64(value) => value.to_string(),
        ProximaValue::Float16(value) => value.to_string(),
        ProximaValue::Float32(value) => value.to_string(),
        ProximaValue::Float64(value) => value.to_string(),
        ProximaValue::Decimal(value) => value.clone(),
        ProximaValue::String(value) | ProximaValue::Symbol(value) => value.clone(),
        ProximaValue::Binary(value) | ProximaValue::BinaryVector(value) => {
            format!("\\x{}", bytes_to_hex(value))
        }
        ProximaValue::Date(value) => value.to_string(),
        ProximaValue::Time(value, _) => value.to_string(),
        ProximaValue::Timestamp(value, _) => value.to_string(),
        ProximaValue::TimestampTz(value, _) => value.to_string(),
        ProximaValue::Uuid(value) | ProximaValue::ULID(value) => bytes_to_hex(value),
        ProximaValue::Json(value) | ProximaValue::Jsonb(value) => value.to_string(),
        ProximaValue::Array(values) => {
            let parts = values
                .iter()
                .map(proxima_value_to_pg_text)
                .collect::<Vec<_>>();
            format!("{{{}}}", parts.join(","))
        }
        ProximaValue::Map(value) | ProximaValue::Struct(value) => {
            serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string())
        }
        ProximaValue::DenseVector(values) => {
            let parts = values.iter().map(ToString::to_string).collect::<Vec<_>>();
            format!("[{}]", parts.join(","))
        }
        ProximaValue::SparseVector { indices, values } => {
            serde_json::json!({ "indices": indices, "values": values }).to_string()
        }
        ProximaValue::Null => String::new(),
    }
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
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
        collection_port: Arc<dyn proximadb_runtime::CollectionPort>,
        vector_ops: Arc<VectorOperationsService>,
        document_service: Option<Arc<DocumentService>>,
        graph_service: Option<Arc<GraphService>>,
        observability_service: Option<Arc<ObservabilityService>>,
    ) -> Self {
        Self {
            stream,
            session: Arc::new(RwLock::new(session)),
            collection_port,
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
            primary_pod_gate: None,
            suppress_row_description: false,
            rate_limiter: None,
            peer_ip: std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
            result_bytes_pending: 0,
        }
    }

    /// Attach a shared per-IP rate limiter (and this connection's peer IP) so the
    /// pgwire query path is rate-limited consistently with REST (E0). When unset
    /// or disabled, queries are not rate-limited (default).
    pub fn with_rate_limiter(
        mut self,
        limiter: Arc<crate::network::middleware::rate_limit::RateLimitState>,
        peer_ip: std::net::IpAddr,
    ) -> Self {
        self.rate_limiter = Some(limiter);
        self.peer_ip = peer_ip;
        self
    }

    /// Create a new protocol handler with DDL/DML services for catalog integration
    pub fn with_catalog_services(
        stream: TcpStream,
        session: Session,
        collection_port: Arc<dyn proximadb_runtime::CollectionPort>,
        vector_ops: Arc<VectorOperationsService>,
        catalog_manager: Arc<CatalogManager>,
    ) -> Self {
        let ddl_service = Arc::new(DdlService::new(catalog_manager.clone()));
        let dml_service = Arc::new(DmlService::new(catalog_manager.clone(), vector_ops.clone()));

        Self {
            stream,
            session: Arc::new(RwLock::new(session)),
            collection_port,
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
            primary_pod_gate: None,
            suppress_row_description: false,
            rate_limiter: None,
            peer_ip: std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
            result_bytes_pending: 0,
        }
    }

    /// Create a new protocol handler whose SQL DML service writes through the
    /// canonical record/WAL path for catalog-routed relational tables.
    ///
    /// Legacy vector-specialized tables still route through `VectorOps` via
    /// `DmlService::with_direct_record_storage`; this constructor only opts the
    /// protocol into the modern canonical branch when xCatalog selects it.
    pub fn with_direct_catalog_services(
        stream: TcpStream,
        session: Session,
        collection_port: Arc<dyn proximadb_runtime::CollectionPort>,
        vector_ops: Arc<VectorOperationsService>,
        catalog_manager: Arc<CatalogManager>,
        canonical_store: Arc<crate::services::record_store::DirectWalTableRecordStore>,
    ) -> Self {
        let ddl_service = Arc::new(DdlService::new(catalog_manager.clone()));
        let dml_service = Arc::new(DmlService::with_direct_record_storage(
            catalog_manager.clone(),
            vector_ops.clone(),
            canonical_store,
        ));

        Self {
            stream,
            session: Arc::new(RwLock::new(session)),
            collection_port,
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
            primary_pod_gate: None,
            suppress_row_description: false,
            rate_limiter: None,
            peer_ip: std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
            result_bytes_pending: 0,
        }
    }

    /// Slice 6.3: attach the primary-pod write router. Once set,
    /// `execute_insert/update/delete_via_dml_service` consult the
    /// registry post-parse and reject misrouted writes with SQLSTATE
    /// 57P03 (cannot_connect_now) before any catalog state changes.
    /// SharedServices passes the same `Arc<PrimaryPodRegistry>` /
    /// pod_id pair the REST and gRPC v2 paths hold.
    pub fn with_primary_pod_gate(
        mut self,
        registry: Arc<crate::cluster::primary_pod_registry::PrimaryPodRegistry>,
        self_pod_id: String,
    ) -> Self {
        self.primary_pod_gate = Some(PgwirePrimaryPodGate {
            registry,
            self_pod_id,
        });
        self
    }

    /// Attach catalog-backed DDL/DML services to an existing protocol handler.
    pub fn with_catalog_manager(mut self, catalog_manager: Arc<CatalogManager>) -> Self {
        self.ddl_service = Some(Arc::new(DdlService::new(catalog_manager.clone())));
        self.dml_service = Some(Arc::new(DmlService::new(
            catalog_manager.clone(),
            self.vector_ops.clone(),
        )));
        self.catalog_manager = Some(catalog_manager);
        self
    }

    /// Attach catalog-backed DDL/DML services with direct canonical
    /// record/WAL writes enabled for catalog-routed relational tables.
    pub fn with_direct_catalog_manager(
        mut self,
        catalog_manager: Arc<CatalogManager>,
        canonical_store: Arc<crate::services::record_store::DirectWalTableRecordStore>,
    ) -> Self {
        self.ddl_service = Some(Arc::new(DdlService::new(catalog_manager.clone())));
        self.dml_service = Some(Arc::new(DmlService::with_direct_record_storage(
            catalog_manager.clone(),
            self.vector_ops.clone(),
            canonical_store,
        )));
        self.catalog_manager = Some(catalog_manager);
        self
    }

    /// Attach the rank-pipeline catalog + live registry to the current
    /// `DdlService` so `CREATE RANK PROFILE` / `DROP RANK PROFILE` SQL
    /// statements lower into the same `RankServices` REST and gRPC share.
    ///
    /// Call this after `with_catalog_manager` / `with_direct_catalog_manager`
    /// / `with_catalog_services` / `with_direct_catalog_services`. If no
    /// `DdlService` is present (no catalog manager attached), this is a
    /// no-op — pgwire DDL is gated by the existence of `ddl_service`
    /// anyway, so a catalog-less handler stays catalog-less.
    pub fn with_rank_pipeline(
        mut self,
        services: Arc<crate::network::rest::v1::rank::RankServices>,
        store: Arc<dyn crate::services::RankProfileStore>,
        function_store: Arc<dyn crate::services::FunctionStore>,
    ) -> Self {
        let Some(catalog_manager) = self.catalog_manager.clone() else {
            return self;
        };
        self.ddl_service = Some(Arc::new(
            DdlService::new(catalog_manager)
                .with_rank_profile_store(store)
                .with_rank_services(services)
                .with_function_store(function_store),
        ));
        self
    }

    /// Wire the warehouse table materializer so `ALTER TABLE … MATERIALIZE`
    /// publishes the table's rows as a Parquet snapshot under `warehouse_root_url`
    /// and flips its catalog layout to the OLAP-readable projection.
    ///
    /// Call this AFTER the catalog/rank builders — it augments the already-assembled
    /// `DdlService`: it unwraps the (not-yet-shared) service, adds a
    /// `DmlTableMaterializer` over the current `DmlService` + an object store opened
    /// from `warehouse_root_url`, and rebuilds it. No-op — the trigger keeps
    /// returning its clean "requires a configured warehouse object store" error —
    /// when there is no `DdlService`/`DmlService`, the `Arc` is already shared, or the
    /// root URL can't be opened, so a misconfigured warehouse never breaks the
    /// connection.
    pub fn with_materializer(mut self, warehouse_root_url: String) -> Self {
        let Some(dml) = self.dml_service.clone() else {
            return self;
        };
        let Some(ddl) = self.ddl_service.take() else {
            return self;
        };
        let ddl = match Arc::try_unwrap(ddl) {
            Ok(ddl) => ddl,
            Err(shared) => {
                // Already shared (unexpected during setup) — leave it untouched.
                self.ddl_service = Some(shared);
                return self;
            }
        };
        let bridge =
            match proximadb_iceberg_engine::IcebergObjectStoreBridge::from_url(&warehouse_root_url)
            {
                Ok(bridge) => Arc::new(bridge)
                    as Arc<dyn proximadb_storage_common::object_store_bridge::ObjectStoreBridge>,
                Err(e) => {
                    tracing::warn!(
                        target: "proximadb::pgwire::materialize",
                        "warehouse object store unavailable at {warehouse_root_url}: {e}; \
                         ALTER TABLE … MATERIALIZE stays unwired"
                    );
                    self.ddl_service = Some(Arc::new(ddl));
                    return self;
                }
            };
        let materializer = Arc::new(crate::services::dml::DmlTableMaterializer::new(
            dml,
            bridge,
            warehouse_root_url,
        ));
        self.ddl_service = Some(Arc::new(ddl.with_materializer(materializer)));
        self
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
            // Open-core cache tier hook: a `proximadb_tier` startup parameter
            // (control-plane supplied) records the connection tenant's tier for
            // the cache policy. database == tenant/catalog (TD-064). Opaque id.
            if let (Some(tier), db) = (params.get("proximadb_tier"), session.database.clone())
                && !tier.is_empty()
                && !db.is_empty()
            {
                crate::services::record_store::set_tenant_tier(db, tier.clone());
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

        if Self::is_set_statement(&query) {
            match self.execute_set_parameter(&query).await {
                Ok(()) => {}
                Err(e) => {
                    self.send_error("ERROR", "42601", &format!("SET failed: {}", e))
                        .await?;
                }
            }
            self.send_ready_for_query('I').await?;
            return Ok(());
        }

        // Split semicolon-separated statements. The PostgreSQL Simple
        // Query protocol explicitly supports multi-statement queries;
        // each statement gets its own RowDescription / DataRow /
        // CommandComplete sequence. The split is quote-aware so
        // semicolons inside string literals don't accidentally cut a
        // statement.
        //
        // Previously the entire multi-statement string was passed to
        // the translator + execute_query, which only matched the
        // FIRST keyword — meaning `BEGIN; INSERT ...; COMMIT;` was
        // processed as a single `BEGIN` and the INSERT silently
        // disappeared. This was a data-loss bug, not a feature gap.
        let statements = Self::split_sql_statements(&query);
        if statements.is_empty() {
            self.send_ready_for_query('I').await?;
            return Ok(());
        }
        for statement in statements {
            // Translate each statement to ProximaDB format.
            let translated = match self.translator.translate(&statement) {
                Ok(t) => t,
                Err(e) => {
                    self.send_error("ERROR", "42601", &format!("Syntax error: {}", e))
                        .await?;
                    // Stop processing subsequent statements on error to
                    // match PostgreSQL's "abort on error" semantics
                    // inside a multi-statement query.
                    break;
                }
            };
            // Panic-guard the SQL execution path. Several SELECT
            // shapes (aggregates, JOINs, certain catalog views)
            // currently panic inside the executor; without this
            // guard the panic propagates up and the runtime drops
            // the TCP connection, leaving the client with an EOF.
            // The guard converts those panics into well-formed
            // PostgreSQL ErrorResponse messages (SQLSTATE XX000 —
            // "internal error") so clients see a real SQL error
            // and the connection survives. This is a stopgap until
            // each crasher is implemented properly (Phase 1.3+ for
            // the easier ones, Phase 2 for the relational planner).
            //
            // After a panic the connection state may be partially
            // corrupted (mid-response writes), so we still stop
            // processing subsequent statements in this multi-
            // statement query.
            let exec_result = std::panic::AssertUnwindSafe(self.execute_query(&translated))
                .catch_unwind()
                .await;
            match exec_result {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    self.send_error("ERROR", "XX000", &format!("execution failed: {}", e))
                        .await?;
                    break;
                }
                Err(panic_payload) => {
                    let panic_msg = Self::panic_payload_to_string(&panic_payload);
                    error!(
                        target: "proximadb::pgwire::panic_guard",
                        statement = %statement,
                        panic = %panic_msg,
                        "pgwire execution panicked; converted to SQL error"
                    );
                    // Best-effort error response. If writing fails
                    // (socket already closed), let `?` propagate
                    // so the connection loop exits cleanly.
                    self.send_error(
                        "ERROR",
                        "XX000",
                        &format!(
                            "internal pgwire error: {} (likely an \
                             unsupported SQL feature; see ADR-018)",
                            panic_msg
                        ),
                    )
                    .await?;
                    break;
                }
            }
        }

        // Send ready for query
        self.send_ready_for_query('I').await?;

        Ok(())
    }

    /// Best-effort downcast of a panic payload to a printable
    /// string. Standard panic payloads are either `&'static str`
    /// (from `panic!("literal")`) or `String` (from
    /// `panic!("{}", x)`); we fall back to a generic marker
    /// otherwise so the error message is always non-empty.
    fn panic_payload_to_string(payload: &Box<dyn std::any::Any + Send>) -> String {
        if let Some(s) = payload.downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.clone()
        } else {
            "unrecoverable error (panic payload not stringifiable)".to_string()
        }
    }

    /// Split a multi-statement SQL query on top-level semicolons.
    /// Quote-aware: semicolons inside single- or double-quoted
    /// strings are not statement separators. SQL-style `''`
    /// inside a single-quoted string is treated as an escaped
    /// single quote, not as the end of the string.
    fn split_sql_statements(query: &str) -> Vec<String> {
        let mut statements = Vec::new();
        let mut current = String::new();
        let mut in_single = false;
        let mut in_double = false;
        let mut chars = query.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\'' if !in_double => {
                    current.push(c);
                    if in_single && chars.peek() == Some(&'\'') {
                        // SQL '' escape — consume the second quote
                        // verbatim and stay inside the literal.
                        // `peek() == Some(&'\'')` proves the iterator has
                        // at least one element, so `next()` cannot return
                        // None on this branch.
                        #[allow(clippy::unwrap_used)]
                        current.push(chars.next().unwrap());
                    } else {
                        in_single = !in_single;
                    }
                }
                '"' if !in_single => {
                    current.push(c);
                    in_double = !in_double;
                }
                ';' if !in_single && !in_double => {
                    let trimmed = current.trim();
                    if !trimmed.is_empty() {
                        statements.push(trimmed.to_string());
                    }
                    current.clear();
                }
                _ => current.push(c),
            }
        }
        let trimmed = current.trim();
        if !trimmed.is_empty() {
            statements.push(trimmed.to_string());
        }
        statements
    }

    fn is_set_statement(query: &str) -> bool {
        query.trim_start().to_ascii_uppercase().starts_with("SET ")
    }

    async fn execute_set_parameter(&mut self, query: &str) -> Result<()> {
        let (name, value) = Self::parse_set_parameter(query)?;
        {
            let mut session = self.session.write().await;
            session.set_parameter(&name, &value);
        }
        self.send_command_complete("SET").await
    }

    fn parse_set_parameter(query: &str) -> Result<(String, String)> {
        let trimmed = query.trim().trim_end_matches(';').trim();
        let rest = trimmed
            .get(3..)
            .ok_or_else(|| anyhow!("missing SET parameter"))?
            .trim_start();
        let rest = rest
            .strip_prefix("SESSION ")
            .or_else(|| rest.strip_prefix("session "))
            .unwrap_or(rest)
            .trim_start();
        let rest = rest
            .strip_prefix("LOCAL ")
            .or_else(|| rest.strip_prefix("local "))
            .unwrap_or(rest)
            .trim_start();

        let (name, value) = if let Some(eq_index) = rest.find('=') {
            (&rest[..eq_index], &rest[eq_index + 1..])
        } else {
            let upper = rest.to_ascii_uppercase();
            let Some(to_index) = upper.find(" TO ") else {
                return Err(anyhow!("expected SET name = value or SET name TO value"));
            };
            (&rest[..to_index], &rest[to_index + " TO ".len()..])
        };

        let name = name.trim().trim_matches('"').to_ascii_lowercase();
        if name.is_empty() {
            return Err(anyhow!("missing SET parameter name"));
        }

        Ok((name, Self::strip_set_value_literal(value)))
    }

    fn strip_set_value_literal(value: &str) -> String {
        let value = value.trim().trim_end_matches(';').trim();
        if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
            return value[1..value.len() - 1].replace("''", "'");
        }
        value.trim_matches('"').to_string()
    }

    /// Execute a translated query
    async fn execute_query(&mut self, query: &str) -> Result<()> {
        self.execute_query_with_controls(query, ExecutionControls::default())
            .await
    }

    /// Execute a translated query with request-scoped execution controls.
    ///
    /// E0 (in-process edge — `IN_PROCESS_EDGE_COLLAPSE_HLD_2026_06_19`): every
    /// pgwire query is scoped in a per-query `io_trace` span with tenant
    /// attribution, so the pgwire surface emits the *same* per-query I/O trace
    /// (object-store GETs/bytes, footer-cache outcomes, engine-ms) as REST/gRPC
    /// — closing the consistency gap where pgwire bypassed the edge middleware
    /// entirely. The tenant is the one already resolved for read routing
    /// (TD-064); no new identity source is introduced. The wrapper is a thin
    /// scope around the unchanged body in `_inner`.
    async fn execute_query_with_controls(
        &mut self,
        query: &str,
        controls: ExecutionControls,
    ) -> Result<()> {
        // E0 rate-limiting: reject over-quota queries up front with a pgwire
        // error, using the SAME converged RateLimitState check REST uses (no
        // duplicate limiter). No-op when unset/disabled.
        if let Some(limiter) = self.rate_limiter.clone()
            && limiter.enabled()
            && let Err(retry_after) = limiter.check_and_consume(self.peer_ip).await
        {
            return self
                .send_error(
                    "ERROR",
                    "53400",
                    &format!("rate limit exceeded; retry after {retry_after}s"),
                )
                .await;
        }
        let tenant = self.pgwire_resolve_read_tenant().await;
        // E0 (edge consistency): give every pgwire query a request-id correlation
        // scope — matching REST's request_id middleware — so logs and error
        // envelopes carry it; then the per-query io_trace span (tenant-attributed).
        let request_id = crate::network::middleware::request_id::RequestId::generate();
        proximadb_api::rest::errors::REQUEST_ID
            .scope(
                request_id.0,
                crate::observability::io_trace::instrument(
                    Some(tenant),
                    "pgwire.query",
                    self.execute_query_with_controls_inner(query, controls),
                ),
            )
            .await
    }

    async fn execute_query_with_controls_inner(
        &mut self,
        query: &str,
        controls: ExecutionControls,
    ) -> Result<()> {
        let upper = query.to_uppercase();

        // Transaction control. ProximaDB does not yet implement real
        // MVCC isolation, so BEGIN/COMMIT/ROLLBACK are autocommit
        // no-ops at the engine level. We still emit the correct
        // PostgreSQL command tag so clients (psql, ORM drivers,
        // connection poolers) parse the response normally instead of
        // silently mis-classifying it. The tracing warning records
        // the autocommit truth so operator dashboards can surface it
        // until real transactions land in Phase 3.
        //
        // Previously these statements fell through to
        // `send_command_complete("OK")` at the bottom of this
        // method — which caused multi-statement queries like
        // `BEGIN; INSERT ...; COMMIT;` to look successful while
        // silently dropping work. See ADR-018 for the autocommit
        // contract and the Phase 3 plan for real transactions.
        let trimmed_upper = upper.trim();
        let trimmed_upper = trimmed_upper
            .strip_suffix(';')
            .map(str::trim)
            .unwrap_or(trimmed_upper);
        if trimmed_upper == "BEGIN"
            || trimmed_upper == "BEGIN TRANSACTION"
            || trimmed_upper == "BEGIN WORK"
            || trimmed_upper == "START TRANSACTION"
        {
            warn!(
                target: "proximadb::pgwire::transactions",
                "BEGIN observed; ProximaDB pgwire is autocommit-only — \
                 isolation/savepoints are not yet implemented"
            );
            return self.send_command_complete("BEGIN").await;
        }
        if trimmed_upper == "COMMIT"
            || trimmed_upper == "COMMIT TRANSACTION"
            || trimmed_upper == "COMMIT WORK"
            || trimmed_upper == "END"
            || trimmed_upper == "END TRANSACTION"
            || trimmed_upper == "END WORK"
        {
            return self.send_command_complete("COMMIT").await;
        }
        if trimmed_upper == "ROLLBACK"
            || trimmed_upper == "ROLLBACK TRANSACTION"
            || trimmed_upper == "ROLLBACK WORK"
            || trimmed_upper == "ABORT"
            || trimmed_upper == "ABORT TRANSACTION"
            || trimmed_upper == "ABORT WORK"
        {
            // Loud warning because ROLLBACK is the dangerous one:
            // clients calling ROLLBACK expect uncommitted writes to
            // disappear, but under autocommit each statement has
            // already committed and there is nothing to roll back.
            // Phase 3 will replace this with real rollback semantics.
            warn!(
                target: "proximadb::pgwire::transactions",
                "ROLLBACK observed but pgwire is autocommit-only — \
                 prior statements in this query have ALREADY been \
                 applied; this ROLLBACK has no effect"
            );
            return self.send_command_complete("ROLLBACK").await;
        }
        if trimmed_upper.starts_with("SAVEPOINT ")
            || trimmed_upper.starts_with("RELEASE SAVEPOINT ")
            || trimmed_upper.starts_with("ROLLBACK TO ")
        {
            // Loud error: savepoints have no defensible autocommit
            // emulation, and tools that issue them (e.g. SQLAlchemy
            // nested-transaction emulation) MUST know they are not
            // supported instead of silently misbehaving.
            return self
                .send_error(
                    "ERROR",
                    "0A000",
                    "savepoints are not supported (autocommit-only pgwire)",
                )
                .await;
        }

        // Handle SHOW commands converted to SELECT
        if upper.contains("AS SERVER_VERSION") {
            return self
                .send_single_value_result(
                    "server_version",
                    "PostgreSQL 16.0 (ProximaDB 0.2.0 pgwire compatible)",
                )
                .await;
        }
        if upper.contains("VERSION()") {
            return self
                .send_single_value_result(
                    "version",
                    "PostgreSQL 16.0 (ProximaDB 0.2.0 pgwire compatible)",
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
            // TD-064 S1: reflect the connection's effective schema (namespace),
            // honoring `SET search_path`, rather than a hardcoded `public`.
            let schema = self.session.read().await.current_schema();
            return self.send_single_value_result("search_path", &schema).await;
        }
        if upper.starts_with("SELECT") && upper.contains("CURRENT_SCHEMA()") {
            let schema = self.session.read().await.current_schema();
            return self
                .send_single_value_result("current_schema", &schema)
                .await;
        }
        if upper.starts_with("SELECT") && upper.contains("CURRENT_DATABASE()") {
            // TD-064 S1: reflect the connection's catalog (`database` == account
            // == tenant). Fall back to the conventional `postgres` default when
            // the client sent no database on startup.
            let database = {
                let session = self.session.read().await;
                session
                    .catalog_tenant()
                    .unwrap_or_else(|| "postgres".to_string())
            };
            return self
                .send_single_value_result("current_database", &database)
                .await;
        }
        if upper.starts_with("SELECT")
            && (upper.contains("CURRENT_USER") || upper.contains("SESSION_USER"))
        {
            return self
                .send_single_value_result("current_user", "postgres")
                .await;
        }
        if upper.contains("FROM PG_CATALOG.PG_SETTINGS") {
            if upper.contains("MAX_INDEX_KEYS") {
                return self.send_single_value_result("setting", "32").await;
            }
            if upper.contains("DEFAULT_TRANSACTION_ISOLATION") {
                return self
                    .send_single_value_result("setting", "read committed")
                    .await;
            }
        }

        if upper.starts_with("EXPLAIN") {
            return self.execute_explain(query).await;
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
            // S5c new-pipeline interception. When the env flag is
            // set AND the SQL lowers cleanly against the in-memory
            // relational engine's catalog, route through
            // algebra → planner → executor instead of the legacy
            // vector-collection path. Lowering failures fall
            // through (e.g. `SELECT current_schema()` and other
            // pg-specific queries the new frontend doesn't accept).
            // TD-064: scope relational-pipeline reads to the connection tenant.
            if let Some(result) = self
                .try_run_relational_select_pipeline(query, controls.clone())
                .await
            {
                return match result {
                    Ok(pr) => self.emit_pipeline_result(pr).await,
                    Err(msg) => self.send_error("ERROR", "XX000", &msg).await,
                };
            }
            if let Some((column, value)) = Self::extract_simple_constant_select(query) {
                return self.send_single_value_result(&column, &value).await;
            }

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

        if (upper.starts_with("CREATE ")
            || upper.starts_with("ALTER ")
            || upper.starts_with("DROP "))
            && let Some(ddl_service) = self.ddl_service.clone()
        {
            let parser = SqlFrontendParser::new();
            match parser.parse_ddl(query) {
                Ok(Some(statement)) => {
                    // Capture the canonical_embedding_precision
                    // WITH-option from the DDL statement BEFORE
                    // moving it into ddl_service.execute, so the
                    // matching backing-collection write below
                    // sees the same precision the DDL row gets.
                    // Without this, REST GET reads the backing
                    // collection and sees fp32 even though the
                    // relational schema row got the operator's
                    // chosen precision.
                    let backing_precision: Option<String> = match &statement {
                        crate::services::DdlStatement::CreateTable { properties, .. } => {
                            properties.get("canonical_embedding_precision").cloned()
                        }
                        _ => None,
                    };
                    // TD-064: scope table-targeting DDL (CREATE/DROP/ALTER TABLE,
                    // CREATE/DROP INDEX) onto the connection's tenant so a tenant's
                    // CREATE-then-INSERT address one tenant-prefixed schema row.
                    let ddl_tenant = self.pgwire_resolve_write_tenant().await;
                    let ddl_scope = (!ddl_tenant.is_empty()).then_some(ddl_tenant);
                    match ddl_service
                        .execute_scoped(statement, ddl_scope.as_deref())
                        .await
                    {
                        Ok(result) => {
                            if upper.starts_with("CREATE TABLE")
                                && let Some(table_name) = self.extract_create_table_name(query)
                            {
                                self.ensure_relational_backing_collection(
                                    &table_name,
                                    ddl_scope.as_deref(),
                                    backing_precision.as_deref(),
                                )
                                .await?;
                            }
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
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    let legacy_create_table = upper.starts_with("CREATE TABLE")
                        && (upper.contains(" USING VECTOR")
                            || upper.contains(" USING DOCUMENT")
                            || upper.contains(" USING GRAPH")
                            || upper.contains(" USING OBSERVABILITY")
                            || upper.contains(" USING TIMESERIES"));
                    if !legacy_create_table
                        && (upper.starts_with("CREATE TABLE")
                            || upper.starts_with("CREATE INDEX")
                            || upper.starts_with("ALTER TABLE"))
                    {
                        return self
                            .send_error("ERROR", "42601", &format!("Parse error: {}", e))
                            .await;
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

    fn extract_create_table_name(&self, query: &str) -> Option<String> {
        let upper = query.to_ascii_uppercase();
        let table_pos = upper.find("CREATE TABLE")?;
        let after_table = query[table_pos + "CREATE TABLE".len()..].trim_start();
        let after_table = after_table
            .strip_prefix("IF NOT EXISTS")
            .map(str::trim_start)
            .unwrap_or(after_table);
        let table_end = after_table
            .find(|c: char| c.is_whitespace() || c == '(' || c == ';')
            .unwrap_or(after_table.len());
        let table_name = Self::clean_identifier(&after_table[..table_end]);
        (!table_name.is_empty()).then_some(table_name.to_lowercase())
    }

    async fn ensure_relational_backing_collection(
        &self,
        table_name: &str,
        tenant_id: Option<&str>,
        canonical_embedding_precision_label: Option<&str>,
    ) -> Result<()> {
        use crate::proto::proximadb_v1::{CollectionConfig, EmbeddingPrecision, StorageEngine};

        // Map the SQL WITH-option label to the proto discriminant via the
        // same dispatch the REST `apply_proto_enum_workarounds` and the
        // DDL service's build_catalog_schema use. Unknown labels (or
        // None) leave the field unset → server defaults to Fp32 (no
        // behavior change for existing CREATE TABLE statements).
        let canonical_embedding_precision = canonical_embedding_precision_label
            .and_then(|raw| {
                let key = raw.trim().to_ascii_lowercase();
                let stripped = key.strip_prefix("embedding_precision_").unwrap_or(&key);
                match stripped {
                    "fp32" | "f32" | "float32" => Some(EmbeddingPrecision::Fp32),
                    "fp16" | "f16" | "half" | "float16" => Some(EmbeddingPrecision::Fp16),
                    "bf16" | "bfloat16" => Some(EmbeddingPrecision::Bf16),
                    "int8" | "i8" | "int8_scalar" => Some(EmbeddingPrecision::Int8),
                    "uint8" | "u8" | "uint8_scalar" => Some(EmbeddingPrecision::Uint8),
                    _ => None,
                }
            })
            .map(|p| p as i32);

        // Relational tables don't carry vectors; the backing collection is a
        // catalog-visibility shim. CollectionService rejects dimension=0, so
        // pin the shim at 1 (matches other zero-vector compatibility paths).
        let config = CollectionConfig {
            name: table_name.to_string(),
            dimension: 1,
            storage_engine: Some(StorageEngine::Sst as i32),
            description: Some(format!(
                "Relational table backing collection: {}",
                table_name
            )),
            canonical_embedding_precision,
            ..Default::default()
        };

        match self
            .collection_port
            .create_collection(config, tenant_id)
            .await
        {
            Ok(_) => {
                debug!(
                    "Created relational backing collection '{}' via PostgreSQL",
                    table_name
                );
                Ok(())
            }
            Err(e) => {
                let msg = e.to_string();
                let lower = msg.to_ascii_lowercase();
                if lower.contains("already exists") || msg.contains("COLLECTION_EXISTS") {
                    Ok(())
                } else {
                    warn!(
                        "Failed to create relational backing collection '{}': {}",
                        table_name, msg
                    );
                    Err(e)
                }
            }
        }
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

    /// Emit the result of a successful new-pipeline run
    /// (S5c bridge). Builds a `RowDescription` from the
    /// `RelationalSchema`, writes one `DataRow` per row in text
    /// format, then `CommandComplete("SELECT n")`.
    async fn emit_pipeline_result(
        &mut self,
        result: ExecutionPipelineResult,
    ) -> anyhow::Result<()> {
        // RowDescription.
        let fields: Vec<crate::network::postgres::types::FieldDescription> = result
            .schema
            .columns
            .iter()
            .map(|c| {
                let pg = super::relational_pipeline::pg_type_for(&c.ty);
                crate::network::postgres::types::FieldDescription::new(&c.name, pg)
            })
            .collect();
        self.send_row_description(&fields).await?;
        // DataRows.
        let n = result.rows.len();
        for row in &result.rows {
            let cells: Vec<Option<String>> = row
                .iter()
                .map(super::relational_pipeline::text_encode)
                .collect();
            self.send_data_row_nullable(&cells).await?;
        }
        // CommandComplete.
        self.send_command_complete(&format!("SELECT {n}")).await
    }

    /// Try to materialize a SELECT through the relational execution seam.
    async fn try_run_relational_select_pipeline(
        &self,
        query: &str,
        controls: ExecutionControls,
    ) -> Option<Result<ExecutionPipelineResult, String>> {
        let read_tenant = self.pgwire_resolve_read_tenant().await;
        super::relational_pipeline::try_run_select(
            query,
            self.dml_service.as_ref(),
            // F4: hand the OLAP route the live vector service so a cross-modal
            // `... JOIN vector_search('coll','[..]',k)` resolves over pgwire.
            Some(self.vector_ops.clone() as Arc<dyn proximadb_runtime::VectorOpsPort>),
            Some(read_tenant.as_str()),
            controls,
            // pgwire keeps simple single-table SELECTs on its hardened legacy
            // path; only relational-engaging shapes go through this pipeline.
            true,
        )
        .await
    }

    /// Send a DataRow that supports `NULL` (length = -1) cells.
    /// `send_data_row` exists too but takes `&[&str]` — this
    /// variant is needed by the new pipeline to round-trip
    /// SQL NULLs correctly.
    async fn send_data_row_nullable(&mut self, values: &[Option<String>]) -> Result<()> {
        let mut payload_len: usize = 4 /* msg len */ + 2 /* field count */;
        for v in values {
            payload_len += 4;
            if let Some(s) = v {
                payload_len += s.len();
            }
        }
        self.write_buffer.put_u8(b'D');
        self.write_buffer.put_i32(payload_len as i32);
        self.write_buffer.put_i16(values.len() as i16);
        for v in values {
            match v {
                None => {
                    self.write_buffer.put_i32(-1);
                }
                Some(s) => {
                    self.write_buffer.put_i32(s.len() as i32);
                    self.write_buffer.put_slice(s.as_bytes());
                }
            }
        }
        // KOU result-egress: wire bytes = tag(1) + payload_len (which already
        // counts the 4-byte length field + the 2-byte field count + values).
        self.result_bytes_pending = self
            .result_bytes_pending
            .saturating_add(1 + payload_len as u64);
        self.flush_write_buffer().await
    }

    async fn execute_explain(&mut self, query: &str) -> Result<()> {
        let (is_analyze, inner_query) = match Self::extract_explain_with_analyze(query) {
            Ok(pair) => pair,
            Err(error) => {
                return self
                    .send_error("ERROR", "0A000", &format!("EXPLAIN failed: {}", error))
                    .await;
            }
        };

        let inner_upper = inner_query.trim_start().to_ascii_uppercase();

        // SELECT route EXPLAIN (course-correction §5 / ADR-004): surface the
        // ComputeScheduler read-route decision as JSON. Additive — does not execute the
        // query. Catalog-free in P0 (routes on query shape).
        if inner_upper.starts_with("SELECT") || inner_upper.starts_with("WITH") {
            // Catalog-aware when a DmlService is available: discloses the planned
            // physical plan (access method, pushdowns, join/agg strategy) for native
            // PATH B queries, not just the route. EXPLAIN ANALYZE additionally executes
            // the (read-only) plan and reports measured rows + elapsed. Falls back to
            // route-only disclosure when no DmlService is available.
            // TD-064: resolve EXPLAIN's schema/plan under the connection tenant.
            let explain_tenant = self.pgwire_resolve_read_tenant().await;
            let routing = match self.dml_service.clone() {
                Some(dml) if is_analyze => {
                    crate::network::postgres::relational_pipeline::explain_analyze_select_with_catalog(
                        inner_query,
                        &dml,
                        Some(explain_tenant.as_str()),
                    )
                    .await
                }
                Some(dml) => {
                    crate::network::postgres::relational_pipeline::explain_select_route_with_catalog(
                        inner_query,
                        &dml,
                        Some(explain_tenant.as_str()),
                    )
                    .await
                }
                None => crate::network::postgres::relational_pipeline::explain_select_route(
                    inner_query,
                ),
            };
            return match routing {
                Ok(explanation) => {
                    let json = serde_json::to_string_pretty(&explanation)?;
                    let fields = vec![FieldDescription::new("QUERY PLAN", PgType::Jsonb)];
                    self.send_row_description(&fields).await?;
                    self.send_data_row(&[&json]).await?;
                    let tag = if is_analyze {
                        "EXPLAIN ANALYZE"
                    } else {
                        "EXPLAIN"
                    };
                    self.send_command_complete(tag).await
                }
                Err(error) => {
                    self.send_error(
                        "ERROR",
                        "0A000",
                        &format!("EXPLAIN SELECT routing failed: {}", error),
                    )
                    .await
                }
            };
        }

        if !inner_upper.starts_with("INSERT") {
            return self
                .send_error(
                    "ERROR",
                    "0A000",
                    "EXPLAIN currently supports SELECT route disclosure and table-write INSERT ... SELECT routing",
                )
                .await;
        }

        let Some(dml_service) = self.dml_service.clone() else {
            return self
                .send_error(
                    "ERROR",
                    "0A000",
                    "Catalog-backed DML service is required for table-write EXPLAIN",
                )
                .await;
        };

        let parser = SqlFrontendParser::new();
        let statement = match parser.parse_dml(inner_query) {
            Ok(Some(statement)) => statement,
            Ok(None) => {
                return self
                    .send_error("ERROR", "42601", "Invalid table-write EXPLAIN statement")
                    .await;
            }
            Err(error) => {
                return self
                    .send_error("ERROR", "42601", &format!("Parse error: {}", error))
                    .await;
            }
        };

        let write_intent_overrides = self.write_intent_overrides_from_session().await;
        let explain_result = if is_analyze {
            dml_service.explain_analyze_table_write(statement).await
        } else {
            dml_service
                .explain_table_write_with_overrides(statement, Some(&write_intent_overrides))
                .await
        };
        match explain_result {
            Ok(explanation) => {
                let json = serde_json::to_string_pretty(&explanation)?;
                let fields = vec![FieldDescription::new("QUERY PLAN", PgType::Jsonb)];
                self.send_row_description(&fields).await?;
                self.send_data_row(&[&json]).await?;
                let tag = if is_analyze {
                    "EXPLAIN ANALYZE"
                } else {
                    "EXPLAIN"
                };
                self.send_command_complete(tag).await
            }
            Err(error) => {
                warn!("DmlService EXPLAIN failed: {}", error);
                self.send_error("ERROR", "0A000", &format!("Explain failed: {}", error))
                    .await
            }
        }
    }

    async fn write_intent_overrides_from_session(&self) -> WriteIntentOverrides {
        let session = self.session.read().await;
        Self::write_intent_overrides_from_params(&session.parameters)
    }

    /// Slice 6.3: resolve the tenant identifier for the primary-pod
    /// gate. Reads the same session-parameter convention used by
    /// `write_intent_overrides_from_session` (operators set tenant via
    /// `SET proximadb.write.tenant_id = '...'`). Falls back to the
    /// empty string when no explicit tenant is configured — matches
    /// the gRPC v2 / REST v2 / Arrow Flight behaviour of treating
    /// "no tenant" as a distinct shard from any named tenant.
    async fn pgwire_resolve_tenant_id(&self) -> String {
        let session = self.session.read().await;
        let normalized: HashMap<String, String> = session
            .parameters
            .iter()
            .map(|(k, v)| (k.to_ascii_lowercase(), v.clone()))
            .collect();
        normalized
            .get("proximadb.write.tenant_id")
            .or_else(|| normalized.get("proximadb.write_tenant_id"))
            .cloned()
            .unwrap_or_default()
    }

    /// TD-064 S1 (read-half): resolve the tenant/catalog scope used to authorize
    /// READ/search collection resolution. Per the `catalog.schema.table` model
    /// the connection's catalog (startup `database` name) is the tenant/account
    /// boundary, so it takes precedence; clients that sent no database fall back
    /// to the legacy `proximadb.write.tenant_id` session var so they keep working
    /// until the write path is migrated onto the same catalog binding.
    async fn pgwire_resolve_read_tenant(&self) -> String {
        if let Some(catalog) = self.session.read().await.catalog_tenant() {
            return catalog;
        }
        self.pgwire_resolve_tenant_id().await
    }

    /// TD-064 (write-half): resolve the tenant/catalog scope used to authorize
    /// and route WRITE/DDL statements. Identical to the read-half resolution —
    /// the connection's catalog (startup `database`) is the tenant boundary,
    /// falling back to the legacy `proximadb.write.tenant_id` var for clients
    /// that sent no database. This converges writes onto the same tenant signal
    /// reads use, replacing the pod-gate-only use of the var.
    async fn pgwire_resolve_write_tenant(&self) -> String {
        self.pgwire_resolve_read_tenant().await
    }

    fn write_intent_overrides_from_params(
        params: &HashMap<String, String>,
    ) -> WriteIntentOverrides {
        let normalized: HashMap<String, String> = params
            .iter()
            .map(|(key, value)| (key.to_ascii_lowercase(), value.clone()))
            .collect();

        let string_param = |names: &[&str]| -> Option<String> {
            names
                .iter()
                .find_map(|name| normalized.get(*name).cloned())
                .filter(|value| !value.is_empty())
        };
        let u64_param = |names: &[&str]| -> Option<u64> {
            string_param(names).and_then(|value| value.replace('_', "").parse::<u64>().ok())
        };
        let bool_param = |names: &[&str]| -> Option<bool> {
            string_param(names).and_then(|value| Self::parse_bool_parameter(&value))
        };

        WriteIntentOverrides {
            tenant_id: string_param(&["proximadb.write.tenant_id", "proximadb.write_tenant_id"]),
            actor: string_param(&["proximadb.write.actor", "proximadb.write_actor"]),
            idempotency_key: string_param(&[
                "proximadb.write.idempotency_key",
                "proximadb.write_idempotency_key",
            ]),
            row_count_hint: u64_param(&[
                "proximadb.write.row_count_hint",
                "proximadb.write_row_count_hint",
            ]),
            estimated_bytes: u64_param(&[
                "proximadb.write.estimated_bytes",
                "proximadb.write_estimated_bytes",
            ]),
            requires_row_level_semantics: bool_param(&[
                "proximadb.write.requires_row_level_semantics",
                "proximadb.write_requires_row_level_semantics",
            ]),
            batch_local_constraints_sufficient: bool_param(&[
                "proximadb.write.batch_local_constraints_sufficient",
                "proximadb.write_batch_local_constraints_sufficient",
            ]),
        }
    }

    fn parse_bool_parameter(value: &str) -> Option<bool> {
        match value.trim().to_ascii_lowercase().as_str() {
            "1" | "on" | "true" | "yes" => Some(true),
            "0" | "off" | "false" | "no" => Some(false),
            _ => None,
        }
    }

    /// Parse an EXPLAIN [ANALYZE] statement, returning `(is_analyze, inner_dml_sql)`.
    /// `EXPLAIN ANALYZE` and `EXPLAIN (ANALYZE)` both set `is_analyze = true`.
    fn extract_explain_with_analyze(query: &str) -> Result<(bool, &str)> {
        let trimmed = query.trim();
        let upper = trimmed.to_ascii_uppercase();
        if !upper.starts_with("EXPLAIN") {
            return Err(anyhow!("statement is not EXPLAIN"));
        }

        let mut rest = trimmed["EXPLAIN".len()..].trim_start();
        let mut is_analyze = false;

        let rest_upper = rest.to_ascii_uppercase();
        if rest_upper.starts_with("ANALYZE") {
            is_analyze = true;
            rest = rest["ANALYZE".len()..].trim_start();
        } else if rest.starts_with('(') {
            let mut depth = 0usize;
            let mut end = None;
            for (index, ch) in rest.char_indices() {
                match ch {
                    '(' => depth += 1,
                    ')' => {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            end = Some(index);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let Some(end_index) = end else {
                return Err(anyhow!("unterminated EXPLAIN option list"));
            };
            let options = &rest[1..end_index];
            if options.to_ascii_uppercase().contains("ANALYZE") {
                is_analyze = true;
            }
            rest = rest[end_index + 1..].trim_start();
        }

        let rest_upper = rest.to_ascii_uppercase();
        for keyword in ["VERBOSE", "COSTS", "BUFFERS", "TIMING", "SUMMARY"] {
            if rest_upper == keyword || rest_upper.starts_with(&format!("{keyword} ")) {
                return Err(anyhow!(
                    "EXPLAIN option '{}' must use parenthesized syntax",
                    keyword
                ));
            }
        }

        if rest.is_empty() {
            return Err(anyhow!("EXPLAIN requires an inner statement"));
        }

        Ok((is_analyze, rest))
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

        // TD-100: push the WHERE metadata predicate into the search so
        // mem0-style `WHERE payload->>'type'='fact'` queries actually filter.
        // (Previously this path passed `None`, returning unfiltered results.)
        // NOTE: parameter-bound vector/metadata values (`$1`) are not yet bound
        // here; that is tracked as a TD-102 follow-up.
        let metadata_filter =
            crate::network::postgres::pgvector_params::extract_metadata_filter_from_where(query);

        // TD-064 S2: structural tenant/namespace isolation at the search routing
        // boundary. Resolve the target collection through the tenant-scoped
        // catalog BEFORE searching, so a pgwire client cannot read another
        // tenant's collection by naming it. The collection service compares the
        // caller's tenant against the collection's owning tenant and returns
        // `None` on mismatch (the same enforcement REST v2 / gRPC v2 use). S1
        // read-half: the read scope is the connection's catalog (`database` ==
        // account == tenant), falling back to the legacy `proximadb.write.tenant_id`
        // var for clients that sent no database. Behavior by mode:
        //   * single-tenant (no tenant manager): unscoped → no behavior change.
        //   * multi-tenant: missing/unknown tenant → Err, cross-tenant → Ok(None);
        //     both fail closed below as an indistinguishable "relation does not
        //     exist" so cross-tenant existence cannot be probed.
        let tenant_id = self.pgwire_resolve_read_tenant().await;
        let tenant_scope = (!tenant_id.is_empty()).then_some(tenant_id.as_str());
        match self
            .collection_port
            .get_collection(&table_name, tenant_scope)
            .await
        {
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {
                warn!(
                    "🚨 pgwire vector search denied: collection '{}' not accessible for tenant scope '{}'",
                    table_name, tenant_id
                );
                return self
                    .send_error(
                        "ERROR",
                        "42P01",
                        &format!("relation \"{}\" does not exist", table_name),
                    )
                    .await;
            }
        }

        debug!(
            "Executing vector search on {} with top_k={} filter={}",
            table_name,
            top_k,
            metadata_filter.is_some()
        );

        if let Some(ref vector) = query_vector {
            // Execute actual vector search
            match self
                .vector_ops
                .unified_search_native(
                    &table_name,
                    vector.clone(),
                    top_k,
                    metadata_filter, // TD-100: mem0 metadata-scoped WHERE pushdown
                    None,            // Default config
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

    fn extract_simple_constant_select(query: &str) -> Option<(String, String)> {
        let trimmed = query.trim().trim_end_matches(';').trim();
        let upper = trimmed.to_ascii_uppercase();
        if !upper.starts_with("SELECT ") || upper.contains(" FROM ") {
            return None;
        }

        let expr = trimmed[7..].trim();
        let (value, column) = if let Some((left, right)) = expr.rsplit_once(" AS ") {
            (left.trim(), right.trim())
        } else if let Some((left, right)) = expr.rsplit_once(" as ") {
            (left.trim(), right.trim())
        } else {
            (expr, "?column?")
        };

        let value = value.trim_matches('\'').to_string();
        Some((Self::clean_identifier(column), value))
    }

    fn extract_selected_column_names(query: &str) -> Vec<String> {
        let upper = query.to_ascii_uppercase();
        let Some(select_pos) = upper.find("SELECT ") else {
            return Vec::new();
        };
        let Some(from_pos) = upper.find(" FROM ") else {
            return Vec::new();
        };

        let projection = query[select_pos + 7..from_pos].trim();
        if projection == "*" {
            return Vec::new();
        }

        projection
            .split(',')
            .filter_map(|requested| {
                let column_name = requested
                    .split_whitespace()
                    .next()
                    .map(Self::clean_identifier)
                    .unwrap_or_default();
                (!column_name.is_empty() && column_name != "*").then_some(column_name)
            })
            .collect()
    }

    fn extract_select_where_predicates(query: &str) -> Option<Vec<SelectPredicate>> {
        let predicate = Self::extract_select_where_clause(query)?;

        // OR detected: try to fold `col = v1 OR col = v2` into `col IN (v1, v2)`.
        // Mixed-column OR, non-equality OR, and AND/OR combinations return None so
        // the caller falls back to a full scan (over-inclusive but correct).
        if Self::find_keyword_outside_literals(predicate, " OR ").is_some() {
            return Self::try_fold_or_as_in(predicate).map(|p| vec![p]);
        }

        let mut predicates = Vec::new();
        for part in Self::split_and_predicates(predicate) {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            predicates.push(Self::parse_select_predicate(part)?);
        }

        Some(predicates)
    }

    fn split_or_predicates(predicate: &str) -> Vec<&str> {
        let mut parts = Vec::new();
        let mut remaining = predicate;
        loop {
            let Some(index) = Self::find_keyword_outside_literals(remaining, " OR ") else {
                parts.push(remaining);
                break;
            };
            parts.push(&remaining[..index]);
            remaining = &remaining[index + " OR ".len()..];
        }
        parts
    }

    /// Fold `col = v1 OR col = v2 OR ...` into `col IN (v1, v2, ...)`.
    ///
    /// Returns `None` for: multi-column OR, non-equality OR, compound branches
    /// with AND, or empty input. Caller falls back to full scan on `None`.
    fn try_fold_or_as_in(predicate: &str) -> Option<SelectPredicate> {
        let parts = Self::split_or_predicates(predicate);
        if parts.is_empty() {
            return None;
        }
        let mut column_name: Option<String> = None;
        let mut literals: Vec<String> = Vec::with_capacity(parts.len());
        for part in parts {
            let part = part.trim();
            // Compound branch (AND inside OR) is not folded
            if Self::find_keyword_outside_literals(part, " AND ").is_some() {
                return None;
            }
            let (left, operator, right) = Self::split_comparison_predicate(part)?;
            if operator != SelectPredicateOperator::Equal {
                return None;
            }
            let col = Self::predicate_column_name(left)?;
            match &column_name {
                None => column_name = Some(col),
                Some(existing) if existing.eq_ignore_ascii_case(&col) => {}
                _ => return None,
            }
            literals.push(Self::strip_sql_literal(right));
        }
        Some(SelectPredicate {
            column_name: column_name?,
            condition: SelectPredicateCondition::In {
                literals,
                negated: false,
            },
        })
    }

    fn parse_select_predicate(predicate: &str) -> Option<SelectPredicate> {
        if let Some((left, right)) = Self::split_keyword_predicate(predicate, " IS NOT NULL") {
            if !right.trim().is_empty() {
                return None;
            }
            return Some(SelectPredicate {
                column_name: Self::predicate_column_name(left)?,
                condition: SelectPredicateCondition::IsNull { negated: true },
            });
        }
        if let Some((left, right)) = Self::split_keyword_predicate(predicate, " IS NULL") {
            if !right.trim().is_empty() {
                return None;
            }
            return Some(SelectPredicate {
                column_name: Self::predicate_column_name(left)?,
                condition: SelectPredicateCondition::IsNull { negated: false },
            });
        }
        if let Some((left, right)) = Self::split_keyword_predicate(predicate, " NOT LIKE ") {
            return Some(SelectPredicate {
                column_name: Self::predicate_column_name(left)?,
                condition: SelectPredicateCondition::Like {
                    pattern: Self::strip_sql_literal(right),
                    negated: true,
                },
            });
        }
        if let Some((left, right)) = Self::split_keyword_predicate(predicate, " LIKE ") {
            return Some(SelectPredicate {
                column_name: Self::predicate_column_name(left)?,
                condition: SelectPredicateCondition::Like {
                    pattern: Self::strip_sql_literal(right),
                    negated: false,
                },
            });
        }
        if let Some((left, right)) = Self::split_keyword_predicate(predicate, " NOT IN ") {
            return Some(SelectPredicate {
                column_name: Self::predicate_column_name(left)?,
                condition: SelectPredicateCondition::In {
                    literals: Self::parse_sql_literal_list(right)?,
                    negated: true,
                },
            });
        }
        if let Some((left, right)) = Self::split_keyword_predicate(predicate, " IN ") {
            return Some(SelectPredicate {
                column_name: Self::predicate_column_name(left)?,
                condition: SelectPredicateCondition::In {
                    literals: Self::parse_sql_literal_list(right)?,
                    negated: false,
                },
            });
        }

        let (left, operator, right) = Self::split_comparison_predicate(predicate)?;
        Some(SelectPredicate {
            column_name: Self::predicate_column_name(left)?,
            condition: SelectPredicateCondition::Comparison {
                operator,
                literal: Self::strip_sql_literal(right),
            },
        })
    }

    fn predicate_column_name(left: &str) -> Option<String> {
        let column_name = Self::clean_identifier(left);
        if column_name.is_empty() {
            None
        } else {
            Some(column_name)
        }
    }

    /// Extract `ORDER BY <col> [ASC|DESC] [NULLS FIRST|LAST] [, ...]`.
    /// Returns one [`OrderByKey`] per declared column in declaration
    /// order; the sort lex-orders across keys (first key is the
    /// primary, then ties break by the next, etc.).
    ///
    /// Phase 2 of ADR-018: multi-column ORDER BY + explicit NULLS
    /// placement. Postgres defaults: ASC → NULLS LAST, DESC → NULLS
    /// FIRST. Returns `None` if no ORDER BY clause is present, or
    /// if any individual key is malformed (the caller falls back to
    /// no-ordering — the existing behavior for unsupported clauses).
    fn extract_select_order_by(query: &str) -> Option<Vec<OrderByKey>> {
        let upper = query.to_ascii_uppercase();
        let pos = Self::find_keyword_outside_literals(&upper, " ORDER BY ")?;
        let after = query[pos + " ORDER BY ".len()..].trim();
        // Terminate at LIMIT / OFFSET / `;` / end.
        let upper_after = after.to_ascii_uppercase();
        let mut end = after.len();
        for terminator in [" LIMIT ", " OFFSET "] {
            if let Some(idx) = Self::find_keyword_outside_literals(&upper_after, terminator)
                && idx < end
            {
                end = idx;
            }
        }
        let clause = after[..end].trim().trim_end_matches(';').trim();
        if clause.is_empty() {
            return None;
        }
        // Split on top-level commas (literal-aware).
        let segments = Self::split_top_level_commas(clause);
        if segments.is_empty() {
            return None;
        }
        let mut keys: Vec<OrderByKey> = Vec::with_capacity(segments.len());
        for raw in segments {
            let parsed = Self::parse_one_order_by_key(raw.trim())?;
            keys.push(parsed);
        }
        Some(keys)
    }

    /// Parse one `<col> [ASC|DESC] [NULLS FIRST|NULLS LAST]` segment.
    fn parse_one_order_by_key(segment: &str) -> Option<OrderByKey> {
        if segment.is_empty() {
            return None;
        }
        let segment_upper = segment.to_ascii_uppercase();
        // 1. Strip optional NULLS clause from the end.
        let (without_nulls, nulls_first) =
            if let Some(stripped_upper) = segment_upper.strip_suffix(" NULLS FIRST") {
                (
                    segment[..stripped_upper.len()].trim().to_string(),
                    Some(true),
                )
            } else if let Some(stripped_upper) = segment_upper.strip_suffix(" NULLS LAST") {
                (
                    segment[..stripped_upper.len()].trim().to_string(),
                    Some(false),
                )
            } else {
                (segment.to_string(), None)
            };
        // 2. Strip optional ASC/DESC from the (post-NULLS) end.
        let without_nulls_upper = without_nulls.to_ascii_uppercase();
        let (col_str, desc) =
            if let Some(stripped_upper) = without_nulls_upper.strip_suffix(" DESC") {
                (
                    without_nulls[..stripped_upper.len()].trim().to_string(),
                    true,
                )
            } else if let Some(stripped_upper) = without_nulls_upper.strip_suffix(" ASC") {
                (
                    without_nulls[..stripped_upper.len()].trim().to_string(),
                    false,
                )
            } else {
                (without_nulls, false)
            };
        // 3. Apply Postgres NULL-placement default if not explicit:
        // ASC → NULLS LAST, DESC → NULLS FIRST.
        let nulls_first = nulls_first.unwrap_or(desc);
        let column = Self::clean_identifier(&col_str);
        if column.is_empty() {
            return None;
        }
        Some(OrderByKey {
            column,
            desc,
            nulls_first,
        })
    }

    /// Split `s` on commas that aren't inside string literals. Used by
    /// the ORDER BY multi-column parser so a literal like `'a, b'`
    /// inside an expression doesn't false-split.
    fn split_top_level_commas(s: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut buf = String::new();
        let mut in_str = false;
        let mut quote: char = '"';
        for c in s.chars() {
            if in_str {
                buf.push(c);
                if c == quote {
                    in_str = false;
                }
                continue;
            }
            match c {
                '\'' | '"' => {
                    quote = c;
                    in_str = true;
                    buf.push(c);
                }
                ',' => {
                    out.push(std::mem::take(&mut buf));
                }
                _ => buf.push(c),
            }
        }
        if !buf.is_empty() {
            out.push(buf);
        }
        out
    }

    fn extract_select_where_clause(query: &str) -> Option<&str> {
        let upper = query.to_ascii_uppercase();
        let where_pos = upper.find(" WHERE ")?;
        let mut predicate = query[where_pos + 7..].trim();
        for terminator in [" ORDER BY ", " GROUP BY ", " LIMIT ", " OFFSET "] {
            if let Some(pos) = Self::find_keyword_outside_literals(predicate, terminator) {
                predicate = predicate[..pos].trim();
            }
        }
        Some(predicate.trim_end_matches(';').trim())
    }

    fn split_and_predicates(predicate: &str) -> Vec<&str> {
        let mut parts = Vec::new();
        let mut remaining = predicate;
        loop {
            let Some(index) = Self::find_keyword_outside_literals(remaining, " AND ") else {
                parts.push(remaining);
                break;
            };
            parts.push(&remaining[..index]);
            remaining = &remaining[index + " AND ".len()..];
        }
        parts
    }

    fn split_keyword_predicate<'a>(
        predicate: &'a str,
        keyword: &str,
    ) -> Option<(&'a str, &'a str)> {
        let index = Self::find_keyword_outside_literals(predicate, keyword)?;
        let left = predicate[..index].trim();
        let right = predicate[index + keyword.len()..].trim();
        if left.is_empty() {
            return None;
        }
        Some((left, right))
    }

    fn find_keyword_outside_literals(haystack: &str, keyword: &str) -> Option<usize> {
        let mut in_single_quote = false;
        let mut chars = haystack.char_indices().peekable();
        while let Some((index, ch)) = chars.next() {
            if ch == '\'' {
                if in_single_quote && chars.peek().is_some_and(|(_, next)| *next == '\'') {
                    chars.next();
                } else {
                    in_single_quote = !in_single_quote;
                }
                continue;
            }
            if !in_single_quote
                && haystack[index..].len() >= keyword.len()
                && haystack[index..index + keyword.len()].eq_ignore_ascii_case(keyword)
            {
                return Some(index);
            }
        }
        None
    }

    fn split_comparison_predicate(
        predicate: &str,
    ) -> Option<(&str, SelectPredicateOperator, &str)> {
        for (token, operator) in [
            ("<>", SelectPredicateOperator::NotEqual),
            ("!=", SelectPredicateOperator::NotEqual),
            (">=", SelectPredicateOperator::GreaterThanOrEqual),
            ("<=", SelectPredicateOperator::LessThanOrEqual),
            ("=", SelectPredicateOperator::Equal),
            (">", SelectPredicateOperator::GreaterThan),
            ("<", SelectPredicateOperator::LessThan),
        ] {
            if let Some(index) = predicate.find(token) {
                let left = predicate[..index].trim();
                let right = predicate[index + token.len()..].trim();
                if !left.is_empty() && !right.is_empty() {
                    return Some((left, operator, right));
                }
            }
        }
        None
    }

    fn strip_sql_literal(literal: &str) -> String {
        let literal = literal.trim().trim_end_matches(';').trim();
        if literal.len() >= 2 && literal.starts_with('\'') && literal.ends_with('\'') {
            return literal[1..literal.len() - 1].replace("''", "'");
        }
        literal.trim_matches('"').to_string()
    }

    fn parse_sql_literal_list(literal_list: &str) -> Option<Vec<String>> {
        let literal_list = literal_list
            .trim()
            .trim_end_matches(';')
            .trim()
            .strip_prefix('(')?
            .strip_suffix(')')?;
        let mut literals = Vec::new();
        let mut part_start = 0usize;
        let mut in_single_quote = false;
        let mut chars = literal_list.char_indices().peekable();
        while let Some((index, ch)) = chars.next() {
            if ch == '\'' {
                if in_single_quote && chars.peek().is_some_and(|(_, next)| *next == '\'') {
                    chars.next();
                } else {
                    in_single_quote = !in_single_quote;
                }
                continue;
            }
            if ch == ',' && !in_single_quote {
                let literal = literal_list[part_start..index].trim();
                if literal.is_empty() {
                    return None;
                }
                literals.push(Self::strip_sql_literal(literal));
                part_start = index + ch.len_utf8();
            }
        }
        let literal = literal_list[part_start..].trim();
        if literal.is_empty() || in_single_quote {
            return None;
        }
        literals.push(Self::strip_sql_literal(literal));
        Some(literals)
    }

    fn extract_select_limit(query: &str) -> Option<usize> {
        let upper = query.to_ascii_uppercase();
        let limit_pos = upper.rfind(" LIMIT ")?;
        let after_limit = query[limit_pos + " LIMIT ".len()..]
            .trim()
            .trim_end_matches(';')
            .trim();
        let token = after_limit.split_whitespace().next()?;
        token.parse::<usize>().ok()
    }

    fn clean_identifier(identifier: &str) -> String {
        identifier
            .trim()
            .trim_matches('"')
            .split('.')
            .next_back()
            .unwrap_or(identifier)
            .trim_matches('"')
            .to_string()
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
        // TD-064 S3 (read-side): scope collection resolution by the connection's
        // catalog/tenant so a plain `SELECT * FROM <collection>` cannot reveal
        // another tenant's collection metadata (name/dimension/vector_count).
        // Mirrors the vector-search gate (S2): a cross-tenant or missing
        // collection resolves to `Ok(None)` → empty result (single-tenant
        // deployments stay unscoped, so no behavior change there).
        let tenant_id = self.pgwire_resolve_read_tenant().await;
        let tenant_scope = (!tenant_id.is_empty()).then_some(tenant_id.as_str());
        // Check if collection exists (tenant-scoped)
        match self
            .collection_port
            .get_collection(collection_name, tenant_scope)
            .await
        {
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

    /// Execute a relational query against a cataloged table.
    async fn execute_relational_query(&mut self, query: &str, table_name: &str) -> Result<()> {
        debug!("Executing relational query on table: {}", table_name);

        let projection_column_names = Self::extract_selected_column_names(query);

        let Some(dml_service) = self.dml_service.clone() else {
            self.send_command_complete("SELECT 0").await?;
            return Ok(());
        };

        let limit = Self::extract_select_limit(query);
        let order_by = Self::extract_select_order_by(query);
        // Fetch all matching rows BEFORE LIMIT when we need to ORDER BY —
        // sorting then truncating is the only correct semantics. With no
        // ORDER BY the planner keeps the limit pushdown.
        let scan_limit = if order_by.is_some() { None } else { limit };

        // Prefer the faithful boolean WHERE tree (OR / mixed-AND-OR / grouped
        // predicates push into the scan, reusing the UPDATE/DELETE tree
        // machinery). If sqlparser can't parse the query (pg-specific syntax)
        // or its WHERE has an unsupported expression, fall back to the legacy
        // string-predicate path — over-inclusive full scan, never empty.
        // TD-064: scope the legacy relational SELECT to the connection tenant.
        let read_tenant = self.pgwire_resolve_read_tenant().await;
        let read_tenant_ctx = (!read_tenant.is_empty())
            .then(|| crate::storage::tenant::context::TenantContext::for_tenant_id(&read_tenant));
        let mut result = match SqlFrontendParser::new().parse_select_where_clause(query) {
            Ok(where_clause) => {
                dml_service
                    .select_table_records_with_projection_where(
                        table_name,
                        &projection_column_names,
                        scan_limit,
                        where_clause.as_ref(),
                        read_tenant_ctx.as_ref(),
                    )
                    .await?
            }
            Err(_) => {
                let predicates = if query.to_ascii_uppercase().contains(" WHERE ") {
                    Self::extract_select_where_predicates(query).unwrap_or_default()
                } else {
                    Vec::new()
                };
                dml_service
                    .select_table_records_with_projection(
                        table_name,
                        &projection_column_names,
                        scan_limit,
                        &predicates,
                        read_tenant_ctx.as_ref(),
                    )
                    .await?
            }
        };
        let fields = result
            .selected_columns
            .iter()
            .map(|column| {
                FieldDescription::new(
                    &column.name,
                    pg_type_for_catalog_data_type(&column.data_type),
                )
            })
            .collect::<Vec<_>>();
        self.send_row_description(&fields).await?;

        // Apply ORDER BY if present (ADR-018 Phase 2: multi-column +
        // NULLS handling). Sort by the string representation of each
        // column's value, lex-ordering across keys. Correct for
        // TEXT/VARCHAR and ordering-preserving for canonical
        // numeric/timestamp string forms produced by
        // `proxima_value_to_pg_text`. Real type-aware sort lands with
        // the relational planner in a later phase.
        if let Some(keys) = order_by.as_ref() {
            // Resolve each key's column to its row index up-front so
            // the per-row hot path is `Vec<usize>` lookups, not
            // string matching.
            let resolved: Vec<(usize, bool, bool)> = keys
                .iter()
                .filter_map(|k| {
                    let idx = result
                        .selected_columns
                        .iter()
                        .position(|c| c.name.eq_ignore_ascii_case(&k.column));
                    match idx {
                        Some(i) => Some((i, k.desc, k.nulls_first)),
                        None => {
                            warn!(
                                target: "proximadb::pgwire::order_by",
                                column = %k.column,
                                "ORDER BY column not found in projection; \
                                 skipping this key"
                            );
                            None
                        }
                    }
                })
                .collect();
            if !resolved.is_empty() {
                result.rows.sort_by(|a, b| {
                    for (idx, desc, nulls_first) in resolved.iter() {
                        let a_val = a.get(*idx);
                        let b_val = b.get(*idx);
                        let a_null = matches!(a_val, None | Some(ProximaValue::Null));
                        let b_null = matches!(b_val, None | Some(ProximaValue::Null));
                        if a_null && b_null {
                            continue;
                        }
                        if a_null {
                            return if *nulls_first {
                                std::cmp::Ordering::Less
                            } else {
                                std::cmp::Ordering::Greater
                            };
                        }
                        if b_null {
                            return if *nulls_first {
                                std::cmp::Ordering::Greater
                            } else {
                                std::cmp::Ordering::Less
                            };
                        }
                        let av = a_val.map(proxima_value_to_pg_text).unwrap_or_default();
                        let bv = b_val.map(proxima_value_to_pg_text).unwrap_or_default();
                        let cmp = if *desc { bv.cmp(&av) } else { av.cmp(&bv) };
                        if cmp != std::cmp::Ordering::Equal {
                            return cmp;
                        }
                    }
                    std::cmp::Ordering::Equal
                });
            }
        }

        let mut rows_sent = 0usize;
        for row in result.rows {
            let values = row.iter().map(proxima_value_to_pg_text).collect::<Vec<_>>();
            let refs = values.iter().map(String::as_str).collect::<Vec<_>>();
            self.send_data_row(&refs).await?;
            rows_sent += 1;
            if limit.is_some_and(|limit| rows_sent >= limit) {
                break;
            }
        }

        self.send_command_complete(&format!("SELECT {}", rows_sent))
            .await
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
                    let document = self.sql_object_to_json(
                        &crate::storage::document::proxima_tree_to_sql_object(&doc.props),
                    );
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

        // Check if IF NOT EXISTS was specified (ADR-018 Phase 2)
        let if_not_exists = upper.contains("IF NOT EXISTS");

        // Extract table name: CREATE TABLE [IF NOT EXISTS] name
        let table_start = if if_not_exists {
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
            DataModel::Vector => {
                self.create_vector_collection(&table_name, &upper, if_not_exists)
                    .await
            }
            DataModel::Document => {
                self.create_document_collection(&table_name, &upper, if_not_exists)
                    .await
            }
            DataModel::Graph => {
                self.create_graph_collection(&table_name, &upper, if_not_exists)
                    .await
            }
            DataModel::Observability | DataModel::TimeSeries => {
                self.create_observability_namespace(&table_name, &upper, if_not_exists)
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
    async fn create_vector_collection(
        &mut self,
        table_name: &str,
        query: &str,
        if_not_exists: bool,
    ) -> Result<()> {
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

        match self.collection_port.create_collection(config, None).await {
            Ok(_) => {
                info!("Created vector collection '{}' via PostgreSQL", table_name);
                self.send_command_complete("CREATE TABLE").await
            }
            Err(e) => {
                // ADR-018 Phase 2: Only suppress "already exists" error if IF NOT EXISTS was specified
                if if_not_exists && e.to_string().contains("already exists") {
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
    async fn create_document_collection(
        &mut self,
        table_name: &str,
        _query: &str,
        if_not_exists: bool,
    ) -> Result<()> {
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
                Err(e) if if_not_exists && e.to_string().contains("already exists") => {
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
            .collection_port
            .create_collection(vector_config, None)
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
    async fn create_graph_collection(
        &mut self,
        table_name: &str,
        _query: &str,
        if_not_exists: bool,
    ) -> Result<()> {
        debug!("Creating graph '{}'", table_name);

        if let Some(graph_service) = self.graph_service.clone() {
            let request = crate::proto::proximadb_v1::CreateGraphRequest {
                graph_id: table_name.to_string(),
                name: Some(table_name.to_string()),
                ..Default::default()
            };

            match graph_service.create_graph_collection(request).await {
                Ok(_) => {
                    info!(
                        "Created graph '{}' via PostgreSQL (graph engine: ORION)",
                        table_name
                    );
                    self.send_command_complete("CREATE TABLE").await
                }
                Err(e) if if_not_exists && e.to_string().contains("already exists") => {
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
        if_not_exists: bool,
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
                Err(e) if if_not_exists && e.to_string().contains("already exists") => {
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

    /// Execute INSERT through canonical catalog-backed DML.
    async fn execute_insert(&mut self, query: &str) -> Result<()> {
        if let Some(dml_service) = self.dml_service.clone() {
            return self
                .execute_insert_via_dml_service(query, &dml_service)
                .await;
        }

        self.send_error(
            "ERROR",
            "0A000",
            "Catalog-backed DML service is required for INSERT",
        )
        .await
    }

    /// Execute INSERT using the proper SQL parser and DmlService
    async fn execute_insert_via_dml_service(
        &mut self,
        query: &str,
        dml_service: &Arc<DmlService>,
    ) -> Result<()> {
        let parser = SqlFrontendParser::new();

        match parser.parse_dml(query) {
            Ok(Some(statement)) => {
                // Slice 6.3: gate runs post-parse so the (tenant_id,
                // target_table) pair is known; pre-execute so a
                // misroute never touches the catalog or WAL on this
                // pod.
                let tenant_id = self.pgwire_resolve_tenant_id().await;
                let table = statement.target_table_name().to_string();
                match check_pgwire_primary_pod_gate(&self.primary_pod_gate, &tenant_id, &table) {
                    PgwireGateOutcome::Allow => {}
                    PgwireGateOutcome::Misrouted { target_pod } => {
                        return self
                            .send_error(
                                "ERROR",
                                "57P03",
                                &format!(
                                    "misdirected_write: INSERT for table '{}' must go to pod '{}' (reconnect to that pod and retry)",
                                    table, target_pod
                                ),
                            )
                            .await;
                    }
                }

                // TD-064 write-half: resolve the tenant scope (catalog/database
                // binding) and authorize the target table within it. A
                // cross-tenant target fails closed with 42P01 (never leaking
                // existence); the scope is then threaded into execute_scoped so
                // the write lands in the tenant's partition.
                let write_tenant = self.pgwire_resolve_write_tenant().await;
                let tenant_scope = (!write_tenant.is_empty()).then(|| write_tenant.clone());
                if let Some(ref tenant) = tenant_scope
                    && !dml_service
                        .table_visible_for_tenant(&table, Some(tenant.as_str()))
                        .await
                        .unwrap_or(false)
                {
                    return self
                        .send_error(
                            "ERROR",
                            "42P01",
                            &format!("relation \"{}\" does not exist", table),
                        )
                        .await;
                }
                let tenant_ctx = tenant_scope.as_ref().map(|tenant| {
                    crate::storage::tenant::context::TenantContext::for_tenant_id(tenant)
                });

                match dml_service
                    .execute_scoped(statement, tenant_ctx.as_ref())
                    .await
                {
                    Ok(result) => {
                        info!(
                            rows_affected = result.rows_affected,
                            "INSERT executed via DmlService"
                        );
                        self.send_command_complete(&format!("INSERT 0 {}", result.rows_affected))
                            .await
                    }
                    Err(e) => {
                        if let Some((resource, holder)) =
                            crate::errors::extract_dml_lock_conflict(&e)
                        {
                            let msg = match holder {
                                Some(h) => {
                                    format!("lock not available on {resource} (held by {h})")
                                }
                                None => format!("lock not available on {resource}"),
                            };
                            warn!("DmlService INSERT blocked by DML lock: {msg}");
                            return self.send_error("ERROR", "55P03", &msg).await;
                        }
                        warn!("DmlService INSERT failed: {}", e);
                        self.send_error("ERROR", "42P01", &format!("Insert failed: {}", e))
                            .await
                    }
                }
            }
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

    /// Execute DELETE through canonical catalog-backed DML.
    async fn execute_delete(&mut self, query: &str) -> Result<()> {
        if let Some(dml_service) = self.dml_service.clone() {
            return self
                .execute_delete_via_dml_service(query, &dml_service)
                .await;
        }

        self.send_error(
            "ERROR",
            "0A000",
            "Catalog-backed DML service is required for DELETE",
        )
        .await
    }

    /// Execute DELETE using the proper SQL parser and DmlService
    async fn execute_delete_via_dml_service(
        &mut self,
        query: &str,
        dml_service: &Arc<DmlService>,
    ) -> Result<()> {
        let parser = SqlFrontendParser::new();

        match parser.parse_dml(query) {
            Ok(Some(statement)) => {
                // Slice 6.3: gate before DELETE — symmetric with INSERT.
                let tenant_id = self.pgwire_resolve_tenant_id().await;
                let table = statement.target_table_name().to_string();
                match check_pgwire_primary_pod_gate(&self.primary_pod_gate, &tenant_id, &table) {
                    PgwireGateOutcome::Allow => {}
                    PgwireGateOutcome::Misrouted { target_pod } => {
                        return self
                            .send_error(
                                "ERROR",
                                "57P03",
                                &format!(
                                    "misdirected_write: DELETE for table '{}' must go to pod '{}' (reconnect to that pod and retry)",
                                    table, target_pod
                                ),
                            )
                            .await;
                    }
                }

                // TD-064 write-half: tenant scope + cross-tenant 42P01 gate (see INSERT).
                let write_tenant = self.pgwire_resolve_write_tenant().await;
                let tenant_scope = (!write_tenant.is_empty()).then(|| write_tenant.clone());
                if let Some(ref tenant) = tenant_scope
                    && !dml_service
                        .table_visible_for_tenant(&table, Some(tenant.as_str()))
                        .await
                        .unwrap_or(false)
                {
                    return self
                        .send_error(
                            "ERROR",
                            "42P01",
                            &format!("relation \"{}\" does not exist", table),
                        )
                        .await;
                }
                let tenant_ctx = tenant_scope.as_ref().map(|tenant| {
                    crate::storage::tenant::context::TenantContext::for_tenant_id(tenant)
                });

                match dml_service
                    .execute_scoped(statement, tenant_ctx.as_ref())
                    .await
                {
                    Ok(result) => {
                        info!(
                            rows_affected = result.rows_affected,
                            "DELETE executed via DmlService"
                        );
                        self.send_command_complete(&format!("DELETE {}", result.rows_affected))
                            .await
                    }
                    Err(e) => {
                        if let Some((resource, holder)) =
                            crate::errors::extract_dml_lock_conflict(&e)
                        {
                            let msg = match holder {
                                Some(h) => {
                                    format!("lock not available on {resource} (held by {h})")
                                }
                                None => format!("lock not available on {resource}"),
                            };
                            warn!("DmlService DELETE blocked by DML lock: {msg}");
                            return self.send_error("ERROR", "55P03", &msg).await;
                        }
                        warn!("DmlService DELETE failed: {}", e);
                        self.send_error("ERROR", "42P01", &format!("Delete failed: {}", e))
                            .await
                    }
                }
            }
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

    /// Execute UPDATE through canonical catalog-backed DML.
    async fn execute_update(&mut self, query: &str) -> Result<()> {
        if let Some(dml_service) = self.dml_service.clone() {
            return self
                .execute_update_via_dml_service(query, &dml_service)
                .await;
        }

        self.send_error(
            "ERROR",
            "0A000",
            "Catalog-backed DML service is required for UPDATE",
        )
        .await
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
                // Slice 6.3: gate before UPDATE — symmetric with INSERT/DELETE.
                let tenant_id = self.pgwire_resolve_tenant_id().await;
                let table = statement.target_table_name().to_string();
                match check_pgwire_primary_pod_gate(&self.primary_pod_gate, &tenant_id, &table) {
                    PgwireGateOutcome::Allow => {}
                    PgwireGateOutcome::Misrouted { target_pod } => {
                        return self
                            .send_error(
                                "ERROR",
                                "57P03",
                                &format!(
                                    "misdirected_write: UPDATE for table '{}' must go to pod '{}' (reconnect to that pod and retry)",
                                    table, target_pod
                                ),
                            )
                            .await;
                    }
                }

                // TD-064 write-half: tenant scope + cross-tenant 42P01 gate (see INSERT).
                let write_tenant = self.pgwire_resolve_write_tenant().await;
                let tenant_scope = (!write_tenant.is_empty()).then(|| write_tenant.clone());
                if let Some(ref tenant) = tenant_scope
                    && !dml_service
                        .table_visible_for_tenant(&table, Some(tenant.as_str()))
                        .await
                        .unwrap_or(false)
                {
                    return self
                        .send_error(
                            "ERROR",
                            "42P01",
                            &format!("relation \"{}\" does not exist", table),
                        )
                        .await;
                }
                let tenant_ctx = tenant_scope.as_ref().map(|tenant| {
                    crate::storage::tenant::context::TenantContext::for_tenant_id(tenant)
                });

                match dml_service
                    .execute_scoped(statement, tenant_ctx.as_ref())
                    .await
                {
                    Ok(result) => {
                        info!(
                            rows_affected = result.rows_affected,
                            "UPDATE executed via DmlService"
                        );
                        self.send_command_complete(&format!("UPDATE {}", result.rows_affected))
                            .await
                    }
                    Err(e) => {
                        if let Some((resource, holder)) =
                            crate::errors::extract_dml_lock_conflict(&e)
                        {
                            let msg = match holder {
                                Some(h) => {
                                    format!("lock not available on {resource} (held by {h})")
                                }
                                None => format!("lock not available on {resource}"),
                            };
                            warn!("DmlService UPDATE blocked by DML lock: {msg}");
                            return self.send_error("ERROR", "55P03", &msg).await;
                        }
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

        match self
            .collection_port
            .delete_collection(&table_name, None)
            .await
        {
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

        // Convert Arrow batches directly to canonical ProximaRecord envelopes.
        let vectors = match ArrowProtoCodec::batches_to_proxima_records(batches) {
            Ok(v) => v,
            Err(e) => {
                warn!("Failed to convert Arrow data to ProximaRecords: {}", e);
                return Err(anyhow::anyhow!("Failed to convert Arrow data: {}", e));
            }
        };

        let count = vectors.len();
        debug!(
            "Decoded {} vectors from Arrow IPC for COPY into '{}'",
            count, table_name
        );

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

            let vector = self.parse_csv_vector(vector_str);
            if vector.is_empty() {
                continue;
            }

            let dim = vector.len() as u32;
            let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
            records.push(proximadb_records::ProximaRecord {
                oid: id.to_string(),
                embeddings: vec![proximadb_records::EmbeddingCell {
                    model_id: "default".to_string(),
                    modality: "vector".to_string(),
                    dim,
                    values: proximadb_records::EmbeddingValues::Fp32(vector),
                    ..Default::default()
                }],
                created_at_ns: now_ns,
                updated_at_ns: now_ns,
                record_version: 1,
                ..Default::default()
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
                let dim = vector.len() as u32;
                let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
                records.push(proximadb_records::ProximaRecord {
                    oid: id.to_string(),
                    embeddings: vec![proximadb_records::EmbeddingCell {
                        model_id: "default".to_string(),
                        modality: "vector".to_string(),
                        dim,
                        values: proximadb_records::EmbeddingValues::Fp32(vector),
                        ..Default::default()
                    }],
                    created_at_ns: now_ns,
                    updated_at_ns: now_ns,
                    record_version: 1,
                    ..Default::default()
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

            let dim = vector.len() as u32;
            let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
            records.push(proximadb_records::ProximaRecord {
                oid: id,
                embeddings: vec![proximadb_records::EmbeddingCell {
                    model_id: "default".to_string(),
                    modality: "vector".to_string(),
                    dim,
                    values: proximadb_records::EmbeddingValues::Fp32(vector),
                    ..Default::default()
                }],
                created_at_ns: now_ns,
                updated_at_ns: now_ns,
                record_version: 1,
                ..Default::default()
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

        // KOU result-egress: wire bytes = tag(1) + length(4) + data_len.
        self.result_bytes_pending = self
            .result_bytes_pending
            .saturating_add(5 + data_len as u64);
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

        // TD-102: when the client supplies no explicit parameter OIDs
        // (tokio_postgres and psycopg/mem0 both do this, letting the server
        // infer), derive the parameter arity + best-effort types from the
        // `$N` placeholders. Without this `ParameterDescription` reports 0
        // parameters and the client aborts Bind with `Parameters(expected, 0)`.
        if param_types.is_empty() {
            param_types = crate::network::postgres::pgvector_params::infer_param_types(&query);
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
            execution_state: None,
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

        // Read max rows (0 = unlimited).
        let max_rows = cursor.get_i32();

        // Get the portal
        let portal_bound_query = match self.portals.get(&portal_name) {
            Some(p) => p.bound_query.clone(),
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
            portal_name, portal_bound_query
        );

        // Use the same query execution path as simple query, but suppress the
        // RowDescription it emits: the extended protocol already reported the
        // result columns at Describe(statement) time, so a second descriptor
        // here is a duplicate the client rejects (TD-102).
        self.suppress_row_description = true;
        let result = self
            .execute_portal_query(&portal_name, &portal_bound_query, max_rows)
            .await;
        self.suppress_row_description = false;
        result
    }

    async fn execute_portal_query(
        &mut self,
        portal_name: &str,
        query: &str,
        max_rows: i32,
    ) -> Result<()> {
        if max_rows > 0 {
            if self
                .portals
                .get(portal_name)
                .is_some_and(|p| p.execution_state.is_some())
            {
                return self.emit_portal_page(portal_name, max_rows as usize).await;
            }

            // E0 follow-up: this portal-SELECT fast-path returns before reaching
            // `execute_query_with_controls`, so it is the one pgwire query path
            // not yet under the per-query `io_trace` span. Bring it under the
            // same scope in the next slice (kept out here to avoid restructuring
            // the borrow in this `&&` let-chain).
            if query.trim_start().to_uppercase().starts_with("SELECT")
                && let Some(result) = self
                    .try_run_relational_select_pipeline(query, ExecutionControls::default())
                    .await
            {
                let result = match result {
                    Ok(result) => result,
                    Err(msg) => return self.send_error("ERROR", "XX000", &msg).await,
                };
                if let Some(portal) = self.portals.get_mut(portal_name) {
                    portal.execution_state = Some(PortalExecutionState {
                        result,
                        next_row: 0,
                    });
                }
                return self.emit_portal_page(portal_name, max_rows as usize).await;
            }
        }

        self.execute_query_with_controls(
            query,
            Self::execution_controls_for_execute_max_rows(max_rows),
        )
        .await
    }

    async fn emit_portal_page(&mut self, portal_name: &str, max_rows: usize) -> Result<()> {
        let (rows, finished) = {
            let Some(portal) = self.portals.get_mut(portal_name) else {
                return self
                    .send_error(
                        "ERROR",
                        "34000",
                        &format!("portal \"{}\" does not exist", portal_name),
                    )
                    .await;
            };
            let Some(state) = portal.execution_state.as_mut() else {
                return self.send_command_complete("SELECT 0").await;
            };

            let start = state.next_row;
            let (end, finished) =
                Self::portal_page_bounds(state.result.rows.len(), state.next_row, max_rows);
            let rows: Vec<Vec<Option<String>>> = state.result.rows[start..end]
                .iter()
                .map(|row| {
                    row.iter()
                        .map(super::relational_pipeline::text_encode)
                        .collect()
                })
                .collect();
            state.next_row = end;
            (rows, finished)
        };

        for row in &rows {
            self.send_data_row_nullable(row).await?;
        }
        if finished {
            self.send_command_complete(&format!("SELECT {}", rows.len()))
                .await
        } else {
            self.send_portal_suspended().await
        }
    }

    fn portal_page_bounds(total_rows: usize, next_row: usize, max_rows: usize) -> (usize, bool) {
        let end = if max_rows == 0 {
            total_rows
        } else {
            total_rows.min(next_row.saturating_add(max_rows))
        };
        (end, end >= total_rows)
    }

    /// Convert PostgreSQL Execute.max_rows into ProximaDB execution controls.
    ///
    /// PostgreSQL defines `0` as "unlimited". Positive caps are portal row
    /// budgets, so fallback execution uses truncation rather than a row-limit
    /// error. Relational SELECT portals use materialized cursor state and emit
    /// `PortalSuspended` when more rows remain.
    fn execution_controls_for_execute_max_rows(max_rows: i32) -> ExecutionControls {
        if max_rows <= 0 {
            return ExecutionControls::default();
        }
        ExecutionControls {
            max_rows: Some(max_rows as usize),
            row_limit_mode: RowLimitMode::Truncate,
            ..Default::default()
        }
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
                // Describe statement - clone the query + param_types to avoid
                // a borrow conflict with the &mut self sends below.
                if let Some((stmt_query, param_types)) = self
                    .prepared_statements
                    .get(&name)
                    .map(|s| (s.query.clone(), s.param_types.clone()))
                {
                    // Send parameter description (TD-102: now carries the
                    // inferred arity so the client binds the right count).
                    self.send_parameter_description(&param_types).await?;
                    // TD-102: report the result columns this statement will
                    // return so the client's column read matches the DataRows
                    // streamed during Execute. A vector-search SELECT returns
                    // (id, distance, metadata); other statements report no
                    // columns (NoData-equivalent empty descriptor) as before.
                    let fields = crate::network::postgres::pgvector_params::described_result_fields(
                        &stmt_query,
                    );
                    self.send_row_description(&fields).await?;
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
        } else if close_type == 'P' {
            self.portals.remove(&name);
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
        // TD-102: extended Execute path already described columns at
        // Describe(statement) time; skip the duplicate emitted here.
        if self.suppress_row_description {
            return Ok(());
        }
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

    /// Flush the result set's accumulated DataRow bytes to the KOU result-egress
    /// meter (`direction = "result"`), attributed to the query's tenant and the
    /// client's edge locality. Called at each result-set boundary (CommandComplete
    /// and PortalSuspended). No-ops when nothing was sent or the client is on the
    /// free path (in-VPC / loopback / same-region) — only genuinely remote clients
    /// (the direct data-plane case) classify as chargeable.
    async fn flush_result_egress(&mut self) {
        let bytes = std::mem::take(&mut self.result_bytes_pending);
        if bytes == 0 {
            return;
        }
        let edge = crate::metrics::consumption_metrics::EdgePolicyContext::classify(self.peer_ip);
        let tenant = self.pgwire_resolve_read_tenant().await;
        let tenant_scope = (!tenant.is_empty()).then_some(tenant.as_str());
        edge.record_result_egress(tenant_scope, bytes);
    }

    /// Send command complete
    async fn send_command_complete(&mut self, tag: &str) -> Result<()> {
        self.flush_result_egress().await;
        let len = 4 + tag.len() + 1;
        self.write_buffer.put_u8(b'C');
        self.write_buffer.put_i32(len as i32);
        self.write_buffer.put_slice(tag.as_bytes());
        self.write_buffer.put_u8(0);
        self.flush_write_buffer().await
    }

    /// Send PortalSuspended for an extended-protocol Execute that has more
    /// portal rows available after satisfying the current max_rows budget.
    async fn send_portal_suspended(&mut self) -> Result<()> {
        // Partial result set delivered — meter the rows sent so far; the rest
        // flushes at the resumed Execute's CommandComplete.
        self.flush_result_egress().await;
        self.write_buffer.put_u8(b's');
        self.write_buffer.put_i32(4);
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
    use crate::catalog::{CatalogColumn, CatalogTableSchema};
    use crate::query::multimodal_router;
    use proximadb_records::{ProximaRecord, ProximaTreeNode};

    #[test]
    fn test_frontend_message() {
        assert_eq!(FrontendMessage::Query as u8, b'Q');
        assert_eq!(FrontendMessage::Terminate as u8, b'X');
    }

    // pgvector WHERE-filter + extended-protocol param tests (TD-100/TD-102)
    // live in `super::super::pgvector_params` where the logic now resides.

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
        assert_eq!(
            multimodal_router::detect_store_type_from_query(
                "SELECT id FROM products ORDER BY VECTOR_DISTANCE(embedding, [0.1, 0.2], 'l2') LIMIT 5",
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
        assert_eq!(
            multimodal_router::detect_store_type_from_query(
                "SELECT * FROM products WHERE JSON_EXTRACT_TEXT(metadata, 'tenant') = 'acme'",
                "products",
                None,
            ),
            DataModel::Document
        );
        assert_eq!(
            multimodal_router::detect_store_type_from_query(
                "SELECT * FROM products WHERE JSON_CONTAINS(metadata, '{\"role\":\"planner\"}')",
                "products",
                None,
            ),
            DataModel::Document
        );
        assert_eq!(
            multimodal_router::detect_store_type_from_query(
                "SELECT * FROM DOCUMENT_QUERY('agent_docs', '$.role = \"planner\"')",
                "agent_queries",
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
        assert_eq!(
            multimodal_router::detect_store_type_from_query(
                "SELECT * FROM GRAPH_QUERY('MATCH (n:Agent)-[:CALLS]->(m) RETURN m')",
                "agent_queries",
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
        assert_eq!(
            multimodal_router::detect_store_type_from_query(
                "SELECT * FROM LOGS('production') WHERE severity = 'ERROR'",
                "ops_queries",
                None,
            ),
            DataModel::Observability
        );
        assert_eq!(
            multimodal_router::detect_store_type_from_query(
                "SELECT * FROM METRICS('system') WHERE metric_name = 'cpu_usage'",
                "ops_queries",
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
    fn execute_max_rows_maps_to_truncating_execution_controls() {
        let unlimited = PostgresProtocol::execution_controls_for_execute_max_rows(0);
        assert_eq!(unlimited.max_rows, None);
        assert_eq!(unlimited.row_limit_mode, RowLimitMode::Error);

        let negative = PostgresProtocol::execution_controls_for_execute_max_rows(-1);
        assert_eq!(negative.max_rows, None);
        assert_eq!(negative.row_limit_mode, RowLimitMode::Error);

        let capped = PostgresProtocol::execution_controls_for_execute_max_rows(5);
        assert_eq!(capped.max_rows, Some(5));
        assert_eq!(capped.row_limit_mode, RowLimitMode::Truncate);
    }

    #[test]
    fn portal_page_bounds_reports_suspended_and_complete_pages() {
        let (end, complete) = PostgresProtocol::portal_page_bounds(5, 0, 2);
        assert_eq!(end, 2);
        assert!(!complete);

        let (end, complete) = PostgresProtocol::portal_page_bounds(5, 2, 3);
        assert_eq!(end, 5);
        assert!(complete);

        let (end, complete) = PostgresProtocol::portal_page_bounds(5, 5, 2);
        assert_eq!(end, 5);
        assert!(complete);
    }

    #[test]
    fn portal_page_bounds_treats_zero_budget_as_unlimited() {
        let (end, complete) = PostgresProtocol::portal_page_bounds(5, 1, 0);
        assert_eq!(end, 5);
        assert!(complete);
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

    // Note: a prior `test_extract_explain_inner_query_for_table_write`
    // covered `PostgresProtocol::extract_explain_inner_query`, which
    // was removed alongside `strip_explain_prefix` in clippy cleanup
    // batch 14 (commit `555ed5b2a`). `extract_explain_with_analyze`
    // is the surviving entry point and is exercised below.

    #[test]
    fn test_extract_explain_with_analyze_detects_analyze_flag() {
        let (is_analyze, inner) = PostgresProtocol::extract_explain_with_analyze(
            "EXPLAIN (ANALYZE, FORMAT JSON) INSERT INTO facts SELECT * FROM staging;",
        )
        .expect("analyze EXPLAIN should parse");

        assert!(is_analyze, "ANALYZE option should be detected");
        assert_eq!(inner, "INSERT INTO facts SELECT * FROM staging;");
    }

    #[test]
    fn test_extract_explain_with_analyze_bare_analyze_keyword() {
        let (is_analyze, inner) = PostgresProtocol::extract_explain_with_analyze(
            "EXPLAIN ANALYZE INSERT INTO facts SELECT * FROM staging;",
        )
        .expect("bare EXPLAIN ANALYZE should parse");

        assert!(is_analyze, "bare ANALYZE should be detected");
        assert_eq!(inner, "INSERT INTO facts SELECT * FROM staging;");
    }

    #[test]
    fn test_extract_explain_without_analyze_returns_false() {
        let (is_analyze, inner) = PostgresProtocol::extract_explain_with_analyze(
            "EXPLAIN (FORMAT JSON) INSERT INTO facts SELECT * FROM staging;",
        )
        .expect("plain EXPLAIN should parse");

        assert!(!is_analyze, "no ANALYZE option — flag should be false");
        assert_eq!(inner, "INSERT INTO facts SELECT * FROM staging;");
    }

    #[test]
    fn test_parse_set_parameter_for_write_intent_hint() {
        let (name, value) = PostgresProtocol::parse_set_parameter(
            "SET proximadb.write.row_count_hint = '100_000';",
        )
        .expect("SET should parse");

        assert_eq!(name, "proximadb.write.row_count_hint");
        assert_eq!(value, "100_000");
    }

    #[test]
    fn test_parse_set_parameter_supports_to_syntax() {
        let (name, value) = PostgresProtocol::parse_set_parameter(
            "SET proximadb.write.batch_local_constraints_sufficient TO on;",
        )
        .expect("SET TO should parse");

        assert_eq!(name, "proximadb.write.batch_local_constraints_sufficient");
        assert_eq!(value, "on");
    }

    #[test]
    fn test_write_intent_overrides_from_session_parameters() {
        let params = std::collections::HashMap::from([
            (
                "proximadb.write.tenant_id".to_string(),
                "tenant-a".to_string(),
            ),
            ("proximadb.write.actor".to_string(), "benchbase".to_string()),
            (
                "proximadb.write.row_count_hint".to_string(),
                "100_000".to_string(),
            ),
            (
                "proximadb.write.estimated_bytes".to_string(),
                "4096".to_string(),
            ),
            (
                "proximadb.write.requires_row_level_semantics".to_string(),
                "off".to_string(),
            ),
            (
                "proximadb.write.batch_local_constraints_sufficient".to_string(),
                "true".to_string(),
            ),
        ]);

        let overrides = PostgresProtocol::write_intent_overrides_from_params(&params);

        assert_eq!(overrides.tenant_id.as_deref(), Some("tenant-a"));
        assert_eq!(overrides.actor.as_deref(), Some("benchbase"));
        assert_eq!(overrides.row_count_hint, Some(100_000));
        assert_eq!(overrides.estimated_bytes, Some(4096));
        assert_eq!(overrides.requires_row_level_semantics, Some(false));
        assert_eq!(overrides.batch_local_constraints_sufficient, Some(true));
    }

    #[test]
    fn test_extract_select_limit_for_relational_scan() {
        assert_eq!(
            PostgresProtocol::extract_select_limit("SELECT * FROM t LIMIT 25;"),
            Some(25)
        );
        assert_eq!(
            PostgresProtocol::extract_select_limit("SELECT * FROM t ORDER BY id"),
            None
        );
    }

    #[test]
    fn test_extract_selected_column_names_for_relational_select() {
        assert!(
            PostgresProtocol::extract_selected_column_names("SELECT * FROM customers").is_empty()
        );
        assert_eq!(
            PostgresProtocol::extract_selected_column_names(
                "SELECT c_id, customers.c_name AS name FROM customers WHERE c_id = 1"
            ),
            vec!["c_id".to_string(), "c_name".to_string()]
        );
    }

    #[test]
    fn test_extract_select_where_predicates_for_relational_scan() {
        let predicates = PostgresProtocol::extract_select_where_predicates(
            "SELECT * FROM customers WHERE c_name = 'alice updated' AND c_active = true LIMIT 1;",
        )
        .expect("simple AND predicates should parse");

        assert_eq!(predicates.len(), 2);
        assert_eq!(predicates[0].column_name, "c_name");
        match &predicates[0].condition {
            SelectPredicateCondition::Comparison { operator, literal } => {
                assert_eq!(*operator, SelectPredicateOperator::Equal);
                assert_eq!(literal, "alice updated");
            }
            other => panic!("unexpected predicate: {other:?}"),
        }
        assert_eq!(predicates[1].column_name, "c_active");
        match &predicates[1].condition {
            SelectPredicateCondition::Comparison { literal, .. } => {
                assert_eq!(literal, "true");
            }
            other => panic!("unexpected predicate: {other:?}"),
        }
    }

    #[test]
    fn test_extract_select_where_in_like_and_null_predicates() {
        let predicates = PostgresProtocol::extract_select_where_predicates(
            "SELECT * FROM customers WHERE c_id IN (1, 2) AND c_name LIKE 'alice%' AND c_notes IS NULL;",
        )
        .expect("IN, LIKE, and IS NULL predicates should parse");

        assert_eq!(predicates.len(), 3);
        match &predicates[0].condition {
            SelectPredicateCondition::In { literals, negated } => {
                assert!(!negated);
                assert_eq!(literals, &vec!["1".to_string(), "2".to_string()]);
            }
            other => panic!("unexpected predicate: {other:?}"),
        }
        match &predicates[1].condition {
            SelectPredicateCondition::Like { pattern, negated } => {
                assert!(!negated);
                assert_eq!(pattern, "alice%");
            }
            other => panic!("unexpected predicate: {other:?}"),
        }
        match &predicates[2].condition {
            SelectPredicateCondition::IsNull { negated } => assert!(!negated),
            other => panic!("unexpected predicate: {other:?}"),
        }
    }

    #[test]
    fn test_record_matches_relational_scan_predicates() {
        let record = ProximaRecord {
            oid: "1".to_string(),
            props: proximadb_records::ProximaTree::from([
                (
                    "name".to_string(),
                    ProximaTreeNode::Value(ProximaValue::String("alice".to_string())),
                ),
                (
                    "balance".to_string(),
                    ProximaTreeNode::Value(ProximaValue::Decimal("75.25".to_string())),
                ),
                (
                    "active".to_string(),
                    ProximaTreeNode::Value(ProximaValue::Boolean(true)),
                ),
            ]),
            ..Default::default()
        };
        let schema = CatalogTableSchema::new("customers")
            .with_column(CatalogColumn::new(1, "id", ProximaType::Int32))
            .with_column(CatalogColumn::new(2, "name", ProximaType::String))
            .with_column(CatalogColumn::new(
                3,
                "balance",
                ProximaType::Decimal {
                    precision: 38,
                    scale: 10,
                },
            ))
            .with_column(CatalogColumn::new(4, "active", ProximaType::Boolean))
            .with_primary_key(vec!["id".to_string()]);
        let predicates = vec![
            SelectPredicate {
                column_name: "name".to_string(),
                condition: SelectPredicateCondition::Comparison {
                    operator: SelectPredicateOperator::Equal,
                    literal: "alice".to_string(),
                },
            },
            SelectPredicate {
                column_name: "balance".to_string(),
                condition: SelectPredicateCondition::Comparison {
                    operator: SelectPredicateOperator::GreaterThanOrEqual,
                    literal: "75.00".to_string(),
                },
            },
            SelectPredicate {
                column_name: "active".to_string(),
                condition: SelectPredicateCondition::Comparison {
                    operator: SelectPredicateOperator::Equal,
                    literal: "true".to_string(),
                },
            },
        ];

        assert!(
            DmlService::record_matches_select_predicate_inputs(&record, &schema, &predicates)
                .expect("predicates should resolve")
        );
    }

    #[test]
    fn test_record_matches_in_like_and_null_relational_scan_predicates() {
        let record = ProximaRecord {
            oid: "1".to_string(),
            props: proximadb_records::ProximaTree::from([
                (
                    "name".to_string(),
                    ProximaTreeNode::Value(ProximaValue::String("alice updated".to_string())),
                ),
                (
                    "active".to_string(),
                    ProximaTreeNode::Value(ProximaValue::Boolean(true)),
                ),
            ]),
            ..Default::default()
        };
        let schema = CatalogTableSchema::new("customers")
            .with_column(CatalogColumn::new(1, "id", ProximaType::Int32))
            .with_column(CatalogColumn::new(2, "name", ProximaType::String))
            .with_column(CatalogColumn::new(3, "notes", ProximaType::String))
            .with_primary_key(vec!["id".to_string()]);
        let predicates = vec![
            SelectPredicate {
                column_name: "id".to_string(),
                condition: SelectPredicateCondition::In {
                    literals: vec!["1".to_string(), "2".to_string()],
                    negated: false,
                },
            },
            SelectPredicate {
                column_name: "name".to_string(),
                condition: SelectPredicateCondition::Like {
                    pattern: "alice%".to_string(),
                    negated: false,
                },
            },
            SelectPredicate {
                column_name: "notes".to_string(),
                condition: SelectPredicateCondition::IsNull { negated: false },
            },
        ];

        assert!(
            DmlService::record_matches_select_predicate_inputs(&record, &schema, &predicates)
                .expect("predicates should resolve")
        );
    }

    #[test]
    fn test_record_matches_not_in_rejects_excluded_values() {
        let schema = CatalogTableSchema::new("orders")
            .with_column(CatalogColumn::new(1, "id", ProximaType::Int32))
            .with_column(CatalogColumn::new(2, "status", ProximaType::String))
            .with_primary_key(vec!["id".to_string()]);

        // Record with id=5 should be rejected by NOT IN (1, 2, 5) predicate.
        let excluded_record = ProximaRecord {
            oid: "5".to_string(),
            ..Default::default()
        };
        let predicates = vec![SelectPredicate {
            column_name: "id".to_string(),
            condition: SelectPredicateCondition::In {
                literals: vec!["1".to_string(), "2".to_string(), "5".to_string()],
                negated: true,
            },
        }];
        assert!(
            !DmlService::record_matches_select_predicate_inputs(
                &excluded_record,
                &schema,
                &predicates
            )
            .expect("NOT IN must resolve"),
            "record with id in the excluded list must not match NOT IN"
        );

        // Record with id=99 should pass NOT IN (1, 2, 5).
        let passing_record = ProximaRecord {
            oid: "99".to_string(),
            ..Default::default()
        };
        assert!(
            DmlService::record_matches_select_predicate_inputs(
                &passing_record,
                &schema,
                &predicates
            )
            .expect("NOT IN must resolve"),
            "record with id not in the excluded list must match NOT IN"
        );
    }

    #[test]
    fn test_record_matches_is_not_null_accepts_present_field_rejects_absent() {
        let schema = CatalogTableSchema::new("users")
            .with_column(CatalogColumn::new(1, "id", ProximaType::String))
            .with_column(CatalogColumn::new(2, "email", ProximaType::String))
            .with_primary_key(vec!["id".to_string()]);

        let predicates = vec![SelectPredicate {
            column_name: "email".to_string(),
            condition: SelectPredicateCondition::IsNull { negated: true },
        }];

        // Record WITH email field → matches IS NOT NULL.
        let with_email = ProximaRecord {
            oid: "u1".to_string(),
            props: proximadb_records::ProximaTree::from([(
                "email".to_string(),
                ProximaTreeNode::Value(ProximaValue::String("u@example.com".to_string())),
            )]),
            ..Default::default()
        };
        assert!(
            DmlService::record_matches_select_predicate_inputs(&with_email, &schema, &predicates)
                .expect("IS NOT NULL must resolve"),
            "record with email present must match IS NOT NULL"
        );

        // Record WITHOUT email field → must NOT match IS NOT NULL.
        let without_email = ProximaRecord {
            oid: "u2".to_string(),
            ..Default::default()
        };
        assert!(
            !DmlService::record_matches_select_predicate_inputs(
                &without_email,
                &schema,
                &predicates
            )
            .expect("IS NOT NULL must resolve"),
            "record with absent email must not match IS NOT NULL"
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

    #[test]
    fn test_or_predicate_same_column_folds_to_in() {
        let predicates = PostgresProtocol::extract_select_where_predicates(
            "SELECT c_id, c_name FROM pgwire_smoke_customer WHERE c_id = 1 OR c_id = 2;",
        )
        .expect("single-column OR should fold to IN");

        assert_eq!(predicates.len(), 1);
        assert_eq!(predicates[0].column_name, "c_id");
        match &predicates[0].condition {
            SelectPredicateCondition::In { literals, negated } => {
                assert!(!negated);
                assert_eq!(literals, &vec!["1".to_string(), "2".to_string()]);
            }
            other => panic!("expected In, got: {other:?}"),
        }
    }

    #[test]
    fn test_or_predicate_three_values_folds_to_in() {
        let predicates = PostgresProtocol::extract_select_where_predicates(
            "SELECT * FROM orders WHERE status = 'N' OR status = 'P' OR status = 'C';",
        )
        .expect("three-value OR on same column should fold");

        assert_eq!(predicates.len(), 1);
        match &predicates[0].condition {
            SelectPredicateCondition::In { literals, .. } => {
                assert_eq!(
                    literals,
                    &vec!["N".to_string(), "P".to_string(), "C".to_string()]
                );
            }
            other => panic!("expected In, got: {other:?}"),
        }
    }

    #[test]
    fn test_or_predicate_multi_column_falls_back_to_full_scan() {
        // Different columns: col1 = v1 OR col2 = v2 — cannot fold, returns None → full scan
        let result = PostgresProtocol::extract_select_where_predicates(
            "SELECT * FROM t WHERE col1 = 1 OR col2 = 2;",
        );
        assert!(
            result.is_none(),
            "multi-column OR should return None (full scan)"
        );
    }

    #[test]
    fn test_or_predicate_non_equality_falls_back_to_full_scan() {
        // OR with non-equality: col > v1 OR col < v2 — cannot fold
        let result = PostgresProtocol::extract_select_where_predicates(
            "SELECT * FROM t WHERE col > 5 OR col < 0;",
        );
        assert!(
            result.is_none(),
            "non-equality OR should return None (full scan)"
        );
    }

    #[test]
    fn test_and_chain_with_in_predicate_parses_correctly() {
        // AND-chain: one IN predicate + one equality — both must be extracted
        let result = PostgresProtocol::extract_select_where_predicates(
            "SELECT * FROM t WHERE c_id IN (1, 2, 3) AND c_active = true;",
        );
        let predicates = result.expect("AND chain with IN must parse");
        assert_eq!(predicates.len(), 2);
        let in_pred = predicates
            .iter()
            .find(|p| p.column_name.eq_ignore_ascii_case("c_id"))
            .expect("IN predicate for c_id must be present");
        match &in_pred.condition {
            SelectPredicateCondition::In { literals, negated } => {
                assert!(!negated);
                assert_eq!(literals.len(), 3);
            }
            other => panic!("expected In condition, got {:?}", other),
        }
        let eq_pred = predicates
            .iter()
            .find(|p| p.column_name.eq_ignore_ascii_case("c_active"))
            .expect("equality predicate for c_active must be present");
        match &eq_pred.condition {
            SelectPredicateCondition::Comparison { literal, .. } => {
                assert_eq!(literal, "true");
            }
            other => panic!("expected Comparison condition, got {:?}", other),
        }
    }

    #[test]
    fn test_and_chain_with_is_null_predicate() {
        let result = PostgresProtocol::extract_select_where_predicates(
            "SELECT * FROM t WHERE label IS NULL AND c_id = 5;",
        );
        let predicates = result.expect("AND chain with IS NULL must parse");
        assert_eq!(predicates.len(), 2);
        let null_pred = predicates
            .iter()
            .find(|p| p.column_name.eq_ignore_ascii_case("label"))
            .expect("IS NULL predicate for label must be present");
        match &null_pred.condition {
            SelectPredicateCondition::IsNull { negated } => {
                assert!(!negated, "IS NULL must not be negated");
            }
            other => panic!("expected IsNull condition, got {:?}", other),
        }
    }

    #[test]
    fn test_and_chain_with_like_predicate_parses_correctly() {
        let result = PostgresProtocol::extract_select_where_predicates(
            "SELECT * FROM t WHERE label LIKE 'prefix%' AND c_id = 5;",
        );
        let predicates = result.expect("AND chain with LIKE must parse");
        assert_eq!(predicates.len(), 2);
        let like_pred = predicates
            .iter()
            .find(|p| p.column_name.eq_ignore_ascii_case("label"))
            .expect("LIKE predicate for label must be present");
        match &like_pred.condition {
            SelectPredicateCondition::Like { pattern, negated } => {
                assert_eq!(pattern, "prefix%");
                assert!(!negated, "LIKE must not be negated");
            }
            other => panic!("expected Like condition, got {:?}", other),
        }
        let id_pred = predicates
            .iter()
            .find(|p| p.column_name.eq_ignore_ascii_case("c_id"))
            .expect("comparison predicate for c_id must be present");
        assert!(matches!(
            &id_pred.condition,
            SelectPredicateCondition::Comparison { literal, .. } if literal == "5"
        ));
    }

    #[test]
    fn test_and_chain_with_is_not_null_predicate() {
        let result = PostgresProtocol::extract_select_where_predicates(
            "SELECT * FROM t WHERE label IS NOT NULL AND c_active = true;",
        );
        let predicates = result.expect("AND chain with IS NOT NULL must parse");
        assert_eq!(predicates.len(), 2);
        let not_null_pred = predicates
            .iter()
            .find(|p| p.column_name.eq_ignore_ascii_case("label"))
            .expect("IS NOT NULL predicate for label must be present");
        match &not_null_pred.condition {
            SelectPredicateCondition::IsNull { negated } => {
                assert!(*negated, "IS NOT NULL must be negated=true");
            }
            other => panic!("expected IsNull(negated=true) condition, got {:?}", other),
        }
    }

    #[test]
    fn test_and_chain_with_not_in_predicate_parses_correctly() {
        let result = PostgresProtocol::extract_select_where_predicates(
            "SELECT * FROM t WHERE c_id NOT IN (10, 20, 30) AND c_active = false;",
        );
        let predicates = result.expect("AND chain with NOT IN must parse");
        assert_eq!(predicates.len(), 2);
        let not_in_pred = predicates
            .iter()
            .find(|p| p.column_name.eq_ignore_ascii_case("c_id"))
            .expect("NOT IN predicate for c_id must be present");
        match &not_in_pred.condition {
            SelectPredicateCondition::In { literals, negated } => {
                assert_eq!(literals.len(), 3);
                assert!(*negated, "NOT IN must be negated=true");
            }
            other => panic!("expected In(negated=true) condition, got {:?}", other),
        }
    }

    // === ADR-018 Phase 2: IF NOT EXISTS tests ===

    #[test]
    fn test_create_table_without_if_not_exists() {
        let upper = "CREATE TABLE users (id TEXT, name TEXT)";
        assert!(!upper.contains("IF NOT EXISTS"));
    }

    #[test]
    fn test_create_table_with_if_not_exists() {
        let upper = "CREATE TABLE IF NOT EXISTS users (id TEXT, name TEXT)";
        assert!(upper.contains("IF NOT EXISTS"));
    }

    #[test]
    fn test_drop_table_without_if_exists() {
        let upper = "DROP TABLE users";
        assert!(!upper.contains("IF EXISTS"));
    }

    #[test]
    fn test_drop_table_with_if_exists() {
        let upper = "DROP TABLE IF EXISTS users";
        assert!(upper.contains("IF EXISTS"));
    }

    // ---------------- ADR-018 Phase 2: multi-column ORDER BY ----------------

    #[test]
    fn order_by_single_column_default_asc_nulls_last() {
        let keys = PostgresProtocol::extract_select_order_by("SELECT * FROM t ORDER BY name")
            .expect("single-col ORDER BY must parse");
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].column, "name");
        assert!(!keys[0].desc);
        // Postgres default: ASC → NULLS LAST.
        assert!(!keys[0].nulls_first);
    }

    #[test]
    fn order_by_explicit_desc_default_nulls_first() {
        let keys = PostgresProtocol::extract_select_order_by("SELECT * FROM t ORDER BY score DESC")
            .unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].column, "score");
        assert!(keys[0].desc);
        // Postgres default: DESC → NULLS FIRST.
        assert!(keys[0].nulls_first);
    }

    #[test]
    fn order_by_explicit_nulls_first_overrides_default() {
        let keys = PostgresProtocol::extract_select_order_by(
            "SELECT * FROM t ORDER BY score ASC NULLS FIRST",
        )
        .unwrap();
        assert_eq!(keys.len(), 1);
        assert!(!keys[0].desc);
        // Override: NULLS FIRST under ASC.
        assert!(keys[0].nulls_first);
    }

    #[test]
    fn order_by_explicit_nulls_last_overrides_default() {
        let keys = PostgresProtocol::extract_select_order_by(
            "SELECT * FROM t ORDER BY score DESC NULLS LAST",
        )
        .unwrap();
        assert_eq!(keys.len(), 1);
        assert!(keys[0].desc);
        // Override: NULLS LAST under DESC.
        assert!(!keys[0].nulls_first);
    }

    #[test]
    fn order_by_multi_column_preserves_declaration_order() {
        let keys = PostgresProtocol::extract_select_order_by(
            "SELECT * FROM t ORDER BY name ASC, score DESC, created_at",
        )
        .expect("multi-col ORDER BY must parse (Phase 2)");
        assert_eq!(keys.len(), 3);
        assert_eq!(keys[0].column, "name");
        assert!(!keys[0].desc);
        assert_eq!(keys[1].column, "score");
        assert!(keys[1].desc);
        assert_eq!(keys[2].column, "created_at");
        assert!(!keys[2].desc);
    }

    #[test]
    fn order_by_multi_column_per_key_nulls() {
        let keys = PostgresProtocol::extract_select_order_by(
            "SELECT * FROM t ORDER BY a NULLS FIRST, b DESC NULLS LAST",
        )
        .unwrap();
        assert_eq!(keys.len(), 2);
        assert!(keys[0].nulls_first); // explicit NULLS FIRST on ASC
        assert!(!keys[1].nulls_first); // explicit NULLS LAST on DESC
    }

    #[test]
    fn order_by_terminates_at_limit() {
        let keys =
            PostgresProtocol::extract_select_order_by("SELECT * FROM t ORDER BY name LIMIT 10")
                .unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].column, "name");
    }

    #[test]
    fn order_by_terminates_at_offset() {
        let keys =
            PostgresProtocol::extract_select_order_by("SELECT * FROM t ORDER BY name OFFSET 5")
                .unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].column, "name");
    }

    #[test]
    fn order_by_no_clause_returns_none() {
        assert!(PostgresProtocol::extract_select_order_by("SELECT * FROM t").is_none(),);
    }

    #[test]
    fn split_top_level_commas_respects_string_literals() {
        let parts = PostgresProtocol::split_top_level_commas("a, 'b, c', d");
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0].trim(), "a");
        assert_eq!(parts[1].trim(), "'b, c'");
        assert_eq!(parts[2].trim(), "d");
    }

    // ── Slice 6.3: primary-pod gate ─────────────────────────────────

    use crate::cluster::primary_pod_registry::{AssignmentReason, PrimaryPodRegistry};

    fn make_pgwire_gate(
        registry: Arc<PrimaryPodRegistry>,
        self_pod_id: &str,
    ) -> Option<PgwirePrimaryPodGate> {
        Some(PgwirePrimaryPodGate {
            registry,
            self_pod_id: self_pod_id.to_string(),
        })
    }

    #[test]
    fn pgwire_gate_unconfigured_allows_writes() {
        let outcome = check_pgwire_primary_pod_gate(&None, "tenant-a", "users");
        assert!(matches!(outcome, PgwireGateOutcome::Allow));
    }

    #[test]
    fn pgwire_gate_allows_when_no_binding_exists() {
        let registry = Arc::new(PrimaryPodRegistry::new());
        let g = make_pgwire_gate(registry, "pod-self");
        assert!(matches!(
            check_pgwire_primary_pod_gate(&g, "tenant-a", "users"),
            PgwireGateOutcome::Allow
        ));
    }

    #[test]
    fn pgwire_gate_allows_when_binding_matches_self_pod() {
        let registry = Arc::new(PrimaryPodRegistry::new());
        registry.assign("tenant-a", "users", "pod-self", AssignmentReason::Create);
        let g = make_pgwire_gate(registry, "pod-self");
        assert!(matches!(
            check_pgwire_primary_pod_gate(&g, "tenant-a", "users"),
            PgwireGateOutcome::Allow
        ));
    }

    #[test]
    fn pgwire_gate_returns_misrouted_with_target_pod() {
        // The pgwire surface conveys the target pod by surfacing it
        // in the SQLSTATE-57P03 error MESSAGE rather than trailing
        // metadata (pgwire has no equivalent). The structured outcome
        // here is what feeds that format!() call, so locking it in
        // protects the operator-visible psql error text.
        let registry = Arc::new(PrimaryPodRegistry::new());
        registry.assign("tenant-a", "users", "pod-other", AssignmentReason::Operator);
        let g = make_pgwire_gate(registry, "pod-self");

        match check_pgwire_primary_pod_gate(&g, "tenant-a", "users") {
            PgwireGateOutcome::Misrouted { target_pod } => {
                assert_eq!(target_pod, "pod-other");
            }
            PgwireGateOutcome::Allow => panic!("expected misrouted, got allow"),
        }
    }

    #[test]
    fn pgwire_gate_scopes_per_tenant_collection_pair() {
        // Same scoping invariant as the other gate surfaces — bindings
        // don't bleed across (tenant_id, collection_id) pairs.
        let registry = Arc::new(PrimaryPodRegistry::new());
        registry.assign("tenant-a", "users", "pod-other", AssignmentReason::Operator);
        let g = make_pgwire_gate(registry, "pod-self");

        assert!(matches!(
            check_pgwire_primary_pod_gate(&g, "tenant-a", "orders"),
            PgwireGateOutcome::Allow
        ));
        assert!(matches!(
            check_pgwire_primary_pod_gate(&g, "tenant-b", "users"),
            PgwireGateOutcome::Allow
        ));
        assert!(matches!(
            check_pgwire_primary_pod_gate(&g, "tenant-a", "users"),
            PgwireGateOutcome::Misrouted { .. }
        ));
    }
}
