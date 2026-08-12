// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Geospatial type definitions, extracted from the root `index/geo` module
//! (TD-DECOMP-59).
//!
//! [`types`] carries [`types::GeoPoint`], [`types::GeoBoundingBox`],
//! [`types::GeoCircle`], [`types::GeoPolygon`], and [`types::GeoDistanceUnit`]
//! — the shared geo DTOs that the sibling geohash/index/queries modules
//! consume. Depends only on `serde`, keeping it a clean horizontal-tier leaf.

pub mod types;
