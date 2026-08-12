// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Batch-group cache tier for stream-based RAG query batching (LLD 6.3, CALL arXiv 2509.18670),
//! extracted from the root `query/cache` module (TD-DECOMP-25).
//!
//! [`batch_group`] holds the per-batch cache (`BatchGroupCache`) that groups batched queries by
//! cluster-access pattern to emit prefetch hints at group transitions. Depends only on `tokio`,
//! keeping it a clean query-tier leaf.

pub mod batch_group;
