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

//! Kafka Consumer Integration for Vector Streaming
//!
//! This module provides Kafka consumer capabilities for ingesting vectors
//! from Kafka topics into ProximaDB collections.
//!
//! ## Features
//!
//! - **Consumer Group Coordination**: Automatic partition assignment and rebalancing
//! - **Multiple Serialization Formats**: JSON, Avro, Protobuf support
//! - **Commit Strategies**: At-least-once, exactly-once, manual commits
//! - **Dead Letter Queue**: Failed messages routed to DLQ topic
//! - **Backpressure Handling**: Integration with stream coordinator
//!
//! ## Configuration
//!
//! ```rust,ignore
//! use proximadb::streaming::kafka::{KafkaConsumerConfig, ConsumerGroupConfig};
//!
//! let config = KafkaConsumerConfig {
//!     brokers: vec!["localhost:9092".to_string()],
//!     topic: "vectors".to_string(),
//!     group: ConsumerGroupConfig {
//!         group_id: "proximadb-consumer".to_string(),
//!         ..Default::default()
//!     },
//!     ..Default::default()
//! };
//! ```
//!
//! ## Message Format
//!
//! The consumer expects messages in one of the supported formats:
//!
//! ### JSON Format
//! ```json
//! {
//!     "id": "vec_123",
//!     "vector": [0.1, 0.2, ...],
//!     "metadata": {"key": "value"},
//!     "collection": "my_collection"
//! }
//! ```

pub mod config;
pub mod consumer;
pub mod deserializer;

pub use config::{
    CommitStrategy, ConsumerGroupConfig, DeserializationFormat, DlqConfig, KafkaConsumerConfig,
};
pub use consumer::{KafkaVectorConsumer, ConsumerHandle, ConsumerStatus};
pub use deserializer::{MessageDeserializer, VectorMessage, DeserializationError};
