//! Metadata-related error types

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Metadata errors
#[derive(Debug, Clone, Error, Serialize, Deserialize)]
pub enum MetadataError {
    /// Schema validation rule violated
    #[error("Schema validation failed: {0}")]
    SchemaValidation(String),

    /// Field value type does not match the expected schema type
    #[error("Field type mismatch: expected {expected}, found {found}")]
    TypeMismatch {
        /// Expected type name
        expected: String,
        /// Actual type name found in the data
        found: String,
    },

    /// A required metadata field was not provided
    #[error("Required field missing: {field}")]
    RequiredFieldMissing {
        /// Name of the missing field
        field: String,
    },
}
