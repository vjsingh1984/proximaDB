from proximadb.v1 import vector_types_pb2 as _vector_types_pb2
from google.protobuf.internal import containers as _containers
from google.protobuf.internal import enum_type_wrapper as _enum_type_wrapper
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from collections.abc import Iterable as _Iterable, Mapping as _Mapping
from typing import ClassVar as _ClassVar, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class CollectionOperation(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    COLLECTION_OPERATION_UNSPECIFIED: _ClassVar[CollectionOperation]
    COLLECTION_CREATE: _ClassVar[CollectionOperation]
    COLLECTION_UPDATE: _ClassVar[CollectionOperation]
    COLLECTION_GET: _ClassVar[CollectionOperation]
    COLLECTION_LIST: _ClassVar[CollectionOperation]
    COLLECTION_DELETE: _ClassVar[CollectionOperation]
    COLLECTION_MIGRATE: _ClassVar[CollectionOperation]
    COLLECTION_GET_ID_BY_NAME: _ClassVar[CollectionOperation]
COLLECTION_OPERATION_UNSPECIFIED: CollectionOperation
COLLECTION_CREATE: CollectionOperation
COLLECTION_UPDATE: CollectionOperation
COLLECTION_GET: CollectionOperation
COLLECTION_LIST: CollectionOperation
COLLECTION_DELETE: CollectionOperation
COLLECTION_MIGRATE: CollectionOperation
COLLECTION_GET_ID_BY_NAME: CollectionOperation

class CollectionConfig(_message.Message):
    __slots__ = ("name", "dimension", "distance_metric", "storage_engine", "tags", "description", "filterable_columns", "index_configs", "quantization", "storage_config", "primary_index", "auto_index_selection", "owner", "embedding_models")
    NAME_FIELD_NUMBER: _ClassVar[int]
    DIMENSION_FIELD_NUMBER: _ClassVar[int]
    DISTANCE_METRIC_FIELD_NUMBER: _ClassVar[int]
    STORAGE_ENGINE_FIELD_NUMBER: _ClassVar[int]
    TAGS_FIELD_NUMBER: _ClassVar[int]
    DESCRIPTION_FIELD_NUMBER: _ClassVar[int]
    FILTERABLE_COLUMNS_FIELD_NUMBER: _ClassVar[int]
    INDEX_CONFIGS_FIELD_NUMBER: _ClassVar[int]
    QUANTIZATION_FIELD_NUMBER: _ClassVar[int]
    STORAGE_CONFIG_FIELD_NUMBER: _ClassVar[int]
    PRIMARY_INDEX_FIELD_NUMBER: _ClassVar[int]
    AUTO_INDEX_SELECTION_FIELD_NUMBER: _ClassVar[int]
    OWNER_FIELD_NUMBER: _ClassVar[int]
    EMBEDDING_MODELS_FIELD_NUMBER: _ClassVar[int]
    name: str
    dimension: int
    distance_metric: _vector_types_pb2.DistanceMetric
    storage_engine: _vector_types_pb2.StorageEngine
    tags: _containers.RepeatedScalarFieldContainer[str]
    description: str
    filterable_columns: _containers.RepeatedCompositeFieldContainer[_vector_types_pb2.FilterableColumnSpec]
    index_configs: _containers.RepeatedCompositeFieldContainer[IndexConfig]
    quantization: _vector_types_pb2.QuantizationConfig
    storage_config: StorageConfig
    primary_index: str
    auto_index_selection: bool
    owner: str
    embedding_models: _containers.RepeatedScalarFieldContainer[str]
    def __init__(self, name: _Optional[str] = ..., dimension: _Optional[int] = ..., distance_metric: _Optional[_Union[_vector_types_pb2.DistanceMetric, str]] = ..., storage_engine: _Optional[_Union[_vector_types_pb2.StorageEngine, str]] = ..., tags: _Optional[_Iterable[str]] = ..., description: _Optional[str] = ..., filterable_columns: _Optional[_Iterable[_Union[_vector_types_pb2.FilterableColumnSpec, _Mapping]]] = ..., index_configs: _Optional[_Iterable[_Union[IndexConfig, _Mapping]]] = ..., quantization: _Optional[_Union[_vector_types_pb2.QuantizationConfig, _Mapping]] = ..., storage_config: _Optional[_Union[StorageConfig, _Mapping]] = ..., primary_index: _Optional[str] = ..., auto_index_selection: bool = ..., owner: _Optional[str] = ..., embedding_models: _Optional[_Iterable[str]] = ...) -> None: ...

class CollectionStats(_message.Message):
    __slots__ = ("vector_count", "index_size_bytes", "data_size_bytes")
    VECTOR_COUNT_FIELD_NUMBER: _ClassVar[int]
    INDEX_SIZE_BYTES_FIELD_NUMBER: _ClassVar[int]
    DATA_SIZE_BYTES_FIELD_NUMBER: _ClassVar[int]
    vector_count: int
    index_size_bytes: int
    data_size_bytes: int
    def __init__(self, vector_count: _Optional[int] = ..., index_size_bytes: _Optional[int] = ..., data_size_bytes: _Optional[int] = ...) -> None: ...

class Collection(_message.Message):
    __slots__ = ("id", "config", "stats", "created_at", "updated_at", "storage_assignment")
    ID_FIELD_NUMBER: _ClassVar[int]
    CONFIG_FIELD_NUMBER: _ClassVar[int]
    STATS_FIELD_NUMBER: _ClassVar[int]
    CREATED_AT_FIELD_NUMBER: _ClassVar[int]
    UPDATED_AT_FIELD_NUMBER: _ClassVar[int]
    STORAGE_ASSIGNMENT_FIELD_NUMBER: _ClassVar[int]
    id: str
    config: CollectionConfig
    stats: CollectionStats
    created_at: int
    updated_at: int
    storage_assignment: StorageAssignment
    def __init__(self, id: _Optional[str] = ..., config: _Optional[_Union[CollectionConfig, _Mapping]] = ..., stats: _Optional[_Union[CollectionStats, _Mapping]] = ..., created_at: _Optional[int] = ..., updated_at: _Optional[int] = ..., storage_assignment: _Optional[_Union[StorageAssignment, _Mapping]] = ...) -> None: ...

class IndexConfig(_message.Message):
    __slots__ = ("index_name", "algorithm", "parameters", "enabled", "update_mode", "async_update_timeout_ms", "async_update_batch_size", "enable_background_optimization", "hnsw_config", "ivf_config", "lsh_config", "flat_config", "pq_config", "annoy_config", "build_concurrency", "memory_limit_mb", "checkpoint_interval_ms", "is_primary", "use_cases", "selectivity_threshold", "use_quantization", "quantization_override", "queue_representation")
    class ParametersEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    INDEX_NAME_FIELD_NUMBER: _ClassVar[int]
    ALGORITHM_FIELD_NUMBER: _ClassVar[int]
    PARAMETERS_FIELD_NUMBER: _ClassVar[int]
    ENABLED_FIELD_NUMBER: _ClassVar[int]
    UPDATE_MODE_FIELD_NUMBER: _ClassVar[int]
    ASYNC_UPDATE_TIMEOUT_MS_FIELD_NUMBER: _ClassVar[int]
    ASYNC_UPDATE_BATCH_SIZE_FIELD_NUMBER: _ClassVar[int]
    ENABLE_BACKGROUND_OPTIMIZATION_FIELD_NUMBER: _ClassVar[int]
    HNSW_CONFIG_FIELD_NUMBER: _ClassVar[int]
    IVF_CONFIG_FIELD_NUMBER: _ClassVar[int]
    LSH_CONFIG_FIELD_NUMBER: _ClassVar[int]
    FLAT_CONFIG_FIELD_NUMBER: _ClassVar[int]
    PQ_CONFIG_FIELD_NUMBER: _ClassVar[int]
    ANNOY_CONFIG_FIELD_NUMBER: _ClassVar[int]
    BUILD_CONCURRENCY_FIELD_NUMBER: _ClassVar[int]
    MEMORY_LIMIT_MB_FIELD_NUMBER: _ClassVar[int]
    CHECKPOINT_INTERVAL_MS_FIELD_NUMBER: _ClassVar[int]
    IS_PRIMARY_FIELD_NUMBER: _ClassVar[int]
    USE_CASES_FIELD_NUMBER: _ClassVar[int]
    SELECTIVITY_THRESHOLD_FIELD_NUMBER: _ClassVar[int]
    USE_QUANTIZATION_FIELD_NUMBER: _ClassVar[int]
    QUANTIZATION_OVERRIDE_FIELD_NUMBER: _ClassVar[int]
    QUEUE_REPRESENTATION_FIELD_NUMBER: _ClassVar[int]
    index_name: str
    algorithm: _vector_types_pb2.IndexingAlgorithm
    parameters: _containers.ScalarMap[str, str]
    enabled: bool
    update_mode: int
    async_update_timeout_ms: int
    async_update_batch_size: int
    enable_background_optimization: bool
    hnsw_config: _vector_types_pb2.HnswConfig
    ivf_config: _vector_types_pb2.IvfConfig
    lsh_config: _vector_types_pb2.LshConfig
    flat_config: str
    pq_config: str
    annoy_config: str
    build_concurrency: int
    memory_limit_mb: int
    checkpoint_interval_ms: int
    is_primary: bool
    use_cases: _containers.RepeatedScalarFieldContainer[str]
    selectivity_threshold: float
    use_quantization: bool
    quantization_override: _vector_types_pb2.QuantizationConfig
    queue_representation: str
    def __init__(self, index_name: _Optional[str] = ..., algorithm: _Optional[_Union[_vector_types_pb2.IndexingAlgorithm, str]] = ..., parameters: _Optional[_Mapping[str, str]] = ..., enabled: bool = ..., update_mode: _Optional[int] = ..., async_update_timeout_ms: _Optional[int] = ..., async_update_batch_size: _Optional[int] = ..., enable_background_optimization: bool = ..., hnsw_config: _Optional[_Union[_vector_types_pb2.HnswConfig, _Mapping]] = ..., ivf_config: _Optional[_Union[_vector_types_pb2.IvfConfig, _Mapping]] = ..., lsh_config: _Optional[_Union[_vector_types_pb2.LshConfig, _Mapping]] = ..., flat_config: _Optional[str] = ..., pq_config: _Optional[str] = ..., annoy_config: _Optional[str] = ..., build_concurrency: _Optional[int] = ..., memory_limit_mb: _Optional[int] = ..., checkpoint_interval_ms: _Optional[int] = ..., is_primary: bool = ..., use_cases: _Optional[_Iterable[str]] = ..., selectivity_threshold: _Optional[float] = ..., use_quantization: bool = ..., quantization_override: _Optional[_Union[_vector_types_pb2.QuantizationConfig, _Mapping]] = ..., queue_representation: _Optional[str] = ...) -> None: ...

class StorageConfig(_message.Message):
    __slots__ = ("storage_path", "data_paths", "compression", "max_file_size_mb", "enable_caching")
    STORAGE_PATH_FIELD_NUMBER: _ClassVar[int]
    DATA_PATHS_FIELD_NUMBER: _ClassVar[int]
    COMPRESSION_FIELD_NUMBER: _ClassVar[int]
    MAX_FILE_SIZE_MB_FIELD_NUMBER: _ClassVar[int]
    ENABLE_CACHING_FIELD_NUMBER: _ClassVar[int]
    storage_path: str
    data_paths: _containers.RepeatedScalarFieldContainer[str]
    compression: _vector_types_pb2.CompressionAlgorithm
    max_file_size_mb: int
    enable_caching: bool
    def __init__(self, storage_path: _Optional[str] = ..., data_paths: _Optional[_Iterable[str]] = ..., compression: _Optional[_Union[_vector_types_pb2.CompressionAlgorithm, str]] = ..., max_file_size_mb: _Optional[int] = ..., enable_caching: bool = ...) -> None: ...

class StorageAssignment(_message.Message):
    __slots__ = ("primary_path", "backup_paths", "engine", "engine_config", "base_location", "assigned_at")
    class EngineConfigEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    PRIMARY_PATH_FIELD_NUMBER: _ClassVar[int]
    BACKUP_PATHS_FIELD_NUMBER: _ClassVar[int]
    ENGINE_FIELD_NUMBER: _ClassVar[int]
    ENGINE_CONFIG_FIELD_NUMBER: _ClassVar[int]
    BASE_LOCATION_FIELD_NUMBER: _ClassVar[int]
    ASSIGNED_AT_FIELD_NUMBER: _ClassVar[int]
    primary_path: str
    backup_paths: _containers.RepeatedScalarFieldContainer[str]
    engine: _vector_types_pb2.StorageEngine
    engine_config: _containers.ScalarMap[str, str]
    base_location: str
    assigned_at: int
    def __init__(self, primary_path: _Optional[str] = ..., backup_paths: _Optional[_Iterable[str]] = ..., engine: _Optional[_Union[_vector_types_pb2.StorageEngine, str]] = ..., engine_config: _Optional[_Mapping[str, str]] = ..., base_location: _Optional[str] = ..., assigned_at: _Optional[int] = ...) -> None: ...

class GetCollectionRequest(_message.Message):
    __slots__ = ("collection_id",)
    COLLECTION_ID_FIELD_NUMBER: _ClassVar[int]
    collection_id: str
    def __init__(self, collection_id: _Optional[str] = ...) -> None: ...

class ListCollectionsRequest(_message.Message):
    __slots__ = ("limit", "offset", "include_stats")
    LIMIT_FIELD_NUMBER: _ClassVar[int]
    OFFSET_FIELD_NUMBER: _ClassVar[int]
    INCLUDE_STATS_FIELD_NUMBER: _ClassVar[int]
    limit: int
    offset: int
    include_stats: bool
    def __init__(self, limit: _Optional[int] = ..., offset: _Optional[int] = ..., include_stats: bool = ...) -> None: ...

class ListCollectionsResponse(_message.Message):
    __slots__ = ("collections",)
    COLLECTIONS_FIELD_NUMBER: _ClassVar[int]
    collections: _containers.RepeatedCompositeFieldContainer[Collection]
    def __init__(self, collections: _Optional[_Iterable[_Union[Collection, _Mapping]]] = ...) -> None: ...

class DeleteCollectionRequest(_message.Message):
    __slots__ = ("collection_id",)
    COLLECTION_ID_FIELD_NUMBER: _ClassVar[int]
    collection_id: str
    def __init__(self, collection_id: _Optional[str] = ...) -> None: ...

class DeleteCollectionResponse(_message.Message):
    __slots__ = ("success",)
    SUCCESS_FIELD_NUMBER: _ClassVar[int]
    success: bool
    def __init__(self, success: bool = ...) -> None: ...

class CollectionRequest(_message.Message):
    __slots__ = ("operation", "collection_id", "collection_config", "query_params", "options", "migration_config")
    class QueryParamsEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    class OptionsEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: bool
        def __init__(self, key: _Optional[str] = ..., value: bool = ...) -> None: ...
    class MigrationConfigEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    OPERATION_FIELD_NUMBER: _ClassVar[int]
    COLLECTION_ID_FIELD_NUMBER: _ClassVar[int]
    COLLECTION_CONFIG_FIELD_NUMBER: _ClassVar[int]
    QUERY_PARAMS_FIELD_NUMBER: _ClassVar[int]
    OPTIONS_FIELD_NUMBER: _ClassVar[int]
    MIGRATION_CONFIG_FIELD_NUMBER: _ClassVar[int]
    operation: int
    collection_id: str
    collection_config: CollectionConfig
    query_params: _containers.ScalarMap[str, str]
    options: _containers.ScalarMap[str, bool]
    migration_config: _containers.ScalarMap[str, str]
    def __init__(self, operation: _Optional[int] = ..., collection_id: _Optional[str] = ..., collection_config: _Optional[_Union[CollectionConfig, _Mapping]] = ..., query_params: _Optional[_Mapping[str, str]] = ..., options: _Optional[_Mapping[str, bool]] = ..., migration_config: _Optional[_Mapping[str, str]] = ...) -> None: ...

class CollectionResponse(_message.Message):
    __slots__ = ("success", "collection", "collections", "error_message", "error_code", "operation", "affected_count", "total_count", "metadata", "processing_time_us")
    class MetadataEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    SUCCESS_FIELD_NUMBER: _ClassVar[int]
    COLLECTION_FIELD_NUMBER: _ClassVar[int]
    COLLECTIONS_FIELD_NUMBER: _ClassVar[int]
    ERROR_MESSAGE_FIELD_NUMBER: _ClassVar[int]
    ERROR_CODE_FIELD_NUMBER: _ClassVar[int]
    OPERATION_FIELD_NUMBER: _ClassVar[int]
    AFFECTED_COUNT_FIELD_NUMBER: _ClassVar[int]
    TOTAL_COUNT_FIELD_NUMBER: _ClassVar[int]
    METADATA_FIELD_NUMBER: _ClassVar[int]
    PROCESSING_TIME_US_FIELD_NUMBER: _ClassVar[int]
    success: bool
    collection: Collection
    collections: _containers.RepeatedCompositeFieldContainer[Collection]
    error_message: str
    error_code: str
    operation: int
    affected_count: int
    total_count: int
    metadata: _containers.ScalarMap[str, str]
    processing_time_us: int
    def __init__(self, success: bool = ..., collection: _Optional[_Union[Collection, _Mapping]] = ..., collections: _Optional[_Iterable[_Union[Collection, _Mapping]]] = ..., error_message: _Optional[str] = ..., error_code: _Optional[str] = ..., operation: _Optional[int] = ..., affected_count: _Optional[int] = ..., total_count: _Optional[int] = ..., metadata: _Optional[_Mapping[str, str]] = ..., processing_time_us: _Optional[int] = ...) -> None: ...

class CollectionSnapshot(_message.Message):
    __slots__ = ("collection", "vectors", "snapshot_timestamp", "snapshot_version")
    COLLECTION_FIELD_NUMBER: _ClassVar[int]
    VECTORS_FIELD_NUMBER: _ClassVar[int]
    SNAPSHOT_TIMESTAMP_FIELD_NUMBER: _ClassVar[int]
    SNAPSHOT_VERSION_FIELD_NUMBER: _ClassVar[int]
    collection: Collection
    vectors: _containers.RepeatedCompositeFieldContainer[_vector_types_pb2.VectorRecord]
    snapshot_timestamp: int
    snapshot_version: str
    def __init__(self, collection: _Optional[_Union[Collection, _Mapping]] = ..., vectors: _Optional[_Iterable[_Union[_vector_types_pb2.VectorRecord, _Mapping]]] = ..., snapshot_timestamp: _Optional[int] = ..., snapshot_version: _Optional[str] = ...) -> None: ...
