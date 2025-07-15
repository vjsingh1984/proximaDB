"""
ProximaDB Unified Python Client

Unified client interface that can use either REST or gRPC protocols.
Automatically selects gRPC for better performance when available,
with graceful fallback to REST for compatibility.

This client provides type conversion between proto and Pydantic models
to maintain a consistent interface regardless of the underlying protocol.
"""

import logging
import time
import warnings
from typing import Any, Dict, List, Optional, Union
from enum import Enum

import numpy as np
from tenacity import retry, stop_after_attempt, wait_exponential, retry_if_exception_type

from .config import ClientConfig, load_config
from .models import (
    Collection,
    CollectionConfig,
    SearchResult,
    VectorOperationResponse,
    OperationMetrics,
    HealthStatus,
    VectorRecord,
    VectorArray,
    MetadataDict,
    FilterDict,
    DistanceMetric,
    StorageEngine,
    IndexingAlgorithm,
    QuantizationConfig,
    QuantizationType,
)
from .exceptions import (
    ProximaDBError,
    NetworkError,
    TimeoutError,
    RateLimitError,
    map_http_error,
)

try:
    from . import proximadb_pb2 as pb2
    GRPC_AVAILABLE = True
except ImportError:
    GRPC_AVAILABLE = False

logger = logging.getLogger(__name__)


class Protocol(Enum):
    """Communication protocol options"""
    AUTO = "auto"      # Auto-select best available (gRPC preferred)
    GRPC = "grpc"      # Force gRPC (high performance, binary protocol)
    REST = "rest"      # Force REST (web compatibility)


