"""
ProximaDB Python Client - Data Models

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
from enum import IntEnum
import numpy as np
from pydantic import BaseModel, Field, field_validator, ConfigDict


# Proto-aligned enums with integer values
class DistanceMetric(IntEnum):
    """Distance metrics aligned with proto definition"""
    DISTANCE_METRIC_UNSPECIFIED = 0
    COSINE = 1
    EUCLIDEAN = 2
    DOT_PRODUCT = 3
    HAMMING = 4
    MANHATTAN = 5
    JACCARD = 6
    CUSTOM = 7


class StorageEngine(IntEnum):
    """Storage engines aligned with proto definition"""
    STORAGE_ENGINE_UNSPECIFIED = 0
    VIPER = 1
    LSM = 2
    MMAP = 3
    HYBRID = 4


class IndexingAlgorithm(IntEnum):
    """Indexing algorithms aligned with proto definition"""
    INDEXING_ALGORITHM_UNSPECIFIED = 0
    HNSW = 1
    IVF = 2
    PQ = 3
    FLAT = 4
    ANNOY = 5


class CollectionOperation(IntEnum):
    """Collection operations aligned with proto definition"""
    COLLECTION_OPERATION_UNSPECIFIED = 0
    COLLECTION_CREATE = 1
    COLLECTION_UPDATE = 2
    COLLECTION_GET = 3
    COLLECTION_LIST = 4
    COLLECTION_DELETE = 5
    COLLECTION_MIGRATE = 6
    COLLECTION_GET_ID_BY_NAME = 7


class VectorOperation(IntEnum):
    """Vector operations aligned with proto definition"""
    VECTOR_OPERATION_UNSPECIFIED = 0
    VECTOR_BATCH = 1
    VECTOR_SEARCH = 2


# Quantization parameter models
class BinaryQuantizationParams(BaseModel):
    """Binary quantization parameters"""
    threshold: float = 0.0


class ScalarQuantizationParams(BaseModel):
    """Scalar quantization parameters (INT8/INT16)"""
    bits: int = Field(default=8, ge=1, le=32)
    scale: float = 1.0
    offset: float = 0.0


class ProductQuantizationParams(BaseModel):
    """Product quantization parameters"""
    num_subvectors: int = Field(default=8, ge=1, le=256)
    bits_per_code: int = Field(default=8, ge=1, le=16)


class UniformQuantizationParams(BaseModel):
    """Uniform quantization parameters"""
    scale: float = 1.0
    offset: float = 0.0


# Search parameters aligned with proto SearchParams
class SearchParams(BaseModel):
    """Search parameters aligned with proto definition"""
    top_k: Optional[int] = Field(default=None, ge=1, le=10000)
    filters: Dict[str, Any] = Field(default_factory=dict)
    accuracy_threshold: Optional[float] = Field(default=None, ge=0.0, le=1.0)
    include_expired: Optional[bool] = None
    timeout_ms: Optional[int] = Field(default=None, ge=1)
    enable_two_stage: Optional[bool] = None
    
    # Quantization hint - one of these
    no_quantization: Optional[bool] = None
    binary_params: Optional[BinaryQuantizationParams] = None
    scalar_params: Optional[ScalarQuantizationParams] = None
    product_params: Optional[ProductQuantizationParams] = None
    uniform_params: Optional[UniformQuantizationParams] = None
    
    # Optional optimization hints
    enable_clustering_hint: Optional[bool] = None
    enable_metadata_filtering_hint: Optional[bool] = None
    custom_hints: Dict[str, Any] = Field(default_factory=dict)

    model_config = ConfigDict(use_enum_values=True)

    def get_quantization_hint(self) -> Optional[str]:
        """Get quantization hint string for backward compatibility"""
        if self.no_quantization:
            return "FP32"
        elif self.binary_params:
            return "BINARY"
        elif self.scalar_params:
            if self.scalar_params.bits == 8:
                return "INT8"
            elif self.scalar_params.bits == 16:
                return "INT16"
            return f"INT{self.scalar_params.bits}"
        elif self.product_params:
            return f"PQ{self.product_params.num_subvectors}"
        elif self.uniform_params:
            return "UNIFORM"
        return None


# Quantization configuration for collections
class StorageQuantizationConfig(BaseModel):
    """Storage-aligned quantization configuration"""
    enabled: bool = False
    level_type: Optional[str] = None  # "scalar", "binary", "product", "uniform"
    level_params: Optional[Union[ScalarQuantizationParams, BinaryQuantizationParams, 
                                ProductQuantizationParams, UniformQuantizationParams]] = None
    codebook_id: Optional[str] = None
    progressive_quantization: bool = False
    storage_compatibility: str = "VIPER_ONLY"  # "VIPER_ONLY", "ALL_ENGINES", "LSM_AND_VIPER"


class IndexQuantizationStrategy(BaseModel):
    """Index quantization strategy"""
    index_name: str
    level_type: str
    level_params: Optional[Union[ScalarQuantizationParams, BinaryQuantizationParams,
                                ProductQuantizationParams, UniformQuantizationParams]] = None
    build_async: bool = False
    codebook_id: Optional[str] = None


class IndexQuantizationConfig(BaseModel):
    """Index-time quantization configuration"""
    enabled: bool = False
    strategies: List[IndexQuantizationStrategy] = Field(default_factory=list)
    auto_select_strategy: bool = False


class SearchQuantizationConfig(BaseModel):
    """Search-time quantization configuration"""
    enabled: bool = False
    default_level_type: str = "scalar"
    default_level_params: Optional[Union[ScalarQuantizationParams, BinaryQuantizationParams,
                                        ProductQuantizationParams, UniformQuantizationParams]] = None
    adaptive_precision: bool = True
    accuracy_threshold: float = 0.95
    candidate_multiplier: int = 3


class QuantizationValidation(BaseModel):
    """Quantization validation configuration"""
    accuracy_threshold: float = 0.95
    validation_sample_size: int = 1000
    enable_quality_monitoring: bool = True
    retraining_threshold: float = 0.90


class QuantizationConfig(BaseModel):
    """Comprehensive quantization configuration"""
    enabled: bool = False
    storage_quantization: Optional[StorageQuantizationConfig] = None
    index_quantization: Optional[IndexQuantizationConfig] = None
    search_quantization: Optional[SearchQuantizationConfig] = None
    compression_ratio_target: float = 4.0
    validation: Optional[QuantizationValidation] = None


# Index configuration
class HnswConfig(BaseModel):
    """HNSW index configuration"""
    m: int = Field(default=16, ge=2, le=100)
    ef_construction: int = Field(default=200, ge=1, le=2000)
    max_m: int = Field(default=16, ge=2, le=100)
    seed: Optional[int] = None
    enable_heuristic: bool = True


class IvfConfig(BaseModel):
    """IVF index configuration"""
    nlist: int = Field(default=100, ge=1, le=10000)
    nprobe: int = Field(default=10, ge=1, le=1000)
    quantizer: Optional[str] = None
    encode_residual: bool = False


class PqConfig(BaseModel):
    """PQ index configuration"""
    subvectors: int = Field(default=8, ge=1, le=256)
    bits_per_subvector: int = Field(default=8, ge=1, le=16)
    training_sample_count: int = Field(default=10000, ge=100)
    enable_reranking: bool = True


class AnnoyConfig(BaseModel):
    """Annoy index configuration"""
    n_trees: int = Field(default=10, ge=1, le=100)
    search_k: int = Field(default=-1, description="Default -1 means n_trees * top_k")
    max_leaf_size: int = Field(default=100, ge=1)
    enable_mmap: bool = True


class IndexConfig(BaseModel):
    """Index configuration wrapper"""
    algorithm_config: Optional[Union[HnswConfig, IvfConfig, PqConfig, AnnoyConfig]] = None
    update_mode: str = Field(default="synchronous", pattern="^(synchronous|asynchronous|hybrid)$")
    max_memory_usage_bytes: Optional[int] = Field(default=None, ge=0)
    enable_gpu_acceleration: bool = False


# Collection configuration
class FilterableColumnSpec(BaseModel):
    """Filterable column specification"""
    name: str
    data_type: str  # "string", "integer", "float", "boolean", "datetime", "array_string", etc.
    indexed: bool = True
    supports_range: bool = False
    estimated_cardinality: Optional[int] = None


class CollectionConfig(BaseModel):
    """Collection configuration aligned with proto"""
    name: str
    dimension: int = Field(ge=1, le=10000)
    distance_metric: DistanceMetric = DistanceMetric.COSINE
    storage_engine: StorageEngine = StorageEngine.VIPER
    indexing_algorithm: IndexingAlgorithm = IndexingAlgorithm.HNSW
    filterable_metadata_fields: List[str] = Field(default_factory=list)
    indexing_config: Dict[str, Any] = Field(default_factory=dict)
    filterable_columns: List[FilterableColumnSpec] = Field(default_factory=list)
    index_config: Optional[IndexConfig] = None
    quantization_config: Optional[QuantizationConfig] = None

    model_config = ConfigDict(use_enum_values=True)


# Vector and metadata models
class MetadataValue(BaseModel):
    """Metadata value that can be string, int, float, bool, or array"""
    value: Union[str, int, float, bool, List[Union[str, int, float]]]


class VectorRecord(BaseModel):
    """Vector record aligned with proto"""
    id: Optional[str] = None
    vector: List[float]
    metadata: Dict[str, Union[str, int, float, bool, List[Union[str, int, float]]]] = Field(default_factory=dict)
    timestamp: Optional[int] = None  # Microseconds since epoch
    version: int = 0
    expires_at: Optional[int] = None  # TTL support

    @field_validator('vector')
    def validate_vector(cls, v):
        if not v:
            raise ValueError("Vector cannot be empty")
        if not all(isinstance(x, (int, float)) for x in v):
            raise ValueError("Vector must contain only numeric values")
        return v


# Search models
class SearchQuery(BaseModel):
    """Search query"""
    vector: Optional[List[float]] = None
    id: Optional[str] = None
    metadata_filter: Dict[str, Any] = Field(default_factory=dict)

    @field_validator('vector')
    def validate_query(cls, v, values):
        if v is None and values.get('id') is None:
            raise ValueError("Either vector or id must be provided")
        return v


class IncludeFields(BaseModel):
    """Fields to include in search results"""
    vector: bool = False
    metadata: bool = True
    score: bool = True
    rank: bool = True


class SearchResult(BaseModel):
    """Search result"""
    id: Optional[str] = None
    score: float
    vector: Optional[List[float]] = None
    metadata: Optional[Dict[str, Any]] = None
    rank: Optional[int] = None


# Response models
# Add Collection alias for backward compatibility
Collection = None  # Will be replaced by proto Collection

class CollectionInfo(BaseModel):
    """Collection information"""
    id: str
    config: CollectionConfig
    stats: Optional[Dict[str, Any]] = None
    created_at: Optional[int] = None
    updated_at: Optional[int] = None

# Alias for backward compatibility
Collection = CollectionInfo


class OperationMetrics(BaseModel):
    """Operation metrics"""
    total_processed: int = 0
    successful_count: int = 0
    failed_count: int = 0
    updated_count: int = 0
    processing_time_us: int = 0
    wal_write_time_us: int = 0
    index_update_time_us: int = 0


class ApiResponse(BaseModel):
    """Generic API response"""
    success: bool
    data: Optional[Any] = None
    error: Optional[str] = None
    message: Optional[str] = None
    metrics: Optional[OperationMetrics] = None