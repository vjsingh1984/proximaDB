// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Geospatial query builders, extracted from the root `index/geo` module
//! (TD-DECOMP-61).
//!
//! [`queries`] provides [`queries::GeoQuery`], [`queries::GeoQueryBuilder`],
//! and [`queries::GeoQueryResult`] over the [`proximadb_geo_types::types`]
//! coordinate types. Depends only on `serde` + the sibling `proximadb-geo-types`
//! crate, keeping it a clean horizontal-tier leaf.

pub mod queries;