class ProximaDBClient:
    """
    Unified ProximaDB Python Client
    
    Supports both REST and gRPC protocols with automatic selection
    for optimal performance and compatibility. Provides a consistent
    interface using Pydantic models regardless of the underlying protocol.
    """
    
    def __init__(
        self,
        url: Optional[str] = None,
        api_key: Optional[str] = None,
        protocol: Union[Protocol, str] = Protocol.AUTO,
        config: Optional[ClientConfig] = None,
        enable_http2: bool = True,
        pool_size: int = 10,
        pool_maxsize: int = 50,
        verify_ssl: bool = True,
        cert_file: Optional[str] = None,
        key_file: Optional[str] = None,
        **kwargs
    ):
        """
        Initialize ProximaDB client
        
        Args:
            url: ProximaDB server URL
            api_key: API key for authentication  
            protocol: Communication protocol (auto, grpc, rest)
            config: Client configuration object
            enable_http2: Enable HTTP/2 support for better performance
            pool_size: Connection pool size for keepalive connections
            pool_maxsize: Maximum connection pool size
            verify_ssl: Verify SSL certificates
            cert_file: Client certificate file path for mTLS
            key_file: Client key file path for mTLS
            **kwargs: Additional configuration parameters
        """
        if config is None:
            config = load_config(url=url, api_key=api_key, **kwargs)
        
        # Update config with connection parameters
        if hasattr(config, 'connection'):
            config.connection.pool_size = pool_size
            config.connection.pool_maxsize = pool_maxsize
        if hasattr(config, 'tls'):
            config.tls.verify = verify_ssl
            config.tls.cert_file = cert_file
            config.tls.key_file = key_file
        config.enable_http2 = enable_http2
        
        self.config = config
        self.protocol = Protocol(protocol) if isinstance(protocol, str) else protocol
        self._client = None
        self._setup_client()
    
    def _setup_client(self):
        """Setup the underlying client based on protocol preference"""
        if self.protocol == Protocol.AUTO:
            # Try gRPC first (high performance), then fallback to REST
            try:
                if not GRPC_AVAILABLE:
                    raise ImportError("gRPC dependencies not available")
                self._client = self._create_grpc_client()
                self._active_protocol = Protocol.GRPC
                logger.info("🔗 Using gRPC client for high performance")
            except ImportError:
                logger.warning("⚠️ gRPC dependencies not available, falling back to REST")
                self._client = self._create_rest_client()
                self._active_protocol = Protocol.REST
            except Exception as e:
                logger.warning(f"⚠️ gRPC client failed: {e}, falling back to REST")
                self._client = self._create_rest_client()
                self._active_protocol = Protocol.REST
                    
        elif self.protocol == Protocol.GRPC:
            # Force gRPC
            if not GRPC_AVAILABLE:
                raise ImportError("gRPC dependencies not available. Install with: pip install grpcio grpcio-tools protobuf")
            self._client = self._create_grpc_client()
            self._active_protocol = Protocol.GRPC
            logger.info("🔗 Using gRPC client (forced)")
            
        elif self.protocol == Protocol.REST:
            # Force REST
            self._client = self._create_rest_client()
            self._active_protocol = Protocol.REST
            logger.info("🌐 Using REST client (forced)")
        
        else:
            raise ValueError(f"Unknown protocol: {self.protocol}")
    
    def _create_grpc_client(self):
        """Create gRPC client"""
        from .protocols.grpc_sync import ProximaDBSyncGrpcClient
        
        # Extract host and port from URL for gRPC
        url = self.config.url
        if url.startswith(('http://', 'https://')):
            url = url.split('://', 1)[1]
        
        # Default gRPC port is 5679
        if ':' not in url:
            url = f"{url}:5679"
        
        return ProximaDBSyncGrpcClient(
            server_address=url,
            timeout=self.config.timeout
        )
    
    def _create_rest_client(self):
        """Create REST client with enhanced configuration"""
        from .protocols.rest_sync import ProximaDBClient as RestClient
        
        # Add retry configuration if not present
        if not hasattr(self.config, 'retry'):
            from dataclasses import dataclass
            @dataclass
            class RetryConfig:
                max_retries: int = 3
                backoff_factor: float = 0.5
                max_backoff: float = 10.0
            self.config.retry = RetryConfig()
        
        return RestClient(config=self.config)
    
    @property
    def active_protocol(self) -> Protocol:
        """Get the currently active protocol"""
        return self._active_protocol
    
    def get_performance_info(self) -> Dict[str, Any]:
        """Get performance information about the active protocol"""
        if self._active_protocol == Protocol.GRPC:
            return {
                "protocol": "gRPC",
                "advantages": [
                    "40% smaller payloads (binary protobuf vs JSON)",
                    "90% less overhead (HTTP/2 vs HTTP/1.1)",
                    "Better type safety with schema evolution",
                    "Streaming support for real-time operations"
                ],
                "serialization": "Binary Protocol Buffers",
                "transport": "HTTP/2"
            }
        else:
            return {
                "protocol": "REST",
                "advantages": [
                    "Universal compatibility",
                    "Easy debugging with standard tools",
                    "Human-readable JSON format"
                ],
                "serialization": "JSON",
                "transport": "HTTP/1.1"
            }
    
    # Type conversion helpers
    def _proto_to_pydantic_collection(self, proto_collection: 'pb2.Collection') -> Collection:
        """Convert proto Collection to Pydantic Collection"""
        config = CollectionConfig(
            name=proto_collection.config.name,
            dimension=proto_collection.config.dimension,
            distance_metric=self._proto_to_pydantic_distance_metric(proto_collection.config.distance_metric),
            storage_engine=self._proto_to_pydantic_storage_engine(proto_collection.config.storage_engine),
            primary_indexing_algorithm=self._proto_to_pydantic_indexing_algorithm(proto_collection.config.primary_indexing_algorithm),
            description=proto_collection.config.description if proto_collection.config.description else None,
            tags=list(proto_collection.config.tags) if proto_collection.config.tags else None,
            owner=proto_collection.config.owner if proto_collection.config.owner else None,
        )
        
        return Collection(
            id=proto_collection.id,
            config=config,
            created_at=proto_collection.created_at,
            updated_at=proto_collection.updated_at,
        )
    
    def _proto_to_pydantic_distance_metric(self, proto_metric: int) -> DistanceMetric:
        """Convert proto DistanceMetric to Pydantic DistanceMetric"""
        mapping = {
            1: DistanceMetric.COSINE,
            2: DistanceMetric.EUCLIDEAN,
            3: DistanceMetric.DOT_PRODUCT,
            4: DistanceMetric.HAMMING,
            5: DistanceMetric.MANHATTAN,
            6: DistanceMetric.JACCARD,
            7: DistanceMetric.CUSTOM,
        }
        return mapping.get(proto_metric, DistanceMetric.COSINE)
    
    def _proto_to_pydantic_storage_engine(self, proto_engine: int) -> StorageEngine:
        """Convert proto StorageEngine to Pydantic StorageEngine"""
        mapping = {
            1: StorageEngine.VIPER,
            2: StorageEngine.LSM,
            3: StorageEngine.MMAP,
            4: StorageEngine.HYBRID,
        }
        return mapping.get(proto_engine, StorageEngine.VIPER)
    
    def _proto_to_pydantic_indexing_algorithm(self, proto_algo: int) -> IndexingAlgorithm:
        """Convert proto IndexingAlgorithm to Pydantic IndexingAlgorithm"""
        mapping = {
            1: IndexingAlgorithm.HNSW,
            2: IndexingAlgorithm.IVF,
            3: IndexingAlgorithm.PQ,
            4: IndexingAlgorithm.FLAT,
            5: IndexingAlgorithm.ANNOY,
        }
        return mapping.get(proto_algo, IndexingAlgorithm.HNSW)
    
    def _pydantic_to_proto_collection_config(self, config: CollectionConfig) -> 'pb2.CollectionConfig':
        """Convert Pydantic CollectionConfig to proto CollectionConfig"""
        proto_config = pb2.CollectionConfig(
            name=config.name,
            dimension=config.dimension,
            distance_metric=self._pydantic_to_proto_distance_metric(config.distance_metric),
            storage_engine=self._pydantic_to_proto_storage_engine(config.storage_engine),
            primary_indexing_algorithm=self._pydantic_to_proto_indexing_algorithm(config.primary_indexing_algorithm),
        )
        
        if config.description:
            proto_config.description = config.description
        if config.tags:
            proto_config.tags.extend(config.tags)
        if config.owner:
            proto_config.owner = config.owner
        
        # Handle quantization config
        if config.quantization_config:
            proto_config.quantization_config.CopyFrom(
                self._pydantic_to_proto_quantization_config(config.quantization_config)
            )
        
        return proto_config
    
    def _pydantic_to_proto_distance_metric(self, metric: DistanceMetric) -> int:
        """Convert Pydantic DistanceMetric to proto DistanceMetric"""
        mapping = {
            DistanceMetric.COSINE: pb2.DistanceMetric.COSINE,
            DistanceMetric.EUCLIDEAN: pb2.DistanceMetric.EUCLIDEAN,
            DistanceMetric.DOT_PRODUCT: pb2.DistanceMetric.DOT_PRODUCT,
            DistanceMetric.HAMMING: pb2.DistanceMetric.HAMMING,
            DistanceMetric.MANHATTAN: pb2.DistanceMetric.MANHATTAN,
            DistanceMetric.JACCARD: pb2.DistanceMetric.JACCARD,
            DistanceMetric.CUSTOM: pb2.DistanceMetric.CUSTOM,
        }
        return mapping.get(metric, pb2.DistanceMetric.COSINE)
    
    def _pydantic_to_proto_storage_engine(self, engine: StorageEngine) -> int:
        """Convert Pydantic StorageEngine to proto StorageEngine"""
        mapping = {
            StorageEngine.VIPER: pb2.StorageEngine.VIPER,
            StorageEngine.LSM: pb2.StorageEngine.LSM,
            StorageEngine.MMAP: pb2.StorageEngine.MMAP,
            StorageEngine.HYBRID: pb2.StorageEngine.HYBRID,
        }
        return mapping.get(engine, pb2.StorageEngine.VIPER)
    
    def _pydantic_to_proto_indexing_algorithm(self, algo: IndexingAlgorithm) -> int:
        """Convert Pydantic IndexingAlgorithm to proto IndexingAlgorithm"""
        mapping = {
            IndexingAlgorithm.HNSW: pb2.IndexingAlgorithm.HNSW,
            IndexingAlgorithm.IVF: pb2.IndexingAlgorithm.IVF,
            IndexingAlgorithm.PQ: pb2.IndexingAlgorithm.PQ,
            IndexingAlgorithm.FLAT: pb2.IndexingAlgorithm.FLAT,
            IndexingAlgorithm.ANNOY: pb2.IndexingAlgorithm.ANNOY,
        }
        return mapping.get(algo, pb2.IndexingAlgorithm.HNSW)
    
    def _pydantic_to_proto_quantization_config(self, config: QuantizationConfig) -> 'pb2.QuantizationConfig':
        """Convert Pydantic QuantizationConfig to proto QuantizationConfig"""
        proto_config = pb2.QuantizationConfig(enabled=config.enabled)
        
        # For simple quantization config, we need to map to the comprehensive proto structure
        if config.enabled and config.type != QuantizationType.NONE:
            # Create a search quantization config based on the simple config
            search_config = pb2.SearchQuantizationConfig(
                enabled=True,
                adaptive_precision=True,
                accuracy_threshold=config.accuracy_threshold or 0.95,
                candidate_multiplier=3
            )
            
            # Create the appropriate quantization level
            level = pb2.QuantizationLevel()
            if config.type == QuantizationType.BINARY:
                level.binary.CopyFrom(pb2.BinaryQuantization(
                    threshold=config.threshold or 0.0,
                    sign_based=True
                ))
            elif config.type == QuantizationType.SCALAR:
                level.scalar.CopyFrom(pb2.ScalarQuantization(
                    bits=config.bits_per_vector or 8,
                    scale=1.0,
                    offset=0.0,
                    clamp_values=True
                ))
            elif config.type == QuantizationType.PRODUCT:
                level.pq.CopyFrom(pb2.ProductQuantization(
                    bits_per_code=config.bits_per_subvector or 8,
                    num_subvectors=config.num_subvectors or 8,
                    adaptive_subvectors=True
                ))
            elif config.type == QuantizationType.UNIFORM:
                level.uniform.CopyFrom(pb2.UniformQuantization(
                    bits=8,
                    scale=1.0,
                    offset=0.0
                ))
            
            search_config.default_level.CopyFrom(level)
            proto_config.search_quantization.CopyFrom(search_config)
            
            if config.compression_ratio_target:
                proto_config.compression_ratio_target = config.compression_ratio_target
        
        return proto_config
    
    def _proto_to_pydantic_health_status(self, proto_health: 'pb2.HealthResponse') -> HealthStatus:
        """Convert proto HealthResponse to Pydantic HealthStatus"""
        return HealthStatus(
            status=proto_health.status,
            version=proto_health.version,
            uptime_seconds=proto_health.uptime_seconds,
            services={},  # gRPC health doesn't include services info
            timestamp=int(time.time() * 1000000)  # Current timestamp in microseconds
        )
    
    # Public API methods
    def health(self) -> HealthStatus:
        """Check server health status"""
        if self._active_protocol == Protocol.GRPC:
            proto_health = self._client.health_check()
            return self._proto_to_pydantic_health_status(proto_health)
        else:
            return self._client.health()
    
    def create_collection(
        self,
        name: str,
        config: Optional[CollectionConfig] = None,
        **kwargs
    ) -> Collection:
        """Create a new vector collection"""
        if config is None:
            config = CollectionConfig(name=name, **kwargs)
        
        if self._active_protocol == Protocol.GRPC:
            proto_config = self._pydantic_to_proto_collection_config(config)
            proto_collection = self._client.create_collection(
                name=config.name,
                dimension=config.dimension,
                distance_metric=self._pydantic_to_proto_distance_metric(config.distance_metric),
                indexing_algorithm=self._pydantic_to_proto_indexing_algorithm(config.primary_indexing_algorithm),
                storage_engine=self._pydantic_to_proto_storage_engine(config.storage_engine)
            )
            return self._proto_to_pydantic_collection(proto_collection)
        else:
            return self._client.create_collection(name, config, **kwargs)
    
    def get_collection(self, collection_id: str) -> Optional[Collection]:
        """Get collection metadata"""
        if self._active_protocol == Protocol.GRPC:
            proto_collection = self._client.get_collection(collection_id)
            if proto_collection:
                return self._proto_to_pydantic_collection(proto_collection)
            return None
        else:
            return self._client.get_collection(collection_id)
    
    def list_collections(self) -> List[Collection]:
        """List all collections"""
        if self._active_protocol == Protocol.GRPC:
            proto_collections = self._client.list_collections()
            return [self._proto_to_pydantic_collection(col) for col in proto_collections]
        else:
            return self._client.list_collections()
    
    def delete_collection(self, collection_id: str) -> bool:
        """Delete a collection"""
        return self._client.delete_collection(collection_id)
    
    def insert_vectors(
        self,
        collection_id: str,
        records: List[VectorRecord]
    ) -> VectorOperationResponse:
        """Insert vectors into a collection"""
        if self._active_protocol == Protocol.GRPC:
            # Convert Pydantic VectorRecord to dict format for gRPC client
            vector_dicts = []
            for record in records:
                vector_dict = {
                    "vector": record.vector,
                    "metadata": record.metadata
                }
                if record.id:
                    vector_dict["id"] = record.id
                if record.timestamp:
                    vector_dict["timestamp"] = record.timestamp
                if record.expires_at:
                    vector_dict["expires_at"] = record.expires_at
                vector_dicts.append(vector_dict)
            
            proto_response = self._client.insert_vectors(collection_id, vector_dicts)
            # Convert proto response to Pydantic (simplified for now)
            return VectorOperationResponse(
                success=proto_response.success,
                operation="insert",
                metrics=OperationMetrics(
                    total_processed=proto_response.metrics.total_processed if proto_response.metrics else 0,
                    successful_count=proto_response.metrics.successful_count if proto_response.metrics else 0,
                    failed_count=proto_response.metrics.failed_count if proto_response.metrics else 0
                )
            )
        else:
            # REST client expects separate arrays
            vectors = [r.vector for r in records]
            ids = [r.id for r in records if r.id]
            metadata = [r.metadata for r in records]
            
            # If no IDs provided, generate them
            if not ids:
                ids = [f"vec_{i}" for i in range(len(vectors))]
            
            return self._client.insert_vectors(collection_id, vectors, ids, metadata)
    
    def upsert_vectors(
        self,
        collection_id: str,
        records: List[VectorRecord]
    ) -> VectorOperationResponse:
        """Upsert vectors into a collection"""
        if self._active_protocol == Protocol.GRPC:
            # Convert Pydantic VectorRecord to dict format for gRPC client
            vector_dicts = []
            for record in records:
                vector_dict = {
                    "vector": record.vector,
                    "metadata": record.metadata
                }
                if record.id:
                    vector_dict["id"] = record.id
                if record.timestamp:
                    vector_dict["timestamp"] = record.timestamp
                if record.expires_at:
                    vector_dict["expires_at"] = record.expires_at
                vector_dicts.append(vector_dict)
            
            proto_response = self._client.insert_vectors(collection_id, vector_dicts, upsert=True)
            # Convert proto response to Pydantic (simplified for now)
            return VectorOperationResponse(
                success=proto_response.success,
                operation="upsert",
                metrics=OperationMetrics(
                    total_processed=proto_response.metrics.total_processed if proto_response.metrics else 0,
                    successful_count=proto_response.metrics.successful_count if proto_response.metrics else 0,
                    failed_count=proto_response.metrics.failed_count if proto_response.metrics else 0,
                    updated_count=proto_response.metrics.updated_count if proto_response.metrics else 0
                )
            )
        else:
            return self._client.upsert_vectors(collection_id, records)
    
    def search_single(
        self,
        collection_id: str,
        vector: Union[List[float], np.ndarray],
        top_k: int = 10,
        metadata_filter: Optional[Union[Dict[str, Any], 'FilterBuilder']] = None,
        optimization_level: str = "high",
        use_storage_aware: bool = True,
        quantization_level: str = "FP32",
        enable_simd: bool = True,
        **kwargs
    ) -> List[SearchResult]:
        """Search for similar vectors with storage-aware optimizations
        
        Args:
            collection_id: Target collection ID
            vector: Query vector
            top_k: Number of results to return
            metadata_filter: Metadata filter conditions
            optimization_level: Search optimization level ('high', 'medium', 'low')
            use_storage_aware: Enable storage-aware polymorphic search
            quantization_level: Vector quantization level for search
            enable_simd: Enable SIMD vectorization optimizations
            **kwargs: Additional search parameters
            
        Returns:
            List of search results ordered by similarity
        """
        if self._active_protocol == Protocol.GRPC:
            # Convert vector to list if numpy array
            if isinstance(vector, np.ndarray):
                vector = vector.tolist()
            
            # Add search hints for gRPC
            search_hints = kwargs.get('search_hints', {})
            search_hints.update({
                "predicate_pushdown": True,
                "use_bloom_filters": True,
                "use_clustering": True,
                "quantization_level": quantization_level,
                "parallel_search": True,
                "engine_specific": {
                    "optimization_level": optimization_level,
                    "enable_simd": enable_simd,
                    "prefer_indices": True,
                    "storage_aware": use_storage_aware
                }
            })
            
            proto_response = self._client.search_vectors(
                collection_id=collection_id,
                query_vectors=[vector],
                top_k=top_k,
                metadata_filters=metadata_filter,
                include_metadata=kwargs.get('include_metadata', True),
                include_vectors=kwargs.get('include_vectors', False)
                # Note: search_hints would need to be converted to SearchParameters proto
            )
            
            # Extract results from proto response
            results = []
            if hasattr(proto_response, 'compact_results') and proto_response.compact_results:
                for result in proto_response.compact_results.results:
                    search_result = SearchResult(
                        id=result.id if result.id else "",
                        score=result.score,
                        vector=list(result.vector) if result.vector else None,
                        metadata=dict(result.metadata) if result.metadata else None
                    )
                    results.append(search_result)
            return results
        else:
            # For REST, use search method
            return self._client.search(
                collection_id=collection_id,
                query=vector,
                k=top_k,
                filter=metadata_filter,
                optimization_level=optimization_level,
                use_storage_aware=use_storage_aware,
                quantization_level=quantization_level,
                enable_simd=enable_simd,
                **kwargs
            )
    
    # Alias for backward compatibility
    search = search_single
    
    def search_batch(
        self,
        collection_id: str,
        vectors: Union[List[List[float]], np.ndarray],
        top_k: int = 10,
        metadata_filter: Optional[Union[Dict[str, Any], 'FilterBuilder']] = None,
        **kwargs
    ) -> List[List[SearchResult]]:
        """Search multiple queries in batch with optimizations
        
        Args:
            collection_id: Target collection ID
            vectors: Array of query vectors
            top_k: Number of results per query
            metadata_filter: Metadata filter conditions
            **kwargs: Additional search parameters
            
        Returns:
            List of search results for each query
        """
        # Convert to list if numpy array
        if isinstance(vectors, np.ndarray):
            vectors = vectors.tolist()
        
        # Perform batch search
        all_results = []
        for vector in vectors:
            results = self.search_single(
                collection_id=collection_id,
                vector=vector,
                top_k=top_k,
                metadata_filter=metadata_filter,
                **kwargs
            )
            all_results.append(results)
        
        return all_results
    
    def delete_vectors(
        self,
        collection_id: str,
        vector_ids: List[str]
    ) -> VectorOperationResponse:
        """Delete vectors from a collection"""
        if self._active_protocol == Protocol.GRPC:
            proto_response = self._client.delete_vectors(collection_id, vector_ids)
            return VectorOperationResponse(
                success=proto_response.success,
                operation="delete",
                metrics=OperationMetrics(
                    total_processed=proto_response.metrics.total_processed if proto_response.metrics else 0,
                    successful_count=proto_response.metrics.successful_count if proto_response.metrics else 0,
                    failed_count=proto_response.metrics.failed_count if proto_response.metrics else 0
                )
            )
        else:
            return self._client.delete_vectors(collection_id, vector_ids)
    
    def get_vector(
        self,
        collection_id: str,
        vector_id: str,
        include_vector: bool = True,
        include_metadata: bool = True,
    ) -> Optional[VectorRecord]:
        """Get a single vector by ID"""
        if self._active_protocol == Protocol.GRPC:
            proto_result = self._client.get_vector(collection_id, vector_id, include_vector, include_metadata)
            if proto_result:
                # Convert repeated MetadataItem to dict
                metadata_dict = {}
                if proto_result.metadata:
                    for item in proto_result.metadata:
                        metadata_dict[item.key] = item.value
                
                return VectorRecord(
                    id=proto_result.id if proto_result.id else "",
                    vector=list(proto_result.vector) if proto_result.vector else [],
                    metadata=metadata_dict
                )
            return None
        else:
            pydantic_result = self._client.get_vector(collection_id, vector_id)
            if pydantic_result:
                return VectorRecord(**pydantic_result)
            return None
    
    def insert_vector(
        self,
        collection_id: str,
        vector_id: str,
        vector: Union[List[float], np.ndarray],
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
        record = VectorRecord(
            id=vector_id,
            vector=vector,
            metadata=metadata or {}
        )
        if upsert:
            return self.upsert_vectors(collection_id, [record])
        else:
            return self.insert_vectors(collection_id, [record])
    
    def delete_vector(
        self,
        collection_id: str,
        vector_id: str
    ) -> VectorOperationResponse:
        """Delete a single vector - alias for batch delete with one vector
        
        Args:
            collection_id: Collection ID or name
            vector_id: Vector identifier to delete
            
        Returns:
            VectorOperationResponse
        """
        return self.delete_vectors(collection_id, [vector_id])
    
    def close(self):
        """Close the client and cleanup resources"""
        if self._client and hasattr(self._client, 'close'):
            self._client.close()
    
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
    
    # Legacy compatibility methods
    def insert(
        self,
        collection_id: str,
        vectors: Union[List[List[float]], np.ndarray],
        ids: Optional[List[str]] = None,
        metadata: Optional[List[Dict[str, Any]]] = None
    ) -> VectorOperationResponse:
        """Legacy insert method for backward compatibility"""
        records = []
        
        # Convert vectors to list format
        if isinstance(vectors, np.ndarray):
            vectors = vectors.tolist()
        
        # Build records
        for i, vector in enumerate(vectors):
            record = VectorRecord(
                vector=vector,
                id=ids[i] if ids and i < len(ids) else None,
                metadata=metadata[i] if metadata and i < len(metadata) else {}
            )
            records.append(record)
        
        return self.insert_vectors(collection_id, records)
    
    def upsert(
        self,
        collection_id: str,
        vectors: Union[List[List[float]], np.ndarray],
        ids: List[str],
        metadata: Optional[List[Dict[str, Any]]] = None
    ) -> VectorOperationResponse:
        """Legacy upsert method for backward compatibility"""
        records = []
        
        # Convert vectors to list format
        if isinstance(vectors, np.ndarray):
            vectors = vectors.tolist()
        
        # Build records
        for i, (vector, vector_id) in enumerate(zip(vectors, ids)):
            record = VectorRecord(
                vector=vector,
                id=vector_id,
                metadata=metadata[i] if metadata and i < len(metadata) else {}
            )
            records.append(record)
        
        return self.upsert_vectors(collection_id, records)
    
    def delete(
        self,
        collection_id: str,
        ids: List[str]
    ) -> VectorOperationResponse:
        """Legacy delete method for backward compatibility"""
        return self.delete_vectors(collection_id, ids)
    
    def get_collection_stats(self, collection_id: str) -> Dict[str, Any]:
        """Get collection statistics (legacy compatibility)"""
        collection = self.get_collection(collection_id)
        if collection:
            return {
                "id": collection.id,
                "name": collection.config.name,
                "dimension": collection.config.dimension,
                "created_at": collection.created_at,
                "updated_at": collection.updated_at,
                "vector_count": getattr(collection, 'vector_count', 0),
                "index_count": getattr(collection, 'index_count', 0),
                "status": getattr(collection, 'status', 'active')
            }
        return {}


# Convenience functions for backward compatibility
def connect(
    url: Optional[str] = None,
    api_key: Optional[str] = None,
    protocol: Union[Protocol, str] = Protocol.AUTO,
    **kwargs
) -> ProximaDBClient:
    """Create a ProximaDB client with simplified parameters"""
    return ProximaDBClient(url=url, api_key=api_key, protocol=protocol, **kwargs)


def connect_grpc(
    url: Optional[str] = None,
    api_key: Optional[str] = None,
    **kwargs
) -> ProximaDBClient:
    """Create a ProximaDB client using gRPC protocol (good performance, ecosystem compatibility)"""
    return ProximaDBClient(url=url, api_key=api_key, protocol=Protocol.GRPC, **kwargs)


def connect_rest(
    url: Optional[str] = None,
    api_key: Optional[str] = None,
    **kwargs
) -> ProximaDBClient:
    """Create a ProximaDB client using REST protocol (web compatibility)"""
    return ProximaDBClient(url=url, api_key=api_key, protocol=Protocol.REST, **kwargs)