// Integration tests for backup/restore system

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use proximadb::{
    storage::persistence::filesystem::FilesystemFactory,
    storage::persistence::filesystem::unified::UnifiedCachingFilesystem,
};

// Import from operations module
use proximadb::operations::{
    backup::{BackupConfig, BackupManager, BackupTarget},
    restore::{RestoreConfig, RestoreManager, ValidationResult},
};

/// Test helper to create a test database with sample data
async fn setup_test_database(base_path: &Path) -> anyhow::Result<()> {
    // Create collection directories
    let collection_dir = base_path.join("d1/collections/test_collection");
    tokio::fs::create_dir_all(&collection_dir).await?;

    // Create sample SST file
    let sst_path = collection_dir.join("data.sst");
    let sample_data = vec![0u8; 1024]; // 1KB of data
    tokio::fs::write(&sst_path, sample_data).await?;

    // Create WAL directory
    let wal_dir = base_path.join("wal");
    tokio::fs::create_dir_all(&wal_dir).await?;

    // Create sample WAL file
    let wal_path = wal_dir.join("wal_0001.wal");
    let wal_data = vec![1u8; 512]; // 512 bytes
    tokio::fs::write(&wal_path, wal_data).await?;

    Ok(())
}

/// Test full backup and restore cycle
#[tokio::test]
async fn test_full_backup_restore_cycle() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let base_path = temp_dir.path();

    // Setup test database
    setup_test_database(base_path).await.unwrap();

    // Setup backup manager
    let backup_base = temp_dir.path().join("backups");
    tokio::fs::create_dir_all(&backup_base).await.unwrap();

    let wal_writer = std::sync::Arc::new(tokio::sync::Mutex::new(None));
    let factory = std::sync::Arc::new(FilesystemFactory::create(Default::default()).await.unwrap());
    let base_path_str = base_path.to_string_lossy().to_string();
    let base_fs = factory.get_filesystem(&base_path_str).unwrap();
    let storage = std::sync::Arc::new(UnifiedCachingFilesystem::with_serializer(
        base_fs,
        "backup_test".to_string(),
        "test_engine".to_string(),
        std::sync::Arc::new(
            proximadb::storage::persistence::filesystem::metadata_traits::GenericMetadataSerializer,
        ),
    ));

    let backup_config = BackupConfig {
        enabled: true,
        backup_interval_secs: 3600,
        retention_count: 5,
        target: BackupTarget::Local {
            path: backup_base.clone(),
        },
        compression_enabled: false, // Disable for simplicity
        verify_checksums: true,
    };

    let backup_manager = BackupManager::new(base_path, wal_writer, storage, backup_config).unwrap();

    // Create backup
    let manifest = backup_manager.create_incremental_backup().await.unwrap();

    // Verify backup was created
    assert!(manifest.backup_id.starts_with("backup_"));
    assert_eq!(
        manifest.backup_type,
        proximadb::operations::BackupType::Full
    );
    assert!(!manifest.data_files.is_empty());

    // Setup restore manager with fresh directory
    let restore_dir = temp_dir.path().join("restore");
    tokio::fs::create_dir_all(&restore_dir).await.unwrap();

    let restore_factory =
        std::sync::Arc::new(FilesystemFactory::create(Default::default()).await.unwrap());
    let restore_dir_str = restore_dir.to_string_lossy().to_string();
    let restore_base_fs = restore_factory.get_filesystem(&restore_dir_str).unwrap();
    let restore_storage = std::sync::Arc::new(UnifiedCachingFilesystem::with_serializer(
        restore_base_fs,
        "restore_test".to_string(),
        "test_engine".to_string(),
        std::sync::Arc::new(
            proximadb::storage::persistence::filesystem::metadata_traits::GenericMetadataSerializer,
        ),
    ));

    let restore_config = RestoreConfig {
        verify_checksums: true,
        continue_on_error: false,
        dry_run: false,
        target: BackupTarget::Local { path: backup_base },
    };

    let restore_manager =
        RestoreManager::new(&restore_dir, restore_storage, restore_config).unwrap();

    // Restore from backup
    let restore_result = restore_manager
        .restore_from_backup(&manifest)
        .await
        .unwrap();

    assert!(restore_result.success);
    assert!(restore_result.files_restored > 0);
    assert!(restore_result.bytes_restored > 0);
    assert_eq!(restore_result.checksum_failures, 0);
    assert!(restore_result.errors.is_empty());

    // Verify restored files exist
    let restored_sst = restore_dir.join("d1/collections/test_collection/data.sst");
    assert!(restored_sst.exists());

    let restored_wal = restore_dir.join("wal/wal_0001.wal");
    assert!(restored_wal.exists());
}

