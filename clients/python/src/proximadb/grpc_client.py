"""
ProximaDB gRPC Client - Updated for Current Proto Schema

This client uses the current ProximaDB gRPC service with unified endpoints.
Aligned with proximadb.proto v1 schema.

Copyright 2025 ProximaDB
"""

import logging
import time
from typing import Optional, Dict, List, Any
import json
from datetime import datetime, timezone

import grpc

from .proximadb_pb2 import *
from . import proximadb_pb2 as pb2
from . import proximadb_pb2_grpc as pb2_grpc
from .exceptions import ProximaDBError
from .models import (
    Collection, 
    SearchResult, 
    InsertResult, 
    BatchResult,
    DeleteResult,
    HealthStatus
)

logger = logging.getLogger(__name__)


class ProximaDBClient:
    """
    ProximaDB gRPC client using the current unified protobuf schema.
    
    Uses the ProximaDB service with CollectionOperation unified endpoint.
    """
    
    def __init__(
        self,
        endpoint: str = "localhost:5679",
        timeout: float = 30.0,
        enable_debug_logging: bool = False,
        use_tls: bool = False,
        compression: Optional[grpc.Compression] = None
    ):
        """
        Initialize gRPC client
        
        Args:
            endpoint: gRPC server endpoint (host:port)
            timeout: Request timeout in seconds
            enable_debug_logging: Enable debug logging
            use_tls: Use TLS/SSL connection
            compression: gRPC compression algorithm
        """
        self.endpoint = endpoint
        self.timeout = timeout
        self.use_tls = use_tls
        self.compression = compression
        
        if enable_debug_logging:
            logging.basicConfig(level=logging.DEBUG)
        
        # Initialize gRPC channel and stub
        self.channel = None
        self.stub = None
        self._connect()
    
    def _connect(self):
        """Establish gRPC connection"""
        try:
            if self.use_tls:
                credentials = grpc.ssl_channel_credentials()
                self.channel = grpc.secure_channel(self.endpoint, credentials)
            else:
                self.channel = grpc.insecure_channel(self.endpoint)
            
            self.stub = pb2_grpc.ProximaDBStub(self.channel)
            logger.info(f"Connected to ProximaDB gRPC service at {self.endpoint}")
            
        except Exception as e:
            raise ProximaDBError(f"Failed to connect to gRPC server: {e}")
    
    async def connect(self):
        """Async connect - for compatibility with existing tests"""
        self._connect()
    
    async def close(self):
        """Close the gRPC connection"""
        if self.channel:
            self.channel.close()
            logger.info("gRPC connection closed")
    
    def _call_with_timeout(self, method, request, timeout=None):
        """Make gRPC call with timeout and error handling"""
        try:
            timeout = timeout or self.timeout
            return method(request, timeout=timeout, compression=self.compression)
        except grpc.RpcError as e:
            raise ProximaDBError(f"gRPC error: {e.code()}: {e.details()}")
        except Exception as e:
            raise ProximaDBError(f"Unexpected error: {e}")
    
    # Collection Operations
    
    async def create_collection(
        self,
        name: str,
        dimension: int,
        distance_metric: int = 1,  # COSINE
        indexing_algorithm: int = 1,  # HNSW
        storage_engine: int = 1,  # VIPER
        filterable_metadata_fields: List[str] = None,
        indexing_config: Dict[str, Any] = None
    ) -> Collection:
        """Create a new collection"""
        
        # Build CollectionConfig
        config = pb2.CollectionConfig(
            name=name,
            dimension=dimension,
            distance_metric=distance_metric,
            indexing_algorithm=indexing_algorithm,
            storage_engine=storage_engine,
            filterable_metadata_fields=filterable_metadata_fields or [],
            indexing_config=indexing_config or {}
        )
        
        # Build CollectionRequest
        request = pb2.CollectionRequest(
            operation=pb2.COLLECTION_CREATE,
            collection_config=config
        )
        
        response = self._call_with_timeout(self.stub.CollectionOperation, request)
        
        if not response.success:
            raise ProximaDBError(f"Failed to create collection: {response.error_message}")
        
        if response.collection:
            return self._convert_collection(response.collection)
        else:
            raise ProximaDBError("No collection returned in response")
    
    async def get_collection(self, name: str) -> Optional[Collection]:
        """Get collection by name"""
        
        request = pb2.CollectionRequest(
            operation=pb2.COLLECTION_GET,
            collection_id=name
        )
        
        response = self._call_with_timeout(self.stub.CollectionOperation, request)
        
        if not response.success:
            if "not found" in response.error_message.lower():
                return None
            raise ProximaDBError(f"Failed to get collection: {response.error_message}")
        
        if response.collection:
            return self._convert_collection(response.collection)
        return None
    
    async def list_collections(self) -> List[Collection]:
        """List all collections"""
        
        request = pb2.CollectionRequest(
            operation=pb2.COLLECTION_LIST
        )
        
        response = self._call_with_timeout(self.stub.CollectionOperation, request)
        
        if not response.success:
            raise ProximaDBError(f"Failed to list collections: {response.error_message}")
        
        return [self._convert_collection(col) for col in response.collections]
    
    async def delete_collection(self, name: str) -> bool:
        """Delete collection by name"""
        
        request = pb2.CollectionRequest(
            operation=pb2.COLLECTION_DELETE,
            collection_id=name
        )
        
        response = self._call_with_timeout(self.stub.CollectionOperation, request)
        
        if not response.success:
            if "not found" in response.error_message.lower():
                return False
            raise ProximaDBError(f"Failed to delete collection: {response.error_message}")
        
        return True
    
    async def update_collection(self, name: str, config: Dict[str, Any]) -> Collection:
        """Update collection configuration"""
        
        # Build updated CollectionConfig
        collection_config = pb2.CollectionConfig(
            name=name,
            dimension=config.get('dimension', 128),
            distance_metric=config.get('distance_metric', 1),
            indexing_algorithm=config.get('indexing_algorithm', 1),
            storage_engine=config.get('storage_engine', 1),
            filterable_metadata_fields=config.get('filterable_metadata_fields', []),
            indexing_config=config.get('indexing_config', {})
        )
        
        request = pb2.CollectionRequest(
            operation=pb2.COLLECTION_UPDATE,
            collection_id=name,
            collection_config=collection_config
        )
        
        response = self._call_with_timeout(self.stub.CollectionOperation, request)
        
        if not response.success:
            raise ProximaDBError(f"Failed to update collection: {response.error_message}")
        
        if response.collection:
            return self._convert_collection(response.collection)
        else:
            raise ProximaDBError("No collection returned in response")
    
    # Health Check
    
    async def health_check(self) -> HealthStatus:
        """Check server health"""
        
        request = pb2.HealthRequest()
        
        response = self._call_with_timeout(self.stub.Health, request)
        
        return HealthStatus(
            status=response.status,
            version=response.version,
            uptime_seconds=response.uptime_seconds,
            message=response.message
        )
    
    # Helper methods
    
    def _convert_collection(self, pb_collection: pb2.Collection) -> Collection:
        """Convert protobuf Collection to Python model"""
        # Handle cases where config might be None
        if pb_collection.config:
            return Collection(
                id=pb_collection.id,
                name=pb_collection.config.name,
                dimension=pb_collection.config.dimension,
                distance_metric=self._distance_metric_to_string(pb_collection.config.distance_metric),
                indexing_algorithm=self._indexing_algorithm_to_string(pb_collection.config.indexing_algorithm),
                storage_engine=self._storage_engine_to_string(pb_collection.config.storage_engine),
                vector_count=pb_collection.stats.vector_count if pb_collection.stats else 0,
                created_at=pb_collection.created_at,
                updated_at=pb_collection.updated_at,
                status='active',
                metric=self._distance_metric_to_string(pb_collection.config.distance_metric),
                index_type=self._indexing_algorithm_to_string(pb_collection.config.indexing_algorithm)
            )
        else:
            # Fallback when config is None
            return Collection(
                id=pb_collection.id,
                name="unknown",  # We don't have the name without config
                dimension=0,
                distance_metric="UNKNOWN",
                indexing_algorithm="UNKNOWN",
                storage_engine="UNKNOWN",
                vector_count=pb_collection.stats.vector_count if pb_collection.stats else 0,
                created_at=pb_collection.created_at,
                updated_at=pb_collection.updated_at,
                status='active'
            )
    
    def _distance_metric_to_string(self, metric: int) -> str:
        """Convert distance metric enum to string"""
        mapping = {
            1: "COSINE",
            2: "EUCLIDEAN", 
            3: "DOT_PRODUCT",
            4: "HAMMING"
        }
        return mapping.get(metric, "COSINE")
    
    def _indexing_algorithm_to_string(self, algo: int) -> str:
        """Convert indexing algorithm enum to string"""
        mapping = {
            1: "HNSW",
            2: "IVF",
            3: "PQ",
            4: "FLAT",
            5: "ANNOY"
        }
        return mapping.get(algo, "HNSW")
    
    def _storage_engine_to_string(self, engine: int) -> str:
        """Convert storage engine enum to string"""
        mapping = {
            1: "VIPER",
            2: "LSM",
            3: "MMAP",
            4: "HYBRID"
        }
        return mapping.get(engine, "VIPER")


# For backward compatibility
class ProximaDBGrpcClient(ProximaDBClient):
    """Backward compatibility alias"""
    pass