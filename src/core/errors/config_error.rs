//! Configuration-related error types

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Configuration errors
#[derive(Debug, Clone, Error, Serialize, Deserialize)]
pub enum ConfigError {
    #[error("Invalid configuration value: {field} = {value}")]
    InvalidValue { field: String, value: String },
    
    #[error("Missing required field: {field}")]
    MissingField { field: String },
    
    #[error("JSON parsing error: {0}")]
    JsonParseError(String),
    
    #[error("TOML parsing error: {0}")]
    TomlParseError(String),
    
    #[error("Validation failed: {0}")]
    ValidationFailed(String),
}