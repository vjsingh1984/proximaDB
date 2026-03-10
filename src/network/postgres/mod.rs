// PostgreSQL Wire Protocol Implementation
//
// Provides PostgreSQL-compatible server for:
// - pgvector migration path
// - Standard PostgreSQL clients (psql, pgAdmin, etc.)
// - Existing application compatibility
//
// Protocol: PostgreSQL Protocol v3.0

pub mod protocol;
pub mod session;
pub mod translator;
pub mod types;

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use self::protocol::PostgresProtocol;
use self::session::SessionManager;
use crate::services::CollectionService;
use crate::services::VectorOperationsService;
use crate::storage::StorageEngine;

/// PostgreSQL-compatible server
pub struct PostgresServer {
    /// Listen address
    bind_address: SocketAddr,
    /// Session manager
    session_manager: Arc<SessionManager>,
    /// Storage engine reference
    storage: Arc<RwLock<StorageEngine>>,
    /// Collection service reference
    collection_service: Arc<CollectionService>,
    /// Vector operations service for search
    vector_ops: Arc<VectorOperationsService>,
    /// Whether the server is running
    running: Arc<std::sync::atomic::AtomicBool>,
}

impl PostgresServer {
    /// Create a new PostgreSQL server
    pub fn new(
        bind_address: SocketAddr,
        storage: Arc<RwLock<StorageEngine>>,
        collection_service: Arc<CollectionService>,
        vector_ops: Arc<VectorOperationsService>,
    ) -> Self {
        Self {
            bind_address,
            session_manager: Arc::new(SessionManager::new()),
            storage,
            collection_service,
            vector_ops,
            running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
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
                    let storage = self.storage.clone();
                    let collection_service = self.collection_service.clone();
                    let vector_ops = self.vector_ops.clone();

                    tokio::spawn(async move {
                        if let Err(e) = Self::handle_connection(
                            stream,
                            addr,
                            session_manager,
                            storage,
                            collection_service,
                            vector_ops,
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
        storage: Arc<RwLock<StorageEngine>>,
        collection_service: Arc<CollectionService>,
        vector_ops: Arc<VectorOperationsService>,
    ) -> Result<()> {
        // Create session
        let session = session_manager.create_session(addr).await?;
        let session_id = session.id.clone();

        // Create protocol handler
        let mut protocol =
            PostgresProtocol::new(stream, session, storage, collection_service, vector_ops);

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
