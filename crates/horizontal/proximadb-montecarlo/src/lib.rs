// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Monte-Carlo option-pricing kernel, extracted from the root `compute`
//! module (TD-DECOMP-41).
//!
//! [`montecarlo`] ships a dependency-free Black-Scholes pricer plus a
//! parallel ([`montecarlo::mc_price_batch_seq`] / rayon) Monte-Carlo path
//! simulator for European option pricing. It mirrors how Spark prices
//! options as a UDF and depends only on `rayon`/`rand`, keeping it a clean
//! horizontal-tier leaf.

pub mod montecarlo;
