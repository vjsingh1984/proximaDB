// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Text-search subsystem: full-text inverted index (BM25), TEXT column storage
//! strategies, and semantic-determination-projection (SDP) chunking — hoisted
//! from the root crate's `storage::engines::core::formats::columnar` +
//! `storage::document` modules (TD-DECOMP-70).
//!
//! [`TextStorageStrategy`] moved with the cluster (from root `core::types`);
//! the root re-exports it from there so existing import paths keep working.

pub mod fulltext_index;
pub mod metadata_filter_strategy;
pub mod metadata_filter_types;
pub mod sdp;
pub mod text_filter;
pub mod text_storage;

pub use sdp::{SdpChunk, SdpChunker, SdpConfig};

/// TEXT storage strategy for columnar storage
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum TextStorageStrategy {
    /// Store inline in main Parquet column (<4KB)
    Inline,
    /// Split into chunks with embeddings (4KB-1MB)
    Chunked,
    /// Store in separate sidecar file (>1MB)
    Sidecar,
    /// Auto-select based on actual size (default)
    #[default]
    Adaptive,
}

impl TextStorageStrategy {
    /// Maximum size in bytes for inline text storage (4KB)
    pub const INLINE_MAX_SIZE: usize = 4 * 1024;
    /// Maximum size in bytes for chunked text storage (1MB)
    pub const CHUNKED_MAX_SIZE: usize = 1024 * 1024;

    /// Determine strategy based on content size
    pub fn for_size(size: usize) -> Self {
        if size <= Self::INLINE_MAX_SIZE {
            Self::Inline
        } else if size <= Self::CHUNKED_MAX_SIZE {
            Self::Chunked
        } else {
            Self::Sidecar
        }
    }
}
