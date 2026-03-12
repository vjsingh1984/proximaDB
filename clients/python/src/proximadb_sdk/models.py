"""
ProximaDB Python Client - Data Models for REST API

These Pydantic models are designed to work with the ProximaDB REST API.
They align with the server-side REST handlers (not proto definitions).

Copyright 2025 ProximaDB

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
"""

from datetime import datetime
from enum import Enum
import math
from typing import Any, Dict, List, Optional, Union

import numpy as np
from pydantic import BaseModel, ConfigDict, Field, field_validator

# Type aliases for convenience
VectorArray = Union[List[List[float]], np.ndarray]
MetadataDict = Dict[str, Union[str, int, float, bool, List[Union[str, int, float]]]]
FilterDict = Dict[str, Any]


# ============================================================================
# ENUMS - String values for REST API
# ============================================================================


class DistanceMetric(str, Enum):
    """Distance metrics for REST API (string-based)

    Note: Server supports 13+ distance metrics. Unsupported metrics
    will fallback to COSINE (server behavior, not client validation).

    For gRPC: Use DistanceMetricType enum with integer values instead.
    """

    COSINE = "cosine"
    EUCLIDEAN = "euclidean"
    DOT_PRODUCT = "dot_product"
    MANHATTAN = "manhattan"  # Fallback: COSINE (server decides)
    HAMMING = "hamming"  # Fallback: COSINE (server decides)
    JACCARD = "jaccard"  # Fallback: COSINE (server decides)

    # Extended metrics supported by server (ProximaDB v1.0+)
    CHEBYSHEV = "chebyshev"
    CANBERRA = "canberra"
    MINKOWSKI = "minkowski"
    ANGULAR = "angular"
    BRAY_CURTIS = "bray_curtis"
    HELLINGER = "hellinger"
    CUSTOM = "custom"


class DistanceMetricType(int, Enum):
    """Distance metric types for gRPC API (integer-based, matches v1 proto)

    Maps to proximadb.v1.types_pb2.DistanceMetric enum.
    Use these constants when working with gRPC to avoid magic numbers.

    Example:
        config = CollectionConfig(distance_metric=DistanceMetricType.COSINE, ...)
    """

    UNSPECIFIED = 0
    COSINE = 1
    EUCLIDEAN = 2
    DOT_PRODUCT = 3
    HAMMING = 4
    MANHATTAN = 5
    JACCARD = 6
    CHEBYSHEV = 7
    CANBERRA = 8
    MINKOWSKI = 9
    ANGULAR = 10
    BRAY_CURTIS = 11
    HELLINGER = 12
    CUSTOM = 13


class StorageEngine(str, Enum):
    """Storage engines for ProximaDB

    ProximaDB supports 6 specialized storage engines that auto-optimize for different workloads.

    Default: VIPER (server default, columnar analytics engine)
    """

    VIPER = "viper"  # Columnar Parquet format with advanced quantization (DEFAULT)
    SST = "sst"  # Row-based, write-optimized with three-stage filtering
    NOVA = "nova"  # Progressive columnar storage with multi-level quantization
    HELIX = "helix"  # Locality-optimized storage with Hilbert curve clustering
    SWIFT = "swift"  # High-speed row-based with FastLanes encoding
    RAPTOR = "raptor"  # Adaptive row-group management with PxK optimization
    MMAP = "mmap"  # Memory-mapped storage
    HYBRID = "hybrid"  # Hybrid storage approach


class IndexingAlgorithm(str, Enum):
    """Indexing algorithms for REST API (string-based)

    Note: All 6 indexing algorithms are fully supported by the server as of 2025-08.
    No fallbacks required - server handles all algorithms natively.

    For gRPC: Use IndexType enum with integer values instead.
    """

    HNSW = "hnsw"  # Hierarchical Navigable Small World
    IVF = "ivf"  # Inverted File Index
    PQ = "pq"  # Product Quantization
    FLAT = "flat"  # Brute force / exhaustive search
    ANNOY = "annoy"  # Approximate Nearest Neighbors Oh Yeah
    LSH = "lsh"  # Locality Sensitive Hashing (fully supported)


class IndexType(int, Enum):
    """Index types for gRPC API (integer-based, matches v1 proto)

    Maps to proximadb.v1.collection_types_pb2.IndexConfig.algorithm field.
    Use these constants when working with gRPC to avoid magic numbers.

    Example:
        ic = IndexConfig(algorithm=IndexType.HNSW, index_name="primary")
    """

    UNSPECIFIED = 0
    HNSW = 1  # Hierarchical Navigable Small World
    IVF = 2  # Inverted File Index
    PQ = 3  # Product Quantization
    FLAT = 4  # Brute force / exhaustive search
    ANNOY = 5  # Approximate Nearest Neighbors Oh Yeah
    LSH = 6  # Locality Sensitive Hashing


class StorageEngineType(int, Enum):
    """Storage engine types for gRPC API (integer-based, matches v1 proto)

    Maps to proximadb.v1.types_pb2.StorageEngine enum.
    Use these constants when working with gRPC to avoid magic numbers.

    ProximaDB supports 6 specialized storage engines that auto-optimize for different workloads.

    Example:
        config = CollectionConfig(storage_engine=StorageEngineType.VIPER, ...)
    """

    UNSPECIFIED = 0
    VIPER = 1  # Columnar Parquet format with advanced quantization (DEFAULT)
    SST = 2  # Row-based, write-optimized with three-stage filtering
    NOVA = 3  # Progressive columnar storage with multi-level quantization
    HELIX = 4  # Locality-optimized storage with Hilbert curve clustering
    SWIFT = 5  # High-speed row-based with FastLanes encoding
    RAPTOR = 6  # Adaptive row-group management with PxK optimization


