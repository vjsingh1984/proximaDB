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

//! # Cross-Model ACID Transactions
//!
//! This module provides the cross-model transaction coordinator and participant
//! scaffolding for vector, document, graph, and time-series engines.
//!
//! The coordinator, WAL records, and in-memory participants are implemented, but
//! live engine-backed participants are still incomplete. Treat this module as
//! infrastructure under construction rather than a fully wired production path.
//!
//! ## Architecture
//!
//! ```text
//! ┌──────────────────────────────────────────────┐
//! │ CrossModelTransactionCoordinator            │
//! │ - begin_transaction()                       │
//! │ - commit_transaction()                      │
//! │ - rollback_transaction()                    │
//! └──────────────────────────────────────────────┘
//!           ↓                ↓
//! ┌──────────────────┐  ┌─────────────────┐
//! │ TwoPhaseCommit   │  │ WALCoordinator  │
//! │ (2PC protocol)   │  │ (WAL recovery)  │
//! └──────────────────┘  └─────────────────┘
//! ```
//!
//! ## Usage
//!
//! ```rust,no_run
//! use proximadb::transaction::{
//!     CrossModelTransactionCoordinator, TransactionConfig
//! };
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Create coordinator
//! let config = TransactionConfig::default();
//! let coordinator = CrossModelTransactionCoordinator::new(config);
//! coordinator.initialize().await?;
//!
//! // Begin transaction
//! let tx_id = coordinator.begin_transaction().await?;
//!
//! // ... perform operations ...
//!
//! // Commit transaction
//! coordinator.commit_transaction(tx_id, &["vector:products".to_string()]).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Current Status
//!
//! - **Coordinator + WAL flow**: Implemented
//! - **2PC participant protocol**: Implemented
//! - **Live engine-backed participants**: Partial / experimental
//! - **Production-ready atomic multi-model writes**: Not yet fully wired

pub mod coordinator;
pub mod participants;
pub mod two_phase_commit;
pub mod wal_coordinator;

pub use coordinator::{CrossModelTransactionCoordinator, TransactionConfig, TransactionStats};
pub use participants::{
    DocumentEngineParticipant, GraphEngineParticipant, TimeSeriesEngineParticipant,
    TransactionBuffer, VectorEngineParticipant,
};
pub use two_phase_commit::{
    TransactionId, TransactionParticipant, TransactionState, TwoPhaseCommit, Vote,
};
pub use wal_coordinator::{TransactionWALRecord, WALCoordinator, WALTransactionState};
