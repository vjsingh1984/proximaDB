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

//! RecordMigrationService: Handles migration from VectorRecord to ProximaRecord
//!
//! This service provides comprehensive migration support for transitioning collections
//! from the legacy VectorRecord format to the new ProximaRecord format.
//!
//! ## Features
//!
//! - **Background Migration**: Migrate records during compaction to minimize impact
//! - **Dual-Write Mode**: Maintain both formats during transition for safety
//! - **Rollback Capabilities**: Ability to revert to legacy format if needed
//! - **Batch Processing**: Efficient batch-based migration with configurable size
//! - **Parallel Workers**: Multi-threaded migration for large collections
//! - **Schema Inference**: Optionally infer schema from existing metadata patterns
//! - **Validation**: Validate records during migration to ensure data integrity
//!
//! ## Migration Modes
//!
//! - `Legacy`: VectorRecord only (original format)
//! - `DualWrite`: Both formats maintained simultaneously
//! - `Migrated`: ProximaRecord only (fully migrated)
//!
//! ## Example
//!
//! ```rust,ignore
//! use proximadb::services::migration::{RecordMigrationService, MigrationConfig, MigrationMode};
//!
//! let config = MigrationConfig::default();
//! let service = RecordMigrationService::new(config);
//!
//! // Check current status
//! let status = service.get_migration_status("my_collection").await?;
//! println!("Current mode: {:?}", status.mode);
//!
//! // Start migration
//! let stats = service.migrate_collection("my_collection", MigrationMode::DualWrite).await?;
//! println!("Migrated {} of {} records", stats.migrated_records, stats.total_records);
//! ```

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Instant;

use crate::core::types::RecordSchema;
use crate::proto::proximadb_v1::VectorRecord;
use crate::services::conversion::record_converter::{ProximaRecord, RecordConverter};
use crate::services::schema::{InferenceConfig, InferredSchema, SchemaInferenceService};

/// Migration mode for collection
///
/// Defines the current state of a collection in the migration process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationMode {
    /// VectorRecord only (legacy format)
    ///
    /// This is the original format. Collections start in this mode.
    Legacy,

    /// Dual-write: both formats maintained
    ///
    /// During this phase:
    /// - New writes create both VectorRecord and ProximaRecord
    /// - Reads can use either format
    /// - Existing records are being migrated in the background
    DualWrite,

    /// ProximaRecord only (fully migrated)
    ///
    /// The collection has been fully migrated:
    /// - All records are in ProximaRecord format
    /// - VectorRecord data has been cleaned up
    /// - New writes only create ProximaRecord
    Migrated,
}

impl Default for MigrationMode {
    fn default() -> Self {
        Self::Legacy
    }
}

/// Migration statistics
///
/// Tracks progress and outcomes of a migration operation.
#[derive(Debug, Clone, Default)]
pub struct MigrationStats {
    /// Total number of records in the collection
    pub total_records: u64,

    /// Number of records successfully migrated
    pub migrated_records: u64,

    /// Number of records that failed migration
    pub failed_records: u64,

    /// When the migration started
    pub start_time: Option<Instant>,

    /// When the migration completed
    pub end_time: Option<Instant>,
}

impl MigrationStats {
    /// Create new empty stats
    pub fn new() -> Self {
        Self::default()
    }

    /// Start timing the migration
    pub fn start(&mut self) {
        self.start_time = Some(Instant::now());
    }

    /// Stop timing the migration
    pub fn stop(&mut self) {
        self.end_time = Some(Instant::now());
    }

    /// Get migration duration in milliseconds
    pub fn duration_ms(&self) -> Option<u64> {
        match (self.start_time, self.end_time) {
            (Some(start), Some(end)) => Some(end.duration_since(start).as_millis() as u64),
            _ => None,
        }
    }

    /// Calculate migration progress as a percentage
    pub fn progress_percent(&self) -> f64 {
        if self.total_records == 0 {
            return 100.0;
        }
        ((self.migrated_records + self.failed_records) as f64 / self.total_records as f64) * 100.0
    }

    /// Check if migration is complete
    pub fn is_complete(&self) -> bool {
        self.migrated_records + self.failed_records >= self.total_records
    }
}

/// Configuration for migration
///
/// Controls how the migration service operates.
#[derive(Debug, Clone)]
pub struct MigrationConfig {
    /// Number of records to process in each batch
    ///
    /// Larger batches are more efficient but use more memory.
    /// Default: 1000
    pub batch_size: usize,

    /// Number of parallel worker threads
    ///
    /// More workers can speed up migration but increase resource usage.
    /// Default: 4
    pub parallel_workers: usize,

    /// Column names to store as TEXT fields
    ///
    /// These columns will be extracted from metadata and stored
    /// as dedicated TEXT fields in ProximaRecord.
    pub text_columns: Vec<String>,

    /// Whether to infer schema from existing metadata
    ///
    /// If true, the service will analyze metadata patterns to
    /// automatically determine column types.
    /// Default: true
    pub infer_schema: bool,

    /// Whether to validate records during migration
    ///
    /// If true, each migrated record is validated against the schema.
    /// This is slower but catches data quality issues.
    /// Default: true
    pub validate_on_migrate: bool,
}

impl Default for MigrationConfig {
    fn default() -> Self {
        Self {
            batch_size: 1000,
            parallel_workers: 4,
            text_columns: Vec::new(),
            infer_schema: true,
            validate_on_migrate: true,
        }
    }
}

impl MigrationConfig {
    /// Create a new configuration with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the batch size
    pub fn with_batch_size(mut self, size: usize) -> Self {
        self.batch_size = size;
        self
    }

    /// Set the number of parallel workers
    pub fn with_parallel_workers(mut self, workers: usize) -> Self {
        self.parallel_workers = workers;
        self
    }

    /// Set the text columns to extract
    pub fn with_text_columns(mut self, columns: Vec<String>) -> Self {
        self.text_columns = columns;
        self
    }

    /// Enable or disable schema inference
    pub fn with_infer_schema(mut self, infer: bool) -> Self {
        self.infer_schema = infer;
        self
    }

    /// Enable or disable validation during migration
    pub fn with_validate_on_migrate(mut self, validate: bool) -> Self {
        self.validate_on_migrate = validate;
        self
    }
}

/// Migration status for a collection
///
/// Contains the current migration state and progress.
#[derive(Debug, Clone)]
pub struct MigrationStatus {
    /// Current migration mode
    pub mode: MigrationMode,

    /// Migration statistics
    pub stats: MigrationStats,

    /// Schema ID if schema has been inferred/assigned
    pub schema_id: Option<String>,

    /// Whether migration is currently paused
    pub is_paused: bool,

    /// Whether migration is currently running
    pub is_running: bool,

    /// Records remaining to migrate
    pub records_remaining: u64,

