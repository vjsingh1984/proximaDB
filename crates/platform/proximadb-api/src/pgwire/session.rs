//! # PostgreSQL Session Management
//!
//! Per-connection session state for PostgreSQL wire protocol.
//!
//! ## Migration Status
//!
//! **TEMPORARY PLACEHOLDER**: Full implementation migrates from
//! `src/network/postgres/session.rs`.

/// PostgreSQL client session
///
/// Tracks per-connection state: authentication, current schema, prepared
/// statements, and portal state.
pub struct PostgresSession {
    /// Session identifier
    pub session_id: u64,
    /// Authenticated username (None until auth completes)
    pub username: Option<String>,
    /// Current schema/namespace
    pub current_schema: String,
}

impl PostgresSession {
    pub fn new(session_id: u64) -> Self {
        Self {
            session_id,
            username: None,
            current_schema: "public".to_string(),
        }
    }
}
