// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Reinforcement-learning reward computation for query-route planning, extracted
//! from the root `query/rl_planner` module (TD-DECOMP-24).
//!
//! [`reward`] computes the reward signal used by the RL route planner. It depends
//! only on `serde`, keeping it a clean query-tier leaf and letting its 15 inline
//! tests run in this crate's own binary rather than the root lib's.

pub mod reward;
