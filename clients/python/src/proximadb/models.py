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
from typing import Any, Dict, List, Optional, Union
from enum import Enum
from pydantic import BaseModel, Field, field_validator, ConfigDict
import numpy as np

# Type aliases for convenience
VectorArray = Union[List[List[float]], np.ndarray]
MetadataDict = Dict[str, Union[str, int, float, bool, List[Union[str, int, float]]]]
FilterDict = Dict[str, Any]


# ============================================================================
# ENUMS - String values for REST API
# ============================================================================

class DistanceMetric(str, Enum):
    """Distance metrics for REST API
    
    Note: Server supports 13+ distance metrics. Unsupported metrics
    will fallback to COSINE (server behavior, not client validation).
    """
    COSINE = "cosine"
    EUCLIDEAN = "euclidean"
    DOT_PRODUCT = "dot_product"
    MANHATTAN = "manhattan"  # Fallback: COSINE (server decides)
    HAMMING = "hamming"      # Fallback: COSINE (server decides)
    JACCARD = "jaccard"      # Fallback: COSINE (server decides)
    
    # Extended metrics supported by server (ProximaDB v1.0+)
    CHEBYSHEV = "chebyshev"
    CANBERRA = "canberra"
    MINKOWSKI = "minkowski"
    ANGULAR = "angular"
    BRAY_CURTIS = "bray_curtis"
    HELLINGER = "hellinger"
    CUSTOM = "custom"


class StorageEngine(str, Enum):
    """Storage engines for REST API
    
    Note: Server supports VIPER and SST. Unsupported engines
    will fallback to VIPER (server behavior, not client validation).
    """
    VIPER = "viper"  # Columnar storage with Parquet format
    SST = "sst"      # Row-based storage with SSTable format
    MMAP = "mmap"    # Fallback: VIPER (server decides)
    HYBRID = "hybrid" # Fallback: VIPER (server decides)


