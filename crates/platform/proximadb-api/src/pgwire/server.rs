//! # PostgreSQL Wire Protocol Server
//!
//! Server that listens for PostgreSQL wire protocol connections.
//!
//! ## Migration Status
//!
//! **TEMPORARY PLACEHOLDER**: Full implementation migrates from
//! `src/network/postgres/`.

use std::net::SocketAddr;
use std::sync::Arc;

use proximadb_runtime::UnifiedHandlers;

/// PostgreSQL wire protocol server
///
/// Accepts connections from any PostgreSQL-compatible client and translates
/// queries to ProximaDB operations via UnifiedHandlers.
pub struct PostgresServer {
    _handlers: Arc<UnifiedHandlers>,
    /// Bind address for the pgwire listener
    pub bind_addr: SocketAddr,
}

impl PostgresServer {
    /// Create a new PostgreSQL wire server bound to the given address
    pub fn new(handlers: Arc<UnifiedHandlers>, bind_addr: SocketAddr) -> Self {
        Self {
            _handlers: handlers,
            bind_addr,
        }
    }

    /// Start listening for PostgreSQL wire protocol connections
    ///
    /// Returns an error - migration in progress, use `src/network/postgres` until complete.
    pub async fn serve(&self) -> Result<(), anyhow::Error> {
        anyhow::bail!("PostgreSQL wire server migration in progress; use src/network/postgres")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::noop_unified_handlers;

    #[tokio::test]
    async fn server_preserves_bind_address_and_reports_migration_status() {
        let bind_addr: SocketAddr = "127.0.0.1:5433".parse().unwrap();
        let server = PostgresServer::new(noop_unified_handlers(), bind_addr);

        assert_eq!(server.bind_addr, bind_addr);
        let error = server.serve().await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("PostgreSQL wire server migration in progress")
        );
    }
}
