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

//! Security validation for typed data
//!
//! Provides per-type validation with SQL injection prevention for ProximaRecord fields.
//! This module implements comprehensive validation strategies for all supported column
//! data types in ProximaDB.
//!
//! # Design Principles
//!
//! - **Defense in Depth**: Multiple layers of validation (type, format, security)
//! - **Fail Fast**: Invalid data is rejected at ingestion time
//! - **Configurable**: Validation rules can be customized per-field
//! - **Performance**: Pre-compiled patterns for efficient validation
//!
//! # Usage
//!
//! ```rust,ignore
//! use proximadb::security::validation::{TypedValueValidator, TextValidator};
//! use proximadb::core::types::{TypedValue, ColumnDataType};
//!
//! // Create a typed value validator with security enabled
//! let mut validator = TypedValueValidator::new().with_security(true);
//!
//! // Validate a text field
//! let text_value = TypedValue::Text("Hello, World!".to_string());
//! assert!(validator.validate_field("greeting", &text_value).is_ok());
//!
//! // Text validator with SQL injection detection
//! let text_validator = TextValidator::new()
//!     .with_max_length(1024)
//!     .with_sql_injection_check(true);
//!
//! assert!(text_validator.validate("normal text").is_ok());
//! assert!(text_validator.validate("'; DROP TABLE users; --").is_err());
//! ```

pub mod metadata_validator;
pub mod text_validator;
pub mod type_validators;

pub use text_validator::{
    TextStorageValidationConfig, TextStorageValidationResult, TextValidationError, TextValidator,
    TextValidatorBuilder,
};
#[allow(deprecated)]
#[deprecated(note = "Use TypeValidationResult instead.")]
pub use type_validators::ValidationResult;
pub use type_validators::{
    BinaryValidator, DecimalValidator, FieldValidationConfig, JsonValidator, TimestampValidator,
    TypeValidationResult, TypedValueValidator, UuidValidator, ValidationError,
    contains_sql_injection_pattern,
};

pub use metadata_validator::{
    CollectionNameValidator, MetadataValidationConfig, MetadataValidator, validate_collection_name,
    validate_record_metadata,
};
