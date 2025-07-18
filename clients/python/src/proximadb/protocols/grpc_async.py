"""
ProximaDB gRPC Client - Updated for Current Proto Schema

This client uses the current ProximaDB gRPC service with unified endpoints.
Aligned with proximadb.proto v1 schema.

Copyright 2025 ProximaDB
"""

import logging
import time
from typing import Optional, Dict, List, Any, Union
import json
from datetime import datetime, timezone
import io

import grpc
# Removed Avro imports - using pure proto messages for proto-first architecture

try:
    from ..proximadb_pb2 import *
    from .. import proximadb_pb2 as pb2
    from .. import proximadb_pb2_grpc as pb2_grpc
except ImportError as e:
    pb2 = None
    pb2_grpc = None
    # Will try again in _connect() method
from ..exceptions import ProximaDBError
from ..models import DeleteResult, BatchResult  # Import Pydantic models for return types
# Note: gRPC client uses proto-generated classes directly - no Pydantic models
# Proto classes are accessed via pb2.ClassName (e.g., pb2.Collection, pb2.SearchResult)

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
        global pb2, pb2_grpc
        
        # Import proto modules dynamically
        try:
            from .. import proximadb_pb2 as pb2_local
            from .. import proximadb_pb2_grpc as pb2_grpc_local
            
            # Check if imports actually contain the expected classes
            if not hasattr(pb2_grpc_local, 'ProximaDBStub'):
                raise ProximaDBError(f"pb2_grpc_local is missing ProximaDBStub: {pb2_grpc_local}")
            if not hasattr(pb2_local, 'DistanceMetric'):
                raise ProximaDBError(f"pb2_local is missing DistanceMetric: {pb2_local}")
                
            logger.debug(f"Proto imports successful: pb2_local={pb2_local}, pb2_grpc_local={pb2_grpc_local}")
        except ImportError as e:
            logger.error(f"Failed to import proto modules: {e}")
            raise ProximaDBError(f"Failed to import proto modules: {e}")
        
        try:
            # Configure message size limits for bulk vector operations (64MB)
            max_message_size = 64 * 1024 * 1024
            options = [
                ('grpc.max_receive_message_length', max_message_size),
                ('grpc.max_send_message_length', max_message_size),
            ]
            
            if self.use_tls:
                credentials = grpc.ssl_channel_credentials()
                self.channel = grpc.secure_channel(self.endpoint, credentials, options=options)
            else:
                self.channel = grpc.insecure_channel(self.endpoint, options=options)
            
            self.stub = pb2_grpc_local.ProximaDBStub(self.channel)
            logger.info(f"Connected to ProximaDB gRPC service at {self.endpoint} (64MB message limit)")
            
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
    
    def _resolve_collection_id(self, collection_id: str) -> str:
        """
        Resolve collection identifier for gRPC API.
        
        The gRPC API accepts both collection names and UUIDs in the collection_id field.
        The server will handle resolution transparently.
        
        Args:
            collection_id: Collection name or UUID
            
        Returns:
            Collection identifier (name or UUID) - passed through as-is
        """
        return collection_id
    
    async def get_collection_id_by_name(self, collection_name: str) -> Optional[str]:
        """
        Get collection UUID by name using the dedicated gRPC operation.
        
        Args:
            collection_name: Name of the collection
            
        Returns:
            Collection UUID if found, None if not found
            
        Raises:
            ProximaDBError: If there's an error retrieving the collection
        """
        try:
            # Use the dedicated COLLECTION_GET_ID_BY_NAME operation
            request = pb2.CollectionRequest(
                operation=pb2.CollectionOperation.COLLECTION_GET_ID_BY_NAME,
                collection_id=collection_name
            )
            
            response = await self._make_call(
                self.stub.CollectionOperation,
                request
            )
            
            if response.success and 'collection_id' in response.metadata:
                return response.metadata['collection_id']
            
            return None
            
        except grpc.RpcError as e:
            if e.code() == grpc.StatusCode.NOT_FOUND:
                return None
            logger.error(f"gRPC error getting collection ID by name: {e}")
            # Fallback to the original method
            try:
                collection = await self.get_collection(collection_name)
                if collection and hasattr(collection, 'id'):
                    return collection.id
                return None
            except CollectionNotFoundError:
                return None
        except Exception as e:
            logger.error(f"Error getting collection ID by name: {e}")
            # Fallback to the original method
            try:
                collection = await self.get_collection(collection_name)
                if collection and hasattr(collection, 'id'):
                    return collection.id
                return None
            except CollectionNotFoundError:
                return None
    
    def create_collection(
        self,
        name: str,
        dimension: int,
        distance_metric: int = None,
        indexing_algorithm: int = None,
        storage_engine: int = None,
        filterable_columns: List = None,
        index_configs: List = None,
        quantization_config = None
    ):
        """Create a new collection"""
        
        # Ensure pb2 is available
        if pb2 is None:
            from .. import proximadb_pb2 as pb2
        
        # Set default values if not provided
        if distance_metric is None:
            distance_metric = pb2.DistanceMetric.COSINE
        if indexing_algorithm is None:
            indexing_algorithm = pb2.IndexingAlgorithm.HNSW
        if storage_engine is None:
            storage_engine = pb2.StorageEngine.VIPER
        if filterable_columns is None:
            filterable_columns = []
        if index_configs is None:
            index_configs = []
        
        # Build CollectionConfig using proto types
        config = pb2.CollectionConfig(
            name=name,
            dimension=dimension,
            distance_metric=distance_metric,
            primary_indexing_algorithm=indexing_algorithm,
            storage_engine=storage_engine,
            filterable_columns=filterable_columns or [],
            index_configs=index_configs or [],
            quantization_config=quantization_config
        )
        
        # Build CollectionRequest
        request = pb2.CollectionRequest(
            operation=pb2.CollectionOperation.COLLECTION_CREATE,
            collection_config=config
        )
        
        response = self._call_with_timeout(self.stub.CollectionOperation, request)
        
        if not response.success:
            raise ProximaDBError(f"Failed to create collection: {response.error_message}")
        
        if response.collection:
            return response.collection  # Return proto object directly
        else:
            raise ProximaDBError("No collection returned in response")
    
    def get_collection(self, name: str):
        """Get collection by name"""
        
        request = pb2.CollectionRequest(
            operation=pb2.CollectionOperation.COLLECTION_GET,
            collection_id=name
        )
        
        response = self._call_with_timeout(self.stub.CollectionOperation, request)
        
        if not response.success:
            if "not found" in response.error_message.lower():
                return None
            raise ProximaDBError(f"Failed to get collection: {response.error_message}")
        
        if response.collection:
            return response.collection  # Return proto object directly
        return None
    
    def list_collections(self):
        """List all collections"""
        
        request = pb2.CollectionRequest(
            operation=pb2.CollectionOperation.COLLECTION_LIST
        )
        
        response = self._call_with_timeout(self.stub.CollectionOperation, request)
        
        if not response.success:
            raise ProximaDBError(f"Failed to list collections: {response.error_message}")
        
        return list(response.collections)  # Return proto objects directly
    
    def delete_collection(self, name: str) -> bool:
        """Delete collection by name"""
        
        request = pb2.CollectionRequest(
            operation=pb2.CollectionOperation.COLLECTION_DELETE,
            collection_id=name
        )
        
        response = self._call_with_timeout(self.stub.CollectionOperation, request)
        
        if not response.success:
            if "not found" in response.error_message.lower():
                return False
            raise ProximaDBError(f"Failed to delete collection: {response.error_message}")
        
        return True
    
    async def update_collection(self, name: str, config):
        """Update collection configuration"""
        
        # Use provided config directly (it's already a proto object)
        collection_config = config
        
        request = pb2.CollectionRequest(
            operation=pb2.CollectionOperation.COLLECTION_UPDATE,
            collection_id=name,
            collection_config=collection_config
        )
        
        response = self._call_with_timeout(self.stub.CollectionOperation, request)
        
        if not response.success:
            raise ProximaDBError(f"Failed to update collection: {response.error_message}")
        
        if response.collection:
            return response.collection  # Return proto object directly
        else:
            raise ProximaDBError("No collection returned in response")
    
    # Health Check
    
    def health_check(self):
        """Check server health"""
        
        request = pb2.HealthRequest()
        
        response = self._call_with_timeout(self.stub.Health, request)
        
        return response  # Return proto object directly
    
    # Helper methods for proto object creation
    
    def create_vector_record(
        self, 
        vector: List[float], 
        id: Optional[str] = None,
        metadata: Optional[Dict[str, Any]] = None,
        timestamp: Optional[int] = None,
        version: int = 0,
        expires_at: Optional[int] = None
    ):
        """Create a VectorRecord proto object"""
        record = pb2.VectorRecord(
            vector=vector,
            version=version
        )
        
        if id is not None:
            record.id = id
        if timestamp is not None:
            record.timestamp = timestamp
        if expires_at is not None:
            record.expires_at = expires_at
        if metadata:
            record.metadata.CopyFrom(self._dict_to_metadata_map(metadata))
        
        return record
    
    def _dict_to_metadata_map(self, metadata: Dict[str, Any]):
        """Convert Python dict to proto MetadataMap"""
        meta_map = pb2.MetadataMap()
        
        for key, value in metadata.items():
            meta_value = pb2.MetadataValue()
            
            if isinstance(value, str):
                meta_value.string_value = value
            elif isinstance(value, int):
                meta_value.int_value = value
            elif isinstance(value, float):
                meta_value.double_value = value
            elif isinstance(value, bool):
                meta_value.bool_value = value
            elif isinstance(value, list):
                if value and isinstance(value[0], str):
                    meta_value.string_array.CopyFrom(pb2.StringArray(values=value))
                elif value and isinstance(value[0], int):
                    meta_value.int_array.CopyFrom(pb2.Int64Array(values=value))
                elif value and isinstance(value[0], float):
                    meta_value.double_array.CopyFrom(pb2.DoubleArray(values=value))
            
            meta_map.fields[key].CopyFrom(meta_value)
        
        return meta_map
    
    # Vector operations
    def insert_vectors(
        self,
        collection_id: str,
        vectors,  # Can be List[Dict] or single Dict or raw vectors
        ids: Optional[List[str]] = None,
        metadata: Optional[List[Dict[str, Any]]] = None,
        upsert: bool = False
    ):
        """
        Unified vector insertion interface - handles single vectors or batches
        
        Args:
            collection_id: Collection name/ID
            vectors: Can be:
                    - List[Dict] with format [{"id": "vec1", "vector": [0.1, 0.2, ...], "metadata": {...}}, ...]
                    - List[List[float]] raw vectors (requires ids parameter)
                    - Single Dict {"id": "vec1", "vector": [...], "metadata": {...}}
                    - Single List[float] raw vector (requires ids parameter)
            ids: Vector IDs (required if vectors is raw vector data)
            metadata: Vector metadata (optional, used with raw vector data)
            upsert: If True, update existing vectors
        
        Returns:
            InsertResult with inserted vector IDs
        """
        try:
            # Normalize input to standard format
            normalized_vectors = self._normalize_vector_input(vectors, ids, metadata)
            
            # Debug logging
            logger.debug(f"Inserting {len(normalized_vectors)} vectors via gRPC")
            logger.debug(f"First vector format: {list(normalized_vectors[0].keys()) if normalized_vectors else 'empty'}")
            
            # Process in chunks to handle large batches efficiently
            chunk_size = 1000  # Reasonable chunk size for gRPC
            total_inserted = 0
            total_failed = 0
            total_duration = 0.0
            
            for i in range(0, len(normalized_vectors), chunk_size):
                chunk = normalized_vectors[i:i + chunk_size]
                
                # Serialize chunk to proper Avro binary format
                # Create proto vector records directly (proto-first architecture)
                proto_vectors = self._create_proto_vector_batch(chunk, collection_id)
                
                # Create VectorBatchRequest with pure proto messages
                request = pb2.VectorBatchRequest(
                    collection_id=collection_id,
                    vectors=proto_vectors
                )
                
                # Call gRPC service
                response = self._call_with_timeout(self.stub.VectorBatch, request)
                
                if response.success:
                    # Get metrics if available
                    metrics = response.metrics if response.metrics else None
                    chunk_count = len(response.vector_ids) if response.vector_ids else len(chunk)
                    chunk_duration = (metrics.processing_time_us / 1000.0) if metrics else 0.0
                    
                    total_inserted += chunk_count
                    total_duration += chunk_duration
                    
                    logger.debug(f"Chunk {i//chunk_size + 1}: {chunk_count}/{len(chunk)} vectors inserted")
                else:
                    error_msg = response.error_message or 'Unknown error'
                    logger.error(f"Chunk {i//chunk_size + 1} failed: {error_msg}")
                    total_failed += len(chunk)
            
            # Return the last response (or could aggregate if needed)
            return response
                
        except grpc.RpcError as e:
            logger.error(f"gRPC error during vector insert: {e}")
            raise ProximaDBError(f"Vector insert failed: {str(e)}")
        except Exception as e:
            logger.error(f"Unexpected error during vector insert: {e}")
            raise ProximaDBError(f"Vector insert failed: {str(e)}")
    
    def _normalize_vector_input(
        self, 
        vectors, 
        ids: Optional[List[str]] = None, 
        metadata: Optional[List[Dict[str, Any]]] = None
    ) -> List[Dict[str, Any]]:
        """
        Normalize various vector input formats to standard format
        
        Returns:
            List[Dict] with format [{"id": "vec1", "vector": [0.1, 0.2, ...], "metadata": {...}}, ...]
        """
        import uuid
        
        # Case 1: Already in correct format (List[Dict])
        if isinstance(vectors, list) and vectors and isinstance(vectors[0], dict):
            if "vector" in vectors[0]:  # Only require "vector" key, "id" is optional
                # Ensure all dicts have an "id" field (generate if missing)
                import uuid
                for i, vector_dict in enumerate(vectors):
                    if "id" not in vector_dict or not vector_dict["id"]:
                        vector_dict["id"] = f"vec_{uuid.uuid4().hex[:8]}"
                return vectors
        
        # Case 2: Single dict vector
        if isinstance(vectors, dict) and "vector" in vectors:
            # Ensure it has an "id" field (generate if missing)
            if "id" not in vectors or not vectors["id"]:
                vectors["id"] = f"vec_{uuid.uuid4().hex[:8]}"
            return [vectors]
        
        # Case 3: Raw vector data - List[List[float]] or List[float]
        normalized = []
        
        # Handle single vector (List[float])
        if isinstance(vectors, list) and vectors and isinstance(vectors[0], (int, float)):
            vectors = [vectors]  # Convert to List[List[float]]
            if ids:
                ids = [ids[0]] if isinstance(ids, list) else [ids]
            if metadata:
                metadata = [metadata[0]] if isinstance(metadata, list) else [metadata]
        
        # Process as List[List[float]]
        if isinstance(vectors, list) and vectors and isinstance(vectors[0], list):
            for i, vector in enumerate(vectors):
                # Generate ID if not provided
                vector_id = ids[i] if ids and i < len(ids) else f"vec_{uuid.uuid4().hex[:8]}"
                
                # Get metadata if provided
                vector_metadata = metadata[i] if metadata and i < len(metadata) else {}
                
                normalized.append({
                    "id": vector_id,
                    "vector": vector,
                    "metadata": vector_metadata
                })
            
            return normalized
        
        # If we get here, unsupported format
        raise ProximaDBError(f"Unsupported vector format: {type(vectors)}")
    
    def insert_single_vector(
        self,
        collection_id: str,
        vector_id: str,
        vector: List[float],
        metadata: Optional[Dict[str, Any]] = None,
        upsert: bool = False
    ):
        """
        Insert a single vector (convenience method)
        
        Args:
            collection_id: Collection name/ID
            vector_id: Vector identifier
            vector: Vector data
            metadata: Optional metadata
            upsert: If True, update existing vector
        
        Returns:
            VectorOperationResponse proto
        """
        vector_dict = {
            "id": vector_id,
            "vector": vector,
            "metadata": metadata or {}
        }
        
        return self.insert_vectors(collection_id, [vector_dict], upsert=upsert)
    
    def search_vectors(
        self,
        collection_id: str,
        query_vectors: List[List[float]],
        top_k: int = 10,
        metadata_filters: Optional[Union[Dict[str, Any], Any]] = None,  # Dict or FilterBuilder
        include_metadata: bool = True,
        include_vectors: bool = False,
        search_params = None
    ):
        """
        Search for similar vectors
        
        Args:
            collection_id: Collection name/ID
            query_vectors: List of query vectors
            top_k: Number of results per query
            metadata_filters: Optional metadata filters
            include_metadata: Include metadata in results
            include_vectors: Include vector data in results
            search_params: Optional search parameters for algorithm tuning
        
        Returns:
            VectorOperationResponse proto with search results
        """
        try:
            # Create search queries
            queries = []
            for vector in query_vectors:
                query = pb2.SearchQuery(vector=vector)
                
                # Add metadata filter if provided
                if metadata_filters:
                    # Check if it's a FilterBuilder instance
                    if hasattr(metadata_filters, 'to_proto_filter'):
                        # Use the FilterBuilder's proto conversion
                        meta_filter = metadata_filters.to_proto_filter()
                        query.metadata_filter.CopyFrom(meta_filter)
                    else:
                        # Legacy dict support - convert dict to MetadataFilter proto
                        meta_filter = pb2.MetadataFilter(
                            operator=pb2.FilterOperator.AND
                        )
                        for key, value in metadata_filters.items():
                            condition = pb2.FilterCondition(
                                field_name=key,
                                operation=pb2.FilterOperation.EQUALS,
                                value=self._value_to_metadata_value(value)
                            )
                            meta_filter.conditions.append(condition)
                        query.metadata_filter.CopyFrom(meta_filter)
                
                queries.append(query)
            
            # Create include fields
            include_fields = pb2.IncludeFields(
                vector=include_vectors,
                metadata=include_metadata,
                score=True,
                rank=True
            )
            
            # Create search request
            request = pb2.VectorSearchRequest(
                collection_id=collection_id,
                queries=queries,
                top_k=top_k,
                include_fields=include_fields
            )
            
            # Add search parameters if provided
            if search_params:
                request.search_params.CopyFrom(search_params)
            
            # Call gRPC service
            response = self._call_with_timeout(self.stub.VectorSearch, request)
            
            return response  # Return proto object directly
                
        except grpc.RpcError as e:
            logger.error(f"gRPC error during vector search: {e}")
            raise ProximaDBError(f"Vector search failed: {str(e)}")
    
    # Removed _value_to_metadata_value - no longer needed with repeated MetadataItem structure
    
    def update_vector(
        self,
        collection_id: str,
        vector_id: str,
        vector: Optional[List[float]] = None,
        metadata: Optional[Dict[str, Any]] = None
    ) -> bool:
        """
        Update a vector
        
        Args:
            collection_id: Collection name/ID
            vector_id: Vector ID to update
            vector: New vector data (optional)
            metadata: New metadata (optional)
        
        Returns:
            True if successful
        """
        # Use upsert to update the vector
        vector_dict = {
            "id": vector_id,
            "vector": vector or [0.0],  # Placeholder if no vector provided
            "metadata": metadata or {}
        }
        
        return self.insert_vectors(collection_id, [vector_dict], upsert=True)
    
    def delete_vector(self, collection_id: str, vector_id: str):
        """
        Delete a vector using expires_at mechanism
        
        Args:
            collection_id: Collection name/ID
            vector_id: Vector ID to delete
            
        Returns:
            VectorOperationResponse proto
        """
        # Create record with expires_at=0 to mark for deletion
        delete_record = {
            "id": vector_id,
            "vector": [0.0],  # Placeholder vector
            "metadata": {},
            "expires_at": 0  # Mark for deletion
        }
        
        return self.insert_vectors(collection_id, [delete_record], upsert=False)
    
    def get_vector(
        self,
        collection_id: str,
        vector_id: str,
        include_vector: bool = True,
        include_metadata: bool = True
    ):
        """
        Get a single vector by ID using VectorGet RPC
        
        Args:
            collection_id: Collection name/ID
            vector_id: Vector ID
            include_vector: Include vector data
            include_metadata: Include metadata
            
        Returns:
            SearchResult proto or None if not found
        """
        # Create include fields
        include_fields = pb2.IncludeFields(
            vector=include_vector,
            metadata=include_metadata,
            score=True,
            rank=True
        )
        
        # Create vector get request
        request = pb2.VectorGetRequest(
            collection_id=collection_id,
            vector_id=vector_id,
            include_fields=include_fields
        )
        
        try:
            response = self._call_with_timeout(self.stub.VectorGet, request)
            
            if response.success:
                # Check for results in compact format
                if hasattr(response, 'result_payload') and response.result_payload:
                    if hasattr(response.result_payload, 'compact_results'):
                        compact_results = response.result_payload.compact_results
                        if compact_results.results:
                            return compact_results.results[0]
                
            return None
            
        except grpc.RpcError as e:
            logger.error(f"gRPC error during vector get: {e}")
            return None
    
    async def update_collection(
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
        try:
            # Use COLLECTION_UPDATE operation
            request = pb2.CollectionRequest(
                operation=pb2.COLLECTION_UPDATE,
                collection_id=collection_id,
                query_params=updates
            )
            
            response = await self.stub.CollectionOperation(request, timeout=self.timeout)
            
            if response.success and response.collection:
                return self._convert_collection(response.collection)
            else:
                raise ProximaDBError(
                    f"Collection update failed: {response.error_message or 'Unknown error'}"
                )
                
        except grpc.RpcError as e:
            logger.error(f"gRPC error during collection update: {e}")
            raise ProximaDBError(f"Collection update failed: {str(e)}")
    
    async def delete_vectors_by_filter(
        self,
        collection_id: str,
        filter: Dict[str, Any]
    ) -> DeleteResult:
        """Delete vectors matching filter criteria
        
        Args:
            collection_id: Collection identifier
            filter: Filter criteria for vector deletion
            
        Returns:
            DeleteResult: Deletion operation result
        """
        try:
            # Create vector selector with metadata filter
            selector = pb2.VectorSelector(metadata_filter=filter)
            
            request = pb2.VectorMutationRequest(
                collection_id=collection_id,
                operation=pb2.MUTATION_DELETE,
                selector=selector
            )
            
            response = await self.stub.VectorMutation(request, timeout=self.timeout)
            
            if response.success:
                return DeleteResult(
                    deleted_count=response.metrics.successful_count,
                    success=True,
                    message=f"Deleted {response.metrics.successful_count} vectors"
                )
            else:
                raise ProximaDBError(
                    f"Vector deletion by filter failed: {response.error_message or 'Unknown error'}"
                )
                
        except grpc.RpcError as e:
            logger.error(f"gRPC error during vector deletion by filter: {e}")
            raise ProximaDBError(f"Vector deletion by filter failed: {str(e)}")
    
    async def get_vector_history(
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
        # This would require a new gRPC endpoint for vector history
        # For now, return placeholder indicating not implemented
        raise ProximaDBError("Vector history not implemented on server yet")
    
    async def multi_search(
        self,
        collection_id: str,
        queries: List[List[float]],
        k: int = 10,
        filter: Optional[Dict[str, Any]] = None,
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
        try:
            # Create multiple search queries
            search_queries = []
            for query in queries:
                search_query = pb2.SearchQuery(vector=query)
                if filter:
                    search_query.metadata_filter.update(filter)
                search_queries.append(search_query)
            
            # Set up include fields
            include_fields = pb2.IncludeFields(
                vector=include_vectors,
                metadata=include_metadata,
                score=True,
                rank=True
            )
            
            request = pb2.VectorSearchRequest(
                collection_id=collection_id,
                queries=search_queries,
                top_k=k,
                include_fields=include_fields
            )
            
            response = await self.stub.VectorSearch(request, timeout=self.timeout)
            
            if response.success:
                results = []
                result_type = response.WhichOneof('result_payload')
                if result_type == 'compact_results':
                    for result_pb in response.compact_results.results:
                        # Convert repeated MetadataItem back to dict
                        metadata_dict = {}
                        if result_pb.metadata:
                            for item in result_pb.metadata:
                                metadata_dict[item.key] = item.value
                        
                        results.append(SearchResult(
                            id=result_pb.id if result_pb.id else None,
                            score=result_pb.score,
                            vector=list(result_pb.vector) if result_pb.vector else None,
                            metadata=metadata_dict if metadata_dict else None
                        ))
                return results
            else:
                raise ProximaDBError(
                    f"Multi-search failed: {response.error_message or 'Unknown error'}"
                )
                
        except grpc.RpcError as e:
            logger.error(f"gRPC error during multi-search: {e}")
            raise ProximaDBError(f"Multi-search failed: {str(e)}")
    
    async def search_with_aggregations(
        self,
        collection_id: str,
        query: List[float],
        aggregations: List[str],
        k: int = 10,
        group_by: Optional[str] = None,
        filter: Optional[Dict[str, Any]] = None
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
        # This would require extending the gRPC search to support aggregations
        # For now, return placeholder indicating not implemented
        raise ProximaDBError("Search with aggregations not implemented on server yet")
    
    async def atomic_insert_vectors(
        self,
        collection_id: str,
        vectors: List[List[float]],
        ids: List[str],
        metadata: Optional[List[Dict[str, Any]]] = None
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
        # For atomic operations, we could extend the VectorInsertRequest with an atomic flag
        # For now, use regular insert_vectors and hope for atomicity
        return await self.insert_vectors(collection_id, vectors, ids, metadata)
    
    async def begin_transaction(self) -> str:
        """Begin a new transaction and return transaction ID
        
        Returns:
            str: Transaction identifier
        """
        # This would require a new gRPC service for transaction management
        # For now, return placeholder indicating not implemented
        raise ProximaDBError("Transactions not implemented on server yet")
    
    async def commit_transaction(self, transaction_id: str) -> bool:
        """Commit a transaction
        
        Args:
            transaction_id: Transaction identifier
            
        Returns:
            bool: True if committed successfully
        """
        # This would require a new gRPC service for transaction management
        # For now, return placeholder indicating not implemented
        raise ProximaDBError("Transactions not implemented on server yet")
    
    async def rollback_transaction(self, transaction_id: str) -> bool:
        """Rollback a transaction
        
        Args:
            transaction_id: Transaction identifier
            
        Returns:
            bool: True if rolled back successfully
        """
        # This would require a new gRPC service for transaction management
        # For now, return placeholder indicating not implemented
        raise ProximaDBError("Transactions not implemented on server yet")

    
    def _create_proto_vector_batch(self, vectors: List[Dict[str, Any]], collection_id: str):
        """
        Create pure proto VectorRecord messages for proto-first architecture
        
        Args:
            vectors: List of vector dictionaries
            collection_id: Collection identifier
            
        Returns:
            List[pb2.VectorRecord]: Proto vector records
        """
        if not vectors:
            raise ProximaDBError("Cannot create proto batch from empty vector list")
        
        logger.debug(f"Creating proto batch for {len(vectors)} vectors")
        
        proto_vectors = []
        current_time = int(time.time() * 1_000_000)  # Microseconds since epoch
        
        try:
            # Convert input vectors to proto VectorRecord messages
            for i, vec in enumerate(vectors):
                try:
                    # Validate vector structure
                    if not isinstance(vec, dict):
                        raise ValueError(f"Vector {i} is not a dictionary: {type(vec)}")
                    if 'vector' not in vec:
                        raise ValueError(f"Vector {i} missing 'vector' field")
                    if not isinstance(vec['vector'], (list, tuple)):
                        raise ValueError(f"Vector {i} 'vector' field is not a list: {type(vec['vector'])}")
                    
                    # Create proto VectorRecord
                    proto_vector = pb2.VectorRecord()
                    
                    # Set ID (optional in proto-first architecture)
                    if vec.get('id'):
                        proto_vector.id = str(vec['id'])
                    
                    # Set vector data (required)
                    proto_vector.vector.extend([float(x) for x in vec['vector']])
                    
                    # Set metadata (repeated MetadataItem for zero-copy efficiency)
                    if vec.get('metadata') and isinstance(vec['metadata'], dict):
                        for key, value in vec['metadata'].items():
                            metadata_item = pb2.MetadataItem()
                            metadata_item.key = str(key)
                            # Convert all values to string for consistent handling
                            if isinstance(value, str):
                                metadata_item.value = value
                            elif isinstance(value, (int, float, bool)):
                                metadata_item.value = str(value)
                            elif value is None:
                                metadata_item.value = ""
                            else:
                                # Convert complex types to JSON string
                                import json
                                metadata_item.value = json.dumps(value)
                            
                            proto_vector.metadata.append(metadata_item)
                    
                    # Set timestamp (optional, microseconds since epoch)
                    timestamp = vec.get('timestamp')
                    if timestamp is not None:
                        if isinstance(timestamp, (int, float)):
                            # Convert to microseconds if needed
                            if timestamp < 1e10:  # Assume seconds, convert to microseconds
                                proto_vector.timestamp = int(timestamp * 1_000_000)
                            else:  # Already in microseconds
                                proto_vector.timestamp = int(timestamp)
                        else:
                            proto_vector.timestamp = current_time
                    else:
                        proto_vector.timestamp = current_time
                    
                    # Set version (default to 1)
                    proto_vector.version = vec.get('version', 1)
                    
                    # Set expires_at (optional)
                    if vec.get('expires_at'):
                        proto_vector.expires_at = int(vec['expires_at'])
                    
                    proto_vectors.append(proto_vector)
                    
                    # Debug first vector
                    if i == 0:
                        logger.debug(f"First vector proto format: id={proto_vector.id}, vector_len={len(proto_vector.vector)}, metadata_count={len(proto_vector.metadata)}")
                    
                except Exception as e:
                    logger.error(f"Failed to process vector {i}: {e}")
                    raise ProximaDBError(f"Failed to process vector {i}: {e}")
            
            logger.debug(f"Successfully converted {len(proto_vectors)} vectors to proto format")
            
            return proto_vectors
            
        except Exception as e:
            logger.error(f"Failed to create proto batch: {e}")
            raise ProximaDBError(f"Failed to create proto batch: {e}")


# For backward compatibility
class ProximaDBGrpcClient(ProximaDBClient):
    """Backward compatibility alias"""
    pass