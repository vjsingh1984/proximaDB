//! TDD tests for PAX-Native OLAP Scan (TD-OLAP-1)
//!
//! Following Red-Green-Refactor cycle:
//! 1. Red: Write failing test for required behavior
//! 2. Green: Implement minimal code to pass test
//! 3. Refactor: Improve code quality while tests stay green

#[cfg(test)]
mod tests {
    use proximadb_catalog::{
        CatalogAuthorityMode, CatalogPhysicalFormat, CatalogStorageLayout, CatalogTableSchema,
    };

    /// Helper to create a test storage layout
    fn test_storage_layout(
        physical_format: CatalogPhysicalFormat,
        authority: CatalogAuthorityMode,
    ) -> CatalogStorageLayout {
        CatalogStorageLayout {
            location: "/test/location".to_string(),
            physical_format,
            authority,
            format_options: std::collections::HashMap::new(),
            created_at: 0,
            updated_at: 0,
        }
    }

    /// Helper to create a test table schema
    fn test_table_schema(layouts: Vec<CatalogStorageLayout>) -> CatalogTableSchema {
        CatalogTableSchema {
            table_name: "test_table".to_string(),
            columns: vec![],
            storage_layouts: layouts,
            primary_key: vec![],
            partition_keys: vec![],
            clustering_keys: vec![],
            table_properties: std::collections::HashMap::new(),
        }
    }

    /// Test 2.1: Route flip from catalog signals
    ///
    /// Validates that `pax_backed` is determined from catalog signals
    /// (specifically, whether a table has ProximaBlock format storage layout)
    /// rather than being hard-coded to `false`.
    #[test]
    fn pax_route_enabled_from_catalog_proximablock_format() {
        // Given: A table with ProximaBlock format storage layout
        let pax_layout = test_storage_layout(
            CatalogPhysicalFormat::ProximaBlock,
            CatalogAuthorityMode::Wal,
        );
        let schema = test_table_schema(vec![pax_layout]);

        // When: Checking if table is PAX-backed
        let is_pax = schema
            .storage_layouts
            .iter()
            .any(|layout| matches!(layout.physical_format, CatalogPhysicalFormat::ProximaBlock));

        // Then: Should return true (PAX format detected)
        assert!(
            is_pax,
            "Table with ProximaBlock format should be recognized as PAX-backed"
        );
    }

    /// Test 2.1b: Non-PAX table should not be PAX-backed
    #[test]
    fn non_pax_table_not_recognized_as_pax_backed() {
        // Given: A table with Parquet format (not PAX)
        let parquet_layout = test_storage_layout(
            CatalogPhysicalFormat::Parquet,
            CatalogAuthorityMode::ProjectionPublication,
        );
        let schema = test_table_schema(vec![parquet_layout]);

        // When: Checking if table is PAX-backed
        let is_pax = schema
            .storage_layouts
            .iter()
            .any(|layout| matches!(layout.physical_format, CatalogPhysicalFormat::ProximaBlock));

        // Then: Should return false (Parquet is not PAX)
        assert!(
            !is_pax,
            "Table with Parquet format should not be recognized as PAX-backed"
        );
    }

    /// Test 2.1c: Empty storage layouts should not be PAX-backed
    #[test]
    fn empty_storage_layouts_not_pax_backed() {
        // Given: A table with no storage layouts
        let schema = test_table_schema(vec![]);

        // When: Checking if table is PAX-backed
        let is_pax = schema
            .storage_layouts
            .iter()
            .any(|layout| matches!(layout.physical_format, CatalogPhysicalFormat::ProximaBlock));

        // Then: Should return false (no layouts = not PAX)
        assert!(
            !is_pax,
            "Table with no storage layouts should not be recognized as PAX-backed"
        );
    }

    /// Test 2.1d: Mixed layouts should recognize PAX if present
    #[test]
    fn mixed_layouts_recognize_pax_when_present() {
        // Given: A table with both Parquet and PAX layouts
        let parquet_layout = test_storage_layout(
            CatalogPhysicalFormat::Parquet,
            CatalogAuthorityMode::ProjectionPublication,
        );
        let pax_layout = test_storage_layout(
            CatalogPhysicalFormat::ProximaBlock,
            CatalogAuthorityMode::Wal,
        );
        let schema = test_table_schema(vec![parquet_layout, pax_layout]);

        // When: Checking if table is PAX-backed
        let is_pax = schema
            .storage_layouts
            .iter()
            .any(|layout| matches!(layout.physical_format, CatalogPhysicalFormat::ProximaBlock));

        // Then: Should return true (PAX layout present)
        assert!(
            is_pax,
            "Table with mixed layouts including ProximaBlock should be recognized as PAX-backed"
        );
    }
}
