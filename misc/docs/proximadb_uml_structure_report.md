# ProximaDB Rust Structure Analysis for UML
## Module Hierarchy and Key Types

### Storage Domain

#### Module: storage

##### storage::atomicity

**Structs:**
- `TransactionContext`
  - pub transaction_id: TransactionId
  - pub start_time: DateTime<Utc>
  - pub state: TransactionState
  - pub executed_operations: Vec<Box<dyn AtomicOperation>>
  - pub rollback_operations: VecDeque<Box<dyn AtomicOperation>>
  - pub isolation_level: IsolationLevel
  - pub timeout: Duration
  - pub locked_resources: HashMap<ResourceId
  - pub metadata: TransactionMetadata
- `OperationResult`
  - pub success: bool
  - pub data: Option<serde_json::Value>
  - pub wal_entries: Vec<WalEntry>
  - pub affected_count: usize
  - pub execution_time: Duration
- `TransactionMetadata`
  - pub client_id: Option<String>
  - pub session_id: Option<String>
  - pub tags: HashMap<String
  - pub operation_count: usize
  - pub estimated_duration: Duration
- `AtomicityManager`
  - private active_transactions: Arc<RwLock<HashMap<TransactionId
  - private next_transaction_id: AtomicU64
  - private lock_manager: Arc<LockManager>
  - private timeout_monitor: Arc<TimeoutMonitor>
  - private deadlock_detector: Arc<DeadlockDetector>
  - private config: AtomicityConfig
  - private stats: Arc<RwLock<AtomicityStats>>
  - private event_tx: broadcast::Sender<TransactionEvent>
- `LockManager`
  - private locks: Arc<RwLock<HashMap<ResourceId
  - private wait_queue: Arc<Mutex<VecDeque<LockRequest>>>
  - private config: LockManagerConfig
- `LockEntry`
  - private holders: Vec<LockHolder>
  - private mode: LockType
  - private waiters: VecDeque<LockRequest>
  - private acquired_at: Instant
- `LockHolder`
  - private transaction_id: TransactionId
  - private lock_type: LockType
  - private acquired_at: Instant
- `LockRequest`
  - private transaction_id: TransactionId
  - private resource_id: ResourceId
  - private lock_type: LockType
  - private response_tx: oneshot::Sender<Result<()>>
  - private requested_at: Instant
- `TimeoutMonitor`
  - private running: AtomicBool
  - private task_handle: Option<tokio::task::JoinHandle<()>>
- `DeadlockDetector`
  - private wait_graph: Arc<RwLock<HashMap<TransactionId
  - private detection_interval: Duration
  - private running: AtomicBool
  - private task_handle: Option<tokio::task::JoinHandle<()>>
- `AtomicityConfig`
  - pub default_timeout: Duration
  - pub max_concurrent_transactions: usize
  - pub default_isolation_level: IsolationLevel
  - pub enable_deadlock_detection: bool
  - pub lock_timeout: Duration
  - pub transaction_log_retention: Duration
- `LockManagerConfig`
  - pub max_lock_wait_time: Duration
  - pub lock_escalation_threshold: usize
  - pub enable_lock_debugging: bool
- `AtomicityStats`
  - pub total_transactions: u64
  - pub committed_transactions: u64
  - pub aborted_transactions: u64
  - pub timed_out_transactions: u64
  - pub avg_transaction_duration_ms: f64
  - pub deadlocks_detected: u64
  - pub active_transaction_count: usize
  - pub lock_contention_events: u64
- `VectorInsertOperation`
  - pub collection_id: CollectionId
  - pub vector_record: VectorRecord
- `VectorUpdateOperation`
  - pub collection_id: CollectionId
  - pub vector_id: VectorId
  - pub vector_record: VectorRecord
- `VectorDeleteOperation`
  - pub collection_id: CollectionId
  - pub vector_id: VectorId
- `BulkVectorOperation`
  - pub collection_id: CollectionId
  - pub operations: Vec<BulkOperation>

**Enums:**
- `TransactionState`
  - Preparing
  - Active
  - Committing
  - RollingBack
  - Committed
  - Aborted
  - TimedOut
- `IsolationLevel`
  - ReadUncommitted
  - ReadCommitted
  - RepeatableRead
  - Serializable
- `OperationType`
  - Insert
  - Update
  - Delete
  - BulkInsert
  - BulkUpdate
  - BulkDelete
  - CreateCollection
  - DeleteCollection
  - BuildIndex
  - UpdateIndex
  - DropIndex
  - Compact
  - SchemaChange
- `ResourceId`
  - Vector(VectorId)
  - Collection(CollectionId)
  - Index(String)
  - Partition(String)
  - Global(String)
- `LockType`
  - Shared
  - Exclusive
  - IntentShared
  - IntentExclusive
- `OperationPriority`
  - Low = 1
  - Normal = 2
  - High = 3
  - Critical = 4
- `TransactionEvent`
  - Started(TransactionId)
  - Committed(TransactionId, Duration)
  - Aborted(TransactionId, String)
  - TimedOut(TransactionId)
  - DeadlockDetected(Vec<TransactionId>)
