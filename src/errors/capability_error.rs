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

//! # Capability Error Module
//!
//! Provides protocol-aware error mapping for capability validation failures.
//! Converts CapabilityCheckError from the query capability registry into
//! protocol-specific error responses for REST, gRPC, and SQL.
//!
//! ## Architecture
//!
//! ```text
//! CapabilityCheckError (from query capability registry)
//!         ↓
//!   CapabilityError::new(check_error)
//!         ↓
//!   ┌────────────────────────────────────────┐
//!   │     Protocol-Specific Error Mapping      │
//!   ├────────────────────────────────────────┤
//!   │ REST │ gRPC │ SQL                     │
//!   │ 400  │ InvalidArg │ Error with hints   │
//!   └────────────────────────────────────────┘
//! ```
//!
//! ## Usage
//!
//! ```rust,ignore
//! use proximaDB::query::capability::{CapabilityCheckError, CapabilitySet};
//! use proximaDB::errors::capability_error::CapabilityError;
//!
//! let check_error = CapabilityCheckError::UnsupportedCapability {
//!     capability: "GraphQuery".to_string(),
//!     available_alternatives: vec!["VectorSearch".to_string()],
//! };
//!
//! let cap_error = CapabilityError::new(check_error);
//!
//! // Convert to ApiError for REST/gRPC
//! let api_error = cap_error.into_api_error();
//! ```

use crate::query::capability::CapabilityCheckError;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Protocol-aware capability error with rich error information
///
/// This error type wraps CapabilityCheckError and provides
/// protocol-specific error formatting for REST, gRPC, and SQL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityError {
    /// The capability that was requested but not supported
    pub capability: String,

    /// Available alternative capabilities
    pub available_alternatives: Vec<String>,

    /// Error message explaining what went wrong
    pub message: String,

    /// Error type classification
    pub error_type: CapabilityErrorType,
}

/// Classification of capability errors
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CapabilityErrorType {
    /// A single capability is not supported
    UnsupportedCapability,

    /// Multiple capabilities are missing
    MultipleUnsupportedCapabilities,
}

impl CapabilityError {
    /// Create a new CapabilityError from a CapabilityCheckError
    pub fn new(check_error: CapabilityCheckError) -> Self {
        match check_error {
            CapabilityCheckError::UnsupportedCapability {
                capability,
                available_alternatives,
            } => {
                let message = if available_alternatives.is_empty() {
                    format!(
                        "The requested capability '{}' is not supported by the selected storage engine.",
                        capability
                    )
                } else {
                    format!(
                        "The requested capability '{}' is not supported. Available alternatives: {}",
                        capability,
                        available_alternatives.join(", ")
                    )
                };

                Self {
                    capability,
                    available_alternatives,
                    message,
                    error_type: CapabilityErrorType::UnsupportedCapability,
                }
            }

            CapabilityCheckError::MultipleUnsupportedCapabilities {
                missing_capabilities,
                available_alternatives,
            } => {
                let message = if available_alternatives.is_empty() {
                    format!(
                        "Multiple capabilities are not supported: {}. Please check the storage engine capabilities.",
                        missing_capabilities.join(", ")
                    )
                } else {
                    format!(
                        "Multiple capabilities are not supported: {}. Available alternatives: {}",
                        missing_capabilities.join(", "),
                        available_alternatives.join(", ")
                    )
                };

                Self {
                    capability: missing_capabilities.join(", "),
                    available_alternatives,
                    message,
                    error_type: CapabilityErrorType::MultipleUnsupportedCapabilities,
                }
            }
        }
    }

    /// Create a REST API error response body
    pub fn to_rest_response(&self) -> serde_json::Value {
        let mut response = serde_json::json!({
            "error": {
                "error_type": self.error_type_to_string(),
                "message": self.message,
                "missing_capability": self.capability,
            }
        });

        // Add alternatives if available
        if !self.available_alternatives.is_empty() {
            response["error"]["available_alternatives"] =
                serde_json::to_value(&self.available_alternatives)
                    .unwrap_or(serde_json::Value::Null);
        }

        response
    }

