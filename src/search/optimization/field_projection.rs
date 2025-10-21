//! Generic Field Projection Interface
//!
//! Provides a unified projection interface that works across different storage formats:
//! - Parquet columnar files (via ColumnProjection)
//! - ProximaDataBlocks row-based storage
//! - Generic HashMap metadata
//!
//! This eliminates the need for format-specific projection code.

use std::collections::HashSet;

/// Field name identifier
pub type FieldName = String;

/// Generic projection specification that works across storage formats
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldProjection {
    /// Fields to include (empty = all fields)
    pub included_fields: HashSet<FieldName>,

    /// Whether to include all fields
    pub include_all: bool,

    /// Whether to include vector data
    pub include_vector: bool,
}

impl FieldProjection {
    /// Create new projection with specific fields
    pub fn new(fields: Vec<FieldName>) -> Self {
        Self {
            included_fields: fields.into_iter().collect(),
            include_all: false,
            include_vector: false,
        }
    }

    /// Create projection with all fields
    pub fn all() -> Self {
        Self {
            included_fields: HashSet::new(),
            include_all: true,
            include_vector: true,
        }
    }

    /// Add field to projection
    pub fn with_field(mut self, field: FieldName) -> Self {
        self.included_fields.insert(field);
        self
    }

    /// Include vector data
    pub fn with_vector(mut self) -> Self {
        self.include_vector = true;
        self
    }

    /// Check if field is included
    pub fn includes_field(&self, field: &str) -> bool {
        self.include_all || self.included_fields.contains(field)
    }

    /// Get number of projected fields
    pub fn field_count(&self) -> usize {
        if self.include_all {
            usize::MAX
        } else {
            self.included_fields.len()
        }
    }

    /// Convert to columnar projection (for Parquet)
    #[cfg(feature = "parquet")]
    pub fn to_columnar_projection(&self) -> crate::storage::engines::core::formats::columnar::columnar_query_engine::column_projector::ColumnProjection{
        use crate::storage::engines::core::formats::columnar::columnar_query_engine::column_projector::ColumnProjection;

        let fields: Vec<String> = self.included_fields.iter().cloned().collect();
        ColumnProjection::new(fields, Vec::new())
    }

    /// Convert to field list for row-based filtering (ProximaDataBlocks)
    pub fn to_field_list(&self) -> Vec<FieldName> {
        self.included_fields.iter().cloned().collect()
    }

    /// Estimate benefit of using this projection
    pub fn estimate_benefit(&self, total_fields: usize) -> f64 {
        if self.include_all || total_fields == 0 {
            1.0 // No benefit
        } else {
            let projected = self.field_count();
            if projected >= total_fields {
                1.0
            } else {
                total_fields as f64 / projected as f64
            }
        }
    }
}

impl Default for FieldProjection {
    fn default() -> Self {
        Self::all()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_projection() {
        let proj = FieldProjection::new(vec!["field1".to_string(), "field2".to_string()]);
        assert_eq!(proj.field_count(), 2);
        assert!(proj.includes_field("field1"));
        assert!(!proj.includes_field("field3"));
    }

    #[test]
    fn test_all_projection() {
        let proj = FieldProjection::all();
        assert!(proj.include_all);
        assert!(proj.includes_field("any_field"));
        assert!(proj.include_vector);
    }

    #[test]
    fn test_with_field() {
        let proj = FieldProjection::new(vec!["f1".to_string()]).with_field("f2".to_string());

        assert_eq!(proj.field_count(), 2);
        assert!(proj.includes_field("f1"));
        assert!(proj.includes_field("f2"));
    }

    #[test]
    fn test_with_vector() {
        let proj = FieldProjection::new(vec!["f1".to_string()]).with_vector();

        assert!(proj.include_vector);
    }

    #[test]
    fn test_to_field_list() {
        let proj = FieldProjection::new(vec!["f1".to_string(), "f2".to_string()]);
        let fields = proj.to_field_list();

        assert_eq!(fields.len(), 2);
        assert!(fields.contains(&"f1".to_string()));
        assert!(fields.contains(&"f2".to_string()));
    }

    #[test]
    fn test_estimate_benefit() {
        let proj = FieldProjection::new(vec!["f1".to_string(), "f2".to_string()]);

        // 2 fields out of 10 = 5x benefit
        assert_eq!(proj.estimate_benefit(10), 5.0);

        // All fields = no benefit
        assert_eq!(proj.estimate_benefit(2), 1.0);
    }
}
