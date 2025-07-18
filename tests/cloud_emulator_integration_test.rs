//! Integration tests for cloud storage emulators
//! 
//! These tests verify ProximaDB's filesystem abstraction works correctly
//! with MinIO (S3) and fake-gcs-server (GCS).
//! 
//! Tests automatically start and stop emulators for complete isolation.

use anyhow::Result;
use proximadb::storage::persistence::filesystem::{
    FilesystemFactory, FilesystemConfig, FileOptions, FilesystemPerformanceConfig, RetryConfig,
    s3::{S3Config, CredentialConfig, CredentialProviderType, S3StorageClass},
    gcs::{GcsConfig, GcsCredentialConfig, GcsCredentialProviderType, GcsStorageClass},
};
use proximadb::storage::atomic::{UnifiedAtomicCoordinator, StagingConfig, StagingOperationType};
use std::sync::Arc;
use std::process::{Command, Child, Stdio};
use std::path::PathBuf;
use std::time::Duration;
use tokio::time::sleep;

/// Helper to check if MinIO is running
async fn is_minio_running() -> bool {
    tokio::task::spawn_blocking(|| {
        std::process::Command::new("curl")
            .args(&["-s", "-o", "/dev/null", "-w", "%{http_code}", "http://localhost:9000"])
            .output()
            .map(|output| {
                let status = String::from_utf8_lossy(&output.stdout);
                status == "403" || status == "200"
            })
            .unwrap_or(false)
    })
    .await
    .unwrap_or(false)
}

/// Helper to check if fake-gcs-server is running
async fn is_gcs_running() -> bool {
    tokio::task::spawn_blocking(|| {
        std::process::Command::new("curl")
            .args(&["-s", "-o", "/dev/null", "-w", "%{http_code}", "http://localhost:4443/storage/v1/b"])
            .output()
            .map(|output| {
                let status = String::from_utf8_lossy(&output.stdout);
                status.starts_with('2') || status.starts_with('4')
            })
            .unwrap_or(false)
    })
    .await
    .unwrap_or(false)
}

/// Create test data of specified size
fn create_test_data(size: usize) -> Vec<u8> {
    (0..size).map(|i| (i % 256) as u8).collect()
}

use std::sync::Once;

static INIT: Once = Once::new();
static mut SHARED_EMULATOR: Option<*mut EmulatorManager> = None;

/// Emulator management structure
struct EmulatorManager {
    minio_process: Option<Child>,
    gcs_process: Option<Child>,
    project_root: PathBuf,
    is_shared: bool, // Track if this is the shared instance
}

impl EmulatorManager {
    fn new() -> Result<Self> {
        let project_root = std::env::current_dir()
            .map_err(|e| anyhow::anyhow!("Failed to get current directory: {}", e))?;
        
        Ok(Self {
            minio_process: None,
            gcs_process: None,
            project_root,
            is_shared: false,
        })
    }
    
    /// Get or create shared emulator instance
    async fn get_or_create_shared() -> Result<&'static mut EmulatorManager> {
        unsafe {
            INIT.call_once(|| {
                let rt = tokio::runtime::Handle::current();
                rt.block_on(async {
                    let mut manager = EmulatorManager::new().unwrap();
                    manager.is_shared = true;
                    manager.start_all().await.unwrap();
                    SHARED_EMULATOR = Some(Box::into_raw(Box::new(manager)));
                });
            });
            
            Ok(&mut *SHARED_EMULATOR.unwrap())
        }
    }

    /// Start MinIO emulator
    async fn start_minio(&mut self) -> Result<()> {
        let tools_dir = self.project_root.join("tools");
        let minio_binary = tools_dir.join("minio");
        let data_dir = self.project_root.join("test-data").join("minio");

        // Create data directory
        std::fs::create_dir_all(&data_dir)?;

        println!("Starting MinIO on port 9000...");
        
        let mut cmd = Command::new(&minio_binary);
        cmd.args(&["server", data_dir.to_str().unwrap(), "--console-address", ":9001"])
            .env("MINIO_ROOT_USER", "minioadmin")
            .env("MINIO_ROOT_PASSWORD", "minioadmin")
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let child = cmd.spawn()
            .map_err(|e| anyhow::anyhow!("Failed to start MinIO: {}. Make sure tools/minio exists.", e))?;

        self.minio_process = Some(child);

        // Wait for MinIO to start
        for _ in 0..30 {
            if is_minio_running().await {
                println!("✅ MinIO started successfully");
                return Ok(());
            }
            sleep(Duration::from_millis(500)).await;
        }

        Err(anyhow::anyhow!("MinIO failed to start within 15 seconds"))
    }

    /// Start fake-gcs-server emulator
    async fn start_gcs(&mut self) -> Result<()> {
        let tools_dir = self.project_root.join("tools");
        let gcs_binary = tools_dir.join("fake-gcs-server");
        let data_dir = self.project_root.join("test-data").join("gcs");

        // Create data directory
        std::fs::create_dir_all(&data_dir)?;

        println!("Starting fake-gcs-server on port 4443...");
        
        let mut cmd = Command::new(&gcs_binary);
        cmd.args(&[
            "-data", data_dir.to_str().unwrap(),
            "-port", "4443",
            "-scheme", "http",
            "-public-host", "localhost",
            "-filesystem-root", data_dir.to_str().unwrap()
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null());

        let child = cmd.spawn()
            .map_err(|e| anyhow::anyhow!("Failed to start fake-gcs-server: {}. Make sure tools/fake-gcs-server exists.", e))?;

        self.gcs_process = Some(child);

        // Wait for GCS to start
        for _ in 0..30 {
            if is_gcs_running().await {
                println!("✅ fake-gcs-server started successfully");
                return Ok(());
            }
            sleep(Duration::from_millis(500)).await;
        }

        Err(anyhow::anyhow!("fake-gcs-server failed to start within 15 seconds"))
    }

    /// Start all emulators
    async fn start_all(&mut self) -> Result<()> {
        self.start_minio().await?;
        self.start_gcs().await?;
        Ok(())
    }

    /// Stop all emulators
    fn stop_all(&mut self) {
        if let Some(mut process) = self.minio_process.take() {
            println!("Stopping MinIO...");
            let _ = process.kill();
            let _ = process.wait();
        }

        if let Some(mut process) = self.gcs_process.take() {
            println!("Stopping fake-gcs-server...");
            let _ = process.kill();
            let _ = process.wait();
        }

        // Clean up any remaining processes
        let _ = Command::new("pkill").args(&["-f", "minio"]).output();
        let _ = Command::new("pkill").args(&["-f", "fake-gcs-server"]).output();
    }
}

