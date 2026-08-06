// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Utility-aware final-evidence scorer, extracted from the root
//! `core/search/hybrid` module (TD-DECOMP-20).
//!
//! [`utility_scorer`] blends vector similarity with operational utility
//! (lexical, source-authority, freshness, diversity, tenant-local historical
//! success) for final ranking. It ships the linear-blend default
//! ([`utility_scorer::LinearUtilityScorer`]) plus the pluggable
//! [`utility_scorer::UtilityScorer`] trait and path-based artifact wrapper
//! ([`utility_scorer::ArtifactUtilityScorer`]). The module depends only on
//! `std`, keeping it a clean horizontal-tier leaf.

pub mod utility_scorer;