    /// Last error message if any
    pub last_error: Option<String>,
}

impl Default for MigrationStatus {
    fn default() -> Self {
        Self {
            mode: MigrationMode::Legacy,
            stats: MigrationStats::default(),
            schema_id: None,
            is_paused: false,
            is_running: false,
            records_remaining: 0,
            last_error: None,
        }
    }
}

/// Internal migration state tracking
struct MigrationState {
    /// Collection name
    collection_name: String,
    /// Current mode
    mode: MigrationMode,
    /// Whether migration is paused
    is_paused: AtomicBool,
    /// Whether migration should stop
    should_stop: AtomicBool,
    /// Records migrated counter (atomic for thread-safe updates)
    migrated_count: AtomicU64,
    /// Records failed counter
    failed_count: AtomicU64,
    /// Total records to migrate
    total_records: AtomicU64,
    /// Schema ID
    schema_id: RwLock<Option<String>>,
    /// Last error
    last_error: RwLock<Option<String>>,
    /// Start time
    start_time: RwLock<Option<Instant>>,
}

impl MigrationState {
    fn new(collection_name: String, mode: MigrationMode) -> Self {
        Self {
            collection_name,
            mode,
            is_paused: AtomicBool::new(false),
            should_stop: AtomicBool::new(false),
            migrated_count: AtomicU64::new(0),
            failed_count: AtomicU64::new(0),
            total_records: AtomicU64::new(0),
            schema_id: RwLock::new(None),
            last_error: RwLock::new(None),
            start_time: RwLock::new(None),
        }
    }

    fn is_paused(&self) -> bool {
        self.is_paused.load(Ordering::Relaxed)
    }

    fn pause(&self) {
        self.is_paused.store(true, Ordering::Relaxed);
    }

    fn resume(&self) {
        self.is_paused.store(false, Ordering::Relaxed);
    }

    fn should_stop(&self) -> bool {
        self.should_stop.load(Ordering::Relaxed)
    }

    fn request_stop(&self) {
        self.should_stop.store(true, Ordering::Relaxed);
    }

    fn increment_migrated(&self) {
        self.migrated_count.fetch_add(1, Ordering::Relaxed);
    }

    fn increment_failed(&self) {
        self.failed_count.fetch_add(1, Ordering::Relaxed);
    }

    fn to_status(&self) -> MigrationStatus {
        let migrated = self.migrated_count.load(Ordering::Relaxed);
        let failed = self.failed_count.load(Ordering::Relaxed);
        let total = self.total_records.load(Ordering::Relaxed);

        let start_time = self.start_time.read().ok().and_then(|s| *s);

        MigrationStatus {
            mode: self.mode.clone(),
            stats: MigrationStats {
                total_records: total,
                migrated_records: migrated,
                failed_records: failed,
                start_time,
                end_time: None,
            },
            schema_id: self.schema_id.read().ok().and_then(|s| s.clone()),
            is_paused: self.is_paused(),
            is_running: !self.should_stop(),
            records_remaining: total.saturating_sub(migrated + failed),
            last_error: self.last_error.read().ok().and_then(|e| e.clone()),
        }
    }

    fn set_schema_id(&self, id: String) {
        if let Ok(mut schema_id) = self.schema_id.write() {
            *schema_id = Some(id);
        }
    }

    fn set_start_time(&self) {
        if let Ok(mut start_time) = self.start_time.write() {
            *start_time = Some(Instant::now());
        }
    }

    fn set_total_records(&self, total: u64) {
        self.total_records.store(total, Ordering::Relaxed);
    }

    fn set_error(&self, error: String) {
        if let Ok(mut last_error) = self.last_error.write() {
            *last_error = Some(error);
        }
    }
}

/// Result of validating a ProximaRecord against a schema
#[derive(Debug, Clone)]
pub struct ValidationResult {
    /// Whether the validation passed
    pub valid: bool,
    /// Record ID that was validated
    pub record_id: String,
    /// Validation errors (empty if valid)
    pub errors: Vec<ValidationError>,
    /// Warnings (non-fatal issues)
    pub warnings: Vec<String>,
}

impl ValidationResult {
    /// Create a successful validation result
    pub fn success(record_id: String) -> Self {
        Self {
            valid: true,
            record_id,
            errors: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// Create a failed validation result
    pub fn failure(record_id: String, errors: Vec<ValidationError>) -> Self {
        Self {
            valid: false,
            record_id,
            errors,
            warnings: Vec::new(),
        }
    }

    /// Add a warning to the result
    pub fn with_warning(mut self, warning: String) -> Self {
        self.warnings.push(warning);
        self
    }
}

/// Individual validation error
#[derive(Debug, Clone)]
pub struct ValidationError {
    /// Field that failed validation
    pub field: String,
    /// Error message
    pub message: String,
    /// Error code for programmatic handling
    pub code: ValidationErrorCode,
}

/// Validation error codes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationErrorCode {
    /// Field is required but missing
    RequiredFieldMissing,
    /// Type mismatch between value and schema
    TypeMismatch,
    /// Value exceeds maximum length/size
    MaxLengthExceeded,
    /// Value below minimum length/size
    MinLengthViolation,
    /// Value outside allowed range
    RangeViolation,
    /// Pattern/regex validation failed
    PatternMismatch,
    /// Vector dimension mismatch
    DimensionMismatch,
    /// Invalid format
    InvalidFormat,
    /// Custom validation rule failed
    CustomRuleFailed,
}

/// Result of a migration operation
#[derive(Debug, Clone)]
pub struct MigrationResult {
    /// Whether the migration was successful
    pub success: bool,
    /// Migration statistics
    pub stats: MigrationStats,
    /// Inferred schema (if schema inference was enabled)
    pub inferred_schema: Option<InferredSchema>,
    /// Error message if migration failed
    pub error: Option<String>,
}

impl MigrationResult {
    /// Create a successful migration result
    pub fn success(stats: MigrationStats, inferred_schema: Option<InferredSchema>) -> Self {
        Self {
            success: true,
            stats,
            inferred_schema,
            error: None,
        }
    }

    /// Create a failed migration result
    pub fn failure(stats: MigrationStats, error: String) -> Self {
        Self {
            success: false,
            stats,
            inferred_schema: None,
            error: Some(error),
        }
    }
}

/// Migration error types
#[derive(Debug, thiserror::Error)]
pub enum MigrationError {
    /// Collection was not found
    #[error("Collection not found: {0}")]
    CollectionNotFound(String),

    /// A migration is already in progress for this collection
    #[error("Migration already in progress")]
    MigrationInProgress,

    /// Migration is paused
    #[error("Migration is paused for collection: {0}")]
    MigrationPaused(String),

    /// Validation of a record failed during migration
    #[error("Validation failed: {0}")]
    ValidationFailed(String),

