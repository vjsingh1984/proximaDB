//! Expression normalization helpers for graph and vector query layers.

/// Normalize a label/property field name (placeholder: identity for now).
pub fn normalize_field(field: &str) -> String {
    field.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_field_preserves_current_identity_contract() {
        assert_eq!(normalize_field("tenant_id"), "tenant_id");
        assert_eq!(normalize_field("nested.property"), "nested.property");
        assert_eq!(normalize_field(""), "");
    }
}
