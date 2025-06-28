"""
ProximaDB Unified Python Client

Unified client interface that can use either REST or gRPC protocols.
Automatically selects gRPC for better performance when available,
with graceful fallback to REST for compatibility.
"""

import logging
import warnings
from typing import Any, Dict, List, Optional, Union
from enum import Enum

import numpy as np

from .config import ClientConfig, load_config
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


class Protocol(Enum):
    """Communication protocol options"""
    AUTO = "auto"      # Auto-select best available (gRPC preferred)
    GRPC = "grpc"      # Force gRPC (high performance, binary protocol)
    REST = "rest"      # Force REST (web compatibility)


class ProximaDBClient:
    """
    Unified ProximaDB Python Client
    
    Supports both REST and gRPC protocols with automatic selection
    for optimal performance and compatibility.
    """
    
    def __init__(
        self,
        url: Optional[str] = None,
        api_key: Optional[str] = None,
        protocol: Union[Protocol, str] = Protocol.AUTO,
        config: Optional[ClientConfig] = None,
        **kwargs
    ):
        """
        Initialize ProximaDB client
        
        Args:
            url: ProximaDB server URL
            api_key: API key for authentication  
            protocol: Communication protocol (auto, grpc, rest)
            config: Client configuration object
            **kwargs: Additional configuration parameters
        """
        if config is None:
            config = load_config(url=url, api_key=api_key, **kwargs)
        
        self.config = config
        self.protocol = Protocol(protocol) if isinstance(protocol, str) else protocol
        self._client = None
        self._setup_client()
    
    def _setup_client(self):
        """Setup the underlying client based on protocol preference"""
        if self.protocol == Protocol.AUTO:
            # Try gRPC first (high performance), then fallback to REST
            try:
                self._client = self._create_grpc_client()
                self._active_protocol = Protocol.GRPC
                logger.info("🔗 Using gRPC client for high performance")
            except ImportError:
                logger.warning("⚠️ gRPC dependencies not available, falling back to REST")
                self._client = self._create_rest_client()
                self._active_protocol = Protocol.REST
            except Exception as e:
                logger.warning(f"⚠️ gRPC client failed: {e}, falling back to REST")
                self._client = self._create_rest_client()
                self._active_protocol = Protocol.REST
                    
        elif self.protocol == Protocol.GRPC:
            # Force gRPC
            self._client = self._create_grpc_client()
            self._active_protocol = Protocol.GRPC
            logger.info("🔗 Using gRPC client (forced)")
            
        elif self.protocol == Protocol.REST:
            # Force REST
            self._client = self._create_rest_client()
            self._active_protocol = Protocol.REST
            logger.info("🌐 Using REST client (forced)")
        
        else:
            raise ValueError(f"Unknown protocol: {self.protocol}")
    
    def _create_grpc_client(self):
        """Create gRPC client"""
        try:
            from .grpc_client import ProximaDBClient as ProximaDBGrpcClient
            
            # Extract host and port from URL
            url = self.config.url
            if url.startswith(('http://', 'https://')):
                url = url.split('://', 1)[1]
            
            return ProximaDBGrpcClient(
                endpoint=url,
                timeout=self.config.timeout,
                enable_debug_logging=self.config.enable_debug_logging
            )
        except ImportError:
            raise ImportError("gRPC dependencies not available. Install with: pip install grpcio grpcio-tools protobuf")
    
    def _create_rest_client(self):
        """Create REST client"""
        from .rest_client import ProximaDBRestClient
        return ProximaDBRestClient(config=self.config)
    
    @property
    def active_protocol(self) -> Protocol:
        """Get the currently active protocol"""
        return self._active_protocol
    
    def get_performance_info(self) -> Dict[str, Any]:
        """Get performance information about the active protocol"""
        if self._active_protocol == Protocol.GRPC:
            return {
                "protocol": "gRPC",
                "advantages": [
                    "40% smaller payloads (binary protobuf vs JSON)",
                    "90% less overhead (HTTP/2 vs HTTP/1.1)",
                    "Better type safety with schema evolution",
                    "Streaming support for real-time operations"
                ],
                "serialization": "Binary Protocol Buffers",
                "transport": "HTTP/2"
            }
        else:
            return {
                "protocol": "REST",
                "advantages": [
                    "Universal compatibility",
                    "Easy debugging with standard tools",
                    "Human-readable JSON format"
                ],
                "serialization": "JSON",
                "transport": "HTTP/1.1"
            }
    
    # Delegate all vector database operations to the underlying client
    def health(self) -> HealthStatus:
        """Check server health status"""
        return self._client.health()
    
    def create_collection(
        self,
        name: str,
        config: Optional[CollectionConfig] = None,
        **kwargs
    ) -> Collection:
        """Create a new vector collection"""
        return self._client.create_collection(name, config, **kwargs)
    
    def get_collection(self, collection_id: str) -> Collection:
        """Get collection metadata"""
        return self._client.get_collection(collection_id)
    
    def list_collections(self) -> List[Collection]:
        """List all collections"""
        return self._client.list_collections()
    
    def delete_collection(self, collection_id: str) -> bool:
        """Delete a collection"""
        return self._client.delete_collection(collection_id)
    
    def insert_vector(
        self,
        collection_id: str,
        vector_id: str,
        vector: Union[List[float], np.ndarray],
        metadata: Optional[MetadataDict] = None,
        upsert: bool = False,
    ) -> InsertResult:
        """Insert a single vector"""
        return self._client.insert_vector(collection_id, vector_id, vector, metadata, upsert)
    
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
        return self._client.insert_vectors(collection_id, vectors, ids, metadata, upsert, batch_size)
    
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
        return self._client.search(
            collection_id, query, k, filter, include_vectors, 
            include_metadata, ef, exact, timeout
        )
    
    def get_vector(
        self,
        collection_id: str,
        vector_id: str,
        include_vector: bool = True,
        include_metadata: bool = True,
    ) -> Optional[Dict[str, Any]]:
        """Get a single vector by ID"""
        return self._client.get_vector(collection_id, vector_id, include_vector, include_metadata)
    
    def delete_vector(self, collection_id: str, vector_id: str) -> DeleteResult:
        """Delete a single vector"""
        return self._client.delete_vector(collection_id, vector_id)
    
    def delete_vectors(self, collection_id: str, vector_ids: List[str]) -> DeleteResult:
        """Delete multiple vectors"""
        return self._client.delete_vectors(collection_id, vector_ids)
    
    def update_collection(
        self,
        collection_id: str,
        updates: Dict[str, Any]
    ) -> Collection:
        """Update collection metadata and configuration"""
        return self._client.update_collection(collection_id, updates)
    
    def get_collection_id_by_name(self, collection_name: str) -> Optional[str]:
        """
        Get collection UUID by name.
        
        This method allows you to resolve a collection name to its UUID,
        which can be useful for operations that specifically need the UUID.
        
        Args:
            collection_name: Name of the collection
            
        Returns:
            Collection UUID if found, None if not found
            
        Raises:
            ProximaDBError: If there's an error retrieving the collection
        """
        if hasattr(self._client, 'get_collection_id_by_name'):
            return self._client.get_collection_id_by_name(collection_name)
        else:
            # Fallback for clients that don't have this method
            try:
                collection = self.get_collection(collection_name)
                if collection and hasattr(collection, 'id'):
                    return collection.id
                return None
            except Exception:
                return None
    
    def delete_vectors_by_filter(
        self,
        collection_id: str,
        filter: FilterDict
    ) -> DeleteResult:
        """Delete vectors matching filter criteria"""
        return self._client.delete_vectors_by_filter(collection_id, filter)
    
    def get_vector_history(
        self,
        collection_id: str,
        vector_id: str,
        limit: int = 10
    ) -> List[Dict[str, Any]]:
        """Get vector version history"""
        return self._client.get_vector_history(collection_id, vector_id, limit)
    
    def multi_search(
        self,
        collection_id: str,
        queries: List[Union[List[float], np.ndarray]],
        k: int = 10,
        filter: Optional[FilterDict] = None,
        include_vectors: bool = False,
        include_metadata: bool = True
    ) -> List[SearchResult]:
        """Search with multiple query vectors"""
        return self._client.multi_search(
            collection_id, queries, k, filter, include_vectors, include_metadata
        )
    
    def search_with_aggregations(
        self,
        collection_id: str,
        query: Union[List[float], np.ndarray],
        aggregations: List[str],
        k: int = 10,
        group_by: Optional[str] = None,
        filter: Optional[FilterDict] = None
    ) -> Dict[str, Any]:
        """Search with result aggregations"""
        return self._client.search_with_aggregations(
            collection_id, query, k, aggregations, group_by, filter
        )
    
    def atomic_insert_vectors(
        self,
        collection_id: str,
        vectors: VectorArray,
        ids: List[str],
        metadata: Optional[List[MetadataDict]] = None
    ) -> BatchResult:
        """Insert vectors atomically (all-or-nothing)"""
        return self._client.atomic_insert_vectors(collection_id, vectors, ids, metadata)
    
    def begin_transaction(self) -> str:
        """Begin a new transaction and return transaction ID"""
        return self._client.begin_transaction()
    
    def commit_transaction(self, transaction_id: str) -> bool:
        """Commit a transaction"""
        return self._client.commit_transaction(transaction_id)
    
    def rollback_transaction(self, transaction_id: str) -> bool:
        """Rollback a transaction"""
        return self._client.rollback_transaction(transaction_id)
    
    def close(self):
        """Close the client and cleanup resources"""
        if self._client and hasattr(self._client, 'close'):
            self._client.close()
    
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


# Convenience functions for backward compatibility
def connect(
    url: Optional[str] = None,
    api_key: Optional[str] = None,
    protocol: Union[Protocol, str] = Protocol.AUTO,
    **kwargs
) -> ProximaDBClient:
    """Create a ProximaDB client with simplified parameters"""
    return ProximaDBClient(url=url, api_key=api_key, protocol=protocol, **kwargs)



def connect_grpc(
    url: Optional[str] = None,
    api_key: Optional[str] = None,
    **kwargs
) -> ProximaDBClient:
    """Create a ProximaDB client using gRPC protocol (good performance, ecosystem compatibility)"""
    return ProximaDBClient(url=url, api_key=api_key, protocol=Protocol.GRPC, **kwargs)


def connect_rest(
    url: Optional[str] = None,
    api_key: Optional[str] = None,
    **kwargs
) -> ProximaDBClient:
    """Create a ProximaDB client using REST protocol (web compatibility)"""
    return ProximaDBClient(url=url, api_key=api_key, protocol=Protocol.REST, **kwargs)