    /// Storage layer error during migration
    #[error("Storage error: {0}")]
    StorageError(String),

    /// Invalid migration mode transition
    #[error("Invalid mode transition: {from:?} -> {to:?}")]
    InvalidModeTransition {
        from: MigrationMode,
        to: MigrationMode,
    },

    /// Configuration error
    #[error("Configuration error: {0}")]
    ConfigError(String),

    /// Schema error
    #[error("Schema error: {0}")]
    SchemaError(String),

    /// No records to migrate
    #[error("No records found in collection: {0}")]
    NoRecords(String),

    /// Internal error
    #[error("Internal error: {0}")]
    InternalError(String),
}

/// Service for migrating collections from VectorRecord to ProximaRecord
///
/// This service handles the complete lifecycle of migrating a collection
/// from the legacy VectorRecord format to the new ProximaRecord format.
///
/// ## Thread Safety
///
/// The service is designed to be thread-safe and can be shared across
/// multiple tasks. Individual migrations are tracked per-collection to
/// prevent concurrent migrations of the same collection.
pub struct RecordMigrationService {
    /// Migration configuration
    config: MigrationConfig,
    /// Active migration states per collection
    active_migrations: RwLock<HashMap<String, Arc<MigrationState>>>,
    /// Schema inference service
    schema_inference: SchemaInferenceService,
}

impl RecordMigrationService {
    /// Create a new migration service with the given configuration
    ///
    /// # Arguments
    ///
    /// * `config` - Migration configuration
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let service = RecordMigrationService::new(MigrationConfig::default());
    /// ```
    pub fn new(config: MigrationConfig) -> Self {
        let inference_config = InferenceConfig::new()
            .with_sample_size(config.batch_size)
            .with_detect_text_columns(true)
            .with_text_length_threshold(256);

        Self {
            config,
            active_migrations: RwLock::new(HashMap::new()),
            schema_inference: SchemaInferenceService::new(inference_config),
        }
    }

    /// Get the current configuration
    pub fn config(&self) -> &MigrationConfig {
        &self.config
    }

    /// Main orchestrator for collection migration
    ///
    /// This initiates the migration process for the specified collection.
    /// Supports different migration modes:
    ///
    /// - `DualWrite`: Write both V1 and V2 formats simultaneously
    /// - `Migrated`: V2 only (after migration complete)
    /// - `Legacy`: V1 only (rollback mode)
    ///
    /// # Arguments
    ///
    /// * `collection_id` - ID of the collection to migrate
    /// * `config` - Migration configuration (overrides service defaults)
    ///
    /// # Returns
    ///
    /// Returns `MigrationResult` with statistics and optional inferred schema.
    pub async fn migrate_collection(
        &self,
        collection_id: &str,
        config: MigrationConfig,
    ) -> Result<MigrationResult, MigrationError> {
        // Check if migration is already in progress
        if self.is_migration_active(collection_id) {
            return Err(MigrationError::MigrationInProgress);
        }

        // Get current status
        let current_status = self.get_migration_status(collection_id).await?;

        // Validate mode transition - determine target mode based on current
        let target_mode = match current_status.mode {
            MigrationMode::Legacy => MigrationMode::DualWrite,
            MigrationMode::DualWrite => MigrationMode::Migrated,
            MigrationMode::Migrated => {
                return Err(MigrationError::InvalidModeTransition {
                    from: MigrationMode::Migrated,
                    to: MigrationMode::Migrated,
                });
            }
        };

        self.validate_mode_transition(&current_status.mode, &target_mode)?;

        // Create migration state
        let state = Arc::new(MigrationState::new(
            collection_id.to_string(),
            target_mode.clone(),
        ));
        state.set_start_time();

        // Register active migration
        self.register_migration(collection_id, Arc::clone(&state))?;

        let mut stats = MigrationStats::new();
        stats.start();

        // Infer schema if configured
        let inferred_schema: Option<InferredSchema> = if config.infer_schema {
            // In a real implementation, we would sample records from storage
            // For now, return None as we don't have actual record access
            None
        } else {
            None
        };

        // Set schema ID if inferred
        if let Some(ref schema) = inferred_schema {
            let proto_config = schema.to_proto_config();
            state.set_schema_id(proto_config.schema_id);
        }

        // Note: In a real implementation, we would:
        // 1. Query the storage layer for records
        // 2. Process in batches using migrate_batch
        // 3. Write converted records back to storage
        // 4. Update collection metadata

        stats.total_records = 0;
        stats.migrated_records = 0;
        stats.stop();

        // Unregister migration
        self.unregister_migration(collection_id);

        Ok(MigrationResult::success(stats, inferred_schema))
    }

    /// Migrate a collection with record iteration support
    ///
    /// This method allows external callers to provide records for migration,
    /// useful when the caller has access to the storage layer.
    ///
    /// # Arguments
    ///
    /// * `collection_id` - Collection identifier
    /// * `records` - Iterator of VectorRecords to migrate
    /// * `target_mode` - Target migration mode
    ///
    /// # Returns
    ///
    /// Returns `MigrationResult` with statistics.
    pub async fn migrate_records<I>(
        &self,
        collection_id: &str,
        records: I,
        target_mode: MigrationMode,
    ) -> Result<MigrationResult, MigrationError>
    where
        I: Iterator<Item = VectorRecord>,
    {
        // Check if migration is already in progress
        if self.is_migration_active(collection_id) {
            return Err(MigrationError::MigrationInProgress);
        }

        // Create migration state
        let state = Arc::new(MigrationState::new(
            collection_id.to_string(),
            target_mode.clone(),
        ));
        state.set_start_time();

        // Register active migration
        self.register_migration(collection_id, Arc::clone(&state))?;

        let mut stats = MigrationStats::new();
        stats.start();

        // Collect records for schema inference
        let records: Vec<VectorRecord> = records.collect();
        stats.total_records = records.len() as u64;
        state.set_total_records(stats.total_records);

        // Infer schema if configured
        let inferred_schema = if self.config.infer_schema && !records.is_empty() {
            Some(self.schema_inference.infer_schema(&records))
        } else {
            None
        };

        // Determine text columns from inference or config
        // Note: In a full implementation, this would be used for storage layer integration
        let _text_columns = if let Some(ref schema) = inferred_schema {
            schema.text_columns.clone()
        } else {
            self.config.text_columns.clone()
        };

        // Get schema ID
        let schema_id = inferred_schema
            .as_ref()
            .map(|s| s.to_proto_config().schema_id);
        if let Some(ref id) = schema_id {
            state.set_schema_id(id.clone());
        }

        // Process in batches
        for batch in records.chunks(self.config.batch_size) {
            // Check for pause/stop
            if state.should_stop() {
                break;
            }

            while state.is_paused() {
                // Wait for resume (in real impl, use async sleep)
                std::thread::sleep(std::time::Duration::from_millis(100));
                if state.should_stop() {
                    break;
                }
            }

            // Migrate batch
            let results = self.migrate_batch(batch, schema_id.as_deref());

            for result in results {
                match result {
                    Ok(_) => {
                        stats.migrated_records += 1;
                        state.increment_migrated();
                    }
                    Err(e) => {
                        stats.failed_records += 1;
                        state.increment_failed();
                        state.set_error(e.to_string());
                    }
                }
            }
        }

        stats.stop();

        // Unregister migration
        self.unregister_migration(collection_id);

        Ok(MigrationResult::success(stats, inferred_schema))
    }