class IndexUpdateMode(str, Enum):
    """Index update modes"""

    SYNCHRONOUS = "synchronous"
    ASYNCHRONOUS = "asynchronous"
    HYBRID_MODE = "hybrid_mode"


class FilterableDataType(str, Enum):
    """Filterable data types"""

    STRING = "string"
    INTEGER = "integer"
    FLOAT = "float"
    BOOLEAN = "boolean"
    DATETIME = "datetime"
    ARRAY_STRING = "array_string"
    ARRAY_INTEGER = "array_integer"
    ARRAY_FLOAT = "array_float"


class CompressionAlgorithm(str, Enum):
    """Compression algorithms for SDK-driven compression"""

    NONE = "none"
    ZSTD = "zstd"
    LZ4 = "lz4"
    SNAPPY = "snappy"


class CompressionLevel(int, Enum):
    """Compression levels (1-9 for ZSTD, ignored for others)"""

    FASTEST = 1
    FAST = 3
    BALANCED = 6
    HIGH = 9


class ServerCapabilities(BaseModel):
    """Server capabilities and fallback behavior for configuration validation"""

    # Fully supported distance metrics (no fallback) - All 13 metrics supported as of 2025-08
    supported_distance_metrics: List[str] = [
        "cosine",
        "euclidean",
        "dot_product",
        "manhattan",
        "hamming",
        "jaccard",
        "chebyshev",
        "canberra",
        "minkowski",
        "angular",
        "bray_curtis",
        "hellinger",
        "custom",
    ]

    # Distance metrics that fallback to cosine (none - all are now supported natively)
    fallback_distance_metrics: List[str] = []

    # Fully supported storage engines (no fallback)
    supported_storage_engines: List[str] = ["viper", "sst"]

    # Storage engines that fallback to viper
    fallback_storage_engines: List[str] = ["mmap", "hybrid"]

    # Fully supported indexing algorithms (no fallback) - All 6 algorithms supported as of 2025-08
    supported_indexing_algorithms: List[str] = [
        "hnsw",
        "ivf",
        "pq",
        "flat",
        "annoy",
        "lsh",
    ]

    # Indexing algorithms that fallback to hnsw (none - all are now supported natively)
    fallback_indexing_algorithms: List[str] = []

    # Quantization types (all supported in VIPER engine)
    supported_quantization_types: List[str] = [
        "none",
        "uniform",
        "pq",
        "scalar",
        "binary",
        "custom",
    ]

    # Server behavior notes
    notes: Dict[str, str] = {
        "fallback_policy": "Server uses intelligent fallbacks instead of errors",
        "dimension_limit": "Server default maximum is 65536 dimensions (configurable)",
        "name_validation": "Collection names must be 8+ characters to avoid collision with 7-char base62 IDs",
        "quantization_engine": "Quantization fully supported in VIPER engine only",
        "filterable_columns": "All FilterableDataType values supported",
    }

    @classmethod
    def get_fallback_for(cls, config_type: str, value: str) -> Optional[str]:
        """Get the fallback value for an unsupported configuration"""
        capabilities = cls()

        if config_type == "distance_metric":
            if value in capabilities.fallback_distance_metrics:
                return "cosine"
        elif config_type == "storage_engine":
            if value in capabilities.fallback_storage_engines:
                return "viper"
        elif config_type == "indexing_algorithm":
            if value in capabilities.fallback_indexing_algorithms:
                return "hnsw"

        return None

    @classmethod
    def is_supported(cls, config_type: str, value: str) -> bool:
        """Check if a configuration value is fully supported (no fallback)"""
        capabilities = cls()

        if config_type == "distance_metric":
            return value in capabilities.supported_distance_metrics
        elif config_type == "storage_engine":
            return value in capabilities.supported_storage_engines
        elif config_type == "indexing_algorithm":
            return value in capabilities.supported_indexing_algorithms
        elif config_type == "quantization_type":
            return value in capabilities.supported_quantization_types

        return True  # Default to supported for unknown types


class CollectionOperationType(str, Enum):
    """Collection operation types"""

    CREATE = "create"
    UPDATE = "update"
    GET = "get"
    LIST = "list"
    DELETE = "delete"
    MIGRATE = "migrate"


class VectorOperationType(str, Enum):
    """Vector operation types"""

    INSERT = "insert"
    UPSERT = "upsert"
    UPDATE = "update"
    DELETE = "delete"
    SEARCH = "search"
    GET = "get"


class FilterOperator(str, Enum):
    """Filter operators"""

    AND = "and"
    OR = "or"
    NOT = "not"


class FilterOperation(str, Enum):
    """Filter operations"""

    EQUALS = "equals"
    NOT_EQUALS = "not_equals"
    GREATER_THAN = "greater_than"
    GREATER_THAN_OR_EQUAL = "greater_than_or_equal"
    LESS_THAN = "less_than"
    LESS_THAN_OR_EQUAL = "less_than_or_equal"
    IN = "in"
    NOT_IN = "not_in"
    CONTAINS = "contains"
    NOT_CONTAINS = "not_contains"
    STARTS_WITH = "starts_with"
    ENDS_WITH = "ends_with"


# ============================================================================
# QUANTIZATION MODELS
# ============================================================================


class QuantizationType(str, Enum):
    """Quantization types for REST API"""

    NONE = "none"
    UNIFORM = "uniform"
    PRODUCT = "pq"
    SCALAR = "scalar"
    BINARY = "binary"
    CUSTOM = "custom"


