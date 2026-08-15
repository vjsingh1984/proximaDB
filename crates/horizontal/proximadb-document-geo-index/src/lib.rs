// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Geohash-based geospatial index for document fields, extracted from the
//! root `storage/document/indexes` module (TD-DECOMP-39).
//!
//! [`geo_index`] maps 2D coordinates to a 1D geohash string for efficient
//! prefix-based spatial range queries. It ships [`geo_index::GeoPoint`] (a
//! validated lat/lon point with a Haversine distance metric) and
//! [`geo_index::GeoIndex`] (point insert/update/remove + radius, bounding-box,
//! and nearest-neighbour queries). The module depends only on `std` +
//! `anyhow`, keeping it a clean horizontal-tier leaf.

pub mod geo_index;
