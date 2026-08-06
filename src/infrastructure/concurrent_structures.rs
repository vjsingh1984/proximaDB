// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Re-export shim.
//!
//! The concurrent data structures (`ConcurrentStorage`, `AtomicMetrics`,
//! `ConcurrentMapping`, `TypedStorage`, …) now live in the standalone
//! `proximadb-concurrent` foundation crate (extracted from the root crate to
//! shrink single-compile RSS — part of the root-crate decomposition track).
//!
//! This file preserves the `crate::infrastructure::concurrent_structures::*`
//! path for existing callers and is intentionally a pure forwarding shim: every
//! public item is re-exported unchanged. New code should depend on
//! `proximadb_concurrent` directly instead of reaching back into the root crate.
pub use proximadb_concurrent::*;