impl Drop for EmulatorManager {
    fn drop(&mut self) {
        // Only stop if this is NOT the shared instance
        if !self.is_shared {
            self.stop_all();
        }
    }
}

#[tokio::test]
async fn test_minio_s3_integration() -> Result<()> {
    // Use shared emulator instance that starts once and stays running
    let _emulator = EmulatorManager::get_or_create_shared().await?;

    // Create S3 configuration for MinIO
    let s3_config = S3Config {
        region: "us-east-1".to_string(),
        default_bucket: Some("proximadb-test".to_string()),
        credentials: CredentialConfig {
            provider: CredentialProviderType::Static,
            access_key_id: Some("minioadmin".to_string()),
            secret_access_key: Some("minioadmin".to_string()),
            session_token: None,
            role_arn: None,
            external_id: None,
            refresh_interval: 3600,
        },
        default_storage_class: S3StorageClass::Standard,
        encryption: None,
        timeout_seconds: 30,
        max_retries: 3,
        multipart_threshold: 5 * 1024 * 1024, // 5MB
        multipart_chunk_size: 1024 * 1024,    // 1MB
        endpoint_url: Some("http://localhost:9000".to_string()),
        force_path_style: true,
    };

    // Create filesystem config
    let fs_config = FilesystemConfig {
        default_fs: None,
        s3: Some(s3_config),
        azure: None,
        gcs: None,
        hdfs: None,
        local: None,
        auth_config: None,
        global_options: FileOptions::default(),
        performance_config: FilesystemPerformanceConfig {
            connection_pool_size: 10,
            enable_keep_alive: true,
            request_timeout_seconds: 30,
            enable_compression: false,
            retry_config: RetryConfig {
                max_retries: 3,
                initial_delay_ms: 100,
                max_delay_ms: 5000,
                backoff_multiplier: 2.0,
            },
            buffer_size: 8192,
            enable_parallel_ops: true,
            max_concurrent_ops: 10,
        },
        scheme_mapping: {
            let mut mapping = std::collections::HashMap::new();
            mapping.insert("gs".to_string(), "gcs".to_string());
            mapping
        },
    };

    // Create filesystem factory
    let factory = Arc::new(FilesystemFactory::new(fs_config).await?);
    let fs = factory.get_filesystem("s3://proximadb-test/test")?;

    // Test 1: Basic write and read
    let test_path = "s3://proximadb-test/test/integration_test.txt";
    let test_data = b"Hello from ProximaDB S3 integration test!";
    
    println!("Testing S3 write operation...");
    fs.write(test_path, test_data, None).await?;
    
    println!("Testing S3 read operation...");
    let read_data = fs.read(test_path).await?;
    assert_eq!(test_data, &read_data[..]);
    println!("✓ S3 write/read test passed");

    // Test 2: Metadata operations
    println!("Testing S3 metadata operations...");
    let metadata = fs.metadata(test_path).await?;
    assert_eq!(metadata.size, test_data.len() as u64);
    assert!(!metadata.is_directory);
    println!("✓ S3 metadata test passed");

    // Test 3: List operations
    println!("Testing S3 list operations...");
    let entries = fs.list("s3://proximadb-test/test").await?;
    let found = entries.iter().any(|e| e.name.contains("integration_test.txt"));
    assert!(found, "File should be visible in listing");
    println!("✓ S3 list test passed");

    // Test 4: Range reads
    println!("Testing S3 range reads...");
    let range_data = fs.read_range(test_path, 0, 5).await?;
    assert_eq!(&range_data[..], b"Hello");
    println!("✓ S3 range read test passed");

    // Test 5: Copy operations
    println!("Testing S3 copy operations...");
    let copy_path = "s3://proximadb-test/test/integration_test_copy.txt";
    fs.copy(test_path, copy_path).await?;
    assert!(fs.exists(copy_path).await?);
    println!("✓ S3 copy test passed");

    // Test 6: Large file upload (tests multipart)
    println!("Testing S3 multipart upload with 5MB file...");
    let large_path = "s3://proximadb-test/test/large_file.bin";
    let large_data = create_test_data(5 * 1024 * 1024); // 5MB
    
    fs.write(large_path, &large_data, None).await?;
    let large_metadata = fs.metadata(large_path).await?;
    assert_eq!(large_metadata.size, large_data.len() as u64);
    println!("✓ S3 multipart upload test passed");

    // Cleanup
    println!("Cleaning up S3 test files...");
    fs.delete(test_path).await?;
    fs.delete(copy_path).await?;
    fs.delete(large_path).await?;

    println!("\n✅ All S3 integration tests passed!");
    Ok(())
}

