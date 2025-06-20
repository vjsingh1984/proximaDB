"""
ProximaDB Python REST Client - Updated for API v1

Implements the complete ProximaDB REST API v1 specification
as documented in REST_API_REFERENCE.adoc

Copyright 2025 ProximaDB
"""

import logging
from typing import Any, Dict, List, Optional, Union
import warnings

import numpy as np
import httpx
from tenacity import retry, stop_after_attempt, wait_exponential, retry_if_exception_type

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
from .exceptions import (
    ProximaDBError,
    NetworkError,
    TimeoutError,
    RateLimitError,
    map_http_error,
)


logger = logging.getLogger(__name__)


class ProximaDBRestClient:
    """
    ProximaDB REST API v1 Client
    
    Implements the complete REST API specification with proper endpoint mapping
    and request/response handling according to the documented API contract.
    """
    
    def __init__(
        self,
        url: Optional[str] = None,
        api_key: Optional[str] = None,
        config: Optional[ClientConfig] = None,
        **kwargs
    ) -> None:
        """Initialize ProximaDB REST client
        
        Args:
            url: ProximaDB server URL (e.g., http://localhost:5678)
            api_key: API key for authentication
            config: Client configuration object
            **kwargs: Additional configuration parameters
        """
        if config is None:
            config = load_config(url=url, api_key=api_key, **kwargs)
        
        self.config = config
        self._setup_logging()
        
        # Initialize HTTP client with API v1 base path
        self._http_client = self._create_http_client()
        
        logger.info(f"🌐 Initialized ProximaDB REST client for {self.config.url}")
    
    def _setup_logging(self) -> None:
        """Setup logging configuration"""
        if self.config.enable_debug_logging:
            level = logging.DEBUG
        else:
            level = getattr(logging, self.config.log_level.value)
        
        logging.getLogger("proximadb").setLevel(level)
    
    def _create_http_client(self) -> httpx.Client:
        """Create configured HTTP client with proper base URL"""
        timeout = httpx.Timeout(
            connect=self.config.connection.connect_timeout,
            read=self.config.connection.read_timeout,
            write=self.config.timeout,
            pool=self.config.connection.total_timeout,
        )
        
        limits = httpx.Limits(
            max_keepalive_connections=self.config.connection.pool_size,
            max_connections=self.config.connection.pool_maxsize,
            keepalive_expiry=self.config.connection.keepalive_timeout,
        )
        
        # Ensure base URL includes /api/v1 for REST API
        base_url = self.config.url.rstrip('/')
        if not base_url.endswith('/api/v1'):
            base_url += '/api/v1'
        
        return httpx.Client(
            base_url=base_url,
            headers=self.config.get_base_headers(),
            timeout=timeout,
            limits=limits,
            verify=self.config.tls.verify,
            cert=(self.config.tls.cert_file, self.config.tls.key_file) if self.config.tls.cert_file else None,
            http2=self.config.enable_http2,
        )
    
    def _make_request(self, method: str, endpoint: str, **kwargs) -> httpx.Response:
        """Make HTTP request with retry logic"""
        @retry(
            stop=stop_after_attempt(self.config.retry.max_retries + 1),
            wait=wait_exponential(
                multiplier=self.config.retry.backoff_factor,
                max=self.config.retry.max_backoff,
            ),
            retry=retry_if_exception_type((NetworkError, TimeoutError, RateLimitError)),
            reraise=True,
        )
        def _request():
            try:
                logger.debug(f"🌐 REST {method} {endpoint}")
                response = self._http_client.request(method, endpoint, **kwargs)
                
                if response.status_code >= 400:
                    self._handle_error_response(response)
                
                return response
                
            except httpx.TimeoutException as e:
                raise TimeoutError(f"Request timeout: {e}", timeout_seconds=self.config.timeout)
            except httpx.NetworkError as e:
                raise NetworkError(f"Network error: {e}", original_error=e)
            except httpx.HTTPStatusError as e:
                self._handle_error_response(e.response)
        
        return _request()
    
    def _handle_error_response(self, response: httpx.Response) -> None:
        """Handle error responses from server"""
        try:
            error_data = response.json()
        except Exception:
            error_data = {"message": response.text or f"HTTP {response.status_code} error"}
        
        raise map_http_error(response.status_code, error_data)
    
    def _normalize_vector(self, vector: Union[List[float], np.ndarray]) -> List[float]:
        """Normalize single vector to list format"""
        if isinstance(vector, np.ndarray):
            if vector.dtype != np.float32:
                vector = vector.astype(np.float32)
            return vector.tolist()
        return vector
    
    def _normalize_vectors(self, vectors: VectorArray) -> List[List[float]]:
        """Normalize vectors to list of lists format"""
        if isinstance(vectors, np.ndarray):
            if vectors.dtype != np.float32:
                vectors = vectors.astype(np.float32)
            return vectors.tolist()
        return vectors
    
    # =============================================================================
    # SYSTEM OPERATIONS (as per API specification)
    # =============================================================================
    
    def health(self) -> HealthStatus:
        """Check server health status
        
        Returns:
            HealthStatus: Server health information
            
        Example:
            >>> client = ProximaDBRestClient()
            >>> health = client.health()
            >>> print(f"Status: {health.status}")
        """
        # Health endpoint is at root level, not under /api/v1
        health_url = self.config.url.rstrip('/') + '/health'
        response = httpx.get(health_url, timeout=self.config.timeout)
        
        if response.status_code != 200:
            self._handle_error_response(response)
        
        data = response.json()
        return HealthStatus(
            status=data.get("status", "unknown"),
            version=data.get("version", "0.1.0"),
            uptime_seconds=0,  # Not provided by simple health check
            total_operations=0,
            successful_operations=0,
            failed_operations=0,
            storage_healthy=True,
            wal_healthy=True,
            timestamp=0
        )
    
    def get_metrics(self) -> Dict[str, Any]:
        """Get system metrics
        
        Returns:
            Dict: System performance metrics
        """
        response = self._make_request("GET", "/metrics")
        return response.json()
    
    # =============================================================================
    # COLLECTION MANAGEMENT (as per API specification)
    # =============================================================================
    
    def create_collection(
        self,
        name: str,
        config: Optional[CollectionConfig] = None,
        **kwargs
    ) -> Collection:
        """Create a new vector collection
        
        Args:
            name: Collection name
            config: Collection configuration
            **kwargs: Additional configuration parameters
            
        Returns:
            Collection: Created collection metadata
            
        Example:
            >>> client = ProximaDBRestClient()
            >>> config = CollectionConfig(
            ...     dimension=768,
            ...     distance_metric="cosine",
            ...     storage_layout="viper"
            ... )
            >>> collection = client.create_collection("documents", config)
        """
        if config is None:
            config = CollectionConfig(**kwargs)
        
        request_data = {
            "name": name,
            "dimension": config.dimension,
            "distance_metric": config.distance_metric,
            "storage_layout": getattr(config, 'storage_layout', 'viper'),
        }
        
        if config.description:
            request_data["config"] = {"description": config.description}
        
        logger.debug(f"🏗️ Creating collection: {name}")
        response = self._make_request("POST", "/collections", json=request_data)
        
        data = response.json()
        return Collection(
            id=data["id"],
            name=data["name"],
            dimension=data["dimension"],
            distance_metric=data["distance_metric"],
            storage_layout=data["storage_layout"],
            created_at=data["created_at"],
            vector_count=data["vector_count"],
            filterable_metadata_fields=[]
        )
    
    def get_collection(self, collection_id: str) -> Collection:
        """Get collection metadata
        
        Args:
            collection_id: Collection identifier
            
        Returns:
            Collection: Collection metadata
        """
        response = self._make_request("GET", f"/collections/{collection_id}")
        data = response.json()
        
        return Collection(
            id=data["id"],
            name=data["name"],
            dimension=data["dimension"],
            distance_metric=data["distance_metric"],
            storage_layout=data["storage_layout"],
            created_at=data["created_at"],
            vector_count=data["vector_count"],
            filterable_metadata_fields=[]
        )
    
    def list_collections(self) -> List[Collection]:
        """List all collections
        
        Returns:
            List[Collection]: List of all collections
        """
        response = self._make_request("GET", "/collections")
        data = response.json()
        
        collections = []
        if "collections" in data:
            for item in data["collections"]:
                collections.append(Collection(
                    id=item["id"],
                    name=item["name"],
                    dimension=item["dimension"],
                    distance_metric=item["distance_metric"],
                    storage_layout=item["storage_layout"],
                    created_at=item["created_at"],
                    vector_count=item["vector_count"],
                    filterable_metadata_fields=[]
                ))
        
        return collections
    
    def delete_collection(self, collection_id: str) -> bool:
        """Delete a collection
        
        Args:
            collection_id: Collection identifier
            
        Returns:
            bool: True if deletion was successful
        """
        response = self._make_request("DELETE", f"/collections/{collection_id}")
        data = response.json()
        return data.get("success", False)
    
    # =============================================================================
    # VECTOR OPERATIONS (as per API specification)
    # =============================================================================
    
    def insert_vector(
        self,
        collection_id: str,
        vector_id: str,
        vector: Union[List[float], np.ndarray],
        metadata: Optional[MetadataDict] = None,
        upsert: bool = False,
    ) -> InsertResult:
        """Insert a single vector
        
        Args:
            collection_id: Target collection ID
            vector_id: Unique vector identifier
            vector: Vector data
            metadata: Optional metadata dictionary
            upsert: Update if vector already exists
            
        Returns:
            InsertResult: Insert operation result
            
        Example:
            >>> client = ProximaDBRestClient()
            >>> result = client.insert_vector(
            ...     "col_123",
            ...     "doc_001", 
            ...     [0.1, 0.2, 0.3, 0.4],
            ...     {"category": "research", "priority": 8}
            ... )
        """
        vector_data = self._normalize_vector(vector)
        
        request_data = {
            "collection_id": collection_id,
            "id": vector_id,
            "vector": vector_data,
            "metadata": metadata or {}
        }
        
        logger.debug(f"📥 Inserting vector: {vector_id} to {collection_id}")
        response = self._make_request("POST", "/vectors", json=request_data)
        
        data = response.json()
        return InsertResult(
            success=data.get("success", True),
            count=data.get("affected_count", 1),
            failed_count=0,
            duration_ms=data.get("processing_time_us", 0) // 1000,
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
        """Insert multiple vectors
        
        Args:
            collection_id: Target collection ID
            vectors: Vector data array
            ids: List of unique vector identifiers
            metadata: Optional list of metadata dictionaries
            upsert: Update if vectors already exist
            batch_size: Override default batch size
            
        Returns:
            BatchResult: Batch insert operation result
        """
        vectors_list = self._normalize_vectors(vectors)
        
        if len(vectors_list) != len(ids):
            raise ValueError("Number of vectors must match number of IDs")
        
        if metadata and len(metadata) != len(vectors_list):
            raise ValueError("Number of metadata items must match number of vectors")
        
        # Prepare vector data
        vector_data = []
        for i, (vector_id, vector) in enumerate(zip(ids, vectors_list)):
            item = {
                "id": vector_id,
                "vector": vector,
                "metadata": metadata[i] if metadata else {}
            }
            vector_data.append(item)
        
        request_data = {
            "collection_id": collection_id,
            "vectors": vector_data,
            "upsert_mode": upsert
        }
        
        logger.debug(f"📥 Batch inserting {len(vector_data)} vectors to {collection_id}")
        response = self._make_request("POST", "/vectors/batch", json=request_data)
        
        data = response.json()
        return BatchResult(
            total_count=len(vector_data),
            successful_count=data.get("affected_count", 0),
            failed_count=len(vector_data) - data.get("affected_count", 0),
            duration_ms=data.get("processing_time_us", 0) // 1000,
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
        """Search for similar vectors
        
        Args:
            collection_id: Target collection ID
            query: Query vector
            k: Number of results to return
            filter: Metadata filter conditions
            include_vectors: Include vector data in results
            include_metadata: Include metadata in results
            ef: HNSW search parameter (higher = more accurate, slower)
            exact: Use exact search instead of approximate
            timeout: Request timeout override
            
        Returns:
            List[SearchResult]: List of search results ordered by similarity
            
        Example:
            >>> client = ProximaDBRestClient()
            >>> results = client.search(
            ...     "col_123",
            ...     [0.1, 0.2, 0.3, 0.4],
            ...     k=5,
            ...     filter={"category": "research"}
            ... )
        """
        query_vector = self._normalize_vector(query)
        
        request_data = {
            "collection_id": collection_id,
            "query_vector": query_vector,
            "top_k": k,
            "include_vector": include_vectors,
            "include_metadata": include_metadata,
            "exact_search": exact
        }
        
        if filter:
            request_data["metadata_filter"] = filter
        
        if ef is not None:
            request_data["ef_search"] = ef
        
        logger.debug(f"🔍 Searching in {collection_id}, top_k={k}")
        response = self._make_request(
            "POST", 
            "/vectors/search", 
            json=request_data,
            timeout=timeout or self.config.timeout
        )
        
        data = response.json()
        results = []
        
        for result in data.get("results", []):
            results.append(SearchResult(
                id=result["id"],
                score=result["score"],
                vector=result.get("vector"),
                metadata=result.get("metadata")
            ))
        
        return results
    
    def get_vector(
        self,
        collection_id: str,
        vector_id: str,
        include_vector: bool = True,
        include_metadata: bool = True,
    ) -> Optional[Dict[str, Any]]:
        """Get a single vector by ID
        
        Args:
            collection_id: Collection identifier
            vector_id: Vector identifier
            include_vector: Include vector data in response
            include_metadata: Include metadata in response
            
        Returns:
            Optional[Dict]: Vector data or None if not found
        """
        params = {
            "include_vector": include_vector,
            "include_metadata": include_metadata,
        }
        
        try:
            response = self._make_request(
                "GET",
                f"/vectors/{collection_id}/{vector_id}",
                params=params
            )
            return response.json()
        except Exception:
            return None
    
    def delete_vector(self, collection_id: str, vector_id: str) -> DeleteResult:
        """Delete a single vector
        
        Args:
            collection_id: Collection identifier
            vector_id: Vector identifier
            
        Returns:
            DeleteResult: Deletion operation result
        """
        response = self._make_request(
            "DELETE",
            f"/vectors/{collection_id}/{vector_id}"
        )
        
        data = response.json()
        return DeleteResult(
            success=data.get("success", True),
            count=data.get("affected_count", 1),
            errors=None
        )
    
    def delete_vectors(self, collection_id: str, vector_ids: List[str]) -> DeleteResult:
        """Delete multiple vectors
        
        Args:
            collection_id: Collection identifier
            vector_ids: List of vector identifiers
            
        Returns:
            DeleteResult: Deletion operation result
        """
        total_deleted = 0
        errors = []
        
        # Delete vectors one by one (REST API doesn't have batch delete)
        for vector_id in vector_ids:
            try:
                result = self.delete_vector(collection_id, vector_id)
                if result.success:
                    total_deleted += result.count
            except Exception as e:
                errors.append(f"Failed to delete {vector_id}: {e}")
        
        return DeleteResult(
            success=total_deleted > 0,
            count=total_deleted,
            errors=errors if errors else None
        )
    
    def close(self) -> None:
        """Close the client and cleanup resources"""
        if hasattr(self, '_http_client'):
            self._http_client.close()
    
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
            pass  # Ignore errors during cleanup


# Alias for backward compatibility
ProximaDBClient = ProximaDBRestClient


# Convenience functions
def connect_rest(
    url: Optional[str] = None,
    api_key: Optional[str] = None,
    **kwargs
) -> ProximaDBRestClient:
    """Create a ProximaDB REST client with simplified parameters"""
    return ProximaDBRestClient(url=url, api_key=api_key, **kwargs)