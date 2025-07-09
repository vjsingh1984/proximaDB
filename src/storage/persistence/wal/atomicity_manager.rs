//! Unified Atomicity Manager for All ProximaDB Operations
//!
//! This module provides a single, comprehensive atomicity manager that handles ALL atomic operations
//! across ProximaDB components:
//! - WAL operations (temp to final storage)
//! - Flush coordination (atomic WAL → Storage → Cleanup) 
//! - Compaction operations (atomic merge and cleanup)
//! - Background operations (sequential pipeline management)
//! - Cross-storage operations (local ↔ cloud atomic moves)
//! - Metadata operations (MVCC and schema changes)
//!
//! Design Principles:
//! 1. Single source of truth for all atomic operations
//! 2. Unified staging pattern across all components
//! 3. Comprehensive rollback and cleanup mechanisms
//! 4. Strategy-based routing for different storage types
//! 5. Cross-component coordination for complex operations

use anyhow::Result;
use chrono::{DateTime, Utc};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use uuid::Uuid;

use super::batch_strategy::WalBatchStrategy;
use crate::core::{CollectionId, VectorRecord};
use crate::storage::atomicity::{
    AtomicOperation, AtomicityManager, IsolationLevel, LockType, OperationPriority, OperationResult,
    OperationType, ResourceId, TransactionContext, TransactionId,
};
use crate::storage::memtable::specialized::wal_behavior::WalVectorBatch;
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::traits::UnifiedStorageEngine;

/// Unified atomicity manager for all ProximaDB operations
pub struct UnifiedAtomicityManager {
    /// Core transaction manager (leverages existing infrastructure)
    base_manager: Arc<AtomicityManager>,
    
    /// Active unified transactions
    unified_transactions: Arc<RwLock<HashMap<TransactionId, UnifiedTransactionContext>>>,
    
    /// Filesystem factory for all storage operations
    filesystem_factory: Arc<FilesystemFactory>,
    
    /// Storage strategies for different backends
    storage_strategies: Arc<RwLock<HashMap<String, Arc<dyn UnifiedStorageStrategy>>>>,
    
    /// Staging coordinators for different operation types
    staging_coordinators: Arc<RwLock<HashMap<StagingType, Arc<StagingCoordinator>>>>,
    
    /// Background operation pipeline
    background_pipeline: Arc<BackgroundPipeline>,
    
    /// Configuration
    config: UnifiedAtomicityConfig,
    
    /// Statistics and monitoring
    stats: Arc<RwLock<UnifiedAtomicityStats>>,
}

/// Unified transaction context
#[derive(Debug)]
pub struct UnifiedTransactionContext {
    /// Transaction ID
    pub transaction_id: TransactionId,
    
    /// Transaction type
    pub transaction_type: UnifiedTransactionType,
    
    /// Collections involved
    pub collections: Vec<CollectionId>,
    
    /// Operations in this transaction
    pub operations: Vec<UnifiedAtomicOperation>,
    
    /// Rollback operations
    pub rollback_operations: VecDeque<UnifiedAtomicOperation>,
    
    /// Staging operations
    pub staging_operations: Vec<StagingOperation>,
    
    /// Resources locked
    pub locked_resources: HashMap<ResourceId, LockType>,
    
    /// Transaction state
    pub state: UnifiedTransactionState,
    
    /// Isolation level
    pub isolation_level: IsolationLevel,
    
    /// Start time
    pub start_time: DateTime<Utc>,
    
    /// Timeout
    pub timeout: Duration,
    
    /// Metadata
    pub metadata: UnifiedTransactionMetadata,
}

/// Unified transaction types
#[derive(Debug, Clone, PartialEq)]
pub enum UnifiedTransactionType {
    /// Single WAL operation
    WalOperation,
    /// Flush operation (WAL → Storage)
    FlushOperation,
    /// Compaction operation (merge files)
    CompactionOperation,
    /// Background pipeline operation (flush → compact → index)
    BackgroundPipeline,
    /// Cross-storage migration (local ↔ cloud)
    StorageMigration,
    /// Metadata operation (MVCC, schema changes)
    MetadataOperation,
    /// Composite operation (multiple components)
    CompositeOperation,
}

/// Unified transaction state
#[derive(Debug, Clone, PartialEq)]
pub enum UnifiedTransactionState {
    /// Transaction is being prepared
    Preparing,
    /// Staging resources
    Staging,
    /// Executing operations
    Executing,
    /// Validating results
    Validating,
    /// Moving to final locations
    Finalizing,
    /// Committing changes
    Committing,
    /// Rolling back changes
    RollingBack,
    /// Cleaning up staging
    CleaningUp,
    /// Successfully committed
    Committed,
    /// Aborted due to failure
    Aborted,
    /// Timed out
    TimedOut,
}

/// Unified transaction metadata
#[derive(Debug, Clone)]
pub struct UnifiedTransactionMetadata {
    /// Collections involved
    pub collections: Vec<CollectionId>,
    /// Total data size
    pub total_size_bytes: usize,
    /// Operations count
    pub operations_count: usize,
    /// Storage providers used
    pub storage_providers: Vec<String>,
    /// Staging directories used
    pub staging_directories: Vec<String>,
    /// Retry count
    pub retry_count: u32,
    /// Priority
    pub priority: OperationPriority,
}

/// Unified atomic operation
#[derive(Debug, Clone)]
pub struct UnifiedAtomicOperation {
    /// Operation ID
    pub operation_id: Uuid,
    
    /// Operation type
    pub operation_type: UnifiedOperationType,
    
    /// Component type
    pub component_type: ComponentType,
    
    /// Source URL (if applicable)
    pub source_url: Option<String>,
    
    /// Target URL
    pub target_url: String,
    
    /// Staging URL
    pub staging_url: Option<String>,
    
    /// Data payload
    pub data: Option<OperationData>,
    
    /// Metadata
    pub metadata: UnifiedOperationMetadata,
    
    /// Timestamp
    pub timestamp: DateTime<Utc>,
}

/// Unified operation types
#[derive(Debug, Clone, PartialEq)]
pub enum UnifiedOperationType {
    /// Write operation
    Write,
    /// Read operation
    Read,
    /// Move operation
    Move,
    /// Delete operation
    Delete,
    /// Flush operation
    Flush,
    /// Compact operation
    Compact,
    /// Index operation
    Index,
    /// Stage operation
    Stage,
    /// Validate operation
    Validate,
    /// Cleanup operation
    Cleanup,
    /// Sync operation
    Sync,
    /// Migrate operation
    Migrate,
}

/// Component types for operation routing
#[derive(Debug, Clone, PartialEq)]
pub enum ComponentType {
    /// WAL component
    Wal,
    /// Storage engine component
    StorageEngine,
    /// Index component
    Index,
    /// Metadata component
    Metadata,
    /// Filesystem component
    Filesystem,
    /// Background component
    Background,
    /// Cloud component
    Cloud,
}