/// Test incremental backup
#[tokio::test]
async fn test_incremental_backup() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let base_path = temp_dir.path();

    // Setup test database
    setup_test_database(base_path).await.unwrap();

    let backup_base = temp_dir.path().join("backups");
    tokio::fs::create_dir_all(&backup_base).await.unwrap();

    let wal_writer = std::sync::Arc::new(tokio::sync::Mutex::new(None));
    let factory = std::sync::Arc::new(FilesystemFactory::create(Default::default()).await.unwrap());
    let base_path_str = base_path.to_string_lossy().to_string();
    let base_fs = factory.get_filesystem(&base_path_str).unwrap();
    let storage = std::sync::Arc::new(UnifiedCachingFilesystem::with_serializer(
        base_fs,
        "backup_test".to_string(),
        "test_engine".to_string(),
        std::sync::Arc::new(
            proximadb::storage::persistence::filesystem::metadata_traits::GenericMetadataSerializer,
        ),
    ));

    let backup_config = BackupConfig {
        enabled: true,
        backup_interval_secs: 3600,
        retention_count: 5,
        target: BackupTarget::Local {
            path: backup_base.clone(),
        },
        compression_enabled: false,
        verify_checksums: true,
    };

    let backup_manager = BackupManager::new(base_path, wal_writer, storage, backup_config).unwrap();

    // Create first backup (full)
    let manifest1 = backup_manager.create_incremental_backup().await.unwrap();
    assert_eq!(
        manifest1.backup_type,
        proximadb::operations::BackupType::Full
    );
    assert!(manifest1.previous_backup_id.is_none());

    // Add new file
    let collection_dir = base_path.join("d1/collections/test_collection");
    let new_sst_path = collection_dir.join("data2.sst");
    tokio::fs::write(&new_sst_path, vec![2u8; 1024])
        .await
        .unwrap();

    // Create second backup (incremental)
    let manifest2 = backup_manager.create_incremental_backup().await.unwrap();
    assert_eq!(
        manifest2.backup_type,
        proximadb::operations::BackupType::Incremental
    );
    assert_eq!(
        manifest2.previous_backup_id,
        Some(manifest1.backup_id.clone())
    );

    // Verify second backup includes previous backup ID
    assert!(manifest2.data_files.len() >= 1); // At least the new file
}

/// Test backup retention policy
#[tokio::test]
async fn test_backup_retention() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let base_path = temp_dir.path();

    setup_test_database(base_path).await.unwrap();

    let backup_base = temp_dir.path().join("backups");
    tokio::fs::create_dir_all(&backup_base).await.unwrap();

    let wal_writer = std::sync::Arc::new(tokio::sync::Mutex::new(None));
    let factory = std::sync::Arc::new(FilesystemFactory::create(Default::default()).await.unwrap());
    let base_path_str = base_path.to_string_lossy().to_string();
    let base_fs = factory.get_filesystem(&base_path_str).unwrap();
    let storage = std::sync::Arc::new(UnifiedCachingFilesystem::with_serializer(
        base_fs,
        "backup_test".to_string(),
        "test_engine".to_string(),
        std::sync::Arc::new(
            proximadb::storage::persistence::filesystem::metadata_traits::GenericMetadataSerializer,
        ),
    ));

    let backup_config = BackupConfig {
        enabled: true,
        backup_interval_secs: 3600,
        retention_count: 3, // Keep only 3 backups
        target: BackupTarget::Local {
            path: backup_base.clone(),
        },
        compression_enabled: false,
        verify_checksums: true,
    };

    let backup_manager = BackupManager::new(base_path, wal_writer, storage, backup_config).unwrap();

    // Create 5 backups
    for _ in 0..5 {
        backup_manager.create_incremental_backup().await.unwrap();
    }

    // List backups
    let backups = backup_manager.list_backups().await.unwrap();

    // Should only have 3 backups due to retention policy
    assert!(backups.len() <= 3);
}