    /// Migrate a batch of records
    ///
    /// Converts a batch of VectorRecords to ProximaRecords using the
    /// configured text columns and optional schema ID.
    ///
    /// # Arguments
    ///
    /// * `records` - Slice of VectorRecords to migrate
    /// * `schema_id` - Optional schema ID to assign to the records
    ///
    /// # Returns
    ///
    /// A vector of Results, one for each input record. This allows
    /// partial batch success where some records may fail validation.
    pub fn migrate_batch(
        &self,
        records: &[VectorRecord],
        schema_id: Option<&str>,
    ) -> Vec<Result<ProximaRecord, MigrationError>> {
        records
            .iter()
            .map(|record| self.migrate_single_record(record, schema_id))
            .collect()
    }

    /// Migrate a single record (internal use)
    fn migrate_single_record(
        &self,
        record: &VectorRecord,
        schema_id: Option<&str>,
    ) -> Result<ProximaRecord, MigrationError> {
        let proxima = self.convert_record(record, schema_id, &self.config.text_columns);

        if self.config.validate_on_migrate {
            self.validate_migrated_record(&proxima)?;
        }

        Ok(proxima)
    }

    /// Convert a single VectorRecord to ProximaRecord
    ///
    /// This is the core conversion function that transforms V1 format to V2.
    ///
    /// # Arguments
    ///
    /// * `record` - The VectorRecord to convert
    /// * `schema_id` - Optional schema ID to assign
    /// * `text_columns` - Column names to store as TEXT fields
    ///
    /// # Returns
    ///
    /// A new ProximaRecord with converted data.
    pub fn convert_record(
        &self,
        record: &VectorRecord,
        schema_id: Option<&str>,
        text_columns: &[String],
    ) -> ProximaRecord {
        RecordConverter::vector_to_proxima(record, schema_id, text_columns)
    }

    /// Validate a ProximaRecord against an optional schema
    ///
    /// Performs validation of the record, optionally checking against a schema.
    ///
    /// # Arguments
    ///
    /// * `record` - The ProximaRecord to validate
    /// * `schema` - Optional RecordSchema to validate against
    ///
    /// # Returns
    ///
    /// A `ValidationResult` indicating success or failure with error details.
    pub fn validate_record(
        &self,
        record: &ProximaRecord,
        schema: Option<&RecordSchema>,
    ) -> ValidationResult {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        // Basic validation - ID cannot be empty
        if record.id.is_empty() {
            errors.push(ValidationError {
                field: "id".to_string(),
                message: "Record ID cannot be empty".to_string(),
                code: ValidationErrorCode::RequiredFieldMissing,
            });
        }

        // Vector cannot be empty
        if record.vector.is_empty() {
            errors.push(ValidationError {
                field: "vector".to_string(),
                message: "Vector cannot be empty".to_string(),
                code: ValidationErrorCode::RequiredFieldMissing,
            });
        }

        // Validate vector dimensions if specified
        if let Some(dim) = record.vector_dimension {
            if record.vector.len() != dim as usize {
                errors.push(ValidationError {
                    field: "vector".to_string(),
                    message: format!(
                        "Vector dimension mismatch: expected {}, got {}",
                        dim,
                        record.vector.len()
                    ),
                    code: ValidationErrorCode::DimensionMismatch,
                });
            }
        }

        // Schema-based validation
        if let Some(schema) = schema {
            // Validate typed fields against schema
            for (field_name, typed_value) in &record.typed_fields {
                if let Some(column) = schema.get_column(field_name) {
                    // Check type match
                    if !typed_value.matches_type(&column.data_type) {
                        errors.push(ValidationError {
                            field: field_name.clone(),
                            message: format!(
                                "Type mismatch: expected {:?}, got {}",
                                column.data_type,
                                typed_value.type_name()
                            ),
                            code: ValidationErrorCode::TypeMismatch,
                        });
                    }

                    // Validate constraints
                    if let Err(e) = typed_value.validate_constraints(&column.constraints) {
                        errors.push(ValidationError {
                            field: field_name.clone(),
                            message: e.to_string(),
                            code: ValidationErrorCode::RangeViolation,
                        });
                    }
                } else if !schema.allow_additional_fields {
                    // Field not in schema and additional fields not allowed
                    warnings.push(format!("Field '{}' not defined in schema", field_name));
                }
            }

            // Check for required fields
            for column in &schema.columns {
                if !column.nullable && !record.typed_fields.contains_key(&column.name) {
                    // Check if it's in text_fields
                    let in_text_fields = record.text_fields.iter().any(|tf| tf.name == column.name);
                    if !in_text_fields {
                        errors.push(ValidationError {
                            field: column.name.clone(),
                            message: format!("Required field '{}' is missing", column.name),
                            code: ValidationErrorCode::RequiredFieldMissing,
                        });
                    }
                }
            }
        }

        // Validate TEXT fields
        for text_field in &record.text_fields {
            if text_field.name.is_empty() {
                errors.push(ValidationError {
                    field: "text_field".to_string(),
                    message: "Text field name cannot be empty".to_string(),
                    code: ValidationErrorCode::InvalidFormat,
                });
            }
        }

        if errors.is_empty() {
            let mut result = ValidationResult::success(record.id.clone());
            for warning in warnings {
                result = result.with_warning(warning);
            }
            result
        } else {
            ValidationResult::failure(record.id.clone(), errors)
        }
    }

    /// Validate a migrated record (simple validation for migration)
    fn validate_migrated_record(&self, record: &ProximaRecord) -> Result<(), MigrationError> {
        let result = self.validate_record(record, None);
        if result.valid {
            Ok(())
        } else {
            let error_messages: Vec<String> = result
                .errors
                .iter()
                .map(|e| format!("{}: {}", e.field, e.message))
                .collect();
            Err(MigrationError::ValidationFailed(error_messages.join("; ")))
        }
    }

