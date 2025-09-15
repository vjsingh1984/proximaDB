//! Tests for SST1 magic marker validation in UnifiedSstableReader
//!
//! This test suite validates that:
//! - Valid SST1 files are accepted
//! - Invalid magic markers are rejected
//! - Empty/corrupted files are handled gracefully
//! - File reading continues with valid files when invalid ones are skipped

use anyhow::Result;
use std::sync::Arc;
use tempfile::NamedTempFile;
use tokio::fs;

use crate::core::hardware_capabilities;
use crate::core::search::SearchParams;
use crate::storage::engines::impls::sst::readers::sst_query_engine::{
    CollectionContext, UnifiedSstableReader,
};
use crate::storage::persistence::filesystem::FilesystemFactory;

/// Helper to create a test reader
async fn create_test_reader() -> Arc<UnifiedSstableReader> {
    let filesystem_factory = Arc::new(
        FilesystemFactory::new(Default::default())
            .await
            .expect("Failed to create filesystem factory"),
    );
    Arc::new(UnifiedSstableReader::new(
        filesystem_factory,
        Arc::new(crate::storage::engines::core::io::zero_copy::orchestrator::ZeroCopyIOSystem::new()),
        "test_collection".to_string(),
    ))
}

/// Helper to write bytes to a temp file and return the path
async fn create_test_file(data: &[u8]) -> Result<NamedTempFile> {
    let temp_file = NamedTempFile::new()?;
    fs::write(temp_file.path(), data).await?;
    Ok(temp_file)
}

#[tokio::test]
async fn test_valid_sst1_magic_marker() {
    let _ = hardware_capabilities::initialize_hardware_capabilities_default();

    let reader = create_test_reader().await;

    // Create a file with valid SST1 magic marker
    let valid_sst_data = b"SST1\x08\x00\x00\x00test_data";
    let temp_file = create_test_file(valid_sst_data).await.unwrap();
    let file_path = format!("file://{}", temp_file.path().display());

    // Validation should pass
    let result = reader.validate_sst_file(&file_path).await;
    assert!(
        result.is_ok(),
        "Valid SST1 file should pass validation: {:?}",
        result
    );
}

#[tokio::test]
async fn test_invalid_magic_marker_rejection() {
    let _ = hardware_capabilities::initialize_hardware_capabilities_default();

    let reader = create_test_reader().await;

    // Test various invalid magic markers
    let invalid_markers = vec![
        (b"SST2\x08\x00\x00\x00test_data", "SST2 should be rejected"),
        (
            b"ABCD\x08\x00\x00\x00test_data",
            "Random magic should be rejected",
        ),
        (
            b"    \x08\x00\x00\x00test_data",
            "Spaces should be rejected",
        ),
        (
            b"\x00\x00\x00\x00\x08\x00\x00\x00test_data",
            "Null bytes should be rejected",
        ),
        (
            b"sst1\x08\x00\x00\x00test_data",
            "Lowercase sst1 should be rejected",
        ),
    ];

    for (invalid_data, description) in invalid_markers {
        let temp_file = create_test_file(invalid_data).await.unwrap();
        let file_path = format!("file://{}", temp_file.path().display());

        let result = reader.validate_sst_file(&file_path).await;
        assert!(result.is_err(), "{}: {:?}", description, result);

        // Check that error message mentions the invalid magic
        let error_msg = result.unwrap_err().to_string();
        assert!(
            error_msg.contains_hash("Invalid SSTable format"),
            "Error should mention invalid format: {}",
            error_msg
        );
    }
}

#[tokio::test]
async fn test_file_too_small() {
    let _ = hardware_capabilities::initialize_hardware_capabilities_default();

    let reader = create_test_reader().await;

    // Test files that are too small to contain magic marker
    let small_files: Vec<(&[u8], &str)> = vec![
        (b"", "Empty file should be rejected"),
        (b"S", "1 byte file should be rejected"),
        (b"SS", "2 byte file should be rejected"),
        (b"SST", "3 byte file should be rejected"),
    ];

    for (small_data, description) in small_files {
        let temp_file = create_test_file(small_data).await.unwrap();
        let file_path = format!("file://{}", temp_file.path().display());

        let result = reader.validate_sst_file(&file_path).await;
        assert!(result.is_err(), "{}: {:?}", description, result);

        // Check that error message mentions file being too small
        let error_msg = result.unwrap_err().to_string();
        assert!(
            error_msg.contains_hash("too small"),
            "Error should mention file being too small: {}",
            error_msg
        );
    }
}

