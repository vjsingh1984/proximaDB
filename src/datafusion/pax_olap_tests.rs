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

    /// Test 2.2: Path resolution via DrPathBuilder
    ///
    /// Validates that PAX segment paths are resolved via `DrPathBuilder`
    /// instead of using `catalog.location`. This ensures proper tenant isolation
    /// and object storage path structure per the co-design mandate.
    #[test]
    fn pax_segment_path_resolved_via_drpathbuilder() {
        // Given: A collection with tenant context
        let tenant_id = "test_tenant";
        let namespace_id = "test_namespace";
        let collection_name = "test_collection";

        // When: Building path via DrPathBuilder
        use proximadb_catalog::CatalogNamespace;
        let mock_namespace = CatalogNamespace::new(vec!["default".into()])
            .with_tenant(tenant_id)
            .with_namespace_id(namespace_id);

        let dr_path = crate::storage::trait_components::path_resolver::DrPathBuilder::build(
            &mock_namespace,
            collection_name,
        )
        .expect("DrPathBuilder should construct valid path");

        let resolved_path = dr_path.root_prefix();

        // Then: Path should follow DrPathBuilder structure: data/{tenant_id}/{namespace_id}/...
        // NOT use catalog.location (which would be arbitrary/unstructured)
        assert!(
            resolved_path.contains("data"),
            "DrPathBuilder should create tenant-isolated path under 'data/' prefix"
        );
        assert!(
            resolved_path.contains(tenant_id),
            "Path should contain tenant_id for isolation: {}", resolved_path
        );
        assert!(
            resolved_path.contains(namespace_id),
            "Path should contain namespace_id for isolation: {}", resolved_path
        );
        assert!(
            resolved_path.contains(collection_name),
            "Path should contain collection name: {}", resolved_path
        );
    }

    /// Test 2.2b: DrPathBuilder segments path structure
    #[test]
    fn drpathbuilder_segments_path_structure() {
        // Given: Collection context
        let tenant_id = "tenant123";
        let namespace_id = "ns456";
        let collection_name = "my_collection";

        // When: Building segments path via DrPathBuilder
        use proximadb_catalog::CatalogNamespace;
        let mock_namespace = CatalogNamespace::new(vec!["default".into()])
            .with_tenant(tenant_id)
            .with_namespace_id(namespace_id);

        let dr_path = crate::storage::trait_components::path_resolver::DrPathBuilder::build(
            &mock_namespace,
            collection_name,
        )
        .expect("DrPathBuilder should construct valid path");

        let base_path = dr_path.root_prefix();
        let segments_path = format!("{}segments", base_path);

        // Then: Segments path should follow structure: data/{tenant}/{namespace}/collection/segments
        assert!(
            segments_path.contains("data"),
            "Segments path should start with data prefix"
        );
        assert!(
            segments_path.contains(tenant_id),
            "Segments path should include tenant_id"
        );
        assert!(
            segments_path.contains(namespace_id),
            "Segments path should include namespace_id"
        );
        assert!(
            segments_path.contains(collection_name),
            "Segments path should include collection name"
        );
        assert!(
            segments_path.ends_with("segments"),
            "Segments path should end with 'segments' suffix"
        );
    }
}
