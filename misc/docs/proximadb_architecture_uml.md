# ProximaDB Architecture UML Diagrams

## 1. High-Level Package Diagram (Mermaid)

```mermaid
graph TB
    subgraph "API Layer"
        REST[REST API<br/>port: 5678]
        GRPC[gRPC API<br/>port: 5679]
        UNIFIED[UnifiedServer]
    end
    
    subgraph "Service Layer"
        CS[CollectionService]
        VS[VectorService]
        SS[SearchService]
        MS[MetricsService]
    end
    
    subgraph "Storage Layer"
        VIPER[VIPER Engine<br/>Parquet Storage]
        WAL[WAL System<br/>Avro/Bincode]
        META[Metadata Store]
        FS[FileSystem<br/>S3/Azure/GCS]
    end
    
    subgraph "Index Layer"
        AXIS[AXIS Manager]
        HNSW[HNSW Index]
        IVF[IVF Index]
        FLAT[Flat Index]
    end
    
    REST --> UNIFIED
    GRPC --> UNIFIED
    UNIFIED --> CS
    UNIFIED --> VS
    CS --> VIPER
    CS --> META
    VS --> VIPER
    VS --> SS
    SS --> AXIS
    AXIS --> HNSW
    AXIS --> IVF
    AXIS --> FLAT
    VIPER --> WAL
    VIPER --> FS
    META --> FS
```

## 2. Core Classes Diagram (PlantUML)

```plantuml
@startuml
!theme blueprint

package "Core Types" {
    class VectorRecord {
        +id: String
        +collection_id: String
        +vector: Vec<f32>
        +metadata: HashMap<String, Value>
        +timestamp: DateTime<Utc>
        +expires_at: Option<DateTime<Utc>>
    }
    
    class Collection {
        +id: String
        +name: String
        +dimension: usize
        +distance_metric: DistanceMetric
        +storage_engine: StorageEngine
        +indexing_algorithm: IndexAlgorithm
        +created_at: DateTime<Utc>
        +metadata: HashMap<String, Value>
    }
    
    enum DistanceMetric {
        Cosine
        Euclidean
        Manhattan
        DotProduct
        Hamming
    }
    
    enum StorageEngine {
        Viper
        Lsm
        Mmap
        Hybrid
    }
}

package "Storage VIPER" {
    class ViperCoreEngine {
        -config: ViperCoreConfig
        -filesystem: Arc<FilesystemFactory>
        -schema_strategy: ViperSchemaStrategy
        -record_buffer: Arc<Mutex<Vec<VectorRecord>>>
        -flush_lock: Arc<Mutex<()>>
        -last_flush: Arc<Mutex<Instant>>
        -stats: Arc<RwLock<ViperStats>>
        +insert_vectors(records: Vec<VectorRecord>)
        +search_vectors(query: Vec<f32>, k: usize)
        +flush_to_parquet()
        +compact_parquet_files()
    }
    
    class ViperSchemaStrategy {
        -filterable_fields: Vec<String>
        -enable_ttl: bool
        -enable_extra_meta: bool
        -dimension: usize
        +create_arrow_schema()
        +create_parquet_schema()
        +adapt_record_to_batch()
    }
}

package "WAL System" {
    abstract class WalStrategy {
        +append_entries(entries: Vec<WalEntry>)
        +read_entries(offset: u64)
        +compact()
        +get_stats()
    }
    
    class AvroStrategy {
        -schema: Schema
        -disk_manager: Arc<DiskManager>
        -memtable: Arc<dyn MemTable>
        +serialize_entries(entries: &[WalEntry])
        +deserialize_entries(data: &[u8])
    }
    
    class BincodeStrategy {
        -config: BincodeConfig
        -disk_manager: Arc<DiskManager>
        -memtable: Arc<dyn MemTable>
        +compress_entries(entries: &[WalEntry])
    }
    
    WalStrategy <|-- AvroStrategy
    WalStrategy <|-- BincodeStrategy
}

package "Unified Types" {
    class CompressionAlgorithm <<enumeration>> {
        None
        Lz4
        Lz4Hc
        Zstd{level: i32}
        Snappy
        Gzip
        Deflate
    }
    
    class SearchResult {
        +id: String
        +score: f32
        +vector: Option<Vec<f32>>
        +metadata: HashMap<String, Value>
        +distance: Option<f32>
        +index_path: Option<String>
        +collection_id: Option<String>
        +created_at: Option<DateTime<Utc>>
    }
    
    class CompactionConfig {
        +enabled: bool
        +strategy: CompactionStrategy
        +trigger_threshold: f32
        +max_parallelism: usize
        +target_file_size: u64
        +min_files_to_compact: usize
        +max_files_to_compact: usize
    }
}

ViperCoreEngine --> VectorRecord
ViperCoreEngine --> ViperSchemaStrategy
Collection --> StorageEngine
Collection --> DistanceMetric
SearchResult --> VectorRecord

@enduml
```

