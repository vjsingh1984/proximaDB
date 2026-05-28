// PostgreSQL Wire Protocol Implementation
//
// Provides PostgreSQL-compatible server for:
// - pgvector migration path
// - Standard PostgreSQL clients (psql, pgAdmin, etc.)
// - Existing application compatibility
//
// Protocol: PostgreSQL Protocol v3.0

/// PostgreSQL Protocol v3.0 message parsing and encoding
pub mod protocol;
/// Bridge to the new relational pipeline (algebra → planner →
/// executor → engine). Opt-in via PROXIMADB_NEW_RELATIONAL_PIPELINE.
pub mod relational_pipeline;
/// Session management for PostgreSQL client connections
pub mod session;
/// SQL-to-ProximaDB query translator (pgvector compatibility)
pub mod translator;
/// PostgreSQL type system mapping and conversions
pub mod types;

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, error, info, warn};

use self::protocol::PostgresProtocol;
use self::session::SessionManager;
use crate::catalog::CatalogManager;
use crate::graph::GraphService;
use crate::observability::ObservabilityService;
use crate::services::TableWalAppender;
use crate::services::VectorOperationsService;
use crate::storage::document::DocumentService;
use proximadb_records::RecordStorage;

/// Optional canonical write dependencies for PostgreSQL wire DML.
///
/// When present, catalog-routed relational tables can write through
/// `DmlService::with_direct_record_storage`; legacy vector-specialized tables
/// continue to use the compatibility route selected by xCatalog.
#[derive(Clone)]
pub struct DirectPgwireWriteServices {
    record_storage: Arc<dyn RecordStorage>,
    wal_appender: Arc<dyn TableWalAppender>,
}

impl DirectPgwireWriteServices {
    /// Build direct pgwire write dependencies.
    pub fn new(
        record_storage: Arc<dyn RecordStorage>,
        wal_appender: Arc<dyn TableWalAppender>,
    ) -> Self {
        Self {
            record_storage,
            wal_appender,
        }
    }
}

/// PostgreSQL-compatible server
pub struct PostgresServer {
    /// Listen address
    bind_address: SocketAddr,
    /// Session manager
    session_manager: Arc<SessionManager>,
    /// Collection port reference (Phase 9 / Task #76)
    collection_port: Arc<dyn proximadb_runtime::CollectionPort>,
    /// Vector operations service for search
    vector_ops: Arc<VectorOperationsService>,
    /// Shared xCatalog manager for SQL DDL/DML and metadata introspection
    catalog_manager: Arc<CatalogManager>,
    /// Document service for JSON document collections
    document_service: Option<Arc<DocumentService>>,
    /// Graph service for graph collections
    graph_service: Option<Arc<GraphService>>,
    /// Observability service for logs/metrics/traces
    observability_service: Option<Arc<ObservabilityService>>,
    /// Optional canonical record/WAL writer for relational pgwire DML.
    direct_write_services: Option<DirectPgwireWriteServices>,
    /// Optional rank-pipeline singleton + durable catalog (R-7c.3
    /// production wiring). When `Some`, every per-connection DDL service
    /// is built with `with_rank_profile_store` + `with_rank_services` so
    /// SQL `CREATE RANK PROFILE` / `DROP RANK PROFILE` reach the same
    /// `RankServices` instance the REST / gRPC / Arrow Flight paths share.
    rank_pipeline: Option<PgwireRankPipeline>,
    /// Whether the server is running
    running: Arc<std::sync::atomic::AtomicBool>,
}

/// Bundle of the rank-pipeline handles pgwire threads into every
/// `DdlService` it constructs. Cloning is cheap (two `Arc`s).
#[derive(Clone)]
pub struct PgwireRankPipeline {
    pub services: Arc<crate::network::rest::v1::rank::RankServices>,
    pub store: Arc<dyn crate::services::RankProfileStore>,
}

