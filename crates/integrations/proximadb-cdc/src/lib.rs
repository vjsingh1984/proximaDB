// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! # ProximaDB CDC (Change Data Capture)
//!
//! CDC event types + runtime, extracted from the root `src/cdc/` module
//! (TD-DECOMP-7, root-monolith decomposition). This first slice moves the
//! self-contained `event` module — pure change-event data types (`ChangeEvent`,
//! `Operation`, `RecordState`, `SourceInfo`, `TransactionInfo`, `ConnectorType`)
//! with deps only `serde`. Subsequent slices move the connectors/sinks/transform/
//! coordinator as cohesive subgraphs.
//!
//! Layering: integration-tier (`crates/integrations/*`); may depend only on
//! foundation/horizontal/contracts.

pub mod event;
