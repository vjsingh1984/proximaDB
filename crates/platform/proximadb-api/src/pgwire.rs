//! # PostgreSQL Wire Protocol API
//!
//! pgvector-compatible PostgreSQL wire protocol server for ProximaDB.
//!
//! ## Protocol Support
//!
//! - PostgreSQL Protocol v3.0 message parsing and encoding
//! - pgvector migration path (`<->` distance operator)
//! - Standard PostgreSQL clients (psql, pgAdmin, JDBC, etc.)
//! - Authentication: trust, password, MD5
//!
//! ## Migration Status
//!
//! **PLACEHOLDER**: Establishes protocol boundary in API crate. Full implementation
//! migrates from `src/network/postgres/`.

pub mod server;
pub mod session;

pub use server::PostgresServer;
pub use session::PostgresSession;