class QuantizationLevel(BaseModel):
    """Quantization level configuration"""

    level_type: str  # "none", "uniform", "pq", "scalar", "binary", "custom"
    bits: Optional[int] = None
    scale: Optional[float] = None
    offset: Optional[float] = None
    num_subvectors: Optional[int] = None
    bits_per_code: Optional[int] = None
    codebook_id: Optional[str] = None
    adaptive_subvectors: Optional[bool] = None
    threshold: Optional[float] = None
    sign_based: Optional[bool] = None
    clamp_values: Optional[bool] = None
    type_id: Optional[str] = None
    bits_per_element: Optional[int] = None
    config: Optional[Dict[str, str]] = None


class StorageQuantizationConfig(BaseModel):
    """Storage quantization configuration"""

    enabled: bool = False
    level: Optional[QuantizationLevel] = None
    codebook_id: Optional[str] = None
    progressive_quantization: bool = False
    storage_compatibility: str = "VIPER_ONLY"


class IndexQuantizationStrategy(BaseModel):
    """Index quantization strategy"""

    index_name: str
    level: QuantizationLevel
    build_async: bool = False
    codebook_id: Optional[str] = None


class IndexQuantizationConfig(BaseModel):
    """Index quantization configuration"""

    enabled: bool = False
    strategies: List[IndexQuantizationStrategy] = Field(default_factory=list)
    auto_select_strategy: bool = False


class SearchQuantizationConfig(BaseModel):
    """Search quantization configuration"""

    enabled: bool = False
    default_level: Optional[QuantizationLevel] = None
    adaptive_precision: bool = True
    accuracy_threshold: float = 0.95
    candidate_multiplier: int = 3


class QuantizationValidation(BaseModel):
    """Quantization validation configuration"""

    accuracy_threshold: float = 0.95
    validation_sample_size: int = 1000
    enable_quality_monitoring: bool = True
    retraining_threshold: float = 0.90


class CompressionConfig(BaseModel):
    """Unified compression configuration matching proto definition

    This configuration aligns with the proto CompressionConfig message,
    providing a unified interface where engine-specific features are
    automatically applied based on the collection's storage_engine.

    Defaults are optimized for VIPER engine (server default).
    """

    # Unified compression settings (proto fields 1-2)
    algorithm: CompressionAlgorithm = Field(
        default=CompressionAlgorithm.NONE,
        description="Compression algorithm (ZSTD/LZ4/Snappy)",
    )
    level: Optional[int] = Field(
        default=None, description="Compression level (1-22 for ZSTD, 1-9 for others)"
    )

    # Global settings (proto field 3-4)
    adaptive: bool = Field(
        default=False,
        description="Enable adaptive compression based on data characteristics",
    )
    min_ratio: Optional[float] = Field(
        default=None,
        description="Minimum compression ratio (e.g., 1.5 = 50% reduction)",
    )

    # VIPER-specific quantization (proto fields 5-7)
    enable_quantization: bool = Field(
        default=False,
        description="Enable VIPER dual columns (FP32 + quantized). Ignored by SST engine.",
    )
    quantization_type: Optional[str] = Field(
        default=None,
        description="VIPER quantization method: 'int8', 'pq8', 'pq4'. Ignored by SST engine.",
    )
    normalization_method: Optional[str] = Field(
        default=None,
        description="VIPER normalization: 'mean', 'trimmed_mean', 'median'. Ignored by SST engine.",
    )

    # SST-specific block sizing (proto fields 8-9)
    block_size_kb: Optional[int] = Field(
        default=None,
        description="SST block size in KB (256-16384). Ignored by VIPER engine.",
    )
    dynamic_block_sizing: bool = Field(
        default=False,
        description="Auto-adjust SST block size based on vector dimensions. Ignored by VIPER engine.",
    )

    @field_validator("level")
    def validate_compression_level(cls, v):
        """Validate compression level matches server-side validation"""
        if v is not None and (v < 1 or v > 22):
            raise ValueError(
                "Compression level must be between 1-22 (1-9 for most algorithms, 1-22 for ZSTD)"
            )
        return v

    @field_validator("min_ratio")
    def validate_compression_ratio(cls, v):
        """Validate compression ratio matches server-side validation"""
        if v is not None and (v < 0.0 or v > 1.0):
            raise ValueError("Minimum compression ratio must be between 0.0 and 1.0")
        return v

    @field_validator("quantization_type")
    def validate_quantization_type(cls, v):
        """Validate quantization type is supported"""
        if v is not None:
            valid_types = {
                "int8",
                "pq8",
                "pq4",
                "uniform",
                "pq",
                "scalar",
                "binary",
                "none",
            }
            if v not in valid_types:
                raise ValueError(
                    f"Quantization type must be one of: {', '.join(valid_types)}"
                )
        return v

    @field_validator("normalization_method")
    def validate_normalization_method(cls, v):
        """Validate normalization method is supported"""
        if v is not None:
            valid_methods = {"mean", "trimmed_mean", "median", "none"}
            if v not in valid_methods:
                raise ValueError(
                    f"Normalization method must be one of: {', '.join(valid_methods)}"
                )
        return v

    @field_validator("block_size_kb")
    def validate_block_size(cls, v):
        """Validate SST block size is within acceptable range"""
        if v is not None and (v < 256 or v > 16384):
            raise ValueError("SST block size must be between 256-16384 KB")
        return v


