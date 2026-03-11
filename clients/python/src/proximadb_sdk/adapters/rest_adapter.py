"""
ProximaDB REST Protocol Adapter

Wraps the REST protocol client to implement the BaseProtocolAdapter interface.
Converts REST responses to standardized Pydantic models.

Copyright 2025 ProximaDB Contributors
Licensed under the Apache License, Version 2.0
"""

import logging
import time
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


class RestProtocolAdapter(BaseProtocolAdapter):
    """REST protocol adapter implementing BaseProtocolAdapter.

    Wraps the existing ProximaDBClient (REST) to provide a consistent
    interface that returns Pydantic models.
    """

    def __init__(
        self,
        url: str = "http://localhost:5678",
        api_key: Optional[str] = None,
        timeout: float = 30.0,
        **kwargs,
    ):
        """Initialize REST protocol adapter.

        Args:
            url: ProximaDB REST server URL
            api_key: Optional API key for authentication
            timeout: Request timeout in seconds
            **kwargs: Additional configuration passed to underlying client
        """
        from ..config import ClientConfig, load_config
        from ..protocols.rest_sync import ProximaDBClient

        # Load config with provided parameters
        config = load_config(url=url, api_key=api_key, timeout=timeout, **kwargs)

        # Create the underlying REST client
        self._client = ProximaDBClient(config=config)
        self._url = url
        self._connected = True

    @property
    def protocol_name(self) -> str:
        """Return the protocol name."""
        return "rest"

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
            result = self._client.health()
            # Result is already a HealthStatus from the underlying client
            if isinstance(result, HealthStatus):
                return result
            # If it's a dict, convert to HealthStatus
            return HealthStatus(**result) if isinstance(result, dict) else result
        except Exception as e:
            logger.error(f"Health check failed: {e}")
            return HealthStatus(
                status="running",
                version="0.0.0",
                uptime_seconds=0,
                timestamp_ms=int(time.time() * 1000),
                services={"rest": "unavailable"},
            )

    # ==========================================================================
    # Collection Operations
    # ==========================================================================

    def create_collection(
        self, name: str, config: Optional[CollectionConfig] = None, **kwargs
    ) -> Collection:
        """Create a new vector collection."""
        result = self._client.create_collection(name=name, config=config, **kwargs)

        # The REST client already returns a Collection model
        if isinstance(result, Collection):
            return result

        # Convert dict to Collection if needed
        if isinstance(result, dict):
            return Collection(**result)

        # Handle wrapper objects
        if hasattr(result, "name") and hasattr(result, "id"):
            return Collection(
                id=getattr(result, "id", ""),
                name=getattr(result, "name", name),
                dimension=getattr(
                    result, "dimension", config.dimension if config else 0
                ),
            )

        return result

    def get_collection(self, collection_id: str) -> Optional[Collection]:
        """Get collection metadata by ID or name."""
        try:
            result = self._client.get_collection(collection_id)

            if result is None:
                return None

            if isinstance(result, Collection):
                return result

            if isinstance(result, dict):
                return Collection(**result)

            # Handle wrapper objects
            if hasattr(result, "name"):
                return Collection(
                    id=getattr(result, "id", collection_id),
                    name=getattr(result, "name", ""),
                    dimension=getattr(result, "dimension", 0),
                )

            return result
        except Exception as e:
            logger.debug(f"Collection not found: {collection_id} - {e}")
            return None

    def list_collections(self) -> List[Collection]:
        """List all collections."""
        try:
            results = self._client.list_collections()
        except Exception as e:
            logger.error(f"Failed to list collections: {e}")
            return []

        collections = []
        for item in results:
            if isinstance(item, Collection):
                collections.append(item)
            elif isinstance(item, dict):
                collections.append(Collection(**item))
            elif hasattr(item, "name"):
                collections.append(
                    Collection(
                        id=getattr(item, "id", ""),
                        name=getattr(item, "name", ""),
                        dimension=getattr(item, "dimension", 0),
                    )
                )

        return collections

    def delete_collection(self, collection_id: str) -> bool:
        """Delete a collection."""
        try:
            result = self._client.delete_collection(collection_id)
            if isinstance(result, bool):
                return result
            if hasattr(result, "success"):
                return result.success
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
        """Insert vectors into a collection."""
        # Convert VectorRecord objects to dicts if needed
        vector_dicts = []
        for v in vectors:
            if isinstance(v, dict):
                vector_dicts.append(v)
            elif hasattr(v, "model_dump"):
                vector_dicts.append(v.model_dump(exclude_none=True))
            else:
                vector_dicts.append(ProtoConverter.vector_record_to_dict(v))

        result = self._client.insert_vectors(collection_id, vector_dicts, **kwargs)

        if isinstance(result, VectorOperationResponse):
            return result

        # Convert dict or other response to VectorOperationResponse
        if isinstance(result, dict):
            return VectorOperationResponse(
                success=result.get("success", True),
                operation="INSERT",
                metrics=OperationMetrics(
                    successful_count=result.get("successful_count", len(vectors)),
                    failed_count=result.get("failed_count", 0),
                    total_count=len(vectors),
                ),
            )

        # Handle wrapper objects
        return VectorOperationResponse(
            success=getattr(result, "success", True),
            operation="INSERT",
            metrics=OperationMetrics(
                successful_count=len(vectors),
                failed_count=0,
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
        # Convert VectorRecord objects to dicts if needed
        vector_dicts = []
        for v in vectors:
            if isinstance(v, dict):
                vector_dicts.append(v)
            elif hasattr(v, "model_dump"):
                vector_dicts.append(v.model_dump(exclude_none=True))
            else:
                vector_dicts.append(ProtoConverter.vector_record_to_dict(v))

        # Use upsert method if available, otherwise insert with upsert flag
        if hasattr(self._client, "upsert_vectors"):
            result = self._client.upsert_vectors(collection_id, vector_dicts, **kwargs)
        else:
            result = self._client.insert_vectors(
                collection_id, vector_dicts, upsert=True, **kwargs
            )

        if isinstance(result, VectorOperationResponse):
            return result

        return VectorOperationResponse(
            success=(
                getattr(result, "success", True) if hasattr(result, "success") else True
            ),
            operation="UPSERT",
            metrics=OperationMetrics(
                successful_count=len(vectors),
                failed_count=0,
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
        if hasattr(self._client, "get_vectors"):
            results = self._client.get_vectors(
                collection_id, vector_ids, include_vectors=include_vectors, **kwargs
            )
        else:
            # Fallback: fetch one by one if batch get not available
            results = []
            for vid in vector_ids:
                try:
                    v = self._client.get_vector(collection_id, vid)
                    if v:
                        results.append(v)
                except Exception:
                    pass

        # Convert to VectorRecord list
        records = []
        for r in results:
            if isinstance(r, VectorRecord):
                records.append(r)
            elif isinstance(r, dict):
                records.append(VectorRecord(**r))
            elif hasattr(r, "id"):
                records.append(
                    VectorRecord(
                        id=getattr(r, "id", ""),
                        vector=list(getattr(r, "vector", [])),
                        metadata=getattr(r, "metadata", {}),
                    )
                )

        return records

    def delete_vectors(
        self, collection_id: str, vector_ids: List[str], **kwargs
    ) -> VectorOperationResponse:
        """Delete vectors by IDs."""
        result = self._client.delete_vectors(collection_id, vector_ids, **kwargs)

        if isinstance(result, VectorOperationResponse):
            return result

        return VectorOperationResponse(
            success=(
                getattr(result, "success", True) if hasattr(result, "success") else True
            ),
            operation="DELETE",
            metrics=OperationMetrics(
                successful_count=len(vector_ids),
                failed_count=0,
                total_count=len(vector_ids),
            ),
        )

    def update_vector_metadata(
        self, collection_id: str, vector_id: str, metadata: MetadataDict, **kwargs
    ) -> VectorOperationResponse:
        """Update metadata for a specific vector."""
        if hasattr(self._client, "update_vector_metadata"):
            result = self._client.update_vector_metadata(
                collection_id, vector_id, metadata, **kwargs
            )
        elif hasattr(self._client, "update_metadata"):
            result = self._client.update_metadata(
                collection_id, vector_id, metadata, **kwargs
            )
        else:
            # Fallback: fetch, update, upsert
            vectors = self.get_vectors(collection_id, [vector_id])
            if vectors:
                v = vectors[0]
                updated_meta = {**v.metadata, **metadata} if v.metadata else metadata
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

        if isinstance(result, VectorOperationResponse):
            return result

        return VectorOperationResponse(
            success=True,
            operation="UPDATE",
            metrics=OperationMetrics(successful_count=1, total_count=1),
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

        results = self._client.search(
            collection_id=collection_id,
            query_vector=query_vector,
            top_k=top_k,
            metadata_filters=filter,
            include_vectors=include_vectors,
            include_metadata=include_metadata,
            **kwargs,
        )

        # Convert to SearchResult list
        search_results = []
        for r in results or []:
            if isinstance(r, SearchResult):
                search_results.append(r)
            elif isinstance(r, dict):
                search_results.append(
                    SearchResult(
                        id=r.get("id", r.get("vector_id", "")),
                        score=r.get("score", r.get("distance", 0.0)),
                        vector=r.get("vector", []) if include_vectors else None,
                        metadata=r.get("metadata", {}) if include_metadata else None,
                    )
                )
            elif hasattr(r, "id"):
                search_results.append(
                    SearchResult(
                        id=getattr(r, "id", ""),
                        score=getattr(r, "score", getattr(r, "distance", 0.0)),
                        vector=(
                            list(getattr(r, "vector", [])) if include_vectors else None
                        ),
                        metadata=(
                            getattr(r, "metadata", {}) if include_metadata else None
                        ),
                    )
                )

        return search_results

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

        if hasattr(self._client, "batch_search"):
            results = self._client.batch_search(
                collection_id=collection_id,
                query_vectors=normalized_queries,
                top_k=top_k,
                metadata_filters=filter,
                include_vectors=include_vectors,
                include_metadata=include_metadata,
                **kwargs,
            )
        else:
            # Fallback: execute individual searches
            results = []
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
                results.append(r)
            return results

        # Convert results
        batch_results = []
        for query_results in results or []:
            search_results = []
            for r in query_results or []:
                if isinstance(r, SearchResult):
                    search_results.append(r)
                elif isinstance(r, dict):
                    search_results.append(
                        SearchResult(
                            id=r.get("id", ""),
                            score=r.get("score", 0.0),
                            vector=r.get("vector") if include_vectors else None,
                            metadata=r.get("metadata") if include_metadata else None,
                        )
                    )
            batch_results.append(search_results)

        return batch_results

    # ==========================================================================
    # Document Operations
    # ==========================================================================

    def create_document_collection(
        self, name: str, config: Optional[Dict[str, Any]] = None, **kwargs
    ) -> Dict[str, Any]:
        """Create a document collection via REST."""
        try:
            import requests
            response = self._client._session.post(
                f"{self._url}/api/v1/documents/collections",
                json={"name": name, **(config or {})},
                timeout=self._client._timeout,
            )
            response.raise_for_status()
            return response.json()
        except Exception as e:
            logger.error(f"Failed to create document collection: {e}")
            raise

    def insert_document(
        self, collection_name: str, document: Dict[str, Any], id: Optional[str] = None, **kwargs
    ) -> Dict[str, Any]:
        """Insert a document via REST."""
        try:
            import requests
            response = self._client._session.post(
                f"{self._url}/api/v1/documents/collections/{collection_name}/documents",
                json={"id": id, "document": document},
                timeout=self._client._timeout,
            )
            response.raise_for_status()
            return response.json()
        except Exception as e:
            logger.error(f"Failed to insert document: {e}")
            raise

    def get_document(
        self, collection_name: str, doc_id: str, projection: Optional[List[str]] = None, **kwargs
    ) -> Optional[Dict[str, Any]]:
        """Get a document by ID via REST."""
        try:
            import requests
            params = {}
            if projection:
                params["projection"] = ",".join(projection)
            response = self._client._session.get(
                f"{self._url}/api/v1/documents/collections/{collection_name}/documents/{doc_id}",
                params=params,
                timeout=self._client._timeout,
            )
            if response.status_code == 404:
                return None
            response.raise_for_status()
            return response.json()
        except Exception as e:
            logger.debug(f"Document not found: {doc_id} - {e}")
            return None

    def query_documents(
        self,
        collection_name: str,
        filter: Optional[Dict[str, Any]] = None,
        projection: Optional[List[str]] = None,
        limit: int = 100,
        **kwargs,
    ) -> Dict[str, Any]:
        """Query documents with filter via REST."""
        try:
            import requests
            body = {}
            if filter:
                body["filter"] = filter
            if projection:
                body["projection"] = projection
            body["limit"] = limit
            response = self._client._session.post(
                f"{self._url}/api/v1/documents/collections/{collection_name}/documents",
                json=body,
                timeout=self._client._timeout,
            )
            response.raise_for_status()
            return response.json()
        except Exception as e:
            logger.error(f"Failed to query documents: {e}")
            raise

    def update_document(
        self, collection_name: str, doc_id: str, updates: List[Dict[str, Any]], **kwargs
    ) -> Dict[str, Any]:
        """Update a document via REST."""
        try:
            import requests
            response = self._client._session.put(
                f"{self._url}/api/v1/documents/collections/{collection_name}/documents/{doc_id}",
                json={"updates": updates},
                timeout=self._client._timeout,
            )
            response.raise_for_status()
            return response.json()
        except Exception as e:
            logger.error(f"Failed to update document: {e}")
            raise

    def delete_document(self, collection_name: str, doc_id: str, **kwargs) -> bool:
        """Delete a document via REST."""
        try:
            import requests
            response = self._client._session.delete(
                f"{self._url}/api/v1/documents/collections/{collection_name}/documents/{doc_id}",
                timeout=self._client._timeout,
            )
            response.raise_for_status()
            result = response.json()
            return result.get("deleted", False)
        except Exception as e:
            logger.error(f"Failed to delete document: {e}")
            return False

    def list_document_collections(self, **kwargs) -> List[Dict[str, Any]]:
        """List all document collections via REST."""
        try:
            import requests
            response = self._client._session.get(
                f"{self._url}/api/v1/documents/collections",
                timeout=self._client._timeout,
            )
            response.raise_for_status()
            return response.json().get("collections", [])
        except Exception as e:
            logger.error(f"Failed to list document collections: {e}")
            return []

    def delete_document_collection(self, collection_name: str, **kwargs) -> bool:
        """Delete a document collection via REST."""
        try:
            import requests
            response = self._client._session.delete(
                f"{self._url}/api/v1/documents/collections/{collection_name}",
                timeout=self._client._timeout,
            )
            response.raise_for_status()
            result = response.json()
            return result.get("success", False)
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
        """Execute hybrid search via REST."""
        try:
            import requests
            response = self._client._session.post(
                f"{self._url}/api/v1/hybrid/search",
                json={
                    "collection": collection,
                    "text_query": text_query,
                    "query_vector": query_vector,
                    "fusion_strategy": fusion_strategy,
                    "top_k": top_k,
                },
                timeout=self._client._timeout,
            )
            response.raise_for_status()
            return response.json()
        except Exception as e:
            logger.error(f"Hybrid search failed: {e}")
            raise

    # ==========================================================================
    # Time-Series Operations
    # ==========================================================================

    def create_timeseries_collection(
        self, name: str, config: Optional[Dict[str, Any]] = None, **kwargs
    ) -> Dict[str, Any]:
        """Create a time-series collection via REST."""
        try:
            import requests
            response = self._client._session.post(
                f"{self._url}/api/v1/timeseries/collections",
                json={"name": name, **(config or {})},
                timeout=self._client._timeout,
            )
            response.raise_for_status()
            return response.json()
        except Exception as e:
            logger.error(f"Failed to create time-series collection: {e}")
            raise

    def ingest_timeseries(
        self, collection_name: str, points: List[Dict[str, Any]], **kwargs
    ) -> Dict[str, Any]:
        """Ingest time-series data points via REST."""
        try:
            import requests
            response = self._client._session.post(
                f"{self._url}/api/v1/timeseries/collections/{collection_name}/ingest",
                json={"points": points},
                timeout=self._client._timeout,
            )
            response.raise_for_status()
            return response.json()
        except Exception as e:
            logger.error(f"Failed to ingest time-series data: {e}")
            raise

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
        """Query time-series data with optional aggregation via REST."""
        try:
            import requests
            body = {
                "start_time": start_time,
                "end_time": end_time,
                "aggregation": aggregation,
            }
            if bucket_ms:
                body["bucket_ms"] = bucket_ms
            if tag_filters:
                body["tag_filters"] = tag_filters
            response = self._client._session.post(
                f"{self._url}/api/v1/timeseries/collections/{collection_name}/query",
                json=body,
                timeout=self._client._timeout,
            )
            response.raise_for_status()
            return response.json()
        except Exception as e:
            logger.error(f"Failed to query time-series data: {e}")
            raise

    def list_timeseries_collections(self, **kwargs) -> List[Dict[str, Any]]:
        """List all time-series collections via REST."""
        try:
            import requests
            response = self._client._session.get(
                f"{self._url}/api/v1/timeseries/collections",
                timeout=self._client._timeout,
            )
            response.raise_for_status()
            return response.json().get("collections", [])
        except Exception as e:
            logger.error(f"Failed to list time-series collections: {e}")
            return []

    def delete_timeseries_collection(self, collection_name: str, **kwargs) -> bool:
        """Delete a time-series collection via REST."""
        try:
            import requests
            response = self._client._session.delete(
                f"{self._url}/api/v1/timeseries/collections/{collection_name}",
                timeout=self._client._timeout,
            )
            response.raise_for_status()
            result = response.json()
            return result.get("success", False)
        except Exception as e:
            logger.error(f"Failed to delete time-series collection: {e}")
            return False

    # ==========================================================================
    # Lifecycle Methods
    # ==========================================================================

    def close(self) -> None:
        """Close the REST client connection."""
        if hasattr(self._client, "close"):
            self._client.close()
        self._connected = False