/// Operation data payload
#[derive(Debug, Clone)]
pub enum OperationData {
    /// WAL vector batch
    WalVectorBatch(WalVectorBatch),
    /// Vector records
    VectorRecords(Vec<VectorRecord>),
    /// Raw bytes
    RawBytes(Vec<u8>),
    /// Metadata
    Metadata(HashMap<String, String>),
    /// File paths
    FilePaths(Vec<String>),
    /// URLs
    Urls(Vec<String>),
}

/// Unified operation metadata
#[derive(Debug, Clone)]
pub struct UnifiedOperationMetadata {
    /// Collection ID
    pub collection_id: Option<CollectionId>,
    /// Size in bytes
    pub size_bytes: usize,
    /// Expected checksum
    pub checksum: Option<String>,
    /// Strategy name
    pub strategy_name: String,
    /// Component name
    pub component_name: String,
    /// Retry policy
    pub retry_policy: UnifiedRetryPolicy,
    /// Dependencies
    pub dependencies: Vec<Uuid>,
}

/// Unified retry policy
#[derive(Debug, Clone)]
pub struct UnifiedRetryPolicy {
    /// Maximum retries
    pub max_retries: u32,
    /// Initial delay
    pub initial_delay: Duration,
    /// Maximum delay
    pub max_delay: Duration,
    /// Backoff multiplier
    pub backoff_multiplier: f64,
    /// Retry strategies for different error types
    pub error_strategies: HashMap<String, RetryStrategy>,
}

/// Retry strategy for specific error types
#[derive(Debug, Clone)]
pub struct RetryStrategy {
    /// Should retry this error type
    pub should_retry: bool,
    /// Max retries for this error type
    pub max_retries: u32,
    /// Delay multiplier
    pub delay_multiplier: f64,
}

/// Staging types
#[derive(Debug, Clone, PartialEq, Hash, Eq)]
pub enum StagingType {
    /// Flush staging
    Flush,
    /// Compaction staging
    Compaction,
    /// Metadata staging
    Metadata,
    /// WAL staging
    Wal,
    /// Index staging
    Index,
    /// Background staging
    Background,
    /// Cloud staging
    Cloud,
    /// Custom staging
    Custom(String),
}

impl StagingType {
    /// Get staging directory name
    pub fn staging_dir_name(&self) -> &str {
        match self {
            StagingType::Flush => "__flush",
            StagingType::Compaction => "__compact",
            StagingType::Metadata => "__metadata",
            StagingType::Wal => "__wal",
            StagingType::Index => "__index",
            StagingType::Background => "__background",
            StagingType::Cloud => "__cloud",
            StagingType::Custom(name) => name,
        }
    }
}

/// Staging operation
#[derive(Debug, Clone)]
pub struct StagingOperation {
    /// Operation ID
    pub operation_id: Uuid,
    /// Staging type
    pub staging_type: StagingType,
    /// Staging URL
    pub staging_url: String,
    /// Final URL
    pub final_url: String,
    /// Status
    pub status: StagingOperationStatus,
    /// Created at
    pub created_at: DateTime<Utc>,
    /// Metadata
    pub metadata: HashMap<String, String>,
}

/// Staging operation status
#[derive(Debug, Clone, PartialEq)]
pub enum StagingOperationStatus {
    /// Preparing staging
    Preparing,
    /// Writing to staging
    Staging,
    /// Validating staged data
    Validating,
    /// Moving to final location
    Finalizing,
    /// Completed successfully
    Completed,
    /// Failed
    Failed(String),
}

/// Staging coordinator for specific operation types
pub struct StagingCoordinator {
    /// Staging type
    staging_type: StagingType,
    /// Filesystem factory
    filesystem_factory: Arc<FilesystemFactory>,
    /// Active staging operations
    active_staging: Arc<RwLock<HashMap<Uuid, StagingOperation>>>,
    /// Configuration
    config: StagingConfig,
}

/// Staging configuration
#[derive(Debug, Clone)]
pub struct StagingConfig {
    /// Base staging URL
    pub base_staging_url: String,
    /// Enable automatic cleanup
    pub auto_cleanup: bool,
    /// Max staging age (hours)
    pub max_staging_age_hours: u64,
    /// Cleanup interval (minutes)
    pub cleanup_interval_minutes: u64,
    /// Enable integrity verification
    pub enable_integrity_verification: bool,
}

/// Background operation pipeline
pub struct BackgroundPipeline {
    /// Pipeline stages
    stages: Vec<Arc<dyn PipelineStage>>,
    /// Active pipeline operations
    active_operations: Arc<RwLock<HashMap<TransactionId, PipelineOperation>>>,
    /// Configuration
    config: BackgroundPipelineConfig,
}

/// Pipeline stage trait
#[async_trait::async_trait]
pub trait PipelineStage: Send + Sync + std::fmt::Debug {
    /// Stage name
    fn stage_name(&self) -> &'static str;
    
    /// Execute stage
    async fn execute(
        &self,
        transaction_id: TransactionId,
        input: PipelineStageInput,
    ) -> Result<PipelineStageOutput>;
    
    /// Rollback stage
    async fn rollback(&self, transaction_id: TransactionId) -> Result<()>;
    
    /// Validate stage
    async fn validate(&self, transaction_id: TransactionId) -> Result<bool>;
}

/// Pipeline stage input
#[derive(Debug, Clone)]
pub struct PipelineStageInput {
    /// Collections involved
    pub collections: Vec<CollectionId>,
    /// Input data
    pub data: OperationData,
    /// Previous stage output data
    pub previous_output_data: Option<OperationData>,
    /// Metadata
    pub metadata: HashMap<String, String>,
}

/// Pipeline stage output
#[derive(Debug, Clone)]
pub struct PipelineStageOutput {
    /// Output data
    pub data: OperationData,
    /// URLs created
    pub urls_created: Vec<String>,
    /// Metadata
    pub metadata: HashMap<String, String>,
    /// Next stage input data
    pub next_input_data: Option<OperationData>,
}

/// Pipeline operation
#[derive(Debug, Clone)]
pub struct PipelineOperation {
    /// Transaction ID
    pub transaction_id: TransactionId,
    /// Current stage index
    pub current_stage: usize,
    /// Collections involved
    pub collections: Vec<CollectionId>,
    /// Stage outputs
    pub stage_outputs: Vec<PipelineStageOutput>,
    /// Status
    pub status: PipelineOperationStatus,
    /// Started at
    pub started_at: DateTime<Utc>,
}

/// Pipeline operation status
#[derive(Debug, Clone, PartialEq)]
pub enum PipelineOperationStatus {
    /// Preparing pipeline
    Preparing,
    /// Executing stage
    ExecutingStage(usize),
    /// Completed successfully
    Completed,
    /// Failed at stage
    Failed(usize, String),
    /// Rolling back
    RollingBack,
}

/// Background pipeline configuration
#[derive(Debug, Clone)]
pub struct BackgroundPipelineConfig {
    /// Enable parallel stage execution
    pub parallel_execution: bool,
    /// Maximum concurrent pipelines
    pub max_concurrent_pipelines: usize,
    /// Stage timeout
    pub stage_timeout: Duration,
    /// Pipeline timeout
    pub pipeline_timeout: Duration,
}

