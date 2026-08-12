// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Geospatial secondary index, extracted from the root `index/geo` module
//! (TD-DECOMP-62).
//!
//! [`index`] provides [`index::GeoIndex`] — a geohash-based spatial index
//! supporting radius/bbox/nearest-k/polygon queries. It composes the sibling
//! geo-types / geohash / geo-queries crates to complete the index/geo
//! subsystem extraction. Depends only on those three crates, keeping it a
//! clean horizontal-tier leaf.

pub mod index;