class QuantizationConfig(BaseModel):
    """Quantization configuration"""

    enabled: bool = False
    type: QuantizationType = QuantizationType.NONE
    progressive_quantization: bool = False

    # Product quantization params
    bits_per_subvector: Optional[int] = None
    num_subvectors: Optional[int] = None

    # Scalar quantization params
    bits_per_vector: Optional[int] = None

    # Binary quantization params
    threshold: Optional[float] = None

    # Common params
    accuracy_threshold: Optional[float] = 0.95
    compression_ratio_target: Optional[float] = None
    validation_sample_size: Optional[int] = 1000
    retraining_threshold: Optional[float] = 0.90


class ComprehensiveQuantizationConfig(BaseModel):
    """Comprehensive quantization configuration matching proto structure"""

    enabled: bool = False
    storage_quantization: Optional[StorageQuantizationConfig] = None
    index_quantization: Optional[IndexQuantizationConfig] = None
    search_quantization: Optional[SearchQuantizationConfig] = None
    compression_ratio_target: Optional[float] = None
    validation: Optional[QuantizationValidation] = None


# ============================================================================
# STORAGE ENGINE CONFIGURATION MODELS
# ============================================================================


class AccessPattern(str, Enum):
    """Access patterns for storage optimization"""

    UNKNOWN = "unknown"
    WRITE_HEAVY = "write_heavy"
    READ_HEAVY = "read_heavy"
    BALANCED = "balanced"
    ARCHIVE = "archive"


class DataDensity(str, Enum):
    """Data density characteristics"""

    UNKNOWN = "unknown"
    DENSE = "dense"  # >80% non-zero values
    SPARSE = "sparse"  # <20% non-zero values
    MIXED = "mixed"


class ParquetWriterSettings(BaseModel):
    """Parquet writer settings for columnar engines"""

    row_group_size: Optional[int] = None
    page_size: Optional[int] = None
    enable_bloom_filters: Optional[bool] = None
    bloom_filter_fpp: Optional[float] = None
    bloom_filter_columns: Optional[List[str]] = None
    enable_column_statistics: Optional[bool] = None
    enable_page_index: Optional[bool] = None
    enable_column_index: Optional[bool] = None
    enable_offset_index: Optional[bool] = None
    page_index_granularity: Optional[int] = None
    enable_dictionary: Optional[bool] = None
    dictionary_threshold: Optional[float] = None
    enable_delta_encoding: Optional[bool] = None
    enable_byte_stream_split: Optional[bool] = None
    enable_pq_sorting: Optional[bool] = None
    pq_sorting_segments: Optional[int] = None
    pq_sorting_codebook_size: Optional[int] = None
    enable_native_metadata: Optional[bool] = None
    metadata_inference_samples: Optional[int] = None
    write_batch_size: Optional[int] = None
    id_less_storage: Optional[bool] = None


class FooterCacheSettings(BaseModel):
    """Footer cache settings for cloud storage optimization"""

    enable: Optional[bool] = None
    max_entries: Optional[int] = None
    ttl_seconds: Optional[int] = None
    time_to_idle_seconds: Optional[int] = None
    enable_persistence: Optional[bool] = None
    persistence_path: Optional[str] = None
    enable_prefetch: Optional[bool] = None
    prefetch_threshold: Optional[int] = None
    warming_interval_seconds: Optional[int] = None
    enable_compression: Optional[bool] = None
    compression_level: Optional[int] = None


class HybridWriterSettings(BaseModel):
    """Hybrid writer settings for adaptive performance"""

    enable: Optional[bool] = None
    initial_mode: Optional[str] = None  # "streaming", "batch", "adaptive"
    enable_auto_switch: Optional[bool] = None
    mode_switch_threshold: Optional[int] = None
    pattern_window_size: Optional[int] = None
    streaming_threshold: Optional[float] = None
    batch_threshold: Optional[int] = None
    max_buffer_size: Optional[int] = None
    buffer_time_limit_seconds: Optional[int] = None
    enable_concurrent_writes: Optional[bool] = None
    max_concurrent_writers: Optional[int] = None
    optimize_row_group_size: Optional[bool] = None
    min_row_group_size: Optional[int] = None
    max_row_group_size: Optional[int] = None


class SstEngineSettings(BaseModel):
    """SST-specific engine settings"""

    enable_bloom_filters: Optional[bool] = None
    bloom_filter_fpp: Optional[float] = None
    compression: Optional[CompressionAlgorithm] = None
    compression_level: Optional[int] = None
    write_buffer_size: Optional[int] = None
    max_write_buffers: Optional[int] = None
    block_size_kb: Optional[int] = None
    dynamic_block_sizing: Optional[bool] = None


class ViperEngineSettings(BaseModel):
    """VIPER-specific engine settings"""

    inherit_global_settings: Optional[bool] = None
    enable_columnar_compression: Optional[bool] = None
    enable_vector_quantization: Optional[bool] = None
    vector_chunk_size: Optional[int] = None
    enable_lazy_loading: Optional[bool] = None


class NovaEngineSettings(BaseModel):
    """NOVA-specific engine settings"""

    inherit_global_settings: Optional[bool] = None
    enable_real_time_mode: Optional[bool] = None
    streaming_buffer_size: Optional[int] = None
    prefer_low_latency: Optional[bool] = None


# Note: StorageEngineConfig is deprecated, use StorageConfig instead


# ============================================================================
# INDEX CONFIGURATION MODELS
# ============================================================================


class HnswConfig(BaseModel):
    """HNSW index configuration"""

    m: int = 16
    ef_construction: int = 200
    ef_search: int = 50
    max_partition_size: int = 100000
    adaptive_parameters: bool = True
    use_simd: bool = True
    memory_limit_mb: int = 512
    lazy_loading: bool = True
    prune_connections: int = 0
    level_multiplier: float = 0.69


