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

//! # MongoDB CDC Connector - INCOMPLETE
//!
//! **Status**: This connector is incomplete and not production-ready.
//!
//! The connector has framework code but the `start()` method does not actually
//! connect to MongoDB or open change streams. The change event parsing is
//! implemented but network I/O is not.
//!
//! ## What Works
//! - Configuration validation
//! - Resume token tracking (in-memory)
//! - Change event parsing (from MongoChangeEvent structs)
//! - Event conversion to ChangeEvent format
//! - Document to RecordState conversion
//!
//! ## What Does NOT Work
//! - Actual MongoDB connection (no network I/O)
//! - Change stream subscription
//! - Resume token persistence on restart
//! - Full document lookup
//!
//! ## Recommended Alternative
//!
//! For production MongoDB CDC, use **Debezium**:
//! - <https://debezium.io/documentation/reference/stable/connectors/mongodb.html>
//!
//! Debezium provides:
//! - Full change stream support
//! - Resume token management
//! - Schema evolution handling
//! - Battle-tested in production
//!
//! You can stream Debezium output to ProximaDB via Kafka or HTTP webhooks.
//!
//! ---
//!
//! ## Legacy Documentation (Archived)
//!
//! This module implements Change Data Capture for MongoDB using change streams.
//!
//! ### Features
//!
//! - Change stream subscription for real-time updates
//! - Resume token tracking for resumable streams
//! - Full document lookup for updates
//! - Collection and database filtering
//!
//! ### MongoDB Setup Requirements
//!
//! 1. MongoDB 3.6+ with replica set or sharded cluster
//! 2. Read access to the oplog (for change streams)
//!
//! ### Example
//!
//! ```rust,ignore
//! use proximadb::cdc::connectors::mongodb::{MongoDbConnector, MongoDbConfig};
//!
//! let config = MongoDbConfig::new("mongodb://localhost:27017")
//!     .with_database("mydb")
//!     .with_collection(MongoCollectionConfig::new("users"));
//!
//! let connector = MongoDbConnector::new(config, offset_store).await?;
//! ```

mod change_event;
mod config;
mod connector;

pub use change_event::{ChangeStreamOperation, DocumentKey, MongoChangeEvent, UpdateDescription};
pub use config::{FullDocumentOption, MongoCollectionConfig, MongoDbConfig};
pub use connector::MongoDbConnector;