## 3. Service Architecture (Mermaid)

```mermaid
classDiagram
    class UnifiedServer {
        +rest_port: u16
        +grpc_port: u16
        +bind_address: SocketAddr
        +services: ServiceRegistry
        +start() Result
        +shutdown() Result
    }
    
    class CollectionService {
        -storage_engine: Arc<StorageEngine>
        -metadata_store: Arc<MetadataStore>
        -index_manager: Arc<AxisIndexManager>
        +create_collection(request) Result
        +get_collection(id) Result
        +list_collections() Result
        +delete_collection(id) Result
    }
    
    class VectorService {
        -storage_engine: Arc<StorageEngine>
        -search_engine: Arc<SearchEngine>
        +insert_vectors(collection_id, vectors) Result
        +get_vectors(collection_id, ids) Result
        +search_vectors(collection_id, query) Result
        +delete_vectors(collection_id, ids) Result
    }
    
    class StorageEngine {
        <<interface>>
        +insert(collection_id, records) Result
        +get(collection_id, ids) Result
        +delete(collection_id, ids) Result
        +flush() Result
    }
    
    class ViperStorageEngine {
        -engine: Arc<ViperCoreEngine>
        -wal: Arc<WalManager>
        +insert(collection_id, records) Result
        +search(query) Result
        +compact() Result
    }
    
    UnifiedServer --> CollectionService
    UnifiedServer --> VectorService
    CollectionService --> StorageEngine
    VectorService --> StorageEngine
    StorageEngine <|.. ViperStorageEngine
```

## 4. Index Architecture (PlantUML)

```plantuml
@startuml
!theme blueprint

package "AXIS Index System" {
    class AxisIndexManager {
        -indexes: HashMap<CollectionId, Box<dyn Index>>
        -analyzer: Arc<CollectionAnalyzer>
        -optimizer: Arc<IndexOptimizer>
        -migration_engine: Arc<MigrationEngine>
        +create_index(collection_id, config) Result
        +search(collection_id, query) Result
        +optimize_index(collection_id) Result
        +migrate_index(collection_id, new_config) Result
    }
    
    interface Index {
        +insert(id: String, vector: Vec<f32>)
        +search(query: Vec<f32>, k: usize) Vec<SearchResult>
        +delete(id: String) bool
        +optimize() Result
        +get_stats() IndexStats
    }
    
    class HNSWIndex {
        -graph: HnswGraph
        -config: HnswConfig
        -distance_metric: DistanceMetric
        +build_graph(vectors) Result
        +add_node(id, vector) Result
        +search_knn(query, k, ef) Result
    }
    
    class IVFIndex {
        -centroids: Vec<Vec<f32>>
        -inverted_lists: Vec<Vec<VectorId>>
        -config: IvfConfig
        +train_centroids(vectors) Result
        +assign_to_cluster(vector) usize
        +search_clusters(query, nprobe) Result
    }
    
    class FlatIndex {
        -vectors: Vec<(String, Vec<f32>)>
        -distance_metric: DistanceMetric
        +brute_force_search(query, k) Result
    }
    
    AxisIndexManager --> Index
    Index <|.. HNSWIndex
    Index <|.. IVFIndex
    Index <|.. FlatIndex
}

package "Performance Optimization" {
    class IndexOptimizer {
        -performance_monitor: Arc<PerformanceMonitor>
        -query_analyzer: Arc<QueryAnalyzer>
        +analyze_workload(collection_id) WorkloadPattern
        +recommend_index_type(pattern) IndexType
        +auto_tune_parameters(index) Result
    }
    
    class CollectionAnalyzer {
        -vector_stats: HashMap<CollectionId, VectorStats>
        -query_patterns: HashMap<CollectionId, QueryPattern>
        +analyze_distribution(vectors) Distribution
        +detect_clustering(vectors) ClusterInfo
        +recommend_strategy() IndexStrategy
    }
}

AxisIndexManager --> IndexOptimizer
AxisIndexManager --> CollectionAnalyzer

@enduml
```