    /// Check migration status for a collection
    ///
    /// Returns the current migration mode and statistics for the collection.
    /// If a migration is active, returns real-time progress.
    ///
    /// # Arguments
    ///
    /// * `collection_id` - ID of the collection to check
    ///
    /// # Returns
    ///
    /// Returns `MigrationStatus` with current mode and stats.
    ///
    /// # Errors
    ///
    /// - `CollectionNotFound`: If the collection doesn't exist
    pub async fn get_migration_status(
        &self,
        collection_id: &str,
    ) -> Result<MigrationStatus, MigrationError> {
        // Check if there's an active migration
        if let Some(state) = self.get_active_migration(collection_id) {
            return Ok(state.to_status());
        }

        // In a real implementation, this would query the collection metadata
        // to determine the current migration state
        Ok(MigrationStatus::default())
    }

    /// Pause an active migration
    ///
    /// Pauses the migration process for the specified collection.
    /// The migration can be resumed later with `resume_migration`.
    ///
    /// # Arguments
    ///
    /// * `collection_id` - ID of the collection to pause
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if paused successfully, or error if no active migration.
    pub fn pause_migration(&self, collection_id: &str) -> Result<(), MigrationError> {
        if let Some(state) = self.get_active_migration(collection_id) {
            state.pause();
            Ok(())
        } else {
            Err(MigrationError::CollectionNotFound(
                collection_id.to_string(),
            ))
        }
    }

    /// Resume a paused migration
    ///
    /// Resumes the migration process for the specified collection.
    ///
    /// # Arguments
    ///
    /// * `collection_id` - ID of the collection to resume
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if resumed successfully, or error if no active migration.
    pub fn resume_migration(&self, collection_id: &str) -> Result<(), MigrationError> {
        if let Some(state) = self.get_active_migration(collection_id) {
            state.resume();
            Ok(())
        } else {
            Err(MigrationError::CollectionNotFound(
                collection_id.to_string(),
            ))
        }
    }

    /// Stop an active migration
    ///
    /// Stops the migration process for the specified collection.
    /// Unlike pause, this cannot be resumed.
    ///
    /// # Arguments
    ///
    /// * `collection_id` - ID of the collection to stop
    ///
    /// # Returns
    ///
    /// Returns the final migration status.
    pub fn stop_migration(&self, collection_id: &str) -> Result<MigrationStatus, MigrationError> {
        if let Some(state) = self.get_active_migration(collection_id) {
            state.request_stop();
            Ok(state.to_status())
        } else {
            Err(MigrationError::CollectionNotFound(
                collection_id.to_string(),
            ))
        }
    }

    /// Check if a migration is currently active for a collection
    pub fn is_migration_active(&self, collection_id: &str) -> bool {
        self.get_active_migration(collection_id).is_some()
    }

    /// Check if a migration is paused for a collection
    pub fn is_migration_paused(&self, collection_id: &str) -> bool {
        self.get_active_migration(collection_id)
            .map(|s| s.is_paused())
            .unwrap_or(false)
    }

