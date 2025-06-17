"""
ProximaDB Real gRPC Client using HTTP/2 protocol

This is the proper gRPC implementation using actual gRPC protocol over HTTP/2.
"""

import asyncio
import logging
from typing import Any, Dict, List, Optional, Union

import grpc
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
from . import proximadb_pb2
from . import proximadb_pb2_grpc

logger = logging.getLogger(__name__)


class ProximaDBGrpcClient:
    """
    Real gRPC client for ProximaDB using HTTP/2 protocol
    
    This client uses actual gRPC protocol for efficient communication.
    """
    
    def __init__(
        self,
        endpoint: str = "localhost:5678",
        api_key: Optional[str] = None,
        timeout: float = 30.0,
        enable_debug_logging: bool = False,
        use_tls: bool = False,
        compression: Optional[grpc.Compression] = grpc.Compression.Gzip
    ):
        """
        Initialize real gRPC client
        
        Args:
            endpoint: gRPC server endpoint (host:port)
            api_key: API key for authentication
            timeout: Request timeout in seconds
            enable_debug_logging: Enable debug logging
            use_tls: Use TLS/SSL connection
            compression: gRPC compression algorithm
        """
        self.endpoint = endpoint
        self.api_key = api_key
        self.timeout = timeout
        self.enable_debug_logging = enable_debug_logging
        self.use_tls = use_tls
        self.compression = compression
        
        if enable_debug_logging:
            logging.getLogger().setLevel(logging.DEBUG)
        
        # Create gRPC channel
        if use_tls:
            credentials = grpc.ssl_channel_credentials()
            self.channel = grpc.secure_channel(endpoint, credentials)
        else:
            self.channel = grpc.insecure_channel(endpoint)
        
        # Create stub
        self.stub = proximadb_pb2_grpc.ProximaDBStub(self.channel)
        
        logger.info(f"🚀 ProximaDB gRPC client initialized (real gRPC/HTTP2): {endpoint}")
    
    def __enter__(self):
        return self
    
    def __exit__(self, exc_type, exc_val, exc_tb):
        self.close()
    
    def close(self):
        """Close the gRPC channel"""
        if hasattr(self, 'channel'):
            self.channel.close()
    
    def health(self) -> HealthStatus:
        """Check server health status using real gRPC"""
        try:
            request = proximadb_pb2.HealthRequest()
            
            metadata = []
            if self.api_key:
                metadata.append(('authorization', f'Bearer {self.api_key}'))
            
            response = self.stub.Health(
                request, 
                timeout=self.timeout,
                metadata=metadata,
                compression=self.compression
            )
            
            return HealthStatus(
                status=response.status,
                version=response.version,
                uptime_seconds=response.uptime_seconds if response.uptime_seconds > 0 else None
            )
        except grpc.RpcError as e:
            logger.error(f"gRPC Health check failed: {e.code()}: {e.details()}")
            return HealthStatus(status="unhealthy", version="unknown")
        except Exception as e:
            logger.error(f"Health check failed: {e}")
            return HealthStatus(status="unhealthy", version="unknown")
    
    def create_collection(
        self,
        name: str,
        config: Optional[CollectionConfig] = None,
        **kwargs
    ) -> Collection:
        """Create a new vector collection using real gRPC"""
        if config is None:
            config = CollectionConfig(dimension=768, **kwargs)
        
        try:
            request = proximadb_pb2.CreateCollectionRequest(
                name=name,
                dimension=config.dimension,
                distance_metric=config.distance_metric.value,
                indexing_algorithm=getattr(config.index_config, 'algorithm', 'hnsw') if config.index_config else 'hnsw',
                storage_layout=config.storage_layout
            )
            
            # Add VIPER-specific optimization fields
            if config.filterable_metadata_fields:
                request.filterable_metadata_fields.extend(config.filterable_metadata_fields)
            
            # Add WAL flush configuration
            if config.flush_config and config.flush_config.max_wal_size_mb:
                request.max_wal_size_mb = config.flush_config.max_wal_size_mb
            
            metadata = []
            if self.api_key:
                metadata.append(('authorization', f'Bearer {self.api_key}'))
            
            response = self.stub.CreateCollection(
                request,
                timeout=self.timeout,
                metadata=metadata,
                compression=self.compression
            )
            
            return Collection(
                id=response.collection.id,
                name=response.collection.name,
                dimension=response.collection.dimension,
                metric=response.collection.distance_metric,
                index_type=response.collection.indexing_algorithm,
                vector_count=response.collection.vector_count,
                config=config
            )
        except grpc.RpcError as e:
            logger.error(f"gRPC CreateCollection failed: {e.code()}: {e.details()}")
            raise ProximaDBError(f"Failed to create collection: {e.details()}")
        except Exception as e:
            logger.error(f"Create collection failed: {e}")
            raise ProximaDBError(f"Failed to create collection: {e}")
    
    def get_collection(self, collection_id: str) -> Collection:
        """Get collection metadata using real gRPC"""
        try:
            request = proximadb_pb2.GetCollectionRequest()
            request.identifier.collection_id = collection_id
            
            metadata = []
            if self.api_key:
                metadata.append(('authorization', f'Bearer {self.api_key}'))
            
            response = self.stub.GetCollection(
                request,
                timeout=self.timeout,
                metadata=metadata,
                compression=self.compression
            )
            
            config = CollectionConfig(
                dimension=response.collection.dimension,
                distance_metric=response.collection.distance_metric,
                storage_layout=getattr(response.collection, 'storage_layout', 'viper')
            )
            
            return Collection(
                id=response.collection.id,
                name=response.collection.name,
                dimension=response.collection.dimension,
                metric=response.collection.distance_metric,
                index_type=response.collection.indexing_algorithm,
                vector_count=response.collection.vector_count,
                config=config
            )
        except grpc.RpcError as e:
            logger.error(f"gRPC GetCollection failed: {e.code()}: {e.details()}")
            raise ProximaDBError(f"Failed to get collection: {e.details()}")
        except Exception as e:
            logger.error(f"Get collection failed: {e}")
            raise ProximaDBError(f"Failed to get collection: {e}")
    
    def list_collections(self) -> List[Collection]:
        """List all collections using real gRPC"""
        try:
            request = proximadb_pb2.ListCollectionsRequest()
            
            metadata = []
            if self.api_key:
                metadata.append(('authorization', f'Bearer {self.api_key}'))
            
            response = self.stub.ListCollections(
                request,
                timeout=self.timeout,
                metadata=metadata,
                compression=self.compression
            )
            
            collections = []
            for col in response.collections:
                config = CollectionConfig(
                    dimension=col.dimension,
                    distance_metric=col.distance_metric,
                    storage_layout=getattr(col, 'storage_layout', 'viper')
                )
                
                collections.append(Collection(
                    id=col.id,
                    name=col.name,
                    dimension=col.dimension,
                    metric=col.distance_metric,
                    index_type=col.indexing_algorithm,
                    vector_count=col.vector_count,
                    config=config
                ))
            
            return collections
        except grpc.RpcError as e:
            logger.error(f"gRPC ListCollections failed: {e.code()}: {e.details()}")
            raise ProximaDBError(f"Failed to list collections: {e.details()}")
        except Exception as e:
            logger.error(f"List collections failed: {e}")
            raise ProximaDBError(f"Failed to list collections: {e}")
    
    def delete_collection(self, collection_id: str) -> bool:
        """Delete a collection using real gRPC"""
        try:
            request = proximadb_pb2.DeleteCollectionRequest()
            request.identifier.collection_id = collection_id
            
            metadata = []
            if self.api_key:
                metadata.append(('authorization', f'Bearer {self.api_key}'))
            
            response = self.stub.DeleteCollection(
                request,
                timeout=self.timeout,
                metadata=metadata,
                compression=self.compression
            )
            
            return response.success
        except grpc.RpcError as e:
            logger.error(f"gRPC DeleteCollection failed: {e.code()}: {e.details()}")
            return False
        except Exception as e:
            logger.error(f"Delete collection failed: {e}")
            return False
    
    def insert_vectors(
        self,
        collection_id: str,
        vectors: VectorArray,
        ids: List[str],
        metadata: Optional[List[MetadataDict]] = None,
        upsert: bool = False,
        batch_size: Optional[int] = None,
    ) -> BatchResult:
        """Insert multiple vectors using real gRPC"""
        # Normalize vectors
        if isinstance(vectors, np.ndarray):
            vectors = vectors.tolist()
        
        try:
            request = proximadb_pb2.BatchInsertRequest()
            request.collection_identifier.collection_id = collection_id
            request.upsert = upsert
            
            # Add vectors
            for i, (vector_id, vector) in enumerate(zip(ids, vectors)):
                vector_msg = request.vectors.add()
                vector_msg.id = vector_id
                vector_msg.vector.extend(vector)
                
                # Add metadata if provided
                if metadata and i < len(metadata) and metadata[i]:
                    from google.protobuf.struct_pb2 import Struct
                    metadata_struct = Struct()
                    metadata_struct.update(metadata[i])
                    vector_msg.metadata.CopyFrom(metadata_struct)
            
            metadata_headers = []
            if self.api_key:
                metadata_headers.append(('authorization', f'Bearer {self.api_key}'))
            
            response = self.stub.BatchInsert(
                request,
                timeout=self.timeout,
                metadata=metadata_headers,
                compression=self.compression
            )
            
            return BatchResult(
                total_count=len(vectors),
                successful_count=response.successful_count,
                failed_count=response.failed_count,
                duration_ms=response.duration_ms,
                errors=list(response.errors) if response.errors else None
            )
        except grpc.RpcError as e:
            logger.error(f"gRPC BatchInsert failed: {e.code()}: {e.details()}")
            raise ProximaDBError(f"Failed to insert vectors: {e.details()}")
        except Exception as e:
            logger.error(f"Insert vectors failed: {e}")
            raise ProximaDBError(f"Failed to insert vectors: {e}")
    
    def insert_vector(
        self,
        collection_id: str,
        vector_id: str,
        vector: Union[List[float], np.ndarray],
        metadata: Optional[MetadataDict] = None,
        upsert: bool = False,
    ) -> BatchResult:
        """Insert a single vector using real gRPC"""
        # Normalize vector
        if isinstance(vector, np.ndarray):
            vector = vector.tolist()
        
        # Use batch insert for single vector
        return self.insert_vectors(
            collection_id=collection_id,
            vectors=[vector],
            ids=[vector_id],
            metadata=[metadata] if metadata else None,
            upsert=upsert
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
        """Search for similar vectors using real gRPC"""
        # Normalize query
        if isinstance(query, np.ndarray):
            query = query.tolist()
        
        try:
            request = proximadb_pb2.SearchRequest()
            request.collection_identifier.collection_id = collection_id
            request.query_vector.extend(query)
            request.k = k
            request.include_vectors = include_vectors
            request.include_metadata = include_metadata
            
            if filter:
                from google.protobuf.struct_pb2 import Struct
                filter_struct = Struct()
                filter_struct.update(filter)
                request.filter.CopyFrom(filter_struct)
            
            if ef is not None:
                request.ef = ef
            
            request.exact = exact
            
            metadata_headers = []
            if self.api_key:
                metadata_headers.append(('authorization', f'Bearer {self.api_key}'))
            
            response = self.stub.Search(
                request,
                timeout=timeout or self.timeout,
                metadata=metadata_headers,
                compression=self.compression
            )
            
            results = []
            for result in response.results:
                search_result = SearchResult(
                    id=result.id,
                    score=result.score,
                    vector=list(result.vector) if result.vector else None,
                    metadata=dict(result.metadata) if result.metadata else {}
                )
                results.append(search_result)
            
            return results
        except grpc.RpcError as e:
            logger.error(f"gRPC Search failed: {e.code()}: {e.details()}")
            raise ProximaDBError(f"Failed to search vectors: {e.details()}")
        except Exception as e:
            logger.error(f"Search failed: {e}")
            raise ProximaDBError(f"Failed to search vectors: {e}")