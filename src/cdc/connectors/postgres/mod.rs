/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! PostgreSQL CDC Connector
//!
//! This module implements Change Data Capture for PostgreSQL using logical replication.
//!
//! ## Features
//!
//! - Logical replication via pgoutput protocol
//! - Automatic replication slot management
//! - LSN-based offset tracking for resume capability
//! - Initial snapshot support
//! - Column-based vector/metadata extraction
//!
//! ## PostgreSQL Setup Requirements
//!
//! 1. Set `wal_level = logical` in postgresql.conf
//! 2. Create a publication: `CREATE PUBLICATION proximadb_pub FOR TABLE ...`
//! 3. Grant replication privileges to the user
//!
//! ## Example
//!
//! ```rust,ignore
//! use proximadb::cdc::connectors::postgres::{PostgresConnector, PostgresConfig, TableConfig};
//!
//! let config = PostgresConfig {
//!     connection_string: "postgres://user:pass@localhost/db".to_string(),
//!     slot_name: "proximadb_cdc".to_string(),
//!     publication: "proximadb_pub".to_string(),
//!     tables: vec![
//!         TableConfig {
//!             schema: "public".to_string(),
//!             table: "products".to_string(),
//!             primary_key: vec!["id".to_string()],
//!             vector_column: Some("embedding".to_string()),
//!             embed_columns: Some(vec!["title".to_string(), "description".to_string()]),
//!             metadata_columns: vec!["category".to_string(), "price".to_string()],
//!         }
//!     ],
//!     snapshot_mode: SnapshotMode::Initial,
//! };
//!
//! let connector = PostgresConnector::new(config, offset_store).await?;
//! ```

mod config;
mod connector;
mod decoder;

pub use config::{ColumnMapping, PostgresConfig, SnapshotMode, TableConfig};
pub use connector::PostgresConnector;
pub use decoder::{ColumnValue, PgOutputDecoder, PgOutputEvent, PgRelation, TupleData};
