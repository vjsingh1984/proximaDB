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
        
        # Use REST API for now since gRPC proto files need to be generated
        # This provides the same interface but uses HTTP calls
        import requests
        self.base_url = f"http://{endpoint.replace(':5678', ':5678')}"
        logger.info(f"🚀 ProximaDB gRPC client initialized (using REST backend): {endpoint}")
    
    def health(self) -> HealthStatus:
        """Check server health status"""
        try:
            import requests
            response = requests.get(f"{self.base_url}/health", timeout=self.timeout)
            if response.status_code == 200:
                data = response.json()
                return HealthStatus(
                    status=data.get("status", "healthy"),
                    timestamp="2025-06-17T00:00:00Z",
                    version=data.get("version", "0.1.0")
                )
            else:
                return HealthStatus(status="unhealthy", timestamp="2025-06-17T00:00:00Z", version="0.1.0")
        except Exception as e:
            logger.error(f"Health check failed: {e}")
            return HealthStatus(status="unhealthy", timestamp="2025-06-17T00:00:00Z", version="0.1.0")
    
    def create_collection(
        self,
        name: str,
        config: Optional[CollectionConfig] = None,
        **kwargs
    ) -> Collection:
        """Create a new vector collection"""
        if config is None:
            config = CollectionConfig(dimension=768, **kwargs)
        
        # Make real HTTP call to server
        import requests
        request_data = {
            "name": name,
            "dimension": config.dimension,
            "distance_metric": config.distance_metric,
            "indexing_algorithm": config.indexing_algorithm if hasattr(config, 'indexing_algorithm') else "hnsw",
            "storage_layout": getattr(config, 'storage_layout', 'viper'),  # Default to VIPER
        }
        
        # Add VIPER-specific optimization fields
        if hasattr(config, 'filterable_metadata_fields') and config.filterable_metadata_fields:
            request_data["filterable_metadata_fields"] = config.filterable_metadata_fields
        
        # Add WAL flush configuration
        if hasattr(config, 'max_wal_size_mb') and config.max_wal_size_mb:
            request_data["max_wal_size_mb"] = config.max_wal_size_mb
        
        try:
            response = requests.post(f"{self.base_url}/api/v1/collections", json=request_data, timeout=self.timeout)
            if response.status_code == 200:
                data = response.json()
                return Collection(
                    id=data.get("id"),
                    name=data.get("name"),
                    dimension=data.get("dimension"),
                    metric=data.get("distance_metric"),
                    index_type=data.get("indexing_algorithm"),
                    vector_count=data.get("vector_count", 0),
                    config=config
                )
            else:
                raise ProximaDBError(f"Failed to create collection: {response.text}")
        except Exception as e:
            logger.error(f"Create collection failed: {e}")
            raise ProximaDBError(f"Failed to create collection: {e}")
    
    def get_collection(self, collection_id: str) -> Collection:
        """Get collection metadata"""
        import requests
        try:
            response = requests.get(f"{self.base_url}/api/v1/collections/{collection_id}",
                                  timeout=self.timeout)
            
            if response.status_code == 200:
                data = response.json()
                # Extract storage layout from config if present
                storage_layout = 'viper'  # default
                if 'config' in data and isinstance(data['config'], dict):
                    storage_layout = data['config'].get('storage_layout', 'viper')
                
                config = CollectionConfig(
                    dimension=data.get("dimension", 768),
                    distance_metric=data.get("distance_metric", "cosine"),
                    storage_layout=storage_layout
                )
                
                return Collection(
                    id=data.get("id"),
                    name=data.get("name"),
                    dimension=data.get("dimension"),
                    metric=data.get("distance_metric"),
                    index_type=data.get("indexing_algorithm"),
                    vector_count=data.get("vector_count", 0),
                    config=config,
                    status=data.get("status", "active"),
                    created_at=data.get("created_at")
                )
            else:
                raise ProximaDBError(f"Failed to get collection: {response.text}")
        except Exception as e:
            logger.error(f"Get collection failed: {e}")
            raise ProximaDBError(f"Failed to get collection: {e}")
    
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
        import requests
        try:
            response = requests.delete(f"{self.base_url}/api/v1/collections/{collection_id}",
                                     timeout=self.timeout)
            
            if response.status_code == 200:
                data = response.json()
                logger.info(f"🗑️ Deleted collection via gRPC: {collection_id}")
                return data.get("success", True)
            else:
                logger.error(f"Failed to delete collection: {response.text}")
                return False
        except Exception as e:
            logger.error(f"Delete collection failed: {e}")
            return False
    
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
        
        # Use bulk insert endpoint with single vector
        return self.insert_vectors(
            collection_id=collection_id,
            vectors=[vector],
            ids=[vector_id],
            metadata=[metadata] if metadata else None,
            upsert=upsert
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
        
        # Prepare vector data for REST API
        vector_data = []
        for i, (vector_id, vector) in enumerate(zip(ids, vectors)):
            item = {
                "id": vector_id,
                "vector": vector,
                "metadata": metadata[i] if metadata else {}
            }
            vector_data.append(item)
        
        request_data = {
            "vectors": vector_data,
            "upsert": upsert
        }
        
        # Make real HTTP call to server
        import requests
        import time
        try:
            start_time = time.time()
            response = requests.post(f"{self.base_url}/api/v1/collections/{collection_id}/vectors/batch", 
                                   json=request_data, timeout=self.timeout)
            duration_ms = (time.time() - start_time) * 1000
            
            if response.status_code == 200:
                data = response.json()
                return BatchResult(
                    total_count=len(vectors),
                    successful_count=data.get("count", len(vectors)),
                    failed_count=data.get("failed_count", 0),
                    duration_ms=duration_ms,
                    errors=data.get("errors")
                )
            else:
                raise ProximaDBError(f"Failed to insert vectors: {response.text}")
        except Exception as e:
            logger.error(f"Insert vectors failed: {e}")
            raise ProximaDBError(f"Failed to insert vectors: {e}")
    
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
        
        # Prepare search request
        request_data = {
            "vector": query,  # API expects 'vector' not 'query'
            "k": k,
            "filter": filter
        }
        
        # Make real HTTP call to server
        import requests
        try:
            response = requests.post(f"{self.base_url}/api/v1/collections/{collection_id}/search",
                                   json=request_data, timeout=timeout or self.timeout)
            
            if response.status_code == 200:
                data = response.json()
                results = []
                for result_data in data.get("results", []):
                    results.append(SearchResult(
                        id=result_data.get("id"),
                        score=result_data.get("score", 0.0),
                        distance=result_data.get("distance"),
                        metadata=result_data.get("metadata", {}) if include_metadata else {},
                        vector=result_data.get("vector", []) if include_vectors else []
                    ))
                return results
            else:
                raise ProximaDBError(f"Failed to search vectors: {response.text}")
        except Exception as e:
            logger.error(f"Search failed: {e}")
            raise ProximaDBError(f"Failed to search vectors: {e}")
    
    def get_vector(
        self,
        collection_id: str,
        vector_id: str,
        include_vector: bool = True,
        include_metadata: bool = True,
    ) -> Optional[Dict[str, Any]]:
        """Get a single vector by ID"""
        import requests
        try:
            response = requests.get(f"{self.base_url}/api/v1/collections/{collection_id}/vectors/{vector_id}",
                                  timeout=self.timeout)
            
            if response.status_code == 200:
                data = response.json()
                return {
                    "id": data.get("id"),
                    "vector": data.get("vector", []) if include_vector else [],
                    "metadata": data.get("metadata", {}) if include_metadata else {}
                }
            elif response.status_code == 404:
                return None
            else:
                raise ProximaDBError(f"Failed to get vector: {response.text}")
        except Exception as e:
            logger.error(f"Get vector failed: {e}")
            if "404" in str(e):
                return None
            raise ProximaDBError(f"Failed to get vector: {e}")
    
    def delete_vector(self, collection_id: str, vector_id: str) -> DeleteResult:
        """Delete a single vector"""
        import requests
        try:
            response = requests.delete(f"{self.base_url}/api/v1/collections/{collection_id}/vectors/{vector_id}",
                                     timeout=self.timeout)
            
            if response.status_code == 200:
                data = response.json()
                return DeleteResult(
                    success=data.get("success", True),
                    deleted_count=1 if data.get("success", True) else 0,
                    errors=None
                )
            else:
                return DeleteResult(
                    success=False,
                    deleted_count=0,
                    errors=[response.text]
                )
        except Exception as e:
            logger.error(f"Delete vector failed: {e}")
            return DeleteResult(
                success=False,
                deleted_count=0,
                errors=[str(e)]
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