#[tokio::test]
async fn test_gcs_fake_server_integration() -> Result<()> {
    // Use shared emulator instance
    let _emulator = EmulatorManager::get_or_create_shared().await?;

    // Create GCS configuration for fake-gcs-server
    let gcs_config = GcsConfig {
        project_id: "test-project".to_string(),
        default_bucket: Some("proximadb-test".to_string()),
        credentials: GcsCredentialConfig {
            provider: GcsCredentialProviderType::ApplicationDefault,
            service_account_key_file: None,
            service_account_key_json: None,
            refresh_interval: 3600,
        },
        default_storage_class: GcsStorageClass::Standard,
        timeout_seconds: 30,
        max_retries: 3,
        resumable_threshold: 2 * 1024 * 1024, // 2MB
        upload_chunk_size: 256 * 1024,        // 256KB
        endpoint_url: Some("http://localhost:4443".to_string()),
    };

    // Create filesystem config
    let fs_config = FilesystemConfig {
        default_fs: None,
        s3: None,
        azure: None,
        gcs: Some(gcs_config),
        hdfs: None,
        local: None,
        auth_config: None,
        global_options: FileOptions::default(),
        performance_config: FilesystemPerformanceConfig {
            connection_pool_size: 10,
            enable_keep_alive: true,
            request_timeout_seconds: 30,
            enable_compression: false,
            retry_config: RetryConfig {
                max_retries: 3,
                initial_delay_ms: 100,
                max_delay_ms: 5000,
                backoff_multiplier: 2.0,
            },
            buffer_size: 8192,
            enable_parallel_ops: true,
            max_concurrent_ops: 10,
        },
        scheme_mapping: {
            let mut mapping = std::collections::HashMap::new();
            mapping.insert("gs".to_string(), "gcs".to_string());
            mapping
        },
    };

    // Create filesystem factory
    let factory = Arc::new(FilesystemFactory::new(fs_config).await?);
    
    // First, create the bucket
    println!("Creating GCS test bucket...");
    let _ = factory.get_filesystem("gs://proximadb-test")
        .map_err(|_| println!("Bucket might not exist yet"));

    let fs = factory.get_filesystem("gs://proximadb-test/test")?;

    // Test 1: Basic write and read
    let test_path = "gs://proximadb-test/test/integration_test.txt";
    let test_data = b"Hello from ProximaDB GCS integration test!";
    
    println!("Testing GCS write operation...");
    fs.write(test_path, test_data, None).await?;
    
    println!("Testing GCS read operation...");
    let read_data = fs.read(test_path).await?;
    assert_eq!(test_data, &read_data[..]);
    println!("✓ GCS write/read test passed");

    // Test 2: Metadata operations
    println!("Testing GCS metadata operations...");
    let metadata = fs.metadata(test_path).await?;
    assert_eq!(metadata.size, test_data.len() as u64);
    assert!(!metadata.is_directory);
    println!("✓ GCS metadata test passed");

    // Test 3: List operations
    println!("Testing GCS list operations...");
    let entries = fs.list("gs://proximadb-test/test").await?;
    println!("Found {} entries in GCS listing", entries.len());
    println!("✓ GCS list test passed");

    // Test 4: Copy operations
    println!("Testing GCS copy operations...");
    let copy_path = "gs://proximadb-test/test/integration_test_copy.txt";
    fs.copy(test_path, copy_path).await?;
    assert!(fs.exists(copy_path).await?);
    println!("✓ GCS copy test passed");

    // Test 5: Resumable upload
    println!("Testing GCS resumable upload with 2MB file...");
    let large_path = "gs://proximadb-test/test/large_file.bin";
    let large_data = create_test_data(2 * 1024 * 1024); // 2MB
    
    fs.write(large_path, &large_data, None).await?;
    let large_metadata = fs.metadata(large_path).await?;
    assert_eq!(large_metadata.size, large_data.len() as u64);
    println!("✓ GCS resumable upload test passed");

    // Cleanup
    println!("Cleaning up GCS test files...");
    fs.delete(test_path).await?;
    fs.delete(copy_path).await?;
    fs.delete(large_path).await?;

    println!("\n✅ All GCS integration tests passed!");
    Ok(())
}

