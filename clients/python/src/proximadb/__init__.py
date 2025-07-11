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

# Unified client interface (recommended)
from .unified_client import ProximaDBClient, connect, connect_grpc, connect_rest, Protocol

# Specialized clients (for advanced use cases)
from .rest_client import ProximaDBRestClient
from .grpc_client import ProximaDBClient as ProximaDBGrpcClient
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
)
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

__all__ = [
    # Unified client interface (recommended)
    "ProximaDBClient",
    "connect", 
    "connect_grpc",
    "connect_rest",
    "Protocol",
    
    # Specialized clients (for advanced use cases)
    "ProximaDBRestClient",
    "ProximaDBGrpcClient",
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
]