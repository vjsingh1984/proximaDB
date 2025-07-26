"""
ProximaDB Python Client - Synchronous Client

Copyright 2025 ProximaDB

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
"""

import logging
import time
from typing import Any, Dict, List, Optional, Union, Iterator
import warnings

import numpy as np
import httpx
from tenacity import retry, stop_after_attempt, wait_exponential, retry_if_exception_type

from ..config import ClientConfig, load_config
from ..models import (
    Collection,
    CollectionConfig,
    SearchResult,
    BatchResult,
    DeleteResult,
    CollectionStats,
    HealthStatus,
    VectorArray,
    MetadataDict,
    FilterDict,
)
from ..exceptions import (
    ProximaDBError,
    NetworkError,
    TimeoutError,
    RateLimitError,
    map_http_error,
)


logger = logging.getLogger(__name__)


class ProximaDBClient:
    """Synchronous ProximaDB client"""
    
    def __init__(
        self,
        url: Optional[str] = None,
        api_key: Optional[str] = None,
        config: Optional[ClientConfig] = None,
        **kwargs
    ) -> None:
        """Initialize ProximaDB client
        
        Args:
            url: ProximaDB server URL
            api_key: API key for authentication
            config: Client configuration object
            **kwargs: Additional configuration parameters
        """
        if config is None:
            config = load_config(url=url, api_key=api_key, **kwargs)
        
        self.config = config
        self._setup_logging()
        
        # Initialize HTTP client
        self._http_client = self._create_http_client()
        
        logger.info(f"Initialized ProximaDB client for {self.config.url}")
    
    def _setup_logging(self) -> None:
        """Setup logging configuration"""
        if self.config.enable_debug_logging:
            level = logging.DEBUG
        else:
            level = getattr(logging, self.config.log_level.value)
        
        logging.getLogger("proximadb").setLevel(level)
    
    def _create_http_client(self) -> httpx.Client:
        """Create configured HTTP client"""
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
        
        return httpx.Client(
            base_url=self.config.url,
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
    
    def _http_post(self, endpoint: str, data: Any) -> Dict[str, Any]:
        """Helper method for POST requests"""
        response = self._make_request("POST", endpoint, json=data)
        return response.json()
    
    def _normalize_vectors(self, vectors: VectorArray) -> List[List[float]]:
        """Normalize vectors to list of lists format"""
        if isinstance(vectors, np.ndarray):
            if vectors.dtype != np.float32:
                vectors = vectors.astype(np.float32)
            return vectors.tolist()
        return vectors
    
    def _validate_vector_dimensions(self, vectors: VectorArray, expected_dim: Optional[int] = None) -> None:
        """Validate vector dimensions"""
        if not self.config.validate_inputs:
            return
        
        if isinstance(vectors, np.ndarray):
            if vectors.ndim != 2:
                raise ValueError("Vector array must be 2-dimensional")
            actual_dim = vectors.shape[1]
        elif isinstance(vectors, list) and vectors:
            actual_dim = len(vectors[0]) if vectors[0] else 0
        else:
            return
        
        if expected_dim is not None and actual_dim != expected_dim:
            from ..exceptions import VectorDimensionError
            raise VectorDimensionError(expected_dim, actual_dim)
    
    def health(self) -> HealthStatus:
        """Check server health status"""
        response = self._make_request("GET", "/health")
        data = response.json()
        
        # Handle nested response structure
        if 'data' in data and isinstance(data['data'], dict):
            health_data = data['data']
            return HealthStatus(
                status=health_data.get('status', 'unknown'),
                version=health_data.get('version', '0.0.0'),
                uptime_seconds=health_data.get('uptime_seconds', 0),
                services=health_data.get('services', {}),
                timestamp=int(time.time() * 1000000)  # Current timestamp in microseconds
            )
        else:
            return HealthStatus(**data)
    
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
            Created collection metadata
            
        Example:
            >>> client = ProximaDBClient()
            >>> config = CollectionConfig(
            ...     dimension=128,
            ...     distance_metric="cosine",
            ...     filterable_metadata_fields=["category", "price", "brand"]
            ... )
            >>> collection = client.create_collection("products", config)
        """
        if config is None:
            config = CollectionConfig(**kwargs)
        
        # Validate filterable metadata fields limit in client
        if config.filterable_metadata_fields and len(config.filterable_metadata_fields) > 16:
            warnings.warn(
                f"Collection '{name}' specifies {len(config.filterable_metadata_fields)} filterable metadata fields. "
                f"Only the first 16 will be used for Parquet optimization. Additional metadata can still be "
                f"inserted via vector operations (stored in extra_meta).",
                UserWarning
            )
        
        # Build config object as expected by server
        config_data = {
            "name": name,
            "dimension": config.dimension,
            "distance_metric": config.distance_metric,
            "primary_indexing_algorithm": config.primary_indexing_algorithm if hasattr(config, 'primary_indexing_algorithm') else "hnsw",
            "storage_engine": getattr(config, 'storage_engine', 'viper'),  # Default to VIPER storage
        }
        
        request_data = {
            "operation": "create",
            "config": config_data
        }
        
        # Debug print
        logger.debug(f"Collection create request: {request_data}")
        
        # Add VIPER-specific optimization fields
        if config.filterable_metadata_fields:
            config_data["filterable_columns"] = [
                {"name": field, "data_type": "string", "indexed": True} 
                for field in config.filterable_metadata_fields
            ]
        
        # Add WAL flush configuration
        if hasattr(config, 'flush_config') and config.flush_config:
            if hasattr(config.flush_config, 'max_wal_size_mb'):
                config_data["max_wal_size_mb"] = config.flush_config.max_wal_size_mb
        
        if config.description:
            config_data["description"] = config.description
        
        response = self._make_request("POST", "/api/v1/collection", json=request_data)
        response_data = response.json()
        
        # Handle unified API response format
        if "collection" in response_data:
            # Response contains collection object
            coll_data = response_data["collection"]
            if coll_data is None:
                # Check if we have an error response
                if "error" in response_data:
                    raise ProximaDBError(f"Collection creation failed: {response_data['error']}")
                else:
                    raise ProximaDBError(f"Unexpected response format: {response_data}")
            return Collection(
                id=coll_data.get("id", name),
                config=CollectionConfig(
                    name=coll_data["config"]["name"],
                    dimension=coll_data["config"]["dimension"],
                    distance_metric=coll_data["config"].get("distance_metric", "cosine"),
                    storage_engine=coll_data["config"].get("storage_engine", "viper"),
                    primary_indexing_algorithm=coll_data["config"].get("primary_indexing_algorithm", "hnsw")
                ),
                created_at=coll_data.get("created_at"),
                updated_at=coll_data.get("updated_at")
            )
        else:
            # Fallback for other response formats
            return Collection(**response_data)
    
    def get_collection(self, collection_id: str) -> Collection:
        """Get collection metadata"""
        # Use the updated GET endpoint
        logger.debug(f"Collection get request to GET /api/v1/collection/{collection_id}")
        response = self._make_request("GET", f"/api/v1/collection/{collection_id}")
        response_data = response.json()
        
        # Server returns simplified format:
        # {
        #   "id": "uuid",
        #   "name": "collection_name",
        #   "dimension": 768,
        #   "metric": "cosine",
        #   "created_at": 1737567890123,
        #   "updated_at": 1737567890123,
        #   "vector_count": 1000,
        #   "indexed": true
        # }
        
        # Create CollectionConfig from response
        config = CollectionConfig(
            name=response_data["name"],
            dimension=response_data["dimension"],
            distance_metric=response_data.get("metric", "cosine"),
            storage_engine=response_data.get("storage_engine", "viper"),
            primary_indexing_algorithm=response_data.get("indexing_algorithm", "hnsw")
        )
        
        # Create CollectionStats if available
        stats = CollectionStats(
            vector_count=response_data.get("vector_count", 0),
            index_size_bytes=response_data.get("index_size_bytes", 0),
            data_size_bytes=response_data.get("data_size_bytes", 0)
        )
        
        return Collection(
            id=response_data["id"],
            config=config,
            stats=stats,
            created_at=response_data.get("created_at"),
            updated_at=response_data.get("updated_at")
        )
    
    def list_collections(self) -> List[Collection]:
        """List all collections"""
        # Use the updated GET endpoint
        logger.debug(f"Collection list request to GET /api/v1/collections")
        response = self._make_request("GET", "/api/v1/collections")
        response_data = response.json()
        
        # Server returns:
        # {
        #   "collections": [
        #     {
        #       "id": "uuid",
        #       "name": "collection_name",
        #       "dimension": 768,
        #       "metric": "cosine",
        #       "created_at": 1737567890123,
        #       "updated_at": 1737567890123,
        #       "vector_count": 1000,
        #       "indexed": true
        #     },
        #     ...
        #   ],
        #   "total_count": 2
        # }
        
        collections = []
        collections_data = response_data.get("collections", [])
        
        for coll_data in collections_data:
            # Create CollectionConfig from response
            config = CollectionConfig(
                name=coll_data["name"],
                dimension=coll_data["dimension"],
                distance_metric=coll_data.get("metric", "cosine"),
                storage_engine=coll_data.get("storage_engine", "viper"),
                primary_indexing_algorithm=coll_data.get("indexing_algorithm", "hnsw")
            )
            
            # Create CollectionStats if available
            stats = CollectionStats(
                vector_count=coll_data.get("vector_count", 0),
                index_size_bytes=coll_data.get("index_size_bytes", 0),
                data_size_bytes=coll_data.get("data_size_bytes", 0)
            )
            
            collections.append(Collection(
                id=coll_data["id"],
                config=config,
                stats=stats,
                created_at=coll_data.get("created_at"),
                updated_at=coll_data.get("updated_at")
            ))
        
        return collections
    
    def delete_collection(self, collection_id: str) -> bool:
        """Delete a collection"""
        # Use standard REST DELETE endpoint
        logger.debug(f"Collection delete request to DELETE /api/v1/collection/{collection_id}")
        response = self._make_request("DELETE", f"/api/v1/collection/{collection_id}")
        return response.json().get("success", False)
    
    def get_collection_stats(self, collection_id: str) -> CollectionStats:
        """Get collection statistics"""
        response = self._make_request("GET", f"/collections/{collection_id}/stats")
        return CollectionStats(**response.json())
    
    def insert_vector(
        self,
        collection_id: str,
        vector_id: str,
        vector: Union[List[float], np.ndarray],
        metadata: Optional[MetadataDict] = None,
        upsert: bool = False,
    ) -> BatchResult:
        """Insert a single vector
        
        Args:
            collection_id: Target collection ID
            vector_id: Unique vector identifier
            vector: Vector data
            metadata: Optional metadata dictionary
            upsert: Update if vector already exists
            
        Returns:
            Insert operation result
        """
        import time
        
        # Normalize vector format
        if isinstance(vector, np.ndarray):
            if vector.dtype != np.float32:
                vector = vector.astype(np.float32)
            vector = vector.tolist()
        
        # Convert metadata to server format
        metadata_items = []
        if metadata:
            metadata_items = [{"key": k, "value": str(v)} for k, v in metadata.items()]
        
        # Use batch API with single vector
        vector_record = {
            "id": vector_id,
            "collection_id": collection_id,
            "vector": vector,
            "metadata": metadata_items,
            "timestamp": int(time.time() * 1000),
            "created_at": int(time.time() * 1000),
            "updated_at": int(time.time() * 1000),
            "version": 1
        }
        
        request_data = {
            "collection_id": collection_id,
            "vectors": [vector_record]
        }
        
        response = self._make_request("POST", "/api/v1/vector/batch", json=request_data)
        
        # Convert response to BatchResult
        resp_data = response.json()
        return BatchResult(
            total=resp_data.get('total', 1),
            success=resp_data.get('success', 1),
            failed=resp_data.get('failed', 0),
            errors=resp_data.get('errors', []),
            duration_ms=resp_data.get('duration_ms', 0.0)
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
            Batch insert operation result
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
        
        # Use batching for large datasets
        effective_batch_size = batch_size or self.config.default_batch_size
        
        if len(vector_data) <= effective_batch_size:
            # Single batch - use unified API
            unified_request = {
                "operation": "upsert" if upsert else "insert",
                "collection_id": collection_id,
                "vectors": vector_data
            }
            response = self._make_request(
                "POST",
                "/api/v1/vector/batch",
                json=unified_request
            )
            
            response_data = response.json()
            # Handle unified API response
            if "metrics" in response_data:
                metrics = response_data["metrics"]
                return BatchResult(
                    total=metrics.get("total_processed", len(vector_data)),
                    success=metrics.get("successful_count", len(vector_data)),
                    failed=metrics.get("failed_count", 0),
                    errors=[],
                    duration_ms=metrics.get("processing_time_us", 0) / 1000.0
                )
            else:
                return BatchResult(
                    total=len(vector_data),
                    success=len(vector_data) if response_data.get("success") else 0,
                    failed=0 if response_data.get("success") else len(vector_data),
                    errors=[],
                    duration_ms=0.0
                )
        
        else:
            # Multiple batches
            total_successful = 0
            total_failed = 0
            all_errors = []
            
            for i in range(0, len(vector_data), effective_batch_size):
                batch_data = vector_data[i:i + effective_batch_size]
                
                try:
                    # Send batch using unified API
                    unified_request = {
                        "operation": "upsert" if upsert else "insert",
                        "collection_id": collection_id,
                        "vectors": batch_data
                    }
                    response = self._make_request(
                        "POST",
                        "/api/v1/vector/batch",
                        json=unified_request
                    )
                    
                    batch_response = response.json()
                    if "data" in batch_response and "success" in batch_response:
                        batch_count = len(batch_response["data"]) if batch_response["data"] else 0
                        total_successful += batch_count
                    else:
                        total_failed += len(batch_data)
                    
                    # Check for errors in response
                    if batch_response.get("error"):
                        all_errors.append(f"Batch {i//effective_batch_size}: {batch_response['error']}")
                
                except Exception as e:
                    total_failed += len(batch_data)
                    all_errors.append(f"Batch {i//effective_batch_size}: {str(e)}")
            
            return BatchResult(
                total_count=len(vector_data),
                successful_count=total_successful,
                failed_count=total_failed,
                duration_ms=0,  # Total duration not tracked for multi-batch
                errors=all_errors if all_errors else None
            )
    
    def search(
        self,
        collection_id: str,
        vector: Union[List[float], np.ndarray],
        top_k: int = 10,
        metadata_filter: Optional[FilterDict] = None,
        include_vectors: bool = False,
        include_metadata: bool = True,
        optimization_level: str = "high",
        use_storage_aware: bool = True,
        quantization_level: str = "FP32",
        enable_simd: bool = True,
        timeout: Optional[float] = None,
    ) -> List[SearchResult]:
        """Search for similar vectors with storage-aware optimizations
        
        Args:
            collection_id: Target collection ID
            vector: Query vector
            top_k: Number of results to return
            metadata_filter: Metadata filter conditions
            include_vectors: Include vector data in results
            include_metadata: Include metadata in results
            optimization_level: Search optimization level ('high', 'medium', 'low')
            use_storage_aware: Enable storage-aware polymorphic search
            quantization_level: Vector quantization level ('FP32', 'PQ8', 'PQ4', 'Binary')
            enable_simd: Enable SIMD vectorization optimizations
            timeout: Request timeout override
            
        Returns:
            List of search results ordered by similarity
        """
        # Normalize query vector
        if isinstance(vector, np.ndarray):
            if vector.dtype != np.float32:
                vector = vector.astype(np.float32)
            vector = vector.tolist()
        
        # Simplified request aligned with server expectations
        query = {"vector": vector}
        
        # Add metadata filter if provided  
        if metadata_filter:
            # Convert dict to proper filter format
            query["metadata_filter"] = {
                "operator": "AND",
                "conditions": [
                    {
                        "field_name": key,
                        "operation": "EQUALS", 
                        "value": {"string_value": str(value)}
                    }
                    for key, value in metadata_filter.items()
                ]
            }
        
        request_data = {
            "collection_id": collection_id,
            "queries": [query],
            "top_k": top_k,
            "include_fields": {
                "vector": include_vectors,
                "metadata": include_metadata,
                "score": True,
                "rank": True
            }
        }
        
        response = self._make_request(
            "POST",
            "/api/v1/vector/search",
            json=request_data,
            timeout=timeout or self.config.timeout,
        )
        
        response_data = response.json()
        
        # Check for errors in response
        if response_data.get("error_message"):
            error_msg = response_data.get("error_message")
            if "not found" in error_msg.lower():
                # For collection not found, return empty results (common pattern)
                return []
            else:
                raise ProximaDBError(f"Search failed: {error_msg}")
        
        # Handle proto-aligned response format
        if "results" in response_data:
            # Extract search results from results field
            search_results = []
            for result in response_data["results"]:
                # Convert proto-aligned result to SearchResult
                search_results.append(SearchResult(
                    id=result.get("id", ""),
                    score=result.get("score", 0.0),
                    distance=result.get("distance", 0.0),
                    rank=result.get("rank", 0),
                    vector=result.get("vector", []) if include_vectors else [],
                    metadata=result.get("metadata", {}) if include_metadata else {},
                    collection_id=collection_id
                ))
            return search_results
        else:
            # Fallback for other response formats
            return []
    
    def search_batch(
        self,
        collection_id: str,
        queries: VectorArray,
        k: int = 10,
        filter: Optional[FilterDict] = None,
        **kwargs
    ) -> List[List[SearchResult]]:
        """Search multiple queries in batch
        
        Args:
            collection_id: Target collection ID
            queries: Array of query vectors
            k: Number of results per query
            filter: Metadata filter conditions
            **kwargs: Additional search parameters
            
        Returns:
            List of search results for each query
        """
        queries_list = self._normalize_vectors(queries)
        
        request_data = {
            "queries": queries_list,
            "k": k,
            "filter": filter,
            "params": {
                "include_metadata": kwargs.get("include_metadata", True),
                "include_vectors": kwargs.get("include_vectors", False),
                "exact_search": kwargs.get("exact", False),
            }
        }
        
        if ef := kwargs.get("ef"):
            request_data["params"]["ef"] = ef
        
        response = self._make_request(
            "POST",
            f"/collections/{collection_id}/search/batch",
            json=request_data,
        )
        
        batch_response = response.json()
        return [
            [SearchResult(**result) for result in query_results]
            for query_results in batch_response["results"]
        ]
    
    def delete_vector(self, collection_id: str, vector_id: str) -> DeleteResult:
        """Delete a single vector"""
        # Use batch delete endpoint with single vector ID
        request_data = {"ids": [vector_id]}
        
        response = self._make_request(
            "DELETE",
            f"/api/v1/vectors/{collection_id}",
            json=request_data
        )
        response_data = response.json()
        
        # Convert VectorOperationResponse to DeleteResult
        return DeleteResult(
            success=response_data.get("success", False),
            deleted_count=response_data.get("metrics", {}).get("successful_count", 0),
            errors=[]
        )
    
    def delete_vectors(self, collection_id: str, vector_ids: List[str]) -> DeleteResult:
        """Delete multiple vectors"""
        request_data = {"ids": vector_ids}
        
        response = self._make_request(
            "DELETE",
            f"/api/v1/vectors/{collection_id}",
            json=request_data
        )
        response_data = response.json()
        
        # Convert VectorOperationResponse to DeleteResult
        return DeleteResult(
            success=response_data.get("success", False),
            deleted_count=response_data.get("metrics", {}).get("successful_count", 0),
            errors=[]
        )
    
    def get_vector(
        self,
        collection_id: str,
        vector_id: str,
        include_vector: bool = True,
        include_metadata: bool = True,
    ) -> Optional[Dict[str, Any]]:
        """Get a single vector by ID"""
        params = {
            "include_vector": include_vector,
            "include_metadata": include_metadata,
        }
        
        response = self._make_request(
            "GET",
            f"/api/v1/vector/get/{collection_id}/{vector_id}",
            params=params
        )
        data = response.json()
        
        # Check if vector was found
        if not data or (isinstance(data, dict) and data.get('error')):
            raise ProximaDBError(f"Vector not found: {vector_id}")
        
        return data
    
    def upsert_vectors(
        self,
        collection_id: str,
        records: List[Any],
    ) -> BatchResult:
        """Upsert multiple vectors (insert or update)
        
        Args:
            collection_id: Target collection ID
            records: List of VectorRecord objects
            
        Returns:
            Batch operation result
        """
        # Convert VectorRecord objects to the format expected by insert_vectors
        vectors = []
        ids = []
        metadatas = []
        
        for record in records:
            vectors.append(record.vector)
            ids.append(record.id)
            metadatas.append(record.metadata if record.metadata else {})
        
        return self.insert_vectors(
            collection_id=collection_id,
            vectors=vectors,
            ids=ids,
            metadata=metadatas,
            upsert=True
        )
    
    def update_vector(
        self,
        collection_id: str,
        vector_id: str,
        vector: Optional[Union[List[float], np.ndarray]] = None,
        metadata: Optional[MetadataDict] = None,
    ) -> BatchResult:
        """Update an existing vector"""
        update_data = {}
        
        if vector is not None:
            if isinstance(vector, np.ndarray):
                if vector.dtype != np.float32:
                    vector = vector.astype(np.float32)
                vector = vector.tolist()
            update_data["vector"] = vector
        
        if metadata is not None:
            update_data["metadata"] = metadata
        
        response = self._make_request(
            "PUT",
            f"/collections/{collection_id}/vectors/{vector_id}",
            json=update_data
        )
        # Convert response to BatchResult
        resp_data = response.json()
        return BatchResult(
            total=resp_data.get('total', 1),
            success=resp_data.get('success', 1),
            failed=resp_data.get('failed', 0),
            errors=resp_data.get('errors', []),
            duration_ms=resp_data.get('duration_ms', 0.0)
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


# Convenience functions
def connect(
    url: Optional[str] = None,
    api_key: Optional[str] = None,
    **kwargs
) -> ProximaDBClient:
    """Create a ProximaDB client with simplified parameters"""
    return ProximaDBClient(url=url, api_key=api_key, **kwargs)


def quick_search(
    collection_id: str,
    query: Union[List[float], np.ndarray],
    k: int = 10,
    url: Optional[str] = None,
    api_key: Optional[str] = None,
) -> List[SearchResult]:
    """Quick one-off search without creating persistent client"""
    with connect(url=url, api_key=api_key) as client:
        return client.search(collection_id, query, k)