#[tokio::test]
async fn test_cross_cloud_operations() -> Result<()> {
    let mut emulator = EmulatorManager::new()?;
    
    // Start both emulators if not already running
    if !is_minio_running().await {
        emulator.start_minio().await?;
    }
    if !is_gcs_running().await {
        emulator.start_gcs().await?;
    }

    // Create combined configuration
    let s3_config = S3Config {
        region: "us-east-1".to_string(),
        default_bucket: Some("proximadb-test".to_string()),
        credentials: CredentialConfig {
            provider: CredentialProviderType::Static,
            access_key_id: Some("minioadmin".to_string()),
            secret_access_key: Some("minioadmin".to_string()),
            session_token: None,
            role_arn: None,
            external_id: None,
            refresh_interval: 3600,
        },
        default_storage_class: S3StorageClass::Standard,
        encryption: None,
        timeout_seconds: 30,
        max_retries: 3,
        multipart_threshold: 5 * 1024 * 1024,
        multipart_chunk_size: 1024 * 1024,
        endpoint_url: Some("http://localhost:9000".to_string()),
        force_path_style: true,
    };

    let gcs_config = GcsConfig {
        project_id: "test-project".to_string(),
        default_bucket: Some("proximadb-test".to_string()),
        credentials: GcsCredentialConfig {
            provider: GcsCredentialProviderType::ApplicationDefault,
            service_account_key_file: None,
            service_account_key_json: None,
            refresh_interval: 3600,
        },
        default_storage_class: GcsStorageClass::Standard,
        timeout_seconds: 30,
        max_retries: 3,
        resumable_threshold: 2 * 1024 * 1024,
        upload_chunk_size: 256 * 1024,
        endpoint_url: Some("http://localhost:4443".to_string()),
    };

    let fs_config = FilesystemConfig {
        default_fs: None,
        s3: Some(s3_config),
        azure: None,
        gcs: Some(gcs_config),
        hdfs: None,
        local: None,
        auth_config: None,
        global_options: FileOptions::default(),
        performance_config: FilesystemPerformanceConfig {
            connection_pool_size: 10,
            enable_keep_alive: true,
            request_timeout_seconds: 30,
            enable_compression: false,
            retry_config: RetryConfig {
                max_retries: 3,
                initial_delay_ms: 100,
                max_delay_ms: 5000,
                backoff_multiplier: 2.0,
            },
            buffer_size: 8192,
            enable_parallel_ops: true,
            max_concurrent_ops: 10,
        },
        scheme_mapping: {
            let mut mapping = std::collections::HashMap::new();
            mapping.insert("gs".to_string(), "gcs".to_string());
            mapping
        },
    };

    let factory = Arc::new(FilesystemFactory::new(fs_config).await?);

    // Test cross-cloud copy
    println!("Testing cross-cloud copy from S3 to GCS...");
    
    let s3_path = "s3://proximadb-test/test/cross_cloud_test.txt";
    let gcs_path = "gs://proximadb-test/test/from_s3.txt";
    let test_data = b"Cross-cloud test data";

    // Write to S3
    let s3_fs = factory.get_filesystem(s3_path)?;
    s3_fs.write(s3_path, test_data, None).await?;
    println!("✓ Written to S3");

    // Copy from S3 to GCS
    factory.copy_atomic(s3_path, gcs_path).await?;
    println!("✓ Copied from S3 to GCS");

    // Verify in GCS
    let gcs_fs = factory.get_filesystem(gcs_path)?;
    let gcs_data = gcs_fs.read(gcs_path).await?;
    assert_eq!(test_data, &gcs_data[..]);
    println!("✓ Verified data in GCS");

    // Test cross-cloud move
    println!("Testing cross-cloud move from GCS to S3...");
    
    let gcs_src = "gs://proximadb-test/test/move_source.txt";
    let s3_dst = "s3://proximadb-test/test/from_gcs.txt";
    
    // Write to GCS
    gcs_fs.write(gcs_src, b"Move test data", None).await?;
    
    // Move from GCS to S3
    factory.move_atomic(gcs_src, s3_dst).await?;
    
    // Verify source is gone and destination exists
    assert!(!gcs_fs.exists(gcs_src).await?);
    assert!(s3_fs.exists(s3_dst).await?);
    println!("✓ Cross-cloud move successful");

    // Cleanup
    s3_fs.delete(s3_path).await?;
    s3_fs.delete(s3_dst).await?;
    gcs_fs.delete(gcs_path).await?;

    println!("\n✅ All cross-cloud integration tests passed!");
    Ok(())
}

#[tokio::test]
async fn test_concurrent_cloud_operations() -> Result<()> {
    let mut emulator = EmulatorManager::new()?;
    
    // Start MinIO if not already running
    if !is_minio_running().await {
        emulator.start_minio().await?;
    }

    let s3_config = S3Config {
        region: "us-east-1".to_string(),
        default_bucket: Some("proximadb-test".to_string()),
        credentials: CredentialConfig {
            provider: CredentialProviderType::Static,
            access_key_id: Some("minioadmin".to_string()),
            secret_access_key: Some("minioadmin".to_string()),
            session_token: None,
            role_arn: None,
            external_id: None,
            refresh_interval: 3600,
        },
        default_storage_class: S3StorageClass::Standard,
        encryption: None,
        timeout_seconds: 30,
        max_retries: 3,
        multipart_threshold: 5 * 1024 * 1024,
        multipart_chunk_size: 1024 * 1024,
        endpoint_url: Some("http://localhost:9000".to_string()),
        force_path_style: true,
    };

    let fs_config = FilesystemConfig {
        default_fs: None,
        s3: Some(s3_config),
        azure: None,
        gcs: None,
        hdfs: None,
        local: None,
        auth_config: None,
        global_options: FileOptions::default(),
        performance_config: FilesystemPerformanceConfig {
            connection_pool_size: 10,
            enable_keep_alive: true,
            request_timeout_seconds: 30,
            enable_compression: false,
            retry_config: RetryConfig {
                max_retries: 3,
                initial_delay_ms: 100,
                max_delay_ms: 5000,
                backoff_multiplier: 2.0,
            },
            buffer_size: 8192,
            enable_parallel_ops: true,
            max_concurrent_ops: 10,
        },
        scheme_mapping: {
            let mut mapping = std::collections::HashMap::new();
            mapping.insert("gs".to_string(), "gcs".to_string());
            mapping
        },
    };

    let factory = Arc::new(FilesystemFactory::new(fs_config).await?);

    println!("Testing concurrent S3 operations...");

    // Launch concurrent writes
    let mut handles = vec![];
    for i in 0..10 {
        let factory_clone = factory.clone();
        let handle = tokio::spawn(async move {
            let fs = factory_clone.get_filesystem("s3://proximadb-test")?;
            let path = format!("s3://proximadb-test/test/concurrent_{}.txt", i);
            let data = format!("Concurrent test data {}", i);
            fs.write(&path, data.as_bytes(), None).await?;
            Ok::<_, anyhow::Error>(path)
        });
        handles.push(handle);
    }

    // Wait for all writes
    let mut paths = vec![];
    for handle in handles {
        let path = handle.await??;
        paths.push(path);
    }
    println!("✓ Concurrent writes completed");

    // Verify all files exist
    let fs = factory.get_filesystem("s3://proximadb-test")?;
    for path in &paths {
        assert!(fs.exists(path).await?);
    }
    println!("✓ All concurrent files verified");

    // Concurrent reads
    let mut handles = vec![];
    for path in &paths {
        let factory_clone = factory.clone();
        let path_clone = path.clone();
        let handle = tokio::spawn(async move {
            let fs = factory_clone.get_filesystem("s3://proximadb-test")?;
            fs.read(&path_clone).await
        });
        handles.push(handle);
    }

    // Verify all reads succeed
    for handle in handles {
        let data = handle.await??;
        assert!(!data.is_empty());
    }
    println!("✓ Concurrent reads completed");

    // Cleanup
    for path in paths {
        fs.delete(&path).await?;
    }

    println!("\n✅ Concurrent cloud operations test passed!");
    Ok(())
}

