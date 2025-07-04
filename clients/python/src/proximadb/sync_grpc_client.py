"""
ProximaDB Synchronous gRPC Client Wrapper

Provides a synchronous interface around the async gRPC client for consistency.
"""

import asyncio
import logging
from typing import Any, Dict, List, Optional
import functools

from .grpc_client import ProximaDBClient as AsyncGrpcClient
from .models import InsertResult, SearchResult
from .exceptions import ProximaDBError

logger = logging.getLogger(__name__)


def sync_method(async_method):
    """Decorator to convert async method to sync"""
    @functools.wraps(async_method)
    def wrapper(self, *args, **kwargs):
        return asyncio.run(async_method(self._async_client, *args, **kwargs))
    return wrapper


class ProximaDBSyncGrpcClient:
    """
    Synchronous wrapper around the async gRPC client
    Provides the same interface as the improved REST client
    """
    
    def __init__(self, server_address: str, timeout: float = 30.0):
        """Initialize sync gRPC client wrapper
        
        Args:
            server_address: gRPC server address (e.g., "localhost:5679")
            timeout: Request timeout in seconds
        """
        self.server_address = server_address
        self.timeout = timeout
        self._async_client = None
        self._loop = None
        
        # Initialize the async client
        self._init_async_client()
        
    def _init_async_client(self):
        """Initialize the async gRPC client"""
        try:
            # Create new event loop for this client
            self._loop = asyncio.new_event_loop()
            asyncio.set_event_loop(self._loop)
            
            # Initialize async client
            self._async_client = AsyncGrpcClient(self.server_address)
            
            # Connect to server
            self._loop.run_until_complete(self._async_client.connect())
            
        except Exception as e:
            logger.error(f"Failed to initialize gRPC client: {e}")
            raise ProximaDBError(f"gRPC client initialization failed: {e}")
    
    def close(self):
        """Close the gRPC client and cleanup"""
        if self._async_client and self._loop:
            try:
                self._loop.run_until_complete(self._async_client.close())
            except:
                pass
            finally:
                if self._loop and not self._loop.is_closed():
                    self._loop.close()
    
    def __enter__(self):
        return self
        
    def __exit__(self, exc_type, exc_val, exc_tb):
        self.close()
    
    # Health and System Operations
    def health_check(self) -> Dict[str, Any]:
        """Check server health - unified interface"""
        try:
            result = asyncio.run(self._async_client.health_check())
            return result
        except Exception as e:
            raise ProximaDBError(f"Health check failed: {e}")
    
    # Collection Operations - Unified Interface  
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
            # Convert string parameters to enum values for gRPC
            distance_metric_map = {"cosine": 1, "euclidean": 2, "dot_product": 3}
            indexing_algorithm_map = {"hnsw": 1, "ivf": 2, "none": 0}
            storage_engine_map = {"viper": 1, "lsm": 2}
            
            result = asyncio.run(self._async_client.create_collection(
                name=collection_id,
                dimension=dimension,
                distance_metric=distance_metric_map.get(distance_metric, 1),
                indexing_algorithm=indexing_algorithm_map.get(indexing_algorithm, 1),
                storage_engine=storage_engine_map.get(storage_engine, 1),
                filterable_metadata_fields=filterable_metadata_fields or [],
                indexing_config=indexing_config or {}
            ))
            
            return {
                "collection_id": result.name,
                "dimension": result.dimension,
                "status": "created"
            }
            
        except Exception as e:
            raise ProximaDBError(f"Collection creation failed: {e}")
    
    def get_collection(self, collection_id: str) -> Dict[str, Any]:
        """Get collection metadata"""
        try:
            result = asyncio.run(self._async_client.get_collection(collection_id))
            if result:
                return {
                    "collection_id": result.name,
                    "dimension": result.dimension,
                    "distance_metric": result.distance_metric,
                    "storage_engine": result.storage_engine
                }
            else:
                raise ProximaDBError(f"Collection {collection_id} not found")
        except Exception as e:
            raise ProximaDBError(f"Get collection failed: {e}")
    
    def list_collections(self) -> List[Dict[str, Any]]:
        """List all collections"""
        try:
            collections = asyncio.run(self._async_client.list_collections())
            return [
                {
                    "collection_id": c.name,
                    "dimension": c.dimension,
                    "distance_metric": c.distance_metric,
                    "storage_engine": c.storage_engine
                }
                for c in collections
            ]
        except Exception as e:
            raise ProximaDBError(f"List collections failed: {e}")
    
    def delete_collection(self, collection_id: str) -> Dict[str, Any]:
        """Delete collection"""
        try:
            result = asyncio.run(self._async_client.delete_collection(collection_id))
            return {"status": "deleted", "collection_id": collection_id}
        except Exception as e:
            raise ProximaDBError(f"Delete collection failed: {e}")
    
    # Vector Operations - Unified Interface
    def insert_vectors(
        self,
        collection_id: str,
        vectors: List[Dict[str, Any]],
        upsert: bool = False
    ) -> InsertResult:
        """Insert vectors with unified interface
        
        Args:
            collection_id: Target collection ID
            vectors: List of vector objects with format:
                    [{"id": "vec1", "vector": [0.1, 0.2, ...], "metadata": {...}}, ...]
            upsert: Whether to update existing vectors
            
        Returns:
            InsertResult with operation details
        """
        try:
            result = asyncio.run(self._async_client.insert_vectors(
                collection_id=collection_id,
                vectors=vectors,
                upsert=upsert
            ))
            return result
        except Exception as e:
            raise ProximaDBError(f"Vector insertion failed: {e}")
    
    def search_vectors(
        self,
        collection_id: str,
        query_vector: List[float],
        top_k: int = 10,
        metadata_filter: Optional[Dict[str, Any]] = None,
        include_vectors: bool = False,
        include_metadata: bool = True
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
            result = asyncio.run(self._async_client.search_vectors(
                collection_id=collection_id,
                query_vectors=[query_vector],
                top_k=top_k,
                metadata_filters=metadata_filter,
                include_vectors=include_vectors,
                include_metadata=include_metadata
            ))
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
            result = asyncio.run(self._async_client.get_vector(
                collection_id=collection_id,
                vector_id=vector_id,
                include_vector=include_vector,
                include_metadata=include_metadata
            ))
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
                
            result = asyncio.run(self._async_client.update_vector(
                collection_id=collection_id,
                vector_id=vector_id,
                **update_data
            ))
            return result
        except Exception as e:
            raise ProximaDBError(f"Update vector failed: {e}")
    
    def delete_vector(self, collection_id: str, vector_id: str) -> Dict[str, Any]:
        """Delete single vector"""
        try:
            result = asyncio.run(self._async_client.delete_vector(
                collection_id=collection_id,
                vector_id=vector_id
            ))
            return {"status": "deleted", "vector_id": vector_id}
        except Exception as e:
            raise ProximaDBError(f"Delete vector failed: {e}")


# Alias for consistency
ProximaDBClient = ProximaDBSyncGrpcClient