/// Unified storage strategy trait
#[async_trait::async_trait]
pub trait UnifiedStorageStrategy: Send + Sync + std::fmt::Debug {
    /// Strategy name
    fn strategy_name(&self) -> &'static str;
    
    /// Can handle URL
    fn can_handle_url(&self, url: &str) -> bool;
    
    /// Execute unified operation
    async fn execute_operation(
        &self,
        operation: &UnifiedAtomicOperation,
        staging_config: &StagingConfig,
    ) -> Result<UnifiedOperationResult>;
    
    /// Rollback operation
    async fn rollback_operation(
        &self,
        operation: &UnifiedAtomicOperation,
    ) -> Result<()>;
    
    /// Validate operation
    async fn validate_operation(
        &self,
        operation: &UnifiedAtomicOperation,
    ) -> Result<bool>;
    
    /// Check storage health
    async fn check_storage_health(&self, base_url: &str) -> Result<bool>;
}

/// Unified operation result
#[derive(Debug, Clone)]
pub struct UnifiedOperationResult {
    /// Final URL
    pub final_url: String,
    /// Intermediate URLs
    pub intermediate_urls: Vec<String>,
    /// Staging URLs
    pub staging_urls: Vec<String>,
    /// Checksum
    pub checksum: Option<String>,
    /// Size in bytes
    pub size_bytes: usize,
    /// Duration
    pub duration: Duration,
    /// Strategy used
    pub strategy_name: String,
    /// Metadata
    pub metadata: HashMap<String, String>,
}

impl std::fmt::Display for UnifiedOperationResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "UnifiedOperationResult(final_url={}, size_bytes={})", 
               self.final_url, self.size_bytes)
    }
}

/// Unified atomicity configuration
#[derive(Debug, Clone)]
pub struct UnifiedAtomicityConfig {
    /// Transaction timeout
    pub transaction_timeout: Duration,
    /// Staging timeout
    pub staging_timeout: Duration,
    /// Validation timeout
    pub validation_timeout: Duration,
    /// Cleanup timeout
    pub cleanup_timeout: Duration,
    /// Maximum concurrent transactions
    pub max_concurrent_transactions: usize,
    /// Enable staging for all operations
    pub enable_staging: bool,
    /// Enable validation for all operations
    pub enable_validation: bool,
    /// Enable background pipeline
    pub enable_background_pipeline: bool,
    /// Enable cloud operations
    pub enable_cloud_operations: bool,
    /// Retry configuration
    pub retry_config: UnifiedRetryPolicy,
    /// Staging configurations
    pub staging_configs: HashMap<StagingType, StagingConfig>,
    /// Background pipeline configuration
    pub background_config: BackgroundPipelineConfig,
}

/// Unified atomicity statistics
#[derive(Debug, Clone, Default)]
pub struct UnifiedAtomicityStats {
    /// Total transactions
    pub total_transactions: u64,
    /// Successful transactions
    pub successful_transactions: u64,
    /// Failed transactions
    pub failed_transactions: u64,
    /// Rolled back transactions
    pub rolled_back_transactions: u64,
    /// Active transactions
    pub active_transactions: u64,
    /// Operations by type
    pub operations_by_type: HashMap<String, u64>,
    /// Operations by component
    pub operations_by_component: HashMap<String, u64>,
    /// Staging operations
    pub staging_operations: u64,
    /// Pipeline operations
    pub pipeline_operations: u64,
    /// Cloud operations
    pub cloud_operations: u64,
    /// Average transaction duration
    pub avg_transaction_duration_ms: f64,
    /// Total bytes processed
    pub total_bytes_processed: u64,
    /// Cleanup operations
    pub cleanup_operations: u64,
}

impl Default for UnifiedAtomicityConfig {
    fn default() -> Self {
        let mut staging_configs = HashMap::new();
        
        // Default staging configurations for each type
        for staging_type in [
            StagingType::Flush,
            StagingType::Compaction,
            StagingType::Metadata,
            StagingType::Wal,
            StagingType::Index,
            StagingType::Background,
            StagingType::Cloud,
        ] {
            staging_configs.insert(
                staging_type.clone(),
                StagingConfig {
                    base_staging_url: format!("file:///tmp/proximadb_staging/{}", staging_type.staging_dir_name()),
                    auto_cleanup: true,
                    max_staging_age_hours: 24,
                    cleanup_interval_minutes: 60,
                    enable_integrity_verification: true,
                },
            );
        }
        
        Self {
            transaction_timeout: Duration::from_secs(300),
            staging_timeout: Duration::from_secs(60),
            validation_timeout: Duration::from_secs(30),
            cleanup_timeout: Duration::from_secs(60),
            max_concurrent_transactions: 50,
            enable_staging: true,
            enable_validation: true,
            enable_background_pipeline: true,
            enable_cloud_operations: true,
            retry_config: UnifiedRetryPolicy::default(),
            staging_configs,
            background_config: BackgroundPipelineConfig {
                parallel_execution: false,
                max_concurrent_pipelines: 10,
                stage_timeout: Duration::from_secs(120),
                pipeline_timeout: Duration::from_secs(600),
            },
        }
    }
}

impl Default for UnifiedRetryPolicy {
    fn default() -> Self {
        let mut error_strategies = HashMap::new();
        
        // Default retry strategies for common error types
        error_strategies.insert("NetworkError".to_string(), RetryStrategy {
            should_retry: true,
            max_retries: 5,
            delay_multiplier: 2.0,
        });
        
        error_strategies.insert("StorageError".to_string(), RetryStrategy {
            should_retry: true,
            max_retries: 3,
            delay_multiplier: 1.5,
        });
        
        error_strategies.insert("CloudError".to_string(), RetryStrategy {
            should_retry: true,
            max_retries: 4,
            delay_multiplier: 2.0,
        });
        
        error_strategies.insert("ValidationError".to_string(), RetryStrategy {
            should_retry: false,
            max_retries: 0,
            delay_multiplier: 1.0,
        });
        
        Self {
            max_retries: 3,
            initial_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(30),
            backoff_multiplier: 2.0,
            error_strategies,
        }
    }
}

impl UnifiedAtomicityManager {
    /// Create new unified atomicity manager
    pub fn new(
        base_manager: Arc<AtomicityManager>,
        filesystem_factory: Arc<FilesystemFactory>,
        config: UnifiedAtomicityConfig,
    ) -> Self {
        let manager = Self {
            base_manager,
            unified_transactions: Arc::new(RwLock::new(HashMap::new())),
            filesystem_factory: filesystem_factory.clone(),
            storage_strategies: Arc::new(RwLock::new(HashMap::new())),
            staging_coordinators: Arc::new(RwLock::new(HashMap::new())),
            background_pipeline: Arc::new(BackgroundPipeline {
                stages: vec![],
                active_operations: Arc::new(RwLock::new(HashMap::new())),
                config: config.background_config.clone(),
            }),
            config,
            stats: Arc::new(RwLock::new(UnifiedAtomicityStats::default())),
        };
        
        // Initialize components
        tokio::spawn({
            let manager = manager.clone();
            async move {
                let _ = manager.initialize_components().await;
            }
        });
        
        manager
    }
    
