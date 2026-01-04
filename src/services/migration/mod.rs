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

//! Record migration services for VectorRecord to ProximaRecord transition
//!
//! This module provides services for migrating collections from the legacy VectorRecord
//! format to the new ProximaRecord format, supporting:
//!
//! - Background migration during compaction
//! - Dual-write mode for gradual rollout
//! - Rollback capabilities
//!
//! ## Migration Workflow
//!
//! ```text
//! 1. Start Migration (Legacy -> DualWrite)
//!    ┌──────────────────────────────────────────┐
//!    │ Collection: my_vectors                   │
//!    │ Mode: Legacy -> DualWrite                │
//!    │ Both VectorRecord and ProximaRecord      │
//!    └──────────────────────────────────────────┘
//!
//! 2. Verify Migration
//!    ┌──────────────────────────────────────────┐
//!    │ Validate all records converted correctly │
//!    │ Check data integrity                     │
//!    │ Monitor for errors                       │
//!    └──────────────────────────────────────────┘
//!
//! 3. Complete Migration (DualWrite -> Migrated)
//!    ┌──────────────────────────────────────────┐
//!    │ Collection: my_vectors                   │
//!    │ Mode: DualWrite -> Migrated              │
//!    │ Only ProximaRecord format                │
//!    └──────────────────────────────────────────┘
//! ```
//!
//! ## Usage Example
//!
//! ```rust,ignore
//! use proximadb::services::migration::{RecordMigrationService, MigrationConfig, MigrationMode};
//!
//! let config = MigrationConfig {
//!     batch_size: 1000,
//!     parallel_workers: 4,
//!     text_columns: vec!["content".to_string(), "description".to_string()],
//!     infer_schema: true,
//!     validate_on_migrate: true,
//! };
//!
//! let service = RecordMigrationService::new(config);
//!
//! // Start migration in dual-write mode
//! let stats = service.migrate_collection("my_collection", MigrationMode::DualWrite).await?;
//! println!("Migrated {} records", stats.migrated_records);
//! ```

pub mod record_migration;

pub use record_migration::{
    MigrationConfig, MigrationError, MigrationMode, MigrationResult, MigrationStats,
    MigrationStatus, RecordMigrationService, ValidationError, ValidationErrorCode,
    ValidationResult,
};
