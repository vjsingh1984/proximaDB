//! Configuration-related error types

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Configuration errors
#[derive(Debug, Clone, Error, Serialize, Deserialize)]
pub enum ConfigError {
    /// A configuration field has an invalid value
    #[error("Invalid configuration value: {field} = {value}")]
    InvalidValue {
        /// Configuration field name
        field: String,
        /// Invalid value that was provided
        value: String,
    },

    /// A required configuration field was not specified
    #[error("Missing required field: {field}")]
    MissingField {
        /// Name of the missing field
        field: String,
    },

    /// Failed to parse JSON configuration
    #[error("JSON parsing error: {0}")]
    JsonParseError(String),

    /// Failed to parse TOML configuration
    #[error("TOML parsing error: {0}")]
    TomlParseError(String),

    /// Configuration failed validation rules
    #[error("Validation failed: {0}")]
    ValidationFailed(String),
}