impl PostgresServer {
    /// Create a new PostgreSQL server
    pub fn new(
        bind_address: SocketAddr,
        collection_port: Arc<dyn proximadb_runtime::CollectionPort>,
        vector_ops: Arc<VectorOperationsService>,
        catalog_manager: Arc<CatalogManager>,
        document_service: Option<Arc<DocumentService>>,
        graph_service: Option<Arc<GraphService>>,
        observability_service: Option<Arc<ObservabilityService>>,
    ) -> Self {
        Self {
            bind_address,
            session_manager: Arc::new(SessionManager::new()),
            collection_port,
            vector_ops,
            catalog_manager,
            document_service,
            graph_service,
            observability_service,
            direct_write_services: None,
            rank_pipeline: None,
            running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Attach the process-wide rank-pipeline so each per-connection
    /// `DdlService` is built with the rank-profile catalog + live
    /// registry wired in. Production callers pass
    /// `SharedServices.rank_services` + `SharedServices.rank_profile_store`.
    pub fn with_rank_pipeline(
        mut self,
        services: Arc<crate::network::rest::v1::rank::RankServices>,
        store: Arc<dyn crate::services::RankProfileStore>,
    ) -> Self {
        self.rank_pipeline = Some(PgwireRankPipeline { services, store });
        self
    }

    /// Enable direct canonical record/WAL writes for catalog-routed relational
    /// pgwire DML while preserving legacy vector compatibility routing.
    pub fn with_direct_record_writes(
        mut self,
        record_storage: Arc<dyn RecordStorage>,
        wal_appender: Arc<dyn TableWalAppender>,
    ) -> Self {
        self.direct_write_services =
            Some(DirectPgwireWriteServices::new(record_storage, wal_appender));
        self
    }

    /// Enable direct canonical record/WAL writes with prebuilt dependencies.
    pub fn with_direct_write_services(mut self, services: DirectPgwireWriteServices) -> Self {
        self.direct_write_services = Some(services);
        self
    }

    /// Start the PostgreSQL server
    pub async fn start(&self) -> Result<()> {
        let listener = TcpListener::bind(self.bind_address).await?;
        info!("PostgreSQL server listening on {}", self.bind_address);

        self.running
            .store(true, std::sync::atomic::Ordering::SeqCst);

        while self.running.load(std::sync::atomic::Ordering::Relaxed) {
            match listener.accept().await {
                Ok((stream, addr)) => {
                    info!("New PostgreSQL connection from {}", addr);
                    let session_manager = self.session_manager.clone();
                    let collection_port = self.collection_port.clone();
                    let vector_ops = self.vector_ops.clone();
                    let catalog_manager = self.catalog_manager.clone();
                    let document_service = self.document_service.clone();
                    let graph_service = self.graph_service.clone();
                    let observability_service = self.observability_service.clone();
                    let direct_write_services = self.direct_write_services.clone();
                    let rank_pipeline = self.rank_pipeline.clone();

                    tokio::spawn(async move {
                        if let Err(e) = Self::handle_connection(
                            stream,
                            addr,
                            session_manager,
                            collection_port,
                            vector_ops,
                            catalog_manager,
                            document_service,
                            graph_service,
                            observability_service,
                            direct_write_services,
                            rank_pipeline,
                        )
                        .await
                        {
                            error!("Connection error from {}: {}", addr, e);
                        }
                    });
                }
                Err(e) => {
                    error!("Accept error: {}", e);
                }
            }
        }

        Ok(())
    }

    /// Stop the server
    pub fn stop(&self) {
        self.running
            .store(false, std::sync::atomic::Ordering::SeqCst);
        info!("PostgreSQL server stopped");
    }

    /// Handle a client connection
    async fn handle_connection(
        stream: TcpStream,
        addr: SocketAddr,
        session_manager: Arc<SessionManager>,
        collection_port: Arc<dyn proximadb_runtime::CollectionPort>,
        vector_ops: Arc<VectorOperationsService>,
        catalog_manager: Arc<CatalogManager>,
        document_service: Option<Arc<DocumentService>>,
        graph_service: Option<Arc<GraphService>>,
        observability_service: Option<Arc<ObservabilityService>>,
        direct_write_services: Option<DirectPgwireWriteServices>,
        rank_pipeline: Option<PgwireRankPipeline>,
    ) -> Result<()> {
        // Create session
        let session = session_manager.create_session(addr).await?;
        let session_id = session.id.clone();

        // Create protocol handler
        let protocol = PostgresProtocol::new(
            stream,
            session,
            collection_port,
            vector_ops,
            document_service,
            graph_service,
            observability_service,
        );
        let mut protocol = if let Some(direct_write_services) = direct_write_services {
            protocol.with_direct_catalog_manager(
                catalog_manager,
                direct_write_services.record_storage,
                direct_write_services.wal_appender,
            )
        } else {
            protocol.with_catalog_manager(catalog_manager)
        };
        if let Some(pipeline) = rank_pipeline {
            protocol = protocol.with_rank_pipeline(pipeline.services, pipeline.store);
        }

        // Run protocol loop
        match protocol.run().await {
            Ok(_) => {
                debug!("Session {} completed normally", session_id);
            }
            Err(e) => {
                warn!("Session {} error: {}", session_id, e);
            }
        }

        // Cleanup session
        session_manager.remove_session(&session_id).await;

        Ok(())
    }
}

/// PostgreSQL server configuration
#[derive(Debug, Clone)]
pub struct PostgresConfig {
    /// Bind address
    pub bind_address: SocketAddr,
    /// Maximum connections
    pub max_connections: usize,
    /// Idle timeout (seconds)
    pub idle_timeout_secs: u64,
    /// Statement cache size
    pub statement_cache_size: usize,
}

impl Default for PostgresConfig {
    fn default() -> Self {
        Self {
            bind_address: "127.0.0.1:5432".parse().unwrap_or_else(|_| {
                "127.0.0.1:5433"
                    .parse()
                    .unwrap_or_else(|_| std::net::SocketAddr::from(([127, 0, 0, 1], 5433)))
            }),
            max_connections: 100,
            idle_timeout_secs: 3600,
            statement_cache_size: 100,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = PostgresConfig::default();
        assert_eq!(config.max_connections, 100);
        assert_eq!(config.idle_timeout_secs, 3600);
    }
}