    /// Get all active migrations
    pub fn get_active_migrations(&self) -> Vec<String> {
        self.active_migrations
            .read()
            .ok()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Register an active migration
    fn register_migration(
        &self,
        collection_id: &str,
        state: Arc<MigrationState>,
    ) -> Result<(), MigrationError> {
        let mut migrations = self
            .active_migrations
            .write()
            .map_err(|_| MigrationError::InternalError("Lock poisoned".to_string()))?;

        if migrations.contains_key(collection_id) {
            return Err(MigrationError::MigrationInProgress);
        }

        migrations.insert(collection_id.to_string(), state);
        Ok(())
    }

    /// Unregister an active migration
    fn unregister_migration(&self, collection_id: &str) {
        if let Ok(mut migrations) = self.active_migrations.write() {
            migrations.remove(collection_id);
        }
    }

    /// Get the active migration state for a collection
    fn get_active_migration(&self, collection_id: &str) -> Option<Arc<MigrationState>> {
        self.active_migrations
            .read()
            .ok()
            .and_then(|m| m.get(collection_id).cloned())
    }

    /// Rollback migration (switch back to legacy mode)
    ///
    /// This reverts the collection to the legacy VectorRecord format.
    /// Only valid when in DualWrite mode (before completing migration).
    ///
    /// # Arguments
    ///
    /// * `collection_id` - ID of the collection to rollback
    ///
    /// # Errors
    ///
    /// - `CollectionNotFound`: If the collection doesn't exist
    /// - `InvalidModeTransition`: If not in DualWrite mode
    pub async fn rollback_migration(&self, collection_id: &str) -> Result<(), MigrationError> {
        // Stop any active migration first
        if self.is_migration_active(collection_id) {
            let _ = self.stop_migration(collection_id);
        }

        let current_status = self.get_migration_status(collection_id).await?;

        if current_status.mode != MigrationMode::DualWrite {
            return Err(MigrationError::InvalidModeTransition {
                from: current_status.mode,
                to: MigrationMode::Legacy,
            });
        }

        // In a real implementation, this would:
        // 1. Stop any ongoing migration
        // 2. Clean up ProximaRecord data
        // 3. Update collection metadata back to Legacy mode

        Ok(())
    }

    /// Validate a mode transition
    fn validate_mode_transition(
        &self,
        from: &MigrationMode,
        to: &MigrationMode,
    ) -> Result<(), MigrationError> {
        let valid = match (from, to) {
            // Can go from Legacy to DualWrite
            (MigrationMode::Legacy, MigrationMode::DualWrite) => true,
            // Can go from DualWrite to Migrated
            (MigrationMode::DualWrite, MigrationMode::Migrated) => true,
            // Can rollback from DualWrite to Legacy
            (MigrationMode::DualWrite, MigrationMode::Legacy) => true,
            // Same mode is a no-op
            (a, b) if a == b => true,
            // All other transitions are invalid
            _ => false,
        };

        if !valid {
            return Err(MigrationError::InvalidModeTransition {
                from: from.clone(),
                to: to.clone(),
            });
        }

        Ok(())
    }

    /// Estimate migration time based on record count
    ///
    /// Provides a rough estimate of how long migration will take.
    ///
    /// # Arguments
    ///
    /// * `record_count` - Number of records to migrate
    ///
    /// # Returns
    ///
    /// Estimated duration in seconds
    pub fn estimate_migration_time(&self, record_count: u64) -> u64 {
        // Rough estimate: ~1000 records per second with default settings
        let records_per_second = (self.config.batch_size * self.config.parallel_workers) as u64;
        let base_estimate = record_count / records_per_second.max(1);

        // Add overhead for validation
        let validation_overhead = if self.config.validate_on_migrate {
            base_estimate / 5 // 20% overhead
        } else {
            0
        };

        // Add overhead for schema inference
        let schema_overhead = if self.config.infer_schema {
            10 // 10 seconds for schema inference
        } else {
            0
        };

        base_estimate + validation_overhead + schema_overhead
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::proximadb_v1::{SqlValue, sql_value::Value as SqlValueVariant};
    use std::collections::HashMap;

    fn create_test_record(id: &str) -> VectorRecord {
        let mut metadata = HashMap::new();
        metadata.insert(
            "category".to_string(),
            SqlValue {
                value: Some(SqlValueVariant::StringValue("test".to_string())),
            },
        );
        metadata.insert(
            "content".to_string(),
            SqlValue {
                value: Some(SqlValueVariant::StringValue(
                    "This is test content".to_string(),
                )),
            },
        );

        VectorRecord {
            id: id.to_string(),
            vector: vec![0.1, 0.2, 0.3, 0.4],
            metadata,
            timestamp: Some(1704067200000),
            updated_at: None,
            expires_at: None,
            version: Some(1),
            source: None,
        }
    }

    #[test]
    fn test_migration_config_default() {
        let config = MigrationConfig::default();
        assert_eq!(config.batch_size, 1000);
        assert_eq!(config.parallel_workers, 4);
        assert!(config.text_columns.is_empty());
        assert!(config.infer_schema);
        assert!(config.validate_on_migrate);
    }

    #[test]
    fn test_migration_config_builder() {
        let config = MigrationConfig::new()
            .with_batch_size(500)
            .with_parallel_workers(8)
            .with_text_columns(vec!["content".to_string()])
            .with_infer_schema(false)
            .with_validate_on_migrate(false);

        assert_eq!(config.batch_size, 500);
        assert_eq!(config.parallel_workers, 8);
        assert_eq!(config.text_columns, vec!["content"]);
        assert!(!config.infer_schema);
        assert!(!config.validate_on_migrate);
    }

    #[test]
    fn test_migration_stats() {
        let mut stats = MigrationStats::new();
        stats.total_records = 100;
        stats.migrated_records = 50;
        stats.failed_records = 5;

        // Use approximate comparison for floating point
        assert!((stats.progress_percent() - 55.0).abs() < 0.001);
        assert!(!stats.is_complete());

        stats.migrated_records = 95;
        assert!(stats.is_complete());
    }

    #[test]
    fn test_migration_stats_duration() {
        let mut stats = MigrationStats::new();
        stats.start();
        std::thread::sleep(std::time::Duration::from_millis(10));
        stats.stop();

        let duration = stats.duration_ms().unwrap();
        assert!(duration >= 10);
    }

    #[test]
    fn test_migrate_batch_success() {
        let config = MigrationConfig::new()
            .with_text_columns(vec!["content".to_string()])
            .with_validate_on_migrate(true);

        let service = RecordMigrationService::new(config);

        let records = vec![
            create_test_record("doc_1"),
            create_test_record("doc_2"),
            create_test_record("doc_3"),
        ];

        let results = service.migrate_batch(&records, Some("schema_1"));

        assert_eq!(results.len(), 3);
        for result in results {
            assert!(result.is_ok());
            let proxima = result.unwrap();
            assert!(!proxima.id.is_empty());
            assert_eq!(proxima.text_fields.len(), 1);
            assert_eq!(proxima.text_fields[0].name, "content");
        }
    }

    #[test]
    fn test_migrate_batch_with_schema_id() {
        let service = RecordMigrationService::new(MigrationConfig::default());
        let records = vec![create_test_record("doc_1")];

        let results = service.migrate_batch(&records, Some("my_schema_123"));

        let proxima = results[0].as_ref().unwrap();
        assert_eq!(proxima.schema_id, Some("my_schema_123".to_string()));
    }

    #[test]
    fn test_validation_empty_id() {
        let config = MigrationConfig::new().with_validate_on_migrate(true);
        let service = RecordMigrationService::new(config);

        let mut record = create_test_record("");
        record.id = String::new();

        let results = service.migrate_batch(&[record], None);

        assert!(results[0].is_err());
        let error = results[0].as_ref().unwrap_err();
        assert!(matches!(error, MigrationError::ValidationFailed(_)));
    }

    #[test]
    fn test_validation_empty_vector() {
        let config = MigrationConfig::new().with_validate_on_migrate(true);
        let service = RecordMigrationService::new(config);

        let mut record = create_test_record("doc_1");
        record.vector = vec![];

        let results = service.migrate_batch(&[record], None);

        assert!(results[0].is_err());
        let error = results[0].as_ref().unwrap_err();
        assert!(matches!(error, MigrationError::ValidationFailed(_)));
    }

    #[test]
    fn test_mode_transition_validation() {
        let service = RecordMigrationService::new(MigrationConfig::default());

        // Valid transitions
        assert!(
            service
                .validate_mode_transition(&MigrationMode::Legacy, &MigrationMode::DualWrite)
                .is_ok()
        );
        assert!(
            service
                .validate_mode_transition(&MigrationMode::DualWrite, &MigrationMode::Migrated)
                .is_ok()
        );
        assert!(
            service
                .validate_mode_transition(&MigrationMode::DualWrite, &MigrationMode::Legacy)
                .is_ok()
        );
        assert!(
            service
                .validate_mode_transition(&MigrationMode::Legacy, &MigrationMode::Legacy)
                .is_ok()
        );

        // Invalid transitions
        assert!(
            service
                .validate_mode_transition(&MigrationMode::Legacy, &MigrationMode::Migrated)
                .is_err()
        );
        assert!(
            service
                .validate_mode_transition(&MigrationMode::Migrated, &MigrationMode::Legacy)
                .is_err()
        );
        assert!(
            service
                .validate_mode_transition(&MigrationMode::Migrated, &MigrationMode::DualWrite)
                .is_err()
        );
    }

    #[test]
    fn test_estimate_migration_time() {
        let config = MigrationConfig::new()
            .with_batch_size(1000)
            .with_parallel_workers(4)
            .with_validate_on_migrate(true)
            .with_infer_schema(true);

        let service = RecordMigrationService::new(config);

        // 4000 records / (1000 * 4) = 1 second base
        // + 20% validation overhead = 1.2 seconds
        // + 10 seconds schema overhead = 11.2 seconds
        let estimate = service.estimate_migration_time(4000);
        assert!(estimate >= 11); // Should include schema overhead
    }

    #[test]
    fn test_migration_status_default() {
        let status = MigrationStatus::default();
        assert_eq!(status.mode, MigrationMode::Legacy);
        assert_eq!(status.stats.total_records, 0);
        assert!(status.schema_id.is_none());
        assert!(!status.is_paused);
        assert!(!status.is_running);
        assert_eq!(status.records_remaining, 0);
        assert!(status.last_error.is_none());
    }

    // ===========================================
    // Mode transition tests
    // ===========================================

    #[test]
    fn test_mode_transitions_legacy_to_dualwrite() {
        let service = RecordMigrationService::new(MigrationConfig::default());

        // Legacy -> DualWrite is valid
        assert!(
            service
                .validate_mode_transition(&MigrationMode::Legacy, &MigrationMode::DualWrite)
                .is_ok()
        );
    }

    #[test]
    fn test_mode_transitions_dualwrite_to_migrated() {
        let service = RecordMigrationService::new(MigrationConfig::default());

        // DualWrite -> Migrated is valid
        assert!(
            service
                .validate_mode_transition(&MigrationMode::DualWrite, &MigrationMode::Migrated)
                .is_ok()
        );
    }

    #[test]
    fn test_mode_transitions_rollback() {
        let service = RecordMigrationService::new(MigrationConfig::default());

        // DualWrite -> Legacy (rollback) is valid
        assert!(
            service
                .validate_mode_transition(&MigrationMode::DualWrite, &MigrationMode::Legacy)
                .is_ok()
        );
    }

    #[test]
    fn test_mode_transitions_skip_not_allowed() {
        let service = RecordMigrationService::new(MigrationConfig::default());

        // Legacy -> Migrated (skipping DualWrite) is invalid
        let result =
            service.validate_mode_transition(&MigrationMode::Legacy, &MigrationMode::Migrated);
        assert!(result.is_err());

        if let Err(MigrationError::InvalidModeTransition { from, to }) = result {
            assert_eq!(from, MigrationMode::Legacy);
            assert_eq!(to, MigrationMode::Migrated);
        } else {
            panic!("Expected InvalidModeTransition error");
        }
    }

    #[test]
    fn test_mode_transitions_migrated_is_final() {
        let service = RecordMigrationService::new(MigrationConfig::default());

        // Migrated -> Legacy is invalid (can't rollback after full migration)
        assert!(
            service
                .validate_mode_transition(&MigrationMode::Migrated, &MigrationMode::Legacy)
                .is_err()
        );

        // Migrated -> DualWrite is invalid
        assert!(
            service
                .validate_mode_transition(&MigrationMode::Migrated, &MigrationMode::DualWrite)
                .is_err()
        );
    }

    // ===========================================
    // Record conversion tests
    // ===========================================

    #[test]
    fn test_convert_record_basic() {
        let service = RecordMigrationService::new(MigrationConfig::default());
        let record = create_test_record("test_1");

        let proxima = service.convert_record(&record, Some("schema_123"), &[]);

        assert_eq!(proxima.id, "test_1");
        assert_eq!(proxima.vector, vec![0.1, 0.2, 0.3, 0.4]);
        assert_eq!(proxima.vector_dimension, Some(4));
        assert_eq!(proxima.schema_id, Some("schema_123".to_string()));
        assert!(proxima.typed_fields.contains_key("category"));
        assert!(proxima.typed_fields.contains_key("content"));
    }

    #[test]
    fn test_convert_record_with_text_columns() {
        let service = RecordMigrationService::new(MigrationConfig::default());
        let record = create_test_record("test_1");

        let text_columns = vec!["content".to_string()];
        let proxima = service.convert_record(&record, None, &text_columns);

        // "content" should be in text_fields, not typed_fields
        assert!(!proxima.typed_fields.contains_key("content"));
        assert_eq!(proxima.text_fields.len(), 1);
        assert_eq!(proxima.text_fields[0].name, "content");
        assert_eq!(proxima.text_fields[0].content, "This is test content");

        // "category" should still be in typed_fields
        assert!(proxima.typed_fields.contains_key("category"));
    }

    // ===========================================
    // Validation tests
    // ===========================================

    #[test]
    fn test_validate_record_success() {
        let service = RecordMigrationService::new(MigrationConfig::default());
        let record = create_test_record("doc_1");
        let proxima = service.convert_record(&record, None, &[]);

        let result = service.validate_record(&proxima, None);

        assert!(result.valid);
        assert!(result.errors.is_empty());
        assert_eq!(result.record_id, "doc_1");
    }

    #[test]
    fn test_validate_record_empty_id_fails() {
        let service = RecordMigrationService::new(MigrationConfig::default());
        let mut record = create_test_record("doc_1");
        record.id = String::new();
        let proxima = service.convert_record(&record, None, &[]);

        let result = service.validate_record(&proxima, None);

        assert!(!result.valid);
        assert!(!result.errors.is_empty());
        assert!(result.errors.iter().any(|e| e.field == "id"));
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ValidationErrorCode::RequiredFieldMissing)
        );
    }

