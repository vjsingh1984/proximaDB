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

//! # MySQL CDC Connector - INCOMPLETE
//!
//! **Status**: This connector is incomplete and not production-ready.
//!
//! The connector has framework code but the `start()` method does not actually
//! connect to MySQL or stream binlog events. The binlog decoder has partial
//! implementation but network I/O is not implemented.
//!
//! ## What Works
//! - Configuration validation
//! - Binlog position and GTID tracking (in-memory)
//! - Binlog event decoding (partial, from buffers)
//! - Event conversion to ChangeEvent format
//!
//! ## What Does NOT Work
//! - Actual MySQL connection (no network I/O)
//! - Slave registration
//! - Binlog dump request
//! - Automatic reconnection
//!
//! ## Recommended Alternative
//!
//! For production MySQL CDC, use **Debezium**:
//! - <https://debezium.io/documentation/reference/stable/connectors/mysql.html>
//!
//! Debezium provides:
//! - Full binlog replication support
//! - GTID and file/position tracking
//! - Schema evolution handling
//! - Battle-tested in production
//!
//! You can stream Debezium output to ProximaDB via Kafka or HTTP webhooks.
//!
//! ---
//!
//! ## Legacy Documentation (Archived)
//!
//! This module implements Change Data Capture for MySQL using binlog replication.
//!
//! ### Features
//!
//! - Binlog replication with row-based events
//! - GTID-based position tracking
//! - Table map event parsing
//! - Automatic reconnection
//!
//! ### MySQL Setup Requirements
//!
//! 1. Enable binlog: `log_bin = mysql-bin` in my.cnf
//! 2. Set format: `binlog_format = ROW`
//! 3. Enable GTID (optional): `gtid_mode = ON`
//! 4. Grant REPLICATION SLAVE, REPLICATION CLIENT privileges
//!
//! ### Example
//!
//! ```rust,ignore
//! use proximadb::cdc::connectors::mysql::{MySqlConnector, MySqlConfig};
//!
//! let config = MySqlConfig::new("mysql://user:pass@localhost/db")
//!     .with_server_id(12345)
//!     .with_table(MySqlTableConfig::new("mydb", "users"));
//!
//! let connector = MySqlConnector::new(config, offset_store).await?;
//! ```

mod config;
mod connector;
mod decoder;

pub use config::{BinlogPosition, GtidMode, MySqlConfig, MySqlTableConfig};
pub use connector::MySqlConnector;
pub use decoder::{BinlogDecoder, BinlogEvent, ColumnDef, RowEvent, TableMapEvent};
