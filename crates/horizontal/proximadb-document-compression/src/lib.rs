// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Document field compression, extracted from the root `storage/document`
//! module (TD-DECOMP-56).
//!
//! [`compression`] provides [`compression::DocumentCompressor`] for per-field
//! codec selection over document storage. Depends only on `anyhow`, keeping it
//! a clean horizontal-tier leaf.

pub mod compression;
