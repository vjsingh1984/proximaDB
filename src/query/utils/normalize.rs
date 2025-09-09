//! Expression normalization and filter pushdown helpers for both graph and vector layers.

/// Normalize label/property field names, unify dotted paths, etc.
pub fn normalize_field(field: &str) -> String {
    // placeholder normalization (e.g., metadata.price -> price if already mapped)
    field.to_string()
}