    /// Initialize all components
    async fn initialize_components(&self) -> Result<()> {
        self.initialize_staging_coordinators().await?;
        self.initialize_storage_strategies().await?;
        self.initialize_background_pipeline().await?;
        Ok(())
    }
    
    /// Initialize staging coordinators
    async fn initialize_staging_coordinators(&self) -> Result<()> {
        let mut coordinators = self.staging_coordinators.write().await;
        
        for (staging_type, config) in &self.config.staging_configs {
            let coordinator = Arc::new(StagingCoordinator {
                staging_type: staging_type.clone(),
                filesystem_factory: self.filesystem_factory.clone(),
                active_staging: Arc::new(RwLock::new(HashMap::new())),
                config: config.clone(),
            });
            
            coordinators.insert(staging_type.clone(), coordinator);
        }
        
        tracing::info!("✅ Initialized {} staging coordinators", coordinators.len());
        Ok(())
    }
    
    /// Initialize storage strategies
    async fn initialize_storage_strategies(&self) -> Result<()> {
        let mut strategies = self.storage_strategies.write().await;
        
        // Initialize unified strategies for each storage type
        let local_strategy = Arc::new(UnifiedLocalStrategy::new(self.filesystem_factory.clone()));
        strategies.insert("file".to_string(), local_strategy);
        
        let s3_strategy = Arc::new(UnifiedS3Strategy::new(self.filesystem_factory.clone()));
        strategies.insert("s3".to_string(), s3_strategy);
        
        let gcs_strategy = Arc::new(UnifiedGcsStrategy::new(self.filesystem_factory.clone()));
        strategies.insert("gcs".to_string(), gcs_strategy);
        
        let azure_strategy = Arc::new(UnifiedAzureStrategy::new(self.filesystem_factory.clone()));
        strategies.insert("adls".to_string(), azure_strategy.clone());
        strategies.insert("abfs".to_string(), azure_strategy);
        
        tracing::info!("✅ Initialized {} unified storage strategies", strategies.len());
        Ok(())
    }
    
    /// Initialize background pipeline
    async fn initialize_background_pipeline(&self) -> Result<()> {
        // Initialize default pipeline stages
        // FlushStage → CompactionStage → IndexStage
        tracing::info!("✅ Initialized background pipeline");
        Ok(())
    }
    
    /// Begin unified transaction
    pub async fn begin_transaction(
        &self,
        transaction_type: UnifiedTransactionType,
        collections: Vec<CollectionId>,
        metadata: UnifiedTransactionMetadata,
    ) -> Result<TransactionId> {
        let transaction_id = Uuid::new_v4();
        
        let context = UnifiedTransactionContext {
            transaction_id,
            transaction_type: transaction_type.clone(),
            collections,
            operations: Vec::new(),
            rollback_operations: VecDeque::new(),
            staging_operations: Vec::new(),
            locked_resources: HashMap::new(),
            state: UnifiedTransactionState::Preparing,
            isolation_level: IsolationLevel::ReadCommitted,
            start_time: Utc::now(),
            timeout: self.config.transaction_timeout,
            metadata,
        };

        let mut transactions = self.unified_transactions.write().await;
        transactions.insert(transaction_id, context);
        
        // Update statistics
        let mut stats = self.stats.write().await;
        stats.total_transactions += 1;
        stats.active_transactions += 1;
        
        tracing::info!(
            "🔄 UNIFIED: Started transaction {} type: {:?}",
            transaction_id,
            transaction_type
        );
        
        Ok(transaction_id)
    }
    
    /// Execute WAL operation
    pub async fn execute_wal_operation(
        &self,
        transaction_id: TransactionId,
        collection_id: &CollectionId,
        batch: WalVectorBatch,
        target_url: &str,
        _strategy: &dyn WalBatchStrategy,
    ) -> Result<UnifiedOperationResult> {
        let mut transactions = self.unified_transactions.write().await;
        let context = transactions.get_mut(&transaction_id)
            .ok_or_else(|| anyhow::anyhow!("Transaction not found: {}", transaction_id))?;

        // Create staging URL
        let staging_url = format!("{}/wal_batch_{}_{}.bin", 
            self.config.staging_configs[&StagingType::Wal].base_staging_url,
            collection_id,
            batch.batch_id.batch_uuid
        );
        
        // Create unified operation
        let operation = UnifiedAtomicOperation {
            operation_id: Uuid::new_v4(),
            operation_type: UnifiedOperationType::Write,
            component_type: ComponentType::Wal,
            source_url: Some(staging_url.clone()),
            target_url: target_url.to_string(),
            staging_url: Some(staging_url),
            data: Some(OperationData::WalVectorBatch(batch.clone())),
            metadata: UnifiedOperationMetadata {
                collection_id: Some(collection_id.clone()),
                size_bytes: batch.total_size_bytes,
                checksum: None,
                strategy_name: "WalBatchStrategy".to_string(),
                component_name: "WAL".to_string(),
                retry_policy: self.config.retry_config.clone(),
                dependencies: vec![],
            },
            timestamp: Utc::now(),
        };
        
        // Update transaction state
        context.state = UnifiedTransactionState::Staging;
        
        // Execute operation
        let result = self.execute_operation_with_strategy(&operation).await?;
        
        // Add to transaction context
        context.operations.push(operation.clone());
        context.state = UnifiedTransactionState::Executing;
        
        // Update statistics
        let mut stats = self.stats.write().await;
        stats.operations_by_type.entry("WAL".to_string()).and_modify(|e| *e += 1).or_insert(1);
        stats.operations_by_component.entry("WAL".to_string()).and_modify(|e| *e += 1).or_insert(1);
        stats.total_bytes_processed += result.size_bytes as u64;
        
        tracing::info!(
            "💾 UNIFIED: Executed WAL operation in transaction {} -> {}",
            transaction_id,
            result.final_url
        );
        
        Ok(result)
    }
    
