// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Query cache policy utilities, extracted from the root `query/cache` module (TD-DECOMP-29).
//! These are the tractable, self-contained files whose deps are intra-module only.
//! The remaining coupled files (query_result_cache, plan_cache, invalidation_coordinator)
//! stay root-side until their root-only deps (ExecutionResult, PlanOutput, QueryCache) extract.

pub mod category_classifier;
pub mod mismatch_cost;
pub mod per_category_policy;
pub mod result_cache_gate;
