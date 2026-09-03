// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Multimodel Arrow schema codecs, extracted from the root `network/arrow_ipc`
//! module (TD-DECOMP-68).
//!
//! [`multimodel_codec`] generates Arrow [`arrow_schema::Schema`]s for each of
//! ProximaDB's data models (document/node/edge/metric/log/trace/relational)
//! from the canonical [`proximadb_catalog_schema::CatalogTableSchema`], plus
//! model detection from catalog descriptors. Depends only on `arrow-schema` +
//! the foundation `proximadb-catalog-schema` crate.

pub mod multimodel_codec;