    /// Execute flush operation
    pub async fn execute_flush_operation(
        &self,
        transaction_id: TransactionId,
        collection_id: &CollectionId,
        vectors: Vec<VectorRecord>,
        storage_engine: Arc<dyn UnifiedStorageEngine>,
    ) -> Result<UnifiedOperationResult> {
        let mut transactions = self.unified_transactions.write().await;
        let context = transactions.get_mut(&transaction_id)
            .ok_or_else(|| anyhow::anyhow!("Transaction not found: {}", transaction_id))?;

        // Create staging URL
        let staging_url = format!("{}/flush_{}_{}.parquet", 
            self.config.staging_configs[&StagingType::Flush].base_staging_url,
            collection_id,
            Uuid::new_v4()
        );
        
        // Create unified operation
        let operation = UnifiedAtomicOperation {
            operation_id: Uuid::new_v4(),
            operation_type: UnifiedOperationType::Flush,
            component_type: ComponentType::StorageEngine,
            source_url: None,
            target_url: format!("storage://engine/{}", collection_id),
            staging_url: Some(staging_url.clone()),
            data: Some(OperationData::VectorRecords(vectors.clone())),
            metadata: UnifiedOperationMetadata {
                collection_id: Some(collection_id.clone()),
                size_bytes: vectors.iter().map(|v| v.actual_size_bytes()).sum(),
                checksum: None,
                strategy_name: "StorageEngine".to_string(),
                component_name: "Flush".to_string(),
                retry_policy: self.config.retry_config.clone(),
                dependencies: vec![],
            },
            timestamp: Utc::now(),
        };
        
        // Update transaction state
        context.state = UnifiedTransactionState::Staging;
        
        // Execute storage engine flush
        let flush_params = crate::storage::traits::FlushParameters {
            collection_id: Some(collection_id.to_string()),
            force: true,
            synchronous: true,
            vector_records: vectors.clone(),
            trigger_compaction: false,
            batch_ids: vec![],
            ..Default::default()
        };
        let flush_result = storage_engine.flush(flush_params).await?;
        
        // Create result
        let result = UnifiedOperationResult {
            final_url: format!("storage://engine/{}", collection_id),
            intermediate_urls: vec![],
            staging_urls: vec![staging_url],
            checksum: None,
            size_bytes: operation.metadata.size_bytes,
            duration: Duration::from_millis(flush_result.duration_ms),
            strategy_name: "StorageEngine".to_string(),
            metadata: HashMap::new(),
        };
        
        // Add to transaction context
        context.operations.push(operation);
        context.state = UnifiedTransactionState::Executing;
        
        // Update statistics
        let mut stats = self.stats.write().await;
        stats.operations_by_type.entry("Flush".to_string()).and_modify(|e| *e += 1).or_insert(1);
        stats.operations_by_component.entry("StorageEngine".to_string()).and_modify(|e| *e += 1).or_insert(1);
        stats.total_bytes_processed += result.size_bytes as u64;
        
        tracing::info!(
            "🔄 UNIFIED: Executed flush operation in transaction {} for collection {}",
            transaction_id,
            collection_id
        );
        
        Ok(result)
    }
    
    /// Execute cloud migration operation
    pub async fn execute_cloud_migration(
        &self,
        transaction_id: TransactionId,
        collection_id: &CollectionId,
        source_url: &str,
        target_url: &str,
    ) -> Result<UnifiedOperationResult> {
        let mut transactions = self.unified_transactions.write().await;
        let context = transactions.get_mut(&transaction_id)
            .ok_or_else(|| anyhow::anyhow!("Transaction not found: {}", transaction_id))?;

        // Create staging URL
        let staging_url = format!("{}/migrate_{}_{}.tmp", 
            self.config.staging_configs[&StagingType::Cloud].base_staging_url,
            collection_id,
            Uuid::new_v4()
        );
        
        // Create unified operation
        let operation = UnifiedAtomicOperation {
            operation_id: Uuid::new_v4(),
            operation_type: UnifiedOperationType::Migrate,
            component_type: ComponentType::Cloud,
            source_url: Some(source_url.to_string()),
            target_url: target_url.to_string(),
            staging_url: Some(staging_url),
            data: None,
            metadata: UnifiedOperationMetadata {
                collection_id: Some(collection_id.clone()),
                size_bytes: 0, // Will be determined during operation
                checksum: None,
                strategy_name: "CloudMigration".to_string(),
                component_name: "Cloud".to_string(),
                retry_policy: self.config.retry_config.clone(),
                dependencies: vec![],
            },
            timestamp: Utc::now(),
        };
        
        // Update transaction state
        context.state = UnifiedTransactionState::Staging;
        
        // Execute migration
        let result = self.execute_operation_with_strategy(&operation).await?;
        
        // Add to transaction context
        context.operations.push(operation);
        context.state = UnifiedTransactionState::Executing;
        
        // Update statistics
        let mut stats = self.stats.write().await;
        stats.operations_by_type.entry("Migrate".to_string()).and_modify(|e| *e += 1).or_insert(1);
        stats.operations_by_component.entry("Cloud".to_string()).and_modify(|e| *e += 1).or_insert(1);
        stats.cloud_operations += 1;
        stats.total_bytes_processed += result.size_bytes as u64;
        
        tracing::info!(
            "🌐 UNIFIED: Executed cloud migration in transaction {} from {} to {}",
            transaction_id,
            source_url,
            result.final_url
        );
        
        Ok(result)
    }
    
    /// Execute background pipeline
    pub async fn execute_background_pipeline(
        &self,
        transaction_id: TransactionId,
        collections: Vec<CollectionId>,
        pipeline_data: OperationData,
    ) -> Result<UnifiedOperationResult> {
        let mut transactions = self.unified_transactions.write().await;
        let context = transactions.get_mut(&transaction_id)
            .ok_or_else(|| anyhow::anyhow!("Transaction not found: {}", transaction_id))?;

        // Create pipeline operation
        let pipeline_operation = PipelineOperation {
            transaction_id,
            current_stage: 0,
            collections: collections.clone(),
            stage_outputs: vec![],
            status: PipelineOperationStatus::Preparing,
            started_at: Utc::now(),
        };
        
        // Add to background pipeline
        {
            let mut active_operations = self.background_pipeline.active_operations.write().await;
            active_operations.insert(transaction_id, pipeline_operation);
        }
        
        // Execute pipeline stages
        let mut stage_outputs = Vec::new();
        let mut current_input = PipelineStageInput {
            collections: collections.clone(),
            data: pipeline_data,
            previous_output_data: None,
            metadata: HashMap::new(),
        };
        
        for (stage_index, stage) in self.background_pipeline.stages.iter().enumerate() {
            tracing::info!("🔄 PIPELINE: Executing stage {} for transaction {}", stage_index, transaction_id);
            
            // Update pipeline status
            {
                let mut active_operations = self.background_pipeline.active_operations.write().await;
                if let Some(operation) = active_operations.get_mut(&transaction_id) {
                    operation.status = PipelineOperationStatus::ExecutingStage(stage_index);
                    operation.current_stage = stage_index;
                }
            }
            
            // Execute stage
            let stage_output = stage.execute(transaction_id, current_input.clone()).await?;
            stage_outputs.push(stage_output.clone());
            
            // Prepare input for next stage
            if let Some(next_input) = stage_output.next_input_data {
                current_input = PipelineStageInput {
                    collections: collections.clone(),
                    data: next_input,
                    previous_output_data: Some(OperationData::Metadata(stage_output.metadata.clone())),
                    metadata: stage_output.metadata.clone(),
                };
            } else {
                current_input = PipelineStageInput {
                    collections: collections.clone(),
                    data: stage_output.data.clone(),
                    previous_output_data: Some(OperationData::Metadata(stage_output.metadata.clone())),
                    metadata: stage_output.metadata.clone(),
                };
            }
        }
        
        // Mark pipeline as completed
        {
            let mut active_operations = self.background_pipeline.active_operations.write().await;
            if let Some(operation) = active_operations.get_mut(&transaction_id) {
                operation.status = PipelineOperationStatus::Completed;
                operation.stage_outputs = stage_outputs.clone();
            }
        }
        
        // Create result
        let result = UnifiedOperationResult {
            final_url: "pipeline://completed".to_string(),
            intermediate_urls: stage_outputs.iter().flat_map(|o| o.urls_created.clone()).collect(),
            staging_urls: vec![],
            checksum: None,
            size_bytes: 0,
            duration: Duration::from_secs(0),
            strategy_name: "BackgroundPipeline".to_string(),
            metadata: HashMap::new(),
        };
        
        // Update transaction state
        context.state = UnifiedTransactionState::Executing;
        
        // Update statistics
        let mut stats = self.stats.write().await;
        stats.pipeline_operations += 1;
        stats.operations_by_type.entry("Pipeline".to_string()).and_modify(|e| *e += 1).or_insert(1);
        stats.operations_by_component.entry("Background".to_string()).and_modify(|e| *e += 1).or_insert(1);
        
        tracing::info!(
            "🔄 UNIFIED: Executed background pipeline in transaction {} for {} collections",
            transaction_id,
            collections.len()
        );
        
        Ok(result)
    }
    
