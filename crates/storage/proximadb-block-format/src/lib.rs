// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! # proximadb-block-format
//!
//! PAX (Partition Attributes Across) block layout for ProximaDB internal storage.
//!
//! ## Design
//!
//! Every ProximaDB storage shard is composed of fixed-size PAX blocks.
//! A PAX block is the fundamental unit of:
//! * **durability** — WAL entries reference blocks by shard/offset
//! * **Iceberg manifest entries** — one block = one data-file descriptor
//! * **Arrow Flight tickets** — workers exchange tickets for specific blocks
//! * **HNSW index co-location** — filter + vector columns are leading stripes
//!
//! ## Block modes
//!
//! | Mode | Row directory | Column stripes | Use case |
//! |------|--------------|----------------|----------|
//! | OLTP | ✓            | ✗              | Small collections (< 1 GB), point lookups |
//! | OLAP | ✗            | ✓              | Bulk scan, Iceberg/Parquet projection |
//! | PAX  | ✓            | ✓              | Default — both OLTP and OLAP access patterns |
//!
//! ## Predicate pruning hierarchy
//!
//! 1. **Block-level**: `BlockHeader.tenant_id_hash` (engine-level RLS),
//!    `min/max_timestamp_ns` (time range), `collection_id_hash`.
//! 2. **Column-level**: `ColumnMeta.min_val/max_val` (predicate pushdown),
//!    `bloom_offset` (membership test for string predicates).
//! 3. **Row-level**: row directory MVCC bounds for snapshot reads (OLTP/PAX).
//!
//! ## Column stripe layout within a PAX block
//!
//! ```text
//! [BlockHeader: 64B]
//! [RowDirectory: N_rows × 32B]            ← OLTP / PAX only
//! [Stripe(id), Stripe(tenant_id), ...]    ← filter columns first (PAX co-location)
//! [Stripe(embedding_0), ...]              ← vector columns after
//! [Stripe(props), Stripe(labels), ...]    ← opaque blobs last
//! [ColumnMeta × N_cols: each 64B]
//! [BlockFooter: 32B]
//! ```
//!
//! ## Dependency constraints (workspace isolation)
//!
//! This crate depends ONLY on:
//! * `proximadb-codec` (encoding schemes — horizontal layer)
//! * `proximadb-data-model` (ProximaValue, ProximaType — foundation layer)
//! * `proximadb-records` (ProximaRecord — foundation layer)
//! * `anyhow`, `serde`, `crc32fast`, `rmp-serde` (external)
//!
//! It has NO dependency on the root `proximadb` crate, `tokio`, `axum`,
//! `tonic`, or any platform/runtime crate. This keeps rebuild times minimal
//! and the block format testable without spinning up a server.
//!
//! See: `docs/12-design/adr/ADR-010-pax-block-format.adoc`

#![forbid(unsafe_code)]

//! PAX Block Format
//!
//! **Architectural Mandate (2026-06-04):**
//! This crate implements the custom PAX (Partition Attributes Across) block format.
//! It is retained strictly as the specialized, high-performance spatial co-location layout
//! for **Vector Search and ANN workloads** (utilized by engines like SST, HELIX, NOVA, VIPER).
//!
//! For **Data Warehouse and Relational workloads**, this custom format is deprecated.
//! All relational DML and MPP analytics must utilize standard **Parquet** encodings
//! (via the `parquet` crate) backed by Iceberg manifests over decoupled Object Storage,
//! driven by the DataFusion execution engine.

pub mod header;
pub mod reader;
pub mod record;
pub mod row_dir;
pub mod stripe;
pub mod writer;

// ---- Top-level re-exports ----

pub use header::{
    BLOCK_MAGIC, BlockCompression, BlockHeader, BlockMode, FORMAT_VERSION, HEADER_SIZE, flags,
    fnv1a_hash,
};
pub use reader::PaxBlockReader;
pub use record::{
    ColumnDescriptor, FlatRow, canonical_columns, col_id, encode_f32_vec_col, encode_i64_col,
    encode_str_col, update_i64_bounds,
};
pub use row_dir::{ROW_ENTRY_SIZE, RowDirectory, RowEntry, row_flags};
pub use stripe::{BlockStats, COLUMN_META_SIZE, ColumnMeta, ColumnRole, ColumnStripe};
pub use writer::{BLOCK_FOOTER_SIZE, BlockFooter, PaxBlockWriter};
