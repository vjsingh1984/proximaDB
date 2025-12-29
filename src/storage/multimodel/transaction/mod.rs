//! # Multi-Model Transaction Support
//!
//! Provides transaction coordination for cross-model operations with
//! support for multiple isolation levels and Two-Phase Commit (2PC).
//!
//! ## Architecture
//!
//! ```text
//! ┌───────────────────────────────────────────────────────────────┐
//! │                   Transaction Coordinator                      │
//! │  ┌─────────────────────────────────────────────────────────┐  │
//! │  │                Two-Phase Commit (2PC)                    │  │
//! │  │  Phase 1: Prepare - Vote to commit/abort                │  │
//! │  │  Phase 2: Commit/Abort - Final decision                 │  │
//! │  └─────────────────────────────────────────────────────────┘  │
//! │                                                               │
//! │  ┌─────────────────────────────────────────────────────────┐  │
//! │  │                Isolation Levels                          │  │
//! │  │  - Read Uncommitted                                     │  │
//! │  │  - Read Committed                                       │  │
//! │  │  - Repeatable Read                                      │  │
//! │  │  - Serializable                                         │  │
//! │  └─────────────────────────────────────────────────────────┘  │
//! │                                                               │
//! │  ┌─────────────────────────────────────────────────────────┐  │
//! │  │                Participant Management                    │  │
//! │  │  - Vector Store participant                             │  │
//! │  │  - Document Store participant                           │  │
//! │  │  - Graph Store participant                              │  │
//! │  │  - RDBMS Store participant                              │  │
//! │  └─────────────────────────────────────────────────────────┘  │
//! └───────────────────────────────────────────────────────────────┘
//! ```

pub mod two_phase_commit;
pub mod isolation;
pub mod coordinator;

// Re-exports
pub use two_phase_commit::{TwoPhaseCommitProtocol, PrepareResult, CommitResult, TransactionState};
pub use isolation::{IsolationLevel, IsolationManager, ReadSnapshot, WriteSet};
pub use coordinator::{TransactionCoordinator, Transaction, TransactionConfig, TransactionStats};