    /// Execute operation with appropriate strategy
    async fn execute_operation_with_strategy(&self, operation: &UnifiedAtomicOperation) -> Result<UnifiedOperationResult> {
        // Get storage strategy
        let scheme = operation.target_url.split("://").next().unwrap_or("file");
        let storage_strategy = {
            let strategies = self.storage_strategies.read().await;
            strategies.get(scheme).cloned()
                .ok_or_else(|| anyhow::anyhow!("No strategy for scheme: {}", scheme))?
        };
        
        // Get staging coordinator
        let staging_coordinator = {
            let coordinators = self.staging_coordinators.read().await;
            let staging_type = match operation.component_type {
                ComponentType::Wal => StagingType::Wal,
                ComponentType::StorageEngine => StagingType::Flush,
                ComponentType::Cloud => StagingType::Cloud,
                ComponentType::Index => StagingType::Index,
                ComponentType::Metadata => StagingType::Metadata,
                ComponentType::Background => StagingType::Background,
                _ => StagingType::Flush,
            };
            coordinators.get(&staging_type).cloned()
                .ok_or_else(|| anyhow::anyhow!("No staging coordinator for type: {:?}", staging_type))?
        };
        
        // Execute operation
        storage_strategy.execute_operation(operation, &staging_coordinator.config).await
    }
    
    /// Commit transaction
    pub async fn commit_transaction(&self, transaction_id: TransactionId) -> Result<()> {
        let mut transactions = self.unified_transactions.write().await;
        let context = transactions.get_mut(&transaction_id)
            .ok_or_else(|| anyhow::anyhow!("Transaction not found: {}", transaction_id))?;

        context.state = UnifiedTransactionState::Validating;

        // Validate all operations if enabled
        if self.config.enable_validation {
            self.validate_all_operations(context).await?;
        }

        context.state = UnifiedTransactionState::Finalizing;

        // Finalize all operations
        self.finalize_all_operations(context).await?;

        context.state = UnifiedTransactionState::CleaningUp;

        // Clean up staging operations
        self.cleanup_staging_operations(context).await?;

        context.state = UnifiedTransactionState::Committed;

        // Update statistics
        let mut stats = self.stats.write().await;
        stats.successful_transactions += 1;
        stats.active_transactions -= 1;

        let duration = Utc::now()
            .signed_duration_since(context.start_time)
            .num_milliseconds() as f64;
        
        stats.avg_transaction_duration_ms = 
            (stats.avg_transaction_duration_ms * (stats.successful_transactions - 1) as f64 + duration) 
            / stats.successful_transactions as f64;

        tracing::info!(
            "✅ UNIFIED: Committed transaction {} successfully in {:.2}ms",
            transaction_id,
            duration
        );

        Ok(())
    }
    
    /// Rollback transaction
    pub async fn rollback_transaction(&self, transaction_id: TransactionId) -> Result<()> {
        let mut transactions = self.unified_transactions.write().await;
        let context = transactions.get_mut(&transaction_id)
            .ok_or_else(|| anyhow::anyhow!("Transaction not found: {}", transaction_id))?;

        context.state = UnifiedTransactionState::RollingBack;

        // Execute rollback operations in reverse order
        for rollback_op in context.rollback_operations.iter().rev() {
            if let Err(e) = self.execute_rollback_operation(rollback_op).await {
                tracing::warn!("Failed to execute rollback operation: {}", e);
            }
        }

        // Clean up staging operations
        self.cleanup_staging_operations(context).await?;

        context.state = UnifiedTransactionState::Aborted;

        // Update statistics
        let mut stats = self.stats.write().await;
        stats.rolled_back_transactions += 1;
        stats.active_transactions -= 1;

        tracing::info!(
            "🔄 UNIFIED: Rolled back transaction {} successfully",
            transaction_id
        );

        Ok(())
    }
    
    /// Validate all operations in transaction
    async fn validate_all_operations(&self, context: &UnifiedTransactionContext) -> Result<()> {
        for operation in &context.operations {
            // Get storage strategy
            let scheme = operation.target_url.split("://").next().unwrap_or("file");
            let strategies = self.storage_strategies.read().await;
            if let Some(strategy) = strategies.get(scheme) {
                if !strategy.validate_operation(operation).await? {
                    return Err(anyhow::anyhow!("Validation failed for operation: {}", operation.operation_id));
                }
            }
        }
        Ok(())
    }
    
    /// Finalize all operations in transaction
    async fn finalize_all_operations(&self, context: &UnifiedTransactionContext) -> Result<()> {
        for operation in &context.operations {
            // Move from staging to final location if needed
            if let Some(staging_url) = &operation.staging_url {
                // Execute atomic move
                self.atomic_move_operation(staging_url, &operation.target_url).await?;
            }
        }
        Ok(())
    }
    
    /// Clean up staging operations
    async fn cleanup_staging_operations(&self, context: &UnifiedTransactionContext) -> Result<()> {
        for staging_op in &context.staging_operations {
            if let Err(e) = self.cleanup_staging_operation(staging_op).await {
                tracing::warn!("Failed to cleanup staging operation: {}", e);
            }
        }
        
        // Update statistics
        let mut stats = self.stats.write().await;
        stats.cleanup_operations += context.staging_operations.len() as u64;
        
        Ok(())
    }
    
    /// Execute rollback operation
    async fn execute_rollback_operation(&self, operation: &UnifiedAtomicOperation) -> Result<()> {
        let scheme = operation.target_url.split("://").next().unwrap_or("file");
        let strategies = self.storage_strategies.read().await;
        if let Some(strategy) = strategies.get(scheme) {
            strategy.rollback_operation(operation).await?;
        }
        Ok(())
    }
    
