/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Archive of deprecated SST block types
//!
//! These types have been superseded by production implementations in
//! `storage::engines::core::formats::proximablocks`.
//!
//! Production types to use instead:
//! - `ProximaDataBlock` → `proximablocks::ProximaDataBlock`
//! - `ProximaBlockMetadata` → `proximablocks::ProximaBlockMetadata`
//! - `BlockCompressionConfig` → `proximablocks::BlockCompressionConfig`
//! - `CompressionAlgorithm` → `core::compression::CompressionAlgorithm`
//! - `QuantizedBlockData` → `proximablocks::QuantizedSection`
//!
//! This file is kept for reference and potential rollback.
//! Last used: 2025-12-25
//! Archived by: TD-001 ProximaBlocks migration

use crate::proto::proximadb_v1::VectorRecord;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// DEPRECATED: Use `proximablocks::ProximaDataBlock` instead
///
/// SST record representation with metadata
/// This is the legacy 5-field version, production uses 20+ fields
#[derive(Debug, Clone)]
pub struct LegacyProximaDataBlock {
    pub block_id: u32,
    pub records: Vec<VectorRecord>,
    pub metadata: LegacyProximaBlockMetadata,
    pub compression: LegacyBlockCompressionConfig,
    pub quantized: Option<LegacyQuantizedBlockData>,
}

/// DEPRECATED: Use `proximablocks::ProximaBlockMetadata` instead
#[derive(Debug, Clone, Default)]
pub struct LegacyProximaBlockMetadata {
    pub min_id: String,
    pub max_id: String,
    pub min_timestamp: u64,
    pub max_timestamp: u64,
    pub record_count: usize,
    pub compressed_size: Option<usize>,
    pub uncompressed_size: usize,
    pub bloom_filter: Option<Vec<u8>>,
}

/// DEPRECATED: Use `proximablocks::QuantizedSection` instead
#[derive(Debug, Clone)]
pub struct LegacyQuantizedBlockData {
    pub binary_codes: Vec<Vec<u8>>,
    pub int8_codes: Vec<Vec<i8>>,
    pub pq_codes: Vec<Vec<u8>>,
    pub codebooks: HashMap<String, Vec<f32>>,
}

/// DEPRECATED: Use `proximablocks::BlockCompressionConfig` instead
#[derive(Debug, Clone)]
pub struct LegacyBlockCompressionConfig {
    pub algorithm: LegacyCompressionAlgorithm,
    pub level: u32,
    pub dictionary: Option<Vec<u8>>,
}

/// DEPRECATED: Use `core::compression::CompressionAlgorithm` instead
#[derive(Debug, Clone)]
pub enum LegacyCompressionAlgorithm {
    None,
    Lz4 { acceleration: i32 },
    Zstd { level: i32 },
    Snappy,
}

impl Default for LegacyBlockCompressionConfig {
    fn default() -> Self {
        LegacyBlockCompressionConfig {
            algorithm: LegacyCompressionAlgorithm::Lz4 { acceleration: 1 },
            level: 3,
            dictionary: None,
        }
    }
}

/// DEPRECATED: SSTable header - duplicate of `sst/mod.rs::SstableHeader`
/// This was a duplicate definition that was never exported
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LegacySstableHeader {
    /// SSTable format version
    pub version: u32,
    /// Number of records
    pub record_count: u64,
    /// Minimum key in this SSTable
    pub min_key: String,
    /// Maximum key in this SSTable
    pub max_key: String,
    /// Creation timestamp
    pub created_at: u64,
    /// LSM level
    pub level: u8,
    /// Size in bytes
    pub file_size: u64,
    /// Bloom filter data
    pub bloom_filter: Option<Vec<u8>>,
    /// Compression type
    pub compression_type: LegacyCompressionType,
}

/// DEPRECATED: Compression types enum - was exported but never used
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum LegacyCompressionType {
    None,
    #[default]
    Lz4,
    Zstd,
    Snappy,
}

/// DEPRECATED: SST record type - production code uses VectorRecord directly
///
/// This was the core record type for the SST engine, representing
/// a single vector entry with LSM-tree specific fields for ordering,
/// leveling, and tombstone markers.
///
/// Production code has been optimized to use VectorRecord directly,
/// eliminating the overhead of converting to/from SstRecord.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LegacySstRecord {
    /// Unique identifier
    pub id: String,
    /// Vector data
    pub vector: Option<Vec<f32>>,
    /// Metadata as JSON value
    pub metadata: Option<serde_json::Value>,
    /// Sequence number for ordering
    pub sequence_number: u64,
    /// LSM tree level
    pub level: u8,
    /// Deletion marker
    pub is_tombstone: bool,
    /// Timestamp
    pub timestamp: u64,
}