    /// Create a gRPC error message
    pub fn to_grpc_message(&self) -> String {
        if self.available_alternatives.is_empty() {
            self.message.clone()
        } else {
            format!(
                "{}. Available alternatives: {}",
                self.message,
                self.available_alternatives.join(", ")
            )
        }
    }

    /// Create a SQL error message
    pub fn to_sql_message(&self) -> String {
        if self.available_alternatives.is_empty() {
            format!("ERROR: {}", self.message)
        } else {
            format!(
                "ERROR: {}\nHINT: Try using: {}",
                self.message,
                self.available_alternatives.join(" or ")
            )
        }
    }

    /// Get HTTP status code for REST errors
    pub fn http_status_code(&self) -> u16 {
        match self.error_type {
            CapabilityErrorType::UnsupportedCapability => 400, // Bad Request
            CapabilityErrorType::MultipleUnsupportedCapabilities => 400, // Bad Request
        }
    }

    /// Get gRPC status code
    pub fn grpc_status_code(&self) -> tonic::Code {
        tonic::Code::InvalidArgument
    }

    /// Convert error type to string for REST responses
    fn error_type_to_string(&self) -> &'static str {
        match self.error_type {
            CapabilityErrorType::UnsupportedCapability => "unsupported_capability",
            CapabilityErrorType::MultipleUnsupportedCapabilities => {
                "multiple_unsupported_capabilities"
            }
        }
    }

    /// Check if this error has available alternatives
    pub fn has_alternatives(&self) -> bool {
        !self.available_alternatives.is_empty()
    }

    /// Get the number of missing capabilities
    pub fn missing_capability_count(&self) -> usize {
        if self.error_type == CapabilityErrorType::UnsupportedCapability {
            1
        } else {
            self.capability.split(", ").count()
        }
    }
}

impl fmt::Display for CapabilityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for CapabilityError {}

/// Helper function to create a capability error with context
///
/// ## Usage
/// ```rust,ignore
/// use proximaDB::errors::capability_error::CapabilityError;
///
/// let error = CapabilityError::unsupported(
///     "GraphQuery",
///     vec!["VectorSearch".to_string(), "DocumentQuery".to_string()]
/// );
/// ```
impl CapabilityError {
    /// Create an unsupported capability error
    pub fn unsupported(capability: &str, alternatives: Vec<String>) -> Self {
        let is_empty = alternatives.is_empty();
        let alternatives_str = alternatives.join(", ");
        Self {
            capability: capability.to_string(),
            available_alternatives: if is_empty {
                vec![]
            } else {
                alternatives.clone()
            },
            message: if is_empty {
                format!(
                    "The requested capability '{}' is not supported by the selected storage engine.",
                    capability
                )
            } else {
                format!(
                    "The requested capability '{}' is not supported. Available alternatives: {}",
                    capability, alternatives_str
                )
            },
            error_type: CapabilityErrorType::UnsupportedCapability,
        }
    }

    /// Create a multiple unsupported capabilities error
    pub fn multiple_unsupported(capabilities: Vec<String>, alternatives: Vec<String>) -> Self {
        let is_empty = alternatives.is_empty();
        let alternatives_str = alternatives.join(", ");
        Self {
            capability: capabilities.join(", "),
            available_alternatives: if is_empty {
                vec![]
            } else {
                alternatives.clone()
            },
            message: if is_empty {
                format!(
                    "Multiple capabilities are not supported: {}. Please check the storage engine capabilities.",
                    capabilities.join(", ")
                )
            } else {
                format!(
                    "Multiple capabilities are not supported: {}. Available alternatives: {}",
                    capabilities.join(", "),
                    alternatives_str
                )
            },
            error_type: CapabilityErrorType::MultipleUnsupportedCapabilities,
        }
    }
}

// ============================================================================
// CONVERSION TRAITS
// ============================================================================

/// Convert CapabilityCheckError to CapabilityError
impl From<CapabilityCheckError> for CapabilityError {
    fn from(check_error: CapabilityCheckError) -> Self {
        Self::new(check_error)
    }
}

// ============================================================================
/// Convert CapabilityError into ApiError for HTTP/gRPC responses
impl From<CapabilityError> for crate::errors::ApiError {
    fn from(err: CapabilityError) -> Self {
        crate::errors::ApiError::InvalidArgument(format!(
            "Capability '{}' not supported: {}",
            err.capability, err.message
        ))
    }
}

