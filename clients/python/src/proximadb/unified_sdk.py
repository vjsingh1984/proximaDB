"""
ProximaDB Unified SDK - Protocol-Agnostic Client

Provides a single, consistent interface that works with both REST and gRPC protocols.
Automatically selects the best available protocol or allows manual selection.
"""

import logging
from typing import Any, Dict, List, Optional, Union
from enum import Enum
import time

from .improved_rest_client import ProximaDBRestClient
from .sync_grpc_client import ProximaDBSyncGrpcClient
from .models import InsertResult, SearchResult
from .exceptions import ProximaDBError

logger = logging.getLogger(__name__)


class Protocol(Enum):
    """Available protocols"""
    AUTO = "auto"
    GRPC = "grpc" 
    REST = "rest"


class ProximaDBUnifiedClient:
    """
    Unified ProximaDB Client with consistent interface across protocols
    
    This client provides the same interface regardless of whether it's using
    gRPC or REST internally, solving the interface inconsistency problem.
    """
    
    def __init__(
        self,
        server_url: str,
        protocol: Protocol = Protocol.AUTO,
        timeout: float = 30.0,
        prefer_grpc: bool = True
    ):
        """Initialize unified client
        
        Args:
            server_url: Server URL. Examples:
                       - "localhost:5679" (gRPC)
                       - "http://localhost:5678" (REST)
                       - "localhost" (auto-detect: try 5679 then 5678)
            protocol: Protocol to use (AUTO, GRPC, REST)
            timeout: Request timeout in seconds
            prefer_grpc: When protocol=AUTO, prefer gRPC over REST
        """
        self.server_url = server_url
        self.protocol = protocol
        self.timeout = timeout
        self.prefer_grpc = prefer_grpc
        
        self._client = None
        self._active_protocol = None
        
        # Initialize the appropriate client
        self._initialize_client()
    
    def _initialize_client(self):
        """Initialize the appropriate client based on protocol selection"""
        
        if self.protocol == Protocol.GRPC:
            self._init_grpc_client()
        elif self.protocol == Protocol.REST:
            self._init_rest_client()
        elif self.protocol == Protocol.AUTO:
            self._init_auto_client()
        else:
            raise ProximaDBError(f"Unsupported protocol: {self.protocol}")
    
    def _init_grpc_client(self):
        """Initialize gRPC client"""
        try:
            # Ensure server_url is in gRPC format
            grpc_url = self._extract_grpc_url(self.server_url)
            self._client = ProximaDBSyncGrpcClient(grpc_url, self.timeout)
            self._active_protocol = Protocol.GRPC
            logger.info(f"Using gRPC protocol: {grpc_url}")
        except Exception as e:
            raise ProximaDBError(f"Failed to initialize gRPC client: {e}")
    
    def _init_rest_client(self):
        """Initialize REST client"""
        try:
            # Ensure server_url is in REST format
            rest_url = self._extract_rest_url(self.server_url)
            self._client = ProximaDBRestClient(rest_url, self.timeout)
            self._active_protocol = Protocol.REST
            logger.info(f"Using REST protocol: {rest_url}")
        except Exception as e:
            raise ProximaDBError(f"Failed to initialize REST client: {e}")
    
    def _init_auto_client(self):
        """Auto-select best available protocol"""
        protocols_to_try = [Protocol.GRPC, Protocol.REST] if self.prefer_grpc else [Protocol.REST, Protocol.GRPC]
        
        last_error = None
        for proto in protocols_to_try:
            try:
                if proto == Protocol.GRPC:
                    self._init_grpc_client()
                else:
                    self._init_rest_client()
                
                # Test connection with health check
                self.health_check()
                logger.info(f"Auto-selected protocol: {self._active_protocol.value}")
                return
                
            except Exception as e:
                last_error = e
                logger.debug(f"Failed to connect with {proto.value}: {e}")
                continue
        
        raise ProximaDBError(f"Failed to connect with any protocol. Last error: {last_error}")
    
    def _extract_grpc_url(self, url: str) -> str:
        """Extract gRPC URL from various formats"""
        if url.startswith(("http://", "https://")):
            # Convert REST URL to gRPC
            host = url.split("://")[1].split(":")[0]
            return f"{host}:5679"
        elif ":" in url:
            return url  # Already in host:port format
        else:
            return f"{url}:5679"  # Add default gRPC port
    
    def _extract_rest_url(self, url: str) -> str:
        """Extract REST URL from various formats"""
        if url.startswith(("http://", "https://")):
            return url  # Already in REST format
        elif ":" in url:
            # Convert gRPC URL to REST
            host, port = url.split(":")
            rest_port = "5678" if port == "5679" else port
            return f"http://{host}:{rest_port}"
        else:
            return f"http://{url}:5678"  # Add default REST URL
    
    @property
    def active_protocol(self) -> Protocol:
        """Get the currently active protocol"""
        return self._active_protocol
    
    def close(self):
        """Close the underlying client"""
        if self._client:
            self._client.close()
    
    def __enter__(self):
        return self
        
    def __exit__(self, exc_type, exc_val, exc_tb):
        self.close()
    
    # =============================================================================
    # UNIFIED API - Same interface for both gRPC and REST
    # =============================================================================
    
    def health_check(self) -> Dict[str, Any]:
        """Check server health"""
        return self._client.health_check()
    
    def create_collection(
        self,
        collection_id: str,
        dimension: int,
        distance_metric: str = "cosine",
        indexing_algorithm: str = "hnsw",
        storage_engine: str = "viper",
        filterable_metadata_fields: Optional[List[str]] = None,
        indexing_config: Optional[Dict[str, Any]] = None
    ) -> Dict[str, Any]:
        """Create a new collection
        
        Args:
            collection_id: Unique collection identifier
            dimension: Vector dimension (e.g., 384, 768, 1536)
            distance_metric: "cosine", "euclidean", or "dot_product"
            indexing_algorithm: "hnsw", "ivf", or "none"
            storage_engine: "viper" or "lsm"
            filterable_metadata_fields: Metadata fields that can be filtered
            indexing_config: Additional index configuration
            
        Returns:
            Collection creation result
        """
        return self._client.create_collection(
            collection_id=collection_id,
            dimension=dimension,
            distance_metric=distance_metric,
            indexing_algorithm=indexing_algorithm,
            storage_engine=storage_engine,
            filterable_metadata_fields=filterable_metadata_fields,
            indexing_config=indexing_config
        )
    
    def get_collection(self, collection_id: str) -> Dict[str, Any]:
        """Get collection metadata"""
        return self._client.get_collection(collection_id)
    
    def list_collections(self) -> List[Dict[str, Any]]:
        """List all collections"""
        return self._client.list_collections()
    
    def delete_collection(self, collection_id: str) -> Dict[str, Any]:
        """Delete a collection"""
        return self._client.delete_collection(collection_id)
    
    def insert_vectors(
        self,
        collection_id: str,
        vectors: List[Dict[str, Any]],
        upsert: bool = False
    ) -> InsertResult:
        """Insert vectors into collection
        
        Args:
            collection_id: Target collection
            vectors: Vector data in format:
                    [{"id": "vec1", "vector": [0.1, 0.2, ...], "metadata": {...}}, ...]
            upsert: Update existing vectors if they exist
            
        Returns:
            InsertResult with operation details
        """
        return self._client.insert_vectors(
            collection_id=collection_id,
            vectors=vectors,
            upsert=upsert
        )
    
    def search_vectors(
        self,
        collection_id: str,
        query_vector: List[float],
        top_k: int = 10,
        metadata_filter: Optional[Dict[str, Any]] = None,
        include_vectors: bool = False,
        include_metadata: bool = True
    ) -> SearchResult:
        """Search for similar vectors
        
        Args:
            collection_id: Target collection
            query_vector: Query vector for similarity search
            top_k: Number of most similar vectors to return
            metadata_filter: Filter results by metadata conditions
            include_vectors: Include vector data in results
            include_metadata: Include metadata in results
            
        Returns:
            SearchResult with similar vectors
        """
        return self._client.search_vectors(
            collection_id=collection_id,
            query_vector=query_vector,
            top_k=top_k,
            metadata_filter=metadata_filter,
            include_vectors=include_vectors,
            include_metadata=include_metadata
        )
    
    def get_vector(
        self,
        collection_id: str,
        vector_id: str,
        include_vector: bool = True,
        include_metadata: bool = True
    ) -> Dict[str, Any]:
        """Get a specific vector by ID"""
        return self._client.get_vector(
            collection_id=collection_id,
            vector_id=vector_id,
            include_vector=include_vector,
            include_metadata=include_metadata
        )
    
    def update_vector(
        self,
        collection_id: str,
        vector_id: str,
        vector: Optional[List[float]] = None,
        metadata: Optional[Dict[str, Any]] = None
    ) -> Dict[str, Any]:
        """Update vector data and/or metadata"""
        return self._client.update_vector(
            collection_id=collection_id,
            vector_id=vector_id,
            vector=vector,
            metadata=metadata
        )
    
    def delete_vector(self, collection_id: str, vector_id: str) -> Dict[str, Any]:
        """Delete a vector"""
        return self._client.delete_vector(collection_id, vector_id)


# Main client class - users should use this
class ProximaDBClient(ProximaDBUnifiedClient):
    """
    Main ProximaDB Client - Unified interface for all protocols
    
    Usage:
        # Auto-select best protocol
        client = ProximaDBClient("localhost")
        
        # Force specific protocol
        client = ProximaDBClient("localhost:5679", protocol=Protocol.GRPC)
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
    """
    pass


# Convenience functions
def connect(server_url: str, **kwargs) -> ProximaDBClient:
    """Connect to ProximaDB with auto-protocol selection"""
    return ProximaDBClient(server_url, **kwargs)

def connect_grpc(server_url: str, **kwargs) -> ProximaDBClient:
    """Connect to ProximaDB using gRPC protocol"""
    return ProximaDBClient(server_url, protocol=Protocol.GRPC, **kwargs)

def connect_rest(server_url: str, **kwargs) -> ProximaDBClient:
    """Connect to ProximaDB using REST protocol"""
    return ProximaDBClient(server_url, protocol=Protocol.REST, **kwargs)