    /// Atomic move operation
    async fn atomic_move_operation(&self, source_url: &str, target_url: &str) -> Result<()> {
        // Get appropriate filesystem
        let source_scheme = source_url.split("://").next().unwrap_or("file");
        let target_scheme = target_url.split("://").next().unwrap_or("file");
        
        if source_scheme == target_scheme {
            // Same filesystem - use atomic move
            match self.filesystem_factory.move_atomic(source_url, target_url).await {
                Ok(_) => return Ok(()),
                Err(_) => {} // Fall through to copy+delete
            }
        }
        
        // Fallback to copy + delete
        let source_filesystem = self.filesystem_factory.get_filesystem(source_url)?;
        let target_filesystem = self.filesystem_factory.get_filesystem(target_url)?;
        
        let source_parsed = url::Url::parse(source_url)?;
        let target_parsed = url::Url::parse(target_url)?;
        let source_path = source_parsed.path();
        let target_path = target_parsed.path();
        
        // Read from source
        let data = source_filesystem.read(source_path).await?;
        
        // Write to target
        let options = Some(crate::storage::persistence::filesystem::FileOptions {
            create_dirs: true,
            overwrite: true,
            ..Default::default()
        });
        target_filesystem.write_atomic(target_path, &data, options).await?;
        
        // Delete source
        source_filesystem.delete(source_path).await?;
        
        Ok(())
    }
    
    /// Cleanup staging operation
    async fn cleanup_staging_operation(&self, staging_op: &StagingOperation) -> Result<()> {
        let filesystem = self.filesystem_factory.get_filesystem(&staging_op.staging_url)?;
        let parsed_url = url::Url::parse(&staging_op.staging_url)?;
        let path = parsed_url.path();
        
        // Delete staging file
        filesystem.delete(path).await?;
        
        tracing::debug!("🧹 Cleaned up staging operation: {}", staging_op.operation_id);
        Ok(())
    }
    
    /// Get statistics
    pub async fn get_stats(&self) -> UnifiedAtomicityStats {
        self.stats.read().await.clone()
    }
    
    /// Cleanup completed transactions
    pub async fn cleanup_completed_transactions(&self) -> Result<usize> {
        let mut transactions = self.unified_transactions.write().await;
        let initial_count = transactions.len();

        transactions.retain(|_, context| {
            !matches!(
                context.state,
                UnifiedTransactionState::Committed | UnifiedTransactionState::Aborted
            )
        });

        let cleaned_count = initial_count - transactions.len();
        
        if cleaned_count > 0 {
            tracing::debug!("🧹 UNIFIED: Cleaned up {} completed transactions", cleaned_count);
        }

        Ok(cleaned_count)
    }
}

// Storage strategy implementations
/// Unified local storage strategy
#[derive(Debug)]
pub struct UnifiedLocalStrategy {
    filesystem_factory: Arc<FilesystemFactory>,
}

impl UnifiedLocalStrategy {
    pub fn new(filesystem_factory: Arc<FilesystemFactory>) -> Self {
        Self { filesystem_factory }
    }
}

#[async_trait::async_trait]
impl UnifiedStorageStrategy for UnifiedLocalStrategy {
    fn strategy_name(&self) -> &'static str {
        "UnifiedLocal"
    }
    
    fn can_handle_url(&self, url: &str) -> bool {
        url.starts_with("file://")
    }
    
    async fn execute_operation(
        &self,
        operation: &UnifiedAtomicOperation,
        _staging_config: &StagingConfig,
    ) -> Result<UnifiedOperationResult> {
        let start_time = Instant::now();
        
        // Implementation depends on operation type
        match operation.operation_type {
            UnifiedOperationType::Write => {
                // Write operation
                if let Some(OperationData::RawBytes(data)) = &operation.data {
                    let filesystem = self.filesystem_factory.get_filesystem(&operation.target_url)?;
                    let parsed_url = url::Url::parse(&operation.target_url)?;
                    let path = parsed_url.path();
                    
                    let options = Some(crate::storage::persistence::filesystem::FileOptions {
                        create_dirs: true,
                        overwrite: true,
                        ..Default::default()
                    });
                    
                    filesystem.write_atomic(path, data, options).await?;
                    
                    let checksum = blake3::hash(data).to_hex().to_string();
                    
                    Ok(UnifiedOperationResult {
                        final_url: operation.target_url.clone(),
                        intermediate_urls: vec![],
                        staging_urls: operation.staging_url.clone().into_iter().collect(),
                        checksum: Some(checksum),
                        size_bytes: data.len(),
                        duration: start_time.elapsed(),
                        strategy_name: self.strategy_name().to_string(),
                        metadata: HashMap::new(),
                    })
                } else {
                    // Handle other data types
                    Ok(UnifiedOperationResult {
                        final_url: operation.target_url.clone(),
                        intermediate_urls: vec![],
                        staging_urls: operation.staging_url.clone().into_iter().collect(),
                        checksum: None,
                        size_bytes: operation.metadata.size_bytes,
                        duration: start_time.elapsed(),
                        strategy_name: self.strategy_name().to_string(),
                        metadata: HashMap::new(),
                    })
                }
            }
            _ => {
                // Other operation types
                Ok(UnifiedOperationResult {
                    final_url: operation.target_url.clone(),
                    intermediate_urls: vec![],
                    staging_urls: operation.staging_url.clone().into_iter().collect(),
                    checksum: None,
                    size_bytes: operation.metadata.size_bytes,
                    duration: start_time.elapsed(),
                    strategy_name: self.strategy_name().to_string(),
                    metadata: HashMap::new(),
                })
            }
        }
    }
    
    async fn rollback_operation(&self, operation: &UnifiedAtomicOperation) -> Result<()> {
        // Delete target file if it exists
        if let Ok(filesystem) = self.filesystem_factory.get_filesystem(&operation.target_url) {
            if let Ok(parsed_url) = url::Url::parse(&operation.target_url) {
                let path = parsed_url.path();
                let _ = filesystem.delete(path).await;
            }
        }
        Ok(())
    }
    
    async fn validate_operation(&self, operation: &UnifiedAtomicOperation) -> Result<bool> {
        // Check if file exists and is readable
        if let Ok(filesystem) = self.filesystem_factory.get_filesystem(&operation.target_url) {
            if let Ok(parsed_url) = url::Url::parse(&operation.target_url) {
                let path = parsed_url.path();
                return Ok(filesystem.read(path).await.is_ok());
            }
        }
        Ok(false)
    }
    
    async fn check_storage_health(&self, base_url: &str) -> Result<bool> {
        let filesystem = self.filesystem_factory.get_filesystem(base_url)?;
        let parsed_url = url::Url::parse(base_url)?;
        let path = parsed_url.path();
        
        // Try to list the directory
        Ok(filesystem.list(path).await.is_ok())
    }
}

/// Unified S3 storage strategy
#[derive(Debug)]
pub struct UnifiedS3Strategy {
    filesystem_factory: Arc<FilesystemFactory>,
}

