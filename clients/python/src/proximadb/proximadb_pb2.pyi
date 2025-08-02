from google.protobuf import struct_pb2 as _struct_pb2
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
    CUSTOM: _ClassVar[DistanceMetric]
    CHEBYSHEV: _ClassVar[DistanceMetric]
    CANBERRA: _ClassVar[DistanceMetric]
    MINKOWSKI: _ClassVar[DistanceMetric]
    ANGULAR: _ClassVar[DistanceMetric]
    BRAY_CURTIS: _ClassVar[DistanceMetric]
    HELLINGER: _ClassVar[DistanceMetric]

class StorageEngine(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    STORAGE_ENGINE_UNSPECIFIED: _ClassVar[StorageEngine]
    VIPER: _ClassVar[StorageEngine]
    SST: _ClassVar[StorageEngine]
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

class VectorOperation(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    VECTOR_OPERATION_UNSPECIFIED: _ClassVar[VectorOperation]
    VECTOR_BATCH: _ClassVar[VectorOperation]
    VECTOR_SEARCH: _ClassVar[VectorOperation]
    VECTOR_GET: _ClassVar[VectorOperation]

class IndexUpdateMode(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    INDEX_UPDATE_MODE_UNSPECIFIED: _ClassVar[IndexUpdateMode]
    SYNCHRONOUS: _ClassVar[IndexUpdateMode]
    ASYNCHRONOUS: _ClassVar[IndexUpdateMode]
    HYBRID_MODE: _ClassVar[IndexUpdateMode]

class RandomProjectionType(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    GAUSSIAN: _ClassVar[RandomProjectionType]
    BINARY: _ClassVar[RandomProjectionType]
    SPARSE: _ClassVar[RandomProjectionType]

class StorageEngineCompatibility(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    STORAGE_ENGINE_COMPATIBILITY_UNSPECIFIED: _ClassVar[StorageEngineCompatibility]
    VIPER_ONLY: _ClassVar[StorageEngineCompatibility]
    ALL_ENGINES: _ClassVar[StorageEngineCompatibility]
    LSM_AND_VIPER: _ClassVar[StorageEngineCompatibility]

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

class FilterOperator(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    FILTER_OPERATOR_UNSPECIFIED: _ClassVar[FilterOperator]
    AND: _ClassVar[FilterOperator]
    OR: _ClassVar[FilterOperator]
    NOT: _ClassVar[FilterOperator]

class FilterOperation(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    FILTER_OPERATION_UNSPECIFIED: _ClassVar[FilterOperation]
    EQUALS: _ClassVar[FilterOperation]
    NOT_EQUALS: _ClassVar[FilterOperation]
    GREATER_THAN: _ClassVar[FilterOperation]
    LESS_THAN: _ClassVar[FilterOperation]
    GREATER_THAN_OR_EQUAL: _ClassVar[FilterOperation]
    LESS_THAN_OR_EQUAL: _ClassVar[FilterOperation]
    IN: _ClassVar[FilterOperation]
    NOT_IN: _ClassVar[FilterOperation]
    CONTAINS: _ClassVar[FilterOperation]
    STARTS_WITH: _ClassVar[FilterOperation]
    ENDS_WITH: _ClassVar[FilterOperation]
DISTANCE_METRIC_UNSPECIFIED: DistanceMetric
COSINE: DistanceMetric
EUCLIDEAN: DistanceMetric
DOT_PRODUCT: DistanceMetric
HAMMING: DistanceMetric
MANHATTAN: DistanceMetric
JACCARD: DistanceMetric
CUSTOM: DistanceMetric
CHEBYSHEV: DistanceMetric
CANBERRA: DistanceMetric
MINKOWSKI: DistanceMetric
ANGULAR: DistanceMetric
BRAY_CURTIS: DistanceMetric
HELLINGER: DistanceMetric
STORAGE_ENGINE_UNSPECIFIED: StorageEngine
VIPER: StorageEngine
SST: StorageEngine
MMAP: StorageEngine
HYBRID: StorageEngine
INDEXING_ALGORITHM_UNSPECIFIED: IndexingAlgorithm
HNSW: IndexingAlgorithm
IVF: IndexingAlgorithm
PQ: IndexingAlgorithm
FLAT: IndexingAlgorithm
ANNOY: IndexingAlgorithm
LSH: IndexingAlgorithm
COLLECTION_OPERATION_UNSPECIFIED: CollectionOperation
COLLECTION_CREATE: CollectionOperation
COLLECTION_UPDATE: CollectionOperation
COLLECTION_GET: CollectionOperation
COLLECTION_LIST: CollectionOperation
COLLECTION_DELETE: CollectionOperation
COLLECTION_MIGRATE: CollectionOperation
COLLECTION_GET_ID_BY_NAME: CollectionOperation
VECTOR_OPERATION_UNSPECIFIED: VectorOperation
VECTOR_BATCH: VectorOperation
VECTOR_SEARCH: VectorOperation
VECTOR_GET: VectorOperation
INDEX_UPDATE_MODE_UNSPECIFIED: IndexUpdateMode
SYNCHRONOUS: IndexUpdateMode
ASYNCHRONOUS: IndexUpdateMode
HYBRID_MODE: IndexUpdateMode
GAUSSIAN: RandomProjectionType
BINARY: RandomProjectionType
SPARSE: RandomProjectionType
STORAGE_ENGINE_COMPATIBILITY_UNSPECIFIED: StorageEngineCompatibility
VIPER_ONLY: StorageEngineCompatibility
ALL_ENGINES: StorageEngineCompatibility
LSM_AND_VIPER: StorageEngineCompatibility
FILTERABLE_DATA_TYPE_UNSPECIFIED: FilterableDataType
FILTERABLE_STRING: FilterableDataType
FILTERABLE_INTEGER: FilterableDataType
FILTERABLE_FLOAT: FilterableDataType
FILTERABLE_BOOLEAN: FilterableDataType
FILTERABLE_DATETIME: FilterableDataType
FILTERABLE_ARRAY_STRING: FilterableDataType
FILTERABLE_ARRAY_INTEGER: FilterableDataType
FILTERABLE_ARRAY_FLOAT: FilterableDataType
FILTER_OPERATOR_UNSPECIFIED: FilterOperator
AND: FilterOperator
OR: FilterOperator
NOT: FilterOperator
FILTER_OPERATION_UNSPECIFIED: FilterOperation
EQUALS: FilterOperation
NOT_EQUALS: FilterOperation
GREATER_THAN: FilterOperation
LESS_THAN: FilterOperation
GREATER_THAN_OR_EQUAL: FilterOperation
LESS_THAN_OR_EQUAL: FilterOperation
IN: FilterOperation
NOT_IN: FilterOperation
CONTAINS: FilterOperation
STARTS_WITH: FilterOperation
ENDS_WITH: FilterOperation

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

class VectorRecord(_message.Message):
    __slots__ = ("id", "vector", "metadata", "timestamp", "updated_at", "expires_at", "version", "rank", "score", "distance")
    ID_FIELD_NUMBER: _ClassVar[int]
    VECTOR_FIELD_NUMBER: _ClassVar[int]
    METADATA_FIELD_NUMBER: _ClassVar[int]
    TIMESTAMP_FIELD_NUMBER: _ClassVar[int]
    UPDATED_AT_FIELD_NUMBER: _ClassVar[int]
    EXPIRES_AT_FIELD_NUMBER: _ClassVar[int]
    VERSION_FIELD_NUMBER: _ClassVar[int]
    RANK_FIELD_NUMBER: _ClassVar[int]
    SCORE_FIELD_NUMBER: _ClassVar[int]
    DISTANCE_FIELD_NUMBER: _ClassVar[int]
    id: str
    vector: _containers.RepeatedScalarFieldContainer[float]
    metadata: _containers.RepeatedCompositeFieldContainer[MetadataItem]
    timestamp: int
    updated_at: int
    expires_at: int
    version: int
    rank: int
    score: float
    distance: float
    def __init__(self, id: _Optional[str] = ..., vector: _Optional[_Iterable[float]] = ..., metadata: _Optional[_Iterable[_Union[MetadataItem, _Mapping]]] = ..., timestamp: _Optional[int] = ..., updated_at: _Optional[int] = ..., expires_at: _Optional[int] = ..., version: _Optional[int] = ..., rank: _Optional[int] = ..., score: _Optional[float] = ..., distance: _Optional[float] = ...) -> None: ...

class MetadataMap(_message.Message):
    __slots__ = ("fields",)
    class FieldsEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: MetadataValue
        def __init__(self, key: _Optional[str] = ..., value: _Optional[_Union[MetadataValue, _Mapping]] = ...) -> None: ...
    FIELDS_FIELD_NUMBER: _ClassVar[int]
    fields: _containers.MessageMap[str, MetadataValue]
    def __init__(self, fields: _Optional[_Mapping[str, MetadataValue]] = ...) -> None: ...

class MetadataValue(_message.Message):
    __slots__ = ("string_value", "int_value", "double_value", "bool_value", "string_array", "int_array", "double_array")
    STRING_VALUE_FIELD_NUMBER: _ClassVar[int]
    INT_VALUE_FIELD_NUMBER: _ClassVar[int]
    DOUBLE_VALUE_FIELD_NUMBER: _ClassVar[int]
    BOOL_VALUE_FIELD_NUMBER: _ClassVar[int]
    STRING_ARRAY_FIELD_NUMBER: _ClassVar[int]
    INT_ARRAY_FIELD_NUMBER: _ClassVar[int]
    DOUBLE_ARRAY_FIELD_NUMBER: _ClassVar[int]
    string_value: str
    int_value: int
    double_value: float
    bool_value: bool
    string_array: StringArray
    int_array: Int64Array
    double_array: DoubleArray
    def __init__(self, string_value: _Optional[str] = ..., int_value: _Optional[int] = ..., double_value: _Optional[float] = ..., bool_value: bool = ..., string_array: _Optional[_Union[StringArray, _Mapping]] = ..., int_array: _Optional[_Union[Int64Array, _Mapping]] = ..., double_array: _Optional[_Union[DoubleArray, _Mapping]] = ...) -> None: ...

class StringArray(_message.Message):
    __slots__ = ("values",)
    VALUES_FIELD_NUMBER: _ClassVar[int]
    values: _containers.RepeatedScalarFieldContainer[str]
    def __init__(self, values: _Optional[_Iterable[str]] = ...) -> None: ...

class Int64Array(_message.Message):
    __slots__ = ("values",)
    VALUES_FIELD_NUMBER: _ClassVar[int]
    values: _containers.RepeatedScalarFieldContainer[int]
    def __init__(self, values: _Optional[_Iterable[int]] = ...) -> None: ...

class DoubleArray(_message.Message):
    __slots__ = ("values",)
    VALUES_FIELD_NUMBER: _ClassVar[int]
    values: _containers.RepeatedScalarFieldContainer[float]
    def __init__(self, values: _Optional[_Iterable[float]] = ...) -> None: ...

class CollectionConfig(_message.Message):
    __slots__ = ("name", "dimension", "distance_metric", "storage_engine", "primary_indexing_algorithm", "filterable_columns", "index_configs", "quantization_config", "primary_index_name", "enable_automatic_index_selection", "description", "tags", "owner")
    NAME_FIELD_NUMBER: _ClassVar[int]
    DIMENSION_FIELD_NUMBER: _ClassVar[int]
    DISTANCE_METRIC_FIELD_NUMBER: _ClassVar[int]
    STORAGE_ENGINE_FIELD_NUMBER: _ClassVar[int]
    PRIMARY_INDEXING_ALGORITHM_FIELD_NUMBER: _ClassVar[int]
    FILTERABLE_COLUMNS_FIELD_NUMBER: _ClassVar[int]
    INDEX_CONFIGS_FIELD_NUMBER: _ClassVar[int]
    QUANTIZATION_CONFIG_FIELD_NUMBER: _ClassVar[int]
    PRIMARY_INDEX_NAME_FIELD_NUMBER: _ClassVar[int]
    ENABLE_AUTOMATIC_INDEX_SELECTION_FIELD_NUMBER: _ClassVar[int]
    DESCRIPTION_FIELD_NUMBER: _ClassVar[int]
    TAGS_FIELD_NUMBER: _ClassVar[int]
    OWNER_FIELD_NUMBER: _ClassVar[int]
    name: str
    dimension: int
    distance_metric: DistanceMetric
    storage_engine: StorageEngine
    primary_indexing_algorithm: IndexingAlgorithm
    filterable_columns: _containers.RepeatedCompositeFieldContainer[FilterableColumnSpec]
    index_configs: _containers.RepeatedCompositeFieldContainer[IndexConfig]
    quantization_config: QuantizationConfig
    primary_index_name: str
    enable_automatic_index_selection: bool
    description: str
    tags: _containers.RepeatedScalarFieldContainer[str]
    owner: str
    def __init__(self, name: _Optional[str] = ..., dimension: _Optional[int] = ..., distance_metric: _Optional[_Union[DistanceMetric, str]] = ..., storage_engine: _Optional[_Union[StorageEngine, str]] = ..., primary_indexing_algorithm: _Optional[_Union[IndexingAlgorithm, str]] = ..., filterable_columns: _Optional[_Iterable[_Union[FilterableColumnSpec, _Mapping]]] = ..., index_configs: _Optional[_Iterable[_Union[IndexConfig, _Mapping]]] = ..., quantization_config: _Optional[_Union[QuantizationConfig, _Mapping]] = ..., primary_index_name: _Optional[str] = ..., enable_automatic_index_selection: bool = ..., description: _Optional[str] = ..., tags: _Optional[_Iterable[str]] = ..., owner: _Optional[str] = ...) -> None: ...

class IndexConfig(_message.Message):
    __slots__ = ("index_name", "algorithm", "update_mode", "async_update_timeout_ms", "async_update_batch_size", "enable_background_optimization", "hnsw_config", "ivf_config", "flat_config", "pq_config", "annoy_config", "lsh_config", "build_concurrency", "memory_limit_mb", "checkpoint_interval_ms", "is_primary", "use_cases", "selectivity_threshold")
    INDEX_NAME_FIELD_NUMBER: _ClassVar[int]
    ALGORITHM_FIELD_NUMBER: _ClassVar[int]
    UPDATE_MODE_FIELD_NUMBER: _ClassVar[int]
    ASYNC_UPDATE_TIMEOUT_MS_FIELD_NUMBER: _ClassVar[int]
    ASYNC_UPDATE_BATCH_SIZE_FIELD_NUMBER: _ClassVar[int]
    ENABLE_BACKGROUND_OPTIMIZATION_FIELD_NUMBER: _ClassVar[int]
    HNSW_CONFIG_FIELD_NUMBER: _ClassVar[int]
    IVF_CONFIG_FIELD_NUMBER: _ClassVar[int]
    FLAT_CONFIG_FIELD_NUMBER: _ClassVar[int]
    PQ_CONFIG_FIELD_NUMBER: _ClassVar[int]
    ANNOY_CONFIG_FIELD_NUMBER: _ClassVar[int]
    LSH_CONFIG_FIELD_NUMBER: _ClassVar[int]
    BUILD_CONCURRENCY_FIELD_NUMBER: _ClassVar[int]
    MEMORY_LIMIT_MB_FIELD_NUMBER: _ClassVar[int]
    CHECKPOINT_INTERVAL_MS_FIELD_NUMBER: _ClassVar[int]
    IS_PRIMARY_FIELD_NUMBER: _ClassVar[int]
    USE_CASES_FIELD_NUMBER: _ClassVar[int]
    SELECTIVITY_THRESHOLD_FIELD_NUMBER: _ClassVar[int]
    index_name: str
    algorithm: IndexingAlgorithm
    update_mode: IndexUpdateMode
    async_update_timeout_ms: int
    async_update_batch_size: int
    enable_background_optimization: bool
    hnsw_config: HnswConfig
    ivf_config: IvfConfig
    flat_config: FlatConfig
    pq_config: PqConfig
    annoy_config: AnnoyConfig
    lsh_config: LshConfig
    build_concurrency: int
    memory_limit_mb: int
    checkpoint_interval_ms: int
    is_primary: bool
    use_cases: _containers.RepeatedScalarFieldContainer[str]
    selectivity_threshold: float
    def __init__(self, index_name: _Optional[str] = ..., algorithm: _Optional[_Union[IndexingAlgorithm, str]] = ..., update_mode: _Optional[_Union[IndexUpdateMode, str]] = ..., async_update_timeout_ms: _Optional[int] = ..., async_update_batch_size: _Optional[int] = ..., enable_background_optimization: bool = ..., hnsw_config: _Optional[_Union[HnswConfig, _Mapping]] = ..., ivf_config: _Optional[_Union[IvfConfig, _Mapping]] = ..., flat_config: _Optional[_Union[FlatConfig, _Mapping]] = ..., pq_config: _Optional[_Union[PqConfig, _Mapping]] = ..., annoy_config: _Optional[_Union[AnnoyConfig, _Mapping]] = ..., lsh_config: _Optional[_Union[LshConfig, _Mapping]] = ..., build_concurrency: _Optional[int] = ..., memory_limit_mb: _Optional[int] = ..., checkpoint_interval_ms: _Optional[int] = ..., is_primary: bool = ..., use_cases: _Optional[_Iterable[str]] = ..., selectivity_threshold: _Optional[float] = ...) -> None: ...

class HnswConfig(_message.Message):
    __slots__ = ("m", "ef_construction", "ef_search", "max_partition_size", "adaptive_parameters", "use_simd", "memory_limit_mb", "lazy_loading", "prune_connections", "level_multiplier")
    M_FIELD_NUMBER: _ClassVar[int]
    EF_CONSTRUCTION_FIELD_NUMBER: _ClassVar[int]
    EF_SEARCH_FIELD_NUMBER: _ClassVar[int]
    MAX_PARTITION_SIZE_FIELD_NUMBER: _ClassVar[int]
    ADAPTIVE_PARAMETERS_FIELD_NUMBER: _ClassVar[int]
    USE_SIMD_FIELD_NUMBER: _ClassVar[int]
    MEMORY_LIMIT_MB_FIELD_NUMBER: _ClassVar[int]
    LAZY_LOADING_FIELD_NUMBER: _ClassVar[int]
    PRUNE_CONNECTIONS_FIELD_NUMBER: _ClassVar[int]
    LEVEL_MULTIPLIER_FIELD_NUMBER: _ClassVar[int]
    m: int
    ef_construction: int
    ef_search: int
    max_partition_size: int
    adaptive_parameters: bool
    use_simd: bool
    memory_limit_mb: int
    lazy_loading: bool
    prune_connections: int
    level_multiplier: float
    def __init__(self, m: _Optional[int] = ..., ef_construction: _Optional[int] = ..., ef_search: _Optional[int] = ..., max_partition_size: _Optional[int] = ..., adaptive_parameters: bool = ..., use_simd: bool = ..., memory_limit_mb: _Optional[int] = ..., lazy_loading: bool = ..., prune_connections: _Optional[int] = ..., level_multiplier: _Optional[float] = ...) -> None: ...

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

class FlatConfig(_message.Message):
    __slots__ = ("enable_simd", "batch_size", "enable_parallel_search")
    ENABLE_SIMD_FIELD_NUMBER: _ClassVar[int]
    BATCH_SIZE_FIELD_NUMBER: _ClassVar[int]
    ENABLE_PARALLEL_SEARCH_FIELD_NUMBER: _ClassVar[int]
    enable_simd: bool
    batch_size: int
    enable_parallel_search: bool
    def __init__(self, enable_simd: bool = ..., batch_size: _Optional[int] = ..., enable_parallel_search: bool = ...) -> None: ...

class PqConfig(_message.Message):
    __slots__ = ("subvectors", "bits_per_subvector", "training_sample_count", "enable_reranking")
    SUBVECTORS_FIELD_NUMBER: _ClassVar[int]
    BITS_PER_SUBVECTOR_FIELD_NUMBER: _ClassVar[int]
    TRAINING_SAMPLE_COUNT_FIELD_NUMBER: _ClassVar[int]
    ENABLE_RERANKING_FIELD_NUMBER: _ClassVar[int]
    subvectors: int
    bits_per_subvector: int
    training_sample_count: int
    enable_reranking: bool
    def __init__(self, subvectors: _Optional[int] = ..., bits_per_subvector: _Optional[int] = ..., training_sample_count: _Optional[int] = ..., enable_reranking: bool = ...) -> None: ...

class AnnoyConfig(_message.Message):
    __slots__ = ("n_trees", "search_k", "max_leaf_size", "enable_mmap")
    N_TREES_FIELD_NUMBER: _ClassVar[int]
    SEARCH_K_FIELD_NUMBER: _ClassVar[int]
    MAX_LEAF_SIZE_FIELD_NUMBER: _ClassVar[int]
    ENABLE_MMAP_FIELD_NUMBER: _ClassVar[int]
    n_trees: int
    search_k: int
    max_leaf_size: int
    enable_mmap: bool
    def __init__(self, n_trees: _Optional[int] = ..., search_k: _Optional[int] = ..., max_leaf_size: _Optional[int] = ..., enable_mmap: bool = ...) -> None: ...

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
    projection: RandomProjectionType
    def __init__(self, n_hash_tables: _Optional[int] = ..., n_hash_functions: _Optional[int] = ..., bucket_width: _Optional[float] = ..., binary_vectors: bool = ..., max_candidates: _Optional[int] = ..., projection: _Optional[_Union[RandomProjectionType, str]] = ...) -> None: ...

class QuantizationConfig(_message.Message):
    __slots__ = ("enabled", "storage_quantization", "index_quantization", "search_quantization", "compression_ratio_target", "validation")
    ENABLED_FIELD_NUMBER: _ClassVar[int]
    STORAGE_QUANTIZATION_FIELD_NUMBER: _ClassVar[int]
    INDEX_QUANTIZATION_FIELD_NUMBER: _ClassVar[int]
    SEARCH_QUANTIZATION_FIELD_NUMBER: _ClassVar[int]
    COMPRESSION_RATIO_TARGET_FIELD_NUMBER: _ClassVar[int]
    VALIDATION_FIELD_NUMBER: _ClassVar[int]
    enabled: bool
    storage_quantization: StorageQuantizationConfig
    index_quantization: IndexQuantizationConfig
    search_quantization: SearchQuantizationConfig
    compression_ratio_target: float
    validation: QuantizationValidation
    def __init__(self, enabled: bool = ..., storage_quantization: _Optional[_Union[StorageQuantizationConfig, _Mapping]] = ..., index_quantization: _Optional[_Union[IndexQuantizationConfig, _Mapping]] = ..., search_quantization: _Optional[_Union[SearchQuantizationConfig, _Mapping]] = ..., compression_ratio_target: _Optional[float] = ..., validation: _Optional[_Union[QuantizationValidation, _Mapping]] = ...) -> None: ...

class StorageQuantizationConfig(_message.Message):
    __slots__ = ("enabled", "level", "codebook_id", "progressive_quantization", "storage_compatibility")
    ENABLED_FIELD_NUMBER: _ClassVar[int]
    LEVEL_FIELD_NUMBER: _ClassVar[int]
    CODEBOOK_ID_FIELD_NUMBER: _ClassVar[int]
    PROGRESSIVE_QUANTIZATION_FIELD_NUMBER: _ClassVar[int]
    STORAGE_COMPATIBILITY_FIELD_NUMBER: _ClassVar[int]
    enabled: bool
    level: QuantizationLevel
    codebook_id: str
    progressive_quantization: bool
    storage_compatibility: StorageEngineCompatibility
    def __init__(self, enabled: bool = ..., level: _Optional[_Union[QuantizationLevel, _Mapping]] = ..., codebook_id: _Optional[str] = ..., progressive_quantization: bool = ..., storage_compatibility: _Optional[_Union[StorageEngineCompatibility, str]] = ...) -> None: ...

class IndexQuantizationConfig(_message.Message):
    __slots__ = ("enabled", "strategies", "auto_select_strategy")
    ENABLED_FIELD_NUMBER: _ClassVar[int]
    STRATEGIES_FIELD_NUMBER: _ClassVar[int]
    AUTO_SELECT_STRATEGY_FIELD_NUMBER: _ClassVar[int]
    enabled: bool
    strategies: _containers.RepeatedCompositeFieldContainer[IndexQuantizationStrategy]
    auto_select_strategy: bool
    def __init__(self, enabled: bool = ..., strategies: _Optional[_Iterable[_Union[IndexQuantizationStrategy, _Mapping]]] = ..., auto_select_strategy: bool = ...) -> None: ...

class IndexQuantizationStrategy(_message.Message):
    __slots__ = ("index_name", "level", "build_async", "codebook_id")
    INDEX_NAME_FIELD_NUMBER: _ClassVar[int]
    LEVEL_FIELD_NUMBER: _ClassVar[int]
    BUILD_ASYNC_FIELD_NUMBER: _ClassVar[int]
    CODEBOOK_ID_FIELD_NUMBER: _ClassVar[int]
    index_name: str
    level: QuantizationLevel
    build_async: bool
    codebook_id: str
    def __init__(self, index_name: _Optional[str] = ..., level: _Optional[_Union[QuantizationLevel, _Mapping]] = ..., build_async: bool = ..., codebook_id: _Optional[str] = ...) -> None: ...

class SearchQuantizationConfig(_message.Message):
    __slots__ = ("enabled", "default_level", "adaptive_precision", "accuracy_threshold", "candidate_multiplier")
    ENABLED_FIELD_NUMBER: _ClassVar[int]
    DEFAULT_LEVEL_FIELD_NUMBER: _ClassVar[int]
    ADAPTIVE_PRECISION_FIELD_NUMBER: _ClassVar[int]
    ACCURACY_THRESHOLD_FIELD_NUMBER: _ClassVar[int]
    CANDIDATE_MULTIPLIER_FIELD_NUMBER: _ClassVar[int]
    enabled: bool
    default_level: QuantizationLevel
    adaptive_precision: bool
    accuracy_threshold: float
    candidate_multiplier: int
    def __init__(self, enabled: bool = ..., default_level: _Optional[_Union[QuantizationLevel, _Mapping]] = ..., adaptive_precision: bool = ..., accuracy_threshold: _Optional[float] = ..., candidate_multiplier: _Optional[int] = ...) -> None: ...

class QuantizationLevel(_message.Message):
    __slots__ = ("none", "uniform", "pq", "scalar", "binary", "custom")
    NONE_FIELD_NUMBER: _ClassVar[int]
    UNIFORM_FIELD_NUMBER: _ClassVar[int]
    PQ_FIELD_NUMBER: _ClassVar[int]
    SCALAR_FIELD_NUMBER: _ClassVar[int]
    BINARY_FIELD_NUMBER: _ClassVar[int]
    CUSTOM_FIELD_NUMBER: _ClassVar[int]
    none: NoQuantization
    uniform: UniformQuantization
    pq: ProductQuantization
    scalar: ScalarQuantization
    binary: BinaryQuantization
    custom: CustomQuantization
    def __init__(self, none: _Optional[_Union[NoQuantization, _Mapping]] = ..., uniform: _Optional[_Union[UniformQuantization, _Mapping]] = ..., pq: _Optional[_Union[ProductQuantization, _Mapping]] = ..., scalar: _Optional[_Union[ScalarQuantization, _Mapping]] = ..., binary: _Optional[_Union[BinaryQuantization, _Mapping]] = ..., custom: _Optional[_Union[CustomQuantization, _Mapping]] = ...) -> None: ...

class NoQuantization(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class UniformQuantization(_message.Message):
    __slots__ = ("bits", "scale", "offset")
    BITS_FIELD_NUMBER: _ClassVar[int]
    SCALE_FIELD_NUMBER: _ClassVar[int]
    OFFSET_FIELD_NUMBER: _ClassVar[int]
    bits: int
    scale: float
    offset: float
    def __init__(self, bits: _Optional[int] = ..., scale: _Optional[float] = ..., offset: _Optional[float] = ...) -> None: ...

class ProductQuantization(_message.Message):
    __slots__ = ("bits_per_code", "num_subvectors", "codebook_id", "adaptive_subvectors")
    BITS_PER_CODE_FIELD_NUMBER: _ClassVar[int]
    NUM_SUBVECTORS_FIELD_NUMBER: _ClassVar[int]
    CODEBOOK_ID_FIELD_NUMBER: _ClassVar[int]
    ADAPTIVE_SUBVECTORS_FIELD_NUMBER: _ClassVar[int]
    bits_per_code: int
    num_subvectors: int
    codebook_id: str
    adaptive_subvectors: bool
    def __init__(self, bits_per_code: _Optional[int] = ..., num_subvectors: _Optional[int] = ..., codebook_id: _Optional[str] = ..., adaptive_subvectors: bool = ...) -> None: ...

class ScalarQuantization(_message.Message):
    __slots__ = ("bits", "scale", "offset", "clamp_values")
    BITS_FIELD_NUMBER: _ClassVar[int]
    SCALE_FIELD_NUMBER: _ClassVar[int]
    OFFSET_FIELD_NUMBER: _ClassVar[int]
    CLAMP_VALUES_FIELD_NUMBER: _ClassVar[int]
    bits: int
    scale: float
    offset: float
    clamp_values: bool
    def __init__(self, bits: _Optional[int] = ..., scale: _Optional[float] = ..., offset: _Optional[float] = ..., clamp_values: bool = ...) -> None: ...

class BinaryQuantization(_message.Message):
    __slots__ = ("threshold", "sign_based")
    THRESHOLD_FIELD_NUMBER: _ClassVar[int]
    SIGN_BASED_FIELD_NUMBER: _ClassVar[int]
    threshold: float
    sign_based: bool
    def __init__(self, threshold: _Optional[float] = ..., sign_based: bool = ...) -> None: ...

class CustomQuantization(_message.Message):
    __slots__ = ("type_id", "bits_per_element", "config")
    class ConfigEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    TYPE_ID_FIELD_NUMBER: _ClassVar[int]
    BITS_PER_ELEMENT_FIELD_NUMBER: _ClassVar[int]
    CONFIG_FIELD_NUMBER: _ClassVar[int]
    type_id: str
    bits_per_element: int
    config: _containers.ScalarMap[str, str]
    def __init__(self, type_id: _Optional[str] = ..., bits_per_element: _Optional[int] = ..., config: _Optional[_Mapping[str, str]] = ...) -> None: ...

class QuantizationValidation(_message.Message):
    __slots__ = ("accuracy_threshold", "validation_sample_size", "enable_quality_monitoring", "retraining_threshold")
    ACCURACY_THRESHOLD_FIELD_NUMBER: _ClassVar[int]
    VALIDATION_SAMPLE_SIZE_FIELD_NUMBER: _ClassVar[int]
    ENABLE_QUALITY_MONITORING_FIELD_NUMBER: _ClassVar[int]
    RETRAINING_THRESHOLD_FIELD_NUMBER: _ClassVar[int]
    accuracy_threshold: float
    validation_sample_size: int
    enable_quality_monitoring: bool
    retraining_threshold: float
    def __init__(self, accuracy_threshold: _Optional[float] = ..., validation_sample_size: _Optional[int] = ..., enable_quality_monitoring: bool = ..., retraining_threshold: _Optional[float] = ...) -> None: ...

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

class Collection(_message.Message):
    __slots__ = ("id", "config", "stats", "created_at", "updated_at")
    ID_FIELD_NUMBER: _ClassVar[int]
    CONFIG_FIELD_NUMBER: _ClassVar[int]
    STATS_FIELD_NUMBER: _ClassVar[int]
    CREATED_AT_FIELD_NUMBER: _ClassVar[int]
    UPDATED_AT_FIELD_NUMBER: _ClassVar[int]
    id: str
    config: CollectionConfig
    stats: CollectionStats
    created_at: int
    updated_at: int
    def __init__(self, id: _Optional[str] = ..., config: _Optional[_Union[CollectionConfig, _Mapping]] = ..., stats: _Optional[_Union[CollectionStats, _Mapping]] = ..., created_at: _Optional[int] = ..., updated_at: _Optional[int] = ...) -> None: ...

class CollectionStats(_message.Message):
    __slots__ = ("vector_count", "index_size_bytes", "data_size_bytes")
    VECTOR_COUNT_FIELD_NUMBER: _ClassVar[int]
    INDEX_SIZE_BYTES_FIELD_NUMBER: _ClassVar[int]
    DATA_SIZE_BYTES_FIELD_NUMBER: _ClassVar[int]
    vector_count: int
    index_size_bytes: int
    data_size_bytes: int
    def __init__(self, vector_count: _Optional[int] = ..., index_size_bytes: _Optional[int] = ..., data_size_bytes: _Optional[int] = ...) -> None: ...

class SearchResult(_message.Message):
    __slots__ = ("id", "score", "vector", "metadata", "rank")
    ID_FIELD_NUMBER: _ClassVar[int]
    SCORE_FIELD_NUMBER: _ClassVar[int]
    VECTOR_FIELD_NUMBER: _ClassVar[int]
    METADATA_FIELD_NUMBER: _ClassVar[int]
    RANK_FIELD_NUMBER: _ClassVar[int]
    id: str
    score: float
    vector: _containers.RepeatedScalarFieldContainer[float]
    metadata: _containers.RepeatedCompositeFieldContainer[MetadataItem]
    rank: int
    def __init__(self, id: _Optional[str] = ..., score: _Optional[float] = ..., vector: _Optional[_Iterable[float]] = ..., metadata: _Optional[_Iterable[_Union[MetadataItem, _Mapping]]] = ..., rank: _Optional[int] = ...) -> None: ...

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
    operation: CollectionOperation
    collection_id: str
    collection_config: CollectionConfig
    query_params: _containers.ScalarMap[str, str]
    options: _containers.ScalarMap[str, bool]
    migration_config: _containers.ScalarMap[str, str]
    def __init__(self, operation: _Optional[_Union[CollectionOperation, str]] = ..., collection_id: _Optional[str] = ..., collection_config: _Optional[_Union[CollectionConfig, _Mapping]] = ..., query_params: _Optional[_Mapping[str, str]] = ..., options: _Optional[_Mapping[str, bool]] = ..., migration_config: _Optional[_Mapping[str, str]] = ...) -> None: ...

class CollectionResponse(_message.Message):
    __slots__ = ("success", "operation", "collection", "collections", "affected_count", "total_count", "metadata", "error_message", "error_code", "processing_time_us")
    class MetadataEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    SUCCESS_FIELD_NUMBER: _ClassVar[int]
    OPERATION_FIELD_NUMBER: _ClassVar[int]
    COLLECTION_FIELD_NUMBER: _ClassVar[int]
    COLLECTIONS_FIELD_NUMBER: _ClassVar[int]
    AFFECTED_COUNT_FIELD_NUMBER: _ClassVar[int]
    TOTAL_COUNT_FIELD_NUMBER: _ClassVar[int]
    METADATA_FIELD_NUMBER: _ClassVar[int]
    ERROR_MESSAGE_FIELD_NUMBER: _ClassVar[int]
    ERROR_CODE_FIELD_NUMBER: _ClassVar[int]
    PROCESSING_TIME_US_FIELD_NUMBER: _ClassVar[int]
    success: bool
    operation: CollectionOperation
    collection: Collection
    collections: _containers.RepeatedCompositeFieldContainer[Collection]
    affected_count: int
    total_count: int
    metadata: _containers.ScalarMap[str, str]
    error_message: str
    error_code: str
    processing_time_us: int
    def __init__(self, success: bool = ..., operation: _Optional[_Union[CollectionOperation, str]] = ..., collection: _Optional[_Union[Collection, _Mapping]] = ..., collections: _Optional[_Iterable[_Union[Collection, _Mapping]]] = ..., affected_count: _Optional[int] = ..., total_count: _Optional[int] = ..., metadata: _Optional[_Mapping[str, str]] = ..., error_message: _Optional[str] = ..., error_code: _Optional[str] = ..., processing_time_us: _Optional[int] = ...) -> None: ...

class VectorBatchRequest(_message.Message):
    __slots__ = ("collection_id", "vectors", "batch_timeout_ms", "request_id")
    COLLECTION_ID_FIELD_NUMBER: _ClassVar[int]
    VECTORS_FIELD_NUMBER: _ClassVar[int]
    BATCH_TIMEOUT_MS_FIELD_NUMBER: _ClassVar[int]
    REQUEST_ID_FIELD_NUMBER: _ClassVar[int]
    collection_id: str
    vectors: _containers.RepeatedCompositeFieldContainer[VectorRecord]
    batch_timeout_ms: int
    request_id: str
    def __init__(self, collection_id: _Optional[str] = ..., vectors: _Optional[_Iterable[_Union[VectorRecord, _Mapping]]] = ..., batch_timeout_ms: _Optional[int] = ..., request_id: _Optional[str] = ...) -> None: ...

class VectorSearchRequest(_message.Message):
    __slots__ = ("collection_id", "queries", "top_k", "distance_metric_override", "search_params", "include_fields", "search_optimization")
    COLLECTION_ID_FIELD_NUMBER: _ClassVar[int]
    QUERIES_FIELD_NUMBER: _ClassVar[int]
    TOP_K_FIELD_NUMBER: _ClassVar[int]
    DISTANCE_METRIC_OVERRIDE_FIELD_NUMBER: _ClassVar[int]
    SEARCH_PARAMS_FIELD_NUMBER: _ClassVar[int]
    INCLUDE_FIELDS_FIELD_NUMBER: _ClassVar[int]
    SEARCH_OPTIMIZATION_FIELD_NUMBER: _ClassVar[int]
    collection_id: str
    queries: _containers.RepeatedCompositeFieldContainer[SearchQuery]
    top_k: int
    distance_metric_override: DistanceMetric
    search_params: SearchParameters
    include_fields: IncludeFields
    search_optimization: SearchParams
    def __init__(self, collection_id: _Optional[str] = ..., queries: _Optional[_Iterable[_Union[SearchQuery, _Mapping]]] = ..., top_k: _Optional[int] = ..., distance_metric_override: _Optional[_Union[DistanceMetric, str]] = ..., search_params: _Optional[_Union[SearchParameters, _Mapping]] = ..., include_fields: _Optional[_Union[IncludeFields, _Mapping]] = ..., search_optimization: _Optional[_Union[SearchParams, _Mapping]] = ...) -> None: ...

class VectorGetRequest(_message.Message):
    __slots__ = ("collection_id", "vector_id", "include_fields")
    COLLECTION_ID_FIELD_NUMBER: _ClassVar[int]
    VECTOR_ID_FIELD_NUMBER: _ClassVar[int]
    INCLUDE_FIELDS_FIELD_NUMBER: _ClassVar[int]
    collection_id: str
    vector_id: str
    include_fields: IncludeFields
    def __init__(self, collection_id: _Optional[str] = ..., vector_id: _Optional[str] = ..., include_fields: _Optional[_Union[IncludeFields, _Mapping]] = ...) -> None: ...

class SearchParameters(_message.Message):
    __slots__ = ("ef_search", "max_connections", "n_probe", "enable_reranking", "batch_size", "timeout_ms", "accuracy_threshold", "enable_parallel_search", "thread_count")
    EF_SEARCH_FIELD_NUMBER: _ClassVar[int]
    MAX_CONNECTIONS_FIELD_NUMBER: _ClassVar[int]
    N_PROBE_FIELD_NUMBER: _ClassVar[int]
    ENABLE_RERANKING_FIELD_NUMBER: _ClassVar[int]
    BATCH_SIZE_FIELD_NUMBER: _ClassVar[int]
    TIMEOUT_MS_FIELD_NUMBER: _ClassVar[int]
    ACCURACY_THRESHOLD_FIELD_NUMBER: _ClassVar[int]
    ENABLE_PARALLEL_SEARCH_FIELD_NUMBER: _ClassVar[int]
    THREAD_COUNT_FIELD_NUMBER: _ClassVar[int]
    ef_search: int
    max_connections: int
    n_probe: int
    enable_reranking: bool
    batch_size: int
    timeout_ms: int
    accuracy_threshold: float
    enable_parallel_search: bool
    thread_count: int
    def __init__(self, ef_search: _Optional[int] = ..., max_connections: _Optional[int] = ..., n_probe: _Optional[int] = ..., enable_reranking: bool = ..., batch_size: _Optional[int] = ..., timeout_ms: _Optional[int] = ..., accuracy_threshold: _Optional[float] = ..., enable_parallel_search: bool = ..., thread_count: _Optional[int] = ...) -> None: ...

class SearchQuery(_message.Message):
    __slots__ = ("vector", "id", "metadata_filter")
    VECTOR_FIELD_NUMBER: _ClassVar[int]
    ID_FIELD_NUMBER: _ClassVar[int]
    METADATA_FILTER_FIELD_NUMBER: _ClassVar[int]
    vector: _containers.RepeatedScalarFieldContainer[float]
    id: str
    metadata_filter: MetadataFilter
    def __init__(self, vector: _Optional[_Iterable[float]] = ..., id: _Optional[str] = ..., metadata_filter: _Optional[_Union[MetadataFilter, _Mapping]] = ...) -> None: ...

class MetadataFilter(_message.Message):
    __slots__ = ("conditions", "operator")
    CONDITIONS_FIELD_NUMBER: _ClassVar[int]
    OPERATOR_FIELD_NUMBER: _ClassVar[int]
    conditions: _containers.RepeatedCompositeFieldContainer[FilterCondition]
    operator: FilterOperator
    def __init__(self, conditions: _Optional[_Iterable[_Union[FilterCondition, _Mapping]]] = ..., operator: _Optional[_Union[FilterOperator, str]] = ...) -> None: ...

class FilterCondition(_message.Message):
    __slots__ = ("field_name", "operation", "value")
    FIELD_NAME_FIELD_NUMBER: _ClassVar[int]
    OPERATION_FIELD_NUMBER: _ClassVar[int]
    VALUE_FIELD_NUMBER: _ClassVar[int]
    field_name: str
    operation: FilterOperation
    value: MetadataValue
    def __init__(self, field_name: _Optional[str] = ..., operation: _Optional[_Union[FilterOperation, str]] = ..., value: _Optional[_Union[MetadataValue, _Mapping]] = ...) -> None: ...

class IncludeFields(_message.Message):
    __slots__ = ("vector", "metadata", "score", "rank")
    VECTOR_FIELD_NUMBER: _ClassVar[int]
    METADATA_FIELD_NUMBER: _ClassVar[int]
    SCORE_FIELD_NUMBER: _ClassVar[int]
    RANK_FIELD_NUMBER: _ClassVar[int]
    vector: bool
    metadata: bool
    score: bool
    rank: bool
    def __init__(self, vector: bool = ..., metadata: bool = ..., score: bool = ..., rank: bool = ...) -> None: ...

class BinaryQuantizationParams(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class ScalarQuantizationParams(_message.Message):
    __slots__ = ("bits",)
    BITS_FIELD_NUMBER: _ClassVar[int]
    bits: int
    def __init__(self, bits: _Optional[int] = ...) -> None: ...

class ProductQuantizationParams(_message.Message):
    __slots__ = ("num_subvectors", "bits_per_code")
    NUM_SUBVECTORS_FIELD_NUMBER: _ClassVar[int]
    BITS_PER_CODE_FIELD_NUMBER: _ClassVar[int]
    num_subvectors: int
    bits_per_code: int
    def __init__(self, num_subvectors: _Optional[int] = ..., bits_per_code: _Optional[int] = ...) -> None: ...

class UniformQuantizationParams(_message.Message):
    __slots__ = ("scale", "offset")
    SCALE_FIELD_NUMBER: _ClassVar[int]
    OFFSET_FIELD_NUMBER: _ClassVar[int]
    scale: float
    offset: float
    def __init__(self, scale: _Optional[float] = ..., offset: _Optional[float] = ...) -> None: ...

class SearchParams(_message.Message):
    __slots__ = ("top_k", "filters", "accuracy_threshold", "include_expired", "timeout_ms", "enable_two_stage", "no_quantization", "binary", "scalar", "product", "uniform", "enable_clustering_hint", "enable_metadata_filtering_hint", "custom_hints")
    class FiltersEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: _struct_pb2.Value
        def __init__(self, key: _Optional[str] = ..., value: _Optional[_Union[_struct_pb2.Value, _Mapping]] = ...) -> None: ...
    class CustomHintsEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: _struct_pb2.Value
        def __init__(self, key: _Optional[str] = ..., value: _Optional[_Union[_struct_pb2.Value, _Mapping]] = ...) -> None: ...
    TOP_K_FIELD_NUMBER: _ClassVar[int]
    FILTERS_FIELD_NUMBER: _ClassVar[int]
    ACCURACY_THRESHOLD_FIELD_NUMBER: _ClassVar[int]
    INCLUDE_EXPIRED_FIELD_NUMBER: _ClassVar[int]
    TIMEOUT_MS_FIELD_NUMBER: _ClassVar[int]
    ENABLE_TWO_STAGE_FIELD_NUMBER: _ClassVar[int]
    NO_QUANTIZATION_FIELD_NUMBER: _ClassVar[int]
    BINARY_FIELD_NUMBER: _ClassVar[int]
    SCALAR_FIELD_NUMBER: _ClassVar[int]
    PRODUCT_FIELD_NUMBER: _ClassVar[int]
    UNIFORM_FIELD_NUMBER: _ClassVar[int]
    ENABLE_CLUSTERING_HINT_FIELD_NUMBER: _ClassVar[int]
    ENABLE_METADATA_FILTERING_HINT_FIELD_NUMBER: _ClassVar[int]
    CUSTOM_HINTS_FIELD_NUMBER: _ClassVar[int]
    top_k: int
    filters: _containers.MessageMap[str, _struct_pb2.Value]
    accuracy_threshold: float
    include_expired: bool
    timeout_ms: int
    enable_two_stage: bool
    no_quantization: bool
    binary: BinaryQuantizationParams
    scalar: ScalarQuantizationParams
    product: ProductQuantizationParams
    uniform: UniformQuantizationParams
    enable_clustering_hint: bool
    enable_metadata_filtering_hint: bool
    custom_hints: _containers.MessageMap[str, _struct_pb2.Value]
    def __init__(self, top_k: _Optional[int] = ..., filters: _Optional[_Mapping[str, _struct_pb2.Value]] = ..., accuracy_threshold: _Optional[float] = ..., include_expired: bool = ..., timeout_ms: _Optional[int] = ..., enable_two_stage: bool = ..., no_quantization: bool = ..., binary: _Optional[_Union[BinaryQuantizationParams, _Mapping]] = ..., scalar: _Optional[_Union[ScalarQuantizationParams, _Mapping]] = ..., product: _Optional[_Union[ProductQuantizationParams, _Mapping]] = ..., uniform: _Optional[_Union[UniformQuantizationParams, _Mapping]] = ..., enable_clustering_hint: bool = ..., enable_metadata_filtering_hint: bool = ..., custom_hints: _Optional[_Mapping[str, _struct_pb2.Value]] = ...) -> None: ...

class VectorOperationResponse(_message.Message):
    __slots__ = ("success", "operation", "metrics", "compact_results", "avro_results", "vector_ids", "error_message", "error_code", "result_info")
    SUCCESS_FIELD_NUMBER: _ClassVar[int]
    OPERATION_FIELD_NUMBER: _ClassVar[int]
    METRICS_FIELD_NUMBER: _ClassVar[int]
    COMPACT_RESULTS_FIELD_NUMBER: _ClassVar[int]
    AVRO_RESULTS_FIELD_NUMBER: _ClassVar[int]
    VECTOR_IDS_FIELD_NUMBER: _ClassVar[int]
    ERROR_MESSAGE_FIELD_NUMBER: _ClassVar[int]
    ERROR_CODE_FIELD_NUMBER: _ClassVar[int]
    RESULT_INFO_FIELD_NUMBER: _ClassVar[int]
    success: bool
    operation: VectorOperation
    metrics: OperationMetrics
    compact_results: SearchResultsCompact
    avro_results: bytes
    vector_ids: _containers.RepeatedScalarFieldContainer[str]
    error_message: str
    error_code: str
    result_info: ResultMetadata
    def __init__(self, success: bool = ..., operation: _Optional[_Union[VectorOperation, str]] = ..., metrics: _Optional[_Union[OperationMetrics, _Mapping]] = ..., compact_results: _Optional[_Union[SearchResultsCompact, _Mapping]] = ..., avro_results: _Optional[bytes] = ..., vector_ids: _Optional[_Iterable[str]] = ..., error_message: _Optional[str] = ..., error_code: _Optional[str] = ..., result_info: _Optional[_Union[ResultMetadata, _Mapping]] = ...) -> None: ...

class SearchResultsCompact(_message.Message):
    __slots__ = ("results", "total_found", "search_algorithm_used")
    RESULTS_FIELD_NUMBER: _ClassVar[int]
    TOTAL_FOUND_FIELD_NUMBER: _ClassVar[int]
    SEARCH_ALGORITHM_USED_FIELD_NUMBER: _ClassVar[int]
    results: _containers.RepeatedCompositeFieldContainer[SearchResult]
    total_found: int
    search_algorithm_used: str
    def __init__(self, results: _Optional[_Iterable[_Union[SearchResult, _Mapping]]] = ..., total_found: _Optional[int] = ..., search_algorithm_used: _Optional[str] = ...) -> None: ...

class ResultMetadata(_message.Message):
    __slots__ = ("result_count", "estimated_size_bytes", "is_avro_binary", "avro_schema_version")
    RESULT_COUNT_FIELD_NUMBER: _ClassVar[int]
    ESTIMATED_SIZE_BYTES_FIELD_NUMBER: _ClassVar[int]
    IS_AVRO_BINARY_FIELD_NUMBER: _ClassVar[int]
    AVRO_SCHEMA_VERSION_FIELD_NUMBER: _ClassVar[int]
    result_count: int
    estimated_size_bytes: int
    is_avro_binary: bool
    avro_schema_version: str
    def __init__(self, result_count: _Optional[int] = ..., estimated_size_bytes: _Optional[int] = ..., is_avro_binary: bool = ..., avro_schema_version: _Optional[str] = ...) -> None: ...

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

class HealthRequest(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class HealthResponse(_message.Message):
    __slots__ = ("status", "version", "uptime_seconds", "active_connections", "memory_usage_bytes", "storage_usage_bytes")
    STATUS_FIELD_NUMBER: _ClassVar[int]
    VERSION_FIELD_NUMBER: _ClassVar[int]
    UPTIME_SECONDS_FIELD_NUMBER: _ClassVar[int]
    ACTIVE_CONNECTIONS_FIELD_NUMBER: _ClassVar[int]
    MEMORY_USAGE_BYTES_FIELD_NUMBER: _ClassVar[int]
    STORAGE_USAGE_BYTES_FIELD_NUMBER: _ClassVar[int]
    status: str
    version: str
    uptime_seconds: int
    active_connections: int
    memory_usage_bytes: int
    storage_usage_bytes: int
    def __init__(self, status: _Optional[str] = ..., version: _Optional[str] = ..., uptime_seconds: _Optional[int] = ..., active_connections: _Optional[int] = ..., memory_usage_bytes: _Optional[int] = ..., storage_usage_bytes: _Optional[int] = ...) -> None: ...

class MetricsRequest(_message.Message):
    __slots__ = ("collection_id", "metric_names")
    COLLECTION_ID_FIELD_NUMBER: _ClassVar[int]
    METRIC_NAMES_FIELD_NUMBER: _ClassVar[int]
    collection_id: str
    metric_names: _containers.RepeatedScalarFieldContainer[str]
    def __init__(self, collection_id: _Optional[str] = ..., metric_names: _Optional[_Iterable[str]] = ...) -> None: ...

class MetricsResponse(_message.Message):
    __slots__ = ("metrics", "timestamp")
    class MetricsEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: float
        def __init__(self, key: _Optional[str] = ..., value: _Optional[float] = ...) -> None: ...
    METRICS_FIELD_NUMBER: _ClassVar[int]
    TIMESTAMP_FIELD_NUMBER: _ClassVar[int]
    metrics: _containers.ScalarMap[str, float]
    timestamp: int
    def __init__(self, metrics: _Optional[_Mapping[str, float]] = ..., timestamp: _Optional[int] = ...) -> None: ...

class CollectionSnapshot(_message.Message):
    __slots__ = ("collections", "version", "timestamp")
    COLLECTIONS_FIELD_NUMBER: _ClassVar[int]
    VERSION_FIELD_NUMBER: _ClassVar[int]
    TIMESTAMP_FIELD_NUMBER: _ClassVar[int]
    collections: _containers.RepeatedCompositeFieldContainer[Collection]
    version: int
    timestamp: int
    def __init__(self, collections: _Optional[_Iterable[_Union[Collection, _Mapping]]] = ..., version: _Optional[int] = ..., timestamp: _Optional[int] = ...) -> None: ...
