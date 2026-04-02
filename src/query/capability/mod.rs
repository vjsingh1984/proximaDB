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

//! # Capability Registry Module
//!
//! This module provides ProximaDB's capability registry system that enables
//! query validation, planning, and API parity by tracking what features each
//! storage engine and query processor supports.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────┐
//! │     Storage Engine Registration          │
//! │  (SST, HELIX, VIPER, SWIFT, NOVA, RAPTOR)│
//! └────────────────┬────────────────────────┘
//!                  ↓
//! ┌─────────────────────────────────────────┐
//! │      Global Capability Registry          │
//! │         (Immutable, Shared)              │
//! └────────────────┬────────────────────────┘
//!                  ↓
//! ┌─────────────────────────────────────────┐
//! │      Plan Validation Layer               │
//! │  Query Requirements vs Available Caps    │
//! └────────────────┬────────────────────────┘
//!                  ↓
//! ┌─────────────────────────────────────────┐
//! │      Protocol Error Mapping              │
//! │  REST │ gRPC │ SQL │ UQL                 │
//! └─────────────────────────────────────────┘
//! ```
//!
//! ## Core Components
//!
//! ### 1. **Capability Enumeration**
//! Defines all system capabilities:
//! - **Query Types**: VectorSearch, GraphQuery, DocumentQuery, etc.
//! - **Operations**: Scan, Filter, Project, Join, Aggregate, Sort
//! - **Features**: Filtering, Quantization, WALRecovery, Replication
//! - **Index Types**: HNSW, IVF, Annoy, LSH
//!
//! ### 2. **CapabilitySet**
//! Efficient set operations for capability matching:
//! - `contains()`: Check if all required capabilities are present
//! - `intersects()`: Check if any capability overlaps
//! - `union()`, `difference()`: Set operations
//!
//! ### 3. **CapabilityRegistry**
//! Central registry for capability discovery:
//! - `register_capabilities()`: Add engine capabilities
//! - `get_capabilities()`: Query engine capabilities
//! - `check_support()`: Validate requirements
//!
//! ## Usage Example
//!
//! ```rust,ignore
//! use proximaDB::query::capability::{Capability, CapabilitySet, CapabilityRegistry};
//!
//! // Define required capabilities for a query
//! let required = CapabilitySet::new(&[
//!     Capability::VectorSearch,
//!     Capability::Filter,
//!     Capability::Quantization,
//! ]);
//!
//! // Check if storage engine supports required capabilities
//! let available = registry.get_capabilities("SST").unwrap();
//! if !available.contains(&required) {
//!     return Err(CapabilityError::UnsupportedCapability {
//!         capability: "VectorSearch with Quantization".to_string(),
//!         available_alternatives: vec!["VectorSearch without Quantization".to_string()],
//!     });
//! }
//! ```

pub mod registry;

pub use registry::{
    Capability, CapabilityCheckError, CapabilityRegistry, CapabilitySet,
};
