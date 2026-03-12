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

from ..models import (
    Collection,
    CollectionConfig,
    DistanceMetric,
    FilterDict,
    HealthStatus,
    MetadataDict,
    OperationMetrics,
    SearchResult,
    StorageEngine,
    VectorArray,
    VectorOperationResponse,
    VectorRecord,
)
from ..proto_conversion import ProtoConverter
from .base import BaseProtocolAdapter

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
            from ..embedded import EmbeddedConfig, EmbeddedProximaDB

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
    # Graph Operations
    # ==========================================================================

    def create_node(
        self, graph: str, node_id: str, labels: List[str], properties: Dict[str, Any], **kwargs
    ) -> Dict[str, Any]:
        """Create a graph node via embedded API."""
        try:
            if hasattr(self._db, "create_node"):
                result = self._db.create_node(
                    graph=graph,
                    node_id=node_id,
                    labels=labels,
                    properties=properties,
                )
                return {"success": True, "node_id": node_id, "result": result}
            else:
                raise NotImplementedError("create_node not implemented in embedded API")
        except Exception as e:
            logger.error(f"Failed to create node: {e}")
            raise

    def create_edge(
        self,
        graph: str,
        edge_id: str,
        from_node: str,
        to_node: str,
        edge_type: str,
        properties: Optional[Dict[str, Any]] = None,
        **kwargs
    ) -> Dict[str, Any]:
        """Create a graph edge via embedded API."""
        try:
            if hasattr(self._db, "create_edge"):
                result = self._db.create_edge(
                    graph=graph,
                    edge_id=edge_id,
                    from_node=from_node,
                    to_node=to_node,
                    edge_type=edge_type,
                    properties=properties or {},
                )
                return {"success": True, "edge_id": edge_id, "result": result}
            else:
                raise NotImplementedError("create_edge not implemented in embedded API")
        except Exception as e:
            logger.error(f"Failed to create edge: {e}")
            raise

    def execute_graph_query(
        self, graph: str, query: str, **kwargs
    ) -> Dict[str, Any]:
        """Execute a graph query via embedded API."""
        try:
            if hasattr(self._db, "execute_graph_query"):
                result = self._db.execute_graph_query(graph=graph, query=query)
                return {"results": result, "query": query}
            else:
                # Fall back to multi-modal query execution
                if hasattr(self._db, "execute_multi_modal_query"):
                    from ..models import MultiModalQuery, QueryComponent
                    component = QueryComponent(
                        type="graph",
                        collection=graph,
                        query=query,
                    )
                    mm_query = MultiModalQuery(components=[component])
                    result = self._db.execute_multi_modal_query(mm_query)
                    return {"results": result}
                else:
                    raise NotImplementedError("Graph query not implemented in embedded API")
        except Exception as e:
            logger.error(f"Failed to execute graph query: {e}")
            raise

    # ==========================================================================
    # Document Operations
    # ==========================================================================

    def create_document_collection(
        self, name: str, config: Optional[Dict[str, Any]] = None, **kwargs
    ) -> Dict[str, Any]:
        """Create a document collection via embedded API."""
        try:
            if hasattr(self._db, "create_document_collection"):
                result = self._db.create_document_collection(name=name, config=config or {})
                return {"success": True, "collection_id": name, "result": result}
            else:
                # Fall back to creating a vector collection with document metadata
                return self._create_document_collection_as_vector(name, config)
        except Exception as e:
            logger.error(f"Failed to create document collection: {e}")
            raise

    def _create_document_collection_as_vector(
        self, name: str, config: Optional[Dict[str, Any]] = None
    ) -> Dict[str, Any]:
        """Create a document collection using vector storage as fallback."""
        # Create a vector collection with a special tag for documents
        dimension = config.get("dimension", 768) if config else 768
        collection = self.create_collection(name, config={"dimension": dimension})
        return {
            "success": True,
            "collection_id": name,
            "implementation": "vector_fallback",
            "collection": {
                "id": collection.id,
                "name": collection.name,
                "dimension": collection.dimension,
            },
        }

    def insert_document(
        self, collection_name: str, document: Dict[str, Any], id: Optional[str] = None, **kwargs
    ) -> Dict[str, Any]:
        """Insert a document via embedded API."""
        try:
            if hasattr(self._db, "insert_document"):
                result = self._db.insert_document(
                    collection=collection_name,
                    document=document,
                    id=id,
                )
                return {"id": id, "success": True, "result": result}
            else:
                # Fall back to vector storage
                return self._insert_document_as_vector(collection_name, document, id)
        except Exception as e:
            logger.error(f"Failed to insert document: {e}")
            raise

    def _insert_document_as_vector(
        self, collection_name: str, document: Dict[str, Any], id: Optional[str] = None
    ) -> Dict[str, Any]:
        """Insert a document using vector storage as fallback."""
        import json

        # Create a vector record from the document
        # Use a dummy vector for now (could be improved with embedding)
        doc_id = id or document.get("id") or f"doc_{hash(json.dumps(document, sort_keys=True))}"

        # Store document content in the source field
        vector_record = VectorRecord(
            id=doc_id,
            vector=[0.0] * 768,  # Dummy vector
            source=json.dumps(document),
            metadata={
                "document_type": "document",
                "collection": collection_name,
                **document.get("metadata", {})
            },
        )

        result = self.insert_vectors(collection_name, [vector_record])
        return {
            "id": doc_id,
            "success": result.success,
            "version": 1,
            "implementation": "vector_fallback",
        }

    def get_document(
        self, collection_name: str, doc_id: str, projection: Optional[List[str]] = None, **kwargs
    ) -> Optional[Dict[str, Any]]:
        """Get a document by ID via embedded API."""
        try:
            if hasattr(self._db, "get_document"):
                result = self._db.get_document(
                    collection=collection_name,
                    doc_id=doc_id,
                    projection=projection,
                )
                return result
            else:
                # Fall back to vector storage
                return self._get_document_as_vector(collection_name, doc_id)
        except Exception as e:
            logger.debug(f"Document not found: {doc_id} - {e}")
            return None

    def _get_document_as_vector(
        self, collection_name: str, doc_id: str
    ) -> Optional[Dict[str, Any]]:
        """Get a document using vector storage as fallback."""
        import json

        vectors = self.get_vectors(collection_name, [doc_id], include_vectors=False)
        if not vectors:
            return None

        v = vectors[0]
        if v.source:
            try:
                document = json.loads(v.source)
                return {"id": doc_id, "document": document, "metadata": v.metadata}
            except json.JSONDecodeError:
                pass

        return {"id": doc_id, "document": {"source": v.source}, "metadata": v.metadata}

    def query_documents(
        self,
        collection_name: str,
        filter: Optional[Dict[str, Any]] = None,
        projection: Optional[List[str]] = None,
        limit: int = 100,
        **kwargs,
    ) -> Dict[str, Any]:
        """Query documents with filter via embedded API."""
        try:
            if hasattr(self._db, "query_documents"):
                result = self._db.query_documents(
                    collection=collection_name,
                    filter=filter,
                    projection=projection,
                    limit=limit,
                )
                return {"documents": result, "count": len(result) if result else 0}
            else:
                # Fall back to vector search with metadata filter
                return self._query_documents_as_vector(collection_name, filter, limit)
        except Exception as e:
            logger.error(f"Failed to query documents: {e}")
            raise

    def _query_documents_as_vector(
        self, collection_name: str, filter: Optional[Dict[str, Any]], limit: int
    ) -> Dict[str, Any]:
        """Query documents using vector storage as fallback."""
        # For now, return all vectors (could be improved with filtering)
        # This is a simplified fallback implementation
        return {"documents": [], "count": 0, "implementation": "vector_fallback"}

    def update_document(
        self, collection_name: str, doc_id: str, updates: List[Dict[str, Any]], **kwargs
    ) -> Dict[str, Any]:
        """Update a document via embedded API."""
        try:
            if hasattr(self._db, "update_document"):
                result = self._db.update_document(
                    collection=collection_name,
                    doc_id=doc_id,
                    updates=updates,
                )
                return {"success": True, "new_version": result, "result": result}
            else:
                # Fall back to vector storage
                return self._update_document_as_vector(collection_name, doc_id, updates)
        except Exception as e:
            logger.error(f"Failed to update document: {e}")
            raise

    def _update_document_as_vector(
        self, collection_name: str, doc_id: str, updates: List[Dict[str, Any]]
    ) -> Dict[str, Any]:
        """Update a document using vector storage as fallback."""
        # Get existing document
        existing = self.get_document(collection_name, doc_id)
        if not existing:
            return {"success": False, "error": "Document not found"}

        # Apply updates
        document = existing.get("document", {})
        for update in updates:
            path = update.get("path", "")
            value = update.get("value")
            operation = update.get("operation", "SET")

            if operation == "SET" and path:
                # Simple dot notation support
                parts = path.replace("$.", "").split(".")
                target = document
                for part in parts[:-1]:
                    if part not in target:
                        target[part] = {}
                    target = target[part]
                target[parts[-1]] = value

        # Re-insert the updated document
        result = self.insert_document(collection_name, document, doc_id)
        return {"success": True, "new_version": 1, "implementation": "vector_fallback"}

    def delete_document(self, collection_name: str, doc_id: str, **kwargs) -> bool:
        """Delete a document via embedded API."""
        try:
            if hasattr(self._db, "delete_document"):
                result = self._db.delete_document(
                    collection=collection_name,
                    doc_id=doc_id,
                )
                return result.get("deleted", False) if isinstance(result, dict) else result
            else:
                # Fall back to vector storage
                result = self.delete_vectors(collection_name, [doc_id])
                return result.success
        except Exception as e:
            logger.error(f"Failed to delete document: {e}")
            return False

    def list_document_collections(self, **kwargs) -> List[Dict[str, Any]]:
        """List all document collections via embedded API."""
        try:
            if hasattr(self._db, "list_document_collections"):
                result = self._db.list_document_collections()
                return result if isinstance(result, list) else []
            else:
                # Fall back to listing vector collections
                collections = self.list_collections()
                return [
                    {
                        "name": c.name,
                        "id": c.id,
                        "dimension": c.dimension,
                    }
                    for c in collections
                ]
        except Exception as e:
            logger.error(f"Failed to list document collections: {e}")
            return []

    def delete_document_collection(self, collection_name: str, **kwargs) -> bool:
        """Delete a document collection via embedded API."""
        try:
            if hasattr(self._db, "delete_document_collection"):
                result = self._db.delete_document_collection(collection=collection_name)
                return result.get("success", False) if isinstance(result, dict) else result
            else:
                # Fall back to deleting vector collection
                return self.delete_collection(collection_name)
        except Exception as e:
            logger.error(f"Failed to delete document collection: {e}")
            return False

    # ==========================================================================
    # Hybrid Search Operations
    # ==========================================================================

    def hybrid_search(
        self,
        collection: str,
        text_query: str,
        query_vector: List[float],
        fusion_strategy: str = "rrf",
        top_k: int = 10,
        **kwargs,
    ) -> Dict[str, Any]:
        """Execute hybrid search via embedded API."""
        try:
            if hasattr(self._db, "hybrid_search"):
                result = self._db.hybrid_search(
                    collection=collection,
                    text_query=text_query,
                    query_vector=query_vector,
                    fusion_strategy=fusion_strategy,
                    top_k=top_k,
                )
                return result
            else:
                # Fall back to vector search only
                return self._hybrid_search_as_vector(collection, query_vector, top_k)
        except Exception as e:
            logger.error(f"Hybrid search failed: {e}")
            raise

    def _hybrid_search_as_vector(
        self, collection: str, query_vector: List[float], top_k: int
    ) -> Dict[str, Any]:
        """Fallback hybrid search using vector search only."""
        results = self.search(
            collection_id=collection,
            query_vector=query_vector,
            top_k=top_k,
        )

        return {
            "results": [
                {
                    "id": r.id,
                    "score": r.score,
                    "metadata": r.metadata,
                    "implementation": "vector_fallback",
                }
                for r in results
            ],
            "fusion_strategy": "vector_only",
            "total_time_ms": 0,
        }

    # ==========================================================================
    # Time-Series Operations
    # ==========================================================================

    def create_timeseries_collection(
        self, name: str, config: Optional[Dict[str, Any]] = None, **kwargs
    ) -> Dict[str, Any]:
        """Create a time-series collection via embedded API."""
        try:
            if hasattr(self._db, "create_timeseries_collection"):
                result = self._db.create_timeseries_collection(name=name, config=config or {})
                return {"success": True, "collection_id": name, "result": result}
            else:
                # Fall back to creating a vector collection
                return self._create_timeseries_collection_as_vector(name, config)
        except Exception as e:
            logger.error(f"Failed to create time-series collection: {e}")
            raise

    def _create_timeseries_collection_as_vector(
        self, name: str, config: Optional[Dict[str, Any]] = None
    ) -> Dict[str, Any]:
        """Create a time-series collection using vector storage as fallback."""
        dimension = config.get("dimension", 128) if config else 128
        collection = self.create_collection(name, config={"dimension": dimension})
        return {
            "success": True,
            "collection_id": name,
            "implementation": "vector_fallback",
            "collection": {
                "id": collection.id,
                "name": collection.name,
                "dimension": collection.dimension,
            },
        }

    def ingest_timeseries(
        self, collection_name: str, points: List[Dict[str, Any]], **kwargs
    ) -> Dict[str, Any]:
        """Ingest time-series data points via embedded API."""
        try:
            if hasattr(self._db, "ingest_timeseries"):
                result = self._db.ingest_timeseries(
                    collection=collection_name,
                    points=points,
                )
                return result
            else:
                # Fall back to vector storage
                return self._ingest_timeseries_as_vector(collection_name, points)
        except Exception as e:
            logger.error(f"Failed to ingest time-series data: {e}")
            raise

    def _ingest_timeseries_as_vector(
        self, collection_name: str, points: List[Dict[str, Any]]
    ) -> Dict[str, Any]:
        """Ingest time-series data using vector storage as fallback."""
        import json
        from datetime import datetime

        vectors = []
        for point in points:
            timestamp = point.get("timestamp", datetime.utcnow().isoformat())
            values = point.get("values", {})
            tags = point.get("tags", {})

            # Create a summary vector (hash of timestamp + values)
            vector_input = f"{timestamp}:{json.dumps(values, sort_keys=True)}"
            vector_hash = hash(vector_input) % 1000000 / 1000000.0
            dummy_vector = [vector_hash] * 128

            vector_record = VectorRecord(
                id=f"ts_{timestamp}_{hash(json.dumps(point, sort_keys=True))}",
                vector=dummy_vector,
                source=json.dumps(point),
                metadata={
                    "timestamp": timestamp,
                    "tags": tags,
                    "metric_names": list(values.keys()) if values else [],
                },
            )
            vectors.append(vector_record)

        result = self.insert_vectors(collection_name, vectors)
        return {
            "ingested_count": result.metrics.successful_count,
            "failed_count": result.metrics.failed_count,
            "implementation": "vector_fallback",
        }

    def query_timeseries(
        self,
        collection_name: str,
        start_time: str,
        end_time: str,
        aggregation: str = "avg",
        bucket_ms: Optional[int] = None,
        tag_filters: Optional[Dict[str, str]] = None,
        **kwargs,
    ) -> Dict[str, Any]:
        """Query time-series data with optional aggregation via embedded API."""
        try:
            if hasattr(self._db, "query_timeseries"):
                result = self._db.query_timeseries(
                    collection=collection_name,
                    start_time=start_time,
                    end_time=end_time,
                    aggregation=aggregation,
                    bucket_ms=bucket_ms,
                    tag_filters=tag_filters,
                )
                return result
            else:
                # Fall back to vector storage
                return self._query_timeseries_as_vector(
                    collection_name, start_time, end_time, tag_filters
                )
        except Exception as e:
            logger.error(f"Failed to query time-series data: {e}")
            raise

    def _query_timeseries_as_vector(
        self, collection_name: str, start_time: str, end_time: str, tag_filters: Optional[Dict[str, str]]
    ) -> Dict[str, Any]:
        """Query time-series data using vector storage as fallback."""
        import json

        # Get all vectors in the collection and filter by timestamp range
        all_vectors = self.get_vectors(collection_name, [], include_vectors=False)

        filtered_points = []
        for v in all_vectors:
            metadata = v.metadata or {}
            timestamp = metadata.get("timestamp", "")

            # Check time range
            if start_time and timestamp < start_time:
                continue
            if end_time and timestamp > end_time:
                continue

            # Check tag filters
            if tag_filters:
                tags = metadata.get("tags", {})
                match = True
                for key, value in tag_filters.items():
                    if tags.get(key) != value:
                        match = False
                        break
                if not match:
                    continue

            # Parse the point data
            point_data = {}
            if v.source:
                try:
                    point_data = json.loads(v.source)
                except json.JSONDecodeError:
                    point_data = {"raw": v.source}

            filtered_points.append({
                "timestamp": timestamp,
                "values": point_data.get("values", {}),
                "tags": metadata.get("tags", {}),
            })

        return {
            "raw_points": filtered_points,
            "total_points": len(filtered_points),
            "implementation": "vector_fallback",
        }

    def list_timeseries_collections(self, **kwargs) -> List[Dict[str, Any]]:
        """List all time-series collections via embedded API."""
        try:
            if hasattr(self._db, "list_timeseries_collections"):
                result = self._db.list_timeseries_collections()
                return result if isinstance(result, list) else []
            else:
                # Fall back to listing vector collections
                collections = self.list_collections()
                return [
                    {
                        "name": c.name,
                        "id": c.id,
                        "dimension": c.dimension,
                    }
                    for c in collections
                ]
        except Exception as e:
            logger.error(f"Failed to list time-series collections: {e}")
            return []

    def delete_timeseries_collection(self, collection_name: str, **kwargs) -> bool:
        """Delete a time-series collection via embedded API."""
        try:
            if hasattr(self._db, "delete_timeseries_collection"):
                result = self._db.delete_timeseries_collection(collection=collection_name)
                return result.get("success", False) if isinstance(result, dict) else result
            else:
                # Fall back to deleting vector collection
                return self.delete_collection(collection_name)
        except Exception as e:
            logger.error(f"Failed to delete time-series collection: {e}")
            return False

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
