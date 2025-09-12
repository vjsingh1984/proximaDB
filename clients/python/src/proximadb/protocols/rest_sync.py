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
import gzip
import json
from typing import Any, Dict, List, Optional, Union, Iterator, Tuple
import warnings

import numpy as np
import httpx
from tenacity import retry, stop_after_attempt, wait_exponential, retry_if_exception_type

from ..config import ClientConfig, load_config
from ..metadata_utils import json_compatible_value
from ..batching_unified import ThreadedBatchProcessor, BatchConfig, BatchStrategy, UnifiedBatchManager
from ..cache import ResponseCache, CacheStrategy
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
    VectorSearchRequest,
    SearchQuery,
    IncludeFields,
    MetadataFilter,
    ServerCapabilities,
    FilterCondition,
    FilterOperator,
    FilterOperation,
    VectorBatchRequest,
    VectorRecord,
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
        enable_batching: bool = False,
        batch_config: Optional[BatchConfig] = None,
        enable_caching: bool = False,
        cache_config: Optional[Dict[str, Any]] = None,
        **kwargs
    ) -> None:
        """Initialize ProximaDB client
        
        Args:
            url: ProximaDB server URL
            api_key: API key for authentication
            config: Client configuration object
            enable_batching: Enable request batching for improved throughput
            batch_config: Configuration for batching behavior
            enable_caching: Enable response caching for read operations
            cache_config: Configuration for caching behavior
            **kwargs: Additional configuration parameters
        """
        if config is None:
            config = load_config(url=url, api_key=api_key, **kwargs)
        
        self.config = config
        self._setup_logging()
        
        # Initialize HTTP client
        self._http_client = self._create_http_client()
        # Capability probes (cached after first attempt)
        self._sks_search_supported: Optional[bool] = None
        self._sks_entities_supported: Optional[bool] = None
        
        # Initialize request batching if enabled
        self.enable_batching = enable_batching
        self._batch_processor: Optional[ThreadedBatchProcessor] = None
        
        if enable_batching:
            # Create batch processor for REST operations
            batch_config = batch_config or BatchConfig()
            # For now, just store the config - actual batching will be implemented later
            self._batch_config = batch_config
            logger.info("Batching enabled with config: %s", batch_config)
        
        # Initialize response caching if enabled
        self.enable_caching = enable_caching
        self._response_cache: Optional[ResponseCache] = None
        
        if enable_caching:
            self._response_cache = ResponseCache(
                # Use default cache settings if not provided
                default_ttl=cache_config.get('default_ttl', 300) if cache_config else 300
            )
            logger.info("Enabled response caching for read operations")
        
        # SKS capability cache and warmup tracking
        self._sks_search_supported: Optional[bool] = None
        self._sks_entities_supported: Optional[bool] = None
        self._warmed_collections: set[str] = set()
        logger.info(f"Initialized ProximaDB client for {self.config.url}")

    def _auto_warmup(self, collection_id: str) -> None:
        """Auto-warmup SKS support once per collection (best-effort)."""
        if not collection_id or collection_id in self._warmed_collections:
            return
        if self._sks_search_supported is None or self._sks_entities_supported is None:
            try:
                self.warmup_sks_capabilities(collection_id)
            except Exception:
                pass
        self._warmed_collections.add(collection_id)

    def get_capabilities(self) -> Dict[str, Any]:
        """Return cached capability flags and warmed collections."""
        return {
            "sks_search_supported": self._sks_search_supported,
            "sks_entities_supported": self._sks_entities_supported,
            "warmed_collections": list(self._warmed_collections),
        }
    
    def _setup_logging(self) -> None:
        """Setup logging configuration"""
        if self.config.enable_debug_logging:
            level = logging.DEBUG
        else:
            # Handle both enum and string log levels
            if hasattr(self.config.log_level, 'value'):
                level = getattr(logging, self.config.log_level.value)
            else:
                level = getattr(logging, str(self.config.log_level).upper(), logging.INFO)
        
        logging.getLogger("proximadb").setLevel(level)
    
    def _create_http_client(self) -> httpx.Client:
        """Create configured HTTP client with compression support"""
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
    
    def _compress_data(self, data: bytes) -> bytes:
        """Compress data using configured algorithm"""
        algorithm = self.config.compression.algorithm.lower()
        level = self.config.compression.level
        
        if algorithm == 'gzip':
            return gzip.compress(data, compresslevel=level or 6)
        elif algorithm == 'deflate':
            import zlib
            return zlib.compress(data, level=level or 6)
        elif algorithm == 'zstd':
            try:
                import zstandard
                cctx = zstandard.ZstdCompressor(level=level or 3)
                return cctx.compress(data)
            except ImportError:
                logger.warning("zstd not available, falling back to gzip")
                return gzip.compress(data, compresslevel=6)
        elif algorithm == 'br' or algorithm == 'brotli':
            try:
                import brotli
                return brotli.compress(data, quality=level or 4)
            except ImportError:
                logger.warning("brotli not available, falling back to gzip")
                return gzip.compress(data, compresslevel=6)
        else:
            # Default to gzip if unknown algorithm
            return gzip.compress(data, compresslevel=6)
    
    def _make_request(self, method: str, endpoint: str, **kwargs) -> httpx.Response:
        """Make HTTP request with retry logic and optional compression"""
        
        # Debug log the endpoint
        logger.debug(f"Making {method} request to {endpoint}")
        
        # Handle request compression if enabled
        # Use rest_enabled for REST protocol
        if hasattr(self.config, 'compression') and self.config.compression.enabled and 'json' in kwargs:
            json_data = kwargs.pop('json')
            json_bytes = json.dumps(json_data).encode('utf-8')
            
            # Debug the payload size
            logger.debug(f"Request payload size: {len(json_bytes)} bytes")
            
            # Only compress if data is larger than threshold
            if len(json_bytes) > self.config.compression.threshold_bytes:
                compressed_data = self._compress_data(json_bytes)
                kwargs['content'] = compressed_data
                kwargs['headers'] = kwargs.get('headers', {})
                
                # Set correct Content-Encoding based on algorithm
                algorithm = self.config.compression.algorithm.lower()
                if algorithm == 'br' or algorithm == 'brotli':
                    kwargs['headers']['Content-Encoding'] = 'br'
                elif algorithm == 'deflate':
                    kwargs['headers']['Content-Encoding'] = 'deflate'
                elif algorithm == 'zstd':
                    kwargs['headers']['Content-Encoding'] = 'zstd'
                else:  # default to gzip
                    kwargs['headers']['Content-Encoding'] = 'gzip'
                    
                kwargs['headers']['Content-Type'] = 'application/json'
                
                logger.debug(
                    f"Compressed request: {len(json_bytes)} -> {len(compressed_data)} bytes "
                    f"({100 * (1 - len(compressed_data) / len(json_bytes)):.1f}% reduction)"
                )
            else:
                # Data too small to benefit from compression
                kwargs['json'] = json_data
        
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
        
        # Inform users about potential server fallbacks for better UX
        fallback_warnings = []
        
        if config.distance_metric:
            metric_str = config.distance_metric if isinstance(config.distance_metric, str) else config.distance_metric.value
            if not ServerCapabilities.is_supported("distance_metric", metric_str):
                fallback = ServerCapabilities.get_fallback_for("distance_metric", metric_str)
                if fallback:
                    fallback_warnings.append(
                        f"💡 Distance metric '{metric_str}' will fallback to '{fallback}' (server decision). "
                        f"For guaranteed support, use: {', '.join(ServerCapabilities().supported_distance_metrics)}"
                    )
        
        if config.storage_engine:
            engine_str = config.storage_engine if isinstance(config.storage_engine, str) else config.storage_engine.value
            if not ServerCapabilities.is_supported("storage_engine", engine_str):
                fallback = ServerCapabilities.get_fallback_for("storage_engine", engine_str)
                if fallback:
                    fallback_warnings.append(
                        f"💡 Storage engine '{engine_str}' will fallback to '{fallback}' (server decision). "
                        f"For guaranteed support, use: {', '.join(ServerCapabilities().supported_storage_engines)}"
                    )
        
        if config.index_configs:
            for ic in (config.index_configs or []):
                algo_str = ic.algorithm if isinstance(ic.algorithm, str) else ic.algorithm.value
                if not ServerCapabilities.is_supported("indexing_algorithm", algo_str):
                    fallback = ServerCapabilities.get_fallback_for("indexing_algorithm", algo_str)
                    if fallback:
                        fallback_warnings.append(
                            f"💡 Indexing algorithm '{algo_str}' will fallback to '{fallback}' (server decision). "
                            f"For guaranteed support, use: {', '.join(ServerCapabilities().supported_indexing_algorithms)}"
                        )
        
        # Issue all fallback warnings at once for better UX
        if fallback_warnings:
            combined_message = (
                f"ProximaDB server will make intelligent fallback decisions for collection '{name}':\n" +
                "\n".join(f"  • {warning}" for warning in fallback_warnings) +
                f"\n\n📚 Server uses smart defaults to ensure your collection works. "
                f"Check the returned collection config to see final server decisions."
            )
            warnings.warn(combined_message, UserWarning)
        
        # Build config object as expected by server
        config_data: Dict[str, Any] = {
            "name": name,
            "dimension": config.dimension,
            "distance_metric": config.distance_metric if isinstance(config.distance_metric, str) else getattr(config.distance_metric, 'value', 'cosine'),
            "storage_engine": getattr(config, 'storage_engine', 'viper') if isinstance(getattr(config, 'storage_engine', 'viper'), str) else getattr(getattr(config, 'storage_engine', None), 'value', 'viper'),
        }

        # Build index_configs aligned with proto
        index_configs: List[Dict[str, Any]] = []
        primary_index_name: Optional[str] = None
        if getattr(config, 'index_configs', None):
            for ic in (config.index_configs or []):
                algo_str = ic.algorithm if isinstance(ic.algorithm, str) else ic.algorithm.value
                entry: Dict[str, Any] = {
                    "index_name": ic.index_name,
                    "algorithm": algo_str,
                    "is_primary": bool(getattr(ic, 'is_primary', False)),
                }
                if entry["is_primary"]:
                    primary_index_name = ic.index_name
                index_configs.append(entry)
        else:
            # Default to a primary HNSW index when none provided
            primary_index_name = f"{name}_primary"
            index_configs.append({
                "index_name": primary_index_name,
                "algorithm": "hnsw",
                "is_primary": True,
            })

        config_data["index_configs"] = index_configs
        if primary_index_name:
            config_data["primary_index"] = primary_index_name

        # Quantization
        if getattr(config, 'quantization', None):
            try:
                # Pydantic v2
                q = config.quantization.model_dump(exclude_none=True)
            except Exception:
                try:
                    q = config.quantization.dict(exclude_none=True)
                except Exception:
                    q = {}
            config_data["quantization"] = q
        
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
            coll_data = response_data["collection"]
            if not coll_data:
                if "error" in response_data:
                    raise ProximaDBError(f"Collection creation failed: {response_data['error']}")
                raise ProximaDBError(f"Unexpected response format: {response_data}")

            # Build config from response if present; otherwise fallback to request config
            cfg_src = coll_data.get("config", {})
            cfg = CollectionConfig(
                name=cfg_src.get("name", name),
                dimension=cfg_src.get("dimension", config.dimension),
                distance_metric=cfg_src.get("distance_metric", getattr(config, 'distance_metric', 'cosine')),
                storage_engine=cfg_src.get("storage_engine", getattr(config, 'storage_engine', 'viper')),
            )
            return Collection(
                id=coll_data.get("id", name),
                config=cfg,
                created_at=coll_data.get("created_at"),
                updated_at=coll_data.get("updated_at"),
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
            primary_indexing_algorithm = None
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
                primary_indexing_algorithm = None
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
        metadata_items = self._convert_metadata_to_rest_format(metadata) if metadata else []
        
        # Use batch API with single vector
        vector_record = {
            "id": vector_id,
            "collection_id": collection_id,
            "vector": vector,
            "metadata": metadata_items,
            "timestamp": int(time.time()),  # Seconds (proto expects seconds)
            "version": 1
        }
        
        request_data = {
            "operation": "upsert" if upsert else "insert",
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
    
    def _convert_metadata_to_rest_format(self, metadata_dict: Dict[str, Any]) -> List[Dict[str, Any]]:
        """Convert Python dict metadata to REST API MetadataItem array format"""
        if not metadata_dict:
            return []
        
        items = []
        for key, value in metadata_dict.items():
            item = {"key": key}
            
            # Set the appropriate typed value field
            if isinstance(value, bool):
                item["bool_value"] = value
            elif isinstance(value, (int, float)):
                item["number_value"] = float(value)
            elif isinstance(value, str):
                item["string_value"] = value
            elif value is None:
                item["string_value"] = ""
            else:
                item["string_value"] = str(value)
                
            items.append(item)
        return items
    
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
        self._auto_warmup(collection_id)
        vectors_list = self._normalize_vectors(vectors)
        
        if len(vectors_list) != len(ids):
            raise ValueError("Number of vectors must match number of IDs")
        
        if metadata and len(metadata) != len(vectors_list):
            raise ValueError("Number of metadata items must match number of vectors")
        
        # Prepare vector data
        vector_data = []
        for i, (vector_id, vector) in enumerate(zip(ids, vectors_list)):
            # Convert metadata dict to REST API format
            metadata_items = self._convert_metadata_to_rest_format(
                metadata[i] if metadata else {}
            )
            item = {
                "id": vector_id,
                "vector": vector,
                "metadata": metadata_items,
                "timestamp": int(time.time())  # Current time in seconds
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
            
            # Debug logging
            logger.debug(f"Sending vector batch request with {len(vector_data)} vectors")
            logger.debug(f"Request payload preview: operation={unified_request['operation']}, collection_id={collection_id}, vector_count={len(vector_data)}")
            if vector_data:
                logger.debug(f"First vector: id={vector_data[0].get('id')}, metadata_items={len(vector_data[0].get('metadata', []))}")
            
            # Print full request for debugging
            import json as debug_json
            logger.debug(f"Full request JSON:\n{debug_json.dumps(unified_request, indent=2)[:1000]}")
            
            # Try SKS entities endpoint first if supported/unknown
            response = None
            if self._sks_entities_supported is not False:
                try:
                    response = self._make_request(
                        "POST",
                        f"/api/v1/entities/{collection_id}",
                        json=unified_request
                    )
                    # If we reached here without exception, consider SKS entities supported
                    self._sks_entities_supported = True
                except Exception as e:
                    # Mark unsupported on 404/405; otherwise remain unknown to retry later
                    try:
                        status = getattr(e, 'response', None).status_code if hasattr(e, 'response') else None
                    except Exception:
                        status = None
                    if status in (404, 405, 501):
                        self._sks_entities_supported = False
                    # Fallback to legacy endpoint
                    response = self._make_request(
                        "POST",
                        "/api/v1/vector/batch",
                        json=unified_request
                    )
            else:
                # Known unsupported: use legacy endpoint directly
                response = self._make_request(
                    "POST",
                    "/api/v1/vector/batch",
                    json=unified_request
                )
            
            response_data = response.json()
            # Handle unified API response
            if "metrics" in response_data:
                metrics = response_data["metrics"]
                result = BatchResult(
                    total=metrics.get("total_processed", len(vector_data)),
                    success=metrics.get("successful_count", len(vector_data)),
                    failed=metrics.get("failed_count", 0),
                    errors=[],
                    duration_ms=metrics.get("processing_time_us", 0) / 1000.0
                )
                
                # Invalidate cache for collection after successful write
                if result.success > 0:
                    self._invalidate_collection_cache(collection_id)
                
                return result
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
                    # For multi-batch, use legacy endpoint to minimize negotiation overhead
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
                errors=all_errors if all_errors else []
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
        search_hints: Optional[Dict[str, Any]] = None,
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
        
        # Build metadata filter if provided
        metadata_filter_obj = None
        if metadata_filter:
            conditions = [
                FilterCondition(
                    field_name=key,
                    operation=FilterOperation.EQUALS,
                    value=value
                )
                for key, value in metadata_filter.items()
            ]
            metadata_filter_obj = MetadataFilter(
                conditions=conditions,
                operator=FilterOperator.AND
            )
        
        # Create search query using model
        search_query = SearchQuery(
            vector=vector,
            id=None,
            metadata_filter=metadata_filter_obj
        )
        
        # Create search request using model (legacy/compat path)
        search_request = VectorSearchRequest(
            collection_id=collection_id,
            queries=[search_query],
            top_k=top_k,
            distance_metric_override=None,  # Use collection default
            search_parameters=None,  # Use defaults
            include_fields=IncludeFields(
                vector=include_vectors,
                metadata=include_metadata,
                score=True,
                rank=True
            ),
            search_optimization=None  # Will be set below
        )
        
        # Convert model to dict for JSON serialization
        request_data = search_request.model_dump(exclude_none=True)
        
        # Add search optimization if hints provided
        if search_hints:
            from ..search_utils import build_search_optimization_rest
            optimization = build_search_optimization_rest(
                enable_two_stage=search_hints.get('enable_two_stage'),
                quantization_hint=search_hints.get('quantization_hint', quantization_level),
                accuracy_threshold=search_hints.get('accuracy_threshold'),
                enable_clustering_hint=search_hints.get('enable_clustering_hint'),
                enable_metadata_filtering_hint=search_hints.get('enable_metadata_filtering_hint'),
                custom_hints=search_hints.get('custom_hints')
            )
            if optimization:
                request_data['search_optimization'] = optimization
        
        # Try SKS-aligned endpoint first; fall back to legacy if unavailable
        if self._sks_search_supported is not False:
            try:
                sks_body = {
                    "vector": vector,
                    "top_k": top_k,
                    "include_vector": include_vectors,
                    "include_metadata": include_metadata,
                }
                sks_resp = self._make_request(
                    "POST",
                    f"/api/v1/search/{collection_id}",
                    json=sks_body,
                    timeout=timeout or self.config.timeout,
                )
                sks_data = sks_resp.json()
                if isinstance(sks_data, dict) and "items" in sks_data:
                    self._sks_search_supported = True
                    results: List[SearchResult] = []
                    for item in sks_data.get("items", []) or []:
                        results.append(
                            SearchResult(
                                id=item.get("entity_id") or item.get("id", ""),
                                score=item.get("score", 0.0),
                                vector=None,  # vectors omitted unless server supports include_vector
                                metadata=(item.get("typed_metadata") or item.get("metadata") or {}) if include_metadata else None,
                            )
                        )
                    return results
            except Exception as e:
                # Disable SKS probe on explicit 404/405/501; otherwise leave as None to retry later
                try:
                    status = getattr(e, 'response', None).status_code if hasattr(e, 'response') else None
                except Exception:
                    status = None
                if status in (404, 405, 501):
                    self._sks_search_supported = False

        # Legacy vector search path
        response = self._make_request(
            "POST",
            "/api/v1/vector/search",
            json=request_data,
            timeout=timeout or self.config.timeout,
        )
        response_data = response.json()
        
        # Check for errors in response
        if isinstance(response_data, dict) and response_data.get("error_message"):
            error_msg = response_data.get("error_message")
            if isinstance(error_msg, str) and "not found" in error_msg.lower():
                return []
            raise ProximaDBError(f"Search failed: {error_msg}")
        
        # Handle proto-aligned response format
        results: List[SearchResult] = []
        for result in response_data.get("results", []) or []:
            results.append(
                SearchResult(
                    id=result.get("id", ""),
                    score=result.get("score", 0.0),
                    rank=result.get("rank", 0),
                    vector=(result.get("vector", []) if include_vectors else None),
                    metadata=(result.get("metadata", {}) if include_metadata else None),
                )
            )
        return results

    def search_envelope(
        self,
        collection_id: str,
        vector: Union[List[float], np.ndarray],
        top_k: int = 10,
        include_vectors: bool = False,
        include_metadata: bool = True,
        timeout: Optional[float] = None,
    ) -> 'SearchEnvelope':
        """Search returning SKS envelope (cursor/progress) when supported.

        Falls back to legacy path and returns envelope with no cursor/progress.
        """
        from ..models import SearchEnvelope, SearchProgress
        self._auto_warmup(collection_id)
        # Normalize vector
        if isinstance(vector, np.ndarray):
            if vector.dtype != np.float32:
                vector = vector.astype(np.float32)
            vector = vector.tolist()

        # Try SKS
        if self._sks_search_supported is not False:
            try:
                sks_body = {
                    "vector": vector,
                    "top_k": top_k,
                    "include_vector": include_vectors,
                    "include_metadata": include_metadata,
                }
                sks_resp = self._make_request(
                    "POST",
                    f"/api/v1/search/{collection_id}",
                    json=sks_body,
                    timeout=timeout or self.config.timeout,
                )
                sks_data = sks_resp.json()
                if isinstance(sks_data, dict) and ("items" in sks_data):
                    self._sks_search_supported = True
                    # Map envelope
                    items: List[SearchResult] = []
                    for item in sks_data.get("items", []) or []:
                        items.append(
                            SearchResult(
                                id=item.get("entity_id") or item.get("id", ""),
                                score=item.get("score", 0.0),
                                vector=None if not include_vectors else item.get("vector"),
                                metadata=(item.get("typed_metadata") or item.get("metadata") or {}) if include_metadata else None,
                            )
                        )
                    progress = None
                    if sks_data.get("progress"):
                        pr = sks_data["progress"]
                        progress = SearchProgress(stage=pr.get("stage", 0), stages=pr.get("stages", 0), complete=pr.get("complete", False))
                    cursor = None
                    has_more = False
                    if sks_data.get("page"):
                        page = sks_data["page"]
                        cursor = page.get("cursor")
                        has_more = page.get("has_more", False)
                    return SearchEnvelope(items=items, total=sks_data.get("total"), cursor=cursor, has_more=has_more, progress=progress)
            except Exception as e:
                try:
                    status = getattr(e, 'response', None).status_code if hasattr(e, 'response') else None
                except Exception:
                    status = None
                if status in (404, 405, 501):
                    self._sks_search_supported = False

        # Legacy fallback
        results = self.search(collection_id, vector, top_k, include_vectors=include_vectors, include_metadata=include_metadata, timeout=timeout)
        return SearchEnvelope(items=results, total=None, cursor=None, has_more=False, progress=None)

    # -----------------------------
    # Graph Operations (REST)
    # -----------------------------
    def graph_shortest_path(
        self,
        start_node_id: str,
        target_node_id: str,
        max_depth: Optional[int] = None,
        edge_types: Optional[List[str]] = None,
        algorithm: str = "DIJKSTRA",
        k: Optional[int] = None,
        enable_prefetch: Optional[bool] = None,
        prefetch_budget: Optional[int] = None,
        timeout: Optional[float] = None,
    ) -> Dict[str, Any]:
        """Compute shortest path via REST with optional prefetch overrides.

        Per-call overrides can be sent as JSON fields or HTTP headers. This method
        sends overrides as headers to keep the body stable.
        """
        body = {
            "start_node_id": start_node_id,
            "target_node_id": target_node_id,
            "algorithm": algorithm,
        }
        if max_depth is not None:
            body["max_depth"] = max_depth
        if edge_types:
            body["edge_types"] = edge_types
        if k is not None:
            body["k"] = k

        headers: Dict[str, str] = {"Content-Type": "application/json"}
        if enable_prefetch is not None:
            headers["x-graph-prefetch-enabled"] = "true" if enable_prefetch else "false"
        if prefetch_budget is not None:
            headers["x-graph-prefetch-budget"] = str(prefetch_budget)

        # Also include overrides in body for endpoints that accept JSON fields
        if enable_prefetch is not None:
            body["enable_prefetch"] = bool(enable_prefetch)
        if prefetch_budget is not None:
            body["prefetch_budget"] = int(prefetch_budget)

        resp = self._make_request(
            "POST",
            "/api/v1/graph/shortest_path",
            json=body,
            headers=headers,
            timeout=timeout or self.config.timeout,
        )
        return resp.json()

    def graph_traverse(
        self,
        start_node_id: str,
        max_depth: int = 3,
        edge_types: Optional[List[str]] = None,
        algorithm: str = "BFS",
        limit: Optional[int] = None,
        timeout_ms: Optional[int] = None,
        max_frontier: Optional[int] = None,
        enable_prefetch: Optional[bool] = None,
        prefetch_budget: Optional[int] = None,
        timeout: Optional[float] = None,
    ) -> Dict[str, Any]:
        """Perform graph traversal via REST with optional prefetch overrides.

        Overrides are sent via headers. Returns the traversal response JSON.
        """
        body: Dict[str, Any] = {
            "start_node_id": start_node_id,
            "max_depth": max_depth,
            "algorithm": algorithm,
        }
        if edge_types:
            body["edge_types"] = edge_types
        if limit is not None:
            body["limit"] = limit
        if timeout_ms is not None:
            body["timeout_ms"] = timeout_ms
        if max_frontier is not None:
            body["max_frontier"] = max_frontier

        headers: Dict[str, str] = {"Content-Type": "application/json"}
        if enable_prefetch is not None:
            headers["x-graph-prefetch-enabled"] = "true" if enable_prefetch else "false"
        if prefetch_budget is not None:
            headers["x-graph-prefetch-budget"] = str(prefetch_budget)

        # Also include overrides in body for compatibility
        if enable_prefetch is not None:
            body["enable_prefetch"] = bool(enable_prefetch)
        if prefetch_budget is not None:
            body["prefetch_budget"] = int(prefetch_budget)

        resp = self._make_request(
            "POST",
            "/api/v1/graph/traverse",
            json=body,
            headers=headers,
            timeout=timeout or self.config.timeout,
        )
        return resp.json()

    def search_next_page(
        self,
        collection_id: str,
        cursor: str,
        include_vectors: bool = False,
        include_metadata: bool = True,
        timeout: Optional[float] = None,
    ) -> 'SearchEnvelope':
        """Fetch next SKS search page by cursor. Returns empty envelope if unsupported."""
        from ..models import SearchEnvelope, SearchProgress
        self._auto_warmup(collection_id)
        if not cursor:
            return SearchEnvelope(items=[], total=None, cursor=None, has_more=False, progress=None)
        if self._sks_search_supported is not True:
            # Unsupported or unknown
            return SearchEnvelope(items=[], total=None, cursor=None, has_more=False, progress=None)
        # Query param per SKS design
        try:
            resp = self._make_request(
                "POST",
                f"/api/v1/search/{collection_id}?cursor={cursor}",
                json={"include_vector": include_vectors, "include_metadata": include_metadata},
                timeout=timeout or self.config.timeout,
            )
            data = resp.json()
            if not isinstance(data, dict) or "items" not in data:
                return SearchEnvelope(items=[], total=None, cursor=None, has_more=False, progress=None)
            items: List[SearchResult] = []
            for item in data.get("items", []) or []:
                items.append(
                    SearchResult(
                        id=item.get("entity_id") or item.get("id", ""),
                        score=item.get("score", 0.0),
                        vector=None if not include_vectors else item.get("vector"),
                        metadata=(item.get("typed_metadata") or item.get("metadata") or {}) if include_metadata else None,
                    )
                )
            progress = None
            if data.get("progress"):
                pr = data["progress"]
                progress = SearchProgress(stage=pr.get("stage", 0), stages=pr.get("stages", 0), complete=pr.get("complete", False))
            next_cursor = None
            has_more = False
            if data.get("page"):
                page = data["page"]
                next_cursor = page.get("cursor")
                has_more = page.get("has_more", False)
            return SearchEnvelope(items=items, total=data.get("total"), cursor=next_cursor, has_more=has_more, progress=progress)
        except Exception:
            return SearchEnvelope(items=[], total=None, cursor=None, has_more=False, progress=None)
    
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
        """Delete a single vector (SKS-first fallback)"""
        self._auto_warmup(collection_id)
        # Try SKS entity DELETE first if supported/unknown
        if getattr(self, "_sks_entities_supported", None) is not False:
            try:
                resp = self._make_request(
                    "DELETE",
                    f"/api/v1/entities/{collection_id}/{vector_id}",
                )
                data = resp.json() if hasattr(resp, 'json') else {}
                self._sks_entities_supported = True
                return DeleteResult(
                    success=bool(data.get("success", True)),
                    deleted_count=1,
                    errors=[],
                )
            except Exception as e:
                try:
                    status = getattr(e, 'response', None).status_code if hasattr(e, 'response') else None
                except Exception:
                    status = None
                if status in (404, 405, 501):
                    self._sks_entities_supported = False
                # fallthrough to legacy
        
        # Legacy batch delete with one ID
        request_data = {"ids": [vector_id]}
        response = self._make_request(
            "DELETE",
            f"/api/v1/vectors/{collection_id}",
            json=request_data
        )
        response_data = response.json()
        return DeleteResult(
            success=response_data.get("success", False),
            deleted_count=response_data.get("metrics", {}).get("successful_count", 0),
            errors=[]
        )
    
    def delete_vectors(self, collection_id: str, vector_ids: List[str]) -> DeleteResult:
        """Delete multiple vectors with SKS-first fallback"""
        self._auto_warmup(collection_id)
        # If SKS entities are supported, issue individual deletes to SKS path
        if getattr(self, "_sks_entities_supported", False):
            deleted = 0
            errors: List[str] = []
            for vid in vector_ids:
                try:
                    res = self.delete_vector(collection_id, vid)
                    if res.success:
                        deleted += 1
                except Exception as e:
                    errors.append(str(e))
            return DeleteResult(success=deleted == len(vector_ids), deleted_count=deleted, errors=errors)
        
        # Legacy batch delete
        request_data = {"ids": vector_ids}
        response = self._make_request(
            "DELETE",
            f"/api/v1/vectors/{collection_id}",
            json=request_data
        )
        response_data = response.json()
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
        """Get a single vector by ID with SKS-first fallback"""
        self._auto_warmup(collection_id)
        # Try SKS entity GET first if supported/unknown
        if getattr(self, "_sks_entities_supported", None) is not False:
            try:
                params = {
                    "include_vector": include_vector,
                    "include_metadata": include_metadata,
                }
                resp = self._make_request(
                    "GET",
                    f"/api/v1/entities/{collection_id}/{vector_id}",
                    params=params,
                )
                data = resp.json()
                if isinstance(data, dict) and (data.get("id") or data.get("entity_id")):
                    self._sks_entities_supported = True
                    return data
            except Exception as e:
                try:
                    status = getattr(e, 'response', None).status_code if hasattr(e, 'response') else None
                except Exception:
                    status = None
                if status in (404, 405, 501):
                    self._sks_entities_supported = False
                # fallthrough to legacy
        
        # Legacy path
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
        # Close batch processor first
        if self._batch_processor:
            self._batch_processor.close()
            self._batch_processor = None
        
        # Close response cache
        if self._response_cache:
            self._response_cache.close()
            self._response_cache = None
        
        if hasattr(self, '_http_client'):
            self._http_client.close()
    
    # Helper methods for cache-aware operations
    def _cached_get(self, operation: str, collection_id: str, params: Dict[str, Any], fetch_func: callable, ttl_seconds: Optional[float] = None) -> Any:
        """Helper method for cache-aware GET operations"""
        if not self.enable_caching or not self._response_cache:
            return fetch_func()
        
        # Try to get from cache first
        cached_result = self._response_cache.get(operation, collection_id, params)
        if cached_result is not None:
            return cached_result
        
        # Fetch from server
        result = fetch_func()
        
        # Cache the result
        if result is not None:
            self._response_cache.put(operation, collection_id, params, result, ttl_seconds)
        
        return result
    
    def _invalidate_collection_cache(self, collection_id: str):
        """Invalidate cache entries for a collection after write operations"""
        if self.enable_caching and self._response_cache:
            self._response_cache.invalidate_collection(collection_id)
    
    # Cache-aware read operations
    def search_cached(
        self,
        collection_id: str,
        vector: Union[List[float], np.ndarray],
        top_k: int = 10,
        metadata_filter: Optional[FilterDict] = None,
        include_vectors: bool = False,
        include_metadata: bool = True,
        ttl_seconds: Optional[float] = None,
        **kwargs
    ) -> List[SearchResult]:
        """Cache-aware vector search
        
        Args:
            collection_id: Target collection ID
            vector: Query vector
            top_k: Number of results to return
            metadata_filter: Metadata filter conditions
            include_vectors: Include vector data in results
            include_metadata: Include metadata in results
            ttl_seconds: Cache TTL override
            **kwargs: Additional search parameters
            
        Returns:
            List of search results
        """
        if not self.enable_caching:
            return self.search(collection_id, vector, top_k, metadata_filter, include_vectors, include_metadata, **kwargs)
        
        # Create cache key parameters
        cache_params = {
            "vector": vector if isinstance(vector, list) else vector.tolist(),
            "top_k": top_k,
            "metadata_filter": metadata_filter,
            "include_vectors": include_vectors,
            "include_metadata": include_metadata,
            **kwargs
        }
        
        def fetch_func():
            return self.search(collection_id, vector, top_k, metadata_filter, include_vectors, include_metadata, **kwargs)
        
        return self._cached_get("search_vectors", collection_id, cache_params, fetch_func, ttl_seconds)
    
    def get_vector_cached(
        self,
        collection_id: str,
        vector_id: str,
        include_metadata: bool = True,
        ttl_seconds: Optional[float] = None
    ) -> Optional[Dict[str, Any]]:
        """Cache-aware vector retrieval
        
        Args:
            collection_id: Target collection ID
            vector_id: Vector identifier
            include_metadata: Include metadata in response
            ttl_seconds: Cache TTL override
            
        Returns:
            Vector data or None if not found
        """
        if not self.enable_caching:
            return self.get_vector(collection_id, vector_id, include_metadata)
        
        cache_params = {
            "vector_id": vector_id,
            "include_metadata": include_metadata
        }
        
        def fetch_func():
            return self.get_vector(collection_id, vector_id, include_metadata)
        
        return self._cached_get("get_vector", collection_id, cache_params, fetch_func, ttl_seconds)
    
    def list_collections_cached(
        self,
        ttl_seconds: Optional[float] = None
    ) -> List[Collection]:
        """Cache-aware collection listing
        
        Args:
            ttl_seconds: Cache TTL override
            
        Returns:
            List of collections
        """
        if not self.enable_caching:
            return self.list_collections()
        
        cache_params = {}  # No parameters for list collections
        
        def fetch_func():
            return self.list_collections()
        
        return self._cached_get("list_collections", "_global", cache_params, fetch_func, ttl_seconds)
    
    def get_collection_cached(
        self,
        collection_id: str,
        ttl_seconds: Optional[float] = None
    ) -> Optional[Collection]:
        """Cache-aware collection retrieval
        
        Args:
            collection_id: Collection identifier
            ttl_seconds: Cache TTL override
            
        Returns:
            Collection object or None if not found
        """
        if not self.enable_caching:
            return self.get_collection(collection_id)
        
        cache_params = {}  # No additional parameters
        
        def fetch_func():
            return self.get_collection(collection_id)
        
        return self._cached_get("get_collection", collection_id, cache_params, fetch_func, ttl_seconds)
    
    def get_cache_stats(self) -> Dict[str, Any]:
        """Get cache performance statistics
        
        Returns:
            Dictionary of cache statistics
            
        Raises:
            RuntimeError: If caching is not enabled
        """
        if not self.enable_caching or not self._response_cache:
            return {"error": "Caching is not enabled"}
        
        return self._response_cache.get_stats()
    
    def clear_cache(self) -> int:
        """Clear all cached responses
        
        Returns:
            Number of entries cleared
            
        Raises:
            RuntimeError: If caching is not enabled
        """
        if not self.enable_caching or not self._response_cache:
            raise RuntimeError("Caching is not enabled. Initialize client with enable_caching=True")
        
        return self._response_cache.clear()
    
    def invalidate_collection_cache(self, collection_id: str) -> int:
        """Invalidate cached responses for a collection
        
        Args:
            collection_id: Collection to invalidate
            
        Returns:
            Number of entries invalidated
            
        Raises:
            RuntimeError: If caching is not enabled
        """
        if not self.enable_caching or not self._response_cache:
            raise RuntimeError("Caching is not enabled. Initialize client with enable_caching=True")
        
        return self._response_cache.invalidate_collection(collection_id)
    
    def warm_cache(
        self,
        warmup_operations: List[Tuple[str, str, Dict[str, Any], Any]],
        batch_size: Optional[int] = None
    ) -> int:
        """Warm cache with predefined operations
        
        Args:
            warmup_operations: List of (operation, collection_id, params, response) tuples
            batch_size: Batch size for warming
            
        Returns:
            Number of entries warmed
            
        Raises:
            RuntimeError: If caching is not enabled
        """
        if not self.enable_caching or not self._response_cache:
            raise RuntimeError("Caching is not enabled. Initialize client with enable_caching=True")
        
        return self._response_cache.warm_cache(warmup_operations, batch_size)
    
    # Batched operations for improved throughput
    def insert_vectors_batched(
        self,
        collection_id: str,
        vectors: VectorArray,
        ids: List[str],
        metadata: Optional[List[MetadataDict]] = None,
        callback: Optional[callable] = None,
        priority: int = 1
    ) -> str:
        """Submit vectors for batched insertion
        
        Args:
            collection_id: Target collection ID
            vectors: Vector data array
            ids: List of unique vector identifiers
            metadata: Optional list of metadata dictionaries
            callback: Optional callback function for result
            priority: Request priority (higher = more urgent)
            
        Returns:
            Request ID for tracking
            
        Raises:
            RuntimeError: If batching is not enabled
        """
        if not self.enable_batching or not self._batch_processor:
            raise RuntimeError("Batching is not enabled. Initialize client with enable_batching=True")
        
        vectors_list = self._normalize_vectors(vectors)
        
        if len(vectors_list) != len(ids):
            raise ValueError("Number of vectors must match number of IDs")
        
        if metadata and len(metadata) != len(vectors_list):
            raise ValueError("Number of metadata items must match number of vectors")
        
        # Prepare vector data
        vector_data = []
        for i, (vector_id, vector) in enumerate(zip(ids, vectors_list)):
            metadata_items = self._convert_metadata_to_rest_format(
                metadata[i] if metadata else {}
            )
            item = {
                "id": vector_id,
                "vector": vector,
                "metadata": metadata_items,
                "timestamp": int(time.time())
            }
            vector_data.append(item)
        
        return self._batch_processor.submit_request(
            operation="insert_vectors",
            collection_id=collection_id,
            data=vector_data,
            callback=callback,
            priority=priority
        )
    
    def upsert_vectors_batched(
        self,
        collection_id: str,
        vectors: VectorArray,
        ids: List[str],
        metadata: Optional[List[MetadataDict]] = None,
        callback: Optional[callable] = None,
        priority: int = 1
    ) -> str:
        """Submit vectors for batched upsert
        
        Args:
            collection_id: Target collection ID
            vectors: Vector data array
            ids: List of unique vector identifiers
            metadata: Optional list of metadata dictionaries
            callback: Optional callback function for result
            priority: Request priority (higher = more urgent)
            
        Returns:
            Request ID for tracking
        """
        if not self.enable_batching or not self._batch_processor:
            raise RuntimeError("Batching is not enabled. Initialize client with enable_batching=True")
        
        vectors_list = self._normalize_vectors(vectors)
        
        if len(vectors_list) != len(ids):
            raise ValueError("Number of vectors must match number of IDs")
        
        if metadata and len(metadata) != len(vectors_list):
            raise ValueError("Number of metadata items must match number of vectors")
        
        # Prepare vector data
        vector_data = []
        for i, (vector_id, vector) in enumerate(zip(ids, vectors_list)):
            metadata_items = self._convert_metadata_to_rest_format(
                metadata[i] if metadata else {}
            )
            item = {
                "id": vector_id,
                "vector": vector,
                "metadata": metadata_items,
                "timestamp": int(time.time())
            }
            vector_data.append(item)
        
        return self._batch_processor.submit_request(
            operation="upsert_vectors",
            collection_id=collection_id,
            data=vector_data,
            callback=callback,
            priority=priority
        )
    
    def delete_vectors_batched(
        self,
        collection_id: str,
        ids: List[str],
        callback: Optional[callable] = None,
        priority: int = 1
    ) -> str:
        """Submit vector IDs for batched deletion
        
        Args:
            collection_id: Target collection ID
            ids: List of vector IDs to delete
            callback: Optional callback function for result
            priority: Request priority (higher = more urgent)
            
        Returns:
            Request ID for tracking
        """
        if not self.enable_batching or not self._batch_processor:
            raise RuntimeError("Batching is not enabled. Initialize client with enable_batching=True")
        
        return self._batch_processor.submit_request(
            operation="delete_vectors",
            collection_id=collection_id,
            data=ids,
            callback=callback,
            priority=priority
        )
    
    def get_batch_metrics(self) -> Dict[str, Any]:
        """Get batching performance metrics
        
        Returns:
            Dictionary of batch processing metrics
            
        Raises:
            RuntimeError: If batching is not enabled
        """
        if not self.enable_batching or not self._batch_processor:
            raise RuntimeError("Batching is not enabled. Initialize client with enable_batching=True")
        
        return self._batch_processor.get_metrics()
    
    def reset_batch_metrics(self) -> None:
        """Reset batching performance metrics
        
        Raises:
            RuntimeError: If batching is not enabled
        """
        if not self.enable_batching or not self._batch_processor:
            raise RuntimeError("Batching is not enabled. Initialize client with enable_batching=True")
        
        self._batch_processor.reset_metrics()
    
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
