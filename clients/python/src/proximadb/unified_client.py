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
)
from .exceptions import ProximaDBError

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
        **kwargs
    ):
        """
        Initialize ProximaDB client
        
        Args:
            url: ProximaDB server URL
            api_key: API key for authentication  
            protocol: Communication protocol (auto, grpc, rest)
            config: Client configuration object
            **kwargs: Additional configuration parameters
        """
        if config is None:
            config = load_config(url=url, api_key=api_key, **kwargs)
        
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
        from .grpc_client import ProximaDBClient as GrpcClient
        
        # Extract host and port from URL for gRPC
        url = self.config.url
        if url.startswith(('http://', 'https://')):
            url = url.split('://', 1)[1]
        
        # Default gRPC port is 5679
        if ':' not in url:
            url = f"{url}:5679"
        
        return GrpcClient(
            endpoint=url,
            timeout=self.config.timeout
        )
    
    def _create_rest_client(self):
        """Create REST client"""
        from .rest_client import ProximaDBRestClient
        return ProximaDBRestClient(config=self.config)
    
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
            return self._client.insert_vectors(collection_id, records)
    
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
        metadata_filter: Optional[Dict[str, Any]] = None,
        **kwargs
    ) -> List[SearchResult]:
        """Search for similar vectors with a single query"""
        if self._active_protocol == Protocol.GRPC:
            # Convert vector to list if numpy array
            if isinstance(vector, np.ndarray):
                vector = vector.tolist()
            
            proto_response = self._client.search_vectors(
                collection_id=collection_id,
                query_vectors=[vector],
                top_k=top_k,
                metadata_filters=metadata_filter,
                include_metadata=kwargs.get('include_metadata', True),
                include_vectors=kwargs.get('include_vectors', False)
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
            return self._client.search_single(
                collection_id=collection_id,
                vector=vector,
                top_k=top_k,
                metadata_filter=metadata_filter,
                **kwargs
            )
    
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
                return VectorRecord(
                    id=proto_result.id if proto_result.id else "",
                    vector=list(proto_result.vector) if proto_result.vector else [],
                    metadata=dict(proto_result.metadata) if proto_result.metadata else {}
                )
            return None
        else:
            pydantic_result = self._client.get_vector(collection_id, vector_id)
            if pydantic_result:
                return VectorRecord(**pydantic_result)
            return None
    
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