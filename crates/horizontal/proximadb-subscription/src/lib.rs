// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Live query subscription primitives, extracted from the root
//! `streaming/subscriptions` module (TD-DECOMP-57).
//!
//! [`subscription`] carries the base subscription state + the shared
//! [`subscription::ScoredResult`] / [`subscription::ResultChange`] types that
//! the sibling evaluator/result-set modules consume. Depends only on
//! `serde`/`tokio`, keeping it a clean horizontal-tier leaf.

pub mod subscription;
