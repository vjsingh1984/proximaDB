"""
ProximaDB Python Client SDK

Copyright 2025 Vijaykumar Singh

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

# Unified client interface
from .unified_client import ProximaDBClient, connect, connect_grpc, connect_rest, Protocol

# Individual client implementations
from .rest_client import ProximaDBRestClient
try:
    from .protocols.grpc_async import ProximaDBGrpcClient
except ImportError:
    # gRPC not available
    ProximaDBGrpcClient = None
from .config import ClientConfig
from .models import (
    Collection,
    CollectionConfig,
    IndexConfiguration,
    SearchResult,
    VectorOperationResponse,
    OperationMetrics,
    DistanceMetric,
    IndexingAlgorithm,
    StorageEngine,
    VectorRecord,
    HealthStatus,
    VectorArray,
    MetadataDict,
    FilterDict,
    QuantizationConfig,
    QuantizationType,
    SearchOptimization,
)

# Aliases for test compatibility
IndexConfig = IndexConfiguration
Vector = VectorRecord

from .exceptions import (
    ProximaDBError,
    AuthenticationError,
    CollectionNotFoundError,
    VectorDimensionError,
    RateLimitError,
    ServerError,
    NetworkError,
)

# Text chunking utilities
from .chunking import (
    TextChunker,
    ChunkingStrategy,
    ChunkingConfig,
    TextChunk,
    create_chunker,
    chunk_by_sentences,
    chunk_by_paragraphs,
    chunk_sliding_window,
)

__version__ = "0.1.0"
__author__ = "Vijaykumar Singh"
__email__ = "singhvjd@gmail.com"

# Protobuf modules for gRPC
try:
    from . import proximadb_pb2
    from . import proximadb_pb2_grpc
except ImportError:
    proximadb_pb2 = None
    proximadb_pb2_grpc = None

__all__ = [
    # Unified client interface
    "ProximaDBClient",
    "connect", 
    "connect_grpc",
    "connect_rest",
    "Protocol",
    
    # Individual client implementations
    "ProximaDBRestClient",
    "ProximaDBGrpcClient",
    
    # Configuration
    "ClientConfig",
    
    # Models
    "Collection",
    "CollectionConfig", 
    "IndexConfiguration",
    "SearchResult",
    "VectorOperationResponse",
    "OperationMetrics",
    "DistanceMetric",
    "IndexingAlgorithm",
    "StorageEngine",
    "VectorRecord",
    "HealthStatus",
    "VectorArray",
    "MetadataDict",
    "FilterDict",
    "QuantizationConfig",
    "QuantizationType",
    "SearchOptimization",
    
    # Aliases for test compatibility
    "IndexConfig",
    "Vector",
    
    # Exceptions
    "ProximaDBError",
    "AuthenticationError",
    "CollectionNotFoundError",
    "VectorDimensionError", 
    "RateLimitError",
    "ServerError",
    "NetworkError",
    
    # Text chunking
    "TextChunker",
    "ChunkingStrategy",
    "ChunkingConfig",
    "TextChunk",
    "create_chunker",
    "chunk_by_sentences",
    "chunk_by_paragraphs",
    "chunk_sliding_window",
    
    # Protobuf modules (for gRPC)
    "proximadb_pb2",
    "proximadb_pb2_grpc",
]

# Filter API
from .filters import (
    FilterBuilder,
    FilterOp,
    LogicalOp,
    eq,
    gt,
    lt,
    in_list,
    and_filters,
    or_filters,
)

__all__.extend([
    # Filter API
    "FilterBuilder",
    "FilterOp", 
    "LogicalOp",
    "eq",
    "gt",
    "lt",
    "in_list",
    "and_filters",
    "or_filters",
])