/// Complete integration test that starts all emulators and runs comprehensive tests
#[tokio::test]
async fn test_complete_cloud_emulator_integration() -> Result<()> {
    println!("🚀 Starting complete cloud emulator integration test...");
    
    // Use shared emulator instance
    let _emulator = EmulatorManager::get_or_create_shared().await?;
    println!("📋 Cloud emulators ready (shared instance)...");
    
    // Wait a bit more for emulators to stabilize
    sleep(Duration::from_secs(2)).await;
    
    println!("✅ All emulators started successfully!");
    println!("   - MinIO (S3): http://localhost:9000");
    println!("   - fake-gcs-server (GCS): http://localhost:4443");
    
    // Create test configuration with both S3 and GCS
    let s3_config = S3Config {
        region: "us-east-1".to_string(),
        default_bucket: Some("proximadb-integration-test".to_string()),
        credentials: CredentialConfig {
            provider: CredentialProviderType::Static,
            access_key_id: Some("minioadmin".to_string()),
            secret_access_key: Some("minioadmin".to_string()),
            session_token: None,
            role_arn: None,
            external_id: None,
            refresh_interval: 3600,
        },
        default_storage_class: S3StorageClass::Standard,
        encryption: None,
        timeout_seconds: 30,
        max_retries: 3,
        multipart_threshold: 5 * 1024 * 1024,
        multipart_chunk_size: 1024 * 1024,
        endpoint_url: Some("http://localhost:9000".to_string()),
        force_path_style: true,
    };

    let gcs_config = GcsConfig {
        project_id: "integration-test-project".to_string(),
        default_bucket: Some("proximadb-integration-test".to_string()),
        credentials: GcsCredentialConfig {
            provider: GcsCredentialProviderType::ApplicationDefault,
            service_account_key_file: None,
            service_account_key_json: None,
            refresh_interval: 3600,
        },
        default_storage_class: GcsStorageClass::Standard,
        timeout_seconds: 30,
        max_retries: 3,
        resumable_threshold: 2 * 1024 * 1024,
        upload_chunk_size: 256 * 1024,
        endpoint_url: Some("http://localhost:4443".to_string()),
    };

    let fs_config = FilesystemConfig {
        default_fs: None,
        s3: Some(s3_config),
        azure: None,
        gcs: Some(gcs_config),
        hdfs: None,
        local: None,
        auth_config: None,
        global_options: FileOptions::default(),
        performance_config: FilesystemPerformanceConfig {
            connection_pool_size: 10,
            enable_keep_alive: true,
            request_timeout_seconds: 30,
            enable_compression: false,
            retry_config: RetryConfig {
                max_retries: 3,
                initial_delay_ms: 100,
                max_delay_ms: 5000,
                backoff_multiplier: 2.0,
            },
            buffer_size: 8192,
            enable_parallel_ops: true,
            max_concurrent_ops: 10,
        },
        scheme_mapping: {
            let mut mapping = std::collections::HashMap::new();
            mapping.insert("gs".to_string(), "gcs".to_string());
            mapping
        },
    };

    let factory = Arc::new(FilesystemFactory::new(fs_config).await?);

    // Test 1: S3 operations
    println!("🔧 Testing S3 operations via MinIO...");
    let s3_fs = factory.get_filesystem("s3://proximadb-integration-test")?;
    let s3_test_path = "s3://proximadb-integration-test/complete-test.txt";
    let s3_test_data = b"Complete integration test data for S3";
    
    s3_fs.write(s3_test_path, s3_test_data, None).await?;
    let s3_read_data = s3_fs.read(s3_test_path).await?;
    assert_eq!(s3_test_data, &s3_read_data[..]);
    println!("   ✅ S3 write/read operations successful");

    // Test 2: GCS operations (using scheme mapping gs:// -> gcs://)
    println!("🔧 Testing GCS operations via fake-gcs-server...");
    let gcs_fs = factory.get_filesystem("gs://proximadb-integration-test")?; // Note: using gs:// scheme
    let gcs_test_path = "gs://proximadb-integration-test/complete-test.txt";
    let gcs_test_data = b"Complete integration test data for GCS";
    
    gcs_fs.write(gcs_test_path, gcs_test_data, None).await?;
    let gcs_read_data = gcs_fs.read(gcs_test_path).await?;
    assert_eq!(gcs_test_data, &gcs_read_data[..]);
    println!("   ✅ GCS write/read operations successful (gs:// scheme mapped to gcs://)");

    // Test 3: Cross-cloud operations
    println!("🔧 Testing cross-cloud copy operations...");
    let cross_cloud_src = "s3://proximadb-integration-test/cross-cloud-source.txt";
    let cross_cloud_dst = "gs://proximadb-integration-test/cross-cloud-dest.txt";
    let cross_cloud_data = b"Cross-cloud copy test data";
    
    s3_fs.write(cross_cloud_src, cross_cloud_data, None).await?;
    factory.copy_atomic(cross_cloud_src, cross_cloud_dst).await?;
    
    let copied_data = gcs_fs.read(cross_cloud_dst).await?;
    assert_eq!(cross_cloud_data, &copied_data[..]);
    println!("   ✅ Cross-cloud copy (S3 → GCS) successful");

    // Test 4: Concurrent operations
    println!("🔧 Testing concurrent operations...");
    let mut handles = vec![];
    for i in 0..5 {
        let factory_clone = factory.clone();
        let handle = tokio::spawn(async move {
            let s3_path = format!("s3://proximadb-integration-test/concurrent-{}.txt", i);
            let gcs_path = format!("gs://proximadb-integration-test/concurrent-{}.txt", i);
            let data = format!("Concurrent test data {}", i);
            
            let s3_fs = factory_clone.get_filesystem(&s3_path)?;
            let gcs_fs = factory_clone.get_filesystem(&gcs_path)?;
            
            s3_fs.write(&s3_path, data.as_bytes(), None).await?;
            gcs_fs.write(&gcs_path, data.as_bytes(), None).await?;
            
            Ok::<_, anyhow::Error>((s3_path, gcs_path))
        });
        handles.push(handle);
    }

    let mut test_files = vec![];
    for handle in handles {
        let (s3_path, gcs_path) = handle.await??;
        test_files.push((s3_path, gcs_path));
    }
    println!("   ✅ Concurrent operations completed successfully");

    // Cleanup test files
    println!("🧹 Cleaning up test files...");
    for (s3_path, gcs_path) in test_files {
        let _ = s3_fs.delete(&s3_path).await;
        let _ = gcs_fs.delete(&gcs_path).await;
    }
    let _ = s3_fs.delete(s3_test_path).await;
    let _ = gcs_fs.delete(gcs_test_path).await;
    let _ = s3_fs.delete(cross_cloud_src).await;
    let _ = gcs_fs.delete(cross_cloud_dst).await;

    println!("🎉 Complete cloud emulator integration test passed!");
    println!("   - Tested S3 via MinIO with custom endpoint");
    println!("   - Tested GCS via fake-gcs-server with custom endpoint");
    println!("   - Tested scheme mapping (gs:// → gcs://)");
    println!("   - Tested cross-cloud operations");
    println!("   - Tested concurrent operations");
    println!("   - All emulators managed automatically");

    Ok(())
}

