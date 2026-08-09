// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Connector type definitions, extracted from the root `connectors` module
//! (TD-DECOMP-43).
//!
//! [`types`] carries the shared connector DTOs — [`types::TableInfo`],
//! [`types::ColumnStatistics`], [`types::TableStatistics`], [`types::Statistics`],
//! and [`types::WriteResult`] — built on `arrow::datatypes::Schema`. It depends
//! only on `arrow`/`serde` (zero `proximadb_*` deps), keeping it a clean
//! horizontal-tier leaf that every connector impl (DuckDB, Spark, …) can share.

pub mod types;
