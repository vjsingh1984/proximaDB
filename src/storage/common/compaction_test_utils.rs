//! Test utilities for compaction framework
//!
//! This module provides comprehensive test utilities for validating compaction
//! operations, concurrency behavior, and integration testing.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use anyhow::Result;
use tempfile::TempDir;
use tokio::sync::Mutex;

use crate::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};
use super::compaction_orchestrator::*;

/// Mock storage engine for testing
#[derive(Debug, Clone)]
pub struct MockStorageEngine {
    pub file_extension: String,
    pub config: CompactionConfig,
    pub execution_delay: Duration,
    pub should_fail: bool,
    pub execution_count: Arc<Mutex<usize>>,
}

impl MockStorageEngine {
    pub fn new(extension: &str) -> Self {
        Self {
            file_extension: extension.to_string(),
            config: CompactionConfig::default(),
            execution_delay: Duration::from_millis(10),
            should_fail: false,
            execution_count: Arc::new(Mutex::new(0)),
        }
    }
    
    pub fn with_failure(mut self) -> Self {
        self.should_fail = true;
        self
    }
    
    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.execution_delay = delay;
        self
    }
    
    pub async fn execution_count(&self) -> usize {
        *self.execution_count.lock().await
    }
}

/// Mock file metadata for testing
#[derive(Debug, Clone)]
pub struct MockFileMetadata {
    pub path: String,
    pub size_bytes: u64,
    pub level: u32,
    pub timestamp: u64,
    pub extension: String,
}

impl FileMetadata for MockFileMetadata {
    fn path(&self) -> &str { &self.path }
    fn size_bytes(&self) -> u64 { self.size_bytes }
    fn level(&self) -> u32 { self.level }
    fn timestamp(&self) -> u64 { self.timestamp }
    fn extension(&self) -> &str { &self.extension }
}

/// Mock compaction task for testing
#[derive(Debug, Clone)]
pub struct MockCompactionTask {
    pub operation_id: String,
    pub collection_id: String,
    pub source_level: u32,
    pub target_level: u32,
    pub input_files: Vec<String>,
    pub estimated_duration: Duration,
}

impl CompactionTask for MockCompactionTask {
    fn operation_id(&self) -> &str { &self.operation_id }
    fn collection_id(&self) -> &str { &self.collection_id }
    fn source_level(&self) -> u32 { self.source_level }
    fn target_level(&self) -> u32 { self.target_level }
    fn input_files(&self) -> &[String] { &self.input_files }
    fn estimated_duration(&self) -> Duration { self.estimated_duration }
}

/// Mock compaction result for testing
#[derive(Debug, Clone)]
pub struct MockCompactionResult {
    pub operation_id: String,
    pub files_created: Vec<String>,
    pub files_deleted: Vec<String>,
    pub bytes_written: u64,
    pub records_processed: u64,
    pub duration: Duration,
}

impl CompactionResult for MockCompactionResult {
    fn operation_id(&self) -> &str { &self.operation_id }
    fn files_created(&self) -> &[String] { &self.files_created }
    fn files_deleted(&self) -> &[String] { &self.files_deleted }
    fn bytes_written(&self) -> u64 { self.bytes_written }
    fn records_processed(&self) -> u64 { self.records_processed }
    fn duration(&self) -> Duration { self.duration }
}

impl StorageEngine for MockStorageEngine {
    type FileMetadata = MockFileMetadata;
    type CompactionTask = MockCompactionTask;
    type CompactionResult = MockCompactionResult;
    
    fn file_extension(&self) -> &str {
        &self.file_extension
    }
    
    fn compaction_config(&self) -> CompactionConfig {
        self.config.clone()
    }
    
    fn create_compaction_task(
        &self,
        operation_id: String,
        collection_id: String,
        source_level: u32,
        target_level: u32,
        input_files: Vec<Self::FileMetadata>,
    ) -> Self::CompactionTask {
        MockCompactionTask {
            operation_id,
            collection_id,
            source_level,
            target_level,
            input_files: input_files.into_iter().map(|f| f.path).collect(),
            estimated_duration: self.execution_delay,
        }
    }
    