/// Test checksum verification during restore
#[tokio::test]
async fn test_checksum_verification() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let base_path = temp_dir.path();

    setup_test_database(base_path).await.unwrap();

    let backup_base = temp_dir.path().join("backups");
    tokio::fs::create_dir_all(&backup_base).await.unwrap();

    let wal_writer = std::sync::Arc::new(tokio::sync::Mutex::new(None));
    let factory = std::sync::Arc::new(FilesystemFactory::create(Default::default()).await.unwrap());
    let base_path_str = base_path.to_string_lossy().to_string();
    let base_fs = factory.get_filesystem(&base_path_str).unwrap();
    let storage = std::sync::Arc::new(UnifiedCachingFilesystem::with_serializer(
        base_fs,
        "backup_test".to_string(),
        "test_engine".to_string(),
        std::sync::Arc::new(
            proximadb::storage::persistence::filesystem::metadata_traits::GenericMetadataSerializer,
        ),
    ));

    let backup_config = BackupConfig {
        enabled: true,
        backup_interval_secs: 3600,
        retention_count: 5,
        target: BackupTarget::Local {
            path: backup_base.clone(),
        },
        compression_enabled: false,
        verify_checksums: true,
    };

    let backup_manager = BackupManager::new(base_path, wal_writer, storage, backup_config).unwrap();

    // Create backup
    let manifest = backup_manager.create_incremental_backup().await.unwrap();

    // Restore with checksum verification enabled
    let restore_dir = temp_dir.path().join("restore");
    tokio::fs::create_dir_all(&restore_dir).await.unwrap();

    let restore_factory =
        std::sync::Arc::new(FilesystemFactory::create(Default::default()).await.unwrap());
    let restore_dir_str = restore_dir.to_string_lossy().to_string();
    let restore_base_fs = restore_factory.get_filesystem(&restore_dir_str).unwrap();
    let restore_storage = std::sync::Arc::new(UnifiedCachingFilesystem::with_serializer(
        restore_base_fs,
        "restore_test".to_string(),
        "test_engine".to_string(),
        std::sync::Arc::new(
            proximadb::storage::persistence::filesystem::metadata_traits::GenericMetadataSerializer,
        ),
    ));

    let restore_config = RestoreConfig {
        verify_checksums: true, // Enable checksum verification
        continue_on_error: false,
        dry_run: false,
        target: BackupTarget::Local { path: backup_base },
    };

    let restore_manager =
        RestoreManager::new(&restore_dir, restore_storage, restore_config).unwrap();

    let restore_result = restore_manager
        .restore_from_backup(&manifest)
        .await
        .unwrap();

    // Should succeed with no checksum failures
    assert!(restore_result.success);
    assert_eq!(restore_result.checksum_failures, 0);
}

