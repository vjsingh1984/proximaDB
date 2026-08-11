// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Implicit (ID-less) record-id generator, extracted from the root
//! `storage/.../parquet_write_engine` module (TD-DECOMP-54).
//!
//! [`implicit_id_generator`] provides [`implicit_id_generator::IdLessLookup`] and
//! its `generate_implicit_id` helper for synthesizing record IDs when none are
//! supplied. Depends only on `anyhow`, keeping it a clean horizontal-tier leaf.

pub mod implicit_id_generator;
