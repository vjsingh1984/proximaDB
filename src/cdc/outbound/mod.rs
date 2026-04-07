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

//! Outbound CDC Module
//!
//! This module provides CDC capabilities for streaming ProximaDB changes
//! to external systems with exactly-once delivery guarantees.
//!
//! ## Features
//!
//! - **WAL Subscription**: Subscribe to ProximaDB's Write-Ahead Log
//! - **Position Tracking**: Resume from last processed position
//! - **Multi-Sink Routing**: Route changes to multiple destinations
//! - **Exactly-Once Delivery**: Transactional guarantees for sinks
//! - **Deduplication**: Prevent duplicate event processing
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                       ProximaDB WAL                            │
//! │  ┌──────┐  ┌──────┐  ┌──────┐  ┌──────┐  ┌──────┐            │
//! │  │Entry1│  │Entry2│  │Entry3│  │Entry4│  │Entry5│  ...       │
//! │  └──┬───┘  └──┬───┘  └──┬───┘  └──┬───┘  └──┬───┘            │
//! └─────┼────────┼────────┼────────┼────────┼─────────────────────┘
//!       │        │        │        │        │
//!       ▼        ▼        ▼        ▼        ▼
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                     WAL Subscriber                              │
//! │  ┌────────────┐  ┌────────────┐  ┌─────────────┐              │
//! │  │Position    │  │Dedup       │  │Transaction  │              │
//! │  │Tracker     │  │Cache       │  │Log          │              │
//! │  └────────────┘  └────────────┘  └─────────────┘              │
//! └─────────────────────────────────────────────────────────────────┘
//!                          │
//!                          ▼
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                     Event Router                                │
//! │            ┌──────────┬──────────┬──────────┐                  │
//! │            ▼          ▼          ▼          ▼                  │
//! │         ┌─────┐    ┌─────┐    ┌─────┐    ┌─────┐              │
//! │         │Kafka│    │Webhook│  │S3    │    │...  │              │
//! │         └─────┘    └─────┘    └─────┘    └─────┘              │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Example
//!
//! ```rust,ignore
//! use proximadb::cdc::outbound::{WalSubscriber, OutboundConfig};
//!
//! let config = OutboundConfig::new()
//!     .with_collection("products")
//!     .with_exactly_once(true);
//!
//! let subscriber = WalSubscriber::new(config, wal_manager).await?;
//!
//! while let Some(event) = subscriber.next().await? {
//!     // Process event
//!     subscriber.ack(event.lsn).await?;
//! }
//! ```

mod config;
mod dedup;
mod exactly_once;
mod position;
mod router;
mod subscriber;

pub use config::{OutboundConfig, RouteConfig, SubscriptionConfig};
pub use dedup::{DeduplicationCache, DeduplicationStrategy};
pub use exactly_once::{ExactlyOnceManager, IdempotencyKey, TransactionState};
pub use position::{Position, PositionStore, PositionTracker};
pub use router::{EventRouter, RouteRule, RoutingDecision};
pub use subscriber::{SubscriberHandle, SubscriptionStatus, WalSubscriber};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdc::event::{ChangeEvent, Operation, SourceInfo};

    #[allow(dead_code)]
    fn create_test_event(lsn: u64) -> ChangeEvent {
        let mut event = ChangeEvent::new(
            SourceInfo::proximadb("testdb", "test_server"),
            Operation::Insert,
            "products",
            format!("prod_{}", lsn),
        );
        event.lsn = lsn;
        event
    }

    #[test]
    fn test_outbound_module_exports() {
        // Verify all public types are accessible
        let _config = OutboundConfig::new();
        let _dedup = DeduplicationCache::new(1000);
        let _tracker = PositionTracker::new();
    }
}
