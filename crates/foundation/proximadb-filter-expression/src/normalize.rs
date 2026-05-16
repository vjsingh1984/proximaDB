//! Expression normalization helpers for graph and vector query layers.

/// Normalize a label/property field name (placeholder: identity for now).
pub fn normalize_field(field: &str) -> String {
    field.to_string()
}
