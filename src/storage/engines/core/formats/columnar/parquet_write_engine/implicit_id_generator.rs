//! ID-less Storage Optimization
//!
//! This module provides utilities for ID-less storage optimization,
//! where vectors are stored without explicit IDs and implicit IDs
//! are generated from row group and row indices.

use anyhow::{Result, anyhow};

/// ID-less vector lookup utilities
pub struct IdLessLookup;

impl IdLessLookup {
    /// Generate implicit ID from row group and row index
    pub fn generate_implicit_id(row_group: u32, row_index: u32) -> String {
        format!("rg{:06}_row{:08}", row_group, row_index)
    }

    /// Parse implicit ID to get row group and row index
    pub fn parse_implicit_id(implicit_id: &str) -> Result<(u32, u32)> {
        let parts: Vec<&str> = implicit_id.split('_').collect();
        if parts.len() != 2 {
            return Err(anyhow!("Invalid implicit ID format: {}", implicit_id));
        }

        let row_group = parts[0]
            .strip_prefix("rg")
            .ok_or_else(|| anyhow!("Invalid row group prefix"))?
            .parse::<u32>()
            .map_err(|e| anyhow!("Failed to parse row group: {}", e))?;

        let row_index = parts[1]
            .strip_prefix("row")
            .ok_or_else(|| anyhow!("Invalid row index prefix"))?
            .parse::<u32>()
            .map_err(|e| anyhow!("Failed to parse row index: {}", e))?;

        Ok((row_group, row_index))
    }

    /// Check if an ID is an implicit ID
    pub fn is_implicit_id(id: &str) -> bool {
        id.starts_with("rg") && id.contains("_row")
    }

    /// Convert a batch of regular IDs to implicit IDs
    pub fn convert_batch_to_implicit(row_group: u32, start_row: u32, count: usize) -> Vec<String> {
        (0..count)
            .map(|i| Self::generate_implicit_id(row_group, start_row + i as u32))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_implicit_id_generation() {
        let id = IdLessLookup::generate_implicit_id(5, 1234);
        assert_eq!(id, "rg000005_row00001234");
    }

    #[test]
    fn test_implicit_id_parsing() {
        let id = "rg000005_row00001234";
        let (rg, row) = IdLessLookup::parse_implicit_id(id).unwrap();
        assert_eq!(rg, 5);
        assert_eq!(row, 1234);
    }

    #[test]
    fn test_is_implicit_id() {
        assert!(IdLessLookup::is_implicit_id("rg000001_row00000000"));
        assert!(!IdLessLookup::is_implicit_id("regular_id_123"));
        assert!(!IdLessLookup::is_implicit_id("test_id"));
    }

    #[test]
    fn test_batch_conversion() {
        let ids = IdLessLookup::convert_batch_to_implicit(2, 100, 3);
        assert_eq!(ids.len(), 3);
        assert_eq!(ids[0], "rg000002_row00000100");
        assert_eq!(ids[1], "rg000002_row00000101");
        assert_eq!(ids[2], "rg000002_row00000102");
    }

    #[test]
    fn test_invalid_id_parsing() {
        assert!(IdLessLookup::parse_implicit_id("invalid_id").is_err());
        assert!(IdLessLookup::parse_implicit_id("rg_row").is_err());
        assert!(IdLessLookup::parse_implicit_id("rgabc_row123").is_err());
    }
}
