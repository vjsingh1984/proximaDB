// SST1 Format Tests

#[cfg(test)]
mod tests {
    use super::super::super::*;
    use crate::proto::proximadb::{CompressionConfig, CompressionAlgorithm};
    use proximadb::core::serialization::CompressionAlgorithm};
    use crate::storage::engines::sst::readers::unified_sstable_reader::UnifiedSstableReader;
    use crate::storage::persistence::filesystem::FilesystemFactory;
    use tempfile::TempDir;
    use std::sync::Arc;
use tracing::{debug, error, info};

    #[tokio::test]
    async fn test_sst1_magic_bytes_write_and_read() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.sstable");
        let filesystem = Arc::new(FilesystemFactory::new());
        
        // Create test records
        let records = vec![
            SstRecord {
                id: "test1".to_string(),
                vector: vec![1.0, 2.0, 3.0],
                metadata: vec![],
                timestamp: 100,
                updated_at: None,
                expires_at: None,
                version: Some(1),
                is_tombstone: false,
                sequence_number: 1,
                level: 0,
            },
        ];
        
        // Write SSTable with SST1 format
        let writer = SstableWriter::new("test_collection".to_string());
        let result = writer.write_sstable(
            file_path.to_str().unwrap(),
            records.clone(),
            None, // No compression config
            filesystem.clone(),
        ).await.unwrap();
        
        // Verify file was written
        assert!(result.bytes_written > 0);
        
        // Read the file and verify SST1 magic bytes
        let fs = filesystem.get_filesystem("file:///").unwrap();
        let data = fs.read(file_path.to_str().unwrap()).await.unwrap();
        
        // Check magic bytes
        assert_eq!(&data[0..4], b"SST1", "File should start with SST1 magic bytes");
        
        // Now try to read with the reader
        let reader = UnifiedSstableReader::new(filesystem.clone());
        let all_records = reader.read_all_vectors(file_path.to_str().unwrap()).await.unwrap();
        
        assert_eq!(all_records.len(), 1);
        assert_eq!(all_records[0].id, "test1");
    }

    #[tokio::test]
    async fn test_sst1_format_with_compression() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("compressed.sstable");
        let filesystem = Arc::new(FilesystemFactory::new());
        
        // Create more records for better compression
        let mut records = vec![];
        for i in 0..100 {
            records.push(SstRecord {
                id: format!("vec_{}", i),
                vector: vec![i as f32; 128], // Repetitive data compresses well
                metadata: vec![],
                timestamp: i as i64,
                updated_at: None,
                expires_at: None,
                version: Some(1),
                is_tombstone: false,
                sequence_number: i as u64,
                level: 0,
            });
        }
        
        // Write with ZSTD compression
        let compression_config = CompressionConfig {
            sst_compression_algorithm: Some(CompressionAlgorithm::CompressionZstd as i32),
            sst_compression_level: Some(6),
            sst_block_size: Some(16384),
            ..Default::default()
        };
        
        let writer = SstableWriter::new("test_collection".to_string());
        let result = writer.write_sstable(
            file_path.to_str().unwrap(),
            records.clone(),
            Some(compression_config),
            filesystem.clone(),
        ).await.unwrap();
        
        debug!("Compressed SSTable written: {} bytes", result.bytes_written);
        
        // Read and verify
        let reader = UnifiedSstableReader::new(filesystem.clone());
        let all_records = reader.read_all_vectors(file_path.to_str().unwrap()).await.unwrap();
        
        assert_eq!(all_records.len(), 100);
        assert_eq!(all_records[0].id, "vec_0");
        assert_eq!(all_records[99].id, "vec_99");
    }

    #[tokio::test]
    async fn test_reject_non_sst1_format() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("invalid.sstable");
        let filesystem = Arc::new(FilesystemFactory::new());
        
        // Write a file without SST1 magic bytes
        let fs = filesystem.get_filesystem("file:///").unwrap();
        let invalid_data = vec![0u8; 100]; // Invalid data
        fs.write(file_path.to_str().unwrap(), &invalid_data).await.unwrap();
        
        // Try to read - should fail
        let reader = UnifiedSstableReader::new(filesystem.clone());
        let result = reader.read_all_vectors(file_path.to_str().unwrap()).await;
        
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains_hash("SST1") || error_msg.contains_hash("magic bytes"),
                "Error should mention SST1 magic bytes: {}", error_msg);
    }

    #[tokio::test]
    async fn test_different_compression_algorithms() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        let temp_dir = TempDir::new().unwrap();
        let filesystem = Arc::new(FilesystemFactory::new());
        
        // Test data
        let records = vec![
            SstRecord {
                id: "test".to_string(),
                vector: vec![1.0; 1024], // Large vector
                metadata: vec![],
                timestamp: 100,
                updated_at: None,
                expires_at: None,
                version: Some(1),
                is_tombstone: false,
                sequence_number: 1,
                level: 0,
            },
        ];
        
        // Test each compression algorithm
        let algorithms = vec![
            (CompressionAlgorithm::CompressionNone, "none.sstable"),
            (CompressionAlgorithm::CompressionZstd, "zstd.sstable"),
            (CompressionAlgorithm::CompressionLz4, "lz4.sstable"),
            (CompressionAlgorithm::CompressionSnappy, "snappy.sstable"),
        ];
        
        for (algo, filename) in algorithms {
            let file_path = temp_dir.path().join(filename);
            
            let compression_config = CompressionConfig {
                sst_compression_algorithm: Some(algo as i32),
                sst_compression_level: Some(3),
                sst_block_size: Some(8192),
                ..Default::default()
            };
            
            let writer = SstableWriter::new("test_collection".to_string());
            let result = writer.write_sstable(
                file_path.to_str().unwrap(),
                records.clone(),
                Some(compression_config),
                filesystem.clone(),
            ).await.unwrap();
            
            debug!("{:?} compression: {} bytes", algo, result.bytes_written);
            
            // Verify we can read it back
            let reader = UnifiedSstableReader::new(filesystem.clone());
            let read_records = reader.read_all_vectors(file_path.to_str().unwrap()).await.unwrap();
            
            assert_eq!(read_records.len(), 1);
            assert_eq!(read_records[0].id, "test");
            assert_eq!(read_records[0].vector.len(), 1024);
        }
    }

    #[tokio::test]
    async fn test_adaptive_block_sizing() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        let temp_dir = TempDir::new().unwrap();
        let filesystem = Arc::new(FilesystemFactory::new());
        
        // Test different block sizes
        let block_sizes = vec![4096, 8192, 16384, 32768, 65536];
        
        for block_size in block_sizes {
            let file_path = temp_dir.path().join(format!("block_{}.sstable", block_size));
            
            // Create records
            let mut records = vec![];
            for i in 0..50 {
                records.push(SstRecord {
                    id: format!("vec_{}", i),
                    vector: vec![i as f32; 256],
                    metadata: vec![],
                    timestamp: i as i64,
                    updated_at: None,
                    expires_at: None,
                    version: Some(1),
                    is_tombstone: false,
                    sequence_number: i as u64,
                    level: 0,
                });
            }
            
            let compression_config = CompressionConfig {
                sst_compression_algorithm: Some(CompressionAlgorithm::CompressionZstd as i32),
                sst_compression_level: Some(3),
                sst_block_size: Some(block_size),
                adaptive_compression: Some(true),
                ..Default::default()
            };
            
            let writer = SstableWriter::new("test_collection".to_string());
            let result = writer.write_sstable(
                file_path.to_str().unwrap(),
                records.clone(),
                Some(compression_config),
                filesystem.clone(),
            ).await.unwrap();
            
            debug!("Block size {}: {} bytes, {} blocks", 
                     block_size, result.bytes_written, result.blocks_written);
            
            // Verify readability
            let reader = UnifiedSstableReader::new(filesystem.clone());
            let read_records = reader.read_all_vectors(file_path.to_str().unwrap()).await.unwrap();
            assert_eq!(read_records.len(), 50);
        }
    }
}