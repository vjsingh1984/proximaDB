"""
ProximaDB Python Client - Data Models

Copyright 2024 Vijaykumar Singh

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

import numpy as np
from pydantic import BaseModel, Field, field_validator, ConfigDict


class DistanceMetric(str, Enum):
    """Supported distance metrics for vector similarity"""
    COSINE = "cosine"
    EUCLIDEAN = "euclidean"
    DOT_PRODUCT = "dot_product"
    MANHATTAN = "manhattan"
    HAMMING = "hamming"


class IndexAlgorithm(str, Enum):
    """Supported vector index algorithms"""
    HNSW = "hnsw"
    IVF = "ivf"
    LSH = "lsh"
    BRUTE_FORCE = "brute_force"
    AUTO = "auto"


class CompressionType(str, Enum):
    """Supported compression algorithms"""
    NONE = "none"
    LZ4 = "lz4"
    ZSTD = "zstd"
    GZIP = "gzip"


class IndexConfig(BaseModel):
    """Vector index configuration"""
    algorithm: IndexAlgorithm = IndexAlgorithm.HNSW
    parameters: Dict[str, Any] = Field(default_factory=dict)
    
    model_config = ConfigDict(use_enum_values=True)


class StorageConfig(BaseModel):
    """Storage configuration for collections"""
    compression: CompressionType = CompressionType.LZ4
    replication_factor: int = Field(default=3, ge=1, le=5)
    enable_tiering: bool = True
    hot_tier_size_gb: Optional[int] = None
    
    model_config = ConfigDict(use_enum_values=True)


class FlushConfig(BaseModel):
    """WAL flush configuration for collections"""
    max_wal_size_mb: Optional[float] = Field(
        default=None,
        ge=1.0,
        description="Maximum WAL size in MB before forced flush (None = use global default: 128MB)"
    )
    
    model_config = ConfigDict(use_enum_values=True)


class CollectionConfig(BaseModel):
    """Configuration for creating collections"""
    dimension: int = Field(..., ge=1, le=65536)
    distance_metric: DistanceMetric = DistanceMetric.COSINE
    index_config: Optional[IndexConfig] = None
    storage_config: Optional[StorageConfig] = None
    
    # Storage layout configuration
    storage_layout: str = Field(
        default="viper",
        description="Storage layout: 'viper' (default), 'lsm', 'rocksdb', 'memory'"
    )
    
    description: Optional[str] = None
    metadata_schema: Optional[Dict[str, str]] = None
    
    # VIPER-specific optimization (only used when storage_layout = 'viper')
    filterable_metadata_fields: Optional[List[str]] = Field(
        default=None, 
        description="Up to 16 metadata field names for Parquet column optimization and efficient filtering (VIPER only)"
    )
    flush_config: Optional[FlushConfig] = Field(
        default=None,
        description="WAL flush configuration (None = use global defaults). SIZE-BASED FLUSH ONLY for stability."
    )
    
    model_config = ConfigDict(use_enum_values=True)
    
    @field_validator('dimension')
    def validate_dimension(cls, v: int) -> int:
        if v <= 0:
            raise ValueError("Dimension must be positive")
        return v
    
    @field_validator('storage_layout')
    def validate_storage_layout(cls, v: str) -> str:
        valid_layouts = {'viper', 'lsm', 'rocksdb', 'memory'}
        if v not in valid_layouts:
            raise ValueError(f"Storage layout '{v}' is not supported. Valid options: {', '.join(valid_layouts)}")
        return v
    
    @field_validator('filterable_metadata_fields')
    def validate_filterable_fields(cls, v: Optional[List[str]]) -> Optional[List[str]]:
        if v is not None:
            # Validate field names
            for field in v:
                if not isinstance(field, str) or not field.strip():
                    raise ValueError("Filterable metadata field names must be non-empty strings")
                if len(field) > 64:
                    raise ValueError(f"Filterable metadata field name '{field}' exceeds 64 character limit")
                if not field.replace('_', '').isalnum():
                    raise ValueError(f"Filterable metadata field name '{field}' must be alphanumeric with underscores")
            
            # Check for duplicates
            if len(v) != len(set(v)):
                raise ValueError("Filterable metadata field names must be unique")
            
            # Check reserved names
            reserved_names = {'id', 'vector', 'vectors', 'timestamp', 'created_at', 'updated_at', 'expires_at', 'extra_meta'}
            for field in v:
                if field.lower() in reserved_names:
                    raise ValueError(f"Filterable metadata field name '{field}' is reserved")
        
        return v


class Collection(BaseModel):
    """Vector collection metadata"""
    id: str
    name: str
    config: Optional[CollectionConfig] = None
    vector_count: int = 0
    created_at: Optional[datetime] = None
    updated_at: Optional[datetime] = None
    status: str = "active"
    # Simple server fields
    dimension: Optional[int] = None
    metric: Optional[str] = None
    index_type: Optional[str] = None
    
    model_config = ConfigDict(
        json_encoders={datetime: lambda v: v.isoformat()}
    )


class Vector(BaseModel):
    """Vector with metadata"""
    id: str
    vector: List[float]
    metadata: Optional[Dict[str, Any]] = None
    
    @field_validator('vector')
    def validate_vector(cls, v: List[float]) -> List[float]:
        if not v:
            raise ValueError("Vector cannot be empty")
        return v
    
    @classmethod
    def from_numpy(cls, id: str, array: np.ndarray, metadata: Optional[Dict[str, Any]] = None) -> "Vector":
        """Create Vector from numpy array"""
        if array.dtype != np.float32:
            array = array.astype(np.float32)
        return cls(id=id, vector=array.tolist(), metadata=metadata)
    
    def to_numpy(self) -> np.ndarray:
        """Convert vector to numpy array"""
        return np.array(self.vector, dtype=np.float32)


class SearchResult(BaseModel):
    """Search result with similarity score"""
    id: Optional[str] = None  # Made optional to match protobuf definition
    score: float
    metadata: Optional[Dict[str, Any]] = None
    distance: Optional[float] = None
    vector: Optional[List[float]] = None  # Include vector data if requested
    
    def to_numpy(self) -> Optional[np.ndarray]:
        """Convert vector to numpy array if available"""
        if self.vector is None:
            return None
        return np.array(self.vector, dtype=np.float32)


class SearchStats(BaseModel):
    """Search performance statistics"""
    total_searched: int
    vectors_scanned: int
    duration_ms: float
    cache_hit_rate: Optional[float] = None
    index_seek_time_us: Optional[int] = None


class SearchParams(BaseModel):
    """Search parameters for fine-tuning"""
    ef: Optional[int] = Field(default=None, ge=1)
    exact_search: bool = False
    timeout_ms: int = Field(default=5000, ge=100, le=60000)
    include_vectors: bool = False
    include_metadata: bool = True


class FilterCondition(BaseModel):
    """Filter condition for metadata filtering"""
    field: str
    operator: str  # eq, ne, gt, gte, lt, lte, in, nin, exists
    value: Union[str, int, float, bool, List[Any]]


class SearchRequest(BaseModel):
    """Search request with all parameters"""
    collection_id: str
    query: List[float]
    k: int = Field(..., ge=1, le=10000)
    filter: Optional[Dict[str, Any]] = None
    params: Optional[SearchParams] = None
    
    @field_validator('query')
    def validate_query(cls, v: List[float]) -> List[float]:
        if not v:
            raise ValueError("Query vector cannot be empty")
        return v
    
    @classmethod
    def from_numpy(cls, collection_id: str, query: np.ndarray, k: int, **kwargs) -> "SearchRequest":
        """Create SearchRequest from numpy array"""
        if query.dtype != np.float32:
            query = query.astype(np.float32)
        return cls(collection_id=collection_id, query=query.tolist(), k=k, **kwargs)


class SearchResponse(BaseModel):
    """Search response with results and statistics"""
    results: List[SearchResult]
    stats: Optional[SearchStats] = None
    request_id: Optional[str] = None
    # Simple server fields
    total_time_ms: Optional[int] = None


class InsertResult(BaseModel):
    """Result of vector insertion operation"""
    count: int
    failed_count: int = 0
    duration_ms: float
    request_id: Optional[str] = None
    errors: Optional[List[str]] = None


class BatchResult(BaseModel):
    """Result of batch operation"""
    total_count: Optional[int] = None
    successful_count: int
    failed_count: int
    duration_ms: float
    request_id: Optional[str] = None
    errors: Optional[List[Dict[str, Any]]] = None
    # Simple server fields
    count: Optional[int] = None


class DeleteResult(BaseModel):
    """Result of delete operation"""
    deleted_count: Optional[int] = None
    not_found_count: int = 0
    duration_ms: Optional[float] = None
    request_id: Optional[str] = None
    # Simple server fields
    deleted: Optional[bool] = None
    count: Optional[int] = None


class CollectionStats(BaseModel):
    """Collection statistics"""
    collection_id: Optional[str] = None
    vector_count: int
    index_size_bytes: int
    data_size_bytes: Optional[int] = None
    memory_usage_bytes: int
    last_updated: Optional[datetime] = None
    # Simple server fields
    dimension: Optional[int] = None
    
    model_config = ConfigDict(
        json_encoders={datetime: lambda v: v.isoformat()}
    )


class ClusterInfo(BaseModel):
    """Cluster information"""
    cluster_id: str
    node_count: int
    leader_node: str
    version: str
    status: str
    uptime_seconds: int


class HealthStatus(BaseModel):
    """Health status response"""
    status: str  # healthy, degraded, unhealthy
    version: str
    uptime_seconds: Optional[int] = None
    cluster_info: Optional[ClusterInfo] = None
    checks: Dict[str, bool] = Field(default_factory=dict)


# Type aliases for convenience
VectorArray = Union[List[List[float]], np.ndarray]
MetadataDict = Dict[str, Any]
FilterDict = Dict[str, Any]