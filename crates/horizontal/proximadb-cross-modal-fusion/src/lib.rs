// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Modality-agnostic cross-modal fusion core, extracted from the root
//! `core/search` module (TD-DECOMP-19).
//!
//! The [`cross_modal_fusion::Fuser`] operates only on `(oid, score)` lists, so it
//! is unit-testable in isolation and reused by every modality's result-merge path.
//! The module has no external dependencies beyond `std`, which keeps it a clean
//! horizontal-tier leaf.

pub mod cross_modal_fusion;