- `BulkOperation`
  - Insert(VectorRecord)
  - Update{
  - vector_id: VectorId
  - record: VectorRecord

**Traits:**
- `AtomicOperation`: Send + Sync + std::fmt::Debug
  - fn execute(&self, context: &mut TransactionContext) -> Result<OperationResult>
  - fn rollback(&self, context: &TransactionContext) -> Result<()>
  - fn validate(&self, context: &TransactionContext) -> Result<()>
  - fn operation_type(&self) -> OperationType
  - fn affected_resources(&self) -> Vec<ResourceId>
  - fn priority(&self) -> OperationPriority
  - fn estimated_duration(&self) -> Duration

##### storage::builder

**Structs:**
- `DataStorageConfig`
  - pub data_urls: Vec<String>
  - pub layout_strategy: StorageLayoutStrategy
  - pub compression: DataCompressionConfig
  - pub segment_size: u64
  - pub enable_mmap: bool
  - pub cache_size_mb: usize
  - pub compaction_config: crate::core::unified_types::CompactionConfig
  - pub tiering_config: Option<DataTieringConfig>
- `DataCompressionConfig`
  - pub compress_vectors: bool
  - pub compress_metadata: bool
  - pub vector_compression: VectorCompressionAlgorithm
  - pub metadata_compression: CompressionLevel
  - pub compression_level: u8
- `DataTieringConfig`
  - pub hot_tier: TierConfig
  - pub warm_tier: TierConfig
  - pub cold_tier: TierConfig
  - pub auto_tier_policies: AutoTierPolicies
- `TierConfig`
  - pub urls: Vec<String>
  - pub compression: DataCompressionConfig
  - pub cache_size_mb: usize
  - pub access_pattern: AccessPattern
- `AutoTierPolicies`
  - pub hot_to_warm_hours: u64
  - pub warm_to_cold_hours: u64
  - pub hot_tier_access_threshold: u32
  - pub enable_predictive_tiering: bool
- `StorageSystemConfig`
  - pub data_storage: DataStorageConfig
  - pub wal_system: WalConfig
  - pub filesystem: FilesystemConfig
  - pub metadata_backend: Option<crate::core::config::MetadataBackendConfig>
  - pub storage_performance: StoragePerformanceConfig
- `StoragePerformanceConfig`
  - pub io_threads: usize
  - pub memory_pool_mb: usize
  - pub batch_config: BatchConfig
  - pub enable_zero_copy: bool
  - pub buffer_config: StorageBufferConfig
- `StorageBufferConfig`
  - pub read_buffer_size: usize
  - pub write_buffer_size: usize
  - pub compaction_buffer_size: usize
- `BatchConfig`
  - pub default_batch_size: usize
  - pub max_batch_size: usize
  - pub batch_timeout_ms: u64
  - pub enable_adaptive_batching: bool
- `StorageSystemBuilder`
  - private config: StorageSystemConfig
- `StorageSystem`
  - private config: StorageSystemConfig
  - private filesystem: Arc<FilesystemFactory>
  - private wal_manager: Arc<WalManager>

**Enums:**
- `StorageLayoutStrategy`
  - Regular
  - Viper
  - Hybrid
- `VectorCompressionAlgorithm`
  - None
  - PQ
  - OPQ
  - SQ
  - BQ
  - FP16
  - INT8
- `CompressionLevel`
  - None
  - Fast
  - High
- `AccessPattern`
  - Random
  - Sequential
  - Mixed

##### storage::disk_manager::mod

**Structs:**
- `DiskManager`
  - private data_dirs: Vec<PathBuf>
  - private current_write_dir: usize

##### storage::encoding::column_family

**Structs:**
- `ColumnFamilyStorage`
  - private families: HashMap<String

##### storage::encoding::mod

**Structs:**
- `ColumnFamilyConfig`
  - pub name: String
  - pub compression: CompressionType
  - pub ttl_seconds: Option<u64>
  - pub storage_format: ColumnStorageFormat
- `Tombstone`
  - pub vector_id: String
  - pub collection_id: String
  - pub deleted_at: u64
  - pub ttl_seconds: Option<u64>

**Enums:**
- `StorageFormat`
  - RowBased{ compression: CompressionType
- `ColumnStorageFormat`
  - Document{ max_document_size_kb: u32
- `CompressionType`
  - None
  - Gzip
  - Lz4
  - Zstd{ level: i32

**Traits:**
- `Encoder`
  - fn encode(&self, records: &[VectorRecord]) -> crate::Result<Vec<u8>>
  - fn decode(&self, data: &[u8]) -> crate::Result<Vec<VectorRecord>>
- `SoftDelete`
  - fn is_deleted(&self, record: &VectorRecord, current_time: u64) -> bool
  - fn mark_deleted(&self, record: &mut VectorRecord, delete_time: u64)

##### storage::encoding::parquet_encoder

**Structs:**
- `ParquetEncoder`
  - private compression: CompressionType
  - private row_group_size: usize

##### storage::engine

**Structs:**
- `StorageEngine`
  - private config: StorageConfig
  - private lsm_trees: Arc<RwLock<HashMap<CollectionId
  - private mmap_readers: Arc<RwLock<HashMap<CollectionId
  - private disk_manager: Arc<DiskManager>
  - private wal_manager: Arc<WalManager>
  - private search_index_manager: Arc<SearchIndexManager>
  - private compaction_manager: Arc<CompactionManager>
  - private metadata_cache: Arc<RwLock<BTreeMap<CollectionId

##### storage::filesystem::atomic_strategy

**Structs:**
- `AtomicWriteConfig`
  - pub strategy: AtomicWriteStrategy
  - pub temp_config: TempDirectoryConfig
  - pub cleanup_config: CleanupConfig
  - pub retry_config: AtomicRetryConfig
- `TempDirectoryConfig`
  - pub cleanup_on_startup: bool
  - pub max_temp_age_hours: u64
  - pub temp_patterns: Vec<String>
- `CleanupConfig`
  - pub enable_auto_cleanup: bool
  - pub cleanup_interval_secs: u64
  - pub cleanup_patterns: Vec<String>
- `AtomicRetryConfig`
  - pub max_retries: usize
  - pub initial_delay_ms: u64
  - pub max_delay_ms: u64
  - pub backoff_multiplier: f64
- `DirectWriteExecutor`
- `SameMountTempExecutor`
  - private temp_suffix: String
  - private config: AtomicWriteConfig
- `CloudOptimizedExecutor`
  - private local_temp_dir: PathBuf
  - private enable_compression: bool
  - private chunk_size_mb: usize
  - private config: AtomicWriteConfig
- `AutoDetectExecutor`
  - private config: AtomicWriteConfig
- `AtomicWriteExecutorFactory`

**Enums:**
- `AtomicWriteStrategy`
  - Direct
  - SameMountTemp{
  - temp_suffix: String, // Default: "___temp"

**Traits:**
- `AtomicWriteExecutor`: Send + Sync
  - fn write_atomic(&self,
        filesystem: &dyn FileSystem,
        final_path: &str,
        data: &[u8],
        options: Option<FileOptions>,) -> FsResult<()>
  - fn cleanup_temp_files(&self, filesystem: &dyn FileSystem) -> FsResult<()>
  - fn strategy_name(&self) -> &str

##### storage::filesystem::auth

**Structs:**
- `AwsCredentials`
  - pub access_key_id: String
  - pub secret_access_key: String
  - pub session_token: Option<String>
  - pub expiration: Option<Instant>
- `AzureCredentials`
  - pub account_name: String
  - pub access_token: String
  - pub expiration: Option<Instant>
- `GcsCredentials`
  - pub access_token: String
  - pub project_id: String
  - pub expiration: Option<Instant>
- `StaticCredentialProvider`
  - private access_key_id: String
  - private secret_access_key: String
  - private session_token: Option<String>
- `EnvironmentCredentialProvider`
- `InstanceMetadataProvider`
  - private http_client: reqwest::Client
- `EcsTaskMetadataProvider`
  - private http_client: reqwest::Client
- `AssumeRoleProvider`
  - private role_arn: String
  - private external_id: Option<String>
  - private http_client: reqwest::Client
- `ProfileCredentialProvider`
  - private profile_name: String
- `ChainCredentialProvider`
  - private providers: Vec<Box<dyn CredentialProvider>>
- `AzureManagedIdentityProvider`
  - private client_id: Option<String>
  - private http_client: reqwest::Client
- `GcsApplicationDefaultProvider`
  - private http_client: reqwest::Client

**Traits:**
- `CredentialProvider`: Send + Sync
  - fn get_credentials(&self) -> FsResult<AwsCredentials>
  - fn refresh_credentials(&self) -> FsResult<AwsCredentials>
- `AzureCredentialProvider`: Send + Sync
  - fn get_credentials(&self) -> FsResult<AzureCredentials>
  - fn refresh_credentials(&self) -> FsResult<AzureCredentials>
- `GcsCredentialProvider`: Send + Sync
  - fn get_credentials(&self) -> FsResult<GcsCredentials>
  - fn refresh_credentials(&self) -> FsResult<GcsCredentials>

##### storage::filesystem::azure

**Structs:**
- `AzureConfig`
  - pub account_name: String
  - pub default_container: Option<String>
  - pub credentials: AzureCredentialConfig
  - pub default_blob_tier: AzureBlobTier
  - pub timeout_seconds: u64
  - pub max_retries: u32
  - pub hierarchical_namespace: bool
  - pub block_size: u64
- `AzureCredentialConfig`
  - pub provider: AzureCredentialProviderType
  - pub account_key: Option<String>
  - pub sas_token: Option<String>
  - pub client_id: Option<String>
  - pub tenant_id: Option<String>
  - pub sp_client_id: Option<String>
  - pub sp_client_secret: Option<String>
  - pub refresh_interval: u64
- `AzureFileSystem`
  - private config: AzureConfig
  - private credential_provider: Box<dyn AzureCredentialProvider>
  - private client: AzureClient
- `AzureClient`
  - private config: AzureConfig
  - private http_client: reqwest::Client
- `AccountKeyProvider`
  - private account_key: String
- `SasTokenProvider`
  - private sas_token: String
- `ServicePrincipalProvider`
  - private tenant_id: String
  - private client_id: String
  - private client_secret: String
  - private http_client: reqwest::Client

**Enums:**
- `AzureBlobTier`
  - Hot
  - Cool
  - Archive
- `AzureCredentialProviderType`
  - AccountKey
  - SasToken
  - ManagedIdentity
  - ServicePrincipal
  - AzureCli
  - Environment

##### storage::filesystem::gcs

**Structs:**
- `GcsConfig`
  - pub project_id: String
  - pub default_bucket: Option<String>
  - pub credentials: GcsCredentialConfig
  - pub default_storage_class: GcsStorageClass
  - pub timeout_seconds: u64
  - pub max_retries: u32
  - pub resumable_threshold: u64
  - pub upload_chunk_size: u64
- `GcsCredentialConfig`
  - pub provider: GcsCredentialProviderType
  - pub service_account_key_file: Option<String>
  - pub service_account_key_json: Option<String>
  - pub refresh_interval: u64
- `GcsFileSystem`
  - private config: GcsConfig
  - private credential_provider: Box<dyn GcsCredentialProvider>
  - private client: GcsClient
- `GcsClient`
  - private config: GcsConfig
  - private http_client: reqwest::Client
- `ServiceAccountKeyFileProvider`
  - private key_file_path: String
  - private http_client: reqwest::Client
- `ServiceAccountKeyJsonProvider`
  - private key_json: String
  - private http_client: reqwest::Client

**Enums:**
- `GcsStorageClass`
  - Standard
  - Nearline
  - Coldline
  - Archive
- `GcsCredentialProviderType`
  - ApplicationDefault
  - ServiceAccountKey
  - ServiceAccountJson
  - ComputeEngine
  - Environment

##### storage::filesystem::hdfs

**Structs:**
- `HdfsConfig`
  - pub namenode_url: String
  - pub user: String
  - pub auth_type: HdfsAuthType
  - pub kerberos_principal: Option<String>
  - pub kerberos_keytab: Option<String>
  - pub timeout_seconds: u64
  - pub max_retries: u32
  - pub block_size: u64
  - pub replication: u16
  - pub buffer_size: u32
- `HdfsFileSystem`
  - private config: HdfsConfig
  - private client: HdfsClient
- `HdfsClient`
  - private config: HdfsConfig
  - private http_client: reqwest::Client
  - private base_url: String

**Enums:**
- `HdfsAuthType`
  - Simple
  - Kerberos
  - PseudoAuth

##### storage::filesystem::local

**Structs:**
- `LocalConfig`
  - pub root_dir: Option<PathBuf>
  - pub follow_symlinks: bool
  - pub default_permissions: Option<u32>
  - pub sync_enabled: bool
- `LocalFileSystem`
  - private config: LocalConfig

##### storage::filesystem::manager

**Structs:**
- `RetryConfig`
  - pub max_retries: usize
  - pub initial_delay_ms: u64
  - pub max_delay_ms: u64
  - pub backoff_multiplier: f64
- `FilesystemKey`
  - private scheme: String
  - private host: Option<String>
  - private base_path: String
- `ManagedFilesystem`
  - private filesystem: Box<dyn FileSystem>
  - private base_path: String
  - private retry_config: RetryConfig
- `FilesystemManager`
  - private filesystems: Arc<RwLock<HashMap<FilesystemKey
  - private auth_providers: HashMap<String
  - private default_retry_config: RetryConfig
  - private config: super::FilesystemConfig

##### storage::filesystem::mod

**Structs:**
- `FileMetadata`
  - pub path: String
  - pub size: u64
  - pub created: Option<chrono::DateTime<chrono::Utc>>
  - pub modified: Option<chrono::DateTime<chrono::Utc>>
  - pub is_directory: bool
  - pub permissions: Option<String>
  - pub etag: Option<String>
  - pub storage_class: Option<String>
- `DirEntry`
  - pub name: String
  - pub path: String
  - pub metadata: FileMetadata
- `FileOptions`
  - pub create_dirs: bool
  - pub overwrite: bool
  - pub buffer_size: Option<usize>
  - pub encryption: Option<String>
  - pub storage_class: Option<String>
  - pub metadata: Option<HashMap<String
  - pub temp_path: Option<String>
- `AuthConfig`
  - pub aws_auth: Option<AwsAuthMethod>
  - pub azure_auth: Option<AzureAuthMethod>
  - pub gcs_auth: Option<GcsAuthMethod>
  - pub enable_credential_caching: bool
  - pub credential_refresh_interval_seconds: u64
- `RetryConfig`
  - pub max_retries: u32
  - pub initial_delay_ms: u64
  - pub max_delay_ms: u64
  - pub backoff_multiplier: f64
- `FilesystemPerformanceConfig`
  - pub connection_pool_size: usize
  - pub enable_keep_alive: bool
  - pub request_timeout_seconds: u64
  - pub enable_compression: bool
  - pub retry_config: RetryConfig
  - pub buffer_size: usize
  - pub enable_parallel_ops: bool
  - pub max_concurrent_ops: usize
- `FilesystemConfig`
  - pub default_fs: Option<String>
  - pub s3: Option<s3::S3Config>
  - pub azure: Option<azure::AzureConfig>
  - pub gcs: Option<gcs::GcsConfig>
  - pub local: Option<local::LocalConfig>
  - pub hdfs: Option<hdfs::HdfsConfig>
  - pub global_options: FileOptions
  - pub auth_config: Option<AuthConfig>
  - pub performance_config: FilesystemPerformanceConfig
- `FilesystemFactory`
  - private config: FilesystemConfig
  - private filesystems: HashMap<String

**Enums:**
- `FilesystemError`
- `TempStrategy`
  - DirectWrite
  - SameDirectory
  - ConfiguredTemp{
  - temp_dir: Option<String>
- `AwsAuthMethod`
  - IamRole
  - CredentialsFile{ profile: Option<String>
- `AzureAuthMethod`
  - ManagedIdentity
  - ServicePrincipal{
  - client_id: String
  - tenant_id: String
- `GcsAuthMethod`
  - ApplicationDefault
  - ServiceAccountFile{ path: String

**Traits:**
- `FileSystem`: Send + Sync + std::fmt::Debug
  - fn read(&self, path: &str) -> FsResult<Vec<u8>>
  - fn write(&self, path: &str, data: &[u8], options: Option<FileOptions>) -> FsResult<()>
  - fn append(&self, path: &str, data: &[u8]) -> FsResult<()>
  - fn delete(&self, path: &str) -> FsResult<()>
  - fn exists(&self, path: &str) -> FsResult<bool>
  - fn metadata(&self, path: &str) -> FsResult<FileMetadata>
  - fn list(&self, path: &str) -> FsResult<Vec<DirEntry>>
  - fn create_dir(&self, path: &str) -> FsResult<()>
  - fn create_dir_all(&self, path: &str) -> FsResult<()>
  - fn copy(&self, from: &str, to: &str) -> FsResult<()>
  - fn move_file(&self, from: &str, to: &str) -> FsResult<()>
  - fn filesystem_type(&self) -> &'static str
  - fn supports_atomic_writes(&self) -> bool

##### storage::filesystem::s3

**Structs:**
- `S3Config`
  - pub region: String
  - pub default_bucket: Option<String>
  - pub credentials: CredentialConfig
  - pub default_storage_class: S3StorageClass
  - pub encryption: Option<S3Encryption>
  - pub timeout_seconds: u64
  - pub max_retries: u32
  - pub multipart_threshold: u64
  - pub multipart_chunk_size: u64
- `S3Encryption`
  - pub method: S3EncryptionMethod
  - pub kms_key_id: Option<String>
- `CredentialConfig`
  - pub provider: CredentialProviderType
  - pub access_key_id: Option<String>
  - pub secret_access_key: Option<String>
  - pub session_token: Option<String>
  - pub role_arn: Option<String>
  - pub external_id: Option<String>
  - pub refresh_interval: u64
- `S3FileSystem`
  - private config: S3Config
  - private credential_provider: Box<dyn CredentialProvider>
  - private client: S3Client
- `S3Client`
  - private config: S3Config
  - private http_client: reqwest::Client

**Enums:**
- `S3StorageClass`
  - Standard
  - StandardIa
  - OneZoneIa
  - Glacier
  - GlacierInstantRetrieval
  - GlacierFlexibleRetrieval
  - GlacierDeepArchive
  - IntelligentTiering
- `S3EncryptionMethod`
  - None
  - Aes256
  - KmsManaged
  - KmsCustomerKey
- `CredentialProviderType`
  - Static
  - InstanceMetadata
  - EcsTaskMetadata
  - AssumeRole
  - Profile(String)
  - Environment
  - Chain

##### storage::filesystem::write_strategy

**Structs:**
- `WriteStrategyFactory`
- `MetadataWriteStrategy`
  - private temp_strategy: TempStrategy

##### storage::lsm::compaction

**Structs:**
- `CompactionTask`
  - pub collection_id: CollectionId
  - pub level: u8
  - pub input_files: Vec<PathBuf>
  - pub output_file: PathBuf
  - pub priority: CompactionPriority
- `CompactionStats`
  - pub total_compactions: u64
  - pub bytes_written: u64
  - pub bytes_read: u64
  - pub files_merged: u64
  - pub avg_compaction_time_ms: u64
  - pub last_compaction_time: Option<chrono::DateTime<chrono::Utc>>
- `CompactionManager`
  - private config: LsmConfig
  - private task_queue: Arc<Mutex<VecDeque<CompactionTask>>>
  - private worker_handles: Vec<JoinHandle<()>>
  - private shutdown_signal: Arc<AtomicBool>
  - private stats: Arc<RwLock<CompactionStats>>
  - private active_compactions: Arc<RwLock<HashMap<CollectionId

**Enums:**
- `CompactionPriority`
  - Low = 0
  - Medium = 1
  - High = 2
  - Critical = 3, // When storage is nearly full

##### storage::lsm::mod

**Structs:**
- `LsmTree`
  - private config: LsmConfig
  - private collection_id: CollectionId
  - private memtable: RwLock<BTreeMap<VectorId
  - private wal_manager: Arc<WalManager>
  - private data_dir: PathBuf
  - private compaction_manager: Option<Arc<CompactionManager>>

**Enums:**
- `LsmEntry`
  - Record(VectorRecord)
  - Tombstone{
  - id: VectorId
  - collection_id: CollectionId
  - timestamp: chrono::DateTime<chrono::Utc>

##### storage::memtable

**Structs:**
- `Memtable`
  - private data: Arc<RwLock<HashMap<CollectionId
  - private max_size_bytes: usize
  - private current_size: Arc<RwLock<usize>>
  - private sequence: Arc<RwLock<u64>>
- `CollectionMemtable`
  - private vectors: BTreeMap<u64
  - private id_to_sequence: HashMap<VectorId
  - private deleted: HashMap<VectorId
  - private stats: CollectionStats
- `MemtableEntry`
  - pub sequence: u64
  - pub operation: MemtableOperation
  - pub timestamp: DateTime<Utc>
- `CollectionStats`
  - private active_count: usize
  - private operation_count: usize
  - private size_bytes: usize
  - private last_updated: DateTime<Utc>
- `MemtableCollectionStats`
  - pub active_vectors: usize
  - pub total_operations: usize
  - pub size_bytes: usize
  - pub last_updated: DateTime<Utc>

**Enums:**
- `MemtableOperation`
  - Put{ record: VectorRecord

##### storage::metadata::atomic

**Structs:**
- `MetadataTransaction`
  - pub id: TransactionId
  - pub operations: Vec<MetadataOperation>
  - pub state: TransactionState
  - pub created_at: DateTime<Utc>
  - pub timeout_at: DateTime<Utc>
  - pub isolation_level: IsolationLevel
- `VersionInfo`
  - private version: u64
  - private transaction_id: TransactionId
  - private committed_at: DateTime<Utc>
  - private metadata: VersionedCollectionMetadata
- `LockInfo`
  - private transaction_id: TransactionId
  - private lock_type: LockType
  - private acquired_at: DateTime<Utc>
- `AtomicMetadataStore`
  - private wal_manager: Arc<MetadataWalManager>
  - private version_store: Arc<RwLock<HashMap<CollectionId
  - private active_transactions: Arc<RwLock<HashMap<TransactionId
  - private lock_table: Arc<RwLock<HashMap<CollectionId
  - private version_counter: Arc<Mutex<u64>>
  - private cleanup_interval: tokio::time::Duration
  - private stats: Arc<RwLock<AtomicStoreStats>>
- `AtomicStoreStats`
  - private transactions_started: u64
  - private transactions_committed: u64
  - private transactions_aborted: u64
  - private transactions_timed_out: u64
  - private lock_conflicts: u64
  - private mvcc_reads: u64

**Enums:**
- `TransactionState`
  - Active
  - Preparing
  - Committed
  - Aborted
  - TimedOut
- `IsolationLevel`
  - ReadCommitted
  - RepeatableRead
  - Serializable
- `LockType`
  - Shared
  - Exclusive

##### storage::metadata::backends::filestore_backend

**Structs:**
- `CollectionRecord`
  - pub uuid: String
  - pub name: String
  - pub dimension: i32
  - pub distance_metric: String
  - pub indexing_algorithm: String
  - pub storage_engine: String
  - pub created_at: i64
  - pub updated_at: i64
  - pub version: i64
  - pub vector_count: i64
  - pub total_size_bytes: i64
  - pub config: String
  - pub description: Option<String>
  - pub tags: Vec<String>
  - pub owner: Option<String>
- `IncrementalOperation`
  - pub operation_type: OperationType
  - pub sequence_number: u64
  - pub timestamp: String
  - pub collection_id: String
  - pub collection_data: Option<CollectionRecord>
- `FilestoreMetadataConfig`
  - pub filestore_url: String
  - pub enable_compression: bool
  - pub enable_backup: bool
  - pub enable_snapshot_archival: bool
  - pub max_archived_snapshots: usize
  - pub temp_directory: Option<String>
- `FilestoreMetadataBackend`
  - private config: FilestoreMetadataConfig
  - private filesystem: Arc<FilesystemFactory>
  - private filestore_url: String
  - private metadata_path: PathBuf
  - private single_index: SingleCollectionIndex
  - private atomic_writer: Box<dyn AtomicWriteExecutor>
  - private recovery_lock: Arc<tokio::sync::RwLock<()>>
  - private sequence_counter: Arc<AtomicU64>
  - private collection_schema: Schema
  - private operation_schema: Schema
- `AvroOperationRead`
  - private operation_type: String
  - private sequence_number: i64
  - private timestamp: String
  - private collection_id: String
  - private collection_data: Option<String>
- `AvroOperation`
  - private operation_type: String
  - private sequence_number: i64
  - private timestamp: String
  - private collection_id: String
  - private collection_data: Option<String>

**Enums:**
- `OperationType`
  - Insert
  - Update
  - Delete

##### storage::metadata::backends::memory_backend

**Structs:**
- `MemoryMetadataBackend`
  - private collections: Arc<RwLock<HashMap<CollectionId
  - private system_metadata: Arc<RwLock<SystemMetadata>>
  - private config: Option<MetadataBackendConfig>
  - private stats: Arc<RwLock<MemoryBackendStats>>
  - private indexes: Arc<RwLock<MemoryIndexes>>
- `MemoryIndexes`
  - private by_name: HashMap<String
  - private by_owner: HashMap<String
  - private by_access_pattern: HashMap<String
  - private by_tag: HashMap<String
  - private keywords: HashMap<String
- `MemoryBackendStats`
  - private total_operations: u64
  - private failed_operations: u64
  - private avg_operation_time_ns: f64
  - private collections_count: u64
  - private memory_usage_bytes: u64
  - private index_hits: u64
  - private full_scans: u64

**Enums:**
- `IndexOperation`
  - Insert
  - Update
  - Delete

##### storage::metadata::backends::mod

**Structs:**
- `MetadataBackendConfig`
  - pub backend_type: MetadataBackendType
  - pub connection: BackendConnectionConfig
  - pub performance: BackendPerformanceConfig
  - pub ha_config: Option<HAConfig>
  - pub backup_config: Option<BackendBackupConfig>
- `BackendConnectionConfig`
  - pub connection_string: String
  - pub auth: Option<AuthConfig>
  - pub pool_config: PoolConfig
  - pub ssl_config: Option<SSLConfig>
  - pub region: Option<String>
  - pub parameters: HashMap<String
- `AuthConfig`
  - pub username: Option<String>
  - pub password: Option<String>
  - pub api_key: Option<String>
  - pub iam_role: Option<String>
  - pub service_account: Option<String>
  - pub token: Option<String>
- `PoolConfig`
  - pub min_connections: u32
  - pub max_connections: u32
  - pub connect_timeout_secs: u64
  - pub idle_timeout_secs: u64
  - pub max_lifetime_secs: u64
- `SSLConfig`
  - pub enabled: bool
  - pub verify_certificates: bool
  - pub client_cert_path: Option<String>
  - pub client_key_path: Option<String>
  - pub ca_cert_path: Option<String>
- `BackendPerformanceConfig`
  - pub enable_read_replicas: bool
  - pub read_replica_endpoints: Vec<String>
  - pub enable_caching: bool
  - pub cache_ttl_secs: u64
  - pub batch_size: usize
  - pub query_timeout_secs: u64
  - pub enable_compression: bool
- `HAConfig`
  - pub multi_region: bool
  - pub regions: Vec<String>
  - pub consistency_level: ConsistencyLevel
  - pub failover: FailoverConfig
  - pub auto_backup: bool
- `FailoverConfig`
  - pub auto_failover: bool
  - pub failover_timeout_secs: u64
  - pub health_check_interval_secs: u64
  - pub max_retries: u32
- `BackendBackupConfig`
  - pub enabled: bool
  - pub frequency_hours: u32
  - pub retention_days: u32
  - pub backup_location: String
  - pub incremental: bool
  - pub point_in_time_recovery: bool
- `BackendStats`
  - pub backend_type: MetadataBackendType
  - pub connected: bool
  - pub active_connections: u32
  - pub total_operations: u64
  - pub failed_operations: u64
  - pub avg_latency_ms: f64
  - pub cache_hit_rate: Option<f64>
  - pub last_backup_time: Option<chrono::DateTime<chrono::Utc>>
  - pub storage_size_bytes: u64
- `MetadataBackendFactory`
- `BackendCapabilities`
  - pub supports_transactions: bool
  - pub supports_schemas: bool
  - pub supports_secondary_indexes: bool
  - pub supports_full_text_search: bool
  - pub supports_geo_queries: bool
  - pub supports_auto_scaling: bool
  - pub supports_multi_region: bool
  - pub supports_encryption_at_rest: bool
  - pub max_document_size_mb: u32
  - pub max_connections: u32

**Enums:**
- `MetadataBackendType`
  - Disk
  - Memory
  - PostgreSQL
  - MySQL
  - SQLite
  - DynamoDB
  - CosmosDB
  - Firestore
  - MultiBackend{
  - primary: Box<MetadataBackendType>
  - fallback: Box<MetadataBackendType>
- `ConsistencyLevel`
  - Eventual
  - Strong
  - Session
  - BoundedStaleness{ max_lag_ms: u64

**Traits:**
- `MetadataBackend`: Send + Sync
  - fn backend_name(&self) -> &'static str
  - fn initialize(&mut self, config: MetadataBackendConfig) -> Result<()>
  - fn health_check(&self) -> Result<bool>
  - fn create_collection(&self, metadata: CollectionMetadata) -> Result<()>
  - fn get_collection(&self,
        collection_id: &CollectionId,) -> Result<Option<CollectionMetadata>>
  - fn update_collection(&self,
        collection_id: &CollectionId,
        metadata: CollectionMetadata,) -> Result<()>
  - fn delete_collection(&self, collection_id: &CollectionId) -> Result<bool>
  - fn list_collections(&self,
        filter: Option<MetadataFilter>,) -> Result<Vec<CollectionMetadata>>
  - fn update_stats(&self,
        collection_id: &CollectionId,
        vector_delta: i64,
        size_delta: i64,) -> Result<()>
  - fn batch_operations(&self, operations: Vec<MetadataOperation>) -> Result<()>
  - fn begin_transaction(&self) -> Result<Option<String>>
  - fn commit_transaction(&self, transaction_id: &str) -> Result<()>
  - fn rollback_transaction(&self, transaction_id: &str) -> Result<()>
  - fn get_system_metadata(&self) -> Result<SystemMetadata>
  - fn update_system_metadata(&self, metadata: SystemMetadata) -> Result<()>
  - fn get_stats(&self) -> Result<BackendStats>
  - fn backup(&self, location: &str) -> Result<String>
  - fn restore(&self, backup_id: &str, location: &str) -> Result<()>
  - fn close(&self) -> Result<()>

##### storage::metadata::compaction

**Structs:**
- `MetadataCompactionConfig`
  - pub base: crate::core::unified_types::CompactionConfig
  - pub max_incremental_operations: usize
  - pub max_incremental_size_bytes: usize
  - pub keep_snapshots: usize
  - pub compress_snapshots: bool
- `CompactionStats`
  - pub last_compaction_time: Option<chrono::DateTime<chrono::Utc>>
  - pub total_compactions: u64
  - pub operations_compacted: u64
  - pub snapshots_created: u64
  - pub archives_created: u64
  - pub bytes_compacted: u64
  - pub current_incremental_count: usize
  - pub current_incremental_size: usize
- `FilestoreCompactionManager`
  - private config: MetadataCompactionConfig
  - private filesystem: Arc<FilesystemFactory>
  - private filestore_url: String
  - private metadata_path: PathBuf
  - private stats: CompactionStats
- `CompactionResult`
  - pub duration: std::time::Duration
  - pub initial_collections: usize
  - pub final_collections: usize
  - pub operations_compacted: usize
  - pub bytes_compacted: usize
  - pub archive_path: Option<String>

##### storage::metadata::indexes

**Structs:**
- `CollectionLookupResult`
  - pub uuid: String
  - pub name: String
  - pub dimension: i32
  - pub distance_metric: String
  - pub indexing_algorithm: String
  - pub storage_engine: String
  - pub vector_count: i64
  - pub total_size_bytes: i64
  - pub created_at: i64
  - pub updated_at: i64
- `IndexStatistics`
  - pub total_collections: usize
  - pub memory_usage_bytes: usize
  - pub uuid_index_hits: u64
  - pub name_index_hits: u64
  - pub prefix_index_hits: u64
  - pub tag_index_hits: u64
  - pub cache_misses: u64
  - pub last_rebuild_time: Option<i64>
  - pub avg_lookup_time_ns: u64
- `MetadataMemoryIndexes`
  - private 1: 1 mapping)
  - private uuid_to_record: DashMap<String
  - private 1: 1 mapping)
  - private name_to_uuid: DashMap<String
  - private name_prefix_index: Arc<RwLock<BTreeMap<String
  - private tag_to_uuids: Arc<RwLock<HashMap<String
  - private size_index: Arc<RwLock<BTreeMap<i64
  - private created_time_index: Arc<RwLock<BTreeMap<i64
  - private stats: Arc<RwLock<IndexStatistics>>

##### storage::metadata::mod

**Structs:**
- `CollectionMetadata`
  - pub id: CollectionId
  - pub name: String
  - pub dimension: usize
  - pub distance_metric: String
  - pub indexing_algorithm: String
  - pub created_at: DateTime<Utc>
  - pub updated_at: DateTime<Utc>
  - pub vector_count: u64
  - pub total_size_bytes: u64
  - pub config: HashMap<String
  - private serde_json: :Value>
  - pub access_pattern: AccessPattern
  - pub retention_policy: Option<RetentionPolicy>
  - pub tags: Vec<String>
  - pub owner: Option<String>
  - pub description: Option<String>
  - pub strategy_config: CollectionStrategyConfig
  - pub strategy_change_history: Vec<StrategyChangeStatus>
  - pub flush_config: Option<CollectionFlushConfig>
- `RetentionPolicy`
  - pub retain_days: u32
  - pub auto_archive: bool
  - pub auto_delete: bool
  - pub cold_storage_days: Option<u32>
  - pub backup_config: Option<BackupConfig>
- `BackupConfig`
  - pub enabled: bool
  - pub frequency_hours: u32
  - pub retain_count: u32
  - pub backup_location: String
- `StrategyChangeStatus`
  - pub change_id: String
  - pub changed_at: DateTime<Utc>
  - pub previous_strategy: CollectionStrategyConfig
  - pub current_strategy: CollectionStrategyConfig
  - pub change_type: StrategyChangeType
  - pub changed_by: Option<String>
  - pub change_reason: Option<String>
- `MetadataFilter`
  - pub access_pattern: Option<AccessPattern>
  - pub tags: Vec<String>
  - pub owner: Option<String>
  - pub min_vector_count: Option<u64>
  - pub max_age_days: Option<u32>
  - pub custom_filter: Option<Box<dyn Fn(&CollectionMetadata) -> bool + Send + Sync>>
- `CollectionFlushConfig`
  - pub max_wal_size_bytes: Option<usize>
- `GlobalFlushDefaults`
  - pub default_max_wal_size_bytes: usize
- `EffectiveFlushConfig`
  - pub max_wal_size_bytes: usize
- `MetadataStorageStats`
  - pub total_collections: u64
  - pub total_metadata_size_bytes: u64
  - pub cache_hit_rate: f64
  - pub avg_operation_latency_ms: f64
  - pub storage_backend: String
  - pub last_backup_time: Option<DateTime<Utc>>
  - pub wal_entries: u64
  - pub wal_size_bytes: u64

**Enums:**
- `AccessPattern`
  - Hot
  - Normal
  - Cold
  - Archive
- `StrategyChangeType`
  - StorageOnly
  - IndexingOnly
  - SearchOnly
  - StorageAndIndexing
  - StorageAndSearch
  - IndexingAndSearch
  - Complete
- `MetadataOperation`
  - CreateCollection(CollectionMetadata)
  - UpdateCollection{
  - collection_id: CollectionId
  - metadata: CollectionMetadata

**Traits:**
- `MetadataStoreInterface`: Send + Sync
  - fn create_collection(&self, metadata: CollectionMetadata) -> Result<()>
  - fn get_collection(&self,
        collection_id: &CollectionId,) -> Result<Option<CollectionMetadata>>
  - fn update_collection(&self,
        collection_id: &CollectionId,
        metadata: CollectionMetadata,) -> Result<()>
  - fn delete_collection(&self, collection_id: &CollectionId) -> Result<bool>
  - fn list_collections(&self,
        filter: Option<MetadataFilter>,) -> Result<Vec<CollectionMetadata>>
  - fn update_stats(&self,
        collection_id: &CollectionId,
        vector_delta: i64,
        size_delta: i64,) -> Result<()>
  - fn batch_operations(&self, operations: Vec<MetadataOperation>) -> Result<()>
  - fn get_system_metadata(&self) -> Result<SystemMetadata>
  - fn update_system_metadata(&self, metadata: SystemMetadata) -> Result<()>
  - fn health_check(&self) -> Result<bool>
  - fn get_storage_stats(&self) -> Result<MetadataStorageStats>

##### storage::metadata::single_index

**Structs:**
- `CollectionIndexEntry`
  - pub record: Arc<CollectionRecord>
  - pub name_key: String
  - pub uuid_key: String
- `SingleIndexMetrics`
  - pub total_collections: usize
  - pub memory_usage_bytes: usize
  - pub lookups_by_uuid: u64
  - pub lookups_by_name: u64
  - pub cache_hits: u64
  - pub cache_misses: u64
  - pub avg_lookup_time_ns: u64
  - pub last_rebuild_timestamp: Option<i64>
- `SingleCollectionIndex`
  - private store: UUID -> CollectionIndexEntry
  - private entries: DashMap<String
  - private metrics: Arc<RwLock<SingleIndexMetrics>>
- `ThreadSafeSingleIndex`
  - private index: SingleCollectionIndex

##### storage::metadata::store

**Structs:**
- `MetadataStoreConfig`
  - pub metadata_base_dir: PathBuf
  - pub metadata_storage_urls: Vec<String>
  - pub enable_atomic_operations: bool
  - pub cache_config: MetadataCacheConfig
  - pub backup_config: Option<MetadataBackupConfig>
  - pub replication_config: Option<MetadataReplicationConfig>
- `MetadataCacheConfig`
  - pub enabled: bool
  - pub max_collections: usize
  - pub ttl_seconds: u64
  - pub preload_hot_collections: bool
  - pub eviction_strategy: CacheEvictionStrategy
- `MetadataBackupConfig`
  - pub enabled: bool
  - pub frequency_minutes: u32
  - pub retain_count: u32
  - pub backup_urls: Vec<String>
  - pub incremental: bool
  - pub compress_backups: bool
- `MetadataReplicationConfig`
  - pub enabled: bool
  - pub replication_factor: u32
  - pub replica_urls: Vec<String>
  - pub consistency_level: ConsistencyLevel
  - pub auto_failover: bool
- `MetadataStore`
  - private config: MetadataStoreConfig
  - private atomic_store: Option<Arc<AtomicMetadataStore>>
  - private wal_manager: Arc<MetadataWalManager>
  - private filesystem: Arc<FilesystemFactory>
  - private system_metadata: Arc<tokio::sync::RwLock<SystemMetadata>>
  - private background_tasks: Vec<tokio::task::JoinHandle<()>>

**Enums:**
- `CacheEvictionStrategy`
  - LRU
  - LFU
  - TTL
  - AccessPattern
- `ConsistencyLevel`
  - Eventual
  - Strong
  - Quorum

##### storage::metadata::unified_index

**Structs:**
- `IndexPerformanceMetrics`
  - pub total_collections: usize
  - pub memory_usage_bytes: usize
  - pub uuid_lookups: u64
  - pub name_lookups: u64
  - pub cache_hits: u64
  - pub cache_misses: u64
  - pub avg_lookup_time_ns: u64
  - pub last_rebuild_timestamp: Option<i64>
- `UnifiedCollectionIndex`
  - private store: UUID -> CollectionRecord (most critical for storage/WAL operations)
  - private collections: DashMap<String
  - private index: Name -> UUID (critical for user API queries)
  - private name_to_uuid: DashMap<String
  - private metrics: Arc<parking_lot::RwLock<IndexPerformanceMetrics>>
- `ThreadSafeUnifiedIndex`
  - private index: UnifiedCollectionIndex

##### storage::metadata::wal

**Structs:**
- `MetadataWalConfig`
  - pub base_config: WalConfig
  - pub keep_all_in_memory: bool
  - pub metadata_flush_threshold: usize
  - pub enable_metadata_cache: bool
  - pub cache_ttl_seconds: u64
- `VersionedCollectionMetadata`
  - pub id: CollectionId
  - pub name: String
  - pub dimension: usize
  - pub distance_metric: String
  - pub indexing_algorithm: String
  - pub created_at: DateTime<Utc>
  - pub updated_at: DateTime<Utc>
  - pub version: u64
  - pub vector_count: u64
  - pub total_size_bytes: u64
  - pub config: HashMap<String
  - private serde_json: :Value>
  - pub description: Option<String>
  - pub tags: Vec<String>
  - pub owner: Option<String>
  - pub access_pattern: AccessPattern
  - pub retention_policy: Option<RetentionPolicy>
- `RetentionPolicy`
  - pub retain_days: u32
  - pub auto_archive: bool
  - pub auto_delete: bool
- `MetadataWalManager`
  - private wal_strategy: Box<dyn WalStrategy>
  - private config: MetadataWalConfig
  - private metadata_cache: Arc<tokio::sync::RwLock<HashMap<CollectionId
  - private cache_timestamps: Arc<tokio::sync::RwLock<HashMap<CollectionId
  - private stats: Arc<tokio::sync::RwLock<MetadataStats>>
- `MetadataStats`
  - pub total_collections: u64
  - pub cache_hits: u64
  - pub cache_misses: u64
  - pub wal_writes: u64
  - pub wal_reads: u64
- `SystemMetadata`
  - pub version: String
  - pub node_id: String
  - pub cluster_name: String
  - pub created_at: DateTime<Utc>
  - pub updated_at: DateTime<Utc>
  - pub total_collections: u64
  - pub total_vectors: u64
  - pub total_size_bytes: u64
  - pub config: HashMap<String
  - private serde_json: :Value>

**Enums:**
- `AccessPattern`
  - Hot
  - Normal
  - Cold
  - Archive

##### storage::mmap::mod

**Structs:**
- `IndexEntry`
  - private offset: u64
  - private length: u32
- `MmapSstFile`
  - private mmap: memmap2::Mmap
  - private index: BTreeMap<VectorId
- `MmapReader`
  - private collection_id: CollectionId
  - private data_dir: PathBuf
  - private sst_files: Arc<RwLock<Vec<MmapSstFile>>>

##### storage::search::mod

**Structs:**
- `SearchResult`
  - pub id: VectorId
  - pub vector: Option<Vec<f32>>
  - pub metadata: HashMap<String
  - pub score: Option<f32>
  - pub distance: Option<f32>
  - pub rank: Option<usize>
- `SearchPlan`
  - pub operation: SearchOperation
  - pub estimated_cost: SearchCost
  - pub execution_strategy: ExecutionStrategy
  - pub index_usage: Vec<IndexUsage>
  - pub pushdown_operations: Vec<PushdownOperation>
- `SearchCost`
  - pub cpu_cost: f64
  - pub io_cost: f64
  - pub memory_cost: f64
  - pub network_cost: f64
  - pub total_estimated_ms: f64
  - pub cardinality_estimate: usize
- `IndexUsage`
  - pub index_name: String
  - pub index_type: String
  - pub selectivity: f64
  - pub scan_type: String
- `SearchStats`
  - pub operation_type: String
  - pub execution_time_ms: f64
  - pub rows_scanned: usize
  - pub rows_returned: usize
  - pub indexes_used: Vec<String>
  - pub cache_hits: usize
  - pub cache_misses: usize
  - pub io_operations: usize
- `IndexInfo`
  - pub name: String
  - pub index_type: String
  - pub columns: Vec<String>
  - pub unique: bool
  - pub size_bytes: usize
  - pub cardinality: usize
  - pub last_updated: chrono::DateTime<chrono::Utc>
- `IndexSpec`
  - pub name: String
  - pub index_type: String
  - pub columns: Vec<String>
  - pub unique: bool
  - pub parameters: HashMap<String
- `ViperSearchEngine`
- `LsmSearchEngine`
- `RocksDbSearchEngine`
- `MemorySearchEngine`
- `SearchOptimizer`
  - pub collection_stats: HashMap<CollectionId
  - pub index_stats: HashMap<String
- `CollectionStats`
  - pub total_vectors: usize
  - pub avg_vector_size: usize
  - pub metadata_cardinality: HashMap<String
  - pub data_distribution: DataDistribution
- `IndexStats`
  - pub selectivity: f64
  - pub access_frequency: usize
  - pub last_used: chrono::DateTime<chrono::Utc>

**Enums:**
- `SearchOperation`
  - ExactLookup{ vector_id: VectorId
- `FilterCondition`
  - Equals(Value)
  - NotEquals(Value)
  - GreaterThan(Value)
  - GreaterThanOrEqual(Value)
  - LessThan(Value)
  - LessThanOrEqual(Value)
  - In(Vec<Value>)
  - NotIn(Vec<Value>)
  - Contains(String),   // For string/array fields
  - StartsWith(String), // For string fields
  - IsNull
  - IsNotNull
  - And(Vec<FilterCondition>)
  - Or(Vec<FilterCondition>)
- `HybridSearchStrategy`
  - FilterFirst
  - SearchFirst
  - Parallel
  - Auto
- `ExecutionStrategy`
  - FullScan
  - IndexScan{ index_name: String
- `PushdownOperation`
  - Filter{ condition: FilterCondition
- `DataDistribution`
  - Uniform
  - Normal{ mean: f64, std_dev: f64

**Traits:**
- `SearchEngine`: Send + Sync
  - fn engine_name(&self) -> &'static str
  - fn create_plan(&self,
        collection_id: &CollectionId,
        operation: SearchOperation,) -> Result<SearchPlan>
  - fn execute_search(&self,
        collection_id: &CollectionId,
        plan: SearchPlan,) -> Result<Vec<SearchResult>>
  - fn search(&self,
        collection_id: &CollectionId,
        operation: SearchOperation,) -> Result<Vec<SearchResult>>

##### storage::search_index

**Structs:**
- `SearchRequest`
  - pub query: Vec<f32>
  - pub k: usize
  - pub collection_id: CollectionId
  - pub filter: Option<Box<dyn Fn(&HashMap<String
  - private serde_json: :Value>) -> bool + Send + Sync>>
- `IndexConfig`
  - pub algorithm: String
  - pub distance_metric: String
  - pub parameters: HashMap<String
  - private serde_json: :Value>
- `SearchIndexManager`
  - private indexes: Arc<RwLock<HashMap<CollectionId
  - private configs: Arc<RwLock<HashMap<CollectionId
  - private data_dir: PathBuf

##### storage::strategy::custom

**Structs:**
- `CustomStrategy`
  - private config: CollectionStrategyConfig
  - private name: String

##### storage::strategy::mod

**Structs:**
- `CollectionStrategyConfig`
  - pub strategy_type: StrategyType
  - pub indexing_config: IndexingConfig
  - pub storage_config: StorageConfig
  - pub search_config: SearchConfig
  - pub performance_config: PerformanceConfig
- `IndexingConfig`
  - pub algorithm: IndexingAlgorithm
  - pub parameters: HashMap<String
  - private serde_json: :Value>
  - pub secondary_algorithm: Option<IndexingAlgorithm>
- `StorageConfig`
  - pub engine_type: StorageEngineType
  - pub parameters: HashMap<String
  - private serde_json: :Value>
  - pub compression: CompressionConfig
  - pub tiering: Option<TieringConfig>
- `SearchConfig`
  - pub distance_metric: DistanceMetric
  - pub parameters: HashMap<String
  - private serde_json: :Value>
  - pub enable_optimization: bool
  - pub caching: Option<SearchCacheConfig>
- `PerformanceConfig`
  - pub num_threads: usize
  - pub memory_limit_mb: usize
  - pub enable_simd: bool
  - pub enable_gpu: bool
  - pub batch_config: BatchConfig
- `BatchConfig`
  - pub batch_size: usize
  - pub batch_timeout_ms: u64
  - pub adaptive_batching: bool
- `CompressionConfig`
  - pub enabled: bool
  - pub algorithm: String
  - pub level: u8
- `TieringConfig`
  - pub enabled: bool
  - pub tiers: Vec<TierConfig>
- `TierConfig`
  - pub name: String
  - pub access_threshold: f64
  - pub storage_type: String
- `SearchCacheConfig`
  - pub enabled: bool
  - pub cache_size_mb: usize
  - pub ttl_seconds: u64
- `SearchFilter`
  - pub metadata_filters: HashMap<String
  - private serde_json: :Value>
  - pub custom_filter: Option<Box<dyn Fn(&VectorRecord) -> bool + Send + Sync>>
- `SearchResult`
  - pub id: VectorId
  - pub score: f32
  - pub vector: Option<Vec<f32>>
  - pub metadata: Option<HashMap<String
  - private serde_json: :Value>>
  - pub collection_id: CollectionId
- `FlushResult`
  - pub entries_flushed: usize
  - pub bytes_written: u64
  - pub flush_time_ms: u64
  - pub strategy_metadata: HashMap<String
  - private serde_json: :Value>
- `CompactionResult`
  - pub entries_compacted: usize
  - pub space_reclaimed: u64
  - pub compaction_time_ms: u64
  - pub efficiency_metrics: HashMap<String
- `StrategyStats`
  - pub strategy_type: StrategyType
  - pub total_operations: u64
  - pub avg_latency_ms: f64
  - pub index_build_time_ms: u64
  - pub search_accuracy: f64
  - pub memory_usage_bytes: u64
  - pub storage_efficiency: f64
- `OptimizationResult`
  - pub improvement_ratio: f64
  - pub optimization_time_ms: u64
  - pub optimization_type: String
  - pub before_metrics: HashMap<String
  - pub after_metrics: HashMap<String
- `StrategyRegistry`
  - private strategies: HashMap<CollectionId
  - private default_strategy_factory: Box<dyn Fn(&CollectionMetadata) -> Result<Arc<dyn CollectionStrategy>> + Send + Sync>

**Enums:**
- `StrategyType`
  - Standard
  - HighPerformance
  - Viper
  - Custom{ name: String
- `IndexingAlgorithm`
  - HNSW{
  - m: usize,               // Number of connections
  - ef_construction: usize, // Size of dynamic candidate list
  - ef_search: usize,       // Size of search candidate list
- `StorageEngineType`
  - LSM
  - VIPER
  - MMAP
  - Custom{ name: String
- `DistanceMetric`
  - Cosine
  - Euclidean
  - DotProduct
  - Manhattan
  - Hamming
  - Jaccard

**Traits:**
- `CollectionStrategy`: Send + Sync
  - fn strategy_name(&self) -> &'static str
  - fn get_config(&self) -> CollectionStrategyConfig
  - fn initialize(&mut self,
        collection_id: CollectionId,
        metadata: &CollectionMetadata,) -> Result<()>
  - fn flush_entries(&self,
        collection_id: &CollectionId,
        entries: Vec<WalEntry>,) -> Result<FlushResult>
  - fn compact_collection(&self, collection_id: &CollectionId) -> Result<CompactionResult>
  - fn search_vectors(&self,
        collection_id: &CollectionId,
        query: Vec<f32>,
        k: usize,
        filter: Option<SearchFilter>,) -> Result<Vec<SearchResult>>
  - fn add_vector(&self,
        collection_id: &CollectionId,
        vector_record: &VectorRecord,) -> Result<()>
  - fn update_vector(&self,
        collection_id: &CollectionId,
        vector_record: &VectorRecord,) -> Result<()>
  - fn remove_vector(&self,
        collection_id: &CollectionId,
        vector_id: &VectorId,) -> Result<bool>
  - fn get_stats(&self, collection_id: &CollectionId) -> Result<StrategyStats>
  - fn optimize_collection(&self, collection_id: &CollectionId) -> Result<OptimizationResult>

##### storage::strategy::standard

**Structs:**
- `StandardStrategy`
  - private config: CollectionStrategyConfig
  - private lsm_trees: HashMap<CollectionId
  - private hnsw_indexes: HashMap<CollectionId
  - private search_engines: HashMap<CollectionId
- `HnswIndex`
- `StandardSearchEngine`
- `HnswStats`
  - private total_operations: u64
  - private avg_latency_ms: f64
  - private build_time_ms: u64
  - private search_accuracy: f64
  - private memory_usage: u64
- `LsmStats`
  - private total_operations: u64
  - private avg_latency_ms: f64
  - private memory_usage: u64
  - private compression_ratio: f64
- `CompactionStats`
  - private entries_compacted: usize
  - private space_reclaimed: u64
  - private compression_ratio: f64
  - private read_amplification: f64
  - private write_amplification: f64

**Traits:**
- `LsmTreeStrategy`
  - fn new_for_strategy(collection_id: &CollectionId,
        config: &super::StorageConfig,) -> Result<LsmTree>
  - fn get_stats(&self) -> Result<LsmStats>
  - fn compact(&self) -> Result<CompactionStats>

##### storage::strategy::viper

**Structs:**
- `ViperStrategy`
  - private config: CollectionStrategyConfig
  - private flusher: Arc<ViperParquetFlusher>
  - private storage_engine: Arc<ViperStorageEngine>
  - private search_engine: Arc<ViperSearchEngine>

##### storage::tiered::mod

**Structs:**
- `TieredStorageConfig`
  - pub deployment_mode: DeploymentMode
  - pub storage_tiers: Vec<StorageTier>
  - pub tiering_policy: TieringPolicy
  - pub background_migration: BackgroundMigrationConfig
  - pub cache_configuration: CacheConfiguration
- `PersistentVolumeConfig`
  - pub name: String
  - pub size_gb: u32
  - pub storage_class: String
  - pub mount_path: PathBuf
  - pub performance_tier: PerformanceTier
- `LocalStorageConfig`
  - pub path: PathBuf
  - pub storage_type: LocalStorageType
  - pub size_gb: Option<u32>
  - pub performance_tier: PerformanceTier
  - pub raid_config: Option<RaidConfig>
- `RaidConfig`
  - pub raid_level: RaidLevel
  - pub devices: Vec<String>
  - pub stripe_size_kb: u32
- `OSOptimizationConfig`
  - pub mmap_enabled: bool
  - pub page_cache_optimization: PageCacheConfig
  - pub io_scheduler: IOScheduler
  - pub numa_awareness: bool
  - pub huge_pages_enabled: bool
  - pub direct_io_threshold_mb: Option<u32>
- `PageCacheConfig`
  - pub enabled: bool
  - pub drop_caches_on_memory_pressure: bool
  - pub readahead_kb: u32
  - pub dirty_ratio: u8
  - pub dirty_background_ratio: u8
- `ColdStartOptimization`
  - pub warm_cache_size_mb: u32
  - pub prefetch_strategies: Vec<PrefetchStrategy>
  - pub lazy_loading_enabled: bool
- `PVCConfig`
  - pub name: String
  - pub storage_class: String
  - pub size: String
  - pub access_modes: Vec<String>
  - pub mount_path: PathBuf
- `StorageTier`
  - pub tier_name: String
  - pub tier_level: TierLevel
  - pub storage_backend: StorageBackend
  - pub access_pattern: AccessPattern
  - pub retention_policy: RetentionPolicy
  - pub performance_characteristics: PerformanceCharacteristics
- `RetentionPolicy`
  - pub hot_retention_hours: u32
  - pub warm_retention_days: u32
  - pub cold_retention_months: u32
  - pub archive_retention_years: Option<u32>
  - pub auto_deletion_enabled: bool
- `PerformanceCharacteristics`
  - pub read_latency_ms: LatencyRange
  - pub write_latency_ms: LatencyRange
  - pub throughput_mbps: ThroughputRange
  - pub iops: IOPSRange
  - pub consistency_level: ConsistencyLevel
- `LatencyRange`
  - pub p50: f32
  - pub p95: f32
  - pub p99: f32
- `ThroughputRange`
  - pub read_mbps: u32
  - pub write_mbps: u32
- `IOPSRange`
  - pub read_iops: u32
  - pub write_iops: u32
- `TieringPolicy`
  - pub auto_tiering_enabled: bool
  - pub promotion_rules: Vec<PromotionRule>
  - pub demotion_rules: Vec<DemotionRule>
  - pub migration_schedule: MigrationSchedule
  - pub cost_optimization: CostOptimizationConfig
- `PromotionRule`
  - pub name: String
  - pub condition: TieringCondition
  - pub from_tier: TierLevel
  - pub to_tier: TierLevel
  - pub priority: u32
- `DemotionRule`
  - pub name: String
  - pub condition: TieringCondition
  - pub from_tier: TierLevel
  - pub to_tier: TierLevel
  - pub priority: u32
- `MigrationSchedule`
  - pub enabled: bool
  - pub migration_windows: Vec<MigrationWindow>
  - pub bandwidth_limit_mbps: u32
  - pub concurrent_migrations: u32
  - pub failure_retry_count: u32
- `MigrationWindow`
  - pub name: String
  - pub cron_schedule: String
  - pub duration_hours: u32
  - pub allowed_migrations: Vec<MigrationType>
- `CostOptimizationConfig`
  - pub enabled: bool
  - pub target_cost_per_gb_per_month: f32
  - pub cost_monitoring_interval_hours: u32
  - pub automatic_adjustments: bool
  - pub cost_alerts: Vec<CostAlert>
- `CostAlert`
  - pub threshold_percent: f32
  - pub notification_channels: Vec<String>
  - pub escalation_levels: Vec<EscalationLevel>
- `EscalationLevel`
  - pub level: u32
  - pub delay_minutes: u32
  - pub actions: Vec<AutomatedAction>
- `BackgroundMigrationConfig`
  - pub enabled: bool
  - pub worker_threads: u32
  - pub migration_batch_size: u32
  - pub throttling: ThrottlingConfig
  - pub monitoring: MigrationMonitoringConfig
- `ThrottlingConfig`
  - pub max_bandwidth_mbps: u32
  - pub max_iops: u32
  - pub backoff_on_error: bool
  - pub priority_queue_enabled: bool
- `MigrationMonitoringConfig`
  - pub progress_reporting_interval_seconds: u32
  - pub success_rate_threshold: f32
  - pub retry_failed_migrations: bool
  - pub dead_letter_queue_enabled: bool
- `CacheConfiguration`
  - pub l1_cache: L1CacheConfig
  - pub l2_cache: L2CacheConfig
  - pub distributed_cache: Option<DistributedCacheConfig>
  - pub cache_eviction: CacheEvictionPolicy
- `L1CacheConfig`
  - pub enabled: bool
  - pub max_size_mb: u32
  - pub ttl_seconds: u32
  - pub cache_type: L1CacheType
- `L2CacheConfig`
  - pub enabled: bool
  - pub cache_path: PathBuf
  - pub max_size_gb: u32
  - pub block_size_kb: u32
  - pub write_through: bool
- `DistributedCacheConfig`
  - pub cache_type: DistributedCacheType
  - pub nodes: Vec<String>
  - pub replication_factor: u32
  - pub consistency_level: CacheConsistencyLevel
- `CacheEvictionPolicy`
  - pub memory_pressure_threshold: f32
  - pub eviction_strategy: EvictionStrategy
  - pub writeback_on_eviction: bool
- `TieredStorageManager`
  - private config: TieredStorageConfig
  - private storage_backends: HashMap<TierLevel
  - private migration_engine: MigrationEngine
  - private cache_manager: CacheManager
  - private metrics_collector: MetricsCollector
- `ObjectMetadata`
  - pub size_bytes: u64
  - pub last_modified: chrono::DateTime<chrono::Utc>
  - pub access_count: u64
  - pub last_accessed: chrono::DateTime<chrono::Utc>
  - pub content_type: String
  - pub compression: Option<String>
  - pub encryption: Option<String>
- `MigrationEngine`
  - private config: BackgroundMigrationConfig
  - private migration_queue: tokio::sync::mpsc::Receiver<MigrationTask>
  - private active_migrations: HashMap<String
- `MigrationTask`
  - pub task_id: String
  - pub object_key: String
  - pub from_tier: TierLevel
  - pub to_tier: TierLevel
  - pub priority: MigrationPriority
  - pub estimated_size_bytes: u64
- `MigrationStatus`
  - pub task_id: String
  - pub status: MigrationState
  - pub progress_percent: f32
  - pub started_at: chrono::DateTime<chrono::Utc>
  - pub estimated_completion: Option<chrono::DateTime<chrono::Utc>>
  - pub error_message: Option<String>
- `CacheManager`
  - private l1_cache: Option<Box<dyn CacheTrait>>
  - private l2_cache: Option<Box<dyn CacheTrait>>
  - private distributed_cache: Option<Box<dyn CacheTrait>>
  - private cache_stats: CacheStats
- `CacheStats`
  - pub hits: u64
  - pub misses: u64
  - pub evictions: u64
  - pub size_bytes: u64
  - pub item_count: u64
- `MetricsCollector`
  - private storage_metrics: HashMap<TierLevel
  - private migration_metrics: MigrationMetrics
  - private cache_metrics: HashMap<String
- `StorageMetrics`
  - pub reads_per_second: f32
  - pub writes_per_second: f32
  - pub latency_percentiles: LatencyRange
  - pub throughput_mbps: f32
  - pub error_rate: f32
  - pub utilization_percent: f32
- `MigrationMetrics`
  - pub active_migrations: u32
  - pub completed_migrations: u64
  - pub failed_migrations: u64
  - pub average_migration_time_seconds: f32
  - pub data_migrated_gb: f64

**Enums:**
- `DeploymentMode`
  - Container{
  - ephemeral_storage_gb: u32
  - persistent_volumes: Vec<PersistentVolumeConfig>
  - cloud_storage_backend: CloudStorageBackend
- `LocalStorageType`
  - NVMe{ iops: u32, throughput_mbps: u32
- `RaidLevel`
  - RAID0,  // Striping for performance
  - RAID1,  // Mirroring for reliability
  - RAID5,  // Parity for balance
  - RAID10, // Stripe + Mirror
- `CloudStorageBackend`
  - AWS{
  - s3_bucket: String
  - storage_classes: HashMap<String, S3StorageClass>
  - intelligent_tiering: bool
  - cross_region_replication: Option<String>
- `S3StorageClass`
  - Standard
  - StandardIA, // Infrequent Access
  - OneZoneIA
  - Glacier
  - GlacierDeepArchive
  - IntelligentTiering
- `AzureAccessTier`
  - Hot
  - Cool
  - Archive
- `GCPStorageClass`
  - Standard
  - Nearline
  - Coldline
  - Archive
- `CloudSyncStrategy`
  - ActiveActive,  // Sync to both clouds
  - ActivePassive, // Primary with backup
  - Geographic,    // Route by user location
- `IOScheduler`
  - CFQ,      // Completely Fair Queuing
  - NOOP,     // No Operation (for SSDs)
  - Deadline, // Deadline scheduler
  - MQ,       // Multi-queue block layer
- `PrefetchStrategy`
  - RecentlyAccessed
  - PopularCollections
  - TenantSpecific
  - None
- `TierLevel`
  - UltraHot, // L1 cache equivalent - memory/NVMe
  - Hot,      // L2 cache equivalent - local SSD
  - Warm,     // L3 cache equivalent - local HDD or network storage
  - Cold,     // Remote storage - S3 Standard
  - Archive,  // Long-term storage - S3 Glacier
- `StorageBackend`
  - MMAP{
  - file_path: PathBuf
  - advise_random: bool
  - advise_sequential: bool
  - populate_pages: bool
- `SyncStrategy`
  - WriteThrough, // Sync on every write
  - WriteBack,    // Batch sync
  - Async,        // Fire and forget
- `BlockVolumeType`
  - EBS{
  - volume_type: String
  - iops: Option<u32>
- `FilesystemType`
  - EXT4
  - XFS
  - ZFS
  - BTRFS
- `AccessPattern`
  - Sequential{
  - prefetch_size_mb: u32
- `ConsistencyLevel`
  - Immediate
  - Eventual{ max_delay_ms: u32
- `PerformanceTier`
  - UltraHigh, // NVMe, memory
  - High,      // SSD
  - Medium,    // Fast HDD
  - Low,       // Standard HDD
  - Archive,   // Tape, cold storage
- `TieringCondition`
  - AccessFrequency{
  - min_accesses_per_hour: u32
- `LogicalOperator`
  - AND
  - OR
  - NOT
- `MigrationType`
  - Promotion
  - Demotion
  - Rebalancing
  - Compression
- `AutomatedAction`
  - DemoteToLowerTier
  - EnableCompression
  - ReduceReplicationFactor
  - NotifyAdmins
  - DisableAutoScaling
- `L1CacheType`
  - LRU,  // Least Recently Used
  - LFU,  // Least Frequently Used
  - ARC,  // Adaptive Replacement Cache
  - TwoQ, // Two Queue algorithm
- `DistributedCacheType`
  - Redis{ cluster_mode: bool
- `CacheConsistencyLevel`
  - Strong
  - Eventual
  - Session
- `EvictionStrategy`
  - LRU
  - LFU
  - Random
  - TTL
  - CostBased{ cost_function: String
- `MigrationPriority`
  - Critical
  - High
  - Medium
  - Low
  - Background
- `MigrationState`
  - Queued
  - InProgress
  - Completed
  - Failed
  - Cancelled

**Traits:**
- `StorageBackendTrait`: Send + Sync
  - fn read(&self, key: &str) -> crate::Result<Option<Vec<u8>>>
  - fn write(&self, key: &str, data: &[u8]) -> crate::Result<()>
  - fn delete(&self, key: &str) -> crate::Result<()>
  - fn list(&self, prefix: &str) -> crate::Result<Vec<String>>
  - fn metadata(&self, key: &str) -> crate::Result<Option<ObjectMetadata>>
  - fn get_performance_characteristics(&self) -> &PerformanceCharacteristics
  - fn get_tier_level(&self) -> TierLevel
- `CacheTrait`: Send + Sync
  - fn get(&self, key: &str) -> crate::Result<Option<Vec<u8>>>
  - fn put(&self, key: &str, data: &[u8], ttl: Option<u64>) -> crate::Result<()>
  - fn evict(&self, key: &str) -> crate::Result<()>
  - fn clear(&self) -> crate::Result<()>
  - fn get_stats(&self) -> CacheStats

##### storage::validation

**Structs:**
- `ConfigValidator`

##### storage::vector::coordinator

**Structs:**
- `VectorStorageCoordinator`
  - private storage_engines: Arc<RwLock<HashMap<String
  - private search_engine: Arc<UnifiedSearchEngine>
  - private index_manager: Arc<UnifiedIndexManager>
  - private operation_router: Arc<OperationRouter>
  - private performance_monitor: Arc<PerformanceMonitor>
  - private config: CoordinatorConfig
  - private stats: Arc<RwLock<CoordinatorStats>>
- `CoordinatorConfig`
  - pub default_engine: String
  - pub enable_cross_engine: bool
  - pub enable_caching: bool
  - pub cache_size_mb: usize
  - pub enable_monitoring: bool
  - pub routing_strategy: RoutingStrategy
- `OperationRouter`
  - private rules: Arc<RwLock<Vec<RoutingRule>>>
  - private collection_assignments: Arc<RwLock<HashMap<CollectionId
  - private engine_loads: Arc<RwLock<HashMap<String
  - private strategy: RoutingStrategy
- `RoutingRule`
  - pub name: String
  - pub condition: RoutingCondition
  - pub target_engine: String
  - pub priority: u32
  - pub enabled: bool
- `PerformanceProfile`
  - pub max_latency_ms: f64
  - pub min_throughput: f64
  - pub min_accuracy: f64
  - pub max_memory_mb: Option<usize>
- `EngineLoad`
  - pub ops_per_second: f64
  - pub cpu_utilization: f64
  - pub memory_usage_mb: f64
  - pub queue_depth: usize
  - pub avg_response_time_ms: f64
- `PerformanceMonitor`
  - private history: Arc<RwLock<Vec<PerformanceSnapshot>>>
  - private collection_profiles: Arc<RwLock<HashMap<CollectionId
  - private engine_metrics: Arc<RwLock<HashMap<String
  - private config: MonitoringConfig
- `PerformanceSnapshot`
  - pub timestamp: chrono::DateTime<chrono::Utc>
  - pub operation_type: VectorOperationType
  - pub collection_id: CollectionId
  - pub engine_name: String
  - pub latency_ms: f64
  - pub throughput: f64
  - pub success: bool
  - pub error_type: Option<String>
- `CollectionProfile`
  - pub collection_id: CollectionId
  - pub total_operations: u64
  - pub avg_latency_ms: f64
  - pub success_rate: f64
  - pub preferred_engine: Option<String>
  - pub access_patterns: AccessPatterns
  - pub data_characteristics: DataCharacteristics
- `AccessPatterns`
  - pub read_write_ratio: f64
  - pub peak_hours: Vec<u8>
  - pub seasonal_patterns: Vec<f64>
  - pub query_types: HashMap<String
- `DataCharacteristics`
  - pub avg_vector_dimension: usize
  - pub total_vectors: usize
  - pub data_distribution: DataDistribution
  - pub sparsity_ratio: f64
  - pub update_frequency: f64
- `EngineMetrics`
  - pub engine_name: String
  - pub total_operations: u64
  - pub avg_latency_ms: f64
  - pub throughput_ops_sec: f64
  - pub error_rate: f64
  - pub resource_utilization: ResourceUtilization
- `ResourceUtilization`
  - pub cpu_percent: f64
  - pub memory_mb: f64
  - pub disk_io_mb_sec: f64
  - pub network_io_mb_sec: f64
- `MonitoringConfig`
  - pub enabled: bool
  - pub sample_rate: f64
  - pub history_retention_days: u32
  - pub alert_thresholds: AlertThresholds
- `AlertThresholds`
  - pub max_latency_ms: f64
  - pub min_success_rate: f64
  - pub max_error_rate: f64
  - pub max_cpu_percent: f64
  - pub max_memory_mb: f64
- `CoordinatorStats`
  - pub total_operations: u64
  - pub operations_by_type: HashMap<VectorOperationType
  - pub operations_by_engine: HashMap<String
  - pub avg_routing_time_ms: f64
  - pub cache_hit_ratio: f64
  - pub cross_engine_operations: u64
- `PerformanceAnalytics`
  - pub avg_latency_ms: f64
  - pub throughput_ops_sec: f64
  - pub success_rate: f64
  - pub engine_performance: HashMap<String
  - pub recommendations: Vec<OptimizationRecommendation>
- `OptimizationRecommendation`
  - pub recommendation_type: String
  - pub description: String
  - pub expected_improvement: f64
  - pub confidence: f64

**Enums:**
- `RoutingStrategy`
  - CollectionBased
  - DataCharacteristics
  - LoadBalanced
  - RoundRobin
  - Custom(String)
- `RoutingCondition`
  - CollectionPattern(String)
  - DimensionRange{ min: usize, max: usize
- `VectorOperationType`
  - Insert
  - Update
  - Delete
  - Search
  - BatchInsert
  - BatchSearch

##### storage::vector::engines::viper_core

**Structs:**
- `FilterableColumn`
  - pub name: String
  - pub data_type: FilterableDataType
  - pub indexed: bool
  - pub supports_range: bool
  - pub estimated_cardinality: Option<usize>
- `ParquetSchemaDesign`
  - pub collection_id: CollectionId
  - pub fields: Vec<ParquetField>
  - pub filterable_columns: Vec<FilterableColumn>
  - pub partition_columns: Vec<String>
  - pub compression: ParquetCompression
  - pub row_group_size: usize
- `ParquetField`
  - pub name: String
  - pub field_type: ParquetFieldType
  - pub nullable: bool
  - pub indexed: bool
- `ProcessedVectorRecord`
  - pub id: String
  - pub vector: Vec<f32>
  - pub timestamp: DateTime<Utc>
  - pub filterable_data: HashMap<String
  - pub extra_meta: HashMap<String
- `ViperCoreEngine`
  - private config: ViperCoreConfig
  - private collections: Arc<RwLock<HashMap<CollectionId
  - private clusters: Arc<RwLock<HashMap<ClusterId
  - private partitions: Arc<RwLock<HashMap<PartitionId
  - private ml_models: Arc<RwLock<HashMap<CollectionId
  - private feature_models: Arc<RwLock<HashMap<CollectionId
  - private atomic_coordinator: Arc<AtomicOperationsCoordinator>
  - private schema_adapter: Arc<SchemaAdapter>
  - private writer_pool: Arc<ParquetWriterPool>
  - private stats: Arc<RwLock<ViperCoreStats>>
  - private filesystem: Arc<FilesystemFactory>
- `ViperCoreConfig`
  - pub enable_ml_clustering: bool
  - pub enable_background_compaction: bool
  - pub compression_config: CompressionConfig
  - pub schema_config: SchemaConfig
  - pub atomic_config: AtomicOperationsConfig
  - pub writer_pool_size: usize
  - pub stats_interval_secs: u64
- `ViperCollectionMetadata`
  - pub collection_id: CollectionId
  - pub dimension: usize
  - pub vector_count: usize
  - pub total_clusters: usize
  - pub storage_format_preference: VectorStorageFormat
  - pub ml_model_version: Option<String>
  - pub feature_importance: Vec<f32>
  - pub compression_stats: CompressionStats
  - pub schema_version: u32
  - pub created_at: DateTime<Utc>
  - pub last_updated: DateTime<Utc>
  - private New: Server-side metadata filtering configuration
  - pub filterable_columns: Vec<FilterableColumn>
  - pub column_indexes: HashMap<String
  - pub flush_size_bytes: Option<usize>
- `CompressionStats`
  - pub sparse_compression_ratio: f32
  - pub dense_compression_ratio: f32
  - pub optimal_sparsity_threshold: f32
  - pub column_compression_ratios: Vec<f32>
- `CompressionConfig`
  - pub algorithm: CompressionAlgorithm
  - pub compression_level: u8
  - pub enable_adaptive_compression: bool
  - pub sparsity_threshold: f32
- `SchemaConfig`
  - pub enable_dynamic_schema: bool
  - pub filterable_fields: Vec<String>
  - pub max_metadata_fields: usize
  - pub enable_column_pruning: bool
- `AtomicOperationsConfig`
  - pub enable_staging_directories: bool
  - pub staging_cleanup_interval_secs: u64
  - pub max_concurrent_operations: usize
  - pub enable_read_during_staging: bool
- `ClusterMetadata`
  - pub cluster_id: ClusterId
  - pub collection_id: CollectionId
  - pub centroid: Vec<f32>
  - pub vector_count: usize
  - pub storage_format: VectorStorageFormat
  - pub compression_ratio: f32
  - pub last_updated: DateTime<Utc>
- `PartitionMetadata`
  - pub partition_id: PartitionId
  - pub cluster_id: ClusterId
  - pub file_url: String
  - pub vector_count: usize
  - pub size_bytes: usize
  - pub compression_ratio: f32
  - pub created_at: DateTime<Utc>
- `ClusterPredictionModel`
  - pub model_id: String
  - pub collection_id: CollectionId
  - pub version: String
  - pub accuracy: f32
  - pub feature_weights: Vec<f32>
  - pub cluster_centroids: Vec<Vec<f32>>
  - pub trained_at: DateTime<Utc>
- `FeatureImportanceModel`
  - pub model_id: String
  - pub collection_id: CollectionId
  - pub importance_scores: Vec<f32>
  - pub selected_features: Vec<usize>
  - pub compression_benefit: f32
  - pub trained_at: DateTime<Utc>
- `ViperCoreStats`
  - pub total_operations: u64
  - pub insert_operations: u64
  - pub search_operations: u64
  - pub flush_operations: u64
  - pub compaction_operations: u64
  - pub avg_compression_ratio: f32
  - pub avg_ml_prediction_accuracy: f32
  - pub total_storage_size_bytes: u64
  - pub active_clusters: usize
  - pub active_partitions: usize
- `AtomicOperationsCoordinator`
  - private lock_manager: Arc<CollectionLockManager>
  - private staging_manager: Arc<StagingOperationsManager>
  - private active_operations: Arc<RwLock<HashMap<CollectionId
  - private config: AtomicOperationsConfig
- `CollectionLockManager`
  - private collection_locks: Arc<RwLock<HashMap<CollectionId
  - private filesystem: Arc<FilesystemFactory>
- `CollectionLock`
  - private reader_count: usize
  - private has_writer: bool
  - private pending_operations: Vec<OperationType>
  - private acquired_at: DateTime<Utc>
  - private allow_reads_during_staging: bool
- `StagingOperationsManager`
  - private staging_dirs: Arc<RwLock<HashMap<String
  - private filesystem: Arc<FilesystemFactory>
  - private cleanup_interval_secs: u64
- `StagingDirectory`
  - pub path: String
  - pub operation_type: OperationType
  - pub collection_id: CollectionId
  - pub created_at: DateTime<Utc>
  - pub expected_completion: DateTime<Utc>
- `AtomicOperation`
  - pub operation_id: String
  - pub operation_type: OperationType
  - pub collection_id: CollectionId
  - pub staging_path: Option<String>
  - pub started_at: DateTime<Utc>
  - pub status: OperationStatus
- `SchemaAdapter`
  - private strategies: HashMap<String
  - private config: SchemaConfig
  - private schema_cache: Arc<RwLock<HashMap<String
- `DefaultSchemaStrategy`
  - private filterable_fields: Vec<String>
- `ParquetWriterPool`
  - private writers: Arc<Mutex<Vec<DefaultParquetWriter>>>
  - private pool_size: usize
  - private writer_factory: Arc<DefaultParquetWriterFactory>
- `CollectionLockGuard`
- `DefaultParquetWriter`
- `DefaultParquetWriterFactory`

**Enums:**
- `FilterableDataType`
  - String
  - Integer
  - Float
  - Boolean
  - DateTime
  - Array(Box<FilterableDataType>)
- `ColumnIndexType`
  - Hash
  - BTree
  - BloomFilter
  - FullText
- `ParquetFieldType`
  - String
  - Int64
  - Double
  - Boolean
  - Timestamp
  - Binary
  - List(Box<ParquetFieldType>)
  - Map, // For extra_meta field
- `ParquetCompression`
  - Uncompressed
  - Snappy
  - Gzip
  - Lzo
  - Zstd
- `VectorStorageFormat`
  - Dense
  - Sparse
  - Adaptive
- `CompressionAlgorithm`
  - Snappy
  - Zstd
  - Lz4
  - Brotli
- `OperationType`
  - Read
  - Insert
  - Flush
  - Compaction
  - SchemaEvolution
- `OperationStatus`
  - Pending
  - InProgress
  - Staging
  - Finalizing
  - Completed
  - Failed(String)

**Traits:**
- `SchemaGenerationStrategy`: Send + Sync
  - fn generate_schema(&self, records: &[VectorRecord]) -> Result<Arc<Schema>>
  - fn get_filterable_fields(&self) -> &[String]
  - fn name(&self) -> &'static str
- `ParquetWriter`: Send + Sync
  - fn write_batch(&mut self, batch: RecordBatch, path: &str) -> Result<()>
  - fn close(&mut self) -> Result<()>
- `ParquetWriterFactory`: Send + Sync
  - fn create_writer(&self) -> Result<DefaultParquetWriter>

##### storage::vector::engines::viper_factory

**Structs:**
- `ViperFactory`
  - private default_config: ViperConfiguration
  - private strategy_registry: HashMap<String
  - private processor_registry: HashMap<String
  - private builder_cache: HashMap<String
- `ViperConfiguration`
  - pub storage_config: ViperStorageConfig
  - pub schema_config: ViperSchemaConfig
  - pub processing_config: ViperProcessingConfig
  - pub ttl_config: TTLConfig
  - pub compaction_config: CompactionConfig
  - pub optimization_config: OptimizationConfig
- `ViperStorageConfig`
  - pub enable_clustering: bool
  - pub cluster_count: usize
  - pub min_vectors_per_partition: usize
  - pub max_vectors_per_partition: usize
  - pub enable_dictionary_encoding: bool
  - pub target_compression_ratio: f64
  - pub parquet_block_size: usize
  - pub row_group_size: usize
  - pub enable_column_stats: bool
  - pub enable_bloom_filters: bool
- `ViperSchemaConfig`
  - pub enable_dynamic_schema: bool
  - pub max_filterable_fields: usize
  - pub enable_ttl_fields: bool
  - pub enable_extra_metadata: bool
  - pub schema_version: u32
  - pub enable_schema_evolution: bool
- `ViperProcessingConfig`
  - pub enable_preprocessing: bool
  - pub enable_postprocessing: bool
  - pub strategy_selection: StrategySelectionMode
  - pub batch_config: BatchProcessingConfig
  - pub enable_ml_optimizations: bool
- `TTLConfig`
  - pub enabled: bool
  - pub default_ttl: Option<Duration>
  - pub cleanup_interval: Duration
  - pub max_cleanup_batch_size: usize
  - pub enable_file_level_filtering: bool
  - pub min_expiration_age: Duration
- `CompactionConfig`
  - pub enabled: bool
  - pub min_files_for_compaction: usize
  - pub max_avg_file_size_kb: usize
  - pub max_files_per_compaction: usize
  - pub target_file_size_mb: usize
  - pub enable_ml_guidance: bool
  - pub priority_scheduling: bool
- `OptimizationConfig`
  - pub enable_adaptive_clustering: bool
  - pub enable_compression_optimization: bool
  - pub enable_feature_importance: bool
  - pub enable_access_pattern_learning: bool
  - pub optimization_interval_secs: u64
- `BatchProcessingConfig`
  - pub default_batch_size: usize
  - pub max_batch_size: usize
  - pub enable_dynamic_sizing: bool
  - pub batch_timeout_ms: u64
- `ViperConfigurationBuilder`
  - private config: ViperConfiguration
- `ProcessedBatch`
  - pub records: Vec<VectorRecord>
  - pub schema: Arc<Schema>
  - pub statistics: ProcessingStatistics
- `ProcessingStatistics`
  - pub records_processed: usize
  - pub processing_time_ms: u64
  - pub optimization_applied: Vec<String>
  - pub compression_ratio: f32
- `ViperSchemaStrategy`
  - private collection_id: CollectionId
  - private config: ViperSchemaConfig
  - private filterable_fields: Vec<String>
- `LegacySchemaStrategy`
  - private collection_id: CollectionId
  - private config: ViperSchemaConfig
- `TimeSeriesSchemaStrategy`
  - private collection_id: CollectionId
  - private config: ViperSchemaConfig
  - private time_precision: TimePrecision
- `StandardVectorProcessor`
  - private config: ViperProcessingConfig
- `TimeSeriesVectorProcessor`
  - private config: ViperProcessingConfig
  - private time_window_seconds: u64
- `SimilarityVectorProcessor`
  - private config: ViperProcessingConfig
  - private cluster_threshold: f32
- `ViperComponents`
  - pub configuration: ViperConfiguration
  - pub schema_strategy: Box<dyn SchemaGenerationStrategy>
  - pub processor: Box<dyn VectorProcessor>
  - pub builder: ViperConfigurationBuilder
- `CollectionConfig`
  - pub name: String
  - pub filterable_metadata_fields: Vec<String>
  - pub vector_dimension: Option<usize>
  - pub estimated_size: Option<usize>
- `ViperSchemaStrategyFactory`
- `LegacySchemaStrategyFactory`
- `TimeSeriesSchemaStrategyFactory`
- `StandardProcessorFactory`
- `TimeSeriesProcessorFactory`
- `SimilarityProcessorFactory`

**Enums:**
- `StrategySelectionMode`
  - Adaptive
  - Fixed(String)
  - MLGuided
  - UserDefined(String)
- `TimePrecision`
  - Millisecond
  - Microsecond
  - Nanosecond

**Traits:**
- `SchemaGenerationStrategy`: Send + Sync
  - fn generate_schema(&self) -> Result<Arc<Schema>>
  - fn get_filterable_fields(&self) -> &[String]
  - fn get_collection_id(&self) -> &CollectionId
  - fn supports_ttl(&self) -> bool
  - fn get_version(&self) -> u32
  - fn name(&self) -> &'static str
- `SchemaStrategyFactory`: Send + Sync
  - fn create_strategy(&self, config: &ViperSchemaConfig, collection_id: &CollectionId) -> Box<dyn SchemaGenerationStrategy>
  - fn name(&self) -> &'static str
- `ProcessorFactory`: Send + Sync
  - fn create_processor(&self, config: &ViperProcessingConfig) -> Box<dyn VectorProcessor>
  - fn name(&self) -> &'static str
- `VectorProcessor`: Send + Sync
  - fn process_records(&self, records: Vec<VectorRecord>) -> Result<ProcessedBatch>
  - fn name(&self) -> &'static str

##### storage::vector::engines::viper_pipeline

**Structs:**
- `ViperPipeline`
  - private processor: Arc<VectorRecordProcessor>
  - private flusher: Arc<ParquetFlusher>
  - private compaction_engine: Arc<CompactionEngine>
  - private config: ViperPipelineConfig
  - private stats: Arc<RwLock<ViperPipelineStats>>
  - private filesystem: Arc<FilesystemFactory>
- `ViperPipelineConfig`
  - pub processing_config: ProcessingConfig
  - pub flushing_config: FlushingConfig
  - pub compaction_config: CompactionConfig
  - pub enable_background_processing: bool
  - pub stats_interval_secs: u64
- `ProcessingConfig`
  - pub enable_preprocessing: bool
  - pub enable_postprocessing: bool
  - pub batch_size: usize
  - pub enable_compression: bool
  - pub sorting_strategy: SortingStrategy
- `FlushingConfig`
  - pub compression_algorithm: CompressionAlgorithm
  - pub compression_level: u8
  - pub enable_dictionary_encoding: bool
  - pub row_group_size: usize
  - pub write_batch_size: usize
  - pub enable_statistics: bool
- `CompactionConfig`
  - pub enable_ml_compaction: bool
  - pub worker_count: usize
  - pub compaction_interval_secs: u64
  - pub target_file_size_mb: usize
  - pub max_files_per_merge: usize
  - pub reclustering_quality_threshold: f32
- `ViperPipelineStats`
  - pub total_records_processed: u64
  - pub total_batches_flushed: u64
  - pub total_bytes_written: u64
  - pub total_compaction_operations: u64
  - pub avg_processing_time_ms: f64
  - pub avg_flush_time_ms: f64
  - pub avg_compaction_time_ms: f64
  - pub compression_ratio: f32
  - pub current_active_operations: usize
- `VectorRecordProcessor`
  - private config: ProcessingConfig
  - private schema_adapter: Arc<SchemaAdapter>
  - private stats: Arc<RwLock<ProcessingStats>>
- `ProcessingStats`
  - pub records_processed: u64
  - pub preprocessing_time_ms: u64
  - pub conversion_time_ms: u64
  - pub postprocessing_time_ms: u64
  - pub total_processing_time_ms: u64
- `SchemaAdapter`
  - private schema_cache: Arc<RwLock<HashMap<String
  - private strategies: HashMap<String
- `ParquetFlusher`
  - private config: FlushingConfig
  - private filesystem: Arc<FilesystemFactory>
  - private stats: Arc<RwLock<FlushingStats>>
  - private writer_pool: Arc<WriterPool>
- `FlushingStats`
  - pub batches_flushed: u64
  - pub bytes_written: u64
  - pub avg_flush_time_ms: f64
  - pub compression_ratio: f32
  - pub writer_pool_utilization: f32
- `WriterPool`
  - private writers: Arc<Mutex<Vec<DefaultParquetWriter>>>
  - private pool_size: usize
  - private writer_factory: Arc<DefaultParquetWriterFactory>
- `FlushResult`
  - pub entries_flushed: u64
  - pub bytes_written: u64
  - pub segments_created: u64
  - pub collections_affected: Vec<CollectionId>
  - pub flush_duration_ms: u64
  - pub compression_ratio: f32
- `CompactionEngine`
  - private config: CompactionConfig
  - private task_queue: Arc<Mutex<VecDeque<CompactionTask>>>
  - private active_compactions: Arc<RwLock<HashMap<CollectionId
  - private optimization_model: Arc<RwLock<Option<CompactionOptimizationModel>>>
  - private worker_handles: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>
  - private shutdown_sender: Arc<Mutex<Option<tokio::sync::broadcast::Sender<()>>>>
  - private stats: Arc<RwLock<CompactionStats>>
  - private filesystem: Arc<FilesystemFactory>
- `CompactionTask`
  - pub task_id: String
  - pub collection_id: CollectionId
  - pub compaction_type: CompactionType
  - pub priority: CompactionPriority
  - pub input_partitions: Vec<PartitionId>
  - pub expected_outputs: usize
  - pub optimization_hints: Option<CompactionOptimizationHints>
  - pub created_at: DateTime<Utc>
  - pub estimated_duration: Duration
- `CompactionOperation`
  - pub operation_id: String
  - pub collection_id: CollectionId
  - pub operation_type: CompactionType
  - pub started_at: DateTime<Utc>
  - pub progress: f32
  - pub status: CompactionStatus
- `CompactionOptimizationModel`
  - pub model_id: String
  - pub version: String
  - pub accuracy: f32
  - pub feature_weights: Vec<f32>
  - pub trained_at: DateTime<Utc>
- `CompactionOptimizationHints`
  - pub recommended_cluster_count: Option<usize>
  - pub important_features: Vec<usize>
  - pub compression_algorithm: Option<CompressionAlgorithm>
  - pub estimated_improvement: f32
- `CompactionStats`
  - pub total_operations: u64
  - pub successful_operations: u64
  - pub failed_operations: u64
  - pub avg_operation_time_ms: f64
  - pub total_bytes_processed: u64
  - pub total_bytes_saved: u64
  - pub avg_compression_improvement: f32

**Enums:**
- `SortingStrategy`
  - ByTimestamp
  - BySimilarity
  - ByMetadata(Vec<String>)
  - None
- `CompressionAlgorithm`
  - Snappy
  - Zstd{ level: u8
- `CompactionType`
  - FileMerging{
  - target_file_size_mb: usize
  - max_files_per_merge: usize
- `CompactionPriority`
  - Low
  - Normal
  - High
  - Critical
- `CompactionStatus`
  - Pending
  - Running
  - Finalizing
  - Completed
  - Failed(String)
- `ReorganizationStrategy`
  - ByFeatureImportance
  - ByAccessPattern
  - ByCompressionRatio

**Traits:**
- `VectorProcessor`
  - fn preprocess_records(&self, records: &mut [VectorRecord]) -> Result<()>
  - fn convert_to_batch(&self,
        records: &[VectorRecord],
        schema: &Arc<Schema>,) -> Result<RecordBatch>
  - fn postprocess_batch(&self, batch: RecordBatch) -> Result<RecordBatch>
- `SchemaGenerationStrategy`: Send + Sync
  - fn generate_schema(&self, records: &[VectorRecord]) -> Result<Arc<Schema>>
  - fn get_collection_id(&self) -> &CollectionId
  - fn get_filterable_fields(&self) -> &[String]

##### storage::vector::engines::viper_utilities

**Structs:**
- `ViperUtilities`
  - private stats_collector: Arc<PerformanceStatsCollector>
  - private ttl_service: Arc<Mutex<TTLCleanupService>>
  - private staging_coordinator: Arc<StagingOperationsCoordinator>
  - private partitioner: Arc<DataPartitioner>
  - private compression_optimizer: Arc<CompressionOptimizer>
  - private config: ViperUtilitiesConfig
  - private service_handles: Vec<tokio::task::JoinHandle<()>>
- `ViperUtilitiesConfig`
  - pub stats_config: StatsConfig
  - pub ttl_config: TTLConfig
  - pub staging_config: StagingConfig
  - pub partitioning_config: PartitioningConfig
  - pub compression_config: CompressionConfig
  - pub enable_background_services: bool
- `PerformanceStatsCollector`
  - private operation_metrics: Arc<RwLock<HashMap<String
  - private collection_stats: Arc<RwLock<HashMap<CollectionId
  - private global_stats: Arc<RwLock<GlobalViperStats>>
  - private config: StatsConfig
- `StatsConfig`
  - pub enable_detailed_tracking: bool
  - pub enable_realtime_metrics: bool
  - pub retention_period_hours: u64
  - pub collection_interval_secs: u64
  - pub enable_profiling: bool
- `OperationMetrics`
  - pub operation_type: String
  - pub collection_id: CollectionId
  - pub records_processed: u64
  - pub total_time_ms: u64
  - pub preprocessing_time_ms: u64
  - pub processing_time_ms: u64
  - pub postprocessing_time_ms: u64
  - pub bytes_written: u64
  - pub compression_ratio: f32
  - pub throughput_ops_sec: f64
  - pub timestamp: DateTime<Utc>
- `CollectionStats`
  - pub collection_id: CollectionId
  - pub total_operations: u64
  - pub total_records: u64
  - pub total_bytes: u64
  - pub avg_latency_ms: f64
  - pub avg_throughput_ops_sec: f64
  - pub avg_compression_ratio: f32
  - pub last_operation: Option<DateTime<Utc>>
  - pub error_count: u64
- `GlobalViperStats`
  - pub total_operations: u64
  - pub total_collections: usize
  - pub total_records_processed: u64
  - pub total_bytes_written: u64
  - pub avg_latency_ms: f64
  - pub avg_compression_ratio: f32
  - pub uptime_seconds: u64
  - pub cache_hit_ratio: f32
  - pub error_rate: f32
- `OperationStatsCollector`
  - private operation_start: Option<Instant>
  - private phase_timers: HashMap<String
  - private metrics: OperationMetrics
- `TTLCleanupService`
  - private config: TTLConfig
  - private filesystem: Arc<FilesystemFactory>
  - private partition_metadata: Arc<RwLock<HashMap<PartitionId
  - private cleanup_task: Option<tokio::task::JoinHandle<()>>
  - private stats: Arc<RwLock<TTLStats>>
- `TTLConfig`
  - pub enabled: bool
  - pub default_ttl: Option<Duration>
  - pub cleanup_interval: Duration
  - pub max_cleanup_batch_size: usize
  - pub enable_file_level_filtering: bool
  - pub min_expiration_age: Duration
  - pub enable_priority_scheduling: bool
- `TTLStats`
  - pub total_cleanup_runs: u64
  - pub vectors_expired: u64
  - pub files_deleted: u64
  - pub partitions_cleaned: u64
  - pub last_cleanup_at: Option<DateTime<Utc>>
  - pub last_cleanup_duration_ms: u64
  - pub errors_count: u64
  - pub avg_cleanup_time_ms: f64
- `CleanupResult`
  - pub vectors_expired: u64
  - pub files_deleted: u64
  - pub partitions_processed: u64
  - pub errors: Vec<String>
  - pub duration_ms: u64
- `StagingOperationsCoordinator`
  - private filesystem: Arc<FilesystemFactory>
  - private config: StagingConfig
  - private active_operations: Arc<RwLock<HashMap<String
  - private optimization_cache: Arc<RwLock<HashMap<String
- `StagingConfig`
  - pub enable_optimizations: bool
  - pub staging_directory: String
  - pub cleanup_interval_secs: u64
  - pub max_staging_age_secs: u64
  - pub enable_atomic_operations: bool
- `StagingOperation`
  - pub operation_id: String
  - pub operation_type: StagingOperationType
  - pub staging_path: String
  - pub target_path: String
  - pub started_at: DateTime<Utc>
  - pub status: StagingStatus
- `ParquetOptimizationConfig`
  - pub rows_per_rowgroup: usize
  - pub num_rowgroups: usize
  - pub target_file_size_mb: usize
  - pub enable_dictionary_encoding: bool
  - pub enable_bloom_filters: bool
  - pub compression_level: i32
  - pub column_order: Vec<String>
- `OptimizedParquetRecords`
  - pub records: Vec<VectorRecord>
  - pub schema_version: u32
  - pub column_order: Vec<String>
  - pub rowgroup_size: usize
  - pub compression_config: ParquetOptimizationConfig
- `DataPartitioner`
  - private config: PartitioningConfig
  - private clustering_models: Arc<RwLock<HashMap<CollectionId
  - private partition_metadata: Arc<RwLock<HashMap<PartitionId
  - private stats: Arc<RwLock<PartitioningStats>>
- `PartitioningConfig`
  - pub enable_ml_clustering: bool
  - pub default_cluster_count: usize
  - pub min_vectors_per_partition: usize
  - pub max_vectors_per_partition: usize
  - pub clustering_algorithm: ClusteringAlgorithm
  - pub enable_adaptive_clustering: bool
  - pub reclustering_threshold: f32
- `ClusteringModel`
  - pub model_id: String
  - pub collection_id: CollectionId
  - pub algorithm: ClusteringAlgorithm
  - pub cluster_count: usize
  - pub centroids: Vec<Vec<f32>>
  - pub accuracy: f32
  - pub trained_at: DateTime<Utc>
- `PartitionMetadata`
  - pub partition_id: PartitionId
  - pub collection_id: CollectionId
  - pub cluster_id: ClusterId
  - pub file_path: String
  - pub vector_count: usize
  - pub size_bytes: usize
  - pub compression_ratio: f32
  - pub created_at: DateTime<Utc>
  - pub last_accessed: DateTime<Utc>
- `PartitioningStats`
  - pub total_partitions_created: u64
  - pub total_reclustering_operations: u64
  - pub avg_vectors_per_partition: f64
  - pub avg_compression_ratio: f32
  - pub avg_clustering_time_ms: f64
- `CompressionOptimizer`
  - private config: CompressionConfig
  - private compression_models: Arc<RwLock<HashMap<CollectionId
  - private stats: Arc<RwLock<CompressionStats>>
  - private algorithm_cache: Arc<RwLock<HashMap<String
- `CompressionConfig`
  - pub enable_adaptive_compression: bool
  - pub default_algorithm: CompressionAlgorithm
  - pub compression_level_range: (u8
  - pub enable_format_optimization: bool
  - pub enable_benchmarking: bool
  - pub optimization_interval_secs: u64
- `CompressionModel`
  - pub model_id: String
  - pub collection_id: CollectionId
  - pub optimal_algorithm: CompressionAlgorithm
  - pub optimal_level: u8
  - pub expected_ratio: f32
  - pub performance_profile: CompressionPerformance
  - pub trained_at: DateTime<Utc>
- `CompressionPerformance`
  - pub compression_ratio: f32
  - pub compression_speed_mb_sec: f32
  - pub decompression_speed_mb_sec: f32
  - pub cpu_usage_percent: f32
  - pub memory_usage_mb: f32
- `AlgorithmPerformance`
  - pub algorithm: CompressionAlgorithm
  - pub avg_compression_ratio: f32
  - pub avg_compression_speed: f32
  - pub avg_decompression_speed: f32
  - pub reliability_score: f32
  - pub usage_count: u64
- `CompressionStats`
  - pub total_compressions: u64
  - pub total_bytes_input: u64
  - pub total_bytes_output: u64
  - pub avg_compression_ratio: f32
  - pub avg_compression_time_ms: f64
  - pub algorithm_usage: HashMap<String
- `PerformanceReport`
  - pub global_stats: GlobalViperStats
  - pub collection_stats: Option<CollectionStats>
  - pub recent_operations: Vec<OperationMetrics>
  - pub recommendations: Vec<PerformanceRecommendation>
- `PerformanceRecommendation`
  - pub recommendation_type: String
  - pub description: String
  - pub expected_improvement: f32
  - pub confidence: f32
- `CompressionRecommendation`
  - pub recommended_algorithm: CompressionAlgorithm
  - pub recommended_level: u8
  - pub expected_ratio: f32
  - pub expected_performance: CompressionPerformance
- `PartitionedData`
  - pub partition_id: PartitionId
  - pub cluster_id: ClusterId
  - pub records: Vec<VectorRecord>
  - pub metadata: PartitionMetadata
- `StagingOperationResult`
  - pub operation_id: String
  - pub staging_path: String
  - pub optimized_records: OptimizedParquetRecords
  - pub estimated_size_mb: f32

**Enums:**
- `StagingOperationType`
  - Flush
  - Compaction
  - Migration
- `StagingStatus`
  - Preparing
  - Writing
  - Finalizing
  - Completed
  - Failed(String)
- `ClusteringAlgorithm`
  - KMeans
  - HDBSCAN
  - SpectralClustering
  - MiniBatchKMeans
  - AdaptiveKMeans
- `CompressionAlgorithm`
  - Snappy
  - Zstd{ level: u8

##### storage::vector::indexing::algorithms

**Structs:**
- `FlatIndex`
  - private vectors: Arc<RwLock<HashMap<VectorId
  - private distance_metric: DistanceMetric
  - private stats: Arc<RwLock<FlatIndexStats>>
- `FlatIndexStats`
  - pub total_vectors: usize
  - pub search_count: u64
  - pub avg_search_time_ms: f64
  - pub total_comparisons: u64
- `IvfIndex`
  - private centroids: Vec<Vec<f32>>
  - private inverted_lists: HashMap<usize
  - private vectors: Arc<RwLock<HashMap<VectorId
  - private config: IvfConfig
  - private distance_metric: DistanceMetric
  - private stats: Arc<RwLock<IvfIndexStats>>
- `IvfConfig`
  - pub n_lists: usize
  - pub n_probes: usize
  - pub train_size: usize
- `IvfIndexStats`
  - pub total_vectors: usize
  - pub n_clusters: usize
  - pub avg_cluster_size: f64
  - pub search_count: u64
  - pub avg_search_time_ms: f64
  - pub avg_clusters_searched: f64
- `LshIndex`
  - private hash_tables: Vec<LshHashTable>
  - private vectors: Arc<RwLock<HashMap<VectorId
  - private config: LshConfig
  - private projection_matrices: Vec<Vec<Vec<f32>>>
  - private stats: Arc<RwLock<LshIndexStats>>
- `LshHashTable`
  - private buckets: HashMap<u64
  - private hash_params: LshHashParams
- `LshHashParams`
  - pub projection_dim: usize
  - pub hash_width: f32
- `LshConfig`
  - pub n_tables: usize
  - pub n_bits: usize
  - pub hash_width: f32
  - pub projection_dim: usize
- `LshIndexStats`
  - pub total_vectors: usize
  - pub n_hash_tables: usize
  - pub avg_bucket_size: f64
  - pub search_count: u64
  - pub avg_search_time_ms: f64
  - pub hash_collisions: u64
- `FlatIndexBuilder`
  - private distance_metric: DistanceMetric
- `IvfIndexBuilder`
  - private config: IvfConfig
  - private distance_metric: DistanceMetric

##### storage::vector::indexing::hnsw

**Structs:**
- `HnswIndex`
  - private layers: Vec<HnswLayer>
  - private vectors: Arc<RwLock<HashMap<VectorId
  - private entry_point: Option<VectorId>
  - private config: HnswConfig
  - private stats: Arc<RwLock<HnswStats>>
  - private distance_metric: DistanceMetric
- `HnswLayer`
  - private connections: HashMap<VectorId
  - private level: usize
- `HnswConfig`
  - pub m: usize
  - pub max_m: usize
  - pub ef_construction: usize
  - pub ef_search: usize
  - pub ml: f64
  - pub enable_simd: bool
  - pub max_layers: usize
- `HnswStats`
  - pub total_nodes: usize
  - pub total_edges: usize
  - pub layer_counts: Vec<usize>
  - pub avg_degree: f64
  - pub search_count: u64
  - pub avg_search_time_ms: f64
  - pub build_time_ms: u64
- `SearchCandidate`
  - pub vector_id: VectorId
  - pub distance: f32
- `HnswIndexBuilder`
  - private config: HnswConfig

##### storage::vector::indexing::manager

**Structs:**
- `UnifiedIndexManager`
  - private collection_indexes: Arc<RwLock<HashMap<CollectionId
  - private index_builders: Arc<RwLock<IndexBuilderRegistry>>
  - private maintenance_scheduler: Arc<IndexMaintenanceScheduler>
  - private stats_collector: Arc<RwLock<IndexStatsCollector>>
  - private optimizer: Arc<IndexOptimizer>
  - private config: UnifiedIndexConfig
  - private operation_semaphore: Arc<Semaphore>
- `UnifiedIndexConfig`
  - pub max_concurrent_operations: usize
  - pub enable_auto_optimization: bool
  - pub maintenance_interval_secs: u64
  - pub index_cache_size_mb: usize
  - pub enable_multi_index: bool
  - pub default_index_type: IndexType
  - pub performance_monitoring: bool
- `MultiIndex`
  - private primary_index: Box<dyn VectorIndex>
  - private metadata_index: Option<Box<dyn MetadataIndex>>
  - private auxiliary_indexes: HashMap<String
  - private selection_strategy: IndexSelectionStrategy
  - private collection_info: CollectionIndexInfo
  - private last_maintenance: std::time::Instant
- `IndexBuilderRegistry`
  - private vector_builders: HashMap<IndexType
  - private metadata_builders: HashMap<String
  - private default_configs: HashMap<IndexType
  - private serde_json: :Value>
- `IndexMaintenanceScheduler`
  - private maintenance_queue: Arc<RwLock<Vec<MaintenanceTask>>>
  - private worker_handle: Option<tokio::task::JoinHandle<()>>
  - private config: MaintenanceConfig
- `MaintenanceTask`
  - pub collection_id: CollectionId
  - pub task_type: MaintenanceTaskType
  - pub priority: MaintenancePriority
  - pub scheduled_at: std::time::Instant
  - pub estimated_duration: Duration
- `MaintenanceConfig`
  - pub enabled: bool
  - pub interval_secs: u64
  - pub max_concurrent_tasks: usize
  - pub maintenance_window_hours: (u8
  - pub auto_rebuild_threshold: f64
- `IndexStatsCollector`
  - private collection_stats: HashMap<CollectionId
  - private global_stats: GlobalIndexStats
  - private performance_history: Vec<PerformanceSnapshot>
- `CollectionIndexStats`
  - pub collection_id: CollectionId
  - pub total_vectors: usize
  - pub index_count: usize
  - pub total_index_size_bytes: u64
  - pub avg_search_latency_ms: f64
  - pub search_accuracy: f64
  - pub last_rebuild: Option<std::time::Instant>
  - pub maintenance_frequency: f64
- `GlobalIndexStats`
  - pub total_collections: usize
  - pub total_indexes: usize
  - pub total_vectors: usize
  - pub cache_hit_ratio: f64
  - pub avg_build_time_ms: f64
  - pub memory_usage_mb: f64
- `PerformanceSnapshot`
  - pub timestamp: std::time::Instant
  - pub avg_search_latency_ms: f64
  - pub cache_hit_ratio: f64
  - pub memory_usage_mb: f64
  - pub operations_per_second: f64
- `IndexOptimizer`
  - private strategies: HashMap<String
  - private analyzer: Arc<PerformanceAnalyzer>
  - private history: Arc<RwLock<Vec<OptimizationResult>>>
- `CollectionIndexInfo`
  - pub collection_id: CollectionId
  - pub vector_dimension: usize
  - pub total_vectors: usize
  - pub data_distribution: DataDistribution
  - pub query_patterns: QueryPatternAnalysis
  - pub performance_requirements: PerformanceRequirements
- `QueryPatternAnalysis`
  - pub typical_k_values: Vec<usize>
  - pub filter_usage_frequency: f64
  - pub query_vector_distribution: DataDistribution
  - pub temporal_patterns: Vec<f64>
  - pub accuracy_requirements: f64
- `PerformanceRequirements`
  - pub max_search_latency_ms: f64
  - pub min_accuracy: f64
  - pub memory_budget_mb: Option<usize>
  - pub throughput_requirement: Option<f64>
- `BuildCostEstimate`
  - pub estimated_time_ms: u64
  - pub memory_requirement_mb: usize
  - pub cpu_intensity: f64
  - pub io_requirement: f64
  - pub disk_space_mb: usize
- `OptimizationRecommendation`
  - pub recommendation_type: OptimizationType
  - pub description: String
  - pub expected_improvement: f64
  - pub implementation_cost: f64
  - pub risk_level: RiskLevel
  - pub parameters: serde_json::Value
- `OptimizationResult`
  - pub collection_id: CollectionId
  - pub optimization: OptimizationRecommendation
  - pub applied_at: std::time::Instant
  - pub actual_improvement: Option<f64>
  - pub success: bool
  - pub notes: String
- `PerformanceAnalyzer`
  - private performance_data: Arc<RwLock<HashMap<CollectionId
  - private analyzers: Vec<Box<dyn PerformanceAnalysisAlgorithm>>
- `PerformanceDataPoint`
  - pub timestamp: std::time::Instant
  - pub search_latency_ms: f64
  - pub accuracy: f64
  - pub memory_usage_mb: f64
  - pub query_type: String
  - pub result_count: usize
- `PerformanceIssue`
  - pub issue_type: PerformanceIssueType
  - pub severity: IssueSeverity
  - pub description: String
  - pub suggested_actions: Vec<String>

**Enums:**
- `MaintenanceTaskType`
  - Rebuild
  - Optimize
  - Compact
  - Rebalance
  - Cleanup
  - StatisticsUpdate
- `MaintenancePriority`
  - Low
  - Normal
  - High
  - Critical
- `IndexSelectionStrategy`
  - Primary
  - QueryAdaptive
  - RoundRobin
  - LoadBalanced
  - Custom(String)
- `OptimizationType`
  - IndexTypeChange{ from: IndexType, to: IndexType
- `RiskLevel`
  - Low
  - Medium
  - High
- `PerformanceIssueType`
  - LatencyDegradation
  - AccuracyDrop
  - MemoryLeakage
  - CacheMisses
  - IndexFragmentation
  - SuboptimalParameters
- `IssueSeverity`
  - Info
  - Warning
  - Error
  - Critical
- `QueryType`
  - VectorSimilarity
  - MetadataFilter
  - Hybrid

**Traits:**
- `IndexBuilder`: Send + Sync
  - fn build_index(&self,
        vectors: Vec<(VectorId, Vec<f32>)
  - fn builder_name(&self) -> &'static str
  - fn index_type(&self) -> IndexType
  - fn estimate_build_cost(&self,
        vector_count: usize,
        dimension: usize,
        config: &serde_json::Value,) -> Result<BuildCostEstimate>
- `MetadataIndexBuilder`: Send + Sync
  - fn build_metadata_index(&self,
        metadata: Vec<(VectorId, serde_json::Value)
  - fn builder_name(&self) -> &'static str
- `MetadataIndex`: Send + Sync
  - fn search(&self,
        filter: &MetadataFilter,
        limit: Option<usize>,) -> Result<Vec<VectorId>>
  - fn add_entry(&mut self, vector_id: VectorId, metadata: serde_json::Value) -> Result<()>
  - fn remove_entry(&mut self, vector_id: &VectorId) -> Result<()>
  - fn statistics(&self) -> Result<IndexStatistics>
- `OptimizationStrategy`: Send + Sync
  - fn optimize(&self,
        collection_info: &CollectionIndexInfo,
        current_performance: &CollectionIndexStats,) -> Result<Vec<OptimizationRecommendation>>
  - fn strategy_name(&self) -> &'static str
- `PerformanceAnalysisAlgorithm`: Send + Sync
  - fn analyze(&self,
        data: &[PerformanceDataPoint],) -> Result<Vec<PerformanceIssue>>
  - fn analyzer_name(&self) -> &'static str

##### storage::vector::indexing::mod

**Structs:**
- `IndexManagerFactory`

##### storage::vector::operations::insert

**Structs:**
- `VectorInsertHandler`
  - private validators: Vec<Box<dyn InsertValidator>>
  - private optimizers: Vec<Box<dyn InsertOptimizer>>
  - private performance_tracker: InsertPerformanceTracker
  - private config: InsertConfig
- `InsertConfig`
  - pub enable_validation: bool
  - pub enable_optimization: bool
  - pub batch_threshold: usize
  - pub max_dimension: usize
  - pub enable_duplicate_detection: bool
  - pub enable_performance_tracking: bool
- `ValidationResult`
  - pub valid: bool
  - pub warnings: Vec<String>
  - pub errors: Vec<String>
  - pub suggestions: Vec<String>
- `InsertOperation`
  - pub records: Vec<VectorRecord>
  - pub collection_id: CollectionId
  - pub options: InsertOptions
- `InsertOptions`
  - pub batch_mode: bool
  - pub validate_before_insert: bool
  - pub update_indexes: bool
  - pub generate_id_if_missing: bool
  - pub skip_duplicates: bool
- `OptimizedInsertOperation`
  - pub original: InsertOperation
  - pub optimizations_applied: Vec<String>
  - pub batches: Vec<InsertBatch>
  - pub estimated_time_ms: u64
- `InsertBatch`
  - pub batch_id: String
  - pub records: Vec<VectorRecord>
  - pub priority: BatchPriority
  - pub processing_hint: ProcessingHint
- `InsertPerformanceTracker`
  - private history: Vec<InsertPerformanceRecord>
  - private stats: InsertStats
- `InsertPerformanceRecord`
  - pub timestamp: chrono::DateTime<chrono::Utc>
  - pub collection_id: CollectionId
  - pub record_count: usize
  - pub duration_ms: u64
  - pub throughput_ops_sec: f64
  - pub validation_time_ms: u64
  - pub optimization_time_ms: u64
  - pub actual_insert_time_ms: u64
  - pub success: bool
  - pub error_type: Option<String>
- `InsertStats`
  - pub total_operations: u64
  - pub total_records: u64
  - pub avg_throughput_ops_sec: f64
  - pub avg_latency_ms: f64
  - pub success_rate: f64
  - pub validation_overhead_percent: f64
  - pub optimization_benefit_percent: f64
- `InsertResult`
  - pub success: bool
  - pub inserted_count: usize
  - pub failed_count: usize
  - pub validation_results: Vec<ValidationResult>
  - pub performance_info: Option<InsertPerformanceInfo>
  - pub error: Option<String>
- `InsertPerformanceInfo`
  - pub total_time_ms: u64
  - pub validation_time_ms: u64
  - pub optimization_time_ms: u64
  - pub insert_time_ms: u64
  - pub throughput_ops_sec: f64
  - pub optimizations_applied: Vec<String>
- `DimensionValidator`
  - private max_dimension: usize
- `DuplicateValidator`
  - private seen_ids: HashMap<VectorId
- `VectorQualityValidator`
- `BatchOptimizer`
  - private batch_threshold: usize
- `VectorizedOptimizer`

**Enums:**
- `BatchPriority`
  - Low
  - Normal
  - High
  - Critical
- `ProcessingHint`
  - Sequential
  - Parallel
  - Vectorized
  - MemoryOptimized

**Traits:**
- `InsertValidator`: Send + Sync
  - fn validate(&self, record: &VectorRecord) -> Result<ValidationResult>
  - fn name(&self) -> &'static str
- `InsertOptimizer`: Send + Sync
  - fn optimize(&self, operation: InsertOperation) -> Result<OptimizedInsertOperation>
  - fn name(&self) -> &'static str

##### storage::vector::operations::search

**Structs:**
- `VectorSearchHandler`
  - private planners: Vec<Box<dyn QueryPlanner>>
  - private processors: Vec<Box<dyn ResultProcessor>>
  - private performance_tracker: SearchPerformanceTracker
  - private config: SearchConfig
- `SearchConfig`
  - pub enable_query_planning: bool
  - pub enable_result_processing: bool
  - pub max_search_time_ms: u64
  - pub default_limit: usize
  - pub enable_performance_tracking: bool
  - pub enable_caching: bool
  - pub cache_ttl_secs: u64
- `QueryPlan`
  - pub plan_id: String
  - pub steps: Vec<ExecutionStep>
  - pub estimated_cost: QueryCost
  - pub optimizations: Vec<String>
  - pub expected_accuracy: f64
- `ExecutionStep`
  - pub step_id: String
  - pub step_type: StepType
  - pub parameters: HashMap<String
  - private serde_json: :Value>
  - pub dependencies: Vec<String>
  - pub estimated_time_ms: u64
- `QueryCost`
  - pub cpu_cost: f64
  - pub io_cost: f64
  - pub memory_cost: f64
  - pub network_cost: f64
  - pub total_time_ms: u64
  - pub efficiency_score: f64
- `SearchPerformanceTracker`
  - private history: Vec<SearchPerformanceRecord>
  - private stats: SearchStats
  - private pattern_analyzer: QueryPatternAnalyzer
- `SearchPerformanceRecord`
  - pub timestamp: chrono::DateTime<chrono::Utc>
  - pub collection_id: CollectionId
  - pub query_type: QueryType
  - pub k_value: usize
  - pub duration_ms: u64
  - pub planning_time_ms: u64
  - pub execution_time_ms: u64
  - pub processing_time_ms: u64
  - pub result_count: usize
  - pub accuracy_score: f64
  - pub success: bool
  - pub error_type: Option<String>
- `SearchStats`
  - pub total_searches: u64
  - pub avg_latency_ms: f64
  - pub avg_accuracy: f64
  - pub success_rate: f64
  - pub planning_overhead_percent: f64
  - pub processing_benefit_percent: f64
  - pub cache_hit_ratio: f64
- `QueryPatternAnalyzer`
  - private patterns: HashMap<String
  - private temporal_patterns: Vec<TemporalPattern>
- `PatternFrequency`
  - pub pattern_type: String
  - pub frequency: u64
  - pub avg_performance: f64
  - pub last_seen: chrono::DateTime<chrono::Utc>
- `TemporalPattern`
  - pub pattern_name: String
  - pub time_range: TimeRange
  - pub query_characteristics: QueryCharacteristics
  - pub performance_profile: PerformanceProfile
- `TimeRange`
  - pub start_hour: u8
  - pub end_hour: u8
  - pub days_of_week: Vec<u8>
- `QueryCharacteristics`
  - pub avg_k_value: f64
  - pub filter_usage_rate: f64
  - pub vector_dimension: usize
  - pub query_complexity: f64
- `PerformanceProfile`
  - pub avg_latency_ms: f64
  - pub throughput_qps: f64
  - pub resource_usage: ResourceUsage
- `ResourceUsage`
  - pub cpu_percent: f64
  - pub memory_mb: f64
  - pub io_ops_per_sec: f64
- `SearchResult`
  - pub results: Vec<VectorSearchResult>
  - pub total_count: usize
  - pub query_plan: Option<QueryPlan>
  - pub performance_info: Option<SearchPerformanceInfo>
  - pub debug_info: Option<SearchDebugInfo>
- `SearchPerformanceInfo`
  - pub total_time_ms: u64
  - pub planning_time_ms: u64
  - pub execution_time_ms: u64
  - pub processing_time_ms: u64
  - pub accuracy_score: f64
- `CostBasedPlanner`
- `HeuristicPlanner`
- `ScoreNormalizer`
- `ResultDeduplicator`
- `QualityEnhancer`

**Enums:**
- `StepType`
  - IndexScan
  - VectorComparison
  - MetadataFilter
  - ResultMerging
  - ScoreCalculation
  - Reranking
  - Deduplication
- `QueryType`
  - SimpleSimilarity
  - FilteredSimilarity
  - HybridSearch
  - RangeQuery
  - BatchSearch
  - AggregateQuery

**Traits:**
- `QueryPlanner`: Send + Sync
  - fn plan_query(&self, context: &SearchContext) -> Result<QueryPlan>
  - fn name(&self) -> &'static str
- `ResultProcessor`: Send + Sync
  - fn process_results(&self,
        results: Vec<VectorSearchResult>,
        context: &SearchContext,) -> Result<Vec<VectorSearchResult>>
  - fn name(&self) -> &'static str

##### storage::vector::search::engine

**Structs:**
- `UnifiedSearchEngine`
  - private algorithms: Arc<RwLock<HashMap<String
  - private cost_model: Arc<SearchCostModel>
  - private result_processor: Arc<ResultProcessor>
  - private tier_managers: Arc<RwLock<HashMap<StorageTier
  - private stats_collector: Arc<RwLock<SearchStatsCollector>>
  - private feature_cache: Arc<RwLock<FeatureSelectionCache>>
  - private search_semaphore: Arc<Semaphore>
  - private config: UnifiedSearchConfig
- `UnifiedSearchConfig`
  - pub max_concurrent_searches: usize
  - pub default_timeout_ms: u64
  - pub enable_cost_optimization: bool
  - pub enable_progressive_search: bool
  - pub enable_feature_selection: bool
  - pub cache_size: usize
  - pub tier_search_parallelism: usize
- `SearchCostModel`
  - private algorithm_performance: RwLock<HashMap<String
  - private collection_factors: RwLock<HashMap<CollectionId
  - private cost_weights: CostWeights
- `AlgorithmPerformanceData`
  - pub avg_execution_time_ms: f64
  - pub avg_accuracy: f64
  - pub total_executions: u64
  - pub success_rate: f64
  - pub memory_usage_mb: f64
  - pub last_updated: chrono::DateTime<chrono::Utc>
- `CollectionCostFactors`
  - pub vector_count: usize
  - pub avg_vector_dimension: usize
  - pub data_distribution: DataDistribution
  - pub index_types: Vec<IndexType>
  - pub access_patterns: AccessPatterns
- `AccessPatterns`
  - pub query_frequency: f64
  - pub typical_k_values: Vec<usize>
  - pub filter_usage_rate: f64
  - pub temporal_patterns: Vec<f64>
- `CostWeights`
  - pub cpu_weight: f64
  - pub io_weight: f64
  - pub memory_weight: f64
  - pub accuracy_weight: f64
  - pub latency_weight: f64
- `ResultProcessor`
  - private merger: Arc<ResultMerger>
  - private post_processors: Vec<Box<dyn ResultPostProcessor>>
  - private score_normalizers: HashMap<String
- `TierConstraints`
  - pub max_results: usize
  - pub timeout_ms: u64
  - pub quality_threshold: f64
  - pub partition_hints: Option<Vec<String>>
  - pub cluster_hints: Option<Vec<u32>>
- `TierCharacteristics`
  - pub tier: StorageTier
  - pub avg_latency_ms: f64
  - pub throughput_vectors_per_sec: f64
  - pub memory_resident: bool
  - pub supports_random_access: bool
  - pub compression_overhead: f64
  - pub index_types_supported: Vec<IndexType>
- `SearchStatsCollector`
  - pub total_searches: u64
  - pub algorithm_usage: HashMap<String
  - pub tier_usage: HashMap<StorageTier
  - pub avg_latency_ms: f64
  - pub accuracy_scores: Vec<f64>
  - pub cost_efficiency_scores: Vec<f64>
- `FeatureSelectionCache`
  - private feature_sets: HashMap<CollectionId
  - private max_size: usize
  - private access_order: Vec<CollectionId>
- `CachedFeatureSet`
  - pub features: Vec<usize>
  - pub importance_scores: Vec<f32>
  - pub confidence: f64
  - pub last_updated: chrono::DateTime<chrono::Utc>
  - pub hit_count: u64
- `ResultMerger`
  - private strategies: HashMap<String
  - private default_strategy: String
- `ScoreBasedMerger`

**Enums:**
- `DataDistribution`
  - Uniform
  - Normal{ mean: f64, std_dev: f64

**Traits:**
- `TierSearcher`: Send + Sync
  - fn search_tier(&self,
        context: &SearchContext,
        tier_constraints: &TierConstraints,) -> Result<Vec<SearchResult>>
  - fn tier_characteristics(&self) -> TierCharacteristics
  - fn health_check(&self) -> Result<bool>
- `MergeStrategy`: Send + Sync
  - fn merge_results(&self,
        results: Vec<Vec<SearchResult>>,
        context: &SearchContext,) -> Result<Vec<SearchResult>>
- `ResultPostProcessor`: Send + Sync
  - fn process_results(&self,
        results: Vec<SearchResult>,
        context: &SearchContext,) -> Result<Vec<SearchResult>>
- `ScoreNormalizer`: Send + Sync
  - fn normalize_score(&self, score: f32, context: &SearchContext) -> f32

##### storage::vector::search::mod

**Structs:**
- `SearchEngineFactory`

##### storage::vector::storage_policy

**Structs:**
- `StoragePolicy`
  - pub collection_name: String
  - pub primary_storage: String
  - pub secondary_storage: Option<String>
  - pub archive_storage: Option<String>
  - pub wal_storage: String
  - pub metadata_storage: String
  - pub index_storage: String
  - pub custom_storage: HashMap<String
  - pub lifecycle: StorageLifecycle
- `StorageLifecycle`
  - pub move_to_secondary_after_seconds: Option<u64>
  - pub move_to_archive_after_seconds: Option<u64>
  - pub delete_after_seconds: Option<u64>
  - pub cache_recent_days: u32
  - pub compression: CompressionPolicy
- `CompressionPolicy`
  - pub compress_primary: bool
  - pub compress_secondary: bool
  - pub compress_archive: bool
  - pub algorithm: String
  - pub level: u8
- `StorageLocation`
  - pub url: String
  - pub expected_latency_us: u64
  - pub expected_throughput_mbps: f64
  - pub cost_per_gb_month: Option<f64>
  - pub supports_mmap: bool
  - pub supports_streaming: bool
- `StoragePolicyManager`
  - private policies: HashMap<String
  - private default_policy: StoragePolicy

##### storage::vector::types

**Structs:**
- `SearchContext`
  - pub collection_id: CollectionId
  - pub query_vector: Vec<f32>
  - pub k: usize
  - pub filters: Option<MetadataFilter>
  - pub strategy: SearchStrategy
  - pub algorithm_hints: HashMap<String
  - pub threshold: Option<f32>
  - pub timeout_ms: Option<u64>
  - pub include_debug_info: bool
  - pub include_vectors: bool
- `SearchResult`
  - pub vector_id: VectorId
  - pub score: f32
  - pub vector: Option<Vec<f32>>
  - pub metadata: Value
  - pub debug_info: Option<SearchDebugInfo>
  - pub storage_info: Option<StorageLocationInfo>
  - pub rank: Option<usize>
  - pub distance: Option<f32>
- `SearchDebugInfo`
  - pub timing: SearchTiming
  - pub algorithm_used: String
  - pub index_usage: Vec<IndexUsageInfo>
  - pub search_path: Vec<SearchStep>
  - pub metrics: SearchMetrics
  - pub cost_breakdown: CostBreakdown
- `SearchTiming`
  - pub total_ms: f64
  - pub planning_ms: f64
  - pub execution_ms: f64
  - pub post_processing_ms: f64
  - pub io_wait_ms: f64
  - pub cpu_time_ms: f64
- `IndexUsageInfo`
  - pub index_name: String
  - pub index_type: IndexType
  - pub selectivity: f64
  - pub entries_scanned: usize
  - pub cache_hit_ratio: f64
- `SearchStep`
  - pub step_name: String
  - pub step_type: SearchStepType
  - pub duration_ms: f64
  - pub items_processed: usize
  - pub items_filtered: usize
- `SearchMetrics`
  - pub vectors_compared: usize
  - pub distance_calculations: usize
  - pub index_traversals: usize
  - pub cache_hits: usize
  - pub cache_misses: usize
  - pub disk_reads: usize
  - pub network_requests: usize
  - pub memory_peak_mb: f64
- `CostBreakdown`
  - pub cpu_cost: f64
  - pub io_cost: f64
  - pub memory_cost: f64
  - pub network_cost: f64
  - pub total_cost: f64
  - pub cost_efficiency: f64
- `StorageLocationInfo`
  - pub storage_url: String
  - pub tier: StorageTier
  - pub partition_id: Option<String>
  - pub cluster_id: Option<u32>
  - pub file_path: Option<String>
  - pub row_group: Option<usize>
- `StorageCapabilities`
  - pub supports_transactions: bool
  - pub supports_streaming: bool
  - pub supports_compression: bool
  - pub supports_encryption: bool
  - pub max_vector_dimension: usize
  - pub max_batch_size: usize
  - pub supported_distances: Vec<DistanceMetric>
  - pub supported_indexes: Vec<IndexType>
- `StorageStatistics`
  - pub total_vectors: usize
  - pub total_collections: usize
  - pub storage_size_bytes: u64
  - pub index_size_bytes: u64
  - pub cache_hit_ratio: f64
  - pub avg_search_latency_ms: f64
  - pub operations_per_second: f64
  - pub last_compaction: Option<DateTime<Utc>>
- `IndexStatistics`
  - pub vector_count: usize
  - pub index_size_bytes: u64
  - pub build_time_ms: u64
  - pub last_optimized: DateTime<Utc>
  - pub search_accuracy: f64
  - pub avg_search_time_ms: f64
- `HealthStatus`
  - pub healthy: bool
  - pub status: String
  - pub last_check: DateTime<Utc>
  - pub response_time_ms: f64
  - pub error_count: usize
  - pub warnings: Vec<String>
- `IndexSpec`
  - pub name: String
  - pub index_type: IndexType
  - pub columns: Vec<String>
  - pub unique: bool
  - pub parameters: serde_json::Value

**Enums:**
- `SearchStrategy`
  - Exact
  - Approximate{
  - target_recall: f32
  - max_candidates: Option<usize>
- `MetadataFilter`
  - Field{
  - field: String
  - condition: FieldCondition
- `FieldCondition`
  - Equals(Value)
  - NotEquals(Value)
  - GreaterThan(Value)
  - GreaterThanOrEqual(Value)
  - LessThan(Value)
  - LessThanOrEqual(Value)
  - In(Vec<Value>)
  - NotIn(Vec<Value>)
  - Contains(String)
  - StartsWith(String)
  - EndsWith(String)
  - Regex(String)
  - IsNull
  - IsNotNull
  - Range{ min: Value, max: Value, inclusive: bool
- `SearchStepType`
  - Planning
  - IndexLookup
  - VectorComparison
  - MetadataFilter
  - ResultMerging
  - PostProcessing
- `StorageTier`
  - UltraHot,  // MMAP + OS cache
  - Hot,       // Local SSD
  - Warm,      // Local HDD/Network storage
  - Cold,      // Cloud storage (S3/Azure/GCS)
- `IndexType`
  - HNSW{
  - m: usize,              // max connections per layer
  - ef_construction: usize, // size of dynamic candidate list
  - ef_search: usize,      // size of search candidate list
- `VectorOperation`
  - Insert{
  - record: VectorRecord
  - index_immediately: bool
- `OperationResult`
  - Inserted{ vector_id: VectorId
- `DistanceMetric`
  - Euclidean
  - Cosine
  - DotProduct
  - Manhattan
  - Hamming
  - Jaccard
  - Custom(String)

**Traits:**
- `VectorStorage`: Send + Sync
  - fn engine_name(&self) -> &'static str
  - fn capabilities(&self) -> StorageCapabilities
  - fn execute_operation(&self,
        operation: VectorOperation,) -> anyhow::Result<OperationResult>
  - fn get_statistics(&self) -> anyhow::Result<StorageStatistics>
  - fn health_check(&self) -> anyhow::Result<HealthStatus>
- `SearchAlgorithm`: Send + Sync
  - fn algorithm_name(&self) -> &'static str
  - fn algorithm_type(&self) -> IndexType
  - fn search(&self,
        context: &SearchContext,) -> anyhow::Result<Vec<SearchResult>>
  - fn estimate_cost(&self,
        context: &SearchContext,) -> anyhow::Result<CostBreakdown>
  - fn configuration(&self) -> HashMap<String, Value>
- `VectorIndex`: Send + Sync
  - fn index_name(&self) -> &str
  - fn index_type(&self) -> IndexType
  - fn add_vector(&mut self,
        vector_id: VectorId,
        vector: &[f32],
        metadata: &Value,) -> anyhow::Result<()>
  - fn remove_vector(&mut self, vector_id: VectorId) -> anyhow::Result<()>
  - fn search(&self,
        query: &[f32],
        k: usize,
        filters: Option<&MetadataFilter>,) -> anyhow::Result<Vec<SearchResult>>
  - fn statistics(&self) -> anyhow::Result<IndexStatistics>
  - fn optimize(&mut self) -> anyhow::Result<()>

##### storage::viper::adapter

**Structs:**
- `VectorRecordSchemaAdapter`<'a>
  - private strategy: &'a dyn SchemaGenerationStrategy
- `TimeSeriesAdapter`<'a>
  - private base_adapter: VectorRecordSchemaAdapter<'a>
  - private time_precision: TimePrecision
- `CompressedVectorAdapter`<'a>
  - private base_adapter: VectorRecordSchemaAdapter<'a>
  - private compression_type: VectorCompressionType

**Enums:**
- `TimePrecision`
  - Milliseconds
  - Seconds
  - Minutes
  - Hours
- `VectorCompressionType`
  - None
  - Quantized8Bit
  - Quantized16Bit
  - DeltaEncoding

##### storage::viper::atomic_operations

**Structs:**
- `CollectionLockManager`
  - private collection_locks: Arc<RwLock<HashMap<CollectionId
  - private filesystem: Arc<FilesystemFactory>
- `CollectionLock`
  - private reader_count: usize
  - private has_writer: bool
  - private pending_operations: Vec<OperationType>
  - private acquired_at: DateTime<Utc>
  - private last_flush_timestamp: Option<DateTime<Utc>>
  - private last_compaction_timestamp: Option<DateTime<Utc>>
  - private optimization: readers can proceed during staging phases
  - private allow_reads_during_staging: bool
- `AtomicFlusher`
  - private lock_manager: Arc<CollectionLockManager>
  - private staging_coordinator: Arc<StagingOperationsCoordinator>
  - private active_flushes: Arc<Mutex<HashMap<CollectionId
- `FlushOperation`
  - pub collection_id: CollectionId
  - pub staging_url: String
  - pub target_files: Vec<String>
  - pub wal_entries_to_clear: Vec<String>
  - pub started_at: DateTime<Utc>
- `AtomicCompactor`
  - private lock_manager: Arc<CollectionLockManager>
  - private staging_coordinator: Arc<StagingOperationsCoordinator>
  - private active_compactions: Arc<Mutex<HashMap<CollectionId
- `CompactionOperation`
  - pub collection_id: CollectionId
  - pub staging_url: String
  - pub source_files: Vec<String>
  - pub target_file: String
  - pub started_at: DateTime<Utc>
- `ReadLockGuard`
  - private collection_id: CollectionId
  - private lock_manager: std::sync::Weak<RwLock<HashMap<CollectionId
- `WriteLockGuard`
  - private collection_id: CollectionId
  - private operation: OperationType
  - private lock_manager: std::sync::Weak<RwLock<HashMap<CollectionId
- `AtomicOperationsFactory`
  - private lock_manager: Arc<CollectionLockManager>
  - private filesystem: Arc<FilesystemFactory>

**Enums:**
- `OperationType`
  - Read
  - Flush
  - Compaction

##### storage::viper::compaction

**Structs:**
- `ViperCompactionEngine`
  - private config: ViperConfig
  - private task_queue: Arc<Mutex<VecDeque<CompactionTask>>>
  - private active_compactions: Arc<RwLock<HashMap<CollectionId
  - private stats: Arc<RwLock<CompactionStats>>
  - private optimization_model: Arc<RwLock<Option<CompactionOptimizationModel>>>
  - private worker_handles: Vec<tokio::task::JoinHandle<()>>
  - private shutdown_sender: Arc<Mutex<Option<mpsc::Sender<()>>>>
- `CompactionTask`
  - pub task_id: String
  - pub collection_id: CollectionId
  - pub compaction_type: CompactionType
  - pub priority: CompactionPriority
  - pub input_partitions: Vec<PartitionId>
  - pub expected_outputs: usize
  - pub optimization_hints: Option<CompactionOptimizationHints>
  - pub created_at: DateTime<Utc>
  - pub estimated_duration: Duration
- `MigrationCriteria`
  - pub age_threshold_days: u32
  - pub access_frequency_threshold: f32
  - pub storage_cost_factor: f32
  - pub performance_requirements: PerformanceRequirements
- `PerformanceRequirements`
  - pub max_latency_ms: u64
  - pub min_throughput_mbps: f32
  - pub availability_sla: f32
- `CompactionOptimizationModel`
  - pub model_id: String
  - pub version: String
  - pub feature_importance: Vec<f32>
  - pub cluster_quality_model: ClusterQualityModel
  - pub compression_model: CompressionModel
  - pub access_pattern_model: AccessPatternModel
  - pub training_metadata: ModelTrainingMetadata
- `ClusterQualityModel`
  - pub quality_weights: Vec<f32>
  - pub expected_quality_improvement: f32
  - pub reclustering_threshold: f32
- `CompressionModel`
  - pub algorithm_performance: HashMap<String
  - pub feature_compression_impact: Vec<f32>
  - pub sparsity_compression_curve: Vec<(f32
- `CompressionPerformance`
  - pub average_ratio: f32
  - pub compression_speed_mbps: f32
  - pub decompression_speed_mbps: f32
  - pub cpu_usage_factor: f32
- `AccessPatternModel`
  - pub temporal_patterns: Vec<TemporalPattern>
  - pub feature_access_correlation: Vec<f32>
  - pub cluster_access_prediction: HashMap<ClusterId
- `TemporalPattern`
  - pub pattern_type: TemporalPatternType
  - pub strength: f32
  - pub time_scale: Duration
- `ModelTrainingMetadata`
  - pub training_data_size: usize
  - pub training_duration: Duration
  - pub model_accuracy: f32
  - pub last_trained: DateTime<Utc>
  - pub next_training_due: DateTime<Utc>
- `CompactionOptimizationHints`
  - pub partition_suggestions: Vec<PartitionSuggestion>
  - pub compression_recommendations: Vec<CompressionRecommendation>
  - pub cluster_improvements: Vec<ClusterImprovement>
  - pub performance_impact: PerformanceImpact
- `PartitionSuggestion`
  - pub current_partition: PartitionId
  - pub suggested_partition: PartitionId
  - pub reason: String
  - pub expected_benefit: PartitionBenefit
- `PartitionBenefit`
  - pub io_improvement: f32
  - pub compression_improvement: f32
  - pub search_performance_improvement: f32
- `CompressionRecommendation`
  - pub algorithm: CompressionAlgorithm
  - pub expected_ratio: f32
  - pub performance_impact: f32
  - pub compatibility_score: f32
- `ClusterImprovement`
  - pub cluster_id: ClusterId
  - pub current_quality: f32
  - pub expected_quality: f32
  - pub improvement_strategy: ClusterImprovementStrategy
- `PerformanceImpact`
  - pub compaction_duration_estimate: Duration
  - pub io_overhead_during_compaction: f32
  - pub expected_read_performance_improvement: f32
  - pub expected_write_performance_improvement: f32
  - pub storage_savings_estimate: f32
- `CompactionOperation`
  - pub task: CompactionTask
  - pub start_time: Instant
  - pub progress: CompactionProgress
  - pub current_phase: CompactionPhase
  - pub worker_handle: tokio::task::JoinHandle<Result<CompactionResult>>
- `CompactionProgress`
  - pub phase: CompactionPhase
  - pub progress_percent: f32
  - pub estimated_remaining: Duration
  - pub bytes_processed: u64
  - pub bytes_total: u64
  - pub files_processed: usize
  - pub files_total: usize
- `CompactionResult`
  - pub task_id: String
  - pub success: bool
  - pub duration: Duration
  - pub input_stats: CompactionInputStats
  - pub output_stats: CompactionOutputStats
  - pub performance_metrics: CompactionPerformanceMetrics
  - pub error: Option<String>
- `CompactionInputStats`
  - pub file_count: usize
  - pub total_size_bytes: u64
  - pub vector_count: usize
  - pub average_compression_ratio: f32
  - pub cluster_count: usize
- `CompactionOutputStats`
  - pub file_count: usize
  - pub total_size_bytes: u64
  - pub vector_count: usize
  - pub compression_ratio: f32
  - pub cluster_count: usize
  - pub size_reduction_percent: f32
- `CompactionPerformanceMetrics`
  - pub read_throughput_mbps: f32
  - pub write_throughput_mbps: f32
  - pub cpu_utilization_percent: f32
  - pub memory_usage_mb: usize
  - pub ml_prediction_accuracy: f32
- `CompactionStats`
  - pub total_compactions: u64
  - pub successful_compactions: u64
  - pub failed_compactions: u64
  - pub total_bytes_processed: u64
  - pub total_bytes_saved: u64
  - pub average_compression_improvement: f32
  - pub average_duration: Duration
  - pub ml_model_accuracy: f32
- `CompactionAnalysis`
  - private needs_compaction: bool
  - private recommended_task: CompactionTask
- `CompactionTrainingData`

**Enums:**
- `CompactionType`
  - FileMerging{
  - target_file_size_mb: usize
  - max_files_per_merge: usize
- `PartitionStrategy`
  - FeatureBased{
  - primary_features: Vec<usize>
  - partition_count: usize
- `CompactionPriority`
  - Low
  - Normal
  - High
  - Critical
- `TemporalPatternType`
  - Periodic{ period: Duration
- `ClusterImprovementStrategy`
  - Split{ suggested_split_count: usize
- `CompactionPhase`
  - Planning
  - DataAnalysis
  - FileReading
  - MLOptimization
  - DataReorganization
  - CompressionOptimization
  - FileWriting
  - IndexUpdating
  - Verification
  - Cleanup
  - Complete

##### storage::viper::compression

**Structs:**
- `SimdCompressionEngine`
  - private backends: Arc<RwLock<HashMap<CompressionAlgorithm
  - private simd_capabilities: SimdCapabilities
  - private stats: Arc<RwLock<CompressionStats>>
  - private selection_model: Arc<RwLock<Option<CompressionSelectionModel>>>
- `CompressionCharacteristics`
  - pub algorithm: CompressionAlgorithm
  - pub compression_speed_mbps: f32
  - pub decompression_speed_mbps: f32
  - pub typical_compression_ratio: f32
  - pub memory_usage_mb: usize
  - pub supports_simd: bool
  - pub best_for_data_types: Vec<DataType>
- `SimdCapabilities`
  - pub has_sse4_2: bool
  - pub has_avx2: bool
  - pub has_avx512: bool
  - pub has_neon: bool
  - pub detected_instruction_set: SimdLevel
- `CompressionStats`
  - pub algorithm_performance: HashMap<CompressionAlgorithm
  - pub data_type_ratios: HashMap<DataType
  - pub selection_accuracy: f32
  - pub total_compressions: u64
  - pub total_decompressions: u64
- `AlgorithmPerformance`
  - pub compression_count: u64
  - pub decompression_count: u64
  - pub avg_compression_ratio: f32
  - pub avg_compression_speed_mbps: f32
  - pub avg_decompression_speed_mbps: f32
  - pub avg_memory_usage_mb: f32
  - pub error_count: u64
- `CompressionSelectionModel`
  - pub feature_weights: Vec<f32>
  - pub data_type_preferences: HashMap<DataType
  - pub performance_predictors: PerformancePredictors
  - pub training_metadata: ModelTrainingMetadata
- `PerformancePredictors`
  - pub ratio_predictor: RatioPredictor
  - pub speed_predictor: SpeedPredictor
  - pub memory_predictor: MemoryPredictor
- `RatioPredictor`
  - pub linear_weights: Vec<f32>
  - pub entropy_factor: f32
  - pub sparsity_factor: f32
  - pub data_type_factors: HashMap<DataType
- `SpeedPredictor`
  - pub base_speeds: HashMap<CompressionAlgorithm
  - pub size_scaling_factors: HashMap<CompressionAlgorithm
  - pub simd_speedup_factors: HashMap<SimdLevel
- `MemoryPredictor`
  - pub base_memory: HashMap<CompressionAlgorithm
  - pub size_scaling: HashMap<CompressionAlgorithm
- `ModelTrainingMetadata`
  - pub training_samples: usize
  - pub training_accuracy: f32
  - pub last_trained: chrono::DateTime<chrono::Utc>
  - pub feature_count: usize
- `CompressionRequest`
  - pub data: Vec<u8>
  - pub data_type: DataType
  - pub priority: CompressionPriority
  - pub optimization_target: OptimizationTarget
  - pub simd_enabled: bool
- `CompressionResult`
  - pub compressed_data: Vec<u8>
  - pub algorithm_used: CompressionAlgorithm
  - pub compression_ratio: f32
  - pub compression_time_ms: f32
  - pub memory_used_mb: f32
  - pub simd_used: bool
- `DecompressionResult`
  - pub decompressed_data: Vec<u8>
  - pub algorithm_used: CompressionAlgorithm
  - pub decompression_time_ms: f32
  - pub memory_used_mb: f32
  - pub simd_used: bool
- `NoCompressionBackend`
- `Lz4Backend`
  - private simd_capabilities: SimdCapabilities
- `ZstdBackend`
  - private level: i32
  - private simd_capabilities: SimdCapabilities
- `SnappyBackend`
  - private simd_capabilities: SimdCapabilities

**Enums:**
- `DataType`
  - DenseVectors
  - SparseVectors
  - Metadata
  - ClusterIds
  - Timestamps
- `CompressionPriority`
  - Low
  - Normal
  - High
  - Critical
- `OptimizationTarget`
  - MaxCompression
  - MaxSpeed
  - Balanced
  - MinMemory

**Traits:**
- `CompressionBackend`: Send + Sync
  - fn compress(&self, data: &[u8]) -> Result<Vec<u8>>
  - fn decompress(&self, compressed: &[u8]) -> Result<Vec<u8>>
  - fn characteristics(&self) -> CompressionCharacteristics
  - fn algorithm(&self) -> CompressionAlgorithm

##### storage::viper::config

**Structs:**
- `ViperConfig`
  - pub enable_clustering: bool
  - pub cluster_count: usize
  - pub min_vectors_per_partition: usize
  - pub max_vectors_per_partition: usize
  - pub enable_dictionary_encoding: bool
  - pub target_compression_ratio: f64
  - pub parquet_block_size: usize
  - pub row_group_size: usize
  - pub enable_column_stats: bool
  - pub enable_bloom_filters: bool
  - pub ttl_config: TTLConfig
  - pub compaction_config: crate::core::unified_types::CompactionConfig
- `TTLConfig`
  - pub enabled: bool
  - pub default_ttl: Option<Duration>
  - pub cleanup_interval: Duration
  - pub max_cleanup_batch_size: usize
  - pub enable_file_level_filtering: bool
  - pub min_file_expiration_age: Duration
- `ViperSchemaBuilder`
  - private filterable_fields: Vec<String>
  - private enable_ttl: bool
  - private enable_extra_meta: bool
  - private vector_dimension: Option<usize>
  - private schema_version: u32
  - private compression_level: Option<i32>

##### storage::viper::factory

**Structs:**
- `ViperSchemaFactory`
- `VectorProcessorFactory`
- `AdapterFactory`
- `ViperPipelineFactory`

##### storage::viper::flusher

**Structs:**
- `FlushResult`
  - pub entries_flushed: u64
  - pub bytes_written: u64
  - pub segments_created: u64
  - pub collections_affected: Vec<CollectionId>
  - pub flush_duration_ms: u64
- `ViperParquetFlusher`
  - private config: ViperConfig
  - private filesystem: Arc<FilesystemFactory>

##### storage::viper::index

**Structs:**
- `IndexVector`
  - pub id: VectorId
  - pub vector: Vec<f32>
  - pub metadata: serde_json::Value
  - pub cluster_id: Option<ClusterId>
- `IndexSearchResult`
  - pub vector_id: VectorId
  - pub distance: f32
  - pub metadata: Option<serde_json::Value>
- `IndexStats`
  - pub total_vectors: usize
  - pub index_size_bytes: usize
  - pub build_time_ms: u64
  - pub search_count: u64
  - pub avg_search_time_ms: f32
- `HNSWIndex`
  - private layers: Vec<HNSWLayer>
  - private vectors: Arc<RwLock<HashMap<VectorId
  - private distance_metric: DistanceMetric
  - private simd_level: SimdLevel
  - private config: HNSWConfig
  - private entry_point: Option<VectorId>
  - private stats: Arc<RwLock<IndexStats>>
- `HNSWConfig`
  - pub m: usize
  - pub ef_construction: usize
  - pub max_layers: usize
  - pub seed: u64
  - pub enable_pruning: bool
- `HNSWLayer`
  - private graph: HashMap<VectorId
  - private level: usize
- `SearchCandidate`
  - private id: VectorId
  - private distance: f32
- `IVFIndex`
  - private centroids: Vec<Vec<f32>>
  - private lists: cluster_id -> vector_ids
  - private inverted_lists: Vec<Vec<VectorId>>
  - private vectors: Arc<RwLock<HashMap<VectorId
  - private distance_metric: DistanceMetric
  - private config: IVFConfig
  - private stats: Arc<RwLock<IndexStats>>
- `IVFConfig`
  - pub num_clusters: usize
  - pub nprobe: usize
  - pub kmeans_iterations: usize
  - pub training_sample_size: usize
- `ViperIndexManager`
  - private hnsw_index: Option<Arc<RwLock<HNSWIndex>>>
  - private ivf_index: Option<Arc<RwLock<IVFIndex>>>
  - private config: ViperConfig
  - private strategy: IndexStrategy

**Enums:**
- `IndexStrategy`
  - HNSWOnly
  - IVFOnly
  - Adaptive{ threshold: usize

**Traits:**
- `VectorIndex`: Send + Sync
  - fn build(&mut self, vectors: Vec<IndexVector>) -> Result<()>
  - fn search(&self,
        query: &[f32],
        k: usize,
        ef: usize,
        filter_candidates: Option<&HashSet<VectorId>>,) -> Result<Vec<IndexSearchResult>>
  - fn add_vector(&mut self, vector: IndexVector) -> Result<()>
  - fn remove_vector(&mut self, vector_id: &VectorId) -> Result<()>
  - fn get_stats(&self) -> IndexStats
  - fn save_to_disk(&self, path: &str) -> Result<()>
  - fn load_from_disk(&mut self, path: &str) -> Result<()>

##### storage::viper::partitioner

**Structs:**
- `FilesystemPartitioner`
  - private storage_root: PathBuf
  - private partitions: Arc<RwLock<HashMap<PartitionId
  - private partition_models: Arc<RwLock<HashMap<CollectionId
  - private directory_cache: Arc<RwLock<DirectoryCache>>
  - private stats_tracker: Arc<RwLock<PartitionMetadataTracker>>
  - private reorg_scheduler: Arc<ReorganizationScheduler>
- `PartitionInfo`
  - pub partition_id: PartitionId
  - pub collection_id: CollectionId
  - pub cluster_id: ClusterId
  - pub storage_layout: StorageLayout
  - pub directory_path: PathBuf
  - pub statistics: PartitionStatistics
  - pub centroid: Vec<f32>
  - pub created_at: DateTime<Utc>
  - pub last_modified: DateTime<Utc>
- `PartitionModel`
  - pub model_id: String
  - pub version: String
  - pub algorithm: ClusteringAlgorithm
  - pub num_clusters: usize
  - pub centroids: Vec<Vec<f32>>
  - pub quality_metrics: ModelQualityMetrics
  - pub training_info: ModelTrainingInfo
  - pub model_data: Vec<u8>
- `ModelQualityMetrics`
  - pub silhouette_score: f32
  - pub davies_bouldin_index: f32
  - pub calinski_harabasz_index: f32
  - pub avg_intra_cluster_distance: f32
  - pub avg_inter_cluster_distance: f32
- `ModelTrainingInfo`
  - pub training_samples: usize
  - pub training_duration_ms: u64
  - pub trained_at: DateTime<Utc>
  - pub next_training_due: DateTime<Utc>
  - pub drift_metrics: DataDriftMetrics
- `DataDriftMetrics`
  - pub kl_divergence: f32
  - pub wasserstein_distance: f32
  - pub feature_drift: Vec<f32>
  - pub drift_detected: bool
- `DirectoryCache`
  - private collection_dirs: HashMap<CollectionId
  - private partition_files: HashMap<PartitionId
  - private last_refresh: Option<DateTime<Utc>>
- `FileMetadata`
  - pub file_name: String
  - pub file_size: u64
  - pub file_type: FileType
  - pub last_modified: DateTime<Utc>
  - pub compression_ratio: f32
- `PartitionMetadataTracker`
  - private access_stats: HashMap<PartitionId
  - private storage_stats: HashMap<PartitionId
  - private health_metrics: PartitionHealthMetrics
- `AccessStatistics`
  - pub total_reads: u64
  - pub total_writes: u64
  - pub last_access: Option<DateTime<Utc>>
  - pub access_frequency: f32
  - pub hot_vectors: Vec<VectorId>
- `StorageStatistics`
  - pub total_size_bytes: u64
  - pub compressed_size_bytes: u64
  - pub vector_count: usize
  - pub avg_vector_size: f32
  - pub density_distribution: DensityDistribution
- `DensityDistribution`
  - pub sparse_count: usize
  - pub dense_count: usize
  - pub avg_density: f32
  - pub density_histogram: Vec<(f32
- `PartitionHealthMetrics`
  - pub total_partitions: usize
  - pub healthy_partitions: usize
  - pub imbalanced_partitions: usize
  - pub stale_partitions: usize
  - pub avg_partition_efficiency: f32
- `ReorganizationScheduler`
  - private task_queue: Arc<RwLock<Vec<ReorganizationTask>>>
  - private active_operations: Arc<RwLock<HashMap<CollectionId
  - private policies: Arc<RwLock<ReorganizationPolicies>>
- `ReorganizationTask`
  - pub task_id: String
  - pub collection_id: CollectionId
  - pub trigger: ReorganizationTrigger
  - pub priority: ReorganizationPriority
  - pub estimated_duration: std::time::Duration
  - pub created_at: DateTime<Utc>
- `ReorganizationOperation`
  - pub task: ReorganizationTask
  - pub status: ReorganizationStatus
  - pub progress: ReorganizationProgress
  - pub start_time: DateTime<Utc>
  - pub worker_handle: tokio::task::JoinHandle<Result<ReorganizationResult>>
- `ReorganizationProgress`
  - pub current_phase: ReorganizationStatus
  - pub progress_percent: f32
  - pub vectors_processed: usize
  - pub partitions_created: usize
  - pub estimated_remaining: std::time::Duration
- `ReorganizationResult`
  - pub success: bool
  - pub duration: std::time::Duration
  - pub old_partitions: usize
  - pub new_partitions: usize
  - pub vectors_moved: usize
  - pub space_saved_bytes: i64
  - pub model_improvement: f32
  - pub error: Option<String>
- `ReorganizationPolicies`
  - pub auto_reorg_enabled: bool
  - pub min_reorg_interval: std::time::Duration
  - pub drift_threshold: f32
  - pub min_vectors_threshold: usize
  - pub max_concurrent_reorgs: usize
  - pub max_io_impact_percent: f32
- `PartitionConfig`
  - pub num_partitions: usize
  - pub clustering_algorithm: ClusteringAlgorithm
  - pub storage_strategy: StorageStrategy
  - pub sparsity_threshold: f32
  - pub row_group_size: usize
  - pub compression: String
  - pub sparse_format: SparseFormat

**Enums:**
- `StorageLayout`
  - DenseParquet{
  - file_path: PathBuf
  - row_group_size: usize
  - compression: String
- `SparseFormat`
  - COO
  - CSR
  - CustomKV
- `ClusteringAlgorithm`
  - KMeans{
  - n_clusters: usize
  - max_iterations: usize
  - tolerance: f32
- `FileType`
  - DenseParquet
  - SparseKeyValue
  - Metadata
  - Index
  - Model
- `ReorganizationTrigger`
  - Scheduled{ interval: std::time::Duration
- `ReorganizationPriority`
  - Low
  - Normal
  - High
  - Critical
- `ReorganizationStatus`
  - Preparing
  - LoadingData
  - TrainingModel
  - Redistributing
  - WritingPartitions
  - UpdatingIndexes
  - Verifying
  - Completing
  - Completed
  - Failed{ error: String
- `StorageStrategy`
  - Auto
  - ForceDense
  - ForceSparse
  - Hybrid

##### storage::viper::processor

**Structs:**
- `VectorRecordProcessor`<'a>
  - private strategy: &'a dyn SchemaGenerationStrategy
- `TimeSeriesVectorProcessor`<'a>
  - private base_processor: VectorRecordProcessor<'a>
  - private time_window_seconds: u64
- `SimilarityVectorProcessor`<'a>
  - private base_processor: VectorRecordProcessor<'a>
  - private cluster_threshold: f32

**Traits:**
- `VectorProcessor`
  - fn preprocess_records(&self, records: &mut [VectorRecord]) -> Result<()>
  - fn convert_to_batch(&self,
        records: &[VectorRecord],
        schema: &Arc<Schema>,) -> Result<RecordBatch>
  - fn postprocess_batch(&self, batch: RecordBatch) -> Result<RecordBatch>

##### storage::viper::schema

**Structs:**
- `ViperSchemaStrategy`
  - private collection_config: CollectionConfig
  - private schema_version: u32
  - private enable_ttl: bool
  - private enable_extra_meta: bool
- `LegacySchemaStrategy`
  - private collection_id: CollectionId

**Traits:**
- `SchemaGenerationStrategy`: Send + Sync
  - fn generate_schema(&self) -> Result<Arc<Schema>>
  - fn get_filterable_fields(&self) -> &[String]
  - fn get_collection_id(&self) -> &CollectionId
  - fn supports_ttl(&self) -> bool
  - fn get_version(&self) -> u32

##### storage::viper::search_engine

**Structs:**
- `ViperProgressiveSearchEngine`
  - private config: ViperConfig
  - private storage_searchers: Arc<RwLock<HashMap<String
  - private search_semaphore: Arc<Semaphore>
  - private feature_cache: Arc<RwLock<FeatureSelectionCache>>
  - private stats: Arc<RwLock<SearchStats>>
  - private index_manager: Arc<RwLock<HashMap<CollectionId
- `TierCharacteristics`
  - pub storage_url: String
  - pub avg_latency_ms: f32
  - pub throughput_mbps: f32
  - pub memory_resident: bool
  - pub supports_random_access: bool
  - pub compression_overhead: f32
- `FeatureSelectionCache`
  - private feature_sets: HashMap<CollectionId
  - private max_cache_size: usize
  - private access_order: Vec<CollectionId>
- `CachedFeatureSet`
  - pub features: Vec<usize>
  - pub importance_scores: Vec<f32>
  - pub last_updated: chrono::DateTime<chrono::Utc>
  - pub hit_count: u64
- `SearchStats`
  - pub total_searches: u64
  - pub storage_hits: HashMap<String
  - pub avg_latency_ms: f32
  - pub cluster_pruning_ratio: f32
  - pub feature_reduction_ratio: f32
- `ScoredResult`
  - private result: ViperSearchResult
  - private score: f32
- `WalVectorRecord`
  - pub id: String
  - pub vector: Vec<f32>
  - pub metadata: serde_json::Value
  - pub timestamp: DateTime<Utc>
  - pub operation_type: WalOperationType
- `LocalFileSearcher`
  - private storage_url: String
- `S3Searcher`
  - private storage_url: String
- `GenericSearcher`
  - private storage_url: String

**Enums:**
- `WalOperationType`
  - Insert
  - Update
  - Delete

**Traits:**
- `TierSearcher`: Send + Sync
  - fn search_tier(&self,
        context: &ViperSearchContext,
        partitions: Vec<PartitionId>,
        important_features: &[usize],) -> Result<Vec<ViperSearchResult>>
  - fn tier_characteristics(&self) -> TierCharacteristics

##### storage::viper::search_impl

**Structs:**
- `ViperSearchEngine`

##### storage::viper::staging_operations

**Structs:**
- `StagingOperationsCoordinator`
  - private filesystem: Arc<FilesystemFactory>
- `ParquetOptimizationConfig`
  - pub rows_per_rowgroup: usize
  - pub num_rowgroups: usize
  - pub num_chunks: usize
  - pub target_file_size_mb: usize
  - pub enable_dictionary_encoding: bool
  - pub enable_bloom_filters: bool
  - pub compression_level: i32
- `OptimizedParquetRecords`
  - pub records: Vec<VectorRecord>
  - pub schema_version: u32
  - pub column_order: Vec<String>
  - pub rowgroup_size: usize
  - pub compression_config: ParquetOptimizationConfig

**Enums:**
- `StagingOperationType`
  - Flush
  - Compaction

##### storage::viper::stats

**Structs:**
- `ViperStats`
  - pub total_entries_clustered: u64
  - pub clusters_created: usize
  - pub clustering_time_ms: u64
  - pub parquet_write_time_ms: u64
  - pub compression_ratio: f64
  - pub avg_vectors_per_cluster: f64
- `ViperPerformanceMetrics`
  - pub operation_type: String
  - pub collection_id: String
  - pub records_processed: u64
  - pub schema_generation_time_ms: u64
  - pub preprocessing_time_ms: u64
  - pub conversion_time_ms: u64
  - pub postprocessing_time_ms: u64
  - pub parquet_write_time_ms: u64
  - pub total_time_ms: u64
  - pub bytes_written: u64
  - pub compression_ratio: f64
  - pub records_per_second: f64
  - pub bytes_per_second: f64
- `ViperStatsCollector`
  - private operation_start: Option<Instant>
  - private phase_timers: HashMap<String
  - private metrics: ViperPerformanceMetrics
- `ViperAggregateStats`
  - pub total_operations: u64
  - pub total_records_processed: u64
  - pub total_bytes_written: u64
  - pub avg_compression_ratio: f64
  - pub avg_records_per_second: f64
  - pub avg_bytes_per_second: f64
  - pub avg_total_time_ms: f64
  - pub operations_by_type: HashMap<String
  - pub performance_by_collection: HashMap<String
- `CollectionPerformanceStats`
  - pub operations_count: u64
  - pub avg_records_per_operation: f64
  - pub avg_compression_ratio: f64
  - pub avg_throughput_records_per_sec: f64
- `ViperStatsAggregator`
  - private aggregate_stats: ViperAggregateStats
  - private recent_metrics: Vec<ViperPerformanceMetrics>
  - private max_recent_metrics: usize
- `PerformanceTrend`
  - pub throughput_change_percent: f64
  - pub is_improving: bool
  - pub confidence: ConfidenceLevel

**Enums:**
- `ConfidenceLevel`
  - Low
  - Medium
  - High

##### storage::viper::storage_engine

**Structs:**
- `ViperStorageEngine`
  - private config: ViperConfig
  - private collections: Arc<RwLock<HashMap<CollectionId
  - private clusters: Arc<RwLock<HashMap<ClusterId
  - private partitions: Arc<RwLock<HashMap<PartitionId
  - private ml_models: Arc<RwLock<HashMap<CollectionId
  - private feature_models: Arc<RwLock<HashMap<CollectionId
  - private compaction_manager: Arc<super::compaction::ViperCompactionEngine>
  - private writer_pool: Arc<ParquetWriterPool>
  - private stats: Arc<RwLock<ViperStorageStats>>
  - private ttl_service: Option<Arc<RwLock<super::ttl::TTLCleanupService>>>
  - private atomic_operations: Arc<super::atomic_operations::AtomicOperationsFactory>
- `CollectionMetadata`
  - pub collection_id: CollectionId
  - pub dimension: usize
  - pub vector_count: usize
  - pub total_clusters: usize
  - pub storage_format_preference: VectorStorageFormat
  - pub ml_model_version: Option<String>
  - pub feature_importance: Vec<f32>
  - pub compression_stats: CompressionStats
  - pub created_at: DateTime<Utc>
  - pub last_updated: DateTime<Utc>
- `CompressionStats`
  - pub sparse_compression_ratio: f32
  - pub dense_compression_ratio: f32
  - pub optimal_sparsity_threshold: f32
  - pub column_compression_ratios: Vec<f32>
- `ClusterPredictionModel`
  - pub model_id: String
  - pub version: String
  - pub accuracy: f32
  - pub feature_weights: Vec<f32>
  - pub cluster_centroids: Vec<Vec<f32>>
  - pub model_data: Vec<u8>
  - pub training_timestamp: DateTime<Utc>
  - pub prediction_stats: PredictionStats
- `FeatureImportanceModel`
  - pub model_id: String
  - pub importance_scores: Vec<f32>
  - pub top_k_features: Vec<usize>
  - pub compression_impact: Vec<f32>
  - pub clustering_impact: Vec<f32>
  - pub last_trained: DateTime<Utc>
- `PredictionStats`
  - pub total_predictions: u64
  - pub correct_predictions: u64
  - pub average_confidence: f32
  - pub prediction_latency_ms: f32
  - pub last_accuracy_check: DateTime<Utc>
- `ParquetWriterPool`
  - private writers: Arc<RwLock<HashMap<PartitionId
  - private max_concurrent_writers: usize
- `ParquetWriter`
  - private partition_id: PartitionId
  - private file_url: String
  - private schema: ParquetSchema
  - private row_group_size: usize
  - private current_batch: Vec<ViperVector>
  - private compression_algorithm: CompressionAlgorithm
- `ParquetSchema`
  - pub schema_version: u32
  - pub id_column: String
  - pub metadata_columns: Vec<String>
  - pub cluster_column: String
  - pub timestamp_column: String
  - pub vector_columns: VectorColumns
- `ViperStorageStats`
  - pub total_vectors: u64
  - pub total_partitions: u64
  - pub total_clusters: u64
  - pub compression_ratio: f32
  - pub ml_prediction_accuracy: f32
  - pub average_cluster_quality: f32
  - pub storage_size_bytes: u64
  - pub read_operations: u64
  - pub write_operations: u64
  - pub compaction_operations: u64
  - pub total_flushes: u64
  - pub total_compactions: u64
  - pub total_vectors_flushed: u64

**Enums:**
- `VectorColumns`
  - Dense{
  - dimension_columns: Vec<String>, // "dim_0", "dim_1", etc.

##### storage::viper::ttl

**Structs:**
- `TTLCleanupService`
  - private config: TTLConfig
  - private filesystem: Arc<FilesystemFactory>
  - private partition_metadata: Arc<RwLock<HashMap<PartitionId
  - private cleanup_task: Option<tokio::task::JoinHandle<()>>
  - private stats: Arc<RwLock<TTLStats>>
- `TTLStats`
  - pub total_cleanup_runs: u64
  - pub vectors_expired: u64
  - pub files_deleted: u64
  - pub last_cleanup_at: Option<DateTime<Utc>>
  - pub last_cleanup_duration_ms: u64
  - pub errors_count: u64
- `CleanupResult`
  - pub vectors_expired: u64
  - pub files_deleted: u64
  - pub partitions_processed: u64
  - pub errors: Vec<String>
  - pub duration_ms: u64

##### storage::viper::types

**Structs:**
- `DenseVectorRecord`
  - pub id: VectorId
  - pub metadata: serde_json::Value
  - pub cluster_id: ClusterId
  - pub vector: Vec<f32>
  - pub timestamp: DateTime<Utc>
  - pub expires_at: Option<DateTime<Utc>>
- `SparseVectorMetadata`
  - pub id: VectorId
  - pub metadata: serde_json::Value
  - pub cluster_id: ClusterId
  - pub dimension_count: u32
  - pub sparsity_ratio: f32
  - pub timestamp: DateTime<Utc>
  - pub expires_at: Option<DateTime<Utc>>
- `SparseVectorData`
  - pub id: VectorId
  - pub dimensions: Vec<(u32
- `ClusterMetadata`
  - pub cluster_id: ClusterId
  - pub collection_id: CollectionId
  - pub centroid: Vec<f32>
  - pub vector_count: usize
  - pub quality_metrics: ClusterQualityMetrics
  - pub partition_id: PartitionId
  - pub last_updated: DateTime<Utc>
  - pub statistics: ClusterStatistics
- `ClusterQualityMetrics`
  - pub intra_cluster_distance: f32
  - pub inter_cluster_distance: f32
  - pub silhouette_score: f32
  - pub density: f32
  - pub stability_score: f32
- `ClusterStatistics`
  - pub min_values: Vec<f32>
  - pub max_values: Vec<f32>
  - pub mean_values: Vec<f32>
  - pub std_dev_values: Vec<f32>
  - pub sparsity_ratio: f32
  - pub frequent_metadata: HashMap<String
  - private serde_json: :Value>
- `PartitionMetadata`
  - pub partition_id: PartitionId
  - pub collection_id: CollectionId
  - pub cluster_ids: Vec<ClusterId>
  - pub statistics: PartitionStatistics
  - pub storage_url: String
  - pub parquet_files: Vec<ParquetFileInfo>
  - pub bloom_filter: Option<Vec<u8>>
  - pub created_at: DateTime<Utc>
  - pub last_modified: DateTime<Utc>
- `PartitionStatistics`
  - pub total_vectors: usize
  - pub storage_size_bytes: u64
  - pub compressed_size_bytes: u64
  - pub compression_ratio: f32
  - pub access_frequency: f32
  - pub last_accessed: DateTime<Utc>
- `ParquetFileInfo`
  - pub file_path: String
  - pub storage_url: String
  - pub file_size_bytes: u64
  - pub row_group_count: usize
  - pub row_count: usize
  - pub cluster_ids: Vec<ClusterId>
  - pub schema_version: u32
  - pub checksum: String
  - pub created_at: DateTime<Utc>
- `ViperSearchResult`
  - pub vector_id: VectorId
  - pub score: f32
  - pub vector: Option<Vec<f32>>
  - pub metadata: serde_json::Value
  - pub cluster_id: ClusterId
  - pub partition_id: PartitionId
  - pub storage_url: String
- `ViperSearchContext`
  - pub collection_id: CollectionId
  - pub query_vector: Vec<f32>
  - pub k: usize
  - pub threshold: Option<f32>
  - pub filters: Option<HashMap<String
  - private serde_json: :Value>>
  - pub cluster_hints: Option<Vec<ClusterId>>
  - pub search_strategy: SearchStrategy
  - pub max_storage_locations: Option<usize>
- `ClusterPrediction`
  - pub cluster_id: ClusterId
  - pub confidence: f32
  - pub alternatives: Vec<(ClusterId
  - pub prediction_metadata: PredictionMetadata
- `PredictionMetadata`
  - pub model_version: String
  - pub features_used: Vec<String>
  - pub predicted_at: DateTime<Utc>
  - pub model_accuracy: f32
  - pub prediction_cost_ms: u64
- `ViperBatchOperation`
  - pub operation_type: BatchOperationType
  - pub vectors: Vec<VectorRecord>
  - pub cluster_predictions: Option<Vec<ClusterPrediction>>
  - pub batch_metadata: BatchMetadata
- `BatchMetadata`
  - pub batch_size: usize
  - pub estimated_duration_ms: u64
  - pub priority: BatchPriority
  - pub source: String
  - pub created_at: DateTime<Utc>

**Enums:**
- `ViperVector`
  - Sparse{
  - metadata: SparseVectorMetadata
  - data: SparseVectorData
- `SearchStrategy`
  - Exhaustive
  - ClusterPruned{
  - max_clusters: usize
  - confidence_threshold: f32
- `VectorStorageFormat`
  - Sparse
  - Dense
  - Auto
- `BatchOperationType`
  - Insert
  - Update
  - Delete
  - Recluster
- `BatchPriority`
  - Low
  - Normal
  - High
  - Critical

##### storage::wal::avro

**Structs:**
- `AvroWalEntry`
  - private entry_id: String
  - private collection_id: String
  - private operation: AvroWalOperation
  - private timestamp: i64
  - private sequence: i64
  - private global_sequence: i64
  - private expires_at: Option<i64>
  - private version: i64
- `AvroWalOperation`
  - private op_type: AvroOpType
  - private vector_id: Option<String>
  - private vector_data: Option<Vec<u8>>
  - private metadata: Option<String>
  - private config: Option<String>
  - private expires_at: Option<i64>
- `AvroWalStrategy`
  - private config: Option<WalConfig>
  - private filesystem: Option<Arc<FilesystemFactory>>
  - private memory_table: Option<WalMemTable>
  - private disk_manager: Option<WalDiskManager>

**Enums:**
- `AvroOpType`
  - Insert
  - Update
  - Delete
  - CreateCollection
  - DropCollection

##### storage::wal::background_manager

**Structs:**
- `BackgroundMaintenanceManager`
  - private collection_status: Arc<RwLock<HashMap<CollectionId
  - private config: Arc<WalConfig>
  - private stats: Arc<Mutex<BackgroundMaintenanceStats>>
- `BackgroundMaintenanceStats`
  - pub total_flush_operations: u64
  - pub total_compaction_operations: u64
  - pub flush_operations_skipped: u64
  - pub compaction_operations_skipped: u64
  - pub average_flush_duration_ms: f64
  - pub average_compaction_duration_ms: f64
  - pub concurrent_operations_prevented: u64

**Enums:**
- `BackgroundTaskStatus`
  - Idle
  - Flushing
  - Compacting
  - FlushAndCompact

##### storage::wal::bincode

**Structs:**
- `BincodeWalEntry`
  - private entry_id: String
  - private collection_id: String
  - private operation: BincodeWalOperation
  - private timestamp: i64
  - private sequence: u64
  - private global_sequence: u64
  - private expires_at: Option<i64>
  - private version: u64
- `BincodeWalOperation`
  - private op_type: u8
  - private vector_id: Option<String>
  - private vector_data: Option<Vec<u8>>
  - private metadata: Option<String>
  - private config: Option<String>
  - private expires_at: Option<i64>
- `BincodeWalStrategy`
  - private config: Option<WalConfig>
  - private filesystem: Option<Arc<FilesystemFactory>>
  - private memory_table: Option<WalMemTable>
  - private disk_manager: Option<WalDiskManager>

##### storage::wal::config

**Structs:**
- `CompressionConfig`
  - pub algorithm: crate::core::unified_types::CompressionAlgorithm
  - pub compress_memory: bool
  - pub compress_disk: bool
  - pub min_compress_size: usize
- `PerformanceConfig`
  - pub memory_flush_size_bytes: usize
  - pub disk_segment_size: usize
  - pub global_flush_threshold: usize
  - pub write_buffer_size: usize
  - pub concurrent_flushes: usize
  - pub batch_threshold: usize
  - pub mvcc_cleanup_interval_secs: u64
  - pub ttl_cleanup_interval_secs: u64
  - pub sync_mode: SyncMode
- `MultiDiskConfig`
  - pub data_directories: Vec<PathBuf>
  - pub distribution_strategy: DiskDistributionStrategy
  - pub collection_affinity: bool
- `MemTableConfig`
  - pub memtable_type: MemTableType
  - pub global_memory_limit: usize
  - pub mvcc_versions_retained: usize
  - pub enable_concurrency: bool
- `WalConfig`
  - pub strategy_type: WalStrategyType
  - pub memtable: MemTableConfig
  - pub multi_disk: MultiDiskConfig
  - pub compression: CompressionConfig
  - pub performance: PerformanceConfig
  - pub enable_mvcc: bool
  - pub enable_ttl: bool
  - pub enable_background_compaction: bool
  - pub collection_overrides: std::collections::HashMap<String
- `CollectionWalConfig`
  - pub memory_flush_size_bytes: Option<usize>
  - pub disk_segment_size: Option<usize>
  - pub compression: Option<CompressionConfig>
  - pub default_ttl_days: Option<u32>
- `CollectionEffectiveConfig`
  - pub disk_segment_size: usize
  - pub compression: CompressionConfig
  - pub default_ttl_days: Option<u32>
  - pub memory_flush_size_bytes: usize

**Enums:**
- `SyncMode`
  - Never
  - Always
  - Periodic
  - PerBatch
- `WalStrategyType`
  - Avro
  - Bincode
- `DiskDistributionStrategy`
  - RoundRobin
  - Hash
  - LoadBalanced
- `MemTableType`
  - SkipList
  - BTree
  - Art
  - HashMap

##### storage::wal::disk

**Structs:**
- `DiskSegment`
  - pub path: PathBuf
  - pub sequence_range: (u64
  - pub size_bytes: u64
  - pub entry_count: u64
  - pub created_at: DateTime<Utc>
  - pub modified_at: DateTime<Utc>
  - pub compression_ratio: f64
- `CollectionDiskLayout`
  - private collection_id: CollectionId
  - private disk_index: usize
  - private base_directory: PathBuf
  - private current_segment: u64
  - private segments: Vec<DiskSegment>
  - private total_size_bytes: u64
- `WalDiskManager`
  - private filesystem: Arc<FilesystemFactory>
  - private config: WalConfig
  - private collection_layouts: Arc<RwLock<HashMap<CollectionId
  - private disk_usage: Arc<RwLock<Vec<u64>>>
  - private disk_directories: Vec<PathBuf>
- `DiskUsage`
  - pub directory: PathBuf
  - pub usage_bytes: u64
  - pub collections: Vec<CollectionId>
- `DiskStats`
  - pub total_segments: u64
  - pub total_size_bytes: u64
  - pub collections_count: usize
  - pub disk_distribution: Vec<DiskUsage>
  - pub compression_ratio: f64

##### storage::wal::factory

**Structs:**
- `WalFactory`
- `StrategyInfo`
  - pub name: &'static str
  - pub description: &'static str
  - pub features: Vec<&'static str>
  - pub performance_profile: PerformanceProfile
  - pub use_cases: Vec<&'static str>
- `PerformanceProfile`
  - pub write_speed: PerformanceRating
  - pub read_speed: PerformanceRating
  - pub compression_ratio: PerformanceRating
  - pub memory_usage: PerformanceRating
  - pub cpu_usage: PerformanceRating
- `StrategySelector`
- `WorkloadCharacteristics`
  - pub write_heavy: bool
  - pub read_heavy: bool
  - pub schema_evolution: bool
  - pub cross_language: bool
  - pub storage_efficiency: bool
  - pub latency_sensitive: bool
  - pub throughput_critical: bool
- `StrategyComparison`
  - pub avro: StrategyInfo
  - pub bincode: StrategyInfo

**Enums:**
- `PerformanceRating`
  - Poor
  - Fair
  - Good
  - Excellent

##### storage::wal::memtable::art

**Structs:**
- `ArtMemTable`
  - private collections: Arc<RwLock<HashMap<CollectionId
  - private config: Option<WalConfig>
  - private global_sequence: Arc<tokio::sync::Mutex<u64>>
  - private total_memory_size: Arc<tokio::sync::Mutex<usize>>
- `CollectionArt`
  - private entries: HashMap<Vec<u8>
  - private collection_id: CollectionId
  - private sequence_counter: u64
  - private memory_size: usize
  - private last_flush_sequence: u64
  - private total_entries: u64
  - private last_write_time: DateTime<Utc>
- `ArtIndex`
  - private root: Option<Box<ArtNode>>
  - private total_keys: usize

**Enums:**
- `ArtNode`
  - Leaf{
  - key: String
  - sequences: Vec<u64>, // For MVCC support

##### storage::wal::memtable::btree

**Structs:**
- `BTreeMemTable`
  - private collections: Arc<RwLock<HashMap<CollectionId
  - private config: Option<WalConfig>
  - private global_sequence: Arc<tokio::sync::Mutex<u64>>
  - private total_memory_size: Arc<tokio::sync::Mutex<usize>>
- `CollectionBTree`
  - private entries: BTreeMap<u64
  - private vector_index: HashMap<VectorId
  - private collection_id: CollectionId
  - private sequence_counter: u64
  - private memory_size: usize
  - private last_flush_sequence: u64
  - private total_entries: u64
  - private last_write_time: DateTime<Utc>

##### storage::wal::memtable::hashmap

**Structs:**
- `HashMapMemTable`
  - private collections: Arc<RwLock<HashMap<CollectionId
  - private config: Option<WalConfig>
  - private global_sequence: Arc<tokio::sync::Mutex<u64>>
  - private total_memory_size: Arc<tokio::sync::Mutex<usize>>
- `CollectionHashMap`
  - private entries_by_sequence: HashMap<u64
  - private vector_index: HashMap<VectorId
  - private collection_id: CollectionId
  - private sequence_counter: u64
  - private memory_size: usize
  - private last_flush_sequence: u64
  - private total_entries: u64
  - private last_write_time: DateTime<Utc>

##### storage::wal::memtable::mod

**Structs:**
- `MemTableStats`
  - pub total_entries: u64
  - pub memory_bytes: usize
  - pub lookup_performance_ms: f64
  - pub insert_performance_ms: f64
  - pub range_scan_performance_ms: f64
- `MemTableMaintenanceStats`
  - pub mvcc_versions_cleaned: u64
  - pub ttl_entries_expired: u64
  - pub memory_compacted_bytes: usize
- `MemTableFactory`
- `MemTableCharacteristics`
  - pub name: &'static str
  - pub description: &'static str
  - pub write_performance: PerformanceRating
  - pub read_performance: PerformanceRating
  - pub range_scan_performance: PerformanceRating
  - pub memory_efficiency: PerformanceRating
  - pub concurrency: PerformanceRating
  - pub ordered: bool
  - pub best_for: &'static [&'static str]
- `MemTableSelector`
- `WorkloadCharacteristics`
  - pub write_heavy: bool
  - pub read_heavy: bool
  - pub range_queries: bool
  - pub ordered_access: bool
  - pub memory_constrained: bool
  - pub high_concurrency: bool
  - pub string_keys: bool
  - pub point_lookups_only: bool
- `WalMemTable`
  - private strategy: Box<dyn MemTableStrategy>

**Enums:**
- `MemTableType`
  - SkipList
  - BTree
  - Art
  - HashMap
- `PerformanceRating`
  - Poor
  - Fair
  - Good
  - Excellent

**Traits:**
- `MemTableStrategy`: Send + Sync + std::fmt::Debug
  - fn strategy_name(&self) -> &'static str
  - fn initialize(&mut self, config: &WalConfig) -> Result<()>
  - fn insert_entry(&self, entry: WalEntry) -> Result<u64>
  - fn insert_batch(&self, entries: Vec<WalEntry>) -> Result<Vec<u64>>
  - fn get_latest_entry(&self,
        collection_id: &CollectionId,
        vector_id: &VectorId,) -> Result<Option<WalEntry>>
  - fn get_entry_history(&self,
        collection_id: &CollectionId,
        vector_id: &VectorId,) -> Result<Vec<WalEntry>>
  - fn get_entries_from(&self,
        collection_id: &CollectionId,
        from_sequence: u64,
        limit: Option<usize>,) -> Result<Vec<WalEntry>>
  - fn get_all_entries(&self, collection_id: &CollectionId) -> Result<Vec<WalEntry>>
  - fn search_vector(&self,
        collection_id: &CollectionId,
        vector_id: &VectorId,) -> Result<Option<WalEntry>>
  - fn clear_flushed(&self, collection_id: &CollectionId, up_to_sequence: u64) -> Result<()>
  - fn drop_collection(&self, collection_id: &CollectionId) -> Result<()>
  - fn collections_needing_flush(&self) -> Result<Vec<CollectionId>>
  - fn needs_global_flush(&self) -> Result<bool>
  - fn get_stats(&self) -> Result<HashMap<CollectionId, MemTableStats>>
  - fn maintenance(&self) -> Result<MemTableMaintenanceStats>
  - fn close(&self) -> Result<()>
  - fn as_any(&self) -> &dyn std::any::Any

##### storage::wal::memtable::skiplist

**Structs:**
- `SkipListMemTable`
  - private collections: Arc<RwLock<HashMap<CollectionId
  - private config: Option<WalConfig>
  - private global_sequence: Arc<tokio::sync::Mutex<u64>>
  - private total_memory_size: Arc<tokio::sync::Mutex<usize>>
- `CollectionSkipList`
  - private entries: SkipMap<u64
  - private vector_index: Arc<RwLock<HashMap<VectorId
  - private collection_id: CollectionId
  - private sequence_counter: u64
  - private memory_size: usize
  - private last_flush_sequence: u64
  - private total_entries: u64
  - private last_write_time: DateTime<Utc>

##### storage::wal::mod

**Structs:**
- `WalEntry`
  - pub entry_id: String
  - pub collection_id: CollectionId
  - pub operation: WalOperation
  - pub timestamp: DateTime<Utc>
  - pub sequence: u64
  - pub global_sequence: u64
  - pub expires_at: Option<DateTime<Utc>>
  - pub version: u64
- `WalStats`
  - pub total_entries: u64
  - pub memory_entries: u64
  - pub disk_segments: u64
  - pub total_disk_size_bytes: u64
  - pub memory_size_bytes: u64
  - pub collections_count: usize
  - pub last_flush_time: Option<DateTime<Utc>>
  - pub write_throughput_entries_per_sec: f64
  - pub read_throughput_entries_per_sec: f64
  - pub compression_ratio: f64
- `FlushResult`
  - pub entries_flushed: u64
  - pub bytes_written: u64
  - pub segments_created: u64
  - pub collections_affected: Vec<CollectionId>
  - pub flush_duration_ms: u64
- `WalManager`
  - private strategy: Box<dyn WalStrategy>
  - private config: WalConfig
  - private stats: Arc<tokio::sync::RwLock<WalStats>>
  - private atomicity_manager: Option<Arc<crate::storage::atomicity::AtomicityManager>>

**Enums:**
- `WalOperation`
  - Insert{
  - vector_id: VectorId
  - record: VectorRecord
  - expires_at: Option<DateTime<Utc>>, // TTL support

**Traits:**
- `WalStrategy`: Send + Sync
  - fn strategy_name(&self) -> &'static str
  - fn initialize(&mut self,
        config: &WalConfig,
        filesystem: Arc<FilesystemFactory>,) -> Result<()>
  - fn serialize_entries(&self, entries: &[WalEntry]) -> Result<Vec<u8>>
  - fn deserialize_entries(&self, data: &[u8]) -> Result<Vec<WalEntry>>
  - fn write_entry(&self, entry: WalEntry) -> Result<u64>
  - fn write_batch(&self, entries: Vec<WalEntry>) -> Result<Vec<u64>>

### Index Domain

#### Module: index

##### index::axis::adaptive_engine

**Structs:**
- `AdaptiveIndexEngine`
  - private collection_analyzer: Arc<CollectionAnalyzer>
  - private strategy_selector: Arc<IndexStrategySelector>
  - private performance_predictor: Arc<PerformancePredictor>
  - private decision_history: Arc<RwLock<Vec<StrategyDecision>>>
  - private config: AxisConfig
- `CollectionCharacteristics`
  - pub collection_id: CollectionId
  - pub vector_count: u64
  - pub average_sparsity: f32
  - pub sparsity_variance: f32
  - pub dimension_variance: Vec<f32>
  - pub query_patterns: QueryPatternAnalysis
  - pub performance_metrics: PerformanceMetrics
  - pub growth_rate: f32
  - pub access_frequency: AccessFrequencyMetrics
  - pub metadata_complexity: MetadataComplexity
- `QueryPatternAnalysis`
  - pub total_queries: u64
  - pub point_query_percentage: f32
  - pub similarity_search_percentage: f32
  - pub metadata_filter_percentage: f32
  - pub average_k: f32
  - pub query_distribution: QueryDistribution
- `QueryDistribution`
  - pub uniform: bool
  - pub hotspot_percentage: f32
  - pub temporal_pattern: TemporalPattern
- `PerformanceMetrics`
  - pub average_query_latency_ms: f64
  - pub p99_query_latency_ms: f64
  - pub queries_per_second: f64
  - pub index_size_mb: f64
  - pub memory_usage_mb: f64
  - pub cache_hit_rate: f64
- `AccessFrequencyMetrics`
  - pub reads_per_second: f64
  - pub writes_per_second: f64
  - pub read_write_ratio: f64
  - pub peak_qps: f64
- `MetadataComplexity`
  - pub field_count: usize
  - pub average_field_cardinality: f64
  - pub nested_depth: usize
  - pub filter_selectivity: f64
- `IndexStrategySelector`
  - private strategy_templates: std::collections::HashMap<StrategyType
- `IndexStrategyTemplate`
  - pub strategy_type: StrategyType
  - pub base_strategy: IndexStrategy
  - pub applicability_conditions: ApplicabilityConditions
- `ApplicabilityConditions`
  - pub min_vector_count: Option<u64>
  - pub max_vector_count: Option<u64>
  - pub min_sparsity: Option<f32>
  - pub max_sparsity: Option<f32>
  - pub required_query_pattern: Option<QueryPatternType>
- `PerformancePredictor`
  - private models: Arc<RwLock<PredictionModels>>
- `PredictionModels`
  - pub latency_model: Option<Box<dyn LatencyPredictor + Send + Sync>>
  - pub throughput_model: Option<Box<dyn ThroughputPredictor + Send + Sync>>
  - pub resource_model: Option<Box<dyn ResourcePredictor + Send + Sync>>
- `PredictionFeatures`
  - pub vector_count: f64
  - pub sparsity: f64
  - pub dimension: f64
  - pub query_rate: f64
  - pub index_type: String
- `LatencyPrediction`
  - pub p50_ms: f64
  - pub p90_ms: f64
  - pub p99_ms: f64
- `ThroughputPrediction`
  - pub max_qps: f64
  - pub sustainable_qps: f64
- `ResourcePrediction`
  - pub memory_mb: f64
  - pub cpu_cores: f64
  - pub disk_mb: f64
- `StrategyDecision`
  - pub collection_id: CollectionId
  - pub timestamp: DateTime<Utc>
  - pub characteristics: CollectionCharacteristics
  - pub recommended_strategy: IndexStrategy
  - pub decision_reason: String
  - pub expected_improvement: f64

**Enums:**
- `TemporalPattern`
  - Uniform
  - Recent,                        // Most queries for recent data
  - Periodic(std::time::Duration), // Periodic access pattern
  - Bursty,                        // Sudden bursts of activity
- `StrategyType`
  - SmallDense
  - LargeDense
  - SmallSparse
  - LargeSparse
  - Mixed
  - MetadataHeavy
  - HighThroughput
  - Analytical
- `QueryPatternType`
  - PointLookup
  - SimilaritySearch
  - FilteredSearch
  - Analytical

**Traits:**
- `LatencyPredictor`
  - fn predict(&self, features: &PredictionFeatures) -> Result<LatencyPrediction>
- `ThroughputPredictor`
  - fn predict(&self, features: &PredictionFeatures) -> Result<ThroughputPrediction>
- `ResourcePredictor`
  - fn predict(&self, features: &PredictionFeatures) -> Result<ResourcePrediction>

##### index::axis::analyzer

**Structs:**
- `CollectionAnalyzer`
  - private query_tracker: Arc<RwLock<QueryPatternTracker>>
  - private performance_collector: Arc<PerformanceMetricsCollector>
  - private vector_analyzer: Arc<VectorCharacteristicsAnalyzer>
  - private metadata_analyzer: Arc<MetadataComplexityAnalyzer>
  - private temporal_detector: Arc<TemporalPatternDetector>
- `QueryPatternTracker`
  - private query_history: HashMap<CollectionId
  - private query_stats: HashMap<CollectionId
  - private max_history_size: usize
- `QueryEvent`
  - pub query_type: QueryType
  - pub timestamp: DateTime<Utc>
  - pub latency_ms: f64
  - pub k_value: Option<usize>
  - pub metadata_filters_count: usize
  - pub result_count: usize
  - pub success: bool
- `QueryStatistics`
  - pub total_queries: u64
  - pub point_queries: u64
  - pub similarity_queries: u64
  - pub filtered_queries: u64
  - pub hybrid_queries: u64
  - pub average_k: f32
  - pub average_latency_ms: f64
  - pub p99_latency_ms: f64
  - pub success_rate: f32
  - pub last_updated: DateTime<Utc>
- `PerformanceMetricsCollector`
  - private current_metrics: Arc<RwLock<HashMap<CollectionId
  - private historical_metrics: Arc<RwLock<HashMap<CollectionId
- `TimestampedMetrics`
  - pub metrics: PerformanceMetrics
  - pub timestamp: DateTime<Utc>
- `VectorCharacteristicsAnalyzer`
  - private cached_characteristics: Arc<RwLock<HashMap<CollectionId
- `VectorCharacteristics`
  - pub vector_count: u64
  - pub dimension: usize
  - pub average_sparsity: f32
  - pub sparsity_variance: f32
  - pub dimension_variance: Vec<f32>
  - pub growth_rate: f32
  - pub last_analyzed: DateTime<Utc>
- `MetadataComplexityAnalyzer`
  - private cached_complexity: Arc<RwLock<HashMap<CollectionId
- `TemporalPatternDetector`
  - private access_patterns: Arc<RwLock<HashMap<CollectionId
- `AccessPattern`
  - pub hourly_distribution: [f32; 24]
  - pub daily_distribution: [f32; 7]
  - pub recent_activity: Vec<ActivityBurst>
  - pub temporal_pattern: TemporalPattern
  - pub last_updated: DateTime<Utc>
- `ActivityBurst`
  - pub start_time: DateTime<Utc>
  - pub duration: Duration
  - pub intensity: f32
  - pub query_count: u64

**Enums:**
- `QueryType`
  - PointLookup
  - SimilaritySearch
  - FilteredSearch
  - Hybrid

##### index::axis::manager

**Structs:**
- `AxisIndexManager`
  - private global_id_index: Arc<GlobalIdIndex>
  - private metadata_index: Arc<MetadataIndex>
  - private dense_vector_index: Arc<DenseVectorIndex>
  - private sparse_vector_index: Arc<SparseVectorIndex>
  - private join_engine: Arc<JoinEngine>
  - private adaptive_engine: Arc<AdaptiveIndexEngine>
  - private migration_engine: Arc<IndexMigrationEngine>
  - private performance_monitor: Arc<PerformanceMonitor>
  - private collection_strategies: Arc<RwLock<HashMap<CollectionId
  - private active_migrations: Arc<RwLock<HashMap<CollectionId
  - private config: AxisConfig
  - private metrics: Arc<RwLock<AxisMetrics>>
- `MigrationStatus`
  - pub migration_id: uuid::Uuid
  - pub from_strategy: IndexStrategy
  - pub to_strategy: IndexStrategy
  - pub start_time: DateTime<Utc>
  - pub progress_percentage: f64
  - pub estimated_completion: Option<DateTime<Utc>>
- `AxisMetrics`
  - pub total_migrations: u64
  - pub successful_migrations: u64
  - pub failed_migrations: u64
  - pub average_migration_time_ms: u64
  - pub total_collections_managed: u64
  - pub total_vectors_indexed: u64
- `HybridQuery`
  - pub collection_id: CollectionId
  - pub vector_query: Option<VectorQuery>
  - pub metadata_filters: Vec<MetadataFilter>
  - pub id_filters: Vec<VectorId>
  - pub k: usize
  - pub include_expired: bool
- `MetadataFilter`
  - pub field: String
  - pub operator: FilterOperator
  - pub value: serde_json::Value
- `QueryResult`
  - pub results: Vec<ScoredResult>
  - pub strategy_used: IndexStrategy
  - pub execution_time_ms: u64
- `ScoredResult`
  - pub vector_id: VectorId
  - pub score: f32
  - pub expires_at: Option<DateTime<Utc>>

**Enums:**
- `VectorQuery`
  - Dense{
  - vector: Vec<f32>
  - similarity_threshold: f32
- `FilterOperator`
  - Equals
  - NotEquals
  - GreaterThan
  - LessThan
  - In
  - NotIn

##### index::axis::migration_engine

**Structs:**
- `IndexMigrationEngine`
  - private executor: Arc<MigrationExecutor>
  - private rollback_manager: Arc<RollbackManager>
  - private progress_tracker: Arc<RwLock<MigrationProgressTracker>>
  - private resource_limiter: Arc<Semaphore>
  - private history: Arc<RwLock<Vec<MigrationHistory>>>
- `MigrationPlan`
  - pub migration_id: uuid::Uuid
  - pub collection_id: CollectionId
  - pub from_strategy: IndexStrategy
  - pub to_strategy: IndexStrategy
  - pub steps: Vec<MigrationStep>
  - pub estimated_duration: Duration
  - pub priority: MigrationPriority
  - pub rollback_points: Vec<RollbackPoint>
- `MigrationStep`
  - pub step_id: String
  - pub step_type: MigrationStepType
  - pub estimated_duration: Duration
  - pub resource_requirements: ResourceRequirements
  - pub can_rollback: bool
- `IndexBuildParams`
  - pub parallel_threads: usize
  - pub memory_limit_mb: usize
  - pub optimization_level: OptimizationLevel
- `ResourceRequirements`
  - pub cpu_cores: f32
  - pub memory_mb: usize
  - pub disk_mb: usize
  - pub io_bandwidth_mbps: f32
- `RollbackPoint`
  - pub point_id: String
  - pub step_id: String
  - pub state_snapshot: StateSnapshot
  - pub created_at: DateTime<Utc>
- `StateSnapshot`
  - pub index_states: Vec<IndexState>
  - pub traffic_distribution: TrafficDistribution
  - pub metadata: serde_json::Value
- `IndexState`
  - pub index_type: IndexType
  - pub vector_count: u64
  - pub last_updated: DateTime<Utc>
  - pub is_active: bool
- `TrafficDistribution`
  - pub read_distribution: Vec<(IndexType
  - pub write_distribution: Vec<(IndexType
- `MigrationResult`
  - pub migration_id: uuid::Uuid
  - pub success: bool
  - pub new_strategy: IndexStrategy
  - pub duration_ms: u64
  - pub vectors_migrated: u64
  - pub performance_improvement: f32
  - pub errors: Vec<MigrationError>
- `MigrationError`
  - pub step_id: String
  - pub error_type: MigrationErrorType
  - pub message: String
  - pub recoverable: bool
- `MigrationExecutor`
  - private step_executors: Vec<Box<dyn StepExecutor + Send + Sync>>
- `MigrationContext`
  - pub collection_id: CollectionId
  - pub migration_id: uuid::Uuid
  - pub from_strategy: IndexStrategy
  - pub to_strategy: IndexStrategy
  - pub progress: Arc<RwLock<MigrationProgress>>
- `StepResult`
  - pub success: bool
  - pub duration: Duration
  - pub vectors_processed: u64
  - pub metrics: StepMetrics
- `StepMetrics`
  - pub cpu_usage: f32
  - pub memory_usage_mb: usize
  - pub io_operations: u64
  - pub errors_encountered: u64
- `RollbackManager`
  - private strategies: Vec<Box<dyn RollbackStrategy + Send + Sync>>
- `MigrationProgressTracker`
  - private active_migrations: Vec<MigrationProgress>
- `MigrationProgress`
  - pub migration_id: uuid::Uuid
  - pub current_step: usize
  - pub total_steps: usize
  - pub vectors_processed: u64
  - pub total_vectors: u64
  - pub start_time: Instant
  - pub estimated_completion: Option<DateTime<Utc>>
  - pub current_phase: MigrationPhase
- `MigrationHistory`
  - pub migration_id: uuid::Uuid
  - pub collection_id: CollectionId
  - pub from_strategy: IndexStrategy
  - pub to_strategy: IndexStrategy
  - pub start_time: DateTime<Utc>
  - pub end_time: DateTime<Utc>
  - pub result: MigrationResult
- `CreateIndexExecutor`
- `CopyDataExecutor`
- `BuildIndexExecutor`
- `VerifyConsistencyExecutor`
- `SwitchTrafficExecutor`

**Enums:**
- `MigrationStepType`
  - CreateNewIndex{ index_type: IndexType
- `OptimizationLevel`
  - Fast,     // Prioritize speed
  - Balanced, // Balance speed and quality
  - Quality,  // Prioritize index quality
- `VerificationType`
  - Checksum
  - SampleQuery
  - FullScan
- `MigrationErrorType`
  - ResourceExhausted
  - DataCorruption
  - ConsistencyCheckFailed
  - Timeout
  - Unknown
- `MigrationPhase`
  - Initializing
  - CreatingIndexes
  - CopyingData
  - BuildingIndexes
  - Verifying
  - SwitchingTraffic
  - Cleanup
  - Completed
  - Failed

**Traits:**
- `StepExecutor`
  - fn execute(&self, step: &MigrationStep, context: &MigrationContext) -> Result<StepResult>
  - fn can_handle(&self, step_type: &MigrationStepType) -> bool
- `RollbackStrategy`
  - fn rollback(&self, point: &RollbackPoint, context: &MigrationContext) -> Result<()>
  - fn supports_point(&self, point: &RollbackPoint) -> bool

##### index::axis::mod

**Structs:**
- `AxisConfig`
  - pub migration_config: MigrationConfig
  - pub monitoring_config: MonitoringConfig
  - pub strategy_config: StrategyConfig
  - pub resource_limits: ResourceLimits
- `MigrationConfig`
  - pub improvement_threshold: f64
  - pub max_concurrent_migrations: usize
  - pub migration_batch_size: usize
  - pub rollback_timeout_seconds: u64
  - pub auto_migration_enabled: bool
- `MonitoringConfig`
  - pub metrics_interval_seconds: u64
  - pub alert_thresholds: AlertThresholds
  - pub detailed_logging: bool
- `AlertThresholds`
  - pub max_query_latency_ms: u64
  - pub min_query_throughput: f64
  - pub max_error_rate: f64
- `StrategyConfig`
  - pub use_ml_models: bool
  - pub evaluation_interval_seconds: u64
  - pub min_training_size: usize
- `ResourceLimits`
  - pub max_index_memory_gb: f64
  - pub max_concurrent_operations: usize
  - pub max_rebuild_time_seconds: u64

**Enums:**
- `MigrationDecision`
  - Migrate{
  - from: IndexStrategy
  - to: IndexStrategy
  - estimated_improvement: f64
  - migration_complexity: f64
  - estimated_duration: std::time::Duration
- `MigrationPriority`
  - Low
  - Medium
  - High
  - Critical

##### index::axis::monitor

**Structs:**
- `PerformanceMonitor`
  - private config: MonitoringConfig
  - private metrics_collector: Arc<MetricsCollector>
  - private alert_manager: Arc<AlertManager>
  - private performance_tracker: Arc<PerformanceTracker>
  - private health_checker: Arc<HealthChecker>
  - private event_broadcaster: broadcast::Sender<MonitoringEvent>
- `MetricsCollector`
  - private collection_metrics: Arc<RwLock<HashMap<CollectionId
  - private system_metrics: Arc<RwLock<SystemMetrics>>
  - private historical_metrics: Arc<RwLock<Vec<HistoricalMetric>>>
  - private retention_period: Duration
- `AlertManager`
  - private thresholds: AlertThresholds
  - private active_alerts: Arc<RwLock<HashMap<String
  - private alert_history: Arc<RwLock<Vec<AlertHistory>>>
  - private subscribers: Arc<RwLock<Vec<Box<dyn AlertSubscriber + Send + Sync>>>>
- `PerformanceTracker`
  - private trends: Arc<RwLock<HashMap<CollectionId
  - private baselines: Arc<RwLock<HashMap<CollectionId
  - private anomaly_detector: Arc<AnomalyDetector>
- `HealthChecker`
  - private component_health: Arc<RwLock<HashMap<String
  - private check_interval: Duration
- `CollectionMetrics`
  - pub collection_id: CollectionId
  - pub query_latency_ms: LatencyMetrics
  - pub throughput_qps: f64
  - pub error_rate: f64
  - pub index_performance: IndexPerformanceMetrics
  - pub resource_usage: ResourceUsageMetrics
  - pub last_updated: DateTime<Utc>
- `LatencyMetrics`
  - pub p50: f64
  - pub p90: f64
  - pub p95: f64
  - pub p99: f64
  - pub p999: f64
  - pub average: f64
  - pub max: f64
- `IndexPerformanceMetrics`
  - pub index_build_time_ms: f64
  - pub index_size_mb: f64
  - pub cache_hit_rate: f64
  - pub false_positive_rate: f64
  - pub recall_rate: f64
- `ResourceUsageMetrics`
  - pub cpu_usage_percent: f64
  - pub memory_usage_mb: f64
  - pub disk_usage_mb: f64
  - pub network_bandwidth_mbps: f64
- `SystemMetrics`
  - pub total_collections: u64
  - pub total_vectors: u64
  - pub total_queries_per_second: f64
  - pub overall_cpu_usage: f64
  - pub overall_memory_usage_mb: f64
  - pub active_migrations: u64
  - pub last_updated: DateTime<Utc>
- `HistoricalMetric`
  - pub timestamp: DateTime<Utc>
  - pub collection_id: Option<CollectionId>
  - pub metric_type: MetricType
  - pub value: f64
- `Alert`
  - pub alert_id: String
  - pub alert_type: AlertType
  - pub severity: AlertSeverity
  - pub collection_id: Option<CollectionId>
  - pub message: String
  - pub triggered_at: DateTime<Utc>
  - pub metric_value: f64
  - pub threshold_value: f64
  - pub resolved: bool
- `AlertHistory`
  - pub alert: Alert
  - pub resolved_at: Option<DateTime<Utc>>
  - pub resolution_time_ms: Option<u64>
- `PerformanceTrend`
  - pub collection_id: CollectionId
  - pub latency_trend: TrendDirection
  - pub throughput_trend: TrendDirection
  - pub error_rate_trend: TrendDirection
  - pub trend_confidence: f64
  - pub last_analyzed: DateTime<Utc>
- `BaselineMetrics`
  - pub collection_id: CollectionId
  - pub baseline_latency_ms: f64
  - pub baseline_throughput_qps: f64
  - pub baseline_error_rate: f64
  - pub established_at: DateTime<Utc>
  - pub sample_count: u64
- `AnomalyDetector`
  - private models: Arc<RwLock<HashMap<CollectionId
- `AnomalyModel`
  - pub collection_id: CollectionId
  - pub model_type: AnomalyModelType
  - pub sensitivity: f64
  - pub training_data: Vec<f64>
  - pub last_trained: DateTime<Utc>
- `ComponentHealth`
  - pub component_name: String
  - pub status: HealthStatus
  - pub last_check: DateTime<Utc>
  - pub error_message: Option<String>
  - pub response_time_ms: f64

**Enums:**
- `MetricType`
  - QueryLatency
  - Throughput
  - ErrorRate
  - CpuUsage
  - MemoryUsage
  - CacheHitRate
- `AlertType`
  - HighLatency
  - LowThroughput
  - HighErrorRate
  - ResourceExhaustion
  - IndexDegradation
  - MigrationFailure
  - SystemHealth
- `AlertSeverity`
  - Info
  - Warning
  - Critical
  - Emergency
- `TrendDirection`
  - Improving
  - Stable
  - Degrading
  - Unknown
- `AnomalyModelType`
  - StatisticalThreshold
  - MovingAverage
  - SeasonalDecomposition
  - MachineLearning
- `HealthStatus`
  - Healthy
  - Degraded
  - Unhealthy
  - Unknown
- `MonitoringEvent`
  - MetricsUpdated{
  - collection_id: CollectionId
  - metrics: CollectionMetrics

**Traits:**
- `AlertSubscriber`
  - fn on_alert(&self, alert: &Alert) -> Result<()>
  - fn on_alert_resolved(&self, alert: &Alert) -> Result<()>

##### index::axis::strategy

**Structs:**
- `IndexStrategy`
  - pub primary_index_type: IndexType
  - pub secondary_indexes: Vec<IndexType>
  - pub optimization_config: OptimizationConfig
  - pub migration_priority: MigrationPriority
  - pub resource_requirements: ResourceRequirements
- `OptimizationConfig`
  - pub enable_caching: bool
  - pub cache_size_mb: usize
  - pub enable_prefetching: bool
  - pub batch_size: usize
  - pub enable_simd: bool
  - pub enable_gpu: bool
  - pub compression: CompressionConfig
- `CompressionConfig`
  - pub enabled: bool
  - pub algorithm: CompressionAlgorithm
  - pub level: u8
- `ResourceRequirements`
  - pub memory_gb: f64
  - pub cpu_cores: f64
  - pub disk_gb: f64
  - pub network_mbps: f64

**Enums:**
- `IndexType`
  - GlobalIdOnly
  - GlobalId
  - Metadata
  - DenseVector
  - SparseVector
  - JoinEngine
  - LightweightHNSW
  - HNSW
  - PartitionedHNSW
  - ProductQuantization
  - VectorCompression
  - LSMTree
  - MinHashLSH
  - InvertedIndex
  - SparseOptimized
  - SparseBasic
  - HybridSmall
  - FullAXIS
- `CompressionAlgorithm`
  - None
  - Lz4
  - Zstd
  - Snappy
  - Adaptive

##### index::mod

**Structs:**
- `GlobalIdIndex`
- `MetadataIndex`
- `DenseVectorIndex`
- `SparseVectorIndex`
- `JoinEngine`

### API/Network Domain

#### Module: network

##### network::grpc::service

**Structs:**
- `ProximaDbGrpcService`
  - private avro_service: Arc<UnifiedAvroService>
  - private collection_service: Arc<CollectionService>

##### network::metrics_service

**Structs:**
- `MetricsServiceConfig`
  - pub enable_prometheus: bool
  - pub enable_json: bool
  - pub enable_health: bool
  - pub enable_alerts: bool
- `MetricsQuery`
  - pub format: Option<String>
  - pub since: Option<String>
- `MetricsHealthResponse`
  - pub status: String
  - pub metrics_enabled: bool
  - pub last_collection: Option<chrono::DateTime<chrono::Utc>>
  - pub collection_interval_ms: u64
- `MetricsService`
  - private config: MetricsServiceConfig
  - private metrics_collector: Arc<MetricsCollector>

##### network::middleware::auth

**Structs:**
- `AuthConfig`
  - pub enabled: bool
  - pub api_keys: HashMap<String
  - pub require_auth_for_health: bool
- `UserInfo`
  - pub user_id: String
  - pub tenant_id: Option<String>
  - pub permissions: Vec<String>
- `AuthErrorResponse`
  - private error: String
  - private message: String
- `AuthLayer`
  - private config: AuthConfig

**Traits:**
- `RequestUserInfo`
  - fn user_info(&self) -> Option<&UserInfo>

##### network::middleware::rate_limit

**Structs:**
- `RateLimitConfig`
  - pub enabled: bool
  - pub max_requests: u32
  - pub window_duration: Duration
  - pub limit_health_endpoints: bool
  - pub global_max_requests: Option<u32>
- `RateLimitBucket`
  - private count: u32
  - private window_start: Instant
- `RateLimitState`
  - private config: RateLimitConfig
  - private buckets: Arc<RwLock<HashMap<IpAddr
  - private global_bucket: Arc<RwLock<RateLimitBucket>>
- `RateLimitErrorResponse`
  - private error: String
  - private message: String
  - private retry_after: u64
- `RateLimitLayer`
  - private state: Arc<RateLimitState>

##### network::mod

**Structs:**
- `NetworkConfig`
  - pub bind_address: String
  - pub port: u16
  - pub enable_grpc: bool
  - pub enable_rest: bool
  - pub enable_dashboard: bool
  - pub auth: AuthConfig
  - pub rate_limit: RateLimitConfig
  - pub request_timeout_secs: u64
  - pub max_request_size: usize
  - pub keep_alive_timeout_secs: u64
  - pub tcp_nodelay: bool
- `AuthConfig`
  - pub enabled: bool
  - pub jwt_secret: Option<String>
  - pub jwt_expiration_secs: u64
  - pub api_keys: Vec<String>
- `RateLimitConfig`
  - pub enabled: bool
  - pub requests_per_minute: u32
  - pub burst_size: u32
  - pub by_ip: bool

##### network::multi_server

**Structs:**
- `MultiServerConfig`
  - pub http_config: RestHttpServerConfig
  - pub grpc_config: GrpcHttpServerConfig
  - pub tls_config: TLSConfig
- `TLSConfig`
  - pub cert_file: Option<String>
  - pub key_file: Option<String>
  - pub key_password: Option<String>
  - private default: 0.0.0.0)
  - pub bind_interface: String
  - pub enabled: bool
- `RestHttpServerConfig`
  - private default: 5678)
  - pub port: u16
  - pub enable_rest: bool
  - pub enable_dashboard: bool
  - pub enable_metrics: bool
  - pub enable_health: bool
  - pub tls_cert_file: Option<String>
  - pub tls_key_file: Option<String>
- `GrpcHttpServerConfig`
  - private default: 5679)
  - pub port: u16
  - pub bind_address: SocketAddr
  - pub tls_bind_address: Option<SocketAddr>
  - pub enable_grpc: bool
  - pub max_message_size: usize
  - pub enable_reflection: bool
  - pub enable_compression: bool
  - pub tls_cert_file: Option<String>
  - pub tls_key_file: Option<String>
- `SharedServices`
  - pub collection_service: Arc<CollectionService>
  - pub vector_service: Arc<UnifiedAvroService>
  - pub metrics_collector: Option<Arc<MetricsCollector>>
  - private storage: Arc<RwLock<StorageEngine>>
- `MultiServer`
  - private config: MultiServerConfig
  - private shared_services: Option<SharedServices>
  - private server_handles: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>
- `ServerStatus`
  - pub http_running: bool
  - pub grpc_running: bool
  - pub http_address: Option<SocketAddr>
  - pub grpc_address: Option<SocketAddr>
  - pub tls_enabled: bool

##### network::rest::handlers

**Structs:**
- `AppState`
  - pub unified_service: Arc<UnifiedAvroService>
  - pub collection_service: Arc<CollectionService>
- `CreateCollectionRequest`
  - pub name: String
  - pub dimension: Option<usize>
  - pub distance_metric: Option<String>
  - pub indexing_algorithm: Option<String>
- `InsertVectorRequest`
  - pub id: Option<String>
  - pub vector: Vec<f32>
  - pub metadata: Option<HashMap<String
  - private serde_json: :Value>>
- `SearchVectorRequest`
  - pub vector: Vec<f32>
  - pub k: Option<usize>
  - pub filters: Option<HashMap<String
  - private serde_json: :Value>>
  - pub include_vectors: Option<bool>
  - pub include_metadata: Option<bool>
- `ApiResponse`<T>
  - pub success: bool
  - pub data: Option<T>
  - pub error: Option<String>
  - pub message: Option<String>

##### network::rest::server

**Structs:**
- `RestServer`
  - private router: Router
  - private bind_addr: SocketAddr

##### network::server_builder

**Structs:**
- `RestHttpServerBuilder`
  - private bind_address: SocketAddr
  - private tls_bind_address: Option<SocketAddr>
  - private enable_rest: bool
  - private enable_dashboard: bool
  - private enable_metrics: bool
  - private enable_health: bool
  - private tls_cert_file: Option<String>
  - private tls_key_file: Option<String>
- `GrpcHttpServerBuilder`
  - private bind_address: SocketAddr
  - private tls_bind_address: Option<SocketAddr>
  - private enable_grpc: bool
  - private tls_cert_file: Option<String>
  - private tls_key_file: Option<String>
  - private max_message_size: usize
  - private enable_reflection: bool
- `MultiServerBuilder`
  - private http_builder: RestHttpServerBuilder
  - private grpc_builder: GrpcHttpServerBuilder

#### Module: proto

##### proto::proximadb

**Structs:**
- `VectorRecord`
  - pub id: ::core::option::Option<::prost::alloc::string::String>
  - pub vector: ::prost::alloc::vec::Vec<f32>
  - pub metadata: ::std::collections::HashMap<
  - private prost: :alloc::string::String
  - private prost: :alloc::string::String
  - pub timestamp: ::core::option::Option<i64>
  - pub version: i64
  - pub expires_at: ::core::option::Option<i64>
- `CollectionConfig`
  - pub name: ::prost::alloc::string::String
  - pub dimension: i32
  - pub distance_metric: i32
  - pub storage_engine: i32
  - pub indexing_algorithm: i32
  - pub filterable_metadata_fields: ::prost::alloc::vec::Vec<
  - private prost: :alloc::string::String
  - pub indexing_config: ::std::collections::HashMap<
  - private prost: :alloc::string::String
  - private prost: :alloc::string::String
  - pub filterable_columns: ::prost::alloc::vec::Vec<FilterableColumnSpec>
- `FilterableColumnSpec`
  - pub name: ::prost::alloc::string::String
  - pub data_type: i32
  - pub indexed: bool
  - pub supports_range: bool
  - pub estimated_cardinality: ::core::option::Option<i32>
- `Collection`
  - pub id: ::prost::alloc::string::String
  - pub config: ::core::option::Option<CollectionConfig>
  - pub stats: ::core::option::Option<CollectionStats>
  - pub created_at: i64
  - pub updated_at: i64
- `CollectionStats`
  - pub vector_count: i64
  - pub index_size_bytes: i64
  - pub data_size_bytes: i64
- `SearchResult`
  - pub id: ::core::option::Option<::prost::alloc::string::String>
  - pub score: f32
  - pub vector: ::prost::alloc::vec::Vec<f32>
  - pub metadata: ::std::collections::HashMap<
  - private prost: :alloc::string::String
  - private prost: :alloc::string::String
  - pub rank: ::core::option::Option<i32>
- `CollectionRequest`
  - pub operation: i32
  - pub collection_id: ::core::option::Option<::prost::alloc::string::String>
  - pub collection_config: ::core::option::Option<CollectionConfig>
  - pub query_params: ::std::collections::HashMap<
  - private prost: :alloc::string::String
  - private prost: :alloc::string::String
  - pub options: ::std::collections::HashMap<::prost::alloc::string::String
  - pub migration_config: ::std::collections::HashMap<
  - private prost: :alloc::string::String
  - private prost: :alloc::string::String
- `CollectionResponse`
  - pub success: bool
  - pub operation: i32
  - pub collection: ::core::option::Option<Collection>
  - pub collections: ::prost::alloc::vec::Vec<Collection>
  - pub affected_count: i64
  - pub total_count: ::core::option::Option<i64>
  - pub metadata: ::std::collections::HashMap<
  - private prost: :alloc::string::String
  - private prost: :alloc::string::String
  - pub error_message: ::core::option::Option<::prost::alloc::string::String>
  - pub error_code: ::core::option::Option<::prost::alloc::string::String>
  - pub processing_time_us: i64
- `VectorInsertRequest`
  - pub collection_id: ::prost::alloc::string::String
  - pub upsert_mode: bool
  - pub vectors_avro_payload: ::prost::alloc::vec::Vec<u8>
  - pub batch_timeout_ms: ::core::option::Option<i64>
  - pub request_id: ::core::option::Option<::prost::alloc::string::String>
- `VectorMutationRequest`
  - pub collection_id: ::prost::alloc::string::String
  - pub operation: i32
  - pub selector: ::core::option::Option<VectorSelector>
  - pub updates: ::core::option::Option<VectorUpdates>
- `VectorSelector`
  - pub ids: ::prost::alloc::vec::Vec<::prost::alloc::string::String>
  - pub metadata_filter: ::std::collections::HashMap<
  - private prost: :alloc::string::String
  - private prost: :alloc::string::String
  - pub vector_match: ::prost::alloc::vec::Vec<f32>
- `VectorUpdates`
  - pub vector: ::prost::alloc::vec::Vec<f32>
  - pub metadata: ::std::collections::HashMap<
  - private prost: :alloc::string::String
  - private prost: :alloc::string::String
  - pub expires_at: ::core::option::Option<i64>
- `VectorSearchRequest`
  - pub collection_id: ::prost::alloc::string::String
  - pub queries: ::prost::alloc::vec::Vec<SearchQuery>
  - pub top_k: i32
  - pub distance_metric_override: ::core::option::Option<i32>
  - pub search_params: ::std::collections::HashMap<
  - private prost: :alloc::string::String
  - private prost: :alloc::string::String
  - pub include_fields: ::core::option::Option<IncludeFields>
- `SearchQuery`
  - pub vector: ::prost::alloc::vec::Vec<f32>
  - pub id: ::core::option::Option<::prost::alloc::string::String>
  - pub metadata_filter: ::std::collections::HashMap<
  - private prost: :alloc::string::String
  - private prost: :alloc::string::String
- `IncludeFields`
  - pub vector: bool
  - pub metadata: bool
  - pub score: bool
  - pub rank: bool
- `VectorOperationResponse`
  - pub success: bool
  - pub operation: i32
  - pub metrics: ::core::option::Option<OperationMetrics>
  - pub vector_ids: ::prost::alloc::vec::Vec<::prost::alloc::string::String>
  - pub error_message: ::core::option::Option<::prost::alloc::string::String>
  - pub error_code: ::core::option::Option<::prost::alloc::string::String>
  - pub result_info: ::core::option::Option<ResultMetadata>
  - private vector_operation_response: :ResultPayload"
  - pub result_payload: ::core::option::Option<vector_operation_response::ResultPayload>
- `SearchResultsCompact`
  - pub results: ::prost::alloc::vec::Vec<SearchResult>
  - pub total_found: i64
  - pub search_algorithm_used: ::core::option::Option<::prost::alloc::string::String>
- `ResultMetadata`
  - pub result_count: i64
  - pub estimated_size_bytes: i64
  - pub is_avro_binary: bool
  - pub avro_schema_version: ::prost::alloc::string::String
- `OperationMetrics`
  - pub total_processed: i64
  - pub successful_count: i64
  - pub failed_count: i64
  - pub updated_count: i64
  - pub processing_time_us: i64
  - pub wal_write_time_us: i64
  - pub index_update_time_us: i64
- `HealthRequest`
- `HealthResponse`
  - pub status: ::prost::alloc::string::String
  - pub version: ::prost::alloc::string::String
  - pub uptime_seconds: i64
  - pub active_connections: i32
  - pub memory_usage_bytes: i64
  - pub storage_usage_bytes: i64
- `MetricsRequest`
  - pub collection_id: ::core::option::Option<::prost::alloc::string::String>
  - pub metric_names: ::prost::alloc::vec::Vec<::prost::alloc::string::String>
- `MetricsResponse`
  - pub metrics: ::std::collections::HashMap<::prost::alloc::string::String
  - pub timestamp: i64
- `ProximaDbClient`<T>
  - private inner: tonic::client::Grpc<T>
- `ProximaDbServer`<T: ProximaDb>
  - private inner: _Inner<T>
  - private accept_compression_encodings: EnabledCompressionEncodings
  - private send_compression_encodings: EnabledCompressionEncodings
  - private max_decoding_message_size: Option<usize>
  - private max_encoding_message_size: Option<usize>
- `_Inner`<T>
- `CollectionOperationSvc`<T: ProximaDb>
- `VectorInsertSvc`<T: ProximaDb>
- `VectorMutationSvc`<T: ProximaDb>
- `VectorSearchSvc`<T: ProximaDb>
- `HealthSvc`<T: ProximaDb>
- `GetMetricsSvc`<T: ProximaDb>

**Enums:**
- `ResultPayload`
  - CompactResults(super::SearchResultsCompact)
  - AvroResults(::prost::alloc::vec::Vec<u8>)
- `DistanceMetric`
  - Unspecified = 0
  - Cosine = 1
  - Euclidean = 2
  - DotProduct = 3
  - Hamming = 4
- `StorageEngine`
  - Unspecified = 0
  - Viper = 1
  - Lsm = 2
  - Mmap = 3
  - Hybrid = 4
- `IndexingAlgorithm`
  - Unspecified = 0
  - Hnsw = 1
  - Ivf = 2
  - Pq = 3
  - Flat = 4
  - Annoy = 5
- `CollectionOperation`
  - Unspecified = 0
  - CollectionCreate = 1
  - CollectionUpdate = 2
  - CollectionGet = 3
  - CollectionList = 4
  - CollectionDelete = 5
  - CollectionMigrate = 6
- `VectorOperation`
  - Unspecified = 0
  - VectorInsert = 1
  - VectorUpsert = 2
  - VectorUpdate = 3
  - VectorDelete = 4
  - VectorSearch = 5
- `MutationType`
  - Unspecified = 0
  - MutationUpdate = 1
  - MutationDelete = 2
- `FilterableDataType`
  - Unspecified = 0
  - FilterableString = 1
  - FilterableInteger = 2
  - FilterableFloat = 3
  - FilterableBoolean = 4
  - FilterableDatetime = 5
  - FilterableArrayString = 6
  - FilterableArrayInteger = 7
  - FilterableArrayFloat = 8

**Traits:**
- `ProximaDb`: Send + Sync + 'static
  - fn collection_operation(&self,
            request: tonic::Request<super::CollectionRequest>,) -> std::result::Result<
            tonic::Response<super::CollectionResponse>,
            tonic::Status,
        >
  - fn vector_insert(&self,
            request: tonic::Request<super::VectorInsertRequest>,) -> std::result::Result<
            tonic::Response<super::VectorOperationResponse>,
            tonic::Status,
        >
  - fn vector_mutation(&self,
            request: tonic::Request<super::VectorMutationRequest>,) -> std::result::Result<
            tonic::Response<super::VectorOperationResponse>,
            tonic::Status,
        >
  - fn vector_search(&self,
            request: tonic::Request<super::VectorSearchRequest>,) -> std::result::Result<
            tonic::Response<super::VectorOperationResponse>,
            tonic::Status,
        >
  - fn health(&self,
            request: tonic::Request<super::HealthRequest>,) -> std::result::Result<tonic::Response<super::HealthResponse>, tonic::Status>
  - fn get_metrics(&self,
            request: tonic::Request<super::MetricsRequest>,) -> std::result::Result<tonic::Response<super::MetricsResponse>, tonic::Status>

### Core Domain

#### Module: core

##### core::config

**Structs:**
- `Config`
  - pub server: ServerConfig
  - pub storage: StorageConfig
  - pub consensus: ConsensusConfig
  - pub api: ApiConfig
  - pub monitoring: MonitoringConfig
  - pub tls: Option<TlsConfig>
- `TlsConfig`
  - pub cert_file: Option<String>
  - pub key_file: Option<String>
  - pub enabled: bool
  - pub bind_interface: Option<String>
- `ServerConfig`
  - pub node_id: String
  - pub bind_address: String
  - pub port: u16
  - pub data_dir: PathBuf
- `StorageConfig`
  - pub data_dirs: Vec<PathBuf>
  - pub wal_dir: PathBuf
  - pub storage_layout: crate::core::storage_layout::StorageLayoutConfig
  - pub mmap_enabled: bool
  - pub lsm_config: LsmConfig
  - pub cache_size_mb: u64
  - pub bloom_filter_bits: u32
  - pub filesystem_config: FilesystemConfig
  - pub metadata_backend: Option<MetadataBackendConfig>
- `MetadataBackendConfig`
  - pub backend_type: String
  - pub storage_url: String
  - pub cloud_config: Option<CloudStorageConfig>
  - pub cache_size_mb: Option<u64>
  - pub flush_interval_secs: Option<u64>
- `CloudStorageConfig`
  - pub s3_config: Option<S3Config>
  - pub azure_config: Option<AzureConfig>
  - pub gcs_config: Option<GcsConfig>
- `S3Config`
  - pub region: String
  - pub bucket: String
  - pub access_key_id: Option<String>
  - pub secret_access_key: Option<String>
  - pub use_iam_role: bool
  - pub endpoint: Option<String>
- `AzureConfig`
  - pub account_name: String
  - pub container: String
  - pub access_key: Option<String>
  - pub sas_token: Option<String>
  - pub use_managed_identity: bool
- `GcsConfig`
  - pub project_id: String
  - pub bucket: String
  - pub service_account_path: Option<String>
  - pub use_workload_identity: bool
- `FilesystemConfig`
  - pub enable_write_strategy_cache: bool
  - pub temp_strategy: TempStrategy
  - pub atomic_config: AtomicOperationsConfig
- `AtomicOperationsConfig`
  - pub enable_local_atomic: bool
  - pub enable_object_store_atomic: bool
  - pub cleanup_temp_on_startup: bool
- `LsmConfig`
  - pub memtable_size_mb: u64
  - pub level_count: u8
  - pub compaction_threshold: u32
  - pub block_size_kb: u32
- `ConsensusConfig`
  - pub node_id: Option<u64>
  - pub cluster_peers: Vec<String>
  - pub election_timeout_ms: u64
  - pub heartbeat_interval_ms: u64
  - pub snapshot_threshold: u64
- `ApiConfig`
  - pub grpc_port: u16
  - pub rest_port: u16
  - pub max_request_size_mb: u64
  - pub timeout_seconds: u64
  - pub enable_tls: Option<bool>
- `MonitoringConfig`
  - pub metrics_enabled: bool
  - pub log_level: String

**Enums:**
- `TempStrategy`
  - SameDirectory
  - ConfiguredTemp{ temp_dir: String

##### core::error

**Enums:**
- `VectorDBError`
- `StorageError`
- `ConsensusError`
- `NetworkError`
- `QueryError`
- `SchemaError`

##### core::global_coordination

**Structs:**
- `GlobalCoordinationConfig`
  - pub deployment_topology: DeploymentTopology
  - pub consistency_model: ConsistencyModel
  - pub replication_strategy: ReplicationStrategy
  - pub conflict_resolution: ConflictResolution
  - pub disaster_recovery: DisasterRecoveryConfig
  - pub metadata_sharding: MetadataShardingConfig
- `RegionConfig`
  - pub region_id: String
  - pub availability_zones: Vec<String>
  - pub capacity_weight: f32
  - pub latency_zone: LatencyZone
  - pub data_residency_constraints: Vec<String>
  - pub disaster_recovery_tier: DRTier
- `EdgeLocationConfig`
  - pub location_id: String
  - pub parent_region: String
  - pub caching_enabled: bool
  - pub read_only: bool
  - pub cache_ttl_seconds: u64
- `DisasterRecoveryConfig`
  - pub enabled: bool
  - pub rto_seconds: u32
  - pub rpo_seconds: u32
  - pub backup_strategy: BackupStrategy
  - pub failover_automation: FailoverAutomation
  - pub testing_schedule: DRTestingSchedule
- `FailoverAutomation`
  - pub enabled: bool
  - pub health_check_interval_seconds: u32
  - pub failure_threshold_count: u32
  - pub automatic_failback: bool
  - pub notification_channels: Vec<String>
- `DRTestingSchedule`
  - pub enabled: bool
  - pub frequency_days: u32
  - pub test_type: DRTestType
  - pub rollback_testing: bool
- `MetadataShardingConfig`
  - pub sharding_strategy: MetadataShardingStrategy
  - pub shard_count: u32
  - pub rebalancing: ShardRebalancingConfig
  - pub hot_shard_detection: HotShardConfig
- `TenantRange`
  - pub start: String
  - pub end: String
  - pub shard_id: u32
- `ShardRebalancingConfig`
  - pub enabled: bool
  - pub trigger_threshold: f32
  - pub rebalancing_strategy: RebalancingStrategy
  - pub migration_rate_limit: u32
  - pub maintenance_window: Option<MaintenanceWindow>
- `MaintenanceWindow`
  - pub timezone: String
  - pub start_hour: u8
  - pub duration_hours: u8
  - pub days_of_week: Vec<u8>
- `HotShardConfig`
  - pub detection_enabled: bool
  - pub cpu_threshold: f32
  - pub memory_threshold: f32
  - pub request_rate_threshold: f32
  - pub mitigation_strategies: Vec<HotShardMitigation>
- `GlobalLoadBalancingConfig`
  - pub strategy: GlobalLBStrategy
  - pub health_checks: GlobalHealthCheckConfig
  - pub traffic_distribution: TrafficDistributionConfig
- `GlobalHealthCheckConfig`
  - pub interval_seconds: u32
  - pub timeout_seconds: u32
  - pub failure_threshold: u32
  - pub success_threshold: u32
  - pub health_check_endpoints: Vec<String>
- `TrafficDistributionConfig`
  - pub default_weights: HashMap<String
  - pub tenant_overrides: HashMap<String
  - pub canary_traffic_percent: f32
  - pub circuit_breaker_enabled: bool
- `DataLocalityConfig`
  - pub residency_rules: HashMap<String
  - pub transfer_restrictions: Vec<TransferRestriction>
  - pub sovereignty_compliance: HashMap<String
- `DataResidencyRule`
  - pub tenant_pattern: String
  - pub allowed_regions: Vec<String>
  - pub prohibited_regions: Vec<String>
  - pub encryption_required: bool
  - pub audit_logging: bool
- `TransferRestriction`
  - pub from_region: String
  - pub to_region: String
  - pub allowed: bool
  - pub encryption_required: bool
  - pub approval_required: bool
- `IntelligentRoutingConfig`
  - pub ml_enabled: bool
  - pub learning_period_days: u32
  - pub routing_features: Vec<RoutingFeature>
  - pub feedback_loop_enabled: bool
  - pub model_update_frequency_hours: u32
- `GlobalMetadataCoordinator`
  - private config: GlobalCoordinationConfig
  - private region_states: HashMap<String
  - private global_metadata_cache: GlobalMetadataCache
  - private coordination_protocol: CoordinationProtocol
- `RegionState`
  - pub region_id: String
  - pub status: RegionStatus
  - pub last_heartbeat: DateTime<Utc>
  - pub lag_seconds: u32
  - pub pending_operations: u32
  - pub health_score: f32
- `GlobalMetadataCache`
  - private tenant_metadata: HashMap<String
  - private collection_metadata: HashMap<String
  - private schema_cache: HashMap<String
  - private routing_table: RoutingTable
- `TenantMetadata`
  - pub tenant_id: String
  - pub account_tier: String
  - pub home_region: String
  - pub allowed_regions: Vec<String>
  - pub resource_quotas: HashMap<String
  - pub data_residency_requirements: Vec<String>
  - pub created_at: DateTime<Utc>
  - pub last_updated: DateTime<Utc>
  - pub version: u64
- `CollectionMetadata`
  - pub collection_id: String
  - pub tenant_id: String
  - pub schema_id: String
  - pub primary_region: String
  - pub replica_regions: Vec<String>
  - pub sharding_strategy: String
  - pub size_bytes: u64
  - pub vector_count: u64
  - pub last_accessed: DateTime<Utc>
  - pub access_pattern: AccessPattern
- `SchemaMetadata`
  - pub schema_id: String
  - pub schema_type: String
  - pub version: u32
  - pub fields: HashMap<String
  - pub indexes: Vec<IndexMetadata>
  - pub created_at: DateTime<Utc>
- `FieldMetadata`
  - pub field_name: String
  - pub field_type: String
  - pub nullable: bool
  - pub indexed: bool
  - pub compression: Option<String>
- `IndexMetadata`
  - pub index_name: String
  - pub index_type: String
  - pub fields: Vec<String>
  - pub unique: bool
  - pub partial: bool
- `RoutingTable`
  - pub tenant_routes: HashMap<String
  - pub collection_routes: HashMap<String
  - pub region_topology: RegionTopology
  - pub last_updated: DateTime<Utc>
- `TenantRoute`
  - pub tenant_id: String
  - pub primary_region: String
  - pub replica_regions: Vec<String>
  - pub load_balancing_weights: HashMap<String
  - pub preferred_az: Option<String>
- `CollectionRoute`
  - pub collection_id: String
  - pub shard_locations: HashMap<u32
  - pub read_preferences: ReadPreference
  - pub write_preference: WritePreference
- `RegionTopology`
  - pub regions: HashMap<String
  - pub latency_matrix: HashMap<(String
  - pub bandwidth_matrix: HashMap<(String
- `RegionInfo`
  - pub region_id: String
  - pub availability_zones: Vec<String>
  - pub coordinates: Option<(f32
  - pub tier: RegionTier
  - pub capacity: RegionCapacity
- `RegionCapacity`
  - pub max_tenants: u32
  - pub max_collections: u32
  - pub max_vectors: u64
  - pub current_utilization: f32

**Enums:**
- `DeploymentTopology`
  - SingleRegion{
  - region: String
  - availability_zones: Vec<String>
  - cross_az_replication: bool
- `LatencyZone`
  - UltraLow, // < 10ms
  - Low,      // < 50ms
  - Medium,   // < 100ms
  - High,     // < 500ms
  - Best,     // Best effort
- `DRTier`
  - Primary, // Main serving region
  - Hot,     // Hot standby (real-time sync)
  - Warm,    // Warm standby (near real-time sync)
  - Cold,    // Cold backup (periodic sync)
  - Archive, // Long-term archive
- `ConsistencyModel`
  - Strong
  - EventualBounded{ max_staleness_seconds: u64
- `ReplicationStrategy`
  - Synchronous{ min_replicas: u32, timeout_ms: u32
- `ConflictResolution`
  - LastWriterWins
  - VectorClock
  - ApplicationDefined{ handler: String
- `CRDTType`
  - GCounter,    // Grow-only counter
  - PNCounter,   // Increment/decrement counter
  - GSet,        // Grow-only set
  - ORSet,       // Observed-remove set
  - LWWRegister, // Last-writer-wins register
- `BackupStrategy`
  - Continuous{ retention_days: u32
- `DRTestType`
  - NonDisruptive, // Test without affecting production
  - Failover,      // Actual failover test
  - FullDR,        // Complete disaster recovery simulation
- `MetadataShardingStrategy`
  - HashByTenant
  - RangeByTenant{ ranges: Vec<TenantRange>
- `RebalancingStrategy`
  - Immediate
  - Scheduled{
  - schedule: String
- `HotShardMitigation`
  - ScaleOut{ max_replicas: u32
- `GlobalLBStrategy`
  - LatencyBased
  - CapacityBased
  - CostOptimized
  - Weighted{
  - latency_weight: f32
  - capacity_weight: f32
  - cost_weight: f32
- `ComplianceFramework`
  - GDPR,   // European Union
  - CCPA,   // California
  - PIPEDA, // Canada
  - LGPD,   // Brazil
  - Custom{
  - name: String
  - requirements: Vec<String>
- `RoutingFeature`
  - Latency
  - Throughput
  - ErrorRate
  - Cost
  - UserLocation
  - TimeOfDay
  - DayOfWeek
  - WorkloadType
  - TenantTier
- `FailoverStrategy`
  - Manual
  - Automatic{
  - health_check_failures: u32
  - failover_timeout_seconds: u32
- `RegionStatus`
  - Healthy
  - Degraded
  - Unhealthy
  - Maintenance
  - Recovering
- `AccessPattern`
  - Hot,     // Frequently accessed
  - Warm,    // Occasionally accessed
  - Cold,    // Rarely accessed
  - Archive, // Long-term storage
- `ReadPreference`
  - Primary
  - PrimaryPreferred
  - Secondary
  - SecondaryPreferred
  - Nearest
- `WritePreference`
  - Primary
  - Majority
  - Acknowledged
- `RegionTier`
  - Tier1, // Primary regions with full capabilities
  - Tier2, // Secondary regions with most capabilities
  - Edge,  // Edge locations with caching only
- `CoordinationProtocol`
  - Raft{
  - leader_region: String
  - follower_regions: Vec<String>

##### core::routing

**Structs:**
- `RoutingConfig`
  - pub strategy: RoutingStrategy
  - pub load_balancing: LoadBalancingStrategy
  - pub tenant_routing: TenantRoutingConfig
  - pub geographic_routing: Option<GeographicRoutingConfig>
  - pub circuit_breaker: CircuitBreakerConfig
- `SizeThreshold`
  - pub max_vectors: u64
  - pub target_clusters: Vec<String>
  - pub scaling_policy: String
- `TenantRoutingConfig`
  - pub tenant_extraction: TenantExtractionConfig
  - pub tenant_mapping: TenantMappingStrategy
  - pub isolation_tiers: HashMap<String
  - pub rate_limiting: HashMap<String
- `TenantExtractionConfig`
  - pub headers: Vec<String>
  - pub jwt_claims: Vec<String>
  - pub url_patterns: Vec<String>
  - pub default_tenant: String
- `IsolationTier`
  - pub name: String
  - pub resource_limits: ResourceLimits
  - pub scaling_policy: ScalingPolicy
  - pub storage_class: StorageClass
  - pub sla_guarantees: SlaGuarantees
- `ResourceLimits`
  - pub max_cpu_cores: Option<f32>
  - pub max_memory_gb: Option<f32>
  - pub max_storage_gb: Option<f32>
  - pub max_requests_per_second: Option<f32>
  - pub max_concurrent_queries: Option<u32>
- `ScalingPolicy`
  - pub auto_scaling_enabled: bool
  - pub min_instances: u32
  - pub max_instances: u32
  - pub scale_up_threshold: f32
  - pub scale_down_threshold: f32
  - pub cooldown_seconds: u32
- `SlaGuarantees`
  - pub availability_percent: f32
  - pub response_time_p99_ms: u32
  - pub throughput_qps: u32
  - pub data_durability_nines: u8
- `RateLimitConfig`
  - pub requests_per_second: f32
  - pub burst_capacity: u32
  - pub queue_timeout_ms: u32
  - pub backoff_strategy: BackoffStrategy
- `GeographicRoutingConfig`
  - pub enabled: bool
  - pub regions: HashMap<String
  - pub latency_routing: bool
  - pub data_residency_rules: HashMap<String
- `RegionConfig`
  - pub clusters: Vec<String>
  - pub primary: bool
  - pub latency_weight: f32
  - pub capacity_weight: f32
- `CircuitBreakerConfig`
  - pub enabled: bool
  - pub failure_threshold: u32
  - pub recovery_timeout_seconds: u32
  - pub half_open_max_calls: u32
- `RoutingContext`
  - pub tenant_id: String
  - pub customer_segment: CustomerSegment
  - pub account_tier: AccountTier
  - pub geographic_region: Option<String>
  - pub workload_type: WorkloadType
  - pub collection_metadata: Option<CollectionMetadata>
  - pub request_metadata: HashMap<String
- `CollectionMetadata`
  - pub size: u64
  - pub query_patterns: Vec<QueryPattern>
  - pub access_frequency: AccessFrequency
  - pub data_sensitivity: DataSensitivity
- `SmartRouter`
  - private config: RoutingConfig
  - private tenant_cache: HashMap<String
  - private cluster_health: HashMap<String
- `ClusterHealth`
  - pub cluster_id: String
  - pub cpu_utilization: f32
  - pub memory_utilization: f32
  - pub active_connections: u32
  - pub queue_depth: u32
  - pub response_time_p99: u32
  - pub error_rate: f32
  - pub last_updated: chrono::DateTime<chrono::Utc>
- `RoutingDecision`
  - pub target_cluster: String
  - pub target_instance: Option<String>
  - pub load_balancing_key: String
  - pub priority: RoutingPriority
  - pub fallback_clusters: Vec<String>
  - pub reason: String

**Enums:**
- `RoutingStrategy`
  - TenantBased{
  - tenant_key: String
  - shard_count: u32
  - consistent_hashing: bool
- `LoadBalancingStrategy`
  - RoundRobin
  - LeastConnections
  - WeightedRoundRobin{
  - weights: HashMap<String, u32>
- `TenantMappingStrategy`
  - Shared
  - Dedicated{
  - enterprise_tenants: HashMap<String, String>, // tenant_id -> cluster_id
- `StorageClass`
  - Premium{ iops: u32, throughput_mbps: u32
- `BackoffStrategy`
  - Exponential{ base_ms: u32, max_ms: u32
- `CustomerSegment`
  - Startup
  - SMB, // Small/Medium Business
  - Enterprise
  - Government
  - NonProfit
- `AccountTier`
  - Free
  - Starter
  - Professional
  - Enterprise
  - Custom
- `WorkloadType`
  - OLTP
  - OLAP
  - ML
  - Ingestion
  - Hybrid
- `QueryPattern`
  - PointQuery,       // Get specific vectors
  - SimilaritySearch, // Vector similarity queries
  - RangeQuery,       // Filtered searches
  - Aggregation,      // Analytics queries
- `AccessFrequency`
  - Hot,     // Accessed frequently
  - Warm,    // Accessed occasionally
  - Cold,    // Rarely accessed
  - Archive, // Long-term storage
- `DataSensitivity`
  - Public
  - Internal
  - Confidential
  - Restricted
- `RoutingPriority`
  - High,   // Enterprise SLA
  - Medium, // Professional tier
  - Low,    // Best effort
  - Batch,  // Background processing

##### core::serverless

**Structs:**
- `ServerlessConfig`
  - pub deployment_mode: DeploymentMode
  - pub cloud_provider: CloudProvider
  - pub auto_scaling: AutoScalingConfig
  - pub multi_tenant: MultiTenantConfig
  - pub observability: ObservabilityConfig
- `AutoScalingConfig`
  - pub enabled: bool
  - pub scale_to_zero: bool
  - pub scale_up_threshold: f32
  - pub scale_down_threshold: f32
  - pub scale_up_delay_seconds: u32
  - pub scale_down_delay_seconds: u32
  - pub metrics: Vec<ScalingMetric>
- `MultiTenantConfig`
  - pub enabled: bool
  - pub isolation_level: IsolationLevel
  - pub namespace_header: String
  - pub default_namespace: String
  - pub resource_limits: HashMap<String
- `ResourceLimit`
  - pub max_vectors_per_collection: Option<u64>
  - pub max_collections: Option<u32>
  - pub max_requests_per_second: Option<f32>
  - pub max_storage_gb: Option<f32>
- `ObservabilityConfig`
  - pub metrics_enabled: bool
  - pub tracing_enabled: bool
  - pub logging_level: String
  - pub metrics_endpoint: String
  - pub traces_endpoint: Option<String>
  - pub health_check_path: String
  - pub readiness_check_path: String
- `QueueMessage`
  - pub id: String
  - pub body: Vec<u8>
  - pub receipt_handle: String
  - pub attributes: HashMap<String

**Enums:**
- `DeploymentMode`
  - Functions{
  - cold_start_optimization: bool
  - memory_size_mb: u32
  - timeout_seconds: u32
- `CloudProvider`
  - AWS{
  - region: String
  - s3_bucket: String
  - dynamodb_table: Option<String>
  - rds_endpoint: Option<String>
  - elasticache_endpoint: Option<String>
- `ScalingMetric`
  - RequestsPerSecond{ target: f32
- `IsolationLevel`
  - Logical
  - Container
  - Cluster

**Traits:**
- `CloudStorage`: Send + Sync
  - fn get(&self, key: &str) -> crate::Result<Option<Vec<u8>>>
  - fn put(&self, key: &str, data: &[u8]) -> crate::Result<()>
  - fn delete(&self, key: &str) -> crate::Result<()>
  - fn list(&self, prefix: &str) -> crate::Result<Vec<String>>
- `CloudCache`: Send + Sync
  - fn get(&self, key: &str) -> crate::Result<Option<Vec<u8>>>
  - fn set(&self, key: &str, data: &[u8], ttl_seconds: Option<u64>) -> crate::Result<()>
  - fn delete(&self, key: &str) -> crate::Result<()>
  - fn clear(&self) -> crate::Result<()>
- `CloudQueue`: Send + Sync
  - fn send(&self, queue_name: &str, message: &[u8]) -> crate::Result<String>
  - fn receive(&self, queue_name: &str) -> crate::Result<Option<QueueMessage>>
  - fn delete(&self, queue_name: &str, receipt_handle: &str) -> crate::Result<()>

##### core::storage_layout

**Structs:**
- `StorageLayoutConfig`
  - pub base_paths: Vec<StorageBasePath>
  - private default: 1)
  - pub node_instance: u32
  - pub assignment_strategy: CollectionAssignmentStrategy
  - pub temp_config: TempDirectoryConfig
- `StorageBasePath`
  - pub base_dir: PathBuf
  - pub instance_id: u32
  - pub mount_point: Option<String>
  - pub disk_type: DiskType
  - pub capacity_config: CapacityConfig
- `CapacityConfig`
  - pub max_wal_size_mb: Option<u64>
  - pub max_storage_size_mb: Option<u64>
  - pub metadata_reserved_mb: u64
  - pub warning_threshold_percent: f64
- `TempDirectoryConfig`
  - pub use_same_directory: bool
  - pub temp_suffix: String
  - private Default: "___temp"
  - pub compaction_suffix: String
  - private Default: "___compaction"
  - pub flush_suffix: String
  - private Default: "___flushed"
  - pub cleanup_on_startup: bool
- `StoragePathResolver`
  - private config: StorageLayoutConfig
  - private assignment_cache: HashMap<String
- `CollectionPaths`
  - pub collection_uuid: String
  - pub wal_path: PathBuf
  - pub storage_path: PathBuf
  - pub metadata_path: PathBuf
  - pub temp_paths: Vec<PathBuf>

**Enums:**
- `DiskType`
  - NvmeSsd{ max_iops: u64
- `CollectionAssignmentStrategy`
  - RoundRobin
  - HashBased
  - PerformanceBased
  - Manual{
  - assignments: HashMap<String, u32>, // collection_uuid -> instance_id
- `StorageType`
  - Wal
  - Storage
  - Metadata
- `TempOperationType`
  - General
  - Compaction
  - Flush
- `StorageLayoutError`

##### core::types

**Structs:**
- `VectorRecord`
  - pub id: VectorId
  - pub collection_id: CollectionId
  - pub vector: Vector
  - pub metadata: HashMap<String
  - private serde_json: :Value>
  - pub timestamp: chrono::DateTime<chrono::Utc>
  - private None: Record never expires (active indefinitely)
  - pub expires_at: Option<chrono::DateTime<chrono::Utc>>
- `SearchQuery`
  - pub collection_id: CollectionId
  - pub vector: Vector
  - pub k: usize
  - pub filters: Option<HashMap<String
  - private serde_json: :Value>>
  - pub threshold: Option<f32>
- `BatchSearchRequest`
  - pub collection_id: CollectionId
  - pub query_vector: Vector
  - pub k: usize
  - pub filter: Option<HashMap<String
  - private serde_json: :Value>>
- `Collection`
  - pub id: CollectionId
  - pub name: String
  - pub dimension: usize
  - pub schema_type: SchemaType
  - pub created_at: chrono::DateTime<chrono::Utc>
  - pub updated_at: chrono::DateTime<chrono::Utc>
- `DocumentSchema`
  - pub fields: HashMap<String
- `RelationalSchema`
  - pub tables: HashMap<String
  - pub relationships: Vec<Relationship>
- `TableSchema`
  - pub columns: HashMap<String
  - pub primary_key: Vec<String>
  - pub indexes: Vec<Index>
- `Relationship`
  - pub from_table: String
  - pub from_column: String
  - pub to_table: String
  - pub to_column: String
  - pub relationship_type: RelationshipType
- `Index`
  - pub name: String
  - pub columns: Vec<String>
  - pub index_type: IndexType

**Enums:**
- `SchemaType`
  - Document
  - Relational
- `FieldType`
  - String
  - Integer
  - Float
  - Boolean
  - Array(Box<FieldType>)
  - Object(HashMap<String, FieldType>)
- `ColumnType`
  - Text
  - Integer
  - Float
  - Boolean
  - Timestamp
  - Vector(usize), // dimension
- `RelationshipType`
  - OneToOne
  - OneToMany
  - ManyToMany
- `IndexType`
  - BTree
  - Hash
  - Vector

##### core::unified_types

**Structs:**
- `CompressionConfig`
  - pub algorithm: CompressionAlgorithm
  - pub level: u8
  - pub compress_vectors: bool
  - pub compress_metadata: bool
- `SearchResult`
  - pub id: String
  - pub score: f32
  - pub vector: Option<Vec<f32>>
  - pub metadata: HashMap<String
  - private serde_json: :Value>
  - pub distance: Option<f32>
  - pub index_path: Option<String>
  - pub collection_id: Option<String>
  - pub created_at: Option<DateTime<Utc>>
- `CompactionConfig`
  - pub enabled: bool
  - pub strategy: CompactionStrategy
  - pub trigger_threshold: f32
  - pub max_parallelism: usize
  - pub target_file_size: u64
  - pub min_files_to_compact: usize
  - pub max_files_to_compact: usize

**Enums:**
- `CompressionAlgorithm`
  - None
  - Lz4
  - Lz4Hc
  - Zstd{ level: i32
- `IndexType`
  - Vector(VectorIndexType)
  - Metadata(MetadataIndexType)
  - Hybrid{
  - vector_index: Box<VectorIndexType>
  - metadata_index: Box<MetadataIndexType>
- `VectorIndexType`
  - Flat
  - Hnsw{
  - m: u32
  - ef_construction: u32
  - ef_search: u32
- `MetadataIndexType`
  - BTree
  - Hash
  - BloomFilter
  - FullText
  - Inverted
- `CompactionStrategy`
  - SizeTiered
  - Leveled
  - Universal
  - Adaptive
- `DistanceMetric`
  - Cosine
  - Euclidean
  - Manhattan
  - DotProduct
  - Hamming
- `StorageEngine`
  - Viper
  - Lsm
  - Mmap
  - Hybrid

### Services Domain

#### Module: services

##### services::collection_service

**Structs:**
- `CollectionService`
  - private metadata_backend: Arc<FilestoreMetadataBackend>
- `CollectionServiceResponse`
  - pub success: bool
  - pub collection_uuid: Option<String>
  - pub storage_path: Option<String>
  - pub error_message: Option<String>
  - pub error_code: Option<String>
  - pub processing_time_us: i64
- `CollectionServiceBuilder`
  - private metadata_backend: Option<Arc<FilestoreMetadataBackend>>

##### services::migration

**Structs:**
- `MigrationService`
  - private metadata_store: Arc<MetadataStore>
- `StrategyMigrationRequest`
  - pub collection_id: CollectionId
  - pub storage_engine: Option<StorageEngineType>
  - pub indexing_algorithm: Option<IndexingAlgorithm>
  - pub distance_metric: Option<DistanceMetric>
  - pub indexing_parameters: Option<std::collections::HashMap<String
  - private serde_json: :Value>>
  - pub storage_parameters: Option<std::collections::HashMap<String
  - private serde_json: :Value>>
  - pub search_parameters: Option<std::collections::HashMap<String
  - private serde_json: :Value>>
  - pub initiated_by: Option<String>
  - pub reason: Option<String>
- `StrategyMigrationResponse`
  - pub success: bool
  - pub change_id: String
  - pub previous_strategy: CollectionStrategyConfig
  - pub new_strategy: CollectionStrategyConfig
  - pub change_type: StrategyChangeType
  - pub changed_at: chrono::DateTime<Utc>
  - pub error_message: Option<String>
- `StrategyTemplates`

##### services::storage_path_service

**Structs:**
- `StoragePathService`
  - private resolver: Arc<RwLock<StoragePathResolver>>
  - private path_cache: Arc<RwLock<HashMap<String
  - private config: StorageLayoutConfig
- `CollectionStorageStats`
  - pub collection_uuid: String
  - pub wal_size_bytes: u64
  - pub storage_size_bytes: u64
  - pub total_size_bytes: u64
  - pub wal_file_count: usize
  - pub storage_file_count: usize

##### services::unified_avro_service

**Structs:**
- `UnifiedAvroService`
  - private storage: Arc<RwLock<StorageEngine>>
  - private wal: Arc<WalManager>
  - private vector_coordinator: Arc<VectorStorageCoordinator>
  - private performance_metrics: Arc<RwLock<ServiceMetrics>>
  - private wal_strategy_type: WalStrategyType
  - private avro_schema_version: u32
- `UnifiedServiceConfig`
  - pub wal_strategy: WalStrategyType
  - pub memtable_type: crate::storage::wal::config::MemTableType
  - pub avro_schema_version: u32
  - pub enable_schema_evolution: bool
- `ServiceMetrics`
  - pub total_operations: u64
  - pub successful_operations: u64
  - pub failed_operations: u64
  - pub avg_processing_time_us: f64
  - pub last_operation_time: Option<chrono::DateTime<chrono::Utc>>

### Compute Domain

#### Module: compute

##### compute::algorithms

**Structs:**
- `SearchResult`
  - pub vector_id: String
  - pub score: f32
  - pub metadata: Option<HashMap<String
  - private serde_json: :Value>>
- `MemoryUsage`
  - pub index_size_bytes: usize
  - pub vector_data_bytes: usize
  - pub metadata_bytes: usize
  - pub total_bytes: usize
- `HNSWIndex`
  - private m: usize
  - private ef_construction: usize
  - private ef: usize
  - private max_layers: usize
  - private ml: f32
  - private distance_computer: Box<dyn DistanceCompute>
  - private layers: layer -> node_id -> connections
  - private layers: Vec<HashMap<usize
  - private storage: node_id -> (vector
  - private vectors: HashMap<usize
  - private serde_json: :Value>>)>
  - private mapping: external_id -> internal_node_id
  - private id_mapping: HashMap<String
  - private mapping: internal_node_id -> external_id
  - private reverse_mapping: HashMap<usize
  - private next_node_id: usize
  - private entry_point: Option<usize>
  - private rng_state: Arc<RwLock<u64>>
- `BruteForceIndex`
  - private distance_computer: Box<dyn DistanceCompute>
  - private vectors: HashMap<String
  - private serde_json: :Value>>)>
- `OrderedFloat`

**Traits:**
- `VectorSearchAlgorithm`: Send + Sync
  - fn add_vector(&mut self,
        id: String,
        vector: Vec<f32>,
        metadata: Option<HashMap<String, serde_json::Value>>,) -> Result<(), String>
  - fn add_vectors(&mut self,
        vectors: Vec<(String, Vec<f32>, Option<HashMap<String, serde_json::Value>>)
  - fn search(&self, query: &[f32], k: usize) -> Result<Vec<SearchResult>, String>
  - fn search_with_filter(&self,
        query: &[f32],
        k: usize,
        filter: &dyn Fn(&HashMap<String, serde_json::Value>) -> bool,
    ) -> Result<Vec<SearchResult>, String>
  - fn remove_vector(&mut self, id: &str) -> Result<bool, String>
  - fn size(&self) -> usize
  - fn memory_usage(&self) -> MemoryUsage
  - fn optimize(&mut self) -> Result<(), String>

##### compute::distance

**Structs:**
- `CosineScalar`
- `EuclideanScalar`
- `DotProductScalar`
- `GenericScalar`
  - private metric: DistanceMetric
- `CosineAvx2`
- `CosineAvx`
- `CosineSse2`
- `EuclideanAvx2`
- `EuclideanAvx`
- `EuclideanSse2`
- `DotProductAvx2`
- `DotProductAvx`
- `DotProductSse2`
- `CosineNeon`
- `EuclideanNeon`
- `DotProductNeon`

**Enums:**
- `DistanceMetric`
  - Cosine
  - Euclidean
  - Manhattan
  - DotProduct
  - Hamming
  - Jaccard
  - Custom(String)
- `PlatformCapability`
  - Scalar
  - X86Sse2
  - X86Avx
  - X86Avx2
  - ArmNeon
  - ArmSve
- `SimdLevel`
  - Scalar
  - None
  - Sse
  - Sse4
  - Avx
  - Avx2
  - Neon

**Traits:**
- `DistanceCompute`: Send + Sync
  - fn distance(&self, a: &[f32], b: &[f32]) -> f32
  - fn distance_batch(&self, query: &[f32], vectors: &[&[f32]]) -> Vec<f32>
  - fn is_similarity(&self) -> bool
  - fn metric(&self) -> DistanceMetric

##### compute::distance_backup

**Structs:**
- `CosineDistance`
  - private simd_level: SimdLevel
- `EuclideanDistance`
  - private simd_level: SimdLevel
- `DotProductDistance`
  - private simd_level: SimdLevel
- `ManhattanDistance`
  - private simd_level: SimdLevel

**Enums:**
- `DistanceMetric`
  - Cosine
  - Euclidean
  - Manhattan
  - DotProduct
  - Hamming
  - Jaccard
  - Custom(String)

**Traits:**
- `DistanceCompute`: Send + Sync
  - fn distance(&self, a: &[f32], b: &[f32]) -> f32
  - fn distance_batch(&self, query: &[f32], vectors: &[&[f32]]) -> Vec<f32>
  - fn distance_matrix(&self, set_a: &[&[f32]], set_b: &[&[f32]]) -> Vec<Vec<f32>>
  - fn is_similarity(&self) -> bool
  - fn metric(&self) -> DistanceMetric

##### compute::distance_optimized

**Structs:**
- `ScalarCalculator`
- `Avx2Calculator`
- `AvxCalculator`
- `Sse41Calculator`
- `Sse2Calculator`
- `NeonCalculator`

**Enums:**
- `SimdCapability`
  - Scalar,     // Always available fallback
  - Sse2
  - Sse41
  - Avx
  - Avx2
  - Neon
  - Sve
- `GpuCapability`
  - None
  - Cuda{ compute_capability: (u32, u32)
- `OpenClPlatform`
  - Intel
  - Amd
  - Nvidia
  - Apple
  - Other
- `MetalFamily`
  - Apple7
  - Apple8
  - M1
  - M2
  - M3

**Traits:**
- `DistanceCalculator`: Send + Sync
  - fn cosine_distance(&self, a: &[f32], b: &[f32]) -> f32
  - fn euclidean_distance(&self, a: &[f32], b: &[f32]) -> f32
  - fn dot_product(&self, a: &[f32], b: &[f32]) -> f32

##### compute::hardware

**Structs:**
- `HardwareInfo`
  - pub backend: ComputeBackend
  - pub device_name: String
  - pub memory_total: u64
  - pub memory_free: u64
  - pub compute_capability: Option<String>
  - pub max_threads_per_block: Option<u32>
  - pub multiprocessor_count: Option<u32>
- `RocmAccelerator`
  - private device_id: u32
  - private initialized: bool
- `CpuAccelerator`
  - private thread_count: usize
  - private use_simd: bool

**Traits:**
- `HardwareAccelerator`: Send + Sync
  - fn initialize(&mut self) -> Result<(), String>
  - fn is_available(&self) -> bool
  - fn get_info(&self) -> HardwareInfo
  - fn batch_dot_product(&self,
        queries: &[Vec<f32>],
        vectors: &[Vec<f32>],) -> Result<Vec<Vec<f32>>, String>
  - fn batch_cosine_similarity(&self,
        queries: &[Vec<f32>],
        vectors: &[Vec<f32>],) -> Result<Vec<Vec<f32>>, String>
  - fn batch_euclidean_distance(&self,
        queries: &[Vec<f32>],
        vectors: &[Vec<f32>],) -> Result<Vec<Vec<f32>>, String>
  - fn matrix_multiply(&self,
        a: &[Vec<f32>],
        b: &[Vec<f32>],) -> Result<Vec<Vec<f32>>, String>
  - fn normalize_vectors(&self, vectors: &[Vec<f32>]) -> Result<Vec<Vec<f32>>, String>

##### compute::hardware_detection

**Structs:**
- `HardwareCapabilities`
  - pub cpu: CpuCapabilities
  - pub gpu: GpuCapabilities
  - pub memory: MemoryInfo
  - pub optimal_paths: OptimizationPaths
- `CpuCapabilities`
  - pub vendor: String
  - pub model_name: String
  - pub cores: u32
  - pub threads: u32
  - pub base_frequency_mhz: u32
  - pub max_frequency_mhz: u32
  - pub has_sse: bool
  - pub has_sse2: bool
  - pub has_sse3: bool
  - pub has_ssse3: bool
  - pub has_sse4_1: bool
  - pub has_sse4_2: bool
  - pub has_avx: bool
  - pub has_avx2: bool
  - pub has_avx512f: bool
  - pub has_avx512bw: bool
  - pub has_avx512vl: bool
  - pub has_fma: bool
  - pub has_aes: bool
  - pub has_pclmulqdq: bool
  - pub has_bmi1: bool
  - pub has_bmi2: bool
  - pub has_popcnt: bool
  - pub has_lzcnt: bool
  - pub is_x86_64: bool
  - pub is_aarch64: bool
  - pub l1_cache_size_kb: Option<u32>
  - pub l2_cache_size_kb: Option<u32>
  - pub l3_cache_size_kb: Option<u32>
- `GpuCapabilities`
  - pub devices: Vec<GpuDevice>
  - pub has_cuda: bool
  - pub has_opencl: bool
  - pub has_rocm: bool
  - pub cuda_version: Option<String>
  - pub preferred_device: Option<usize>
- `GpuDevice`
  - pub id: u32
  - pub name: String
  - pub compute_capability: Option<(u32
  - pub memory_total_mb: u64
  - pub memory_free_mb: u64
  - pub multiprocessor_count: u32
  - pub max_threads_per_block: u32
  - pub is_cuda: bool
  - pub is_opencl: bool
- `MemoryInfo`
  - pub total_gb: f64
  - pub available_gb: f64
  - pub page_size_kb: u32
  - pub numa_nodes: u32
- `OptimizationPaths`
  - pub simd_level: SimdLevel
  - pub preferred_backend: ComputeBackend
  - pub batch_sizes: BatchSizeConfig
  - pub memory_strategy: MemoryStrategy
- `BatchSizeConfig`
  - pub vector_operations: usize
  - pub similarity_search: usize
  - pub bulk_insert: usize
  - pub index_build: usize

**Enums:**
- `SimdLevel`
  - None
  - Sse
  - Sse4
  - Avx
  - Avx2
  - Avx512
  - Neon
- `ComputeBackend`
  - Cpu{ simd_level: SimdLevel
- `MemoryStrategy`
  - Conservative
  - Balanced
  - Aggressive

##### compute::mod

**Structs:**
- `ComputeConfig`
  - pub acceleration: AccelerationConfig
  - pub algorithms: AlgorithmConfig
  - pub memory: MemoryConfig
  - pub performance: PerformanceConfig
- `AccelerationConfig`
  - pub backend_priority: Vec<ComputeBackend>
  - pub cpu_vectorization: CpuVectorization
  - pub gpu: GpuConfig
  - pub math_library: MathLibrary
- `CpuVectorization`
  - pub avx512: bool
  - pub avx2: bool
  - pub sse42: bool
  - pub neon: bool
  - pub auto_detect: bool
- `GpuConfig`
  - pub memory_pool: GpuMemoryPool
  - pub batch_size: usize
  - pub unified_memory: bool
  - pub memory_limit_gb: Option<f32>
- `AlgorithmConfig`
  - pub default_metric: DistanceMetric
  - pub index_algorithm: IndexAlgorithm
  - pub search_params: SearchParams
  - pub quantization: QuantizationConfig
- `SearchParams`
  - pub accuracy_target: f32
  - pub max_search_time_ms: u32
  - pub early_termination_threshold: Option<f32>
  - pub parallel_threads: Option<usize>
- `MemoryConfig`
  - pub prefetch_strategy: PrefetchStrategy
  - pub mmap_config: MmapConfig
  - pub cache_config: CacheConfig
- `MmapConfig`
  - pub madvise: MadviseHint
  - pub populate: bool
  - pub huge_pages: bool
  - pub numa_node: Option<u32>
- `CacheConfig`
  - pub l1_cache_size: usize
  - pub l2_cache_size: usize
  - pub replacement_policy: CachePolicy
- `PerformanceConfig`
  - pub simd_enabled: bool
  - pub loop_unrolling: bool
  - pub branch_prediction: bool
  - pub memory_prefault: bool
  - pub thread_affinity: ThreadAffinity
- `HardwareInfo`
  - pub cpu_features: CpuFeatures
  - pub gpu_devices: Vec<GpuDevice>
  - pub memory_info: MemoryInfo
  - pub numa_topology: NumaTopology
- `CpuFeatures`
  - pub avx512_support: bool
  - pub avx2_support: bool
  - pub sse42_support: bool
  - pub neon_support: bool
  - pub core_count: usize
  - pub thread_count: usize
  - pub cache_sizes: CacheSizes
- `CacheSizes`
  - pub l1_data: usize
  - pub l1_instruction: usize
  - pub l2: usize
  - pub l3: usize
- `GpuDevice`
  - pub device_id: u32
  - pub name: String
  - pub compute_capability: String
  - pub memory_total: u64
  - pub memory_free: u64
  - pub backend: GpuBackend
- `MemoryInfo`
  - pub total_memory: u64
  - pub available_memory: u64
  - pub page_size: usize
  - pub huge_page_size: Option<usize>
- `NumaTopology`
  - pub node_count: usize
  - pub nodes: Vec<NumaNode>
- `NumaNode`
  - pub node_id: u32
  - pub cpu_cores: Vec<usize>
  - pub memory_total: u64
  - pub memory_free: u64

**Enums:**
- `ComputeBackend`
  - CUDA{ device_id: Option<u32>
- `GpuMemoryPool`
  - Simple
  - Pooled{ pool_size_gb: f32
- `MathLibrary`
  - IntelMKL
  - OpenBLAS
  - BLIS
  - Native
  - Auto
- `IndexAlgorithm`
  - HNSW{
  - m: usize,               // Number of bi-directional links
  - ef_construction: usize, // Size of candidate set
  - max_elements: usize,    // Maximum number of elements
- `PrefetchStrategy`
  - None
  - Sequential{ distance: usize
- `MadviseHint`
  - Normal
  - Random
  - Sequential
  - WillNeed
  - DontNeed
- `CachePolicy`
  - LRU,  // Least Recently Used
  - LFU,  // Least Frequently Used
  - ARC,  // Adaptive Replacement Cache
  - TwoQ, // Two Queue
- `ThreadAffinity`
  - None
  - Cores(Vec<usize>)
  - NumaNode(u32)
  - Auto
- `GpuBackend`
  - CUDA{ version: String

##### compute::quantization

**Structs:**
- `QuantizationConfig`
  - pub quantization_type: QuantizationType
  - pub num_subquantizers: usize
  - pub num_centroids: usize
  - pub bits_per_code: usize
  - pub training_iterations: usize
  - pub enable_fast_distance: bool
  - pub compression_ratio: f32
- `QuantizationEngine`
  - private config: QuantizationConfig
  - private quantizer: Box<dyn VectorQuantizer + Send + Sync>
- `QuantizedVector`
  - pub codes: Vec<u8>
  - pub norm: f32
  - pub metadata: HashMap<String
- `ProductQuantizer`
  - private config: QuantizationConfig
  - private codebooks: Vec<Vec<Vec<f32>>>
  - private subvector_dim: usize
  - private trained: bool
- `ScalarQuantizer`
  - private config: QuantizationConfig
  - private min_values: Vec<f32>
  - private max_values: Vec<f32>
  - private scale_factors: Vec<f32>
  - private trained: bool
- `BinaryQuantizer`
  - private config: QuantizationConfig
  - private trained: bool
- `AdditiveQuantizer`
  - private config: QuantizationConfig
  - private trained: bool
- `ResidualVectorQuantizer`
  - private config: QuantizationConfig
  - private trained: bool
- `OptimizedProductQuantizer`
  - private config: QuantizationConfig
  - private base_quantizer: ProductQuantizer
  - private trained: bool

**Enums:**
- `QuantizationType`
  - None
  - ProductQuantization
  - ScalarQuantization
  - BinaryQuantization
  - AdditiveQuantization
  - ResidualVectorQuantization
  - OptimizedProductQuantization

**Traits:**
- `VectorQuantizer`
  - fn train(&mut self, vectors: &[Vec<f32>]) -> Result<()>
  - fn quantize(&self, vectors: &[Vec<f32>]) -> Result<Vec<QuantizedVector>>
  - fn dequantize(&self, codes: &[QuantizedVector]) -> Result<Vec<Vec<f32>>>
  - fn compute_distances(&self, query: &[f32], codes: &[QuantizedVector]) -> Result<Vec<f32>>
  - fn memory_per_vector(&self) -> usize
  - fn is_trained(&self) -> bool

### Schema Domain

#### Module: schema

##### schema::collection_avro

**Structs:**
- `CollectionRecord`
  - pub uuid: String
  - pub name: String
  - pub display_name: String
  - pub dimension: i32
  - pub distance_metric: DistanceMetric
  - pub indexing_algorithm: IndexingAlgorithm
  - pub storage_engine: StorageEngine
  - pub created_at: i64
  - pub updated_at: i64
  - pub version: i64
  - pub vector_count: i64
  - pub total_size_bytes: i64
  - pub config: String
  - pub description: Option<String>
  - pub tags: Vec<String>
  - pub owner: Option<String>
  - pub access_pattern: AccessPattern
  - pub filterable_fields: Vec<String>
  - pub indexing_config: String
  - pub retention_policy: Option<String>
  - pub schema_version: i32
- `CollectionAvroSchema`

**Enums:**
- `DistanceMetric`
  - Cosine
  - Euclidean
  - DotProduct
  - Hamming
- `IndexingAlgorithm`
  - Hnsw
  - Ivf
  - Pq
  - Flat
  - Annoy
- `StorageEngine`
  - Viper
  - Lsm
  - Mmap
  - Hybrid
- `AccessPattern`
  - Hot
  - Normal
  - Cold
  - Archive
- `CollectionOperation`
  - Insert(CollectionRecord)
  - Update(CollectionRecord)
  - Delete(String), // Collection name

### Query Domain

#### Module: query

##### query::mod

**Structs:**
- `QueryEngine`
  - private TODO: Add vector index

### Monitoring Domain

#### Module: monitoring

##### monitoring::dashboard::mod

**Structs:**
- `DashboardState`
  - private metrics_collector: Arc<MetricsCollector>
- `MetricsQuery`
  - private since: Option<String>
  - private format: Option<String>
- `HealthResponse`
  - pub status: String
  - pub timestamp: chrono::DateTime<chrono::Utc>
  - pub version: String
  - pub uptime_seconds: f64

##### monitoring::metrics::collector

**Structs:**
- `MetricsCollector`
  - private config: MetricsConfig
  - private system: Arc<RwLock<System>>
  - private current_metrics: Arc<RwLock<SystemMetrics>>
  - private metrics_history: Arc<RwLock<Vec<SystemMetrics>>>
  - private active_alerts: Arc<RwLock<Vec<Alert>>>
  - private rate_limiter: MetricsRateLimiter
  - private start_time: Instant
  - private event_sender: mpsc::UnboundedSender<MetricsEvent>
  - private shutdown: Arc<RwLock<bool>>
- `MetricsSummary`
  - pub timestamp: DateTime<Utc>
  - pub system_health: f64
  - pub cpu_usage: f64
  - pub memory_usage_percent: f64
  - pub disk_usage_percent: f64
  - pub query_latency_p99: f64
  - pub queries_per_second: f64
  - pub cache_hit_rate: f64
  - pub active_alerts_count: usize
  - pub critical_alerts_count: usize

**Enums:**
- `MetricsEvent`
  - SystemMetricsUpdated(SystemMetrics)
  - AlertGenerated(Alert)
  - AlertResolved(String), // Alert ID
  - MetricsCleanup

**Traits:**
- `MetricsProvider`
  - fn collect_metrics(&self) -> Result<HashMap<String, f64>>

##### monitoring::metrics::exporters

**Structs:**
- `PrometheusExporter`
- `JsonExporter`
- `OpenTelemetryExporter`
- `MetricsFormatter`

**Traits:**
- `MetricsExporter`
  - fn export_metrics(&self, metrics: &[Metric]) -> Result<String>
  - fn export_system_metrics(&self, system_metrics: &SystemMetrics) -> Result<String>
  - fn content_type(&self) -> &'static str

##### monitoring::metrics::index_metrics

**Structs:**
- `IndexMetricsCollector`
  - private registry: Arc<MetricsRegistry>
  - private total_indexes_gauge: Arc<Gauge>
  - private index_memory_usage_gauge: Arc<Gauge>
  - private index_build_operations_counter: Arc<Counter>
  - private index_rebuild_operations_counter: Arc<Counter>
  - private search_operations_counter: Arc<Counter>
  - private search_latency_histogram: Arc<HistogramMetric>
  - private vector_insertions_counter: Arc<Counter>
  - private vector_deletions_counter: Arc<Counter>

##### monitoring::metrics::mod

**Structs:**
- `MetricValue`
  - pub value: f64
  - pub timestamp: DateTime<Utc>
  - pub labels: HashMap<String
- `Metric`
  - pub name: String
  - pub help: String
  - pub metric_type: MetricType
  - pub values: Vec<MetricValue>
- `HistogramBucket`
  - pub upper_bound: f64
  - pub count: u64
- `Histogram`
  - pub sum: f64
  - pub count: u64
  - pub buckets: Vec<HistogramBucket>
- `Summary`
  - pub sum: f64
  - pub count: u64
  - pub quantiles: Vec<(f64
- `SystemMetrics`
  - pub timestamp: DateTime<Utc>
  - pub server: ServerMetrics
  - pub storage: StorageMetrics
  - pub index: IndexMetrics
  - pub query: QueryMetrics
  - pub custom: HashMap<String
- `ServerMetrics`
  - pub uptime_seconds: f64
  - pub cpu_usage_percent: f64
  - pub memory_usage_bytes: u64
  - pub memory_available_bytes: u64
  - pub disk_usage_bytes: u64
  - pub disk_available_bytes: u64
  - pub network_bytes_in: u64
  - pub network_bytes_out: u64
  - pub open_connections: u32
  - pub active_requests: u32
- `StorageMetrics`
  - pub total_vectors: u64
  - pub total_collections: u32
  - pub storage_size_bytes: u64
  - pub wal_size_bytes: u64
  - pub memtable_size_bytes: u64
  - pub compaction_operations: u64
  - pub flush_operations: u64
  - pub read_operations_per_second: f64
  - pub write_operations_per_second: f64
  - pub cache_hit_rate: f64
- `IndexMetrics`
  - pub total_indexes: u32
  - pub index_memory_usage_bytes: u64
  - pub index_build_operations: u64
  - pub index_rebuild_operations: u64
  - pub index_optimization_operations: u64
  - pub search_operations_per_second: f64
  - pub average_search_latency_ms: f64
  - pub p99_search_latency_ms: f64
  - pub vector_insertions_per_second: f64
  - pub vector_deletions_per_second: f64
- `QueryMetrics`
  - pub total_queries: u64
  - pub successful_queries: u64
  - pub failed_queries: u64
  - pub average_query_latency_ms: f64
  - pub p50_query_latency_ms: f64
  - pub p90_query_latency_ms: f64
  - pub p99_query_latency_ms: f64
  - pub queries_per_second: f64
  - pub slow_queries: u64
  - pub timeout_queries: u64
- `AlertThresholds`
  - pub max_cpu_usage_percent: f64
  - pub max_memory_usage_percent: f64
  - pub max_disk_usage_percent: f64
  - pub max_query_latency_ms: f64
  - pub min_cache_hit_rate: f64
  - pub max_error_rate: f64
- `Alert`
  - pub id: String
  - pub level: AlertLevel
  - pub message: String
  - pub metric_name: String
  - pub current_value: f64
  - pub threshold_value: f64
  - pub timestamp: DateTime<Utc>
  - pub acknowledged: bool
- `MetricsConfig`
  - pub collection_interval_seconds: u64
  - pub retention_hours: u64
  - pub enable_prometheus: bool
  - pub prometheus_port: u16
  - pub enable_detailed_logging: bool
  - pub alert_thresholds: AlertThresholds
  - pub histogram_buckets: Vec<f64>
- `MetricsRateLimiter`
  - private last_emit: Arc<RwLock<Instant>>
  - private min_interval: Duration

**Enums:**
- `MetricType`
  - Counter
  - Gauge
  - Histogram
  - Summary
- `AlertLevel`
  - Info
  - Warning
  - Critical

##### monitoring::metrics::query_metrics

**Structs:**
- `QueryMetricsCollector`
  - private registry: Arc<MetricsRegistry>
  - private total_queries_counter: Arc<Counter>
  - private successful_queries_counter: Arc<Counter>
  - private failed_queries_counter: Arc<Counter>
  - private query_latency_histogram: Arc<HistogramMetric>
  - private queries_per_second_gauge: Arc<Gauge>
  - private slow_queries_counter: Arc<Counter>
  - private timeout_queries_counter: Arc<Counter>

##### monitoring::metrics::registry

**Structs:**
- `MetricsRegistry`
  - private metrics: Arc<RwLock<HashMap<String
  - private collectors: Arc<RwLock<HashMap<String
  - private histogram_configs: Arc<RwLock<HashMap<String
- `HistogramConfig`
  - pub buckets: Vec<f64>
  - pub labels: Vec<String>
- `Counter`
  - private name: String
  - private help: String
  - private value: Arc<RwLock<f64>>
  - private labels: HashMap<String
- `Gauge`
  - private name: String
  - private help: String
  - private value: Arc<RwLock<f64>>
  - private labels: HashMap<String
- `HistogramMetric`
  - private name: String
  - private help: String
  - private config: HistogramConfig
  - private buckets: Arc<RwLock<Vec<Arc<RwLock<u64>>>>>
  - private sum: Arc<RwLock<f64>>
  - private count: Arc<RwLock<u64>>
  - private labels: HashMap<String
- `SummaryMetric`
  - private name: String
  - private help: String
  - private sum: Arc<RwLock<f64>>
  - private count: Arc<RwLock<u64>>
  - private quantiles: Arc<RwLock<Vec<f64>>>
  - private labels: HashMap<String

**Traits:**
- `MetricCollectorFn`: Send + Sync
  - fn collect(&self) -> Result<Vec<MetricValue>>

##### monitoring::metrics::server_metrics

**Structs:**
- `ServerMetricsCollector`
  - private registry: Arc<MetricsRegistry>
  - private start_time: Instant
  - private uptime_counter: Arc<Counter>
  - private cpu_usage_gauge: Arc<Gauge>
  - private memory_usage_gauge: Arc<Gauge>
  - private memory_available_gauge: Arc<Gauge>
  - private disk_usage_gauge: Arc<Gauge>
  - private disk_available_gauge: Arc<Gauge>
  - private network_bytes_in_counter: Arc<Counter>
  - private network_bytes_out_counter: Arc<Counter>
  - private open_connections_gauge: Arc<Gauge>
  - private active_requests_gauge: Arc<Gauge>
  - private connection_count: Arc<RwLock<u32>>
  - private request_count: Arc<RwLock<u32>>

##### monitoring::metrics::storage_metrics

**Structs:**
- `StorageMetricsCollector`
  - private registry: Arc<MetricsRegistry>
  - private total_vectors_gauge: Arc<Gauge>
  - private total_collections_gauge: Arc<Gauge>
  - private storage_size_gauge: Arc<Gauge>
  - private wal_size_gauge: Arc<Gauge>
  - private memtable_size_gauge: Arc<Gauge>
  - private compaction_operations_counter: Arc<Counter>
  - private flush_operations_counter: Arc<Counter>
  - private read_operations_counter: Arc<Counter>
  - private write_operations_counter: Arc<Counter>
  - private cache_hit_counter: Arc<Counter>
  - private cache_miss_counter: Arc<Counter>

## Consolidation Opportunities

### Duplicate Structs to Consolidate
- **Config Types**: Multiple `*Config` structs could inherit from common base
- **Stats/Metrics**: Various `*Stats` and `*Metrics` structs share similar fields
- **Result Types**: Multiple `*Result` structs with similar success/error patterns
- **Metadata Types**: Several `*Metadata` structs that could be unified

### Common Patterns for Base Traits
- **Storage Engines**: Common interface for VIPER, WAL, and other storage backends
- **Indexing Strategies**: Unified trait for HNSW, IVF, and other index types
- **Compaction**: Common interface for various compaction strategies
- **Serialization**: Unified approach for Avro, Bincode, and JSON serialization

