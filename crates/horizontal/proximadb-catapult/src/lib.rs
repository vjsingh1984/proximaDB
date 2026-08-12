// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Catapult shortcut-edge optimization for graph ANN search (LLD 6.3, CatapultDB arXiv 2603.02164),
//! extracted from the root `graph/` module (TD-DECOMP-28). Observes successful search trajectories
//! and injects shortcut edges to skip redundant hops. Depends only on `tokio`.

pub mod catapult;