class IvfConfig(BaseModel):
    """IVF index configuration"""

    n_lists: int = 100
    n_probe: int = 1
    quantization_bits: int = 8
    use_pq: bool = False
    pq_subspaces: int = 8
    train_on_insert: bool = False
    min_train_size: int = 1000


class FlatConfig(BaseModel):
    """Flat index configuration"""

    enable_simd: bool = True
    batch_size: int = 1000
    enable_parallel_search: bool = True


class PqConfig(BaseModel):
    """Product Quantization index configuration"""

    subvectors: int = 8
    bits_per_subvector: int = 8
    training_sample_count: int = 10000
    enable_reranking: bool = True


class AnnoyConfig(BaseModel):
    """Annoy index configuration"""

    n_trees: int = 10
    search_k: int = -1
    max_leaf_size: int = 100
    enable_mmap: bool = True


class RandomProjectionType(str, Enum):
    """Random projection types for LSH"""

    GAUSSIAN = "gaussian"
    BINARY = "binary"
    SPARSE = "sparse"


class LshConfig(BaseModel):
    """LSH index configuration"""

    n_hash_tables: int = 10
    n_hash_functions: int = 8
    bucket_width: float = 4.0
    binary_vectors: bool = False
    max_candidates: int = 100
    projection: RandomProjectionType = RandomProjectionType.GAUSSIAN


class IndexConfiguration(BaseModel):
    """Index configuration"""

    index_name: str
    algorithm: Union[IndexingAlgorithm, IndexType]
    update_mode: IndexUpdateMode = IndexUpdateMode.SYNCHRONOUS
    async_update_timeout_ms: Optional[int] = None
    async_update_batch_size: Optional[int] = None
    enable_background_optimization: Optional[bool] = None
    hnsw_config: Optional[HnswConfig] = None
    ivf_config: Optional[IvfConfig] = None
    flat_config: Optional[FlatConfig] = None
    pq_config: Optional[PqConfig] = None
    annoy_config: Optional[AnnoyConfig] = None
    lsh_config: Optional[LshConfig] = None
    build_concurrency: Optional[int] = None
    memory_limit_mb: Optional[int] = None
    checkpoint_interval_ms: Optional[int] = None
    is_primary: Optional[bool] = None
    use_cases: Optional[List[str]] = None
    selectivity_threshold: Optional[float] = None


# ============================================================================
# COLLECTION MODELS
# ============================================================================


class FilterableColumn(BaseModel):
    """Filterable column specification"""

    name: str
    data_type: FilterableDataType
    indexed: bool = True
    supports_range: bool = False
    estimated_cardinality: Optional[int] = None


class CollectionConfig(BaseModel):
    """Collection configuration aligned with proto CollectionConfig"""

    model_config = ConfigDict(populate_by_name=True)

    # CORE CONFIGURATION (Required)
    name: str = Field(
        min_length=8
    )  # Minimum 8 characters to prevent collision with 7-char base62 IDs
    dimension: int = Field(
        ge=1, le=65536
    )  # Server default maximum is 65536 (configurable)
    distance_metric: Optional[DistanceMetric] = (
        DistanceMetric.COSINE
    )  # Default to most common metric

    # STORAGE CONFIGURATION
    storage_engine: Optional[StorageEngine] = (
        StorageEngine.SST
    )  # Default to SST (fast, production-ready)
    storage_config: Optional["StorageConfig"] = None  # Complete storage configuration
    compression: Optional["CompressionConfig"] = (
        None  # Optional compression configuration (SDK convenience)
    )

    # INDEX CONFIGURATION
    index_configs: Optional[List[IndexConfiguration]] = None
    primary_index: Optional[str] = None  # Primary index name
    auto_index_selection: Optional[bool] = None  # Auto-select best index

    # SCHEMA CONFIGURATION
    filterable_columns: Optional[List[FilterableColumn]] = None
    quantization_config: Optional[QuantizationConfig] = Field(
        None, alias="quantization"
    )  # Vector quantization configuration
    primary_indexing_algorithm: Optional[IndexingAlgorithm] = (
        None  # Primary indexing algorithm
    )

    @property
    def quantization(self):
        """Alias property for backward compatibility"""
        return self.quantization_config

    # METADATA
    description: Optional[str] = None
    tags: Optional[List[str]] = None
    owner: Optional[str] = None

    # Additional Python SDK fields
    metadata_schema: Optional[Dict[str, Any]] = None
    filterable_metadata_fields: Optional[List[str]] = None

    @field_validator("name")
    def validate_name_length(cls, v):
        """Validate collection name is at least 8 characters to prevent collision with 7-char base62 IDs"""
        if not v or not v.strip():
            raise ValueError("Collection name cannot be empty")
        v = v.strip()
        if len(v) < 8:
            raise ValueError(
                "Collection name must be at least 8 characters long to prevent collision with 7-character base62 collection IDs"
            )
        return v

    def model_post_init(self, __context):
        """Post-initialization validation to align compression config with storage engine"""
        (
            super().model_post_init(__context)
            if hasattr(super(), "model_post_init")
            else None
        )

        # Apply engine-specific compression defaults and validation
        if self.compression:
            self._validate_compression_for_engine()

    def _validate_compression_for_engine(self):
        """Validate compression configuration aligns with storage engine capabilities"""
        if not self.compression:
            return

        engine = self.storage_engine or StorageEngine.VIPER  # Use server default

        if engine == StorageEngine.VIPER:
            # VIPER engine: quantization features are valid
            if self.compression.block_size_kb is not None:
                import warnings

                warnings.warn(
                    "block_size_kb is ignored by VIPER engine (only applies to SST engine)",
                    UserWarning,
                    stacklevel=2,
                )

            # Apply VIPER-optimized defaults
            if (
                self.compression.quantization_type
                and not self.compression.enable_quantization
            ):
                # Auto-enable quantization if type is specified
                self.compression.enable_quantization = True

        elif engine == StorageEngine.SST:
            # SST engine: quantization features are ignored
            if self.compression.enable_quantization:
                import warnings

                warnings.warn(
                    "enable_quantization is ignored by SST engine (only applies to VIPER engine)",
                    UserWarning,
                    stacklevel=2,
                )

            if self.compression.quantization_type:
                import warnings

                warnings.warn(
                    "quantization_type is ignored by SST engine (only applies to VIPER engine)",
                    UserWarning,
                    stacklevel=2,
                )

            # Apply SST-optimized defaults
            if self.compression.block_size_kb is None:
                self.compression.block_size_kb = 8192  # Reasonable default for SST

        # Validate algorithm-level compatibility
        if self.compression.level is not None:
            algo = self.compression.algorithm
            if algo == CompressionAlgorithm.ZSTD and self.compression.level > 22:
                raise ValueError("ZSTD compression level must be between 1-22")
            elif algo != CompressionAlgorithm.ZSTD and self.compression.level > 9:
                raise ValueError(f"{algo.value} compression level must be between 1-9")

    @property
    def index_config(self):
        """Backward compatibility property for singular index_config access"""
        if self.index_configs and len(self.index_configs) > 0:
            return self.index_configs[0]
        return None


