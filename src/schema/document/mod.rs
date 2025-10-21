use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Document schema definition for ProximaDB
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentSchema {
    /// Schema name/identifier
    pub name: String,
    /// Schema version for evolution
    pub version: String,
    /// Field definitions
    pub fields: HashMap<String, FieldDefinition>,
    /// Optional description
    pub description: Option<String>,
    /// Schema metadata
    pub metadata: HashMap<String, String>,
}

/// Field definition in document schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldDefinition {
    /// Field data type
    pub field_type: FieldType,
    /// Whether field is required
    pub required: bool,
    /// Whether field is indexed for search
    pub indexed: bool,
    /// Optional field description
    pub description: Option<String>,
    /// Field validation constraints
    pub constraints: Option<FieldConstraints>,
}

/// Supported field types in document schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FieldType {
    String,
    Integer,
    Float,
    Boolean,
    DateTime,
    Vector(u32), // Vector with dimension
    Array(Box<FieldType>),
    Object(HashMap<String, FieldDefinition>),
    Json, // Free-form JSON
}

/// Field validation constraints
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldConstraints {
    /// Minimum value (for numeric types)
    pub min_value: Option<f64>,
    /// Maximum value (for numeric types)
    pub max_value: Option<f64>,
    /// Minimum length (for string types)
    pub min_length: Option<usize>,
    /// Maximum length (for string types)
    pub max_length: Option<usize>,
    /// Pattern matching (for string types)
    pub pattern: Option<String>,
    /// Allowed values (enum-like constraint)
    pub allowed_values: Option<Vec<String>>,
}

impl DocumentSchema {
    /// Create new document schema
    pub fn new(name: String, version: String) -> Self {
        Self {
            name,
            version,
            fields: HashMap::new(),
            description: None,
            metadata: HashMap::new(),
        }
    }

    /// Add field to schema
    pub fn add_field(&mut self, name: String, field_def: FieldDefinition) -> &mut Self {
        self.fields.insert(name, field_def);
        self
    }

    /// Validate document against schema
    pub fn validate_document(&self, document: &serde_json::Value) -> Result<(), ValidationError> {
        // Basic validation implementation
        if let Some(obj) = document.as_object() {
            // Check required fields
            for (field_name, field_def) in &self.fields {
                if field_def.required && !obj.contains_key(field_name) {
                    return Err(ValidationError::MissingRequiredField(field_name.clone()));
                }

                // Validate field type if present
                if let Some(value) = obj.get(field_name) {
                    self.validate_field_value(field_name, &field_def.field_type, value)?;
                }
            }
        } else if !self.fields.is_empty() {
            return Err(ValidationError::InvalidDocumentStructure(
                "Expected object".to_string(),
            ));
        }

        Ok(())
    }

    fn validate_field_value(
        &self,
        field_name: &str,
        field_type: &FieldType,
        value: &serde_json::Value,
    ) -> Result<(), ValidationError> {
        match field_type {
            FieldType::String => {
                if !value.is_string() {
                    return Err(ValidationError::TypeMismatch(
                        field_name.to_string(),
                        "string".to_string(),
                    ));
                }
            }
            FieldType::Integer => {
                if !value.is_i64() && !value.is_u64() {
                    return Err(ValidationError::TypeMismatch(
                        field_name.to_string(),
                        "integer".to_string(),
                    ));
                }
            }
            FieldType::Float => {
                if !value.is_f64() {
                    return Err(ValidationError::TypeMismatch(
                        field_name.to_string(),
                        "float".to_string(),
                    ));
                }
            }
            FieldType::Boolean => {
                if !value.is_boolean() {
                    return Err(ValidationError::TypeMismatch(
                        field_name.to_string(),
                        "boolean".to_string(),
                    ));
                }
            }
            FieldType::Array(_) => {
                if !value.is_array() {
                    return Err(ValidationError::TypeMismatch(
                        field_name.to_string(),
                        "array".to_string(),
                    ));
                }
            }
            FieldType::Object(_) => {
                if !value.is_object() {
                    return Err(ValidationError::TypeMismatch(
                        field_name.to_string(),
                        "object".to_string(),
                    ));
                }
            }
            FieldType::Vector(expected_dim) => {
                if let Some(array) = value.as_array() {
                    if array.len() != *expected_dim as usize {
                        return Err(ValidationError::VectorDimensionMismatch(
                            field_name.to_string(),
                            *expected_dim,
                            array.len(),
                        ));
                    }
                    // Ensure all elements are numbers
                    if !array.iter().all(|v| v.is_f64() || v.is_i64() || v.is_u64()) {
                        return Err(ValidationError::TypeMismatch(
                            field_name.to_string(),
                            "numeric array".to_string(),
                        ));
                    }
                } else {
                    return Err(ValidationError::TypeMismatch(
                        field_name.to_string(),
                        "vector array".to_string(),
                    ));
                }
            }
            _ => {} // Skip validation for other types
        }

        Ok(())
    }
}

/// Schema validation errors
#[derive(Debug)]
pub enum ValidationError {
    MissingRequiredField(String),
    TypeMismatch(String, String),
    VectorDimensionMismatch(String, u32, usize),
    InvalidDocumentStructure(String),
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            ValidationError::MissingRequiredField(field) => {
                write!(f, "Missing required field: {}", field)
            }
            ValidationError::TypeMismatch(field, expected) => write!(
                f,
                "Type mismatch for field {}: expected {}",
                field, expected
            ),
            ValidationError::VectorDimensionMismatch(field, expected, actual) => write!(
                f,
                "Vector dimension mismatch for field {}: expected {}, got {}",
                field, expected, actual
            ),
            ValidationError::InvalidDocumentStructure(msg) => {
                write!(f, "Invalid document structure: {}", msg)
            }
        }
    }
}

impl std::error::Error for ValidationError {}
