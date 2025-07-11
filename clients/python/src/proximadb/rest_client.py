"""
ProximaDB Python REST Client - Updated for API v1

Implements the complete ProximaDB REST API v1 specification
using Pydantic models for type safety and validation.

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
    # Collection models
    Collection,
    CollectionConfig,
    CollectionOperationType,
    CollectionOperationRequest,
    CollectionResponse,
    CollectionStats,
    # Vector models
    VectorRecord,
    VectorOperationType,
    VectorBatchRequest,
    VectorSearchRequest,
    VectorOperationResponse,
    # Search models
    SearchQuery,
    SearchResult,
    SearchParameters,
    IncludeFields,
    SearchOptimization,
    MetadataFilter,
    # System models
    HealthStatus,
    ApiResponse,
    ApiError,
    OperationMetrics,
    # Type aliases
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
    and request/response handling using Pydantic models.
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
    
    def _parse_api_response(self, response: httpx.Response, model=None) -> Any:
        """Parse API response and optionally convert to Pydantic model"""
        data = response.json()
        
        # Handle API response wrapper
        if isinstance(data, dict) and "success" in data:
            api_response = ApiResponse(**data)
            if not api_response.success:
                error = api_response.error or ApiError(
                    code="UNKNOWN", 
                    message=api_response.message or "Operation failed"
                )
                raise ProximaDBError(f"{error.code}: {error.message}")
            
            # Return the data field, optionally converted to model
            if model and api_response.data is not None:
                return model(**api_response.data) if isinstance(api_response.data, dict) else api_response.data
            return api_response.data
        
        # Direct response without wrapper
        if model:
            return model(**data) if isinstance(data, dict) else data
        return data
    
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
    # SYSTEM OPERATIONS
    # =============================================================================
    
    def health(self) -> HealthStatus:
        """Check server health status
        
        Returns:
            HealthStatus: Server health information
        """
        # Health endpoint is at root level
        health_url = self.config.url.rstrip('/') + '/health'
        response = httpx.get(health_url, timeout=self.config.timeout)
        
        if response.status_code != 200:
            self._handle_error_response(response)
        
        data = response.json()
        if data.get("success", False) and "data" in data:
            health_data = data["data"]
            return HealthStatus(
                status=health_data.get("status", "healthy"),
                version=health_data.get("version", "0.1.0"),
                uptime_seconds=health_data.get("uptime_seconds", 0),
                services=health_data.get("services", {}),
                timestamp=health_data.get("timestamp", 0)
            )
        else:
            raise ProximaDBError(f"Health check failed: {data.get('error', 'Unknown error')}")
    
    def get_metrics(self) -> Dict[str, Any]:
        """Get system metrics
        
        Returns:
            Dict: System performance metrics
        """
        response = self._make_request("GET", "/metrics")
        return self._parse_api_response(response)
    
    # =============================================================================
    # COLLECTION MANAGEMENT
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
        """
        if config is None:
            config = CollectionConfig(name=name, **kwargs)
        else:
            # Ensure name is set in config
            config.name = name
        
        # Create request using Pydantic model
        request = CollectionOperationRequest(
            operation=CollectionOperationType.CREATE,
            config=config
        )
        
        logger.debug(f"🏗️ Creating collection: {name}")
        response = self._make_request("POST", "/collections/operation", json=request.model_dump())
        
        # Parse response
        coll_response = self._parse_api_response(response, CollectionResponse)
        if coll_response.collection:
            return coll_response.collection
        else:
            raise ProximaDBError("Collection creation response missing collection data")
    
    def get_collection(self, collection_id: str) -> Collection:
        """Get collection metadata
        
        Args:
            collection_id: Collection identifier (name or UUID)
            
        Returns:
            Collection: Collection metadata
        """
        request = CollectionOperationRequest(
            operation=CollectionOperationType.GET,
            collection_id=collection_id
        )
        
        response = self._make_request("POST", "/collections/operation", json=request.model_dump())
        
        coll_response = self._parse_api_response(response, CollectionResponse)
        if coll_response.collection:
            return coll_response.collection
        else:
            raise ProximaDBError(f"Collection not found: {collection_id}")
    
    def list_collections(self) -> List[Collection]:
        """List all collections
        
        Returns:
            List[Collection]: List of collections
        """
        request = CollectionOperationRequest(
            operation=CollectionOperationType.LIST
        )
        
        response = self._make_request("POST", "/collections/operation", json=request.model_dump())
        
        coll_response = self._parse_api_response(response, CollectionResponse)
        return coll_response.collections or []
    
    def delete_collection(self, collection_id: str) -> bool:
        """Delete a collection
        
        Args:
            collection_id: Collection identifier
            
        Returns:
            bool: True if deletion was successful
        """
        request = CollectionOperationRequest(
            operation=CollectionOperationType.DELETE,
            collection_id=collection_id
        )
        
        response = self._make_request("POST", "/collections/operation", json=request.model_dump())
        
        coll_response = self._parse_api_response(response, CollectionResponse)
        return coll_response.success
    
    def update_collection(
        self,
        collection_id: str,
        config: CollectionConfig
    ) -> Collection:
        """Update collection configuration
        
        Args:
            collection_id: Collection identifier
            config: Updated configuration
            
        Returns:
            Collection: Updated collection metadata
        """
        request = CollectionOperationRequest(
            operation=CollectionOperationType.UPDATE,
            collection_id=collection_id,
            config=config
        )
        
        response = self._make_request("POST", "/collections/operation", json=request.model_dump())
        
        coll_response = self._parse_api_response(response, CollectionResponse)
        if coll_response.collection:
            return coll_response.collection
        else:
            raise ProximaDBError("Collection update response missing collection data")
    
    # =============================================================================
    # VECTOR OPERATIONS
    # =============================================================================
    
    def insert_vectors(
        self,
        collection_id: str,
        records: List[VectorRecord]
    ) -> VectorOperationResponse:
        """Insert vectors into a collection
        
        Args:
            collection_id: Collection identifier
            records: List of vector records to insert
            
        Returns:
            VectorOperationResponse: Operation metrics and results
        """
        request = VectorBatchRequest(
            collection_id=collection_id,
            operation=VectorOperationType.INSERT,
            records=records
        )
        
        response = self._make_request("POST", "/vectors/batch", json=request.model_dump())
        
        return self._parse_api_response(response, VectorOperationResponse)
    
    def upsert_vectors(
        self,
        collection_id: str,
        records: List[VectorRecord]
    ) -> VectorOperationResponse:
        """Upsert vectors into a collection
        
        Args:
            collection_id: Collection identifier
            records: List of vector records to upsert
            
        Returns:
            VectorOperationResponse: Operation metrics and results
        """
        request = VectorBatchRequest(
            collection_id=collection_id,
            operation=VectorOperationType.UPSERT,
            records=records
        )
        
        response = self._make_request("POST", "/vectors/batch", json=request.model_dump())
        
        return self._parse_api_response(response, VectorOperationResponse)
    
    def delete_vectors(
        self,
        collection_id: str,
        vector_ids: List[str]
    ) -> VectorOperationResponse:
        """Delete vectors from a collection
        
        Args:
            collection_id: Collection identifier
            vector_ids: List of vector IDs to delete
            
        Returns:
            VectorOperationResponse: Operation metrics and results
        """
        # Create records with expires_at=0 for deletion
        records = [
            VectorRecord(id=vid, vector=[0.0], expires_at=0)
            for vid in vector_ids
        ]
        
        request = VectorBatchRequest(
            collection_id=collection_id,
            operation=VectorOperationType.DELETE,
            records=records
        )
        
        response = self._make_request("POST", "/vectors/batch", json=request.model_dump())
        
        return self._parse_api_response(response, VectorOperationResponse)
    
    def search(
        self,
        collection_id: str,
        queries: List[SearchQuery],
        search_params: Optional[SearchParameters] = None,
        include_fields: Optional[IncludeFields] = None,
        search_optimization: Optional[SearchOptimization] = None
    ) -> List[SearchResult]:
        """Search for similar vectors
        
        Args:
            collection_id: Collection identifier
            queries: List of search queries
            search_params: Search parameters
            include_fields: Fields to include in results
            search_optimization: Search optimization hints
            
        Returns:
            List[SearchResult]: Search results for each query
        """
        request = VectorSearchRequest(
            collection_id=collection_id,
            queries=queries,
            search_params=search_params,
            include_fields=include_fields,
            search_optimization=search_optimization
        )
        
        response = self._make_request("POST", "/vectors/search", json=request.model_dump())
        
        vec_response = self._parse_api_response(response, VectorOperationResponse)
        return vec_response.results or []
    
    def search_single(
        self,
        collection_id: str,
        vector: Union[List[float], np.ndarray],
        top_k: int = 10,
        metadata_filter: Optional[MetadataFilter] = None,
        **kwargs
    ) -> List[SearchResult]:
        """Search for similar vectors with a single query
        
        Args:
            collection_id: Collection identifier
            vector: Query vector
            top_k: Number of results to return
            metadata_filter: Metadata filter
            **kwargs: Additional search optimization parameters
            
        Returns:
            List[SearchResult]: Search results
        """
        # Normalize vector
        vector = self._normalize_vector(vector)
        
        # Create search query
        query = SearchQuery(
            vector=vector,
            metadata_filter=metadata_filter
        )
        
        # Create search optimization from kwargs
        search_optimization = SearchOptimization(
            top_k=top_k,
            **kwargs
        )
        
        # Perform search
        results = self.search(
            collection_id=collection_id,
            queries=[query],
            search_optimization=search_optimization
        )
        
        return results
    
    def get_vector(
        self,
        collection_id: str,
        vector_id: str
    ) -> VectorRecord:
        """Get a single vector by ID
        
        Args:
            collection_id: Collection identifier
            vector_id: Vector ID
            
        Returns:
            VectorRecord: Vector record
        """
        response = self._make_request("GET", f"/collections/{collection_id}/vectors/{vector_id}")
        
        data = self._parse_api_response(response)
        return VectorRecord(**data)
    
    # =============================================================================
    # LEGACY COMPATIBILITY METHODS
    # =============================================================================
    
    def insert_vector(
        self,
        collection_id: str,
        vector_id: Optional[str] = None,
        vector: Optional[Union[List[float], np.ndarray]] = None,
        vectors: Optional[Union[List[List[float]], np.ndarray]] = None,
        ids: Optional[List[str]] = None,
        metadata: Optional[Union[MetadataDict, List[MetadataDict]]] = None,
        **kwargs
    ) -> VectorOperationResponse:
        """Legacy insert method for backward compatibility"""
        warnings.warn(
            "insert_vector is deprecated. Use insert_vectors instead.",
            DeprecationWarning,
            stacklevel=2
        )
        
        # Convert legacy parameters to VectorRecord list
        records = []
        
        if vector is not None:
            # Single vector
            record = VectorRecord(
                id=vector_id,
                vector=self._normalize_vector(vector),
                metadata=metadata if isinstance(metadata, dict) else {}
            )
            records.append(record)
        elif vectors is not None:
            # Multiple vectors
            vectors = self._normalize_vectors(vectors)
            if ids is None:
                ids = [None] * len(vectors)
            if metadata is None:
                metadata = [{}] * len(vectors)
            elif isinstance(metadata, dict):
                metadata = [metadata] * len(vectors)
            
            for i, (vec, vid, meta) in enumerate(zip(vectors, ids, metadata)):
                record = VectorRecord(
                    id=vid,
                    vector=vec,
                    metadata=meta
                )
                records.append(record)
        
        return self.insert_vectors(collection_id, records)
    
    def search_vectors(
        self,
        collection_id: str,
        query_vector: Union[List[float], np.ndarray],
        top_k: int = 10,
        filters: Optional[FilterDict] = None,
        **kwargs
    ) -> List[SearchResult]:
        """Legacy search method for backward compatibility"""
        warnings.warn(
            "search_vectors is deprecated. Use search or search_single instead.",
            DeprecationWarning,
            stacklevel=2
        )
        
        return self.search_single(
            collection_id=collection_id,
            vector=query_vector,
            top_k=top_k,
            filters=filters,
            **kwargs
        )
    
    def close(self) -> None:
        """Close the HTTP client connection"""
        self._http_client.close()
        logger.info("REST client connection closed")
    
    def __enter__(self):
        """Context manager entry"""
        return self
    
    def __exit__(self, exc_type, exc_val, exc_tb):
        """Context manager exit"""
        self.close()