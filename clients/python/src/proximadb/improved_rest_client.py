"""
ProximaDB Improved REST Client - Unified Interface

This client provides the same interface as the gRPC client for consistency.
"""

import logging
import time
from typing import Any, Dict, List, Optional, Union
import json

import numpy as np
import httpx
from tenacity import retry, stop_after_attempt, wait_exponential, retry_if_exception_type

from .models import InsertResult, SearchResult, Collection
from .exceptions import ProximaDBError, NetworkError, map_http_error

logger = logging.getLogger(__name__)


class ProximaDBRestClient:
    """
    ProximaDB REST Client with unified interface matching gRPC client
    """
    
    def __init__(self, base_url: str, timeout: float = 30.0):
        """Initialize REST client
        
        Args:
            base_url: Base URL for ProximaDB REST API (e.g., "http://localhost:5678")
            timeout: Request timeout in seconds
        """
        self.base_url = base_url.rstrip('/')
        self.timeout = timeout
        self._session = httpx.Client(timeout=timeout)
        
    def __enter__(self):
        return self
        
    def __exit__(self, exc_type, exc_val, exc_tb):
        self._session.close()
        
    def close(self):
        """Close the HTTP session"""
        self._session.close()
        
    def _make_request(self, method: str, endpoint: str, **kwargs) -> httpx.Response:
        """Make HTTP request with error handling"""
        url = f"{self.base_url}{endpoint}"
        
        try:
            response = self._session.request(method, url, **kwargs)
            response.raise_for_status()
            return response
        except httpx.HTTPError as e:
            if hasattr(e, 'response') and e.response is not None:
                try:
                    response_data = e.response.json()
                except:
                    response_data = {"message": e.response.text}
                error = map_http_error(e.response.status_code, response_data)
                raise error
            raise NetworkError(f"Request failed: {e}")
    
    # Health and System Operations
    def health_check(self) -> Dict[str, Any]:
        """Check server health - unified interface"""
        response = self._make_request("GET", "/health")
        data = response.json()
        return {
            "status": data.get("status", "unknown"),
            "version": data.get("version", "unknown")
        }
    
    # Collection Operations - Unified Interface (matches gRPC)
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
        """Create collection with unified interface matching gRPC
        
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
        payload = {
            "collection_id": collection_id,
            "dimension": dimension,
            "distance_metric": distance_metric,
            "indexing_algorithm": indexing_algorithm,
            "storage_engine": storage_engine,
            "filterable_metadata_fields": filterable_metadata_fields or [],
            "indexing_config": indexing_config or {}
        }
        
        response = self._make_request("POST", "/api/v1/collections", json=payload)
        return response.json()
    
    def get_collection(self, collection_id: str) -> Dict[str, Any]:
        """Get collection metadata"""
        response = self._make_request("GET", f"/api/v1/collections/{collection_id}")
        return response.json()
    
    def list_collections(self) -> List[Dict[str, Any]]:
        """List all collections"""
        response = self._make_request("GET", "/api/v1/collections")
        data = response.json()
        return data.get("collections", [])
    
    def delete_collection(self, collection_id: str) -> Dict[str, Any]:
        """Delete collection"""
        response = self._make_request("DELETE", f"/api/v1/collections/{collection_id}")
        return response.json()
    
    # Vector Operations - Unified Interface (matches gRPC)
    def insert_vectors(
        self,
        collection_id: str,
        vectors: List[Dict[str, Any]],
        upsert: bool = False
    ) -> InsertResult:
        """Insert vectors with unified interface matching gRPC
        
        Args:
            collection_id: Target collection ID
            vectors: List of vector objects with format:
                    [{"id": "vec1", "vector": [0.1, 0.2, ...], "metadata": {...}}, ...]
            upsert: Whether to update existing vectors
            
        Returns:
            InsertResult with operation details
        """
        # Convert unified format to REST API format
        payload = {
            "vectors": vectors,
            "upsert": upsert
        }
        
        start_time = time.time()
        response = self._make_request(
            "POST", 
            f"/api/v1/collections/{collection_id}/vectors",
            json=payload
        )
        duration_ms = (time.time() - start_time) * 1000
        
        data = response.json()
        return InsertResult(
            count=data.get("inserted_count", len(vectors)),
            failed_count=data.get("failed_count", 0),
            duration_ms=duration_ms,
            request_id=data.get("request_id"),
            errors=data.get("errors", [])
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
        """Search vectors with unified interface matching gRPC
        
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
        payload = {
            "query_vector": query_vector,
            "top_k": top_k,
            "metadata_filter": metadata_filter,
            "include_vectors": include_vectors,
            "include_metadata": include_metadata
        }
        
        start_time = time.time()
        response = self._make_request(
            "POST",
            f"/api/v1/collections/{collection_id}/search", 
            json=payload
        )
        duration_ms = (time.time() - start_time) * 1000
        
        data = response.json()
        
        # Convert REST response to unified format
        results = []
        for item in data.get("results", []):
            result_item = {
                "id": item.get("id"),
                "score": item.get("score", 0.0),
                "distance": item.get("distance", item.get("score", 0.0))
            }
            
            if include_vectors and "vector" in item:
                result_item["vector"] = item["vector"]
                
            if include_metadata and "metadata" in item:
                result_item["metadata"] = item["metadata"]
                
            results.append(result_item)
        
        return SearchResult(
            results=results,
            total_count=len(results),
            duration_ms=duration_ms,
            request_id=data.get("request_id")
        )
    
    def get_vector(
        self,
        collection_id: str,
        vector_id: str,
        include_vector: bool = True,
        include_metadata: bool = True
    ) -> Dict[str, Any]:
        """Get single vector by ID"""
        params = {
            "include_vector": include_vector,
            "include_metadata": include_metadata
        }
        
        response = self._make_request(
            "GET",
            f"/api/v1/collections/{collection_id}/vectors/{vector_id}",
            params=params
        )
        return response.json()
    
    def update_vector(
        self,
        collection_id: str,
        vector_id: str,
        vector: Optional[List[float]] = None,
        metadata: Optional[Dict[str, Any]] = None
    ) -> Dict[str, Any]:
        """Update vector data and/or metadata"""
        payload = {}
        if vector is not None:
            payload["vector"] = vector
        if metadata is not None:
            payload["metadata"] = metadata
            
        response = self._make_request(
            "PUT",
            f"/api/v1/collections/{collection_id}/vectors/{vector_id}",
            json=payload
        )
        return response.json()
    
    def delete_vector(self, collection_id: str, vector_id: str) -> Dict[str, Any]:
        """Delete single vector"""
        response = self._make_request(
            "DELETE",
            f"/api/v1/collections/{collection_id}/vectors/{vector_id}"
        )
        return response.json()


# Backward compatibility alias
ProximaDBClient = ProximaDBRestClient