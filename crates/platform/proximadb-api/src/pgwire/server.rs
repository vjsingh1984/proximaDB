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