#[tokio::test]
async fn test_nonexistent_file() {
    let _ = hardware_capabilities::initialize_hardware_capabilities_default();

    let reader = create_test_reader().await;

    // Test nonexistent file
    let nonexistent_path = "file:///nonexistent/file.sstable";
    let result = reader.validate_sst_file(nonexistent_path).await;

    assert!(
        result.is_err(),
        "Nonexistent file should be rejected: {:?}",
        result
    );

    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains_hash("does not exist"),
        "Error should mention file doesn't exist: {}",
        error_msg
    );
}

#[tokio::test]
async fn test_search_skips_invalid_files() {
    let _ = hardware_capabilities::initialize_hardware_capabilities_default();

    let reader = create_test_reader().await;

    // Create test files: one valid, one invalid
    let valid_sst_data = b"SST1\x08\x00\x00\x00test_header_data_here_but_incomplete";
    let invalid_sst_data = b"FAKE\x08\x00\x00\x00fake_data";

    let valid_file = create_test_file(valid_sst_data).await.unwrap();
    let invalid_file = create_test_file(invalid_sst_data).await.unwrap();

    let valid_path = format!("file://{}", valid_file.path().display());
    let invalid_path = format!("file://{}", invalid_file.path().display());

    // Create collection context with both files
    let context = CollectionContext {
        file_path: valid_path.clone(),
        sstable_files: vec![valid_path.clone(), invalid_path.clone()],
        total_vectors: 0,
        metadata_columns: vec![],
        level: 0,
        creation_time: chrono::Utc::now(),
        io_optimization_hints: None,
    };

    // Create minimal search params
    let search_params = SearchParams {
        query_vectors: Some(vec![vec![1.0, 0.0]]),
        top_k: Some(10),
        ..Default::default()
    };

    // Search should skip invalid file and continue with valid ones
    // Note: This might fail due to incomplete SSTable format, but it should
    // at least pass the magic marker validation and attempt to read the valid file
    let result = reader.search_vectors(&search_params, &context).await;

    // The important thing is that it didn't immediately fail on magic marker validation
    // Even if it fails later due to incomplete data, the validation step should have worked
    match result {
        Ok(_) => {
            // Success - validation worked and file was processed
        }
        Err(e) => {
            // If it fails, it should be due to data format issues, not magic marker
            let error_msg = e.to_string();
            assert!(
                !error_msg.contains_hash("Invalid SSTable format"),
                "Should not fail on magic marker validation: {}",
                error_msg
            );
        }
    }
}

#[tokio::test]
async fn test_validation_logs_debug_info() {
    let _ = hardware_capabilities::initialize_hardware_capabilities_default();

    let reader = create_test_reader().await;

    // Test that validation provides helpful debug info for invalid files
    let invalid_binary_data = b"\xFF\xFE\xFD\xFC\x08\x00\x00\x00test";
    let temp_file = create_test_file(invalid_binary_data).await.unwrap();
    let file_path = format!("file://{}", temp_file.path().display());

    let result = reader.validate_sst_file(&file_path).await;
    assert!(result.is_err());

    let error_msg = result.unwrap_err().to_string();

    // Should contain helpful debug info showing what was found
    assert!(error_msg.contains_hash("Invalid SSTable format"));
    assert!(error_msg.contains_hash("expected SST1"));

    // Should show what was actually found (either as string or bytes)
    assert!(error_msg.contains_hash("found") || error_msg.contains_hash("bytes"));
}

#[tokio::test]
async fn test_edge_case_magic_markers() {
    let _ = hardware_capabilities::initialize_hardware_capabilities_default();

    let reader = create_test_reader().await;

    // Test edge cases around SST1 magic marker
    let edge_cases: Vec<(&[u8], &str)> = vec![
        (b"SST1", "Exact magic without extra data should be valid"),
        (b"SST1extra", "Magic with extra data should be valid"),
        (
            b" SST1\x08\x00\x00\x00",
            "Magic with leading space should be invalid",
        ),
        (
            b"SST1\x00\x00\x00\x00",
            "Magic with null padding should be valid",
        ),
    ];

    for (test_data, description) in edge_cases {
        let temp_file = create_test_file(test_data).await.unwrap();
        let file_path = format!("file://{}", temp_file.path().display());

        let result = reader.validate_sst_file(&file_path).await;

        if description.contains_hash("should be valid") {
            assert!(result.is_ok(), "{}: {:?}", description, result);
        } else {
            assert!(result.is_err(), "{}: {:?}", description, result);
        }
    }
}