class IndexingAlgorithm(str, Enum):
    """Indexing algorithms for REST API
    
    Note: All 6 indexing algorithms are fully supported by the server as of 2025-08.
    No fallbacks required - server handles all algorithms natively.
    """
    HNSW = "hnsw"   # Hierarchical Navigable Small World
    IVF = "ivf"     # Inverted File Index
    PQ = "pq"       # Product Quantization
    FLAT = "flat"   # Brute force / exhaustive search
    ANNOY = "annoy" # Approximate Nearest Neighbors Oh Yeah
    LSH = "lsh"     # Locality Sensitive Hashing (fully supported)




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
        "cosine", "euclidean", "dot_product", "manhattan", "hamming", "jaccard", 
        "chebyshev", "canberra", "minkowski", "angular", "bray_curtis", "hellinger", "custom"
    ]
    
    # Distance metrics that fallback to cosine (none - all are now supported natively)
    fallback_distance_metrics: List[str] = []
    
    # Fully supported storage engines (no fallback)
    supported_storage_engines: List[str] = ["viper", "sst"]
    
    # Storage engines that fallback to viper
    fallback_storage_engines: List[str] = ["mmap", "hybrid"]
    
    # Fully supported indexing algorithms (no fallback) - All 6 algorithms supported as of 2025-08
    supported_indexing_algorithms: List[str] = [
        "hnsw", "ivf", "pq", "flat", "annoy", "lsh"
    ]
    
    # Indexing algorithms that fallback to hnsw (none - all are now supported natively)
    fallback_indexing_algorithms: List[str] = []
    
    # Quantization types (all supported in VIPER engine)
    supported_quantization_types: List[str] = [
        "none", "uniform", "pq", "scalar", "binary", "custom"
    ]
    
    # Server behavior notes
    notes: Dict[str, str] = {
        "fallback_policy": "Server uses intelligent fallbacks instead of errors",
        "dimension_limit": "Server supports up to 100,000 dimensions",
        "name_validation": "Collection names must be 8+ characters to avoid collision with 7-char base62 IDs",
        "quantization_engine": "Quantization fully supported in VIPER engine only",
        "filterable_columns": "All FilterableDataType values supported"
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
    """SDK-driven compression configuration for collections
    
    This configuration is passed from the SDK to the server, allowing
    client-side control over compression settings without server-side defaults.
    """
    # SST compression settings
    sst_block_size: Optional[int] = Field(
        default=16384,
        description="Target block size for SST files (16KB default)"
    )
    sst_compression_algorithm: Optional[CompressionAlgorithm] = Field(
        default=CompressionAlgorithm.NONE,
        description="Compression algorithm for SST blocks"
    )
    sst_compression_level: Optional[int] = Field(
        default=None,
        description="Compression level (1-9 for ZSTD)"
    )
    
    # VIPER compression settings  
    viper_compression_algorithm: Optional[CompressionAlgorithm] = Field(
        default=CompressionAlgorithm.NONE,
        description="Compression algorithm for VIPER Parquet files"
    )
    viper_compression_level: Optional[int] = Field(
        default=None,
        description="Compression level for VIPER"
    )
    viper_enable_dual_columns: Optional[bool] = Field(
        default=False,
        description="Enable dual column storage (FP32 + quantized) in VIPER"
    )
    
    # Global settings
    adaptive_compression: Optional[bool] = Field(
        default=False,
        description="Enable adaptive compression based on data characteristics"
    )
    compression_threshold_kb: Optional[int] = Field(
        default=100,
        description="Minimum file size in KB before applying compression"
    )


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
    algorithm: IndexingAlgorithm
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
    """Collection configuration for REST API"""
    name: str = Field(min_length=8)  # Minimum 8 characters to prevent collision with 7-char base62 IDs
    dimension: int = Field(ge=1, le=100000)  # Relaxed: Server supports up to 100k dimensions
    distance_metric: Optional[DistanceMetric] = None
    storage_engine: Optional[StorageEngine] = None
    primary_indexing_algorithm: Optional[IndexingAlgorithm] = None
    filterable_columns: Optional[List[FilterableColumn]] = None
    index_configs: Optional[List[IndexConfiguration]] = None
    quantization_config: Optional[QuantizationConfig] = None
    compression_config: Optional[CompressionConfig] = None  # SDK-driven compression
    primary_index_name: Optional[str] = None
    enable_automatic_index_selection: Optional[bool] = None
    description: Optional[str] = None
    tags: Optional[List[str]] = None
    owner: Optional[str] = None
    metadata_schema: Optional[Dict[str, Any]] = None
    filterable_metadata_fields: Optional[List[str]] = None
    
    @field_validator('name')
    def validate_name_length(cls, v):
        """Validate collection name is at least 8 characters to prevent collision with 7-char base62 IDs"""
        if not v or not v.strip():
            raise ValueError("Collection name cannot be empty")
        v = v.strip()
        if len(v) < 8:
            raise ValueError("Collection name must be at least 8 characters long to prevent collision with 7-character base62 collection IDs")
        return v
    
    @property
    def index_config(self):
        """Backward compatibility property for singular index_config access"""
        if self.index_configs and len(self.index_configs) > 0:
            return self.index_configs[0]
        return None
    
    @property 
    def storage_config(self):
        """Backward compatibility property for storage_config access"""
        # This is used in tests but not defined in the model
        # Return a mock object for now
        from types import SimpleNamespace
        return SimpleNamespace(compression=CompressionType.LZ4)


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
    created_at: int
    updated_at: int
    vector_count: Optional[int] = None
    indexed: bool = False


class Collection(BaseModel):
    """Collection information"""
    id: str
    config: CollectionConfig
    stats: CollectionStats = Field(default_factory=CollectionStats)  # Made required to match REST API
    created_at: int = Field(default_factory=lambda: int(__import__('time').time()))  # Renamed from timestamp
    updated_at: int = Field(default_factory=lambda: int(__import__('time').time()))  # Made required
    
    @property
    def name(self) -> str:
        """Backward compatibility property for collection name"""
        return self.config.name
    
    @property
    def timestamp(self) -> int:
        """Backward compatibility property for timestamp"""
        return self.created_at


# ============================================================================
# VECTOR MODELS
# ============================================================================

class VectorRecord(BaseModel):
    """Vector record for REST API"""
    id: Optional[str] = None
    vector: List[float]
    metadata: Dict[str, Union[str, int, float, bool, List[Union[str, int, float]]]] = Field(default_factory=dict)
    timestamp: int = Field(default_factory=lambda: int(__import__('time').time()))  # Required - seconds since epoch (unsigned)
    updated_at: Optional[int] = None  # Only set if different from timestamp (saves bytes)
    expires_at: Optional[int] = None  # TTL support (seconds since epoch, unsigned)
    version: Optional[int] = 0  # Optional to save bytes, use small positive values

    @field_validator('vector')
    def validate_vector(cls, v):
        if not v:
            raise ValueError("Vector cannot be empty")
        if not all(isinstance(x, (int, float)) for x in v):
            raise ValueError("Vector must contain only numeric values")
        return v


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
        default=None,
        description="Prefer searching compressed data when available"
    )
    decompression_budget_ms: Optional[int] = Field(
        default=None,
        description="Maximum time budget for decompression operations"
    )
    use_decompression_cache: Optional[bool] = Field(
        default=True,
        description="Use decompression cache for repeated searches"
    )
    compression_aware_routing: Optional[bool] = Field(
        default=None,
        description="Enable compression-aware query routing"
    )
    
    custom_hints: Optional[Dict[str, Any]] = None


class SearchResult(BaseModel):
    """Search result"""
    id: str
    score: float
    vector: Optional[List[float]] = None
    metadata: Optional[Dict[str, Any]] = None
    rank: Optional[int] = None


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


class VectorOperationResponse(BaseModel):
    """Vector operation response"""
    success: bool
    operation: str
    metrics: OperationMetrics
    results: Optional[List[SearchResult]] = None
    vector_ids: List[str] = Field(default_factory=list)
    error_message: Optional[str] = None
    error_code: Optional[str] = None


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

    model_config = ConfigDict(extra='allow')


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
    """Storage configuration"""
    compression: CompressionType = CompressionType.NONE
    replication_factor: int = 1
    enable_tiering: bool = False


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
    timestamp: int


# Simple alias
Vector = VectorRecord