## 5. Identified Duplications (Mermaid)

```mermaid
graph LR
    subgraph "Configuration Duplicates"
        CC1[CollectionConfig<br/>schema_types.rs]
        CC2[CollectionConfig<br/>storage/types.rs]
        CC3[CollectionConfiguration<br/>services/]
        CC1 -.->|DUPLICATE| CC2
        CC2 -.->|DUPLICATE| CC3
    end
    
    subgraph "Metadata Duplicates"
        MD1[CollectionMetadata<br/>storage/]
        MD2[VectorMetadata<br/>core/]
        MD3[Metadata<br/>api/]
        MD1 -.->|SIMILAR| MD2
        MD2 -.->|SIMILAR| MD3
    end
    
    subgraph "Stats Duplicates"
        ST1[StorageStats]
        ST2[IndexStats]
        ST3[PerformanceStats]
        ST4[CollectionStats]
        ST1 -.->|OVERLAP| ST2
        ST2 -.->|OVERLAP| ST3
        ST3 -.->|OVERLAP| ST4
    end
    
    subgraph "Result Types"
        R1[SearchResult<br/>compute/]
        R2[VectorSearchResult<br/>storage/]
        R3[QueryResult<br/>api/]
        R1 -.->|DUPLICATE| R2
        R2 -.->|DUPLICATE| R3
    end
```

## 6. Consolidation Proposal (PlantUML)

```plantuml
@startuml
!theme blueprint

package "Proposed Unified Types" {
    abstract class BaseConfig {
        +id: String
        +name: String
        +created_at: DateTime<Utc>
        +updated_at: DateTime<Utc>
        +metadata: HashMap<String, Value>
        +validate() Result
    }
    
    class CollectionConfig extends BaseConfig {
        +dimension: usize
        +distance_metric: DistanceMetric
        +storage_engine: StorageEngine
        +index_config: IndexConfig
        +compression_config: CompressionConfig
    }
    
    abstract class BaseStats {
        +timestamp: DateTime<Utc>
        +period_seconds: u64
        +to_metrics() HashMap<String, f64>
    }
    
    class StorageStats extends BaseStats {
        +total_bytes: u64
        +vector_count: u64
        +compression_ratio: f32
        +write_throughput: f64
        +read_throughput: f64
    }
    
    class IndexStats extends BaseStats {
        +index_size_bytes: u64
        +search_latency_p95: f64
        +build_time_ms: u64
        +optimization_count: u32
    }
    
    interface Result<T> {
        +success: bool
        +data: Option<T>
        +error: Option<Error>
        +metadata: HashMap<String, Value>
    }
    
    class SearchResult implements Result {
        +matches: Vec<VectorMatch>
        +query_time_ms: u64
        +index_used: String
    }
    
    class VectorMatch {
        +id: String
        +score: f32
        +vector: Option<Vec<f32>>
        +metadata: HashMap<String, Value>
    }
}

note right of BaseConfig : Base class for all configurations\nProvides common fields and validation

note right of BaseStats : Base class for all statistics\nProvides metrics conversion

note right of Result : Generic result interface\nUnifies all API responses

@enduml
```