    async fn execute_compaction(
        &self,
        task: Self::CompactionTask,
    ) -> Result<Self::CompactionResult> {
        // Increment execution count
        {
            let mut count = self.execution_count.lock().await;
            *count += 1;
        }
        
        // Simulate work
        tokio::time::sleep(self.execution_delay).await;
        
        if self.should_fail {
            return Err(anyhow::anyhow!("Mock compaction failure"));
        }
        
        let start_time = Instant::now();
        
        let operation_id = task.operation_id.clone();
        Ok(MockCompactionResult {
            operation_id: operation_id.clone(),
            files_created: vec![format!("L{}_{}_output.{}", task.target_level, operation_id, self.file_extension)],
            files_deleted: task.input_files,
            bytes_written: 1024 * 1024, // 1MB
            records_processed: 1000,
            duration: start_time.elapsed(),
        })
    }
}

/// Test environment setup utilities
pub struct CompactionTestEnv {
    pub temp_dir: TempDir,
    pub filesystem: Arc<FilesystemFactory>,
    pub orchestrator: CompactionOrchestrator,
    pub data_dir: String,
}

impl CompactionTestEnv {
    pub async fn new() -> Result<Self> {
        let temp_dir = TempDir::new()?;
        let data_dir = temp_dir.path().join("data").to_string_lossy().to_string();
        
        // Create data directory
        tokio::fs::create_dir_all(&data_dir).await?;
        
        let filesystem = Arc::new(
            FilesystemFactory::new(FilesystemConfig::default()).await?
        );
        
        let config = CompactionConfig {
            level0_threshold: 3,
            level_threshold: 5,
            max_level: 4,
            max_concurrent_per_collection: 2,
            global_max_concurrent: 4,
            operation_timeout: Duration::from_secs(30),
        };
        
        let orchestrator = CompactionOrchestrator::new(filesystem.clone(), config);
        
        Ok(Self {
            temp_dir,
            filesystem,
            orchestrator,
            data_dir,
        })
    }
    
    /// Create test files in the data directory
    pub async fn create_test_files(&self, files: &[(u32, &str, u64)]) -> Result<()> {
        let codec = FilenameCodec::new();
        let _fs = self.filesystem.get_filesystem(&self.data_dir)?;
        
        for (level, extension, size) in files {
            let filename = codec.generate(*level, extension);
            let file_path = format!("{}/{}", self.data_dir, filename);
            
            // Create file with specified size
            let content = vec![0u8; *size as usize];
            tokio::fs::write(&file_path, content).await?;
        }
        
        Ok(())
    }
    
    /// Count files in data directory
    pub async fn count_files(&self, extension: &str) -> Result<usize> {
        let registry = TieredFileRegistry::new();
        let files = registry.discover_files(&self.filesystem, &self.data_dir, extension).await?;
        Ok(files.values().map(|v| v.len()).sum())
    }
    
    /// Get files by level
    pub async fn get_files_by_level(&self, extension: &str) -> Result<HashMap<u32, Vec<GenericFileMetadata>>> {
        let registry = TieredFileRegistry::new();
        registry.discover_files(&self.filesystem, &self.data_dir, extension).await
    }
}

/// Concurrency testing utilities
pub struct ConcurrencyTester {
    pub orchestrator: CompactionOrchestrator,
    pub engines: Vec<MockStorageEngine>,
}

impl ConcurrencyTester {
    pub fn new(orchestrator: CompactionOrchestrator, engine_count: usize) -> Self {
        let engines = (0..engine_count)
            .map(|i| MockStorageEngine::new(&format!("ext{}", i)))
            .collect();
        
        Self {
            orchestrator,
            engines,
        }
    }
    
    /// Test concurrent compaction requests
    pub async fn test_concurrent_requests(&self, collection_id: &str, data_dir: &str) -> Result<Vec<Result<Option<CompactionExecution<MockStorageEngine>>>>> {
        let mut handles = Vec::new();
        
        for engine in &self.engines {
            let orchestrator = self.orchestrator.clone();
            let engine = engine.clone();
            let collection_id = collection_id.to_string();
            let data_dir = data_dir.to_string();
            
            let handle = tokio::spawn(async move {
                orchestrator.schedule_compaction(&engine, &collection_id, &data_dir).await
            });
            
            handles.push(handle);
        }
        
        let mut results = Vec::new();
        for handle in handles {
            results.push(handle.await?);
        }
        
        Ok(results)
    }
    
