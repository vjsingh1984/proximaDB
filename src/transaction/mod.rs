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
//! This module provides ACID transaction support across multiple data models
//! (vector, document, graph, time-series) using two-phase commit protocol.
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
//! ## Features
//!
//! - **Atomicity**: All-or-nothing updates across models
//! - **Consistency**: Data integrity across models
//! - **Isolation**: Transactions don't interfere
//! - **Durability**: WAL-based crash recovery

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
