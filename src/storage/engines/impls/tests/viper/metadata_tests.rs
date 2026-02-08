//! VIPER Engine Metadata Tests
//!
//! Tests for VIPER metadata serialization and codebook sidecar storage.

use std::collections::HashMap;

// Tests from unified_metadata_serializer.rs

#[test]
fn test_viper_metadata_serialization() {
    use crate::storage::engines::impls::viper::unified_metadata_serializer::{
        ClusterInfo, RowGroupMetadata, ViperCachedMetadata, ViperMetadataSerializer,
    };
    use crate::storage::persistence::filesystem::metadata_traits::EngineMetadataSerializer;

    let metadata = ViperCachedMetadata {
        file_path: "/data/viper/collection1.parquet".to_string(),
        total_rows: 1000000,
        row_group_count: 10,
        row_groups: vec![RowGroupMetadata {
            id: 0,
            row_count: 100000,
            file_offset: 0,
            total_byte_size: 1024000,
            compressed_size: 512000,
            centroid: Some(vec![0.1, 0.2, 0.3]),
            radius: Some(0.5),
        }],
        column_stats: HashMap::new(),
        cluster_metadata: Some(vec![ClusterInfo {
            cluster_id: 0,
            centroid: vec![0.1, 0.2, 0.3],
            vector_count: 1000,
            radius: 0.5,
        }]),
        parquet_footer: Some(vec![1, 2, 3, 4]),
        file_size: 10485760,
        last_modified: 1234567890,
    };

    let serializer = ViperMetadataSerializer::new();

    // Test serialization
    let bytes = serializer.serialize(&metadata).unwrap();
    assert!(!bytes.is_empty());

    // Test deserialization
    let deserialized = serializer.deserialize(&bytes).unwrap();
    let restored = deserialized.downcast_ref::<ViperCachedMetadata>().unwrap();

    assert_eq!(restored.file_path, metadata.file_path);
    assert_eq!(restored.total_rows, metadata.total_rows);
    assert_eq!(restored.row_group_count, metadata.row_group_count);
}

#[test]
fn test_parquet_footer_extraction() {
    use crate::storage::engines::impls::viper::unified_metadata_serializer::ViperMetadataSerializer;
    use crate::storage::persistence::filesystem::metadata_traits::EngineMetadataSerializer;

    let serializer = ViperMetadataSerializer::new();

    // Create mock Parquet file data
    let mut data = Vec::new();
    data.extend_from_slice(b"PAR1"); // Magic bytes at start
    data.extend_from_slice(&vec![0u8; 100]); // Some data
    let footer = b"footer_content";
    data.extend_from_slice(footer);
    data.extend_from_slice(&(footer.len() as u32).to_le_bytes()); // Footer size
    data.extend_from_slice(b"PAR1"); // Magic bytes at end

    // Test extraction
    let extracted = serializer.extract_cacheable_component(&data, "test.parquet");
    assert!(extracted.is_some());

    let extracted_bytes = extracted.unwrap();
    assert_eq!(&extracted_bytes[..], footer);
}

#[test]
fn test_should_cache_metadata() {
    use crate::storage::engines::impls::viper::unified_metadata_serializer::ViperMetadataSerializer;
    use crate::storage::persistence::filesystem::metadata_traits::EngineMetadataSerializer;

    let serializer = ViperMetadataSerializer::new();

    assert!(serializer.should_cache_metadata("/data/viper/file.parquet"));
    assert!(serializer.should_cache_metadata("/collections/viper/data.bin"));
    assert!(serializer.should_cache_metadata("cluster_metadata.json"));
    assert!(!serializer.should_cache_metadata("/tmp/random.txt"));
}

// Tests from codebook_sidecar.rs

#[tokio::test]
async fn test_viper_sidecar_write_read() {
    use crate::storage::engines::core::formats::codebook_metadata::QuantizationCodebookMetadata;
    use crate::storage::engines::impls::viper::codebook_sidecar::ViperCodebookSidecarManager;
    use crate::storage::persistence::filesystem::FilesystemFactory;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let config = crate::storage::persistence::filesystem::FilesystemConfig::default();
    let fs_factory = FilesystemFactory::create(config).await.unwrap();
    let filesystem = fs_factory
        .get_unified_caching_filesystem(
            "file:///tmp",
            "test_collection".to_string(),
            "viper".to_string(),
        )
        .unwrap();

    let manager = ViperCodebookSidecarManager::new("test_collection".to_string(), filesystem);

    let parquet_path = temp_dir.path().join("test.parquet");
    let metadata = QuantizationCodebookMetadata {
        collection_id: "test_collection".to_string(),
        binary_codebook: None,
        int8_codebook: None,
        pq_codebooks: Default::default(),
        created_at: 1234567890,
        training_samples: 1000,
        schema_version: 1,
    };

    // Write sidecar
    manager
        .write_sidecar(&parquet_path, &metadata)
        .await
        .unwrap();

    // Read back
    let read_metadata = manager.read_sidecar(&parquet_path).await.unwrap().unwrap();
    assert_eq!(read_metadata.collection_id, metadata.collection_id);
    assert_eq!(read_metadata.training_samples, metadata.training_samples);
}

#[test]
fn test_sidecar_path_generation() {
    use crate::storage::engines::impls::viper::codebook_sidecar::ViperCodebookSidecarManager;
    use std::path::Path;

    let parquet_path = Path::new("/data/collection/segment_001.parquet");
    let sidecar_path = ViperCodebookSidecarManager::sidecar_path(&parquet_path);
    assert_eq!(
        sidecar_path.file_name().unwrap().to_str().unwrap(),
        "segment_001.codebook.json"
    );
}
