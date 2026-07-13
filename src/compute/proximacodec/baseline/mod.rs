// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Baseline encoder/decoder — shim re-exporting the `proximadb-codec` crate.
//!
//! The baseline (non-SIMD, non-GPU) implementations now live in the
//! `proximadb-codec` horizontal crate. This module keeps the historical
//! `crate::compute::proximacodec::baseline` path resolving for existing
//! consumers during the root-crate decomposition.

pub use proximadb_codec::baseline::*;
