//! Metadata-related error types

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Metadata errors
#[derive(Debug, Clone, Error, Serialize, Deserialize)]
pub enum MetadataError {
    #[error("Schema validation failed: {0}")]
    SchemaValidation(String),
    
    #[error("Field type mismatch: expected {expected}, found {found}")]
    TypeMismatch { expected: String, found: String },
    
    #[error("Required field missing: {field}")]
    RequiredFieldMissing { field: String },
}