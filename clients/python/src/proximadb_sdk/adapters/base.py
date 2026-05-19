"""
ProximaDB Adapter Base Class

Abstract base class defining the interface for transport and embedded adapters.
Enables consistent API regardless of whether calls use REST, gRPC, or direct
in-process embedded bindings.

Copyright 2025 ProximaDB Contributors
Licensed under the Apache License, Version 2.0
"""

from abc import ABC, abstractmethod
from typing import Any, Dict, List, Optional, Union

from ..models import (
    BatchResult,
    Collection,
    CollectionConfig,
    FilterDict,
    HealthStatus,
    MetadataDict,
    SearchResult,
    VectorArray,
    VectorOperationResponse,
    VectorRecord,
)
from ..models_v2 import ProximaRecord


class BaseProtocolAdapter(ABC):
    """Abstract base class for SDK adapters.

    Adapters encapsulate transport-specific or embedded binding logic, enabling
    the unified client to delegate operations without conditional branches.

    All methods return Pydantic models, regardless of the underlying
    native format (JSON for REST, protobuf for gRPC, PyO3 objects for embedded).
    """

    @property
    @abstractmethod
    def protocol_name(self) -> str:
        """Return the protocol name (e.g., 'rest', 'grpc', 'embedded')."""
        pass

    @property
    @abstractmethod
    def is_connected(self) -> bool:
        """Check if the adapter is connected and operational."""
        pass

    # ==========================================================================
    # Health & Server Operations
    # ==========================================================================

    @abstractmethod
    def health(self) -> HealthStatus:
        """Check adapter health status."""
        pass

    # ==========================================================================
    # Collection Operations
    # ==========================================================================

    @abstractmethod
    def create_collection(
        self, name: str, config: Optional[CollectionConfig] = None, **kwargs
    ) -> Collection:
        """Create a new vector collection."""
        pass

    @abstractmethod
    def get_collection(self, collection_id: str) -> Optional[Collection]:
        """Get collection metadata by ID or name."""
        pass

    @abstractmethod
    def list_collections(self) -> List[Collection]:
        """List all collections."""
        pass

    @abstractmethod
    def delete_collection(self, collection_id: str) -> bool:
        """Delete a collection."""
        pass

    # ==========================================================================
    # Record Operations
    # ==========================================================================

    @abstractmethod
    def insert_records(
        self,
        collection_id: str,
        records: Union[List[ProximaRecord], List[Dict[str, Any]]],
        **kwargs,
    ) -> BatchResult:
        """Insert ProximaRecord-shaped payloads into a collection."""
        pass

    @abstractmethod
    def upsert_records(
        self,
        collection_id: str,
        records: Union[List[ProximaRecord], List[Dict[str, Any]]],
        **kwargs,
    ) -> BatchResult:
        """Upsert ProximaRecord-shaped payloads into a collection."""
        pass

    # ==========================================================================
    # Vector Compatibility Aliases
    # ==========================================================================

    @abstractmethod
    def insert_vectors(
        self,
        collection_id: str,
        vectors: Union[List[VectorRecord], List[Dict[str, Any]]],
        **kwargs,
    ) -> VectorOperationResponse:
        """Compatibility alias for record-native inserts."""
        pass

    @abstractmethod
    def upsert_vectors(
        self,
        collection_id: str,
        vectors: Union[List[VectorRecord], List[Dict[str, Any]]],
        **kwargs,
    ) -> VectorOperationResponse:
        """Compatibility alias for record-native upserts."""
        pass

    @abstractmethod
    def get_vectors(
        self,
        collection_id: str,
        vector_ids: List[str],
        include_vectors: bool = True,
        **kwargs,
    ) -> List[VectorRecord]:
        """Get vectors by IDs."""
        pass

    @abstractmethod
    def delete_vectors(
        self, collection_id: str, vector_ids: List[str], **kwargs
    ) -> VectorOperationResponse:
        """Delete vectors by IDs."""
        pass

    @abstractmethod
    def update_vector_metadata(
        self, collection_id: str, vector_id: str, metadata: MetadataDict, **kwargs
    ) -> VectorOperationResponse:
        """Update metadata for a specific vector."""
        pass

    # ==========================================================================
    # Search Operations
    # ==========================================================================

    @abstractmethod
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
        pass

    @abstractmethod
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
        pass

    # ==========================================================================
    # Document Operations
    # ==========================================================================

    def create_document_collection(
        self, name: str, config: Optional[Dict[str, Any]] = None, **kwargs
    ) -> Dict[str, Any]:
        """Create a document collection."""
        raise NotImplementedError(
            f"{self.__class__.__name__} does not support document collections"
        )

    def insert_document(
        self,
        collection_name: str,
        document: Dict[str, Any],
        id: Optional[str] = None,
        **kwargs,
    ) -> Dict[str, Any]:
        """Insert a document."""
        raise NotImplementedError(
            f"{self.__class__.__name__} does not support document inserts"
        )

    def get_document(
        self,
        collection_name: str,
        doc_id: str,
        projection: Optional[List[str]] = None,
        **kwargs,
    ) -> Optional[Dict[str, Any]]:
        """Get a document by ID."""
        raise NotImplementedError(
            f"{self.__class__.__name__} does not support document reads"
        )

    def query_documents(
        self,
        collection_name: str,
        filter: Optional[Dict[str, Any]] = None,
        projection: Optional[List[str]] = None,
        limit: int = 100,
        **kwargs,
    ) -> Dict[str, Any]:
        """Query documents with filter."""
        raise NotImplementedError(
            f"{self.__class__.__name__} does not support document queries"
        )

    def update_document(
        self, collection_name: str, doc_id: str, updates: List[Dict[str, Any]], **kwargs
    ) -> Dict[str, Any]:
        """Update a document."""
        raise NotImplementedError(
            f"{self.__class__.__name__} does not support document updates"
        )

    def delete_document(self, collection_name: str, doc_id: str, **kwargs) -> bool:
        """Delete a document."""
        raise NotImplementedError(
            f"{self.__class__.__name__} does not support document deletes"
        )

    def list_document_collections(self, **kwargs) -> List[Dict[str, Any]]:
        """List all document collections."""
        raise NotImplementedError(
            f"{self.__class__.__name__} does not support document collection listing"
        )

    def delete_document_collection(self, collection_name: str, **kwargs) -> bool:
        """Delete a document collection."""
        raise NotImplementedError(
            f"{self.__class__.__name__} does not support document collection deletion"
        )

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
        """Execute hybrid search combining BM25 and vector similarity."""
        raise NotImplementedError(
            f"{self.__class__.__name__} does not support hybrid search"
        )

    # ==========================================================================
    # Time-Series Operations
    # ==========================================================================

    def create_timeseries_collection(
        self, name: str, config: Optional[Dict[str, Any]] = None, **kwargs
    ) -> Dict[str, Any]:
        """Create a time-series collection."""
        raise NotImplementedError(
            f"{self.__class__.__name__} does not support time-series collections"
        )

    def ingest_timeseries(
        self, collection_name: str, points: List[Dict[str, Any]], **kwargs
    ) -> Dict[str, Any]:
        """Ingest time-series data points."""
        raise NotImplementedError(
            f"{self.__class__.__name__} does not support time-series ingest"
        )

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
        """Query time-series data with optional aggregation."""
        raise NotImplementedError(
            f"{self.__class__.__name__} does not support time-series queries"
        )

    def list_timeseries_collections(self, **kwargs) -> List[Dict[str, Any]]:
        """List all time-series collections."""
        raise NotImplementedError(
            f"{self.__class__.__name__} does not support time-series collection listing"
        )

    def delete_timeseries_collection(self, collection_name: str, **kwargs) -> bool:
        """Delete a time-series collection."""
        raise NotImplementedError(
            f"{self.__class__.__name__} does not support time-series collection deletion"
        )

    # ==========================================================================
    # Lifecycle Methods
    # ==========================================================================

    def close(self) -> None:
        """Close any open connections. Override in subclasses if needed."""
        pass

    def __enter__(self):
        """Context manager entry."""
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        """Context manager exit."""
        self.close()
        return False