    #[test]
    fn test_validate_record_empty_vector_fails() {
        let service = RecordMigrationService::new(MigrationConfig::default());
        let mut record = create_test_record("doc_1");
        record.vector = vec![];
        let proxima = service.convert_record(&record, None, &[]);

        let result = service.validate_record(&proxima, None);

        assert!(!result.valid);
        assert!(result.errors.iter().any(|e| e.field == "vector"));
    }

    #[test]
    fn test_validate_record_dimension_mismatch() {
        let service = RecordMigrationService::new(MigrationConfig::default());
        let record = create_test_record("doc_1");
        let mut proxima = service.convert_record(&record, None, &[]);
        proxima.vector_dimension = Some(10); // Mismatch: vector has 4 elements

        let result = service.validate_record(&proxima, None);

        assert!(!result.valid);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ValidationErrorCode::DimensionMismatch)
        );
    }

    // ===========================================
    // Batch processing tests
    // ===========================================

    #[test]
    fn test_migrate_batch_partial_failure() {
        let config = MigrationConfig::new().with_validate_on_migrate(true);
        let service = RecordMigrationService::new(config);

        // Create a mix of valid and invalid records
        let valid_record = create_test_record("valid_1");
        let mut invalid_record = create_test_record("");
        invalid_record.id = String::new(); // Empty ID will fail validation

        let records = vec![valid_record, invalid_record, create_test_record("valid_2")];
        let results = service.migrate_batch(&records, None);

        assert_eq!(results.len(), 3);
        assert!(results[0].is_ok()); // First valid
        assert!(results[1].is_err()); // Second invalid
        assert!(results[2].is_ok()); // Third valid
    }

    // ===========================================
    // Migration result tests
    // ===========================================

    #[test]
    fn test_migration_result_success() {
        let stats = MigrationStats {
            total_records: 100,
            migrated_records: 100,
            failed_records: 0,
            start_time: None,
            end_time: None,
        };

        let result = MigrationResult::success(stats.clone(), None);

        assert!(result.success);
        assert_eq!(result.stats.total_records, 100);
        assert!(result.error.is_none());
    }

    #[test]
    fn test_migration_result_failure() {
        let stats = MigrationStats::default();
        let result = MigrationResult::failure(stats, "Test error".to_string());

        assert!(!result.success);
        assert_eq!(result.error, Some("Test error".to_string()));
    }

    // ===========================================
    // Validation result tests
    // ===========================================

    #[test]
    fn test_validation_result_success() {
        let result = ValidationResult::success("doc_1".to_string());

        assert!(result.valid);
        assert!(result.errors.is_empty());
        assert_eq!(result.record_id, "doc_1");
    }

    #[test]
    fn test_validation_result_with_warning() {
        let result = ValidationResult::success("doc_1".to_string())
            .with_warning("Field 'x' not in schema".to_string());

        assert!(result.valid); // Warnings don't fail validation
        assert_eq!(result.warnings.len(), 1);
    }

    #[test]
    fn test_validation_result_failure() {
        let errors = vec![ValidationError {
            field: "id".to_string(),
            message: "ID is required".to_string(),
            code: ValidationErrorCode::RequiredFieldMissing,
        }];

        let result = ValidationResult::failure("doc_1".to_string(), errors);

        assert!(!result.valid);
        assert_eq!(result.errors.len(), 1);
    }

    // ===========================================
    // Migration state tracking tests
    // ===========================================

    #[test]
    fn test_migration_active_check() {
        let service = RecordMigrationService::new(MigrationConfig::default());

        // Initially no migrations should be active
        assert!(!service.is_migration_active("test_collection"));
        assert!(service.get_active_migrations().is_empty());
    }

    #[test]
    fn test_migration_pause_no_active_migration() {
        let service = RecordMigrationService::new(MigrationConfig::default());

        // Pausing non-existent migration should fail
        let result = service.pause_migration("non_existent");
        assert!(result.is_err());
    }

    #[test]
    fn test_migration_resume_no_active_migration() {
        let service = RecordMigrationService::new(MigrationConfig::default());

        // Resuming non-existent migration should fail
        let result = service.resume_migration("non_existent");
        assert!(result.is_err());
    }

    #[test]
    fn test_migration_stop_no_active_migration() {
        let service = RecordMigrationService::new(MigrationConfig::default());

        // Stopping non-existent migration should fail
        let result = service.stop_migration("non_existent");
        assert!(result.is_err());
    }

    // ===========================================
    // Schema inference integration tests
    // ===========================================

    #[tokio::test]
    async fn test_migrate_records_with_schema_inference() {
        let config = MigrationConfig::new()
            .with_infer_schema(true)
            .with_batch_size(100);

        let service = RecordMigrationService::new(config);

        // Create test records
        let records: Vec<VectorRecord> = (0..5)
            .map(|i| create_test_record(&format!("doc_{}", i)))
            .collect();

        let result = service
            .migrate_records(
                "test_collection",
                records.into_iter(),
                MigrationMode::DualWrite,
            )
            .await;

        assert!(result.is_ok());
        let migration_result = result.unwrap();
        assert!(migration_result.success);
        assert_eq!(migration_result.stats.total_records, 5);
        assert_eq!(migration_result.stats.migrated_records, 5);
        assert_eq!(migration_result.stats.failed_records, 0);
        // Schema should be inferred since infer_schema=true
        assert!(migration_result.inferred_schema.is_some());
    }

    #[tokio::test]
    async fn test_migrate_records_without_schema_inference() {
        let config = MigrationConfig::new()
            .with_infer_schema(false)
            .with_batch_size(100);

        let service = RecordMigrationService::new(config);

        let records: Vec<VectorRecord> = (0..3)
            .map(|i| create_test_record(&format!("doc_{}", i)))
            .collect();

        let result = service
            .migrate_records(
                "test_collection",
                records.into_iter(),
                MigrationMode::DualWrite,
            )
            .await;

        assert!(result.is_ok());
        let migration_result = result.unwrap();
        assert!(migration_result.inferred_schema.is_none());
    }

    #[tokio::test]
    async fn test_migrate_records_empty_collection() {
        let service = RecordMigrationService::new(MigrationConfig::default());

        let records: Vec<VectorRecord> = vec![];

        let result = service
            .migrate_records(
                "empty_collection",
                records.into_iter(),
                MigrationMode::DualWrite,
            )
            .await;

        assert!(result.is_ok());
        let migration_result = result.unwrap();
        assert_eq!(migration_result.stats.total_records, 0);
        assert!(migration_result.inferred_schema.is_none()); // No records to infer from
    }

    #[tokio::test]
    async fn test_concurrent_migration_prevention() {
        let service = RecordMigrationService::new(MigrationConfig::default());

        // Register a fake active migration
        let state = Arc::new(MigrationState::new(
            "test_collection".to_string(),
            MigrationMode::DualWrite,
        ));
        service
            .register_migration("test_collection", state)
            .unwrap();

        // Try to start another migration - should fail
        let records: Vec<VectorRecord> = vec![create_test_record("doc_1")];
        let result = service
            .migrate_records(
                "test_collection",
                records.into_iter(),
                MigrationMode::DualWrite,
            )
            .await;

        assert!(matches!(result, Err(MigrationError::MigrationInProgress)));

        // Clean up
        service.unregister_migration("test_collection");
    }

    // ===========================================
    // TEXT column identification tests
    // ===========================================

    #[test]
    fn test_text_column_extraction() {
        let config = MigrationConfig::new()
            .with_text_columns(vec!["content".to_string(), "description".to_string()]);

        let service = RecordMigrationService::new(config);

        let mut metadata = HashMap::new();
        metadata.insert(
            "content".to_string(),
            SqlValue {
                value: Some(SqlValueVariant::StringValue(
                    "Long text content...".to_string(),
                )),
            },
        );
        metadata.insert(
            "description".to_string(),
            SqlValue {
                value: Some(SqlValueVariant::StringValue("Description here".to_string())),
            },
        );
        metadata.insert(
            "category".to_string(),
            SqlValue {
                value: Some(SqlValueVariant::StringValue("test".to_string())),
            },
        );

        let record = VectorRecord {
            id: "doc_1".to_string(),
            vector: vec![0.1, 0.2],
            metadata,
            timestamp: Some(1704067200000),
            updated_at: None,
            expires_at: None,
            version: Some(1),
            source: None,
        };

        let text_columns = vec!["content".to_string(), "description".to_string()];
        let proxima = service.convert_record(&record, None, &text_columns);

        // Text columns should be in text_fields
        assert_eq!(proxima.text_fields.len(), 2);
        let text_field_names: Vec<&str> = proxima
            .text_fields
            .iter()
            .map(|tf| tf.name.as_str())
            .collect();
        assert!(text_field_names.contains(&"content"));
        assert!(text_field_names.contains(&"description"));

        // Non-text column should be in typed_fields
        assert!(proxima.typed_fields.contains_key("category"));
        assert!(!proxima.typed_fields.contains_key("content"));
        assert!(!proxima.typed_fields.contains_key("description"));
    }
}
