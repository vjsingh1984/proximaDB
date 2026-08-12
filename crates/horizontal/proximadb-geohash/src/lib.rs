// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Geohash encoding/decoding, extracted from the root `index/geo` module
//! (TD-DECOMP-60).
//!
//! [`geohash`] provides [`geohash::GeoHash`] + [`geohash::encode_geohash`] /
//! [`geohash::decode_geohash`] / [`geohash::geohash_neighbors`] over the
//! [`proximadb_geo_types::types`] coordinate types. Depends only on the sibling
//! `proximadb-geo-types` crate, keeping it a clean horizontal-tier leaf.

pub mod geohash;