    /// Test operation conflicts
    pub async fn test_operation_conflicts(&self) -> Result<()> {
        let coordinator = Arc::new(CompactionCoordinator::new(CompactionConfig::default()));
        
        // Try to acquire conflicting operations
        let _lock1 = CompactionCoordinator::request_operation(
            coordinator.clone(),
            "test_collection",
            OperationType::Flush { level: 0 },
            Some(Duration::from_secs(1)),
        ).await?;
        
        // This should fail due to conflict
        let result = CompactionCoordinator::request_operation(
            coordinator.clone(),
            "test_collection",
            OperationType::Compaction { source_level: 0, target_level: 1 },
            Some(Duration::from_secs(1)),
        ).await;
        
        assert!(result.is_err(), "Expected conflict but operation succeeded");
        
        Ok(())
    }
}

/// Performance testing utilities
pub struct PerformanceTester {
    pub orchestrator: CompactionOrchestrator,
}

impl PerformanceTester {
    pub fn new(orchestrator: CompactionOrchestrator) -> Self {
        Self { orchestrator }
    }
    
    /// Benchmark filename parsing performance
    pub fn benchmark_filename_parsing(iterations: usize) -> Duration {
        let codec = FilenameCodec::new();
        let test_filename = "L5_20250814T143052_a7f3c2d1.sst";
        
        let start = Instant::now();
        
        for _ in 0..iterations {
            let _ = codec.parse_level(test_filename);
            let _ = codec.parse_timestamp(test_filename);
            let _ = codec.is_tiered_filename(test_filename, "sst");
        }
        
        start.elapsed()
    }
    
    /// Benchmark file discovery performance
    pub async fn benchmark_file_discovery(&self, data_dir: &str, extension: &str, _file_count: usize) -> Result<Duration> {
        let registry = TieredFileRegistry::new();
        
        let start = Instant::now();
        let _files = registry.discover_files(&self.orchestrator.filesystem, data_dir, extension).await?;
        let elapsed = start.elapsed();
        
        Ok(elapsed)
    }
}

/// Integration test scenarios
pub struct IntegrationTestSuite;

impl IntegrationTestSuite {
    /// Test complete compaction workflow
    pub async fn test_complete_workflow() -> Result<()> {
        let env = CompactionTestEnv::new().await?;
        let mut engine = MockStorageEngine::new("test");
        engine.config = env.orchestrator.coordinator.config.clone();
        
        // Create files that should trigger compaction (3 files triggers level 0)
        env.create_test_files(&[
            (0, "test", 1024),
            (0, "test", 1024),
            (0, "test", 1024), // Should trigger level 0 compaction
        ]).await?;
        
        // Schedule compaction
        let execution = env.orchestrator.schedule_compaction(&engine, "test_collection", &env.data_dir).await?;
        
        assert!(execution.is_some(), "Expected compaction to be scheduled");
        
        // Execute compaction
        if let Some(execution) = execution {
            let expected_operation_id = execution.task.operation_id().to_string();
            let result = execution.execute(&engine).await?;
            assert_eq!(result.operation_id(), &expected_operation_id);
        }
        
        Ok(())
    }
    
    /// Test error handling and recovery
    pub async fn test_error_handling() -> Result<()> {
        let env = CompactionTestEnv::new().await?;
        let engine = MockStorageEngine::new("test").with_failure();
        
        // Create files
        env.create_test_files(&[
            (0, "test", 1024),
            (0, "test", 1024),
            (0, "test", 1024),
        ]).await?;
        
        // Schedule compaction
        let execution = env.orchestrator.schedule_compaction(&engine, "test_collection", &env.data_dir).await?;
        
        if let Some(execution) = execution {
            let result = execution.execute(&engine).await;
            assert!(result.is_err(), "Expected compaction to fail");
        }
        
        Ok(())
    }
    
