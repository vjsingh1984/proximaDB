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

//! Schema management services
//!
//! This module provides services for managing ProximaRecord schemas, including:
//!
//! - Schema inference from existing VectorRecord metadata
//! - Type detection and pattern recognition
//! - TEXT column identification
//!
//! ## Schema Inference
//!
//! The `SchemaInferenceService` analyzes existing VectorRecord metadata to infer
//! appropriate column types for ProximaRecord. This includes:
//!
//! - Basic type inference (string, integer, float, boolean)
//! - Temporal pattern detection (timestamps, dates)
//! - UUID pattern detection
//! - Decimal/financial value detection
//! - TEXT column identification based on content length
//!
//! ## Usage Example
//!
//! ```rust,ignore
//! use proximadb::services::schema::{SchemaInferenceService, InferenceConfig};
//!
//! let config = InferenceConfig {
//!     sample_size: 1000,
//!     confidence_threshold: 0.8,
//!     detect_text_columns: true,
//!     text_length_threshold: 256,
//! };
//!
//! let service = SchemaInferenceService::new(config);
//! let schema = service.infer_schema(&vector_records);
//!
//! println!("Inferred {} columns with {:.1}% confidence",
//!     schema.columns.len(),
//!     schema.confidence * 100.0
//! );
//!
//! for col in schema.recommended_text_columns() {
//!     println!("Recommend TEXT storage for: {}", col);
//! }
//! ```

pub mod evolution;
pub mod inference;

pub use evolution::{
    CompatibilityIssue, CompatibilityLevel, CompatibilityResult, EvolutionConfig, EvolutionResult,
    IssueSeverity, MigrationEstimate, SchemaChange, SchemaEvolutionService, SchemaVersion,
    column_type_to_filterable,
};
pub use inference::{
    InferenceConfig, InferredColumn, InferredSchema, SchemaInferenceService, detect_boolean,
    detect_numeric_type, detect_timestamp, detect_uuid,
};
