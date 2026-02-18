"""
ProximaDB gRPC Protocol Adapter

Wraps the gRPC protocol client to implement the BaseProtocolAdapter interface.
Converts gRPC/protobuf responses to standardized Pydantic models.

Copyright 2025 ProximaDB Contributors
Licensed under the Apache License, Version 2.0
"""

import logging
from typing import Any, Dict, List, Optional, Union

from ..models import (
    Collection,
    CollectionConfig,
    FilterDict,
    HealthStatus,
    MetadataDict,
    OperationMetrics,
    SearchResult,
    VectorArray,
    VectorOperationResponse,
    VectorRecord,
)
from ..proto_conversion import ProtoConverter
from .base import BaseProtocolAdapter

logger = logging.getLogger(__name__)


class GrpcProtocolAdapter(BaseProtocolAdapter):
    """gRPC protocol adapter implementing BaseProtocolAdapter.

    Wraps the existing ProximaDBSyncGrpcClient to provide a consistent
    interface that returns Pydantic models.
    """

    def __init__(
        self,
        server_address: str = "localhost:5678",
        timeout: float = 60.0,
        pool_size: int = 5,
        max_message_size: int = 64 * 1024 * 1024,
        **kwargs,
    ):
        """Initialize gRPC protocol adapter.

        Args:
            server_address: gRPC server address (host:port). Default is unified port 5678
                           which handles both REST and gRPC via TCP multiplexing.
            timeout: Request timeout in seconds
            pool_size: Number of gRPC channels in connection pool
            max_message_size: Maximum message size in bytes
            **kwargs: Additional configuration passed to underlying client
        """
        from ..protocols.grpc_sync import ProximaDBSyncGrpcClient

        # Create the underlying gRPC client
        self._client = ProximaDBSyncGrpcClient(
            server_address=server_address,
            timeout=timeout,
            pool_size=pool_size,
            max_message_size=max_message_size,
            **kwargs,
        )
        self._server_address = server_address
        self._connected = True

    @property
    def protocol_name(self) -> str:
        """Return the protocol name."""
        return "grpc"

    @property
    def is_connected(self) -> bool:
        """Check if the adapter is connected and operational."""
        return self._connected

    # ==========================================================================
    # Health & Server Operations
    # ==========================================================================

    def health(self) -> HealthStatus:
        """Check server health status."""
        try:
            result = self._client.health_check()

            # Convert HealthCheckResponse to HealthStatus
            if hasattr(result, "healthy"):
                return HealthStatus(
                    status="healthy" if result.healthy else "unhealthy",
                    healthy=result.healthy,
                    timestamp_ms=int(getattr(result, "latency_ms", 0)),
                    services={"grpc": "ok" if result.healthy else "error"},
                    version=getattr(result, "version", None),
                )

            return HealthStatus(
                status="unknown", healthy=False, timestamp_ms=0, services={}
            )
        except Exception as e:
            logger.error(f"Health check failed: {e}")
            return HealthStatus(
                status="unhealthy",
                healthy=False,
                timestamp_ms=0,
                services={"grpc": str(e)},
            )

    # ==========================================================================
    # Collection Operations
    # ==========================================================================

    def create_collection(
        self, name: str, config: Optional[CollectionConfig] = None, **kwargs
    ) -> Collection:
        """Create a new vector collection."""
        # Extract parameters from config or kwargs
        dimension = config.dimension if config else kwargs.get("dimension", 128)
        distance_metric = ProtoConverter.distance_metric_to_int(
            config.distance_metric if config else kwargs.get("distance_metric")
        )
        storage_engine = ProtoConverter.storage_engine_to_int(
            config.storage_engine if config else kwargs.get("storage_engine")
        )

        result = self._client.create_collection(
            name=name,
            dimension=dimension,
            distance_metric=distance_metric,
            storage_engine=storage_engine,
            **{
                k: v
                for k, v in kwargs.items()
                if k not in ["dimension", "distance_metric", "storage_engine"]
            },
        )

        # Convert CollectionWrapper or proto to Collection
        return self._to_collection(result, name, dimension)

    def get_collection(self, collection_id: str) -> Optional[Collection]:
        """Get collection metadata by ID or name."""
        try:
            result = self._client.get_collection(collection_id)

            if result is None:
                return None

            return self._to_collection(result, collection_id)
        except Exception as e:
            logger.debug(f"Collection not found: {collection_id} - {e}")
            return None

    def list_collections(self) -> List[Collection]:
        """List all collections."""
        results = self._client.list_collections()

        collections = []
        for item in results or []:
            try:
                collections.append(self._to_collection(item))
            except Exception as e:
                logger.warning(f"Failed to convert collection: {e}")

        return collections

    def delete_collection(self, collection_id: str) -> bool:
        """Delete a collection."""
        try:
            result = self._client.delete_collection(collection_id)

            if hasattr(result, "success"):
                return result.success
            return True
        except Exception as e:
            logger.error(f"Failed to delete collection: {e}")
            return False

    def _to_collection(
        self, result: Any, fallback_name: str = "", fallback_dimension: int = 0
    ) -> Collection:
        """Convert various result types to Collection model."""
        if isinstance(result, Collection):
            return result

        if isinstance(result, dict):
            return Collection(**result)

        # Handle CollectionWrapper or protobuf objects
        name = getattr(result, "name", fallback_name)
        dimension = getattr(result, "dimension", fallback_dimension)
        coll_id = getattr(result, "id", name)

        return Collection(
            id=coll_id,
            name=name,
            dimension=dimension,
        )

    # ==========================================================================
    # Vector Operations
    # ==========================================================================

    def insert_vectors(
        self,
        collection_id: str,
        vectors: Union[List[VectorRecord], List[Dict[str, Any]]],
        **kwargs,
    ) -> VectorOperationResponse:
        """Insert vectors into a collection."""
        # Convert VectorRecord objects to dicts
        vector_dicts = []
        for v in vectors:
            if isinstance(v, dict):
                vector_dicts.append(v)
            elif hasattr(v, "model_dump"):
                vector_dicts.append(v.model_dump(exclude_none=True))
            else:
                vector_dicts.append(ProtoConverter.vector_record_to_dict(v))

        result = self._client.insert_vectors(
            collection_id=collection_id, vectors=vector_dicts, **kwargs
        )

        return self._to_vector_operation_response(result, "INSERT", len(vectors))

    def upsert_vectors(
        self,
        collection_id: str,
        vectors: Union[List[VectorRecord], List[Dict[str, Any]]],
        **kwargs,
    ) -> VectorOperationResponse:
        """Upsert (insert or update) vectors in a collection."""
        # Convert VectorRecord objects to dicts
        vector_dicts = []
        for v in vectors:
            if isinstance(v, dict):
                vector_dicts.append(v)
            elif hasattr(v, "model_dump"):
                vector_dicts.append(v.model_dump(exclude_none=True))
            else:
                vector_dicts.append(ProtoConverter.vector_record_to_dict(v))

        # gRPC insert with upsert flag
        result = self._client.insert_vectors(
            collection_id=collection_id, vectors=vector_dicts, upsert=True, **kwargs
        )

        return self._to_vector_operation_response(result, "UPSERT", len(vectors))

    def get_vectors(
        self,
        collection_id: str,
        vector_ids: List[str],
        include_vectors: bool = True,
        **kwargs,
    ) -> List[VectorRecord]:
        """Get vectors by IDs."""
        if hasattr(self._client, "get_vectors"):
            results = self._client.get_vectors(
                collection_id, vector_ids, include_vectors=include_vectors, **kwargs
            )
        else:
            # Fallback: not implemented in gRPC client
            logger.warning("get_vectors not implemented in gRPC client")
            return []

        # Convert to VectorRecord list
        records = []
        for r in results or []:
            if isinstance(r, VectorRecord):
                records.append(r)
            elif isinstance(r, dict):
                records.append(VectorRecord(**r))
            elif hasattr(r, "id"):
                records.append(
                    VectorRecord(
                        id=getattr(r, "id", ""),
                        vector=list(getattr(r, "vector", [])),
                        metadata=dict(getattr(r, "metadata", {})),
                    )
                )

        return records

    def delete_vectors(
        self, collection_id: str, vector_ids: List[str], **kwargs
    ) -> VectorOperationResponse:
        """Delete vectors by IDs."""
        if hasattr(self._client, "delete_vectors"):
            result = self._client.delete_vectors(collection_id, vector_ids, **kwargs)
        else:
            # Fallback: not implemented
            return VectorOperationResponse(
                success=False,
                operation="DELETE",
                error_message="delete_vectors not implemented in gRPC client",
            )

        return self._to_vector_operation_response(result, "DELETE", len(vector_ids))

    def update_vector_metadata(
        self, collection_id: str, vector_id: str, metadata: MetadataDict, **kwargs
    ) -> VectorOperationResponse:
        """Update metadata for a specific vector."""
        if hasattr(self._client, "update_vector_metadata"):
            result = self._client.update_vector_metadata(
                collection_id, vector_id, metadata, **kwargs
            )
            return self._to_vector_operation_response(result, "UPDATE", 1)

        # Fallback: not implemented
        return VectorOperationResponse(
            success=False,
            operation="UPDATE",
            error_message="update_vector_metadata not implemented in gRPC client",
        )

    def _to_vector_operation_response(
        self, result: Any, operation: str, total_count: int
    ) -> VectorOperationResponse:
        """Convert various result types to VectorOperationResponse."""
        if isinstance(result, VectorOperationResponse):
            return result

        if isinstance(result, dict):
            return VectorOperationResponse(
                success=result.get("success", True),
                operation=operation,
                metrics=OperationMetrics(
                    successful_count=result.get("successful_count", total_count),
                    failed_count=result.get("failed_count", 0),
                    total_count=total_count,
                ),
                error_message=result.get("error_message"),
            )

        # Handle wrapper objects
        success = getattr(result, "success", True)
        metrics = getattr(result, "metrics", None)

        if metrics:
            return VectorOperationResponse(
                success=success,
                operation=operation,
                metrics=OperationMetrics(
                    successful_count=getattr(metrics, "successful_count", total_count),
                    failed_count=getattr(metrics, "failed_count", 0),
                    duration_ms=getattr(metrics, "duration_ms", 0),
                    total_count=total_count,
                ),
                error_message=getattr(result, "error_message", None),
            )

        return VectorOperationResponse(
            success=success,
            operation=operation,
            metrics=OperationMetrics(
                successful_count=total_count if success else 0,
                failed_count=0 if success else total_count,
                total_count=total_count,
            ),
            error_message=getattr(result, "error_message", None),
        )

    # ==========================================================================
    # Search Operations
    # ==========================================================================

    def search(
        self,
        collection_id: str,
        query_vector: VectorArray,
        top_k: int = 10,
        filter: Optional[FilterDict] = None,
        include_vectors: bool = False,
        include_metadata: bool = True,
        **kwargs,
    ) -> List[SearchResult]:
        """Search for similar vectors."""
        # Normalize query vector
        if hasattr(query_vector, "tolist"):
            query_vector = query_vector.tolist()

        results = self._client.search_vectors(
            collection_id=collection_id,
            query_vector=list(query_vector),
            top_k=top_k,
            metadata_filters=filter,
            include_vectors=include_vectors,
            include_metadata=include_metadata,
            **kwargs,
        )

        return self._to_search_results(results, include_vectors, include_metadata)

    def batch_search(
        self,
        collection_id: str,
        query_vectors: List[VectorArray],
        top_k: int = 10,
        filter: Optional[FilterDict] = None,
        include_vectors: bool = False,
        include_metadata: bool = True,
        **kwargs,
    ) -> List[List[SearchResult]]:
        """Batch search for similar vectors."""
        # Normalize query vectors
        normalized_queries = []
        for qv in query_vectors:
            if hasattr(qv, "tolist"):
                normalized_queries.append(qv.tolist())
            else:
                normalized_queries.append(list(qv))

        # Use search_vectors with multiple query vectors
        results = self._client.search_vectors(
            collection_id=collection_id,
            query_vectors=normalized_queries,
            top_k=top_k,
            metadata_filters=filter,
            include_vectors=include_vectors,
            include_metadata=include_metadata,
            **kwargs,
        )

        # Handle single query result vs batch results
        if results and not isinstance(results[0], list):
            # Single query result - wrap in list for batch format
            return [self._to_search_results(results, include_vectors, include_metadata)]

        # Multiple query results
        batch_results = []
        for query_results in results or []:
            batch_results.append(
                self._to_search_results(
                    query_results, include_vectors, include_metadata
                )
            )

        return batch_results

    def _to_search_results(
        self, results: Any, include_vectors: bool, include_metadata: bool
    ) -> List[SearchResult]:
        """Convert various result types to SearchResult list."""
        if results is None:
            return []

        search_results = []
        for r in results:
            if isinstance(r, SearchResult):
                search_results.append(r)
            elif isinstance(r, dict):
                search_results.append(
                    SearchResult(
                        id=r.get("id", r.get("vector_id", "")),
                        score=r.get("score", r.get("distance", 0.0)),
                        vector=r.get("vector") if include_vectors else None,
                        metadata=r.get("metadata") if include_metadata else None,
                    )
                )
            elif hasattr(r, "id"):
                vector = None
                if include_vectors and hasattr(r, "vector"):
                    vector = list(r.vector) if r.vector else None

                metadata = None
                if include_metadata and hasattr(r, "metadata"):
                    metadata = dict(r.metadata) if r.metadata else {}

                search_results.append(
                    SearchResult(
                        id=getattr(r, "id", ""),
                        score=getattr(r, "score", getattr(r, "distance", 0.0)),
                        vector=vector,
                        metadata=metadata,
                    )
                )

        return search_results

    # ==========================================================================
    # Lifecycle Methods
    # ==========================================================================

    def close(self) -> None:
        """Close the gRPC client connection pool."""
        if hasattr(self._client, "close"):
            self._client.close()
        self._connected = False