/// Test backup validation
#[tokio::test]
async fn test_backup_validation() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let base_path = temp_dir.path();

    setup_test_database(base_path).await.unwrap();

    let backup_base = temp_dir.path().join("backups");
    tokio::fs::create_dir_all(&backup_base).await.unwrap();

    let wal_writer = std::sync::Arc::new(tokio::sync::Mutex::new(None));
    let factory = std::sync::Arc::new(FilesystemFactory::create(Default::default()).await.unwrap());
    let base_path_str = base_path.to_string_lossy().to_string();
    let base_fs = factory.get_filesystem(&base_path_str).unwrap();
    let storage = std::sync::Arc::new(UnifiedCachingFilesystem::with_serializer(
        base_fs,
        "backup_test".to_string(),
        "test_engine".to_string(),
        std::sync::Arc::new(
            proximadb::storage::persistence::filesystem::metadata_traits::GenericMetadataSerializer,
        ),
    ));

    let backup_config = BackupConfig {
        enabled: true,
        backup_interval_secs: 3600,
        retention_count: 5,
        target: BackupTarget::Local {
            path: backup_base.clone(),
        },
        compression_enabled: false,
        verify_checksums: true,
    };

    let backup_manager = BackupManager::new(base_path, wal_writer, storage, backup_config).unwrap();

    // Create backup
    let manifest = backup_manager.create_incremental_backup().await.unwrap();

    // Validate backup
    let restore_factory =
        std::sync::Arc::new(FilesystemFactory::create(Default::default()).await.unwrap());
    let base_path_str2 = base_path.to_string_lossy().to_string();
    let restore_base_fs = restore_factory.get_filesystem(&base_path_str2).unwrap();
    let restore_storage = std::sync::Arc::new(UnifiedCachingFilesystem::with_serializer(
        restore_base_fs,
        "validation_test".to_string(),
        "test_engine".to_string(),
        std::sync::Arc::new(
            proximadb::storage::persistence::filesystem::metadata_traits::GenericMetadataSerializer,
        ),
    ));

    let restore_config = RestoreConfig {
        verify_checksums: true,
        continue_on_error: false,
        dry_run: false,
        target: BackupTarget::Local { path: backup_base },
    };

    let restore_manager = RestoreManager::new(base_path, restore_storage, restore_config).unwrap();

    let validation = restore_manager.validate_backup(&manifest).await.unwrap();

    // Should be valid
    assert!(validation.valid);
    assert!(validation.errors.is_empty());
    assert!(validation.total_files > 0);
}

/// Test restore statistics tracking
#[tokio::test]
async fn test_restore_statistics() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let base_path = temp_dir.path();

    setup_test_database(base_path).await.unwrap();

    let backup_base = temp_dir.path().join("backups");
    tokio::fs::create_dir_all(&backup_base).await.unwrap();

    let wal_writer = std::sync::Arc::new(tokio::sync::Mutex::new(None));
    let factory = std::sync::Arc::new(FilesystemFactory::create(Default::default()).await.unwrap());
    let base_path_str = base_path.to_string_lossy().to_string();
    let base_fs = factory.get_filesystem(&base_path_str).unwrap();
    let storage = std::sync::Arc::new(UnifiedCachingFilesystem::with_serializer(
        base_fs,
        "backup_test".to_string(),
        "test_engine".to_string(),
        std::sync::Arc::new(
            proximadb::storage::persistence::filesystem::metadata_traits::GenericMetadataSerializer,
        ),
    ));

    let backup_config = BackupConfig {
        enabled: true,
        backup_interval_secs: 3600,
        retention_count: 5,
        target: BackupTarget::Local {
            path: backup_base.clone(),
        },
        compression_enabled: false,
        verify_checksums: true,
    };

    let backup_manager = BackupManager::new(base_path, wal_writer, storage, backup_config).unwrap();

    // Create backup
    let manifest = backup_manager.create_incremental_backup().await.unwrap();

    // Check backup statistics
    let backup_stats = backup_manager.stats().await;
    assert_eq!(backup_stats.backups_created, 1);
    assert!(backup_stats.total_bytes_backed_up > 0);
    assert!(backup_stats.last_backup_timestamp.is_some());
    assert!(backup_stats.last_backup_duration_ms.is_some());

    // Restore backup
    let restore_dir = temp_dir.path().join("restore");
    tokio::fs::create_dir_all(&restore_dir).await.unwrap();

    let restore_factory =
        std::sync::Arc::new(FilesystemFactory::create(Default::default()).await.unwrap());
    let restore_dir_str = restore_dir.to_string_lossy().to_string();
    let restore_base_fs = restore_factory.get_filesystem(&restore_dir_str).unwrap();
    let restore_storage = std::sync::Arc::new(UnifiedCachingFilesystem::with_serializer(
        restore_base_fs,
        "restore_test".to_string(),
        "test_engine".to_string(),
        std::sync::Arc::new(
            proximadb::storage::persistence::filesystem::metadata_traits::GenericMetadataSerializer,
        ),
    ));

    let restore_config = RestoreConfig {
        verify_checksums: true,
        continue_on_error: false,
        dry_run: false,
        target: BackupTarget::Local { path: backup_base },
    };

    let restore_manager =
        RestoreManager::new(&restore_dir, restore_storage, restore_config).unwrap();

    restore_manager
        .restore_from_backup(&manifest)
        .await
        .unwrap();

    // Check restore statistics
    let restore_stats = restore_manager.stats().await;
    assert!(restore_stats.files_restored > 0);
    assert!(restore_stats.bytes_restored > 0);
    assert_eq!(restore_stats.checksum_failures, 0);
    assert!(restore_stats.restore_duration_ms.is_some());
}

