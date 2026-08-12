// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Pushdown protocol types, extracted from the root `connectors` module
//! (TD-DECOMP-47).
//!
//! [`pushdown`] carries the negotiation protocol for predicate / projection /
//! aggregate / vector-search / graph-traversal pushdown —
//! [`pushdown::PushdownRequest`] / [`pushdown::PushdownResponse`] and the
//! expression types engines exchange to push work to the storage layer. Depends
//! only on `serde`, keeping it a clean horizontal-tier leaf.

pub mod pushdown;