class CollectionStats(BaseModel):
    """Collection statistics"""

    vector_count: int = 0
    index_size_bytes: int = 0
    data_size_bytes: int = 0


class CollectionInfo(BaseModel):
    """Collection info for list response"""

    id: str
    name: str
    dimension: int
    metric: str
    created_at_ms: int  # Milliseconds since epoch (signed int64)
    updated_at_ms: int  # Milliseconds since epoch (signed int64)
    vector_count: Optional[int] = None
    indexed: bool = False

    # Backward compatibility properties
    @property
    def created_at(self) -> int:
        """Backward compatibility: created_at in seconds"""
        return self.created_at_ms // 1000

    @created_at.setter
    def created_at(self, value: int):
        """Backward compatibility: created_at in seconds"""
        self.created_at_ms = value * 1000

    @property
    def updated_at(self) -> int:
        """Backward compatibility: updated_at in seconds"""
        return self.updated_at_ms // 1000

    @updated_at.setter
    def updated_at(self, value: int):
        """Backward compatibility: updated_at in seconds"""
        self.updated_at_ms = value * 1000


class Collection(BaseModel):
    """Collection information"""

    id: str
    config: CollectionConfig
    stats: CollectionStats = Field(
        default_factory=CollectionStats
    )  # Made required to match REST API
    created_at_ms: int = Field(
        default_factory=lambda: int(__import__("time").time() * 1000)
    )  # Milliseconds since epoch (signed int64)
    updated_at_ms: int = Field(
        default_factory=lambda: int(__import__("time").time() * 1000)
    )  # Milliseconds since epoch (signed int64)

    @property
    def name(self) -> str:
        """Backward compatibility property for collection name"""
        return self.config.name

    @property
    def timestamp(self) -> int:
        """Backward compatibility property for timestamp (seconds)"""
        return self.created_at_ms // 1000

    @property
    def created_at(self) -> int:
        """Backward compatibility: created_at in seconds"""
        return self.created_at_ms // 1000

    @created_at.setter
    def created_at(self, value: int):
        """Backward compatibility: created_at in seconds"""
        self.created_at_ms = value * 1000

    @property
    def updated_at(self) -> int:
        """Backward compatibility: updated_at in seconds"""
        return self.updated_at_ms // 1000

    @updated_at.setter
    def updated_at(self, value: int):
        """Backward compatibility: updated_at in seconds"""
        self.updated_at_ms = value * 1000

    @property
    def dimension(self) -> int:
        """Backward compatibility property for dimension"""
        return self.config.dimension

    @property
    def distance_metric(self):
        """Backward compatibility property for distance metric"""
        return self.config.distance_metric

    @property
    def storage_engine(self):
        """Backward compatibility property for storage engine"""
        return self.config.storage_engine

    @property
    def vector_count(self) -> int:
        """Backward compatibility property for vector count"""
        return self.stats.vector_count


# ============================================================================
# VECTOR MODELS
# ============================================================================


