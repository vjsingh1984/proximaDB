// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Record-granularity buffer-pool primitive (LLD §4, VeloANN arXiv 2602.22805), extracted from
//! the root `storage/cache` module (TD-DECOMP-26). A generic clock admission/eviction pool;
//! depends only on `tokio`.

pub mod record_buffer_pool;