// UNIT TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::capability::CapabilityCheckError;

    #[test]
    fn test_unsupported_capability_error() {
        let check_error = CapabilityCheckError::UnsupportedCapability {
            capability: "GraphQuery".to_string(),
            available_alternatives: vec!["VectorSearch".to_string(), "DocumentQuery".to_string()],
        };

        let cap_error = CapabilityError::new(check_error);

        assert_eq!(cap_error.capability, "GraphQuery");
        assert_eq!(cap_error.available_alternatives.len(), 2);
        assert!(cap_error.has_alternatives());
        assert_eq!(cap_error.missing_capability_count(), 1);
        assert_eq!(cap_error.http_status_code(), 400);
    }

    #[test]
    fn test_multiple_unsupported_capabilities_error() {
        let check_error = CapabilityCheckError::MultipleUnsupportedCapabilities {
            missing_capabilities: vec!["GraphQuery".to_string(), "FullTextSearch".to_string()],
            available_alternatives: vec!["VectorSearch".to_string()],
        };

        let cap_error = CapabilityError::new(check_error);

        assert_eq!(cap_error.capability, "GraphQuery, FullTextSearch");
        assert_eq!(cap_error.available_alternatives.len(), 1);
        assert!(cap_error.has_alternatives());
        assert_eq!(cap_error.missing_capability_count(), 2);
        assert_eq!(cap_error.http_status_code(), 400);
    }

    #[test]
    fn test_error_without_alternatives() {
        let check_error = CapabilityCheckError::UnsupportedCapability {
            capability: "SomeFutureCapability".to_string(),
            available_alternatives: vec![],
        };

        let cap_error = CapabilityError::new(check_error);

        assert_eq!(cap_error.available_alternatives.len(), 0);
        assert!(!cap_error.has_alternatives());
    }

    #[test]
    fn test_rest_response_formatting() {
        let check_error = CapabilityCheckError::UnsupportedCapability {
            capability: "GraphQuery".to_string(),
            available_alternatives: vec!["VectorSearch".to_string()],
        };

        let cap_error = CapabilityError::new(check_error);
        let response = cap_error.to_rest_response();

        assert!(response["error"]["missing_capability"].is_string());
        assert!(response["error"]["available_alternatives"].is_array());
        assert_eq!(response["error"]["error_type"], "unsupported_capability");
    }

    #[test]
    fn test_grpc_message_formatting() {
        let check_error = CapabilityCheckError::UnsupportedCapability {
            capability: "GraphQuery".to_string(),
            available_alternatives: vec!["VectorSearch".to_string()],
        };

        let cap_error = CapabilityError::new(check_error);
        let message = cap_error.to_grpc_message();

        assert!(message.contains("GraphQuery"));
        assert!(message.contains("VectorSearch"));
    }

    #[test]
    fn test_sql_message_formatting() {
        let check_error = CapabilityCheckError::UnsupportedCapability {
            capability: "GraphQuery".to_string(),
            available_alternatives: vec!["VectorSearch".to_string()],
        };

        let cap_error = CapabilityError::new(check_error);
        let message = cap_error.to_sql_message();

        assert!(message.starts_with("ERROR:"));
        assert!(message.contains("HINT:"));
        assert!(message.contains("VectorSearch"));
    }

    #[test]
    fn test_sql_message_without_alternatives() {
        let check_error = CapabilityCheckError::UnsupportedCapability {
            capability: "SomeCapability".to_string(),
            available_alternatives: vec![],
        };

        let cap_error = CapabilityError::new(check_error);
        let message = cap_error.to_sql_message();

        assert!(message.starts_with("ERROR:"));
        assert!(!message.contains("HINT:"));
    }

    #[test]
    fn test_conversion_to_api_error() {
        let check_error = CapabilityCheckError::UnsupportedCapability {
            capability: "GraphQuery".to_string(),
            available_alternatives: vec!["VectorSearch".to_string()],
        };

        let cap_error = CapabilityError::new(check_error);
        let api_error = crate::errors::ApiError::from(cap_error);

        // Should convert to InvalidArgument
        assert!(
            api_error.to_string().contains("GraphQuery")
                || api_error.to_string().contains("capability")
        );
    }
}
