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

//! # Change Data Capture (CDC) Module
//!
//! This module provides CDC capabilities for ProximaDB, enabling:
//!
//! - **Outbound CDC**: Stream ProximaDB changes to external systems (Kafka, webhooks)
//! - **Schema Transformation**: Map source schemas to vector embeddings
//! - **Inbound CDC**: Capture changes from external databases (via Debezium integration)
//!
//! ## Production vs Experimental
//!
//! ### Production-Ready (Always Available)
//!
//! - **Event types**: `ChangeEvent`, `Operation`, `RecordState`, `TransactionInfo`
//! - **Outbound CDC**: `WalSubscriber`, `PositionTracker`, `EventRouter`
//! - **Sinks**: `KafkaSink`, `WebhookSink` for event delivery
//! - **Transforms**: `SchemaMapper`, `FilterRuleSet`, `EmbeddingPipeline`
//! - **Exactly-Once**: `ExactlyOnceManager`, `DeduplicationCache`
//!
//! ### Experimental (Feature-Gated)
//!
//! Native database connectors require `experimental-cdc-connectors` feature:
//! - PostgreSQL, MySQL, MongoDB connectors have partial implementations
//! - **Recommended**: Use Debezium for production inbound CDC
//!
//! ## Architecture
//!
//! ```text
//! External Sources                    ProximaDB                    External Sinks
//! ┌─────────────┐                                                 ┌─────────────┐
//! │  Debezium   │───┐                                         ┌───│   Kafka     │
//! │  (via Kafka)│   │    ┌──────────────────────────────┐     │   └─────────────┘
//! └─────────────┘   │    │       CDC Coordinator        │     │   ┌─────────────┐
//!                   ├───▶│  ┌────────┐    ┌─────────┐  │─────┼───│  Webhooks   │
//! ┌─────────────┐   │    │  │Offset  │    │Transform│  │     │   └─────────────┘
//! │  ProximaDB  │   │    │  │Store   │    │Pipeline │  │     │   ┌─────────────┐
//! │    WAL      │───┘    │  └────────┘    └─────────┘  │     └───│   S3/GCS    │
//! └─────────────┘        └──────────────────────────────┘         └─────────────┘
//! ```
//!
//! ## Features
//!
//! - **Unified Event Format**: Consistent change events across all sources
//! - **Offset Management**: Durable offset storage for resume capability
//! - **Schema Mapping**: Transform source records to vector embeddings
//! - **Exactly-Once Delivery**: Transactional guarantees for sinks
//! - **Multi-Sink Routing**: Route events to multiple destinations
//!
//! ## Outbound CDC Example
//!
//! ```rust,ignore
//! use proximadb::cdc::{WalSubscriber, OutboundConfig, KafkaSink, KafkaConfig};
//!
//! // Subscribe to ProximaDB WAL changes
//! let config = OutboundConfig::new()
//!     .with_name("analytics_pipeline")
//!     .with_collection("products")
//!     .with_exactly_once(true);
//!
//! let subscriber = WalSubscriber::new("analytics", config);
//!
//! // Configure Kafka sink
//! let kafka_config = KafkaConfig::new(vec!["localhost:9092".to_string()])
//!     .with_topic_pattern("proximadb.{collection}");
//!
//! // Poll and send events
//! while let Some(events) = subscriber.poll_events().await? {
//!     for event in events {
//!         // Route to Kafka
//!     }
//!     subscriber.acknowledge(last_lsn).await?;
//! }
//! ```
//!
//! ## Debezium Integration (Recommended for External Databases)
//!
//! For production CDC from PostgreSQL, MySQL, or MongoDB, use Debezium:
//!
//! 1. Deploy Debezium Connect with appropriate connector
//! 2. Configure Debezium to output to Kafka
//! 3. Consume Kafka topics with ProximaDB's webhook sink or custom consumer
//!
//! ```rust,ignore
//! use proximadb::cdc::{WebhookConfig, WebhookSink};
//!
//! // Configure webhook to receive Debezium events
//! let config = WebhookConfig::new("http://localhost:8080/cdc/debezium")
//!     .with_timeout(5000)
//!     .with_retries(3);
//! ```
//!
//! See `docs/guides/cdc-debezium-integration.adoc` for detailed setup.

pub mod config;
pub mod connectors;
pub mod coordinator;
pub mod error;
pub mod event;
pub mod metrics;
pub mod offset;
pub mod outbound;
pub mod sinks;
pub mod source;
pub mod transform;

// Re-export main types
pub use config::{CdcConfig, SinkConfig, SourceConfig, TransformConfig};
pub use coordinator::{CdcCoordinator, CoordinatorHandle};
pub use error::{CdcError, CdcResult};
pub use event::{
    ChangeEvent, ConnectorType, Operation, RecordState, SourceInfo, TransactionInfo,
};
pub use metrics::CdcMetrics;
pub use offset::{FileOffsetStore, MemoryOffsetStore, Offset, OffsetStore};
pub use sinks::{CdcSink, KafkaConfig, KafkaSink, SinkError, WebhookConfig, WebhookSink};
pub use source::{CdcSource, SourceHandle, SourceStatus};
pub use transform::{
    EmbeddingPipeline, FilterRule, FilterRuleSet, SchemaMapper, TransformPipeline,
};

// Outbound CDC exports
pub use outbound::{
    DeduplicationCache, DeduplicationStrategy, EventRouter, ExactlyOnceManager, IdempotencyKey,
    OutboundConfig, Position, PositionTracker, RouteRule, RoutingDecision, SubscriberHandle,
    SubscriptionConfig, SubscriptionStatus, TransactionState, WalSubscriber,
};