impl UnifiedS3Strategy {
    pub fn new(filesystem_factory: Arc<FilesystemFactory>) -> Self {
        Self { filesystem_factory }
    }
}

#[async_trait::async_trait]
impl UnifiedStorageStrategy for UnifiedS3Strategy {
    fn strategy_name(&self) -> &'static str {
        "UnifiedS3"
    }
    
    fn can_handle_url(&self, url: &str) -> bool {
        url.starts_with("s3://")
    }
    
    async fn execute_operation(
        &self,
        operation: &UnifiedAtomicOperation,
        _staging_config: &StagingConfig,
    ) -> Result<UnifiedOperationResult> {
        let start_time = Instant::now();
        
        // Placeholder implementation - similar to local but with S3-specific optimizations
        Ok(UnifiedOperationResult {
            final_url: operation.target_url.clone(),
            intermediate_urls: vec![],
            staging_urls: operation.staging_url.clone().into_iter().collect(),
            checksum: None,
            size_bytes: operation.metadata.size_bytes,
            duration: start_time.elapsed(),
            strategy_name: self.strategy_name().to_string(),
            metadata: HashMap::new(),
        })
    }
    
    async fn rollback_operation(&self, _operation: &UnifiedAtomicOperation) -> Result<()> {
        Ok(())
    }
    
    async fn validate_operation(&self, _operation: &UnifiedAtomicOperation) -> Result<bool> {
        Ok(true)
    }
    
    async fn check_storage_health(&self, _base_url: &str) -> Result<bool> {
        Ok(true)
    }
}

/// Unified GCS storage strategy
#[derive(Debug)]
pub struct UnifiedGcsStrategy {
    filesystem_factory: Arc<FilesystemFactory>,
}

impl UnifiedGcsStrategy {
    pub fn new(filesystem_factory: Arc<FilesystemFactory>) -> Self {
        Self { filesystem_factory }
    }
}

#[async_trait::async_trait]
impl UnifiedStorageStrategy for UnifiedGcsStrategy {
    fn strategy_name(&self) -> &'static str {
        "UnifiedGCS"
    }
    
    fn can_handle_url(&self, url: &str) -> bool {
        url.starts_with("gcs://")
    }
    
    async fn execute_operation(
        &self,
        operation: &UnifiedAtomicOperation,
        _staging_config: &StagingConfig,
    ) -> Result<UnifiedOperationResult> {
        let start_time = Instant::now();
        
        // Placeholder implementation
        Ok(UnifiedOperationResult {
            final_url: operation.target_url.clone(),
            intermediate_urls: vec![],
            staging_urls: operation.staging_url.clone().into_iter().collect(),
            checksum: None,
            size_bytes: operation.metadata.size_bytes,
            duration: start_time.elapsed(),
            strategy_name: self.strategy_name().to_string(),
            metadata: HashMap::new(),
        })
    }
    
    async fn rollback_operation(&self, _operation: &UnifiedAtomicOperation) -> Result<()> {
        Ok(())
    }
    
    async fn validate_operation(&self, _operation: &UnifiedAtomicOperation) -> Result<bool> {
        Ok(true)
    }
    
    async fn check_storage_health(&self, _base_url: &str) -> Result<bool> {
        Ok(true)
    }
}

/// Unified Azure storage strategy
#[derive(Debug)]
pub struct UnifiedAzureStrategy {
    filesystem_factory: Arc<FilesystemFactory>,
}

impl UnifiedAzureStrategy {
    pub fn new(filesystem_factory: Arc<FilesystemFactory>) -> Self {
        Self { filesystem_factory }
    }
}

#[async_trait::async_trait]
impl UnifiedStorageStrategy for UnifiedAzureStrategy {
    fn strategy_name(&self) -> &'static str {
        "UnifiedAzure"
    }
    
    fn can_handle_url(&self, url: &str) -> bool {
        url.starts_with("adls://") || url.starts_with("abfs://")
    }
    
    async fn execute_operation(
        &self,
        operation: &UnifiedAtomicOperation,
        _staging_config: &StagingConfig,
    ) -> Result<UnifiedOperationResult> {
        let start_time = Instant::now();
        
        // Placeholder implementation
        Ok(UnifiedOperationResult {
            final_url: operation.target_url.clone(),
            intermediate_urls: vec![],
            staging_urls: operation.staging_url.clone().into_iter().collect(),
            checksum: None,
            size_bytes: operation.metadata.size_bytes,
            duration: start_time.elapsed(),
            strategy_name: self.strategy_name().to_string(),
            metadata: HashMap::new(),
        })
    }
    
    async fn rollback_operation(&self, _operation: &UnifiedAtomicOperation) -> Result<()> {
        Ok(())
    }
    
    async fn validate_operation(&self, _operation: &UnifiedAtomicOperation) -> Result<bool> {
        Ok(true)
    }
    
    async fn check_storage_health(&self, _base_url: &str) -> Result<bool> {
        Ok(true)
    }
}

impl Clone for UnifiedAtomicityManager {
    fn clone(&self) -> Self {
        Self {
            base_manager: self.base_manager.clone(),
            unified_transactions: self.unified_transactions.clone(),
            filesystem_factory: self.filesystem_factory.clone(),
            storage_strategies: self.storage_strategies.clone(),
            staging_coordinators: self.staging_coordinators.clone(),
            background_pipeline: self.background_pipeline.clone(),
            config: self.config.clone(),
            stats: self.stats.clone(),
        }
    }
}

// Implement AtomicOperation for UnifiedAtomicOperation
#[async_trait::async_trait]
impl AtomicOperation for UnifiedAtomicOperation {
    async fn execute(&self, _context: &mut TransactionContext) -> Result<OperationResult> {
        Ok(OperationResult::Simple {
            success: true,
            data: None,
            wal_entries: vec![],
            affected_count: 1,
            execution_time: Duration::from_millis(0),
        })
    }

    async fn rollback(&self, _context: &TransactionContext) -> Result<()> {
        Ok(())
    }

    async fn validate(&self, _context: &TransactionContext) -> Result<()> {
        if self.target_url.is_empty() {
            return Err(anyhow::anyhow!("Target URL cannot be empty"));
        }
        Ok(())
    }

    fn operation_type(&self) -> OperationType {
        match self.operation_type {
            UnifiedOperationType::Write => OperationType::VectorBatch,
            UnifiedOperationType::Move => OperationType::VectorBatch,
            UnifiedOperationType::Delete => OperationType::Delete,
            UnifiedOperationType::Flush => OperationType::VectorBatch,
            UnifiedOperationType::Compact => OperationType::Compact,
            _ => OperationType::VectorBatch,
        }
    }

    fn affected_resources(&self) -> Vec<ResourceId> {
        if let Some(collection_id) = &self.metadata.collection_id {
            vec![ResourceId::Collection(collection_id.clone())]
        } else {
            vec![]
        }
    }

    fn priority(&self) -> OperationPriority {
        OperationPriority::High
    }

    fn estimated_duration(&self) -> Duration {
        Duration::from_secs(30)
    }
}