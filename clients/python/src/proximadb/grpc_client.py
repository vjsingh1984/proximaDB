"""
ProximaDB Synchronous gRPC Client

Synchronous wrapper around gRPC client to match REST client interface.
"""

import asyncio
import logging
from typing import Any, Dict, List, Optional, Union

import numpy as np

from .models import (
    Collection,
    CollectionConfig,
    SearchResult,
    InsertResult,
    BatchResult,
    DeleteResult,
    HealthStatus,
    VectorArray,
    MetadataDict,
    FilterDict,
)
from .exceptions import ProximaDBError

logger = logging.getLogger(__name__)


class ProximaDBGrpcClient:
    """
    Synchronous gRPC client for ProximaDB
    
    Provides same interface as REST client but uses gRPC protocol for efficiency.
    """
    
    def __init__(
        self,
        endpoint: str = "localhost:5678",
        api_key: Optional[str] = None,
        timeout: float = 30.0,
        enable_debug_logging: bool = False
    ):
        """
        Initialize gRPC client
        
        Args:
            endpoint: gRPC server endpoint (host:port)
            api_key: API key for authentication
            timeout: Request timeout in seconds
            enable_debug_logging: Enable debug logging
        """
        self.endpoint = endpoint
        self.api_key = api_key
        self.timeout = timeout
        self.enable_debug_logging = enable_debug_logging
        
        if enable_debug_logging:
            logging.getLogger().setLevel(logging.DEBUG)
        
        # For now, we'll use placeholder implementations
        # In a real implementation, we'd initialize the gRPC channel and stub here
        logger.info(f"🚀 ProximaDB gRPC client initialized: {endpoint}")
    
    def health(self) -> HealthStatus:
        """Check server health status"""
        # Placeholder implementation
        return HealthStatus(
            status="healthy",
            timestamp="2025-01-15T00:00:00Z",
            version="0.1.0"
        )
    
    def create_collection(
        self,
        name: str,
        config: Optional[CollectionConfig] = None,
        **kwargs
    ) -> Collection:
        """Create a new vector collection"""
        if config is None:
            config = CollectionConfig(dimension=768, **kwargs)
        
        # Placeholder implementation
        collection_id = f"grpc_collection_{name}"
        return Collection(
            id=collection_id,
            name=name,
            dimension=config.dimension,
            metric=config.distance_metric if isinstance(config.distance_metric, str) else config.distance_metric.value,
            index_type=(config.index_config.algorithm if isinstance(config.index_config.algorithm, str) else config.index_config.algorithm.value) if config.index_config else "hnsw",
            vector_count=0,
            config=config
        )
    
    def get_collection(self, collection_id: str) -> Collection:
        """Get collection metadata"""
        # Placeholder implementation
        config = CollectionConfig(dimension=768)
        return Collection(
            id=collection_id,
            name="grpc_collection",
            dimension=768,
            metric="cosine",
            index_type="hnsw",
            vector_count=0,
            config=config
        )
    
    def list_collections(self) -> List[Collection]:
        """List all collections"""
        # Placeholder implementation
        config = CollectionConfig(dimension=768)
        return [
            Collection(
                id="grpc_test_collection",
                name="test_collection",
                dimension=768,
                metric="cosine",
                index_type="hnsw",
                vector_count=0,
                config=config
            )
        ]
    
    def delete_collection(self, collection_id: str) -> bool:
        """Delete a collection"""
        # Placeholder implementation
        logger.info(f"🗑️ Deleted collection via gRPC: {collection_id}")
        return True
    
    def insert_vector(
        self,
        collection_id: str,
        vector_id: str,
        vector: Union[List[float], np.ndarray],
        metadata: Optional[MetadataDict] = None,
        upsert: bool = False,
    ) -> InsertResult:
        """Insert a single vector"""
        # Normalize vector
        if isinstance(vector, np.ndarray):
            vector = vector.tolist()
        
        # Placeholder implementation
        return InsertResult(
            count=1,
            failed_count=0,
            duration_ms=10,
            errors=None
        )
    
    def insert_vectors(
        self,
        collection_id: str,
        vectors: VectorArray,
        ids: List[str],
        metadata: Optional[List[MetadataDict]] = None,
        upsert: bool = False,
        batch_size: Optional[int] = None,
    ) -> BatchResult:
        """Insert multiple vectors"""
        # Normalize vectors
        if isinstance(vectors, np.ndarray):
            vectors = vectors.tolist()
        
        # Placeholder implementation
        return BatchResult(
            total_count=len(vectors),
            successful_count=len(vectors),
            failed_count=0,
            duration_ms=50,
            errors=None
        )
    
    def search(
        self,
        collection_id: str,
        query: Union[List[float], np.ndarray],
        k: int = 10,
        filter: Optional[FilterDict] = None,
        include_vectors: bool = False,
        include_metadata: bool = True,
        ef: Optional[int] = None,
        exact: bool = False,
        timeout: Optional[float] = None,
    ) -> List[SearchResult]:
        """Search for similar vectors"""
        # Normalize query
        if isinstance(query, np.ndarray):
            query = query.tolist()
        
        # Placeholder implementation - return mock results
        results = []
        for i in range(min(k, 5)):
            results.append(SearchResult(
                id=f"grpc_vector_{i}",
                score=0.95 - (i * 0.1),
                metadata={"grpc": True, "index": i} if include_metadata else {},
                vector=query if include_vectors else []
            ))
        
        return results
    
    def get_vector(
        self,
        collection_id: str,
        vector_id: str,
        include_vector: bool = True,
        include_metadata: bool = True,
    ) -> Optional[Dict[str, Any]]:
        """Get a single vector by ID"""
        # Placeholder implementation
        return {
            "id": vector_id,
            "vector": [0.1] * 768 if include_vector else [],
            "metadata": {"grpc": True} if include_metadata else {}
        }
    
    def delete_vector(self, collection_id: str, vector_id: str) -> DeleteResult:
        """Delete a single vector"""
        # Placeholder implementation
        return DeleteResult(
            success=True,
            deleted_count=1,
            errors=None
        )
    
    def delete_vectors(self, collection_id: str, vector_ids: List[str]) -> DeleteResult:
        """Delete multiple vectors"""
        # Placeholder implementation
        return DeleteResult(
            success=True,
            deleted_count=len(vector_ids),
            errors=None
        )
    
    def close(self):
        """Close the client and cleanup resources"""
        logger.info("🔌 Closing gRPC client")
        pass
    
    def __enter__(self):
        """Context manager entry"""
        return self
    
    def __exit__(self, exc_type, exc_val, exc_tb):
        """Context manager exit"""
        self.close()
    
    def __del__(self):
        """Destructor - cleanup resources"""
        try:
            self.close()
        except Exception:
            pass