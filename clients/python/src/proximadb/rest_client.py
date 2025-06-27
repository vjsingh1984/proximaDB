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
        
        # Use the base URL directly - our REST API doesn't use /api/v1 prefix
        base_url = self.config.url.rstrip('/')
        
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
        if data.get("success", False) and "data" in data:
            health_data = data["data"]
            return HealthStatus(
                status=health_data.get("status", "unknown"),
                version=health_data.get("version", "0.1.0"),
                uptime_seconds=0,  # Not provided by simple health check
                total_operations=0,
                successful_operations=0,
                failed_operations=0,
                storage_healthy=True,
                wal_healthy=True,
                timestamp=0
            )
        else:
            raise ProximaDBError(f"Health check failed: {data.get('error', 'Unknown error')}")
    
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
            "indexing_algorithm": getattr(config, 'indexing_algorithm', 'hnsw'),
        }
        
        logger.debug(f"🏗️ Creating collection: {name}")
        response = self._make_request("POST", "/collections", json=request_data)
        
        data = response.json()
        if data.get("success", False):
            # The actual API returns just the collection name on success
            return Collection(
                id=data["data"],  # collection name is returned as data
                name=data["data"],
                dimension=config.dimension,
                metric=config.distance_metric,
                index_type=getattr(config, 'indexing_algorithm', 'hnsw'),
                created_at=0,  # Will be populated when we get the collection
                vector_count=0
            )
        else:
            raise ProximaDBError(f"Failed to create collection: {data.get('error', 'Unknown error')}")
    
    def get_collection(self, collection_id: str) -> Collection:
        """Get collection metadata
        
        Args:
            collection_id: Collection identifier
            
        Returns:
            Collection: Collection metadata
        """
        response = self._make_request("GET", f"/collections/{collection_id}")
        data = response.json()
        
        if data.get("success", False) and "data" in data:
            coll_data = data["data"]
            return Collection(
                id=coll_data["uuid"],
                name=coll_data["name"],
                dimension=coll_data["dimension"],
                metric=coll_data["distance_metric"],
                index_type=coll_data.get("indexing_algorithm", "hnsw"),
                created_at=coll_data.get("created_at", 0),
                vector_count=coll_data.get("vector_count", 0)
            )
        else:
            raise ProximaDBError(f"Failed to get collection: {data.get('error', 'Collection not found')}")
    
    def list_collections(self) -> List[str]:
        """List all collections
        
        Returns:
            List[str]: List of collection names
        """
        response = self._make_request("GET", "/collections")
        data = response.json()
        
        if data.get("success", False) and "data" in data:
            return data["data"]  # API returns list of collection names
        else:
            raise ProximaDBError(f"Failed to list collections: {data.get('error', 'Unknown error')}")
    
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
            "id": vector_id,
            "vector": vector_data,
            "metadata": metadata or {}
        }
        
        logger.debug(f"📥 Inserting vector: {vector_id} to {collection_id}")
        response = self._make_request("POST", f"/collections/{collection_id}/vectors", json=request_data)
        
        data = response.json()
        return InsertResult(
            count=1 if data.get("success", True) else 0,
            failed_count=0 if data.get("success", True) else 1,
            duration_ms=data.get("processing_time_us", 0) // 1000,
            errors=None if data.get("success", True) else [data.get("error", "Unknown error")]
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
        
        logger.debug(f"📥 Batch inserting {len(vector_data)} vectors to {collection_id}")
        response = self._make_request("POST", f"/collections/{collection_id}/vectors/batch", json=vector_data)
        
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
            "vector": query_vector,
            "k": k,
            "include_vectors": include_vectors,
            "include_metadata": include_metadata,
        }
        
        if filter:
            request_data["filters"] = filter
        
        logger.debug(f"🔍 Searching in {collection_id}, top_k={k}")
        response = self._make_request(
            "POST", 
            f"/collections/{collection_id}/search", 
            json=request_data,
            timeout=timeout or self.config.timeout
        )
        
        data = response.json()
        results = []
        
        if data.get("success", False) and "data" in data:
            for result in data["data"]:
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
                f"/collections/{collection_id}/vectors/{vector_id}",
                params=params
            )
            data = response.json()
            if data.get("success", False):
                return data.get("data")
            return None
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
            f"/collections/{collection_id}/vectors/{vector_id}"
        )
        
        data = response.json()
        return DeleteResult(
            deleted_count=1 if data.get("success", True) else 0,
            count=1 if data.get("success", True) else 0,
            errors=None if data.get("success", True) else [data.get("error", "Unknown error")]
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
            deleted_count=total_deleted,
            count=total_deleted,
            errors=errors if errors else None
        )
    
    def update_collection(
        self,
        collection_id: str,
        updates: Dict[str, Any]
    ) -> Collection:
        """Update collection metadata and configuration
        
        Args:
            collection_id: Collection identifier
            updates: Dictionary of fields to update
            
        Returns:
            Collection: Updated collection metadata
        """
        url = f"{self.config.url}/collections/{collection_id}"
        
        try:
            response = self._http_client.patch(url, json=updates, timeout=self.config.timeout)
            self._handle_error_response(response)
            data = response.json()
            return Collection(**data)
        except httpx.RequestError as e:
            logger.error(f"Network error updating collection {collection_id}: {e}")
            raise NetworkError(f"Failed to update collection: {e}")
    
    def delete_vectors_by_filter(
        self,
        collection_id: str,
        filter: FilterDict
    ) -> DeleteResult:
        """Delete vectors matching filter criteria
        
        Args:
            collection_id: Collection identifier
            filter: Filter criteria for vector deletion
            
        Returns:
            DeleteResult: Deletion operation result
        """
        url = f"{self.config.url}/collections/{collection_id}/vectors"
        
        try:
            response = self._http_client.delete(
                url,
                json={"filter": filter},
                timeout=self.config.timeout
            )
            self._handle_error_response(response)
            data = response.json()
            return DeleteResult(**data)
        except httpx.RequestError as e:
            logger.error(f"Network error deleting vectors by filter: {e}")
            raise NetworkError(f"Failed to delete vectors: {e}")
    
    def get_vector_history(
        self,
        collection_id: str,
        vector_id: str,
        limit: int = 10
    ) -> List[Dict[str, Any]]:
        """Get vector version history
        
        Args:
            collection_id: Collection identifier
            vector_id: Vector identifier
            limit: Maximum number of history entries
            
        Returns:
            List[Dict[str, Any]]: Vector version history
        """
        url = f"{self.config.url}/collections/{collection_id}/vectors/{vector_id}/history"
        params = {"limit": limit}
        
        try:
            response = self._http_client.get(url, params=params, timeout=self.config.timeout)
            self._handle_error_response(response)
            data = response.json()
            return data.get("history", [])
        except httpx.RequestError as e:
            logger.error(f"Network error getting vector history: {e}")
            raise NetworkError(f"Failed to get vector history: {e}")
    
    def multi_search(
        self,
        collection_id: str,
        queries: List[Union[List[float], np.ndarray]],
        k: int = 10,
        filter: Optional[FilterDict] = None,
        include_vectors: bool = False,
        include_metadata: bool = True
    ) -> List[SearchResult]:
        """Search with multiple query vectors
        
        Args:
            collection_id: Collection identifier
            queries: List of query vectors
            k: Number of results per query
            filter: Optional metadata filter
            include_vectors: Include vector data in results
            include_metadata: Include metadata in results
            
        Returns:
            List[SearchResult]: Combined search results from all queries
        """
        url = f"{self.config.url}/collections/{collection_id}/multi_search"
        
        # Normalize query vectors
        normalized_queries = [self._normalize_vector(q) for q in queries]
        
        payload = {
            "queries": normalized_queries,
            "k": k,
            "include_vectors": include_vectors,
            "include_metadata": include_metadata
        }
        
        if filter:
            payload["filter"] = filter
        
        try:
            response = self._http_client.post(url, json=payload, timeout=self.config.timeout)
            self._handle_error_response(response)
            data = response.json()
            
            results = []
            for result_data in data.get("results", []):
                results.append(SearchResult(**result_data))
            
            return results
        except httpx.RequestError as e:
            logger.error(f"Network error in multi-search: {e}")
            raise NetworkError(f"Multi-search failed: {e}")
    
    def search_with_aggregations(
        self,
        collection_id: str,
        query: Union[List[float], np.ndarray],
        k: int = 10,
        aggregations: List[str],
        group_by: Optional[str] = None,
        filter: Optional[FilterDict] = None
    ) -> Dict[str, Any]:
        """Search with result aggregations
        
        Args:
            collection_id: Collection identifier
            query: Query vector
            k: Number of results
            aggregations: List of fields to aggregate
            group_by: Field to group results by
            filter: Optional metadata filter
            
        Returns:
            Dict[str, Any]: Search results with aggregations
        """
        url = f"{self.config.url}/collections/{collection_id}/search_aggregated"
        
        # Normalize query vector
        normalized_query = self._normalize_vector(query)
        
        payload = {
            "query": normalized_query,
            "k": k,
            "aggregations": aggregations
        }
        
        if group_by:
            payload["group_by"] = group_by
        if filter:
            payload["filter"] = filter
        
        try:
            response = self._http_client.post(url, json=payload, timeout=self.config.timeout)
            self._handle_error_response(response)
            return response.json()
        except httpx.RequestError as e:
            logger.error(f"Network error in aggregated search: {e}")
            raise NetworkError(f"Aggregated search failed: {e}")
    
    def atomic_insert_vectors(
        self,
        collection_id: str,
        vectors: VectorArray,
        ids: List[str],
        metadata: Optional[List[MetadataDict]] = None
    ) -> BatchResult:
        """Insert vectors atomically (all-or-nothing)
        
        Args:
            collection_id: Collection identifier
            vectors: Vector data to insert
            ids: Vector identifiers
            metadata: Optional metadata for each vector
            
        Returns:
            BatchResult: Atomic insertion result
        """
        url = f"{self.config.url}/collections/{collection_id}/vectors/atomic"
        
        # Normalize vectors
        normalized_vectors = self._normalize_vectors(vectors)
        
        payload = {
            "vectors": normalized_vectors,
            "ids": ids,
            "atomic": True
        }
        
        if metadata:
            payload["metadata"] = metadata
        
        try:
            response = self._http_client.post(url, json=payload, timeout=self.config.timeout)
            self._handle_error_response(response)
            data = response.json()
            return BatchResult(**data)
        except httpx.RequestError as e:
            logger.error(f"Network error in atomic insert: {e}")
            raise NetworkError(f"Atomic insert failed: {e}")
    
    def begin_transaction(self) -> str:
        """Begin a new transaction and return transaction ID
        
        Returns:
            str: Transaction identifier
        """
        url = f"{self.config.url}/transactions"
        
        try:
            response = self._http_client.post(url, json={}, timeout=self.config.timeout)
            self._handle_error_response(response)
            data = response.json()
            return data["transaction_id"]
        except httpx.RequestError as e:
            logger.error(f"Network error beginning transaction: {e}")
            raise NetworkError(f"Failed to begin transaction: {e}")
    
    def commit_transaction(self, transaction_id: str) -> bool:
        """Commit a transaction
        
        Args:
            transaction_id: Transaction identifier
            
        Returns:
            bool: True if committed successfully
        """
        url = f"{self.config.url}/transactions/{transaction_id}/commit"
        
        try:
            response = self._http_client.post(url, json={}, timeout=self.config.timeout)
            self._handle_error_response(response)
            data = response.json()
            return data.get("success", False)
        except httpx.RequestError as e:
            logger.error(f"Network error committing transaction: {e}")
            raise NetworkError(f"Failed to commit transaction: {e}")
    
    def rollback_transaction(self, transaction_id: str) -> bool:
        """Rollback a transaction
        
        Args:
            transaction_id: Transaction identifier
            
        Returns:
            bool: True if rolled back successfully
        """
        url = f"{self.config.url}/transactions/{transaction_id}/rollback"
        
        try:
            response = self._http_client.post(url, json={}, timeout=self.config.timeout)
            self._handle_error_response(response)
            data = response.json()
            return data.get("success", False)
        except httpx.RequestError as e:
            logger.error(f"Network error rolling back transaction: {e}")
            raise NetworkError(f"Failed to rollback transaction: {e}")
    
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