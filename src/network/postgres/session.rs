// PostgreSQL session management
//
// Provides:
// - Session state tracking
// - Connection pooling
// - Transaction management

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use anyhow::Result;
use tokio::sync::RwLock;
use tracing::{debug, info};

/// Session manager for PostgreSQL connections
pub struct SessionManager {
    /// Active sessions
    sessions: RwLock<HashMap<String, Arc<RwLock<Session>>>>,
    /// Next process ID
    next_process_id: AtomicU32,
    /// Session counter
    session_counter: AtomicU64,
}

impl SessionManager {
    /// Create a new session manager
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            next_process_id: AtomicU32::new(1000),
            session_counter: AtomicU64::new(0),
        }
    }

    /// Create a new session
    pub async fn create_session(&self, addr: SocketAddr) -> Result<Session> {
        let process_id = self.next_process_id.fetch_add(1, Ordering::SeqCst);
        let session_num = self.session_counter.fetch_add(1, Ordering::SeqCst);

        let session = Session {
            id: format!("pg-{}-{}", process_id, session_num),
            process_id,
            secret_key: rand::random(),
            user: String::new(),
            database: String::new(),
            client_addr: addr,
            state: SessionState::Startup,
            transaction_state: TransactionState::Idle,
            parameters: HashMap::new(),
            created_at: std::time::Instant::now(),
            last_activity: std::time::Instant::now(),
        };

        // Store session
        let session_arc = Arc::new(RwLock::new(session.clone()));
        let mut sessions = self.sessions.write().await;
        sessions.insert(session.id.clone(), session_arc);

        info!("Created PostgreSQL session: {}", session.id);

        Ok(session)
    }

    /// Get a session by ID
    pub async fn get_session(&self, id: &str) -> Option<Arc<RwLock<Session>>> {
        let sessions = self.sessions.read().await;
        sessions.get(id).cloned()
    }

    /// Remove a session
    pub async fn remove_session(&self, id: &str) {
        let mut sessions = self.sessions.write().await;
        if sessions.remove(id).is_some() {
            info!("Removed PostgreSQL session: {}", id);
        }
    }

    /// Get active session count
    pub async fn session_count(&self) -> usize {
        self.sessions.read().await.len()
    }

    /// Cleanup idle sessions
    pub async fn cleanup_idle(&self, timeout_secs: u64) -> usize {
        let mut sessions = self.sessions.write().await;
        let timeout = std::time::Duration::from_secs(timeout_secs);
        let mut removed = 0;

        sessions.retain(|id, session| {
            let session = session.try_read();
            match session {
                Ok(s) => {
                    if s.last_activity.elapsed() > timeout {
                        debug!("Removing idle session: {}", id);
                        removed += 1;
                        false
                    } else {
                        true
                    }
                }
                Err(_) => true, // Keep locked sessions
            }
        });

        removed
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

/// PostgreSQL session
#[derive(Debug, Clone)]
pub struct Session {
    /// Session ID
    pub id: String,
    /// Backend process ID
    pub process_id: u32,
    /// Secret key for cancellation
    pub secret_key: u32,
    /// Connected user
    pub user: String,
    /// Connected database
    pub database: String,
    /// Client address
    pub client_addr: SocketAddr,
    /// Session state
    pub state: SessionState,
    /// Transaction state
    pub transaction_state: TransactionState,
    /// Session parameters
    pub parameters: HashMap<String, String>,
    /// Creation time
    pub created_at: std::time::Instant,
    /// Last activity time
    pub last_activity: std::time::Instant,
}

impl Session {
    /// Update last activity time
    pub fn touch(&mut self) {
        self.last_activity = std::time::Instant::now();
    }

    /// Set a session parameter
    pub fn set_parameter(&mut self, name: &str, value: &str) {
        self.parameters.insert(name.to_string(), value.to_string());
    }

    /// Get a session parameter
    pub fn get_parameter(&self, name: &str) -> Option<&String> {
        self.parameters.get(name)
    }

    /// TD-064 S1: the connection's effective schema == ProximaDB namespace.
    ///
    /// `SET search_path TO <schema>[, ...]` is captured into `parameters`; we
    /// take the first (primary) entry, stripping quotes/whitespace, and default
    /// to `public` exactly like PostgreSQL when the client never set one.
    pub fn current_schema(&self) -> String {
        self.parameters
            .get("search_path")
            .and_then(|raw| raw.split(',').next())
            .map(|s| s.trim().trim_matches('"').to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "public".to_string())
    }

    /// TD-064 S1: the connection's catalog == account == tenant.
    ///
    /// Per the `catalog.schema.table` model the startup `database` name is the
    /// connection's catalog/tenant boundary. Empty (client sent no database)
    /// → `None`, so callers fall back to the legacy `proximadb.write.tenant_id`
    /// session var during the read/write-binding migration (S1 read-half;
    /// the write path is migrated separately).
    pub fn catalog_tenant(&self) -> Option<String> {
        let database = self.database.trim();
        (!database.is_empty()).then(|| database.to_string())
    }

    /// Start a transaction
    pub fn begin_transaction(&mut self) {
        self.transaction_state = TransactionState::InTransaction;
    }

    /// Commit a transaction
    pub fn commit_transaction(&mut self) {
        self.transaction_state = TransactionState::Idle;
    }

    /// Rollback a transaction
    pub fn rollback_transaction(&mut self) {
        self.transaction_state = TransactionState::Idle;
    }

    /// Mark transaction as failed
    pub fn fail_transaction(&mut self) {
        if self.transaction_state == TransactionState::InTransaction {
            self.transaction_state = TransactionState::Failed;
        }
    }
}

/// Session state
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SessionState {
    /// Initial startup
    Startup,
    /// Authenticating
    Authenticating,
    /// Ready for queries
    Ready,
    /// Processing a query
    Processing,
    /// Terminated
    Terminated,
}

/// Transaction state
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TransactionState {
    /// Not in transaction
    Idle,
    /// In active transaction
    InTransaction,
    /// Transaction failed
    Failed,
}

impl TransactionState {
    /// Get the PostgreSQL status byte
    pub fn status_byte(&self) -> char {
        match self {
            TransactionState::Idle => 'I',
            TransactionState::InTransaction => 'T',
            TransactionState::Failed => 'E',
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_session_manager() {
        let manager = SessionManager::new();

        let addr: SocketAddr = "127.0.0.1:12345".parse().unwrap();
        let session = manager.create_session(addr).await.unwrap();

        assert!(!session.id.is_empty());
        assert_eq!(manager.session_count().await, 1);

        manager.remove_session(&session.id).await;
        assert_eq!(manager.session_count().await, 0);
    }

    #[test]
    fn test_transaction_state() {
        let mut session = Session {
            id: "test".to_string(),
            process_id: 1,
            secret_key: 123,
            user: String::new(),
            database: String::new(),
            client_addr: "127.0.0.1:5432".parse().unwrap(),
            state: SessionState::Ready,
            transaction_state: TransactionState::Idle,
            parameters: HashMap::new(),
            created_at: std::time::Instant::now(),
            last_activity: std::time::Instant::now(),
        };

        assert_eq!(session.transaction_state.status_byte(), 'I');

        session.begin_transaction();
        assert_eq!(session.transaction_state, TransactionState::InTransaction);
        assert_eq!(session.transaction_state.status_byte(), 'T');

        session.fail_transaction();
        assert_eq!(session.transaction_state, TransactionState::Failed);
        assert_eq!(session.transaction_state.status_byte(), 'E');

        session.rollback_transaction();
        assert_eq!(session.transaction_state, TransactionState::Idle);
    }

    fn blank_session() -> Session {
        Session {
            id: "test".to_string(),
            process_id: 1,
            secret_key: 123,
            user: String::new(),
            database: String::new(),
            client_addr: "127.0.0.1:5432".parse().unwrap(),
            state: SessionState::Ready,
            transaction_state: TransactionState::Idle,
            parameters: HashMap::new(),
            created_at: std::time::Instant::now(),
            last_activity: std::time::Instant::now(),
        }
    }

    #[test]
    fn current_schema_defaults_to_public_and_honors_search_path() {
        let mut session = blank_session();
        // No search_path set → PostgreSQL default.
        assert_eq!(session.current_schema(), "public");

        // First entry of a comma list, quote/space-stripped, wins.
        session.set_parameter("search_path", "\"sales\" , public");
        assert_eq!(session.current_schema(), "sales");

        // Empty value falls back to the default.
        session.set_parameter("search_path", "");
        assert_eq!(session.current_schema(), "public");
    }

    #[test]
    fn catalog_tenant_is_database_or_none() {
        let mut session = blank_session();
        // No database on startup → None (callers fall back to the write var).
        assert_eq!(session.catalog_tenant(), None);

        session.database = "acme".to_string();
        assert_eq!(session.catalog_tenant().as_deref(), Some("acme"));

        // Whitespace-only is treated as unset.
        session.database = "   ".to_string();
        assert_eq!(session.catalog_tenant(), None);
    }
}
