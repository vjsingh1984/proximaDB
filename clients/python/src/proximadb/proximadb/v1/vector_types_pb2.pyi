from proximadb.v1 import types_pb2 as _types_pb2
from proximadb.v1 import entity_pb2 as _entity_pb2
from google.protobuf.internal import containers as _containers
from google.protobuf.internal import enum_type_wrapper as _enum_type_wrapper
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from collections.abc import Iterable as _Iterable, Mapping as _Mapping
from typing import ClassVar as _ClassVar, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class DistanceMetric(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    DISTANCE_METRIC_UNSPECIFIED: _ClassVar[DistanceMetric]
    COSINE: _ClassVar[DistanceMetric]
    EUCLIDEAN: _ClassVar[DistanceMetric]
    DOT_PRODUCT: _ClassVar[DistanceMetric]
    HAMMING: _ClassVar[DistanceMetric]
    MANHATTAN: _ClassVar[DistanceMetric]
    JACCARD: _ClassVar[DistanceMetric]
    ANGULAR: _ClassVar[DistanceMetric]
    CHEBYSHEV: _ClassVar[DistanceMetric]
    CANBERRA: _ClassVar[DistanceMetric]
    MINKOWSKI: _ClassVar[DistanceMetric]
    BRAY_CURTIS: _ClassVar[DistanceMetric]
    HELLINGER: _ClassVar[DistanceMetric]
    CUSTOM: _ClassVar[DistanceMetric]

class StorageEngine(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    STORAGE_ENGINE_UNSPECIFIED: _ClassVar[StorageEngine]
    VIPER: _ClassVar[StorageEngine]
    SST: _ClassVar[StorageEngine]
    NOVA: _ClassVar[StorageEngine]
    HELIX: _ClassVar[StorageEngine]
    SWIFT: _ClassVar[StorageEngine]
    RAPTOR: _ClassVar[StorageEngine]
    MMAP: _ClassVar[StorageEngine]
    HYBRID: _ClassVar[StorageEngine]

class IndexingAlgorithm(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    INDEXING_ALGORITHM_UNSPECIFIED: _ClassVar[IndexingAlgorithm]
    HNSW: _ClassVar[IndexingAlgorithm]
    IVF: _ClassVar[IndexingAlgorithm]
    PQ: _ClassVar[IndexingAlgorithm]
    FLAT: _ClassVar[IndexingAlgorithm]
    ANNOY: _ClassVar[IndexingAlgorithm]
    LSH: _ClassVar[IndexingAlgorithm]

class VectorOperation(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    VECTOR_OPERATION_UNSPECIFIED: _ClassVar[VectorOperation]
    VECTOR_BATCH: _ClassVar[VectorOperation]
    VECTOR_SEARCH: _ClassVar[VectorOperation]
    VECTOR_GET: _ClassVar[VectorOperation]

class VectorServiceOperation(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    VECTOR_SERVICE_OPERATION_UNSPECIFIED: _ClassVar[VectorServiceOperation]
    VS_BATCH: _ClassVar[VectorServiceOperation]
    VS_SEARCH: _ClassVar[VectorServiceOperation]
    VS_GET: _ClassVar[VectorServiceOperation]

class FilterOperation(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    FILTER_OPERATION_UNSPECIFIED: _ClassVar[FilterOperation]
    OP_EQUALS: _ClassVar[FilterOperation]
    OP_NOT_EQUALS: _ClassVar[FilterOperation]
    OP_GREATER_THAN: _ClassVar[FilterOperation]
    OP_GREATER_THAN_OR_EQUAL: _ClassVar[FilterOperation]
    OP_LESS_THAN: _ClassVar[FilterOperation]
    OP_LESS_THAN_OR_EQUAL: _ClassVar[FilterOperation]
    OP_IN: _ClassVar[FilterOperation]
    OP_NOT_IN: _ClassVar[FilterOperation]
    OP_CONTAINS: _ClassVar[FilterOperation]
    OP_STARTS_WITH: _ClassVar[FilterOperation]
    OP_ENDS_WITH: _ClassVar[FilterOperation]

class FilterOperator(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    FILTER_OPERATOR_UNSPECIFIED: _ClassVar[FilterOperator]
    LOGICAL_AND: _ClassVar[FilterOperator]
    LOGICAL_OR: _ClassVar[FilterOperator]
    LOGICAL_NOT: _ClassVar[FilterOperator]

class CompressionAlgorithm(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    COMPRESSION_NONE: _ClassVar[CompressionAlgorithm]
    COMPRESSION_ZSTD: _ClassVar[CompressionAlgorithm]
    COMPRESSION_LZ4: _ClassVar[CompressionAlgorithm]
    COMPRESSION_SNAPPY: _ClassVar[CompressionAlgorithm]
    COMPRESSION_GZIP: _ClassVar[CompressionAlgorithm]
    COMPRESSION_BROTLI: _ClassVar[CompressionAlgorithm]
    COMPRESSION_BZIP2: _ClassVar[CompressionAlgorithm]
    COMPRESSION_DEFLATE: _ClassVar[CompressionAlgorithm]
    COMPRESSION_XZ: _ClassVar[CompressionAlgorithm]
    COMPRESSION_ZLIB: _ClassVar[CompressionAlgorithm]
    COMPRESSION_LZO: _ClassVar[CompressionAlgorithm]
    COMPRESSION_LZ4HC: _ClassVar[CompressionAlgorithm]
    COMPRESSION_LZMA: _ClassVar[CompressionAlgorithm]

class FilterableDataType(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    FILTERABLE_DATA_TYPE_UNSPECIFIED: _ClassVar[FilterableDataType]
    FILTERABLE_STRING: _ClassVar[FilterableDataType]
    FILTERABLE_INTEGER: _ClassVar[FilterableDataType]
    FILTERABLE_FLOAT: _ClassVar[FilterableDataType]
    FILTERABLE_BOOLEAN: _ClassVar[FilterableDataType]
    FILTERABLE_DATETIME: _ClassVar[FilterableDataType]
    FILTERABLE_ARRAY_STRING: _ClassVar[FilterableDataType]
    FILTERABLE_ARRAY_INTEGER: _ClassVar[FilterableDataType]
    FILTERABLE_ARRAY_FLOAT: _ClassVar[FilterableDataType]

class ColumnEncoding(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    COLUMN_ENCODING_UNSPECIFIED: _ClassVar[ColumnEncoding]
    RLE: _ClassVar[ColumnEncoding]
    DICTIONARY: _ClassVar[ColumnEncoding]
    DELTA: _ClassVar[ColumnEncoding]
    PLAIN: _ClassVar[ColumnEncoding]
DISTANCE_METRIC_UNSPECIFIED: DistanceMetric
COSINE: DistanceMetric
EUCLIDEAN: DistanceMetric
DOT_PRODUCT: DistanceMetric
HAMMING: DistanceMetric
MANHATTAN: DistanceMetric
JACCARD: DistanceMetric
ANGULAR: DistanceMetric
CHEBYSHEV: DistanceMetric
CANBERRA: DistanceMetric
MINKOWSKI: DistanceMetric
BRAY_CURTIS: DistanceMetric
HELLINGER: DistanceMetric
CUSTOM: DistanceMetric
STORAGE_ENGINE_UNSPECIFIED: StorageEngine
VIPER: StorageEngine
SST: StorageEngine
NOVA: StorageEngine
HELIX: StorageEngine
SWIFT: StorageEngine
RAPTOR: StorageEngine
MMAP: StorageEngine
HYBRID: StorageEngine
INDEXING_ALGORITHM_UNSPECIFIED: IndexingAlgorithm
HNSW: IndexingAlgorithm
IVF: IndexingAlgorithm
PQ: IndexingAlgorithm
FLAT: IndexingAlgorithm
ANNOY: IndexingAlgorithm
LSH: IndexingAlgorithm
VECTOR_OPERATION_UNSPECIFIED: VectorOperation
VECTOR_BATCH: VectorOperation
VECTOR_SEARCH: VectorOperation
VECTOR_GET: VectorOperation
VECTOR_SERVICE_OPERATION_UNSPECIFIED: VectorServiceOperation
VS_BATCH: VectorServiceOperation
VS_SEARCH: VectorServiceOperation
VS_GET: VectorServiceOperation
FILTER_OPERATION_UNSPECIFIED: FilterOperation
OP_EQUALS: FilterOperation
OP_NOT_EQUALS: FilterOperation
OP_GREATER_THAN: FilterOperation
OP_GREATER_THAN_OR_EQUAL: FilterOperation
OP_LESS_THAN: FilterOperation
OP_LESS_THAN_OR_EQUAL: FilterOperation
OP_IN: FilterOperation
OP_NOT_IN: FilterOperation
OP_CONTAINS: FilterOperation
OP_STARTS_WITH: FilterOperation
OP_ENDS_WITH: FilterOperation
FILTER_OPERATOR_UNSPECIFIED: FilterOperator
LOGICAL_AND: FilterOperator
LOGICAL_OR: FilterOperator
LOGICAL_NOT: FilterOperator
COMPRESSION_NONE: CompressionAlgorithm
COMPRESSION_ZSTD: CompressionAlgorithm
COMPRESSION_LZ4: CompressionAlgorithm
COMPRESSION_SNAPPY: CompressionAlgorithm
COMPRESSION_GZIP: CompressionAlgorithm
COMPRESSION_BROTLI: CompressionAlgorithm
COMPRESSION_BZIP2: CompressionAlgorithm
COMPRESSION_DEFLATE: CompressionAlgorithm
COMPRESSION_XZ: CompressionAlgorithm
COMPRESSION_ZLIB: CompressionAlgorithm
COMPRESSION_LZO: CompressionAlgorithm
COMPRESSION_LZ4HC: CompressionAlgorithm
COMPRESSION_LZMA: CompressionAlgorithm
FILTERABLE_DATA_TYPE_UNSPECIFIED: FilterableDataType
FILTERABLE_STRING: FilterableDataType
FILTERABLE_INTEGER: FilterableDataType
FILTERABLE_FLOAT: FilterableDataType
FILTERABLE_BOOLEAN: FilterableDataType
FILTERABLE_DATETIME: FilterableDataType
FILTERABLE_ARRAY_STRING: FilterableDataType
FILTERABLE_ARRAY_INTEGER: FilterableDataType
FILTERABLE_ARRAY_FLOAT: FilterableDataType
COLUMN_ENCODING_UNSPECIFIED: ColumnEncoding
RLE: ColumnEncoding
DICTIONARY: ColumnEncoding
DELTA: ColumnEncoding
PLAIN: ColumnEncoding

class IncludeFields(_message.Message):
    __slots__ = ("vector", "metadata", "score", "rank", "source", "source_options")
    class SourceOptionsEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: bool
        def __init__(self, key: _Optional[str] = ..., value: bool = ...) -> None: ...
    VECTOR_FIELD_NUMBER: _ClassVar[int]
    METADATA_FIELD_NUMBER: _ClassVar[int]
    SCORE_FIELD_NUMBER: _ClassVar[int]
    RANK_FIELD_NUMBER: _ClassVar[int]
    SOURCE_FIELD_NUMBER: _ClassVar[int]
    SOURCE_OPTIONS_FIELD_NUMBER: _ClassVar[int]
    vector: bool
    metadata: bool
    score: bool
    rank: bool
    source: bool
    source_options: _containers.ScalarMap[str, bool]
    def __init__(self, vector: bool = ..., metadata: bool = ..., score: bool = ..., rank: bool = ..., source: bool = ..., source_options: _Optional[_Mapping[str, bool]] = ...) -> None: ...

class MetadataItem(_message.Message):
    __slots__ = ("key", "string_value", "number_value", "bool_value")
    KEY_FIELD_NUMBER: _ClassVar[int]
    STRING_VALUE_FIELD_NUMBER: _ClassVar[int]
    NUMBER_VALUE_FIELD_NUMBER: _ClassVar[int]
    BOOL_VALUE_FIELD_NUMBER: _ClassVar[int]
    key: str
    string_value: str
    number_value: float
    bool_value: bool
    def __init__(self, key: _Optional[str] = ..., string_value: _Optional[str] = ..., number_value: _Optional[float] = ..., bool_value: bool = ...) -> None: ...

class SearchQuery(_message.Message):
    __slots__ = ("vector", "filters", "advanced_filter")
    class FiltersEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: _types_pb2.SqlValue
        def __init__(self, key: _Optional[str] = ..., value: _Optional[_Union[_types_pb2.SqlValue, _Mapping]] = ...) -> None: ...
    VECTOR_FIELD_NUMBER: _ClassVar[int]
    FILTERS_FIELD_NUMBER: _ClassVar[int]
    ADVANCED_FILTER_FIELD_NUMBER: _ClassVar[int]
    vector: _containers.RepeatedScalarFieldContainer[float]
    filters: _containers.MessageMap[str, _types_pb2.SqlValue]
    advanced_filter: _entity_pb2.MetadataFilter
    def __init__(self, vector: _Optional[_Iterable[float]] = ..., filters: _Optional[_Mapping[str, _types_pb2.SqlValue]] = ..., advanced_filter: _Optional[_Union[_entity_pb2.MetadataFilter, _Mapping]] = ...) -> None: ...

class SearchParams(_message.Message):
    __slots__ = ("top_k", "accuracy_threshold", "include_expired", "timeout_ms", "enable_two_stage", "enable_clustering_hint", "enable_metadata_filtering_hint", "custom_hints")
    class CustomHintsEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: _types_pb2.SqlValue
        def __init__(self, key: _Optional[str] = ..., value: _Optional[_Union[_types_pb2.SqlValue, _Mapping]] = ...) -> None: ...
    TOP_K_FIELD_NUMBER: _ClassVar[int]
    ACCURACY_THRESHOLD_FIELD_NUMBER: _ClassVar[int]
    INCLUDE_EXPIRED_FIELD_NUMBER: _ClassVar[int]
    TIMEOUT_MS_FIELD_NUMBER: _ClassVar[int]
    ENABLE_TWO_STAGE_FIELD_NUMBER: _ClassVar[int]
    ENABLE_CLUSTERING_HINT_FIELD_NUMBER: _ClassVar[int]
    ENABLE_METADATA_FILTERING_HINT_FIELD_NUMBER: _ClassVar[int]
    CUSTOM_HINTS_FIELD_NUMBER: _ClassVar[int]
    top_k: int
    accuracy_threshold: float
    include_expired: bool
    timeout_ms: int
    enable_two_stage: bool
    enable_clustering_hint: bool
    enable_metadata_filtering_hint: bool
    custom_hints: _containers.MessageMap[str, _types_pb2.SqlValue]
    def __init__(self, top_k: _Optional[int] = ..., accuracy_threshold: _Optional[float] = ..., include_expired: bool = ..., timeout_ms: _Optional[int] = ..., enable_two_stage: bool = ..., enable_clustering_hint: bool = ..., enable_metadata_filtering_hint: bool = ..., custom_hints: _Optional[_Mapping[str, _types_pb2.SqlValue]] = ...) -> None: ...

class SearchOptimization(_message.Message):
    __slots__ = ("top_k", "accuracy_threshold", "filters")
    class FiltersEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: _types_pb2.SqlValue
        def __init__(self, key: _Optional[str] = ..., value: _Optional[_Union[_types_pb2.SqlValue, _Mapping]] = ...) -> None: ...
    TOP_K_FIELD_NUMBER: _ClassVar[int]
    ACCURACY_THRESHOLD_FIELD_NUMBER: _ClassVar[int]
    FILTERS_FIELD_NUMBER: _ClassVar[int]
    top_k: int
    accuracy_threshold: float
    filters: _containers.MessageMap[str, _types_pb2.SqlValue]
    def __init__(self, top_k: _Optional[int] = ..., accuracy_threshold: _Optional[float] = ..., filters: _Optional[_Mapping[str, _types_pb2.SqlValue]] = ...) -> None: ...

class VectorSearchRequest(_message.Message):
    __slots__ = ("collection_id", "queries", "top_k", "include_fields", "search_params", "distance_metric_override", "search_optimization")
    COLLECTION_ID_FIELD_NUMBER: _ClassVar[int]
    QUERIES_FIELD_NUMBER: _ClassVar[int]
    TOP_K_FIELD_NUMBER: _ClassVar[int]
    INCLUDE_FIELDS_FIELD_NUMBER: _ClassVar[int]
    SEARCH_PARAMS_FIELD_NUMBER: _ClassVar[int]
    DISTANCE_METRIC_OVERRIDE_FIELD_NUMBER: _ClassVar[int]
    SEARCH_OPTIMIZATION_FIELD_NUMBER: _ClassVar[int]
    collection_id: str
    queries: _containers.RepeatedCompositeFieldContainer[SearchQuery]
    top_k: int
    include_fields: IncludeFields
    search_params: SearchParams
    distance_metric_override: int
    search_optimization: SearchOptimization
    def __init__(self, collection_id: _Optional[str] = ..., queries: _Optional[_Iterable[_Union[SearchQuery, _Mapping]]] = ..., top_k: _Optional[int] = ..., include_fields: _Optional[_Union[IncludeFields, _Mapping]] = ..., search_params: _Optional[_Union[SearchParams, _Mapping]] = ..., distance_metric_override: _Optional[int] = ..., search_optimization: _Optional[_Union[SearchOptimization, _Mapping]] = ...) -> None: ...

class VectorRecord(_message.Message):
    __slots__ = ("id", "vector", "metadata", "timestamp", "updated_at", "expires_at", "version", "source")
    class MetadataEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: _types_pb2.SqlValue
        def __init__(self, key: _Optional[str] = ..., value: _Optional[_Union[_types_pb2.SqlValue, _Mapping]] = ...) -> None: ...
    ID_FIELD_NUMBER: _ClassVar[int]
    VECTOR_FIELD_NUMBER: _ClassVar[int]
    METADATA_FIELD_NUMBER: _ClassVar[int]
    TIMESTAMP_FIELD_NUMBER: _ClassVar[int]
    UPDATED_AT_FIELD_NUMBER: _ClassVar[int]
    EXPIRES_AT_FIELD_NUMBER: _ClassVar[int]
    VERSION_FIELD_NUMBER: _ClassVar[int]
    SOURCE_FIELD_NUMBER: _ClassVar[int]
    id: str
    vector: _containers.RepeatedScalarFieldContainer[float]
    metadata: _containers.MessageMap[str, _types_pb2.SqlValue]
    timestamp: int
    updated_at: int
    expires_at: int
    version: int
    source: str
    def __init__(self, id: _Optional[str] = ..., vector: _Optional[_Iterable[float]] = ..., metadata: _Optional[_Mapping[str, _types_pb2.SqlValue]] = ..., timestamp: _Optional[int] = ..., updated_at: _Optional[int] = ..., expires_at: _Optional[int] = ..., version: _Optional[int] = ..., source: _Optional[str] = ...) -> None: ...

class VectorBatchRequest(_message.Message):
    __slots__ = ("collection_id", "vectors")
    COLLECTION_ID_FIELD_NUMBER: _ClassVar[int]
    VECTORS_FIELD_NUMBER: _ClassVar[int]
    collection_id: str
    vectors: _containers.RepeatedCompositeFieldContainer[VectorRecord]
    def __init__(self, collection_id: _Optional[str] = ..., vectors: _Optional[_Iterable[_Union[VectorRecord, _Mapping]]] = ...) -> None: ...

class VectorGetRequest(_message.Message):
    __slots__ = ("collection_id", "vector_id", "include_vector", "include_metadata")
    COLLECTION_ID_FIELD_NUMBER: _ClassVar[int]
    VECTOR_ID_FIELD_NUMBER: _ClassVar[int]
    INCLUDE_VECTOR_FIELD_NUMBER: _ClassVar[int]
    INCLUDE_METADATA_FIELD_NUMBER: _ClassVar[int]
    collection_id: str
    vector_id: str
    include_vector: bool
    include_metadata: bool
    def __init__(self, collection_id: _Optional[str] = ..., vector_id: _Optional[str] = ..., include_vector: bool = ..., include_metadata: bool = ...) -> None: ...

class OperationMetrics(_message.Message):
    __slots__ = ("total_processed", "successful_count", "failed_count", "updated_count", "processing_time_us", "wal_write_time_us", "index_update_time_us")
    TOTAL_PROCESSED_FIELD_NUMBER: _ClassVar[int]
    SUCCESSFUL_COUNT_FIELD_NUMBER: _ClassVar[int]
    FAILED_COUNT_FIELD_NUMBER: _ClassVar[int]
    UPDATED_COUNT_FIELD_NUMBER: _ClassVar[int]
    PROCESSING_TIME_US_FIELD_NUMBER: _ClassVar[int]
    WAL_WRITE_TIME_US_FIELD_NUMBER: _ClassVar[int]
    INDEX_UPDATE_TIME_US_FIELD_NUMBER: _ClassVar[int]
    total_processed: int
    successful_count: int
    failed_count: int
    updated_count: int
    processing_time_us: int
    wal_write_time_us: int
    index_update_time_us: int
    def __init__(self, total_processed: _Optional[int] = ..., successful_count: _Optional[int] = ..., failed_count: _Optional[int] = ..., updated_count: _Optional[int] = ..., processing_time_us: _Optional[int] = ..., wal_write_time_us: _Optional[int] = ..., index_update_time_us: _Optional[int] = ...) -> None: ...

class SearchVectorRecord(_message.Message):
    __slots__ = ("id", "score", "vector", "metadata", "version", "similarity", "timestamp", "source", "expanded_context", "semantic_similarity", "quantization_info", "engine_stats", "index_path")
    class MetadataEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: _types_pb2.SqlValue
        def __init__(self, key: _Optional[str] = ..., value: _Optional[_Union[_types_pb2.SqlValue, _Mapping]] = ...) -> None: ...
    class EngineStatsEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    ID_FIELD_NUMBER: _ClassVar[int]
    SCORE_FIELD_NUMBER: _ClassVar[int]
    VECTOR_FIELD_NUMBER: _ClassVar[int]
    METADATA_FIELD_NUMBER: _ClassVar[int]
    VERSION_FIELD_NUMBER: _ClassVar[int]
    SIMILARITY_FIELD_NUMBER: _ClassVar[int]
    TIMESTAMP_FIELD_NUMBER: _ClassVar[int]
    SOURCE_FIELD_NUMBER: _ClassVar[int]
    EXPANDED_CONTEXT_FIELD_NUMBER: _ClassVar[int]
    SEMANTIC_SIMILARITY_FIELD_NUMBER: _ClassVar[int]
    QUANTIZATION_INFO_FIELD_NUMBER: _ClassVar[int]
    ENGINE_STATS_FIELD_NUMBER: _ClassVar[int]
    INDEX_PATH_FIELD_NUMBER: _ClassVar[int]
    id: str
    score: float
    vector: _containers.RepeatedScalarFieldContainer[float]
    metadata: _containers.MessageMap[str, _types_pb2.SqlValue]
    version: int
    similarity: float
    timestamp: int
    source: str
    expanded_context: _containers.RepeatedScalarFieldContainer[str]
    semantic_similarity: float
    quantization_info: str
    engine_stats: _containers.ScalarMap[str, str]
    index_path: str
    def __init__(self, id: _Optional[str] = ..., score: _Optional[float] = ..., vector: _Optional[_Iterable[float]] = ..., metadata: _Optional[_Mapping[str, _types_pb2.SqlValue]] = ..., version: _Optional[int] = ..., similarity: _Optional[float] = ..., timestamp: _Optional[int] = ..., source: _Optional[str] = ..., expanded_context: _Optional[_Iterable[str]] = ..., semantic_similarity: _Optional[float] = ..., quantization_info: _Optional[str] = ..., engine_stats: _Optional[_Mapping[str, str]] = ..., index_path: _Optional[str] = ...) -> None: ...

class SearchResult(_message.Message):
    __slots__ = ("results", "total_found", "collection_id")
    RESULTS_FIELD_NUMBER: _ClassVar[int]
    TOTAL_FOUND_FIELD_NUMBER: _ClassVar[int]
    COLLECTION_ID_FIELD_NUMBER: _ClassVar[int]
    results: _containers.RepeatedCompositeFieldContainer[SearchVectorRecord]
    total_found: int
    collection_id: str
    def __init__(self, results: _Optional[_Iterable[_Union[SearchVectorRecord, _Mapping]]] = ..., total_found: _Optional[int] = ..., collection_id: _Optional[str] = ...) -> None: ...

class VectorOperationResponse(_message.Message):
    __slots__ = ("success", "operation", "metrics", "results", "vector_ids", "error_message", "error_code")
    SUCCESS_FIELD_NUMBER: _ClassVar[int]
    OPERATION_FIELD_NUMBER: _ClassVar[int]
    METRICS_FIELD_NUMBER: _ClassVar[int]
    RESULTS_FIELD_NUMBER: _ClassVar[int]
    VECTOR_IDS_FIELD_NUMBER: _ClassVar[int]
    ERROR_MESSAGE_FIELD_NUMBER: _ClassVar[int]
    ERROR_CODE_FIELD_NUMBER: _ClassVar[int]
    success: bool
    operation: int
    metrics: OperationMetrics
    results: SearchResult
    vector_ids: _containers.RepeatedScalarFieldContainer[str]
    error_message: str
    error_code: str
    def __init__(self, success: bool = ..., operation: _Optional[int] = ..., metrics: _Optional[_Union[OperationMetrics, _Mapping]] = ..., results: _Optional[_Union[SearchResult, _Mapping]] = ..., vector_ids: _Optional[_Iterable[str]] = ..., error_message: _Optional[str] = ..., error_code: _Optional[str] = ...) -> None: ...

class QuantizationConfig(_message.Message):
    __slots__ = ("enabled", "strategy", "custom_levels", "enable_progressive_search", "binary_filter_selectivity", "int8_ranking_selectivity", "pq_ranking_selectivity", "training_sample_size", "quality_threshold", "enable_adaptive_training", "optimize_for_storage", "optimize_for_memory", "enable_simd_acceleration", "enable_binary", "enable_int8", "enable_pq", "pq_segments", "pq_bits", "pq_codebooks", "binary_threshold", "int8_threshold", "pq_threshold")
    class Strategy(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
        __slots__ = ()
        SMART_DEFAULTS: _ClassVar[QuantizationConfig.Strategy]
        CUSTOM_LEVELS: _ClassVar[QuantizationConfig.Strategy]
        MINIMAL: _ClassVar[QuantizationConfig.Strategy]
        AGGRESSIVE: _ClassVar[QuantizationConfig.Strategy]
    SMART_DEFAULTS: QuantizationConfig.Strategy
    CUSTOM_LEVELS: QuantizationConfig.Strategy
    MINIMAL: QuantizationConfig.Strategy
    AGGRESSIVE: QuantizationConfig.Strategy
    ENABLED_FIELD_NUMBER: _ClassVar[int]
    STRATEGY_FIELD_NUMBER: _ClassVar[int]
    CUSTOM_LEVELS_FIELD_NUMBER: _ClassVar[int]
    ENABLE_PROGRESSIVE_SEARCH_FIELD_NUMBER: _ClassVar[int]
    BINARY_FILTER_SELECTIVITY_FIELD_NUMBER: _ClassVar[int]
    INT8_RANKING_SELECTIVITY_FIELD_NUMBER: _ClassVar[int]
    PQ_RANKING_SELECTIVITY_FIELD_NUMBER: _ClassVar[int]
    TRAINING_SAMPLE_SIZE_FIELD_NUMBER: _ClassVar[int]
    QUALITY_THRESHOLD_FIELD_NUMBER: _ClassVar[int]
    ENABLE_ADAPTIVE_TRAINING_FIELD_NUMBER: _ClassVar[int]
    OPTIMIZE_FOR_STORAGE_FIELD_NUMBER: _ClassVar[int]
    OPTIMIZE_FOR_MEMORY_FIELD_NUMBER: _ClassVar[int]
    ENABLE_SIMD_ACCELERATION_FIELD_NUMBER: _ClassVar[int]
    ENABLE_BINARY_FIELD_NUMBER: _ClassVar[int]
    ENABLE_INT8_FIELD_NUMBER: _ClassVar[int]
    ENABLE_PQ_FIELD_NUMBER: _ClassVar[int]
    PQ_SEGMENTS_FIELD_NUMBER: _ClassVar[int]
    PQ_BITS_FIELD_NUMBER: _ClassVar[int]
    PQ_CODEBOOKS_FIELD_NUMBER: _ClassVar[int]
    BINARY_THRESHOLD_FIELD_NUMBER: _ClassVar[int]
    INT8_THRESHOLD_FIELD_NUMBER: _ClassVar[int]
    PQ_THRESHOLD_FIELD_NUMBER: _ClassVar[int]
    enabled: bool
    strategy: QuantizationConfig.Strategy
    custom_levels: _containers.RepeatedCompositeFieldContainer[QuantizationLevel]
    enable_progressive_search: bool
    binary_filter_selectivity: float
    int8_ranking_selectivity: float
    pq_ranking_selectivity: float
    training_sample_size: int
    quality_threshold: float
    enable_adaptive_training: bool
    optimize_for_storage: bool
    optimize_for_memory: bool
    enable_simd_acceleration: bool
    enable_binary: bool
    enable_int8: bool
    enable_pq: bool
    pq_segments: int
    pq_bits: int
    pq_codebooks: int
    binary_threshold: float
    int8_threshold: float
    pq_threshold: float
    def __init__(self, enabled: bool = ..., strategy: _Optional[_Union[QuantizationConfig.Strategy, str]] = ..., custom_levels: _Optional[_Iterable[_Union[QuantizationLevel, _Mapping]]] = ..., enable_progressive_search: bool = ..., binary_filter_selectivity: _Optional[float] = ..., int8_ranking_selectivity: _Optional[float] = ..., pq_ranking_selectivity: _Optional[float] = ..., training_sample_size: _Optional[int] = ..., quality_threshold: _Optional[float] = ..., enable_adaptive_training: bool = ..., optimize_for_storage: bool = ..., optimize_for_memory: bool = ..., enable_simd_acceleration: bool = ..., enable_binary: bool = ..., enable_int8: bool = ..., enable_pq: bool = ..., pq_segments: _Optional[int] = ..., pq_bits: _Optional[int] = ..., pq_codebooks: _Optional[int] = ..., binary_threshold: _Optional[float] = ..., int8_threshold: _Optional[float] = ..., pq_threshold: _Optional[float] = ...) -> None: ...

class QuantizationLevel(_message.Message):
    __slots__ = ("level_id", "type", "bits", "num_subvectors", "adaptive_subvectors", "scale", "offset", "clamp_values", "threshold", "sign_based", "enable_in_storage", "enable_in_index", "search_priority", "min_recall", "enable_validation")
    class QuantizationType(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
        __slots__ = ()
        BINARY: _ClassVar[QuantizationLevel.QuantizationType]
        SCALAR: _ClassVar[QuantizationLevel.QuantizationType]
        PRODUCT: _ClassVar[QuantizationLevel.QuantizationType]
        UNIFORM: _ClassVar[QuantizationLevel.QuantizationType]
        NONE: _ClassVar[QuantizationLevel.QuantizationType]
    BINARY: QuantizationLevel.QuantizationType
    SCALAR: QuantizationLevel.QuantizationType
    PRODUCT: QuantizationLevel.QuantizationType
    UNIFORM: QuantizationLevel.QuantizationType
    NONE: QuantizationLevel.QuantizationType
    LEVEL_ID_FIELD_NUMBER: _ClassVar[int]
    TYPE_FIELD_NUMBER: _ClassVar[int]
    BITS_FIELD_NUMBER: _ClassVar[int]
    NUM_SUBVECTORS_FIELD_NUMBER: _ClassVar[int]
    ADAPTIVE_SUBVECTORS_FIELD_NUMBER: _ClassVar[int]
    SCALE_FIELD_NUMBER: _ClassVar[int]
    OFFSET_FIELD_NUMBER: _ClassVar[int]
    CLAMP_VALUES_FIELD_NUMBER: _ClassVar[int]
    THRESHOLD_FIELD_NUMBER: _ClassVar[int]
    SIGN_BASED_FIELD_NUMBER: _ClassVar[int]
    ENABLE_IN_STORAGE_FIELD_NUMBER: _ClassVar[int]
    ENABLE_IN_INDEX_FIELD_NUMBER: _ClassVar[int]
    SEARCH_PRIORITY_FIELD_NUMBER: _ClassVar[int]
    MIN_RECALL_FIELD_NUMBER: _ClassVar[int]
    ENABLE_VALIDATION_FIELD_NUMBER: _ClassVar[int]
    level_id: str
    type: QuantizationLevel.QuantizationType
    bits: int
    num_subvectors: int
    adaptive_subvectors: bool
    scale: float
    offset: float
    clamp_values: bool
    threshold: float
    sign_based: bool
    enable_in_storage: bool
    enable_in_index: bool
    search_priority: int
    min_recall: float
    enable_validation: bool
    def __init__(self, level_id: _Optional[str] = ..., type: _Optional[_Union[QuantizationLevel.QuantizationType, str]] = ..., bits: _Optional[int] = ..., num_subvectors: _Optional[int] = ..., adaptive_subvectors: bool = ..., scale: _Optional[float] = ..., offset: _Optional[float] = ..., clamp_values: bool = ..., threshold: _Optional[float] = ..., sign_based: bool = ..., enable_in_storage: bool = ..., enable_in_index: bool = ..., search_priority: _Optional[int] = ..., min_recall: _Optional[float] = ..., enable_validation: bool = ...) -> None: ...

class FilterableColumnSpec(_message.Message):
    __slots__ = ("name", "data_type", "indexed", "supports_range", "estimated_cardinality")
    NAME_FIELD_NUMBER: _ClassVar[int]
    DATA_TYPE_FIELD_NUMBER: _ClassVar[int]
    INDEXED_FIELD_NUMBER: _ClassVar[int]
    SUPPORTS_RANGE_FIELD_NUMBER: _ClassVar[int]
    ESTIMATED_CARDINALITY_FIELD_NUMBER: _ClassVar[int]
    name: str
    data_type: FilterableDataType
    indexed: bool
    supports_range: bool
    estimated_cardinality: int
    def __init__(self, name: _Optional[str] = ..., data_type: _Optional[_Union[FilterableDataType, str]] = ..., indexed: bool = ..., supports_range: bool = ..., estimated_cardinality: _Optional[int] = ...) -> None: ...

class SourceContent(_message.Message):
    __slots__ = ("text_content", "binary_content", "external_reference")
    TEXT_CONTENT_FIELD_NUMBER: _ClassVar[int]
    BINARY_CONTENT_FIELD_NUMBER: _ClassVar[int]
    EXTERNAL_REFERENCE_FIELD_NUMBER: _ClassVar[int]
    text_content: str
    binary_content: bytes
    external_reference: str
    def __init__(self, text_content: _Optional[str] = ..., binary_content: _Optional[bytes] = ..., external_reference: _Optional[str] = ...) -> None: ...

class CompressionConfig(_message.Message):
    __slots__ = ("algorithm", "level", "adaptive", "min_ratio", "enable_quantization", "quantization_type", "normalization_method", "block_size_kb", "dynamic_block_sizing")
    ALGORITHM_FIELD_NUMBER: _ClassVar[int]
    LEVEL_FIELD_NUMBER: _ClassVar[int]
    ADAPTIVE_FIELD_NUMBER: _ClassVar[int]
    MIN_RATIO_FIELD_NUMBER: _ClassVar[int]
    ENABLE_QUANTIZATION_FIELD_NUMBER: _ClassVar[int]
    QUANTIZATION_TYPE_FIELD_NUMBER: _ClassVar[int]
    NORMALIZATION_METHOD_FIELD_NUMBER: _ClassVar[int]
    BLOCK_SIZE_KB_FIELD_NUMBER: _ClassVar[int]
    DYNAMIC_BLOCK_SIZING_FIELD_NUMBER: _ClassVar[int]
    algorithm: CompressionAlgorithm
    level: int
    adaptive: bool
    min_ratio: float
    enable_quantization: bool
    quantization_type: str
    normalization_method: str
    block_size_kb: int
    dynamic_block_sizing: bool
    def __init__(self, algorithm: _Optional[_Union[CompressionAlgorithm, str]] = ..., level: _Optional[int] = ..., adaptive: bool = ..., min_ratio: _Optional[float] = ..., enable_quantization: bool = ..., quantization_type: _Optional[str] = ..., normalization_method: _Optional[str] = ..., block_size_kb: _Optional[int] = ..., dynamic_block_sizing: bool = ...) -> None: ...

class MetadataValue(_message.Message):
    __slots__ = ("string_value", "int_value", "double_value", "bool_value")
    STRING_VALUE_FIELD_NUMBER: _ClassVar[int]
    INT_VALUE_FIELD_NUMBER: _ClassVar[int]
    DOUBLE_VALUE_FIELD_NUMBER: _ClassVar[int]
    BOOL_VALUE_FIELD_NUMBER: _ClassVar[int]
    string_value: str
    int_value: int
    double_value: float
    bool_value: bool
    def __init__(self, string_value: _Optional[str] = ..., int_value: _Optional[int] = ..., double_value: _Optional[float] = ..., bool_value: bool = ...) -> None: ...

class FilterCondition(_message.Message):
    __slots__ = ("field_name", "operation", "value")
    FIELD_NAME_FIELD_NUMBER: _ClassVar[int]
    OPERATION_FIELD_NUMBER: _ClassVar[int]
    VALUE_FIELD_NUMBER: _ClassVar[int]
    field_name: str
    operation: int
    value: MetadataValue
    def __init__(self, field_name: _Optional[str] = ..., operation: _Optional[int] = ..., value: _Optional[_Union[MetadataValue, _Mapping]] = ...) -> None: ...

class HnswConfig(_message.Message):
    __slots__ = ("m", "ef_construction", "ef_search", "max_partition_size", "adaptive_parameters", "use_simd", "memory_limit_mb", "lazy_loading")
    M_FIELD_NUMBER: _ClassVar[int]
    EF_CONSTRUCTION_FIELD_NUMBER: _ClassVar[int]
    EF_SEARCH_FIELD_NUMBER: _ClassVar[int]
    MAX_PARTITION_SIZE_FIELD_NUMBER: _ClassVar[int]
    ADAPTIVE_PARAMETERS_FIELD_NUMBER: _ClassVar[int]
    USE_SIMD_FIELD_NUMBER: _ClassVar[int]
    MEMORY_LIMIT_MB_FIELD_NUMBER: _ClassVar[int]
    LAZY_LOADING_FIELD_NUMBER: _ClassVar[int]
    m: int
    ef_construction: int
    ef_search: int
    max_partition_size: int
    adaptive_parameters: bool
    use_simd: bool
    memory_limit_mb: int
    lazy_loading: bool
    def __init__(self, m: _Optional[int] = ..., ef_construction: _Optional[int] = ..., ef_search: _Optional[int] = ..., max_partition_size: _Optional[int] = ..., adaptive_parameters: bool = ..., use_simd: bool = ..., memory_limit_mb: _Optional[int] = ..., lazy_loading: bool = ...) -> None: ...

class IvfConfig(_message.Message):
    __slots__ = ("n_lists", "n_probe", "quantization_bits", "use_pq", "pq_subspaces", "train_on_insert", "min_train_size")
    N_LISTS_FIELD_NUMBER: _ClassVar[int]
    N_PROBE_FIELD_NUMBER: _ClassVar[int]
    QUANTIZATION_BITS_FIELD_NUMBER: _ClassVar[int]
    USE_PQ_FIELD_NUMBER: _ClassVar[int]
    PQ_SUBSPACES_FIELD_NUMBER: _ClassVar[int]
    TRAIN_ON_INSERT_FIELD_NUMBER: _ClassVar[int]
    MIN_TRAIN_SIZE_FIELD_NUMBER: _ClassVar[int]
    n_lists: int
    n_probe: int
    quantization_bits: int
    use_pq: bool
    pq_subspaces: int
    train_on_insert: bool
    min_train_size: int
    def __init__(self, n_lists: _Optional[int] = ..., n_probe: _Optional[int] = ..., quantization_bits: _Optional[int] = ..., use_pq: bool = ..., pq_subspaces: _Optional[int] = ..., train_on_insert: bool = ..., min_train_size: _Optional[int] = ...) -> None: ...

class LshConfig(_message.Message):
    __slots__ = ("n_hash_tables", "n_hash_functions", "bucket_width", "binary_vectors", "max_candidates", "projection")
    N_HASH_TABLES_FIELD_NUMBER: _ClassVar[int]
    N_HASH_FUNCTIONS_FIELD_NUMBER: _ClassVar[int]
    BUCKET_WIDTH_FIELD_NUMBER: _ClassVar[int]
    BINARY_VECTORS_FIELD_NUMBER: _ClassVar[int]
    MAX_CANDIDATES_FIELD_NUMBER: _ClassVar[int]
    PROJECTION_FIELD_NUMBER: _ClassVar[int]
    n_hash_tables: int
    n_hash_functions: int
    bucket_width: float
    binary_vectors: bool
    max_candidates: int
    projection: int
    def __init__(self, n_hash_tables: _Optional[int] = ..., n_hash_functions: _Optional[int] = ..., bucket_width: _Optional[float] = ..., binary_vectors: bool = ..., max_candidates: _Optional[int] = ..., projection: _Optional[int] = ...) -> None: ...