/// Test dry-run restore
#[tokio::test]
async fn test_dry_run_restore() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let base_path = temp_dir.path();

    setup_test_database(base_path).await.unwrap();

    let backup_base = temp_dir.path().join("backups");
    tokio::fs::create_dir_all(&backup_base).await.unwrap();

    let wal_writer = std::sync::Arc::new(tokio::sync::Mutex::new(None));
    let factory = std::sync::Arc::new(FilesystemFactory::create(Default::default()).await.unwrap());
    let base_path_str = base_path.to_string_lossy().to_string();
    let base_fs = factory.get_filesystem(&base_path_str).unwrap();
    let storage = std::sync::Arc::new(UnifiedCachingFilesystem::with_serializer(
        base_fs,
        "backup_test".to_string(),
        "test_engine".to_string(),
        std::sync::Arc::new(
            proximadb::storage::persistence::filesystem::metadata_traits::GenericMetadataSerializer,
        ),
    ));

    let backup_config = BackupConfig {
        enabled: true,
        backup_interval_secs: 3600,
        retention_count: 5,
        target: BackupTarget::Local {
            path: backup_base.clone(),
        },
        compression_enabled: false,
        verify_checksums: true,
    };

    let backup_manager = BackupManager::new(base_path, wal_writer, storage, backup_config).unwrap();

    // Create backup
    let manifest = backup_manager.create_incremental_backup().await.unwrap();

    // Dry-run restore
    let restore_dir = temp_dir.path().join("restore");
    tokio::fs::create_dir_all(&restore_dir).await.unwrap();

    let restore_factory =
        std::sync::Arc::new(FilesystemFactory::create(Default::default()).await.unwrap());
    let restore_dir_str = restore_dir.to_string_lossy().to_string();
    let restore_base_fs = restore_factory.get_filesystem(&restore_dir_str).unwrap();
    let restore_storage = std::sync::Arc::new(UnifiedCachingFilesystem::with_serializer(
        restore_base_fs,
        "restore_test".to_string(),
        "test_engine".to_string(),
        std::sync::Arc::new(
            proximadb::storage::persistence::filesystem::metadata_traits::GenericMetadataSerializer,
        ),
    ));

    let restore_config = RestoreConfig {
        verify_checksums: true,
        continue_on_error: false,
        dry_run: true, // Enable dry run
        target: BackupTarget::Local { path: backup_base },
    };

    let restore_manager =
        RestoreManager::new(&restore_dir, restore_storage, restore_config).unwrap();

    let restore_result = restore_manager
        .restore_from_backup(&manifest)
        .await
        .unwrap();

    // Should report success but no files actually restored
    assert!(restore_result.success);
    assert!(restore_result.files_restored > 0);

    // Verify no files were actually created
    let restored_sst = restore_dir.join("d1/collections/test_collection/data.sst");
    assert!(!restored_sst.exists());
}