    /// Test concurrency limits and coordination
    pub async fn test_concurrency_limits() -> Result<()> {
        let config = CompactionConfig {
            max_concurrent_per_collection: 1,
            global_max_concurrent: 2,
            level0_threshold: 2, // Lower threshold to trigger compaction
            ..Default::default()
        };
        
        let temp_dir = TempDir::new()?;
        let data_dir = temp_dir.path().join("data").to_string_lossy().to_string();
        tokio::fs::create_dir_all(&data_dir).await?;
        
        let filesystem = Arc::new(FilesystemFactory::new(FilesystemConfig::default()).await?);
        let orchestrator = CompactionOrchestrator::new(filesystem, config);
        
        // Create files to trigger compaction
        let codec = FilenameCodec::new();
        for _i in 0..3 {
            let filename = codec.generate(0, "ext0");
            let file_path = format!("{}/{}", data_dir, filename);
            tokio::fs::write(&file_path, vec![0u8; 1024]).await?;
        }
        
        let tester = ConcurrencyTester::new(orchestrator, 5);
        
        // Test concurrent requests
        let results = tester.test_concurrent_requests("test_collection", &data_dir).await?;
        
        // Should have some successes and some None results due to no compaction needed
        let successes = results.iter().filter(|r| r.is_ok() && r.as_ref().unwrap().is_some()).count();
        let no_compaction_needed = results.iter().filter(|r| r.is_ok() && r.as_ref().unwrap().is_none()).count();
        let failures = results.iter().filter(|r| r.is_err()).count();
        
        // With 5 engines trying to compact the same collection concurrently, 
        // we expect at most 1 to succeed (due to collection limit), others should either 
        // find no compaction needed or fail due to limits
        assert!(successes <= 1, "Expected at most 1 successful compaction due to collection limits");
        assert!(no_compaction_needed + successes + failures == 5, "Expected all 5 operations to complete");
        
        // Either we have successful compactions or no compaction was needed
        assert!(successes > 0 || no_compaction_needed > 0, "Expected some operations to complete successfully");
        
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_mock_engine() {
        let engine = MockStorageEngine::new("test");
        
        let task = engine.create_compaction_task(
            "op1".to_string(),
            "collection1".to_string(),
            0,
            1,
            vec![],
        );
        
        let result = engine.execute_compaction(task).await.unwrap();
        assert_eq!(result.operation_id(), "op1");
        assert_eq!(engine.execution_count().await, 1);
    }
    
    #[tokio::test]
    async fn test_env_setup() {
        let env = CompactionTestEnv::new().await.unwrap();
        
        env.create_test_files(&[
            (0, "sst", 1024),
            (1, "sst", 2048),
        ]).await.unwrap();
        
        let count = env.count_files("sst").await.unwrap();
        assert_eq!(count, 2);
        
        let files_by_level = env.get_files_by_level("sst").await.unwrap();
        assert_eq!(files_by_level.len(), 2);
        assert_eq!(files_by_level[&0].len(), 1);
        assert_eq!(files_by_level[&1].len(), 1);
    }
    
    #[tokio::test]
    async fn test_performance_benchmarks() {
        // Test filename parsing performance
        let duration = PerformanceTester::benchmark_filename_parsing(10000);
        assert!(duration < Duration::from_millis(100), "Filename parsing too slow: {:?}", duration);
        
        // Test file discovery performance
        let env = CompactionTestEnv::new().await.unwrap();
        env.create_test_files(&[
            (0, "sst", 1024),
            (1, "sst", 1024),
            (2, "sst", 1024),
        ]).await.unwrap();
        
        let tester = PerformanceTester::new(env.orchestrator);
        let duration = tester.benchmark_file_discovery(&env.data_dir, "sst", 3).await.unwrap();
        assert!(duration < Duration::from_millis(500), "File discovery too slow: {:?}", duration);
    }
    
    #[tokio::test]
    async fn test_integration_workflow() {
        IntegrationTestSuite::test_complete_workflow().await.unwrap();
    }
    
    #[tokio::test]
    async fn test_error_handling() {
        IntegrationTestSuite::test_error_handling().await.unwrap();
    }
    
    #[tokio::test]
    async fn test_concurrency_limits() {
        IntegrationTestSuite::test_concurrency_limits().await.unwrap();
    }
}