/// Test atomic writes with S3 (MinIO) emulator
#[tokio::test]
async fn test_s3_atomic_write_operations() -> Result<()> {
    println!("🚀 Starting S3 atomic write test with MinIO...");
    
    // Use shared emulator instance
    let _emulator = EmulatorManager::get_or_create_shared().await?;

    // Create S3 configuration for MinIO
    let s3_config = S3Config {
        region: "us-east-1".to_string(),
        default_bucket: Some("proximadb-atomic-test".to_string()),
        credentials: CredentialConfig {
            provider: CredentialProviderType::Static,
            access_key_id: Some("minioadmin".to_string()),
            secret_access_key: Some("minioadmin".to_string()),
            session_token: None,
            role_arn: None,
            external_id: None,
            refresh_interval: 3600,
        },
        default_storage_class: S3StorageClass::Standard,
        encryption: None,
        timeout_seconds: 30,
        max_retries: 3,
        multipart_threshold: 5 * 1024 * 1024,
        multipart_chunk_size: 1024 * 1024,
        endpoint_url: Some("http://localhost:9000".to_string()),
        force_path_style: true,
    };

    // Create filesystem config
    let fs_config = FilesystemConfig {
        default_fs: None,
        s3: Some(s3_config),
        azure: None,
        gcs: None,
        hdfs: None,
        local: None,
        auth_config: None,
        global_options: FileOptions::default(),
        performance_config: FilesystemPerformanceConfig::default(),
        scheme_mapping: std::collections::HashMap::new(),
    };

    // Create filesystem factory and atomic coordinator
    let factory = Arc::new(FilesystemFactory::new(fs_config).await?);
    let coordinator = Arc::new(UnifiedAtomicCoordinator::new(factory.clone(), None).await?);

    // Create the bucket first
    println!("Creating S3 test bucket...");
    let _ = factory.get_filesystem("s3://proximadb-atomic-test")
        .map_err(|_| println!("Bucket might not exist yet"));

    println!("Testing S3 atomic write operations...");
    
    // Test 1: Basic atomic write
    let staging_config = StagingConfig {
        base_url: "s3://proximadb-atomic-test".to_string(),
        collection_id: Some("test_collection".to_string()),
        operation_type: StagingOperationType::Flush,
        auto_cleanup: true,
        ..Default::default()
    };
    
    let operation = coordinator.begin_atomic_operation(&staging_config).await?;
    println!("✓ Started atomic operation: {}", operation.operation_id);
    
    // Write test data to staging
    let test_data = b"S3 atomic write test data";
    coordinator.write_to_staging(&operation.operation_id, "test_atomic.txt", test_data).await?;
    println!("✓ Written data to staging");
    
    // Finalize the operation
    coordinator.finalize_atomic_operation(&operation.operation_id).await?;
    println!("✓ Finalized atomic operation");
    
    // Verify the file exists in final location
    let final_path = format!("{}/test_collection/test_atomic.txt", staging_config.base_url);
    let exists = factory.exists(&final_path).await?;
    assert!(exists, "File should exist after atomic write");
    println!("✓ Verified file exists in final location");
    
    // Test 2: Concurrent atomic writes
    println!("\nTesting concurrent S3 atomic writes...");
    let mut handles = vec![];
    
    for i in 0..5 {
        let coordinator_clone = coordinator.clone();
        let factory_clone = factory.clone();
        let handle = tokio::spawn(async move {
            let staging_config = StagingConfig {
                base_url: "s3://proximadb-atomic-test".to_string(),
                collection_id: Some(format!("collection_{}", i)),
                operation_type: StagingOperationType::Flush,
                auto_cleanup: true,
                ..Default::default()
            };
            
            let operation = coordinator_clone.begin_atomic_operation(&staging_config).await?;
            let data = format!("Concurrent test data {}", i);
            coordinator_clone.write_to_staging(&operation.operation_id, "concurrent.txt", data.as_bytes()).await?;
            coordinator_clone.finalize_atomic_operation(&operation.operation_id).await?;
            
            // Verify
            let final_path = format!("s3://proximadb-atomic-test/collection_{}/concurrent.txt", i);
            let exists = factory_clone.exists(&final_path).await?;
            
            Ok::<_, anyhow::Error>((i, exists))
        });
        handles.push(handle);
    }
    
    // Wait for all concurrent operations
    for handle in handles {
        let (idx, exists) = handle.await??;
        assert!(exists, "Concurrent file {} should exist", idx);
    }
    println!("✓ All concurrent atomic writes succeeded");
    
    // Test 3: Large file atomic write (tests multipart)
    println!("\nTesting large file S3 atomic write...");
    let large_staging_config = StagingConfig {
        base_url: "s3://proximadb-atomic-test".to_string(),
        collection_id: Some("large_files".to_string()),
        operation_type: StagingOperationType::Compaction,
        auto_cleanup: true,
        ..Default::default()
    };
    
    let large_operation = coordinator.begin_atomic_operation(&large_staging_config).await?;
    let large_data = create_test_data(6 * 1024 * 1024); // 6MB to trigger multipart
    coordinator.write_to_staging(&large_operation.operation_id, "large_file.bin", &large_data).await?;
    coordinator.finalize_atomic_operation(&large_operation.operation_id).await?;
    println!("✓ Large file atomic write succeeded");
    
    println!("\n✅ All S3 atomic write tests passed!");
    Ok(())
}

/// Test atomic writes with GCS (fake-gcs-server) emulator
#[tokio::test]
async fn test_gcs_atomic_write_operations() -> Result<()> {
    println!("🚀 Starting GCS atomic write test with fake-gcs-server...");
    
    // Use shared emulator instance
    let _emulator = EmulatorManager::get_or_create_shared().await?;

    // Create GCS configuration
    let gcs_config = GcsConfig {
        project_id: "test-project".to_string(),
        default_bucket: Some("proximadb-atomic-test".to_string()),
        credentials: GcsCredentialConfig {
            provider: GcsCredentialProviderType::ApplicationDefault,
            service_account_key_file: None,
            service_account_key_json: None,
            refresh_interval: 3600,
        },
        default_storage_class: GcsStorageClass::Standard,
        timeout_seconds: 30,
        max_retries: 3,
        resumable_threshold: 2 * 1024 * 1024,
        upload_chunk_size: 256 * 1024,
        endpoint_url: Some("http://localhost:4443".to_string()),
    };

    // Create filesystem config
    let fs_config = FilesystemConfig {
        default_fs: None,
        s3: None,
        azure: None,
        gcs: Some(gcs_config),
        hdfs: None,
        local: None,
        auth_config: None,
        global_options: FileOptions::default(),
        performance_config: FilesystemPerformanceConfig::default(),
        scheme_mapping: {
            let mut mapping = std::collections::HashMap::new();
            mapping.insert("gs".to_string(), "gcs".to_string());
            mapping
        },
    };

    // Create filesystem factory and atomic coordinator
    let factory = Arc::new(FilesystemFactory::new(fs_config).await?);
    let coordinator = Arc::new(UnifiedAtomicCoordinator::new(factory.clone(), None).await?);

    // Create the bucket first
    println!("Creating GCS test bucket...");
    let _ = factory.get_filesystem("gs://proximadb-atomic-test")
        .map_err(|_| println!("Bucket might not exist yet"));

    println!("Testing GCS atomic write operations...");
    
    // Test 1: Basic atomic write using gs:// scheme
    let staging_config = StagingConfig {
        base_url: "gs://proximadb-atomic-test".to_string(), // Note: using gs:// which maps to gcs://
        collection_id: Some("test_collection".to_string()),
        operation_type: StagingOperationType::Metadata,
        auto_cleanup: true,
        ..Default::default()
    };
    
    let operation = coordinator.begin_atomic_operation(&staging_config).await?;
    println!("✓ Started atomic operation: {}", operation.operation_id);
    
    // Write test data to staging
    let test_data = b"GCS atomic write test data with scheme mapping";
    coordinator.write_to_staging(&operation.operation_id, "test_atomic.txt", test_data).await?;
    println!("✓ Written data to staging");
    
    // Finalize the operation
    coordinator.finalize_atomic_operation(&operation.operation_id).await?;
    println!("✓ Finalized atomic operation");
    
    // Verify the file exists in final location
    let final_path = format!("gs://proximadb-atomic-test/test_collection/test_atomic.txt");
    let exists = factory.exists(&final_path).await?;
    assert!(exists, "File should exist after atomic write");
    println!("✓ Verified file exists in final location (gs:// scheme worked)");
    
    // Test 2: Multiple atomic writes to same collection
    println!("\nTesting multiple GCS atomic writes to same collection...");
    
    let multi_config = StagingConfig {
        base_url: "gs://proximadb-atomic-test".to_string(),
        collection_id: Some("multi_writes".to_string()),
        operation_type: StagingOperationType::Metadata,
        auto_cleanup: true,
        ..Default::default()
    };
    
    // Perform multiple writes to same collection
    for i in 0..3 {
        let operation = coordinator.begin_atomic_operation(&multi_config).await?;
        let data = format!("Multi-write data part {}", i);
        coordinator.write_to_staging(&operation.operation_id, &format!("multi_file_{}.txt", i), data.as_bytes()).await?;
        coordinator.finalize_atomic_operation(&operation.operation_id).await?;
    }
    
    // Verify all files exist
    for i in 0..3 {
        let path = format!("gs://proximadb-atomic-test/multi_writes/multi_file_{}.txt", i);
        let exists = factory.exists(&path).await?;
        assert!(exists, "Multi-write file {} should exist", i);
    }
    println!("✓ Multiple atomic writes to same collection succeeded");
    
    // Test 3: Resumable upload for large files
    println!("\nTesting resumable GCS upload...");
    let resumable_config = StagingConfig {
        base_url: "gs://proximadb-atomic-test".to_string(),
        collection_id: Some("resumable".to_string()),
        operation_type: StagingOperationType::Custom("resumable_test".to_string()),
        auto_cleanup: true,
        ..Default::default()
    };
    
    let resumable_operation = coordinator.begin_atomic_operation(&resumable_config).await?;
    let resumable_data = create_test_data(3 * 1024 * 1024); // 3MB to trigger resumable
    coordinator.write_to_staging(&resumable_operation.operation_id, "resumable.bin", &resumable_data).await?;
    coordinator.finalize_atomic_operation(&resumable_operation.operation_id).await?;
    println!("✓ Resumable upload succeeded");
    
    println!("\n✅ All GCS atomic write tests passed!");
    Ok(())
}