/// Test backup list ordering
#[tokio::test]
async fn test_backup_list_ordering() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let base_path = temp_dir.path();

    setup_test_database(base_path).await.unwrap();

    let backup_base = temp_dir.path().join("backups");
    tokio::fs::create_dir_all(&backup_base).await.unwrap();

    let wal_writer = std::sync::Arc::new(tokio::sync::Mutex::new(None));
    let factory = std::sync::Arc::new(FilesystemFactory::create(Default::default()).await.unwrap());
    let base_path_str = base_path.to_string_lossy().to_string();
    let base_fs = factory.get_filesystem(&base_path_str).unwrap();
    let storage = std::sync::Arc::new(UnifiedCachingFilesystem::with_serializer(
        base_fs,
        "backup_test".to_string(),
        "test_engine".to_string(),
        std::sync::Arc::new(
            proximadb::storage::persistence::filesystem::metadata_traits::GenericMetadataSerializer,
        ),
    ));

    let backup_config = BackupConfig {
        enabled: true,
        backup_interval_secs: 3600,
        retention_count: 10,
        target: BackupTarget::Local {
            path: backup_base.clone(),
        },
        compression_enabled: false,
        verify_checksums: true,
    };

    let backup_manager = BackupManager::new(base_path, wal_writer, storage, backup_config).unwrap();

    // Create 3 backups with small delay between them
    let mut timestamps = Vec::new();
    for _ in 0..3 {
        let manifest = backup_manager.create_incremental_backup().await.unwrap();
        timestamps.push(manifest.timestamp);
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }

    // List backups
    let backups = backup_manager.list_backups().await.unwrap();

    // Should have 3 backups
    assert_eq!(backups.len(), 3);

    // Should be ordered by timestamp descending (most recent first)
    assert!(backups[0].timestamp >= backups[1].timestamp);
    assert!(backups[1].timestamp >= backups[2].timestamp);
}

/// Test continue on error mode
#[tokio::test]
async fn test_continue_on_error() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let base_path = temp_dir.path();

    setup_test_database(base_path).await.unwrap();

    let backup_base = temp_dir.path().join("backups");
    tokio::fs::create_dir_all(&backup_base).await.unwrap();

    let wal_writer = std::sync::Arc::new(tokio::sync::Mutex::new(None));
    let factory = std::sync::Arc::new(FilesystemFactory::create(Default::default()).await.unwrap());
    let base_path_str = base_path.to_string_lossy().to_string();
    let base_fs = factory.get_filesystem(&base_path_str).unwrap();
    let storage = std::sync::Arc::new(UnifiedCachingFilesystem::with_serializer(
        base_fs,
        "backup_test".to_string(),
        "test_engine".to_string(),
        std::sync::Arc::new(
            proximadb::storage::persistence::filesystem::metadata_traits::GenericMetadataSerializer,
        ),
    ));

    let backup_config = BackupConfig {
        enabled: true,
        backup_interval_secs: 3600,
        retention_count: 5,
        target: BackupTarget::Local {
            path: backup_base.clone(),
        },
        compression_enabled: false,
        verify_checksums: true,
    };

    let backup_manager = BackupManager::new(base_path, wal_writer, storage, backup_config).unwrap();

    // Create backup
    let manifest = backup_manager.create_incremental_backup().await.unwrap();

    // Delete one file to simulate corruption
    let backup_path = backup_base.join(&manifest.backup_id);
    let data_file = backup_path.join("d1/collections/test_collection/data.sst");
    if data_file.exists() {
        tokio::fs::remove_file(&data_file).await.unwrap();
    }

    // Restore with continue_on_error enabled
    let restore_dir = temp_dir.path().join("restore");
    tokio::fs::create_dir_all(&restore_dir).await.unwrap();

    let restore_factory =
        std::sync::Arc::new(FilesystemFactory::create(Default::default()).await.unwrap());
    let restore_dir_str = restore_dir.to_string_lossy().to_string();
    let restore_base_fs = restore_factory.get_filesystem(&restore_dir_str).unwrap();
    let restore_storage = std::sync::Arc::new(UnifiedCachingFilesystem::with_serializer(
        restore_base_fs,
        "restore_test".to_string(),
        "test_engine".to_string(),
        std::sync::Arc::new(
            proximadb::storage::persistence::filesystem::metadata_traits::GenericMetadataSerializer,
        ),
    ));

    let restore_config = RestoreConfig {
        verify_checksums: false, // Disable to test continue on error
        continue_on_error: true, // Enable continue on error
        dry_run: false,
        target: BackupTarget::Local { path: backup_base },
    };

    let restore_manager =
        RestoreManager::new(&restore_dir, restore_storage, restore_config).unwrap();

    let restore_result = restore_manager
        .restore_from_backup(&manifest)
        .await
        .unwrap();

    // Should have errors but still report success (continue on error)
    assert!(!restore_result.errors.is_empty());
}
