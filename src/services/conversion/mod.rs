/*
 * Copyright 2025 ProximaDB
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

//! # Record Conversion Module
//!
//! Provides conversion utilities between VectorRecord (v1) and ProximaRecord (v2)
//! for backward compatibility and gradual migration.
//!
//! ## Architecture
//!
//! ```text
//! VectorRecord (v1)          ProximaRecord (v2)
//! ┌─────────────────┐        ┌─────────────────────┐
//! │ id              │ ←───→  │ id                  │
//! │ vector          │ ←───→  │ vector              │
//! │ metadata (Map)  │ ─────→ │ typed_fields (Map)  │
//! │                 │ ─────→ │ text_fields (Vec)   │
//! │ timestamp       │ ←───→  │ timestamp_ms        │
//! │ updated_at      │ ←───→  │ updated_at_ms       │
//! │ expires_at      │ ←───→  │ expires_at_ms       │
//! │ version         │ ←───→  │ version             │
//! │ source          │ ←───→  │ source              │
//! └─────────────────┘        └─────────────────────┘
//! ```
//!
//! ## Usage
//!
//! ```rust,ignore
//! use proximadb::services::conversion::RecordConverter;
//!
//! // Convert VectorRecord to ProximaRecord
//! let proxima = RecordConverter::vector_to_proxima(&vector_record, None, &["content"]);
//!
//! // Convert ProximaRecord back to VectorRecord
//! let vector = RecordConverter::proxima_to_vector(&proxima_record);
//! ```
//!
//! ## TEXT Column Extraction
//!
//! When converting VectorRecord → ProximaRecord, specified text columns are extracted
//! from the metadata map and stored as dedicated TextField entries. This enables:
//!
//! - Columnar storage in LargeUtf8 format
//! - Lazy loading (skip TEXT unless needed)
//! - N-gram bloom filters for CONTAINS queries
//! - Separate sidecar storage for large text
//!
//! ## Feature Flag Integration
//!
//! Collections can enable ProximaRecord via the `enable_proxima_record` flag:
//!
//! ```toml
//! [collection.my_vectors]
//! enable_proxima_record = true
//! text_columns = ["content", "description"]
//! ```

pub mod record_converter;

pub use record_converter::RecordConverter;
