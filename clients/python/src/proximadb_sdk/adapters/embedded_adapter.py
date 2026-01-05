"""
ProximaDB Embedded Protocol Adapter

Wraps the PyO3 embedded bindings to implement the BaseProtocolAdapter interface.
Converts raw PyO3 responses (often ints) to standardized Pydantic models.

This adapter is the key to Task 2.2: Unified Embedded API - ensuring
embedded mode returns the same response types as REST/gRPC modes.

Copyright 2025 ProximaDB Contributors
Licensed under the Apache License, Version 2.0
"""

import logging
import time
from typing import Any, Dict, List, Optional, Union

from .base import BaseProtocolAdapter
from ..models import (
    Collection,
    CollectionConfig,
    SearchResult,
    VectorOperationResponse,
    HealthStatus,
    VectorRecord,
    VectorArray,
    MetadataDict,
    FilterDict,
    OperationMetrics,
    DistanceMetric,
    StorageEngine,
)
from ..proto_conversion import ProtoConverter

logger = logging.getLogger(__name__)


class EmbeddedProtocolAdapter(BaseProtocolAdapter):
    """Embedded protocol adapter implementing BaseProtocolAdapter.

    Wraps the PyO3 EmbeddedProximaDB bindings to provide a consistent
    interface that returns Pydantic models. This is critical for ensuring
    embedded mode has API parity with REST/gRPC modes.

    Key transformations:
    - insert() returns int (count) -> VectorOperationResponse
    - search() returns list of tuples -> List[SearchResult]
    - create_collection() returns raw object -> Collection
    """

    def __init__(
        self,
        data_dir: str = "/tmp/proximadb/data",
        config: Optional[Dict[str, Any]] = None,
        **kwargs,
    ):
        """Initialize embedded protocol adapter.

        Args:
            data_dir: Directory for persistent storage
            config: Optional configuration dictionary
            **kwargs: Additional configuration passed to embedded DB
        """
        try:
            # Import the PyO3 bindings
            from ..embedded import EmbeddedProximaDB, EmbeddedConfig

            # Build config
            if config:
                embedded_config = EmbeddedConfig(**config)
            else:
                embedded_config = EmbeddedConfig(data_dir=data_dir, **kwargs)

            # Create the embedded database instance
            self._db = EmbeddedProximaDB(config=embedded_config)
            self._data_dir = data_dir
            self._connected = True
            self._collections: Dict[str, Collection] = {}

        except ImportError as e:
            logger.error(f"Embedded mode not available: {e}")
            raise ImportError(
                "Embedded mode requires the proximadb native extension. "
                "Install with: pip install proximadb[embedded]"
            ) from e

    @property
    def protocol_name(self) -> str:
        """Return the protocol name."""
        return "embedded"

    @property
    def is_connected(self) -> bool:
        """Check if the adapter is connected and operational."""
        return self._connected and self._db is not None

    # ==========================================================================
    # Health & Server Operations
    # ==========================================================================

    def health(self) -> HealthStatus:
        """Check embedded database health status."""
        if not self._connected or self._db is None:
            return HealthStatus(
                status="unhealthy",
                healthy=False,
                timestamp_ms=int(time.time() * 1000),
                services={"embedded": "not initialized"},
            )

        try:
            # Basic health check - list collections to verify DB is operational
            collections = (
                self._db.list_collections()
                if hasattr(self._db, "list_collections")
                else []
            )

            return HealthStatus(
                status="healthy",
                healthy=True,
                timestamp_ms=int(time.time() * 1000),
                services={
                    "embedded": "ok",
                    "collections_count": len(collections) if collections else 0,
                },
            )
        except Exception as e:
            return HealthStatus(
                status="unhealthy",
                healthy=False,
                timestamp_ms=int(time.time() * 1000),
                services={"embedded": str(e)},
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

        # Convert distance metric to string for embedded API
        distance_metric = "cosine"
        if config and config.distance_metric:
            distance_metric = ProtoConverter.distance_metric_to_str(
                config.distance_metric
            )
        elif "distance_metric" in kwargs:
            distance_metric = ProtoConverter.distance_metric_to_str(
                kwargs["distance_metric"]
            )

        # Convert storage engine to string for embedded API
        storage_engine = "sst"
        if config and config.storage_engine:
            storage_engine = ProtoConverter.storage_engine_to_str(config.storage_engine)
        elif "storage_engine" in kwargs or "engine" in kwargs:
            engine = kwargs.get("storage_engine") or kwargs.get("engine")
            storage_engine = ProtoConverter.storage_engine_to_str(engine)

        # Create collection via embedded API
        try:
            result = self._db.create_collection(
                name=name,
                dimension=dimension,
                distance_metric=distance_metric,
                engine=storage_engine,
            )

            # Build Collection model
            collection = Collection(
                id=name,  # Embedded mode uses name as ID
                name=name,
                dimension=dimension,
            )

            # Cache collection for later lookups
            self._collections[name] = collection

            return collection

        except Exception as e:
            logger.error(f"Failed to create collection: {e}")
            raise

    def get_collection(self, collection_id: str) -> Optional[Collection]:
        """Get collection metadata by ID or name."""
        # Check cache first
        if collection_id in self._collections:
            return self._collections[collection_id]

        try:
            if hasattr(self._db, "get_collection"):
                result = self._db.get_collection(collection_id)
                if result:
                    collection = Collection(
                        id=collection_id,
                        name=getattr(result, "name", collection_id),
                        dimension=getattr(result, "dimension", 0),
                    )
                    self._collections[collection_id] = collection
                    return collection

            # Fallback: check if collection exists in list
            collections = self.list_collections()
            for c in collections:
                if c.id == collection_id or c.name == collection_id:
                    return c

            return None
        except Exception as e:
            logger.debug(f"Collection not found: {collection_id} - {e}")
            return None

    def list_collections(self) -> List[Collection]:
        """List all collections."""
        try:
            if hasattr(self._db, "list_collections"):
                results = self._db.list_collections()

                collections = []
                for item in results or []:
                    if isinstance(item, Collection):
                        collections.append(item)
                    elif isinstance(item, str):
                        # Some embedded APIs return just names
                        collection = Collection(id=item, name=item, dimension=0)
                        collections.append(collection)
                    elif hasattr(item, "name"):
                        collection = Collection(
                            id=getattr(item, "id", getattr(item, "name", "")),
                            name=getattr(item, "name", ""),
                            dimension=getattr(item, "dimension", 0),
                        )
                        collections.append(collection)

                return collections

            return list(self._collections.values())
        except Exception as e:
            logger.error(f"Failed to list collections: {e}")
            return []

    def delete_collection(self, collection_id: str) -> bool:
        """Delete a collection."""
        try:
            if hasattr(self._db, "delete_collection"):
                self._db.delete_collection(collection_id)

            # Remove from cache
            self._collections.pop(collection_id, None)
            return True
        except Exception as e:
            logger.error(f"Failed to delete collection: {e}")
            return False

    # ==========================================================================
    # Vector Operations
    # ==========================================================================

    def insert_vectors(
        self,
        collection_id: str,
        vectors: Union[List[VectorRecord], List[Dict[str, Any]]],
        **kwargs,
    ) -> VectorOperationResponse:
        """Insert vectors into a collection.

        The embedded API typically returns an int (count of inserted vectors).
        This method wraps that into a VectorOperationResponse for API consistency.
        """
        start_time = time.time()

        # Convert VectorRecord objects to the format expected by embedded API
        vector_data = []
        for v in vectors:
            if isinstance(v, dict):
                vector_data.append(v)
            elif hasattr(v, "model_dump"):
                vector_data.append(v.model_dump(exclude_none=True))
            else:
                vector_data.append(ProtoConverter.vector_record_to_dict(v))

        try:
            # Call embedded insert - typically returns int count
            result = self._db.insert(collection_id, vector_data)

            duration_ms = (time.time() - start_time) * 1000

            # Handle different return types
            if isinstance(result, int):
                # Embedded API returns count of inserted vectors
                return VectorOperationResponse(
                    success=True,
                    operation="INSERT",
                    metrics=OperationMetrics(
                        successful_count=result,
                        failed_count=len(vectors) - result,
                        duration_ms=duration_ms,
                        total_count=len(vectors),
                    ),
                )
            elif isinstance(result, VectorOperationResponse):
                return result
            else:
                # Assume success if we got here
                return VectorOperationResponse(
                    success=True,
                    operation="INSERT",
                    metrics=OperationMetrics(
                        successful_count=len(vectors),
                        failed_count=0,
                        duration_ms=duration_ms,
                        total_count=len(vectors),
                    ),
                )

        except Exception as e:
            duration_ms = (time.time() - start_time) * 1000
            return VectorOperationResponse(
                success=False,
                operation="INSERT",
                error_message=str(e),
                metrics=OperationMetrics(
                    successful_count=0,
                    failed_count=len(vectors),
                    duration_ms=duration_ms,
                    total_count=len(vectors),
                ),
            )

    def upsert_vectors(
        self,
        collection_id: str,
        vectors: Union[List[VectorRecord], List[Dict[str, Any]]],
        **kwargs,
    ) -> VectorOperationResponse:
        """Upsert (insert or update) vectors in a collection."""
        start_time = time.time()

        # Convert VectorRecord objects
        vector_data = []
        for v in vectors:
            if isinstance(v, dict):
                vector_data.append(v)
            elif hasattr(v, "model_dump"):
                vector_data.append(v.model_dump(exclude_none=True))
            else:
                vector_data.append(ProtoConverter.vector_record_to_dict(v))

        try:
            # Use upsert if available, otherwise insert
            if hasattr(self._db, "upsert"):
                result = self._db.upsert(collection_id, vector_data)
            else:
                result = self._db.insert(collection_id, vector_data)

            duration_ms = (time.time() - start_time) * 1000

            if isinstance(result, int):
                return VectorOperationResponse(
                    success=True,
                    operation="UPSERT",
                    metrics=OperationMetrics(
                        successful_count=result,
                        failed_count=len(vectors) - result,
                        duration_ms=duration_ms,
                        total_count=len(vectors),
                    ),
                )

            return VectorOperationResponse(
                success=True,
                operation="UPSERT",
                metrics=OperationMetrics(
                    successful_count=len(vectors),
                    failed_count=0,
                    duration_ms=duration_ms,
                    total_count=len(vectors),
                ),
            )

        except Exception as e:
            duration_ms = (time.time() - start_time) * 1000
            return VectorOperationResponse(
                success=False,
                operation="UPSERT",
                error_message=str(e),
                metrics=OperationMetrics(
                    successful_count=0,
                    failed_count=len(vectors),
                    duration_ms=duration_ms,
                    total_count=len(vectors),
                ),
            )

    def get_vectors(
        self,
        collection_id: str,
        vector_ids: List[str],
        include_vectors: bool = True,
        **kwargs,
    ) -> List[VectorRecord]:
        """Get vectors by IDs."""
        try:
            if hasattr(self._db, "get"):
                results = self._db.get(
                    collection_id, vector_ids, include_vectors=include_vectors
                )
            elif hasattr(self._db, "get_vectors"):
                results = self._db.get_vectors(
                    collection_id, vector_ids, include_vectors=include_vectors
                )
            else:
                logger.warning("get_vectors not implemented in embedded API")
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
                            vector=(
                                list(getattr(r, "vector", []))
                                if include_vectors
                                else None
                            ),
                            metadata=dict(getattr(r, "metadata", {})),
                        )
                    )

            return records

        except Exception as e:
            logger.error(f"Failed to get vectors: {e}")
            return []

    def delete_vectors(
        self, collection_id: str, vector_ids: List[str], **kwargs
    ) -> VectorOperationResponse:
        """Delete vectors by IDs."""
        start_time = time.time()

        try:
            if hasattr(self._db, "delete"):
                result = self._db.delete(collection_id, vector_ids)
            elif hasattr(self._db, "delete_vectors"):
                result = self._db.delete_vectors(collection_id, vector_ids)
            else:
                return VectorOperationResponse(
                    success=False,
                    operation="DELETE",
                    error_message="delete_vectors not implemented in embedded API",
                )

            duration_ms = (time.time() - start_time) * 1000

            if isinstance(result, int):
                return VectorOperationResponse(
                    success=True,
                    operation="DELETE",
                    metrics=OperationMetrics(
                        successful_count=result,
                        failed_count=len(vector_ids) - result,
                        duration_ms=duration_ms,
                        total_count=len(vector_ids),
                    ),
                )

            return VectorOperationResponse(
                success=True,
                operation="DELETE",
                metrics=OperationMetrics(
                    successful_count=len(vector_ids),
                    failed_count=0,
                    duration_ms=duration_ms,
                    total_count=len(vector_ids),
                ),
            )

        except Exception as e:
            duration_ms = (time.time() - start_time) * 1000
            return VectorOperationResponse(
                success=False,
                operation="DELETE",
                error_message=str(e),
                metrics=OperationMetrics(
                    successful_count=0,
                    failed_count=len(vector_ids),
                    duration_ms=duration_ms,
                    total_count=len(vector_ids),
                ),
            )

    def update_vector_metadata(
        self, collection_id: str, vector_id: str, metadata: MetadataDict, **kwargs
    ) -> VectorOperationResponse:
        """Update metadata for a specific vector."""
        start_time = time.time()

        try:
            if hasattr(self._db, "update_metadata"):
                result = self._db.update_metadata(collection_id, vector_id, metadata)
            else:
                # Fallback: get, update, upsert
                vectors = self.get_vectors(collection_id, [vector_id])
                if vectors:
                    v = vectors[0]
                    updated_meta = (
                        {**v.metadata, **metadata} if v.metadata else metadata
                    )
                    return self.upsert_vectors(
                        collection_id,
                        [
                            VectorRecord(
                                id=vector_id, vector=v.vector, metadata=updated_meta
                            )
                        ],
                    )
                return VectorOperationResponse(
                    success=False,
                    operation="UPDATE",
                    error_message=f"Vector {vector_id} not found",
                )

            duration_ms = (time.time() - start_time) * 1000

            return VectorOperationResponse(
                success=True,
                operation="UPDATE",
                metrics=OperationMetrics(
                    successful_count=1,
                    failed_count=0,
                    duration_ms=duration_ms,
                    total_count=1,
                ),
            )

        except Exception as e:
            duration_ms = (time.time() - start_time) * 1000
            return VectorOperationResponse(
                success=False,
                operation="UPDATE",
                error_message=str(e),
                metrics=OperationMetrics(
                    successful_count=0,
                    failed_count=1,
                    duration_ms=duration_ms,
                    total_count=1,
                ),
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
        """Search for similar vectors.

        The embedded API typically returns a list of tuples (id, score, metadata).
        This method converts them to SearchResult objects.
        """
        # Normalize query vector to list
        if hasattr(query_vector, "tolist"):
            query_vector = query_vector.tolist()
        else:
            query_vector = list(query_vector)

        try:
            # Call embedded search
            results = self._db.search(
                collection_id,
                query_vector,
                k=top_k,
                filter=filter,
                include_vectors=include_vectors,
                include_metadata=include_metadata,
            )

            return self._to_search_results(results, include_vectors, include_metadata)

        except Exception as e:
            logger.error(f"Search failed: {e}")
            return []

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

        try:
            if hasattr(self._db, "batch_search"):
                results = self._db.batch_search(
                    collection_id,
                    normalized_queries,
                    k=top_k,
                    filter=filter,
                    include_vectors=include_vectors,
                    include_metadata=include_metadata,
                )

                # Convert batch results
                batch_results = []
                for query_results in results or []:
                    batch_results.append(
                        self._to_search_results(
                            query_results, include_vectors, include_metadata
                        )
                    )
                return batch_results

            else:
                # Fallback: execute individual searches
                batch_results = []
                for qv in normalized_queries:
                    r = self.search(
                        collection_id,
                        qv,
                        top_k,
                        filter,
                        include_vectors,
                        include_metadata,
                        **kwargs,
                    )
                    batch_results.append(r)
                return batch_results

        except Exception as e:
            logger.error(f"Batch search failed: {e}")
            return [[] for _ in query_vectors]

    def _to_search_results(
        self, results: Any, include_vectors: bool, include_metadata: bool
    ) -> List[SearchResult]:
        """Convert embedded search results to SearchResult list.

        Handles various result formats:
        - List of tuples (id, score, metadata, vector)
        - List of dicts
        - List of objects with attributes
        """
        if results is None:
            return []

        search_results = []
        for r in results:
            try:
                if isinstance(r, SearchResult):
                    search_results.append(r)
                elif isinstance(r, tuple):
                    # Common format: (id, score, metadata, vector) or (id, score)
                    result_id = r[0] if len(r) > 0 else ""
                    score = r[1] if len(r) > 1 else 0.0
                    metadata = r[2] if len(r) > 2 and include_metadata else None
                    vector = r[3] if len(r) > 3 and include_vectors else None

                    search_results.append(
                        SearchResult(
                            id=str(result_id),
                            score=float(score),
                            vector=list(vector) if vector else None,
                            metadata=dict(metadata) if metadata else None,
                        )
                    )
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
            except Exception as e:
                logger.warning(f"Failed to convert search result: {e}")

        return search_results

    # ==========================================================================
    # Lifecycle Methods
    # ==========================================================================

    def close(self) -> None:
        """Close the embedded database."""
        if self._db is not None:
            try:
                if hasattr(self._db, "close"):
                    self._db.close()
                elif hasattr(self._db, "shutdown"):
                    self._db.shutdown()
            except Exception as e:
                logger.warning(f"Error closing embedded database: {e}")
            finally:
                self._db = None
                self._connected = False
                self._collections.clear()
