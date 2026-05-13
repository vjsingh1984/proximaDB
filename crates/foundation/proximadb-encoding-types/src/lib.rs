//! # ProximaDB Encoding Types
//!
//! Foundation encoding types for ProximaDB.
//!
//! ## Purpose
//!
//! This crate provides the single source of truth for encoding types
//! across the entire ProximaDB codebase. It eliminates the proliferation of
//! duplicate encoding definitions found throughout the codebase.
//!
//! ## Types
//!
//! - [`EncodingFormat`] - Standardized encoding format enum
//! - [`EncodingConfig`] - Configuration for encoding
//!
//! ## Migration
//!
//! If you're using legacy encoding types, migrate to this crate's types
//! using the provided conversion traits.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Standardized encoding format enum.
///
/// This is the single source of truth for encoding formats across ProximaDB.
/// All other encoding type definitions should migrate to use this enum.
///
/// ## Variants
///
/// - `Binary` - Binary encoding
/// - `Json` - JSON encoding
/// - `Protobuf` - Protocol Buffers encoding
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EncodingFormat {
    /// Binary encoding
    ///
    /// Native binary format for efficient storage and transmission
    Binary,

    /// JSON encoding
    ///
    /// Human-readable text format
    Json,

    /// Protocol Buffers encoding
    ///
    /// Compact binary format with schema
    Protobuf,
}

impl Default for EncodingFormat {
    fn default() -> Self {
        Self::Binary
    }
}

impl fmt::Display for EncodingFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Binary => write!(f, "binary"),
            Self::Json => write!(f, "json"),
            Self::Protobuf => write!(f, "protobuf"),
        }
    }
}

impl EncodingFormat {
    /// Create from string representation
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "binary" | "bin" => Some(Self::Binary),
            "json" => Some(Self::Json),
            "protobuf" | "proto" => Some(Self::Protobuf),
            _ => None,
        }
    }

    /// Check if this is a text-based encoding
    pub fn is_text(&self) -> bool {
        matches!(self, Self::Json)
    }

    /// Check if this is a binary encoding
    pub fn is_binary(&self) -> bool {
        !self.is_text()
    }

    /// Get the MIME type for this encoding format
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Binary => "application/octet-stream",
            Self::Json => "application/json",
            Self::Protobuf => "application/x-protobuf",
        }
    }

    /// Get the file extension for this encoding format
    pub fn file_extension(&self) -> &'static str {
        match self {
            Self::Binary => "bin",
            Self::Json => "json",
            Self::Protobuf => "pb",
        }
    }
}

/// Configuration for encoding
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EncodingConfig {
    /// Encoding format
    pub format: EncodingFormat,

    /// Whether to use pretty printing (for text formats)
    pub pretty: bool,

    /// Whether to use streaming encoding
    pub streaming: bool,
}

impl Default for EncodingConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl EncodingConfig {
    /// Create a new encoding config with binary format
    pub fn new() -> Self {
        Self {
            format: EncodingFormat::Binary,
            pretty: false,
            streaming: false,
        }
    }

    /// Create an encoding config for a specific format
    pub fn with_format(format: EncodingFormat) -> Self {
        Self {
            format,
            pretty: false,
            streaming: false,
        }
    }

    /// Enable pretty printing
    pub fn with_pretty(mut self) -> Self {
        self.pretty = true;
        self
    }

    /// Enable streaming encoding
    pub fn with_streaming(mut self) -> Self {
        self.streaming = true;
        self
    }

    /// Create a binary encoding config
    pub fn binary() -> Self {
        Self::with_format(EncodingFormat::Binary)
    }

    /// Create a JSON encoding config
    pub fn json() -> Self {
        Self::with_format(EncodingFormat::Json)
    }

    /// Create a Protobuf encoding config
    pub fn protobuf() -> Self {
        Self::with_format(EncodingFormat::Protobuf)
    }

    /// Get the encoding format
    pub fn format(&self) -> EncodingFormat {
        self.format
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encoding_format_default() {
        assert_eq!(EncodingFormat::default(), EncodingFormat::Binary);
    }

    #[test]
    fn test_encoding_format_display() {
        assert_eq!(EncodingFormat::Binary.to_string(), "binary");
        assert_eq!(EncodingFormat::Json.to_string(), "json");
        assert_eq!(EncodingFormat::Protobuf.to_string(), "protobuf");
    }

    #[test]
    fn test_encoding_format_from_str() {
        assert_eq!(
            EncodingFormat::from_str("binary"),
            Some(EncodingFormat::Binary)
        );
        assert_eq!(
            EncodingFormat::from_str("bin"),
            Some(EncodingFormat::Binary)
        );
        assert_eq!(EncodingFormat::from_str("json"), Some(EncodingFormat::Json));
        assert_eq!(
            EncodingFormat::from_str("protobuf"),
            Some(EncodingFormat::Protobuf)
        );
        assert_eq!(
            EncodingFormat::from_str("proto"),
            Some(EncodingFormat::Protobuf)
        );
        assert_eq!(EncodingFormat::from_str("unknown"), None);
    }

    #[test]
    fn test_encoding_format_is_text() {
        assert!(!EncodingFormat::Binary.is_text());
        assert!(EncodingFormat::Json.is_text());
        assert!(!EncodingFormat::Protobuf.is_text());
    }

    #[test]
    fn test_encoding_format_is_binary() {
        assert!(EncodingFormat::Binary.is_binary());
        assert!(!EncodingFormat::Json.is_binary());
        assert!(EncodingFormat::Protobuf.is_binary());
    }

    #[test]
    fn test_encoding_format_mime_type() {
        assert_eq!(
            EncodingFormat::Binary.mime_type(),
            "application/octet-stream"
        );
        assert_eq!(EncodingFormat::Json.mime_type(), "application/json");
        assert_eq!(
            EncodingFormat::Protobuf.mime_type(),
            "application/x-protobuf"
        );
    }

    #[test]
    fn test_encoding_format_file_extension() {
        assert_eq!(EncodingFormat::Binary.file_extension(), "bin");
        assert_eq!(EncodingFormat::Json.file_extension(), "json");
        assert_eq!(EncodingFormat::Protobuf.file_extension(), "pb");
    }

    #[test]
    fn test_encoding_config_default() {
        let config = EncodingConfig::default();
        assert_eq!(config.format(), EncodingFormat::Binary);
        assert!(!config.pretty);
        assert!(!config.streaming);
    }

    #[test]
    fn test_encoding_config_builder() {
        let config = EncodingConfig::json().with_pretty().with_streaming();

        assert_eq!(config.format(), EncodingFormat::Json);
        assert!(config.pretty);
        assert!(config.streaming);
    }

    #[test]
    fn test_encoding_config_constructors() {
        assert_eq!(EncodingConfig::binary().format(), EncodingFormat::Binary);
        assert_eq!(EncodingConfig::json().format(), EncodingFormat::Json);
        assert_eq!(
            EncodingConfig::protobuf().format(),
            EncodingFormat::Protobuf
        );
    }

    #[test]
    fn test_encoding_format_serialization() {
        let format = EncodingFormat::Json;
        let json = serde_json::to_string(&format).unwrap();
        assert_eq!(json, "\"json\"");

        let deserialized: EncodingFormat = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, EncodingFormat::Json);
    }

    #[test]
    fn test_encoding_config_serialization() {
        let config = EncodingConfig::json().with_pretty();
        let json = serde_json::to_string(&config).unwrap();

        let deserialized: EncodingConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.format(), EncodingFormat::Json);
        assert!(deserialized.pretty);
    }
}
