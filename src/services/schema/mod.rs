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
//! This module provides services for managing cataloged `ProximaRecord` schemas, including:
//!
//! - Schema evolution checks
//! - Compatibility validation
//! - Migration estimates for cataloged schema changes
//!
//! ## Schema Evolution
//!
//! Schema inference from legacy vector metadata has been removed as an internal
//! service. Protocol handlers and SDKs must lower input through xCatalog and
//! `ProximaRecord`/`ProximaValue`; schema-on-write and schema-on-read behavior
//! belongs in catalog/type validation rather than a `VectorRecord` adapter.
//!
//! ## Usage Example
//!
//! ```rust,ignore
//! use proximadb::services::schema::{SchemaEvolutionService, EvolutionConfig};
//!
//! let service = SchemaEvolutionService::new(EvolutionConfig::default());
//! let result = service.analyze_change(&current_schema, &proposed_change)?;
//!
//! println!("Compatible: {}", result.compatible);
//! ```

pub mod evolution;

pub use evolution::{
    CompatibilityIssue, CompatibilityLevel, CompatibilityResult, EvolutionConfig, EvolutionResult,
    IssueSeverity, MigrationEstimate, SchemaChange, SchemaEvolutionService, SchemaVersion,
    column_type_to_filterable,
};
