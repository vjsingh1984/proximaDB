// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Pluggable value validators for typed columns, extracted from the root
//! `core/types` module (TD-DECOMP-44).
//!
//! [`validators`] ships the [`validators::TypeValidator`] trait plus concrete
//! validators (UUID, JSON, geographic point/lat/lon, numeric range, string
//! depth, etc.) and a [`validators::ValidatorRegistry`] for name→validator
//! lookup. Depends only on `anyhow`/`regex`/`serde_json`/`uuid` (zero
//! `proximadb_*` deps), keeping it a clean horizontal-tier leaf.

pub mod validators;