class VectorRecord(BaseModel):
    """Vector record for REST API"""

    id: Optional[str] = None
    vector: List[float]
    metadata: Dict[str, Union[str, int, float, bool, List[Union[str, int, float]]]] = (
        Field(default_factory=dict)
    )
    timestamp_ms: int = Field(
        default_factory=lambda: int(__import__("time").time() * 1000)
    )  # Required - milliseconds since epoch (signed int64)
    updated_at_ms: Optional[int] = (
        None  # Only set if different from timestamp_ms (saves bytes)
    )
    expires_at_ms: Optional[int] = (
        None  # TTL support (milliseconds since epoch, signed int64)
    )
    version: Optional[int] = 0  # Optional to save bytes, use small positive values
    source: Optional[str] = (
        None  # Original content that generated this vector (e.g., chunk text for RAG)
    )

    @field_validator("vector")
    def validate_vector(cls, v):
        if not v:
            raise ValueError("Vector cannot be empty")
        if not all(isinstance(x, (int, float)) for x in v):
            raise ValueError("Vector must contain only numeric values")
        if not all(math.isfinite(float(x)) for x in v):
            raise ValueError("Vector must contain only finite numeric values")
        return v

    def __getitem__(self, key: str) -> Any:
        """Support legacy dict-style access for SDK callers/tests."""
        return self.model_dump()[key]

    def get(self, key: str, default: Any = None) -> Any:
        """Support legacy dict.get(...) access for SDK callers/tests."""
        return self.model_dump().get(key, default)

    # Backward compatibility properties
    @property
    def timestamp(self) -> int:
        """Backward compatibility: timestamp in seconds"""
        return self.timestamp_ms // 1000

    @timestamp.setter
    def timestamp(self, value: int):
        """Backward compatibility: timestamp in seconds"""
        self.timestamp_ms = value * 1000

    @property
    def updated_at(self) -> Optional[int]:
        """Backward compatibility: updated_at in seconds"""
        return self.updated_at_ms // 1000 if self.updated_at_ms else None

    @updated_at.setter
    def updated_at(self, value: Optional[int]):
        """Backward compatibility: updated_at in seconds"""
        self.updated_at_ms = value * 1000 if value is not None else None

    @property
    def expires_at(self) -> Optional[int]:
        """Backward compatibility: expires_at in seconds"""
        return self.expires_at_ms // 1000 if self.expires_at_ms else None

    @expires_at.setter
    def expires_at(self, value: Optional[int]):
        """Backward compatibility: expires_at in seconds"""
        self.expires_at_ms = value * 1000 if value is not None else None


# ============================================================================
# SEARCH MODELS
# ============================================================================


class FilterCondition(BaseModel):
    """Filter condition"""

    field_name: str
    operation: FilterOperation
    value: Any


class MetadataFilter(BaseModel):
    """Metadata filter"""

    conditions: List[FilterCondition]
    operator: FilterOperator = FilterOperator.AND


class SearchQuery(BaseModel):
    """Search query"""

    vector: List[float]
    filters: Dict[str, Any] = (
        {}
    )  # Simple equality filters (proto map field - always include)
    id: Optional[str] = None
    metadata_filter: Optional[MetadataFilter] = None


class SearchParameters(BaseModel):
    """Search parameters"""

    ef_search: Optional[int] = None
    max_connections: Optional[int] = None
    n_probe: Optional[int] = None
    enable_reranking: Optional[bool] = None
    batch_size: Optional[int] = None
    timeout_ms: Optional[int] = None
    accuracy_threshold: Optional[float] = None
    enable_parallel_search: Optional[bool] = None
    thread_count: Optional[int] = None


class IncludeFields(BaseModel):
    """Fields to include in search results"""

    vector: bool = False
    metadata: bool = True
    score: bool = True
    rank: bool = True


class QuantizationHint(BaseModel):
    """Quantization hint for search"""

    hint_type: str  # "none", "binary", "scalar", "product", "uniform"
    parameters: Optional[Dict[str, Any]] = None


class SearchOptimization(BaseModel):
    """Search optimization hints including compression-aware options"""

    top_k: Optional[int] = None
    filters: Optional[Dict[str, Any]] = None
    accuracy_threshold: Optional[float] = None
    include_expired: Optional[bool] = None
    timeout_ms: Optional[int] = None
    enable_two_stage: Optional[bool] = None
    quantization_hint: Optional[QuantizationHint] = None
    enable_clustering_hint: Optional[bool] = None
    enable_metadata_filtering_hint: Optional[bool] = None

    # Compression-aware search hints
    prefer_compressed_search: Optional[bool] = Field(
        default=None, description="Prefer searching compressed data when available"
    )
    decompression_budget_ms: Optional[int] = Field(
        default=None, description="Maximum time budget for decompression operations"
    )
    use_decompression_cache: Optional[bool] = Field(
        default=True, description="Use decompression cache for repeated searches"
    )
    compression_aware_routing: Optional[bool] = Field(
        default=None, description="Enable compression-aware query routing"
    )

    custom_hints: Optional[Dict[str, Any]] = None


class SearchResult(BaseModel):
    """Search result - aligned with SearchVectorRecord proto"""

    id: str
    score: float
    vector: Optional[List[float]] = None
    metadata: Optional[Dict[str, Any]] = None
    rank: Optional[int] = None
    # Additional SearchVectorRecord fields (proto field 5-13)
    version: Optional[int] = None  # Proto field 5
    similarity: Optional[float] = None  # Proto field 6
    timestamp: Optional[int] = None  # Proto field 7 (milliseconds)
    source: Optional[str] = None  # Proto field 8 (original content for RAG)
    expanded_context: Optional[List[str]] = None  # Proto field 9
    semantic_similarity: Optional[float] = None  # Proto field 10
    quantization_info: Optional[str] = None  # Proto field 11
    engine_stats: Optional[Dict[str, str]] = None  # Proto field 12
    index_path: Optional[str] = None  # Proto field 13

    # Backward compatibility - map timestamp to timestamp_ms
    @property
    def timestamp_ms(self) -> Optional[int]:
        """Alias for timestamp field"""
        return self.timestamp


class SearchProgress(BaseModel):
    """Progress state for progressive search"""

    stage: int
    stages: int
    complete: bool


class SearchEnvelope(BaseModel):
    """Envelope for paginated/progressive SKS search results"""

    items: List[SearchResult]
    total: Optional[int] = None
    cursor: Optional[str] = None
    has_more: bool = False
    progress: Optional[SearchProgress] = None


class VectorGetResponse(BaseModel):
    """Vector get response"""

    id: str
    collection_id: str
    vector: Optional[List[float]] = None
    metadata: Optional[Dict[str, Any]] = None
    score: Optional[float] = None
    rank: Optional[int] = None