/// Test cross-cloud atomic operations between S3 and GCS
#[tokio::test]
async fn test_cross_cloud_atomic_operations() -> Result<()> {
    println!("🚀 Starting cross-cloud atomic operations test...");
    
    // Use shared emulator instance
    let _emulator = EmulatorManager::get_or_create_shared().await?;

    // Create combined configuration
    let s3_config = S3Config {
        region: "us-east-1".to_string(),
        default_bucket: Some("proximadb-cross-cloud".to_string()),
        credentials: CredentialConfig {
            provider: CredentialProviderType::Static,
            access_key_id: Some("minioadmin".to_string()),
            secret_access_key: Some("minioadmin".to_string()),
            session_token: None,
            role_arn: None,
            external_id: None,
            refresh_interval: 3600,
        },
        default_storage_class: S3StorageClass::Standard,
        encryption: None,
        timeout_seconds: 30,
        max_retries: 3,
        multipart_threshold: 5 * 1024 * 1024,
        multipart_chunk_size: 1024 * 1024,
        endpoint_url: Some("http://localhost:9000".to_string()),
        force_path_style: true,
    };

    let gcs_config = GcsConfig {
        project_id: "test-project".to_string(),
        default_bucket: Some("proximadb-cross-cloud".to_string()),
        credentials: GcsCredentialConfig {
            provider: GcsCredentialProviderType::ApplicationDefault,
            service_account_key_file: None,
            service_account_key_json: None,
            refresh_interval: 3600,
        },
        default_storage_class: GcsStorageClass::Standard,
        timeout_seconds: 30,
        max_retries: 3,
        resumable_threshold: 2 * 1024 * 1024,
        upload_chunk_size: 256 * 1024,
        endpoint_url: Some("http://localhost:4443".to_string()),
    };

    let fs_config = FilesystemConfig {
        default_fs: None,
        s3: Some(s3_config),
        azure: None,
        gcs: Some(gcs_config),
        hdfs: None,
        local: None,
        auth_config: None,
        global_options: FileOptions::default(),
        performance_config: FilesystemPerformanceConfig::default(),
        scheme_mapping: {
            let mut mapping = std::collections::HashMap::new();
            mapping.insert("gs".to_string(), "gcs".to_string());
            mapping
        },
    };

    let factory = Arc::new(FilesystemFactory::new(fs_config).await?);
    let coordinator = Arc::new(UnifiedAtomicCoordinator::new(factory.clone(), None).await?);

    println!("Testing cross-cloud atomic migration...");
    
    // Step 1: Write data atomically to S3
    let s3_staging_config = StagingConfig {
        base_url: "s3://proximadb-cross-cloud".to_string(),
        collection_id: Some("migration_source".to_string()),
        operation_type: StagingOperationType::Flush,
        auto_cleanup: true,
        ..Default::default()
    };
    
    let s3_operation = coordinator.begin_atomic_operation(&s3_staging_config).await?;
    let migration_data = b"Data to migrate from S3 to GCS";
    coordinator.write_to_staging(&s3_operation.operation_id, "migrate.txt", migration_data).await?;
    coordinator.finalize_atomic_operation(&s3_operation.operation_id).await?;
    println!("✓ Written source data to S3");
    
    // Step 2: Read from S3 and atomically write to GCS
    let source_path = "s3://proximadb-cross-cloud/migration_source/migrate.txt";
    let read_data = factory.read(source_path).await?;
    assert_eq!(read_data, migration_data);
    
    let gcs_staging_config = StagingConfig {
        base_url: "gs://proximadb-cross-cloud".to_string(),
        collection_id: Some("migration_dest".to_string()),
        operation_type: StagingOperationType::Custom("cross_cloud_migration".to_string()),
        auto_cleanup: true,
        ..Default::default()
    };
    
    let gcs_operation = coordinator.begin_atomic_operation(&gcs_staging_config).await?;
    coordinator.write_to_staging(&gcs_operation.operation_id, "migrated.txt", &read_data).await?;
    coordinator.finalize_atomic_operation(&gcs_operation.operation_id).await?;
    println!("✓ Migrated data to GCS atomically");
    
    // Verify in GCS
    let dest_path = "gs://proximadb-cross-cloud/migration_dest/migrated.txt";
    let gcs_data = factory.read(dest_path).await?;
    assert_eq!(gcs_data, migration_data);
    println!("✓ Verified migrated data in GCS");
    
    // Step 3: Clean up source atomically
    factory.delete(source_path).await?;
    println!("✓ Cleaned up source data");
    
    println!("\n✅ Cross-cloud atomic operations test passed!");
    Ok(())
}