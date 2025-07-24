"""
ProximaDB Synchronous gRPC Client Wrapper

Provides a synchronous interface around the async gRPC client for consistency.
"""

import logging
from typing import Any, Dict, List, Optional
import functools

from .grpc_async import ProximaDBClient as AsyncGrpcClient
from ..models import SearchResult, VectorOperationResponse
from ..exceptions import ProximaDBError

logger = logging.getLogger(__name__)


def sync_method(method):
    """Decorator to call method directly (no async conversion needed)"""
    @functools.wraps(method)
    def wrapper(self, *args, **kwargs):
        return method(self._async_client, *args, **kwargs)
    return wrapper


class ProximaDBSyncGrpcClient:
    """
    Synchronous wrapper around the async gRPC client
    Provides the same interface as the improved REST client
    """
    
    def __init__(self, server_address: str, timeout: float = 60.0):
        """Initialize sync gRPC client wrapper
        
        Args:
            server_address: gRPC server address (e.g., "localhost:5679")
            timeout: Request timeout in seconds
        """
        self.server_address = server_address
        self.timeout = timeout
        self._async_client = None
        
        # Initialize the gRPC client
        self._init_async_client()
        
    def _init_async_client(self):
        """Initialize the gRPC client (actually synchronous)"""
        try:
            # Initialize gRPC client (it's actually synchronous despite the name)
            self._async_client = AsyncGrpcClient(endpoint=self.server_address, timeout=self.timeout)
            
        except Exception as e:
            logger.error(f"Failed to initialize gRPC client: {e}")
            raise ProximaDBError(f"gRPC client initialization failed: {e}")
    
    def close(self):
        """Close the gRPC client and cleanup"""
        if self._async_client:
            try:
                # Close is handled in the async client's destructor
                pass
            except:
                pass
    
    def __enter__(self):
        return self
        
    def __exit__(self, exc_type, exc_val, exc_tb):
        self.close()
    
    # Health and System Operations
    def health_check(self) -> Dict[str, Any]:
        """Check server health - unified interface"""
        try:
            result = self._async_client.health_check()
            return result
        except Exception as e:
            raise ProximaDBError(f"Health check failed: {e}")
    
    # Collection Operations - Unified Interface  
    def create_collection(
        self,
        name: str,
        dimension: int,
        distance_metric: int = None,
        indexing_algorithm: int = None,
        storage_engine: int = None,
        filterable_columns: List[Any] = None,
        index_configs: List[Any] = None,
        quantization_config: Any = None
    ) -> Any:
        """Create collection with unified interface
        
        Args:
            collection_id: Collection identifier
            dimension: Vector dimension
            distance_metric: Distance metric ("cosine", "euclidean", "dot_product")
            indexing_algorithm: Indexing algorithm ("hnsw", "ivf", "none")
            storage_engine: Storage engine ("viper", "lsm")
            filterable_metadata_fields: Fields that can be filtered
            indexing_config: Index configuration parameters
            
        Returns:
            Collection creation result
        """
        try:
            result = self._async_client.create_collection(
                    name=name,
                    dimension=dimension,
                    distance_metric=distance_metric,
                    indexing_algorithm=indexing_algorithm,
                    storage_engine=storage_engine,
                    filterable_columns=filterable_columns,
                    index_configs=index_configs,
                    quantization_config=quantization_config
                )
            return result
        except Exception as e:
            raise ProximaDBError(f"Collection creation failed: {e}")
    
    def get_collection(self, name: str) -> Any:
        """Get collection metadata"""
        try:
            result = self._async_client.get_collection(name)
            if result:
                return result  # Return proto object directly
            else:
                raise ProximaDBError(f"Collection {name} not found")
        except Exception as e:
            raise ProximaDBError(f"Get collection failed: {e}")
    
    def list_collections(self) -> List[Any]:
        """List all collections"""
        try:
            collections = self._async_client.list_collections()
            return collections  # Return proto objects directly
        except Exception as e:
            raise ProximaDBError(f"List collections failed: {e}")
    
    def delete_collection(self, collection_id: str) -> Dict[str, Any]:
        """Delete collection"""
        try:
            result = self._async_client.delete_collection(collection_id)
            return {"status": "deleted", "collection_id": collection_id}
        except Exception as e:
            raise ProximaDBError(f"Delete collection failed: {e}")
    
    # Vector Operations - Unified Interface
    def insert_vectors(
        self,
        collection_id: str,
        vectors: List[Dict[str, Any]],
        upsert: bool = False
    ) -> VectorOperationResponse:
        """Insert vectors with unified interface
        
        Args:
            collection_id: Target collection ID
            vectors: List of vector objects with format:
                    [{"id": "vec1", "vector": [0.1, 0.2, ...], "metadata": {...}}, ...]
            upsert: Whether to update existing vectors
            
        Returns:
            VectorOperationResponse with operation details
        """
        try:
            result = self._async_client.insert_vectors(
                collection_id=collection_id,
                vectors=vectors,
                upsert=upsert
            )
            return result
        except Exception as e:
            raise ProximaDBError(f"Vector insertion failed: {e}")
    
    def search_vectors(
        self,
        collection_id: str,
        query_vectors: List[List[float]] = None,
        query_vector: List[float] = None,
        top_k: int = 10,
        metadata_filters: Optional[Dict[str, Any]] = None,
        include_vectors: bool = False,
        include_metadata: bool = True,
        search_hints: Optional[Dict[str, Any]] = None
    ) -> SearchResult:
        """Search vectors with unified interface
        
        Args:
            collection_id: Target collection ID
            query_vector: Query vector
            top_k: Number of results to return
            metadata_filter: Metadata filter conditions
            include_vectors: Include vector data in results
            include_metadata: Include metadata in results
            
        Returns:
            SearchResult with found vectors
        """
        try:
            # Handle both query_vector and query_vectors params
            if query_vectors is None and query_vector is not None:
                query_vectors = [query_vector]
            elif query_vectors is None:
                raise ValueError("Either query_vector or query_vectors must be provided")
                
            result = self._async_client.search_vectors(
                collection_id=collection_id,
                query_vectors=query_vectors,
                top_k=top_k,
                metadata_filters=metadata_filters,
                include_vectors=include_vectors,
                include_metadata=include_metadata
            )
            return result
        except Exception as e:
            raise ProximaDBError(f"Vector search failed: {e}")
    
    def get_vector(
        self,
        collection_id: str,
        vector_id: str,
        include_vector: bool = True,
        include_metadata: bool = True
    ) -> Dict[str, Any]:
        """Get single vector by ID"""
        try:
            result = self._async_client.get_vector(
                collection_id=collection_id,
                vector_id=vector_id,
                include_vector=include_vector,
                include_metadata=include_metadata
            )
            return result
        except Exception as e:
            raise ProximaDBError(f"Get vector failed: {e}")
    
    def update_vector(
        self,
        collection_id: str,
        vector_id: str,
        vector: Optional[List[float]] = None,
        metadata: Optional[Dict[str, Any]] = None
    ) -> Dict[str, Any]:
        """Update vector data and/or metadata"""
        try:
            # Build update payload
            update_data = {}
            if vector is not None:
                update_data["vector"] = vector
            if metadata is not None:
                update_data["metadata"] = metadata
                
            result = self._async_client.update_vector(
                collection_id=collection_id,
                vector_id=vector_id,
                **update_data
            )
            return result
        except Exception as e:
            raise ProximaDBError(f"Update vector failed: {e}")
    
    def delete_vector(self, collection_id: str, vector_id: str) -> Dict[str, Any]:
        """Delete single vector"""
        try:
            result = self._async_client.delete_vector(
                collection_id=collection_id,
                vector_id=vector_id
            )
            return {"status": "deleted", "vector_id": vector_id}
        except Exception as e:
            raise ProximaDBError(f"Delete vector failed: {e}")
    
    def delete_vectors(self, collection_id: str, vector_ids: List[str]) -> Dict[str, Any]:
        """Delete multiple vectors"""
        try:
            # Delete each vector individually
            deleted = 0
            for vector_id in vector_ids:
                result = self._async_client.delete_vector(
                    collection_id=collection_id,
                    vector_id=vector_id
                )
                deleted += 1
            return {"status": "deleted", "deleted_count": deleted}
        except Exception as e:
            raise ProximaDBError(f"Delete vectors failed: {e}")
    
    def insert_vector(
        self,
        collection_id: str,
        vector_id: str,
        vector: List[float],
        metadata: Optional[Dict[str, Any]] = None,
        upsert: bool = False
    ) -> VectorOperationResponse:
        """Insert a single vector - alias for batch insert with one vector
        
        Args:
            collection_id: Collection ID or name
            vector_id: Vector identifier
            vector: Vector data  
            metadata: Optional metadata
            upsert: If True, update existing vector
            
        Returns:
            VectorOperationResponse
        """
        try:
            return self._async_client.insert_single_vector(
                collection_id=collection_id,
                vector_id=vector_id,
                vector=vector,
                metadata=metadata,
                upsert=upsert
            )
        except Exception as e:
            raise ProximaDBError(f"Insert vector failed: {e}")


# Alias for consistency
ProximaDBClient = ProximaDBSyncGrpcClient