class ListCollectionsResponse(BaseModel):
    """List collections response"""

    collections: List[CollectionInfo]
    total_count: int


# ============================================================================
# REQUEST/RESPONSE MODELS
# ============================================================================


class CollectionOperationRequest(BaseModel):
    """Collection operation request"""

    operation: CollectionOperationType
    collection_id: Optional[str] = None
    collection_name: Optional[str] = None
    config: Optional[CollectionConfig] = None
    query_params: Optional[Dict[str, str]] = None
    options: Optional[Dict[str, bool]] = None


class CollectionResponse(BaseModel):
    """Collection operation response"""

    success: bool
    operation: str
    collection: Optional[Collection] = None
    collections: Optional[List[Collection]] = None
    affected_count: int = 0
    total_count: Optional[int] = None
    metadata: Dict[str, str] = Field(default_factory=dict)
    error_message: Optional[str] = None
    error_code: Optional[str] = None
    processing_time_us: int = 0


class VectorBatchRequest(BaseModel):
    """Vector batch operation request - aligned with REST API"""

    collection_id: str
    vectors: List[VectorRecord]  # Changed from 'records' to match REST API
    batch_timeout_ms: Optional[int] = None
    request_id: Optional[str] = None


class VectorSearchRequest(BaseModel):
    """Vector search request"""

    collection_id: str
    queries: List[SearchQuery]
    top_k: int = 10
    distance_metric_override: Optional[str] = None
    search_parameters: Optional[SearchParameters] = None  # Fixed field name
    include_fields: Optional[IncludeFields] = None
    search_optimization: Optional[SearchOptimization] = None


class OperationMetrics(BaseModel):
    """Operation metrics"""

    total_processed: int = 0
    successful_count: int = 0
    failed_count: int = 0
    updated_count: int = 0
    processing_time_us: int = 0
    wal_write_time_us: int = 0
    index_update_time_us: int = 0


class DeleteResult(BaseModel):
    """Delete operation result"""

    deleted_count: int = 0
    success: bool = True
    message: Optional[str] = None
    metrics: Optional[OperationMetrics] = None


class BatchResult(BaseModel):
    """Batch operation result"""

    total: int = 0
    success: int = 0
    failed: int = 0
    errors: List[str] = Field(default_factory=list)
    duration_ms: float = 0.0
    metrics: OperationMetrics = Field(default_factory=OperationMetrics)


class VectorOperationResponse(BaseModel):
    """Vector operation response"""

    success: Union[bool, int]
    operation: str
    metrics: OperationMetrics
    results: Optional[List[SearchResult]] = None
    vector_ids: List[str] = Field(default_factory=list)
    error_message: Optional[str] = None
    error_code: Optional[str] = None

    @property
    def count(self) -> int:
        """Backward compatibility: return successful count from metrics"""
        return self.metrics.successful_count if self.metrics else 0


class ApiError(BaseModel):
    """API error details"""

    code: str
    message: str
    details: Optional[Any] = None


class ApiResponse(BaseModel):
    """Generic API response wrapper"""

    success: bool
    data: Optional[Any] = None
    error: Optional[ApiError] = None
    message: Optional[str] = None

    model_config = ConfigDict(extra="allow")


# ============================================================================
# HEALTH AND MONITORING
# ============================================================================


class CompressionType(str, Enum):
    """Compression types"""

    NONE = "none"
    LZ4 = "lz4"
    ZSTD = "zstd"
    SNAPPY = "snappy"


class StorageConfig(BaseModel):
    """Complete storage configuration matching proto StorageConfig"""

    # Storage location and persistence
    storage_location: Optional[str] = None  # Override default storage path
    persistent: Optional[bool] = True  # Whether data persists after restart

    # Compression configuration
    compression: Optional[CompressionConfig] = None

    # Optimization hints
    access_pattern: Optional[AccessPattern] = None
    data_density: Optional[DataDensity] = None
    frequent_updates: Optional[bool] = None
    expected_size_gb: Optional[int] = None
    read_write_ratio: Optional[float] = None

    # Quick presets
    preset: Optional[str] = (
        None  # "maximum_performance", "balanced", "memory_constrained", "cloud_optimized", "real_time", "archive"
    )

    # Master optimization control
    enable_all_optimizations: Optional[bool] = True  # Default enabled

    # Specific configuration overrides
    parquet_writer: Optional[ParquetWriterSettings] = None
    footer_cache: Optional[FooterCacheSettings] = None
    hybrid_writer: Optional[HybridWriterSettings] = None

    # Engine-specific settings
    sst_settings: Optional[SstEngineSettings] = None
    viper_settings: Optional[ViperEngineSettings] = None
    nova_settings: Optional[NovaEngineSettings] = None


class FlushConfig(BaseModel):
    """Flush configuration"""

    force_flush: bool = False
    timeout_ms: int = 5000
    include_secondary_indexes: bool = True
    include_metadata: bool = True
    max_wal_size_mb: Optional[float] = None


class HealthStatus(BaseModel):
    """Health check response"""

    status: str
    version: str
    uptime_seconds: int
    services: Dict[str, str]
    timestamp_ms: int  # Milliseconds since epoch (signed int64)

    # Backward compatibility property
    @property
    def timestamp(self) -> int:
        """Backward compatibility: timestamp in seconds"""
        return self.timestamp_ms // 1000

    @timestamp.setter
    def timestamp(self, value: int):
        """Backward compatibility: timestamp in seconds"""
        self.timestamp_ms = value * 1000


# Simple alias
Vector = VectorRecord
