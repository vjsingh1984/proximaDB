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

//! # CDC Connectors
//!
//! This module provides connectors for various source databases.
//!
//! ## Production Status
//!
//! **Important**: All native database connectors are currently **experimental** and require
//! the `experimental-cdc-connectors` feature flag. They have partial implementations without
//! complete network I/O.
//!
//! ### Recommended Production Approach
//!
//! For production CDC from external databases, we recommend using **Debezium**:
//!
//! 1. Deploy Debezium to capture changes from your source database
//! 2. Configure Debezium to output to Kafka
//! 3. Use ProximaDB's Kafka sink to consume changes
//!
//! Alternatively, use the webhook sink with Debezium's HTTP connector.
//!
//! See `docs/guides/cdc-debezium-integration.adoc` for detailed setup instructions.
//!
//! ## Available Connectors (Experimental)
//!
//! These connectors require the `experimental-cdc-connectors` feature:
//!
//! - **PostgreSQL**: Logical replication with pgoutput protocol decoder
//!   - Has complete pgoutput parsing
//!   - Missing: actual network connection layer
//!
//! - **MySQL**: Binlog replication (INCOMPLETE)
//!   - Has binlog event decoder
//!   - Missing: MySQL connection, slave registration, binlog dump
//!
//! - **MongoDB**: Change streams (INCOMPLETE)
//!   - Has change event parsing
//!   - Missing: MongoDB connection, change stream subscription
//!
//! ## Core CDC Features (Always Available)
//!
//! The following CDC components are always available without feature flags:
//!
//! - **Event types**: `ChangeEvent`, `Operation`, `RecordState`
//! - **Outbound CDC**: Stream ProximaDB changes to external systems
//! - **Sinks**: Kafka and webhook sinks for event delivery
//! - **Transforms**: Schema mapping, filtering, embedding pipeline

#[cfg(feature = "experimental-cdc-connectors")]
pub mod mongodb;
#[cfg(feature = "experimental-cdc-connectors")]
pub mod mysql;
#[cfg(feature = "experimental-cdc-connectors")]
pub mod postgres;

// Re-export main connector types (only when feature is enabled)
#[cfg(feature = "experimental-cdc-connectors")]
pub use mongodb::{
    ChangeStreamOperation, DocumentKey, FullDocumentOption, MongoChangeEvent, MongoCollectionConfig,
    MongoDbConfig, MongoDbConnector, UpdateDescription,
};
#[cfg(feature = "experimental-cdc-connectors")]
pub use mysql::{BinlogPosition, GtidMode, MySqlConfig, MySqlConnector, MySqlTableConfig};
#[cfg(feature = "experimental-cdc-connectors")]
pub use postgres::{PostgresConfig, PostgresConnector, SnapshotMode, TableConfig};
