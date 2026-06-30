"""
Document service client for ProximaDB v2.

This module provides the DocumentServiceClient for interacting with the
ProximaDB Document v2 gRPC service. The document body is carried as the
canonical v2 ``TypedValue`` (full ProximaValue coverage), so decimals,
timestamps, UUIDs, and vectors survive losslessly — encoding is delegated
to the shared transport-agnostic codec (``protocols/_grpc_v2_codec.py``),
never hand-rolled here.

Copyright 2025 ProximaDB Contributors
Licensed under the Apache License, Version 2.0
"""

from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional


@dataclass
class Document:
    """Represents a document in ProximaDB.

    ``props`` is the document body: a ``{field: python_value}`` map. Each
    value is encoded to / decoded from the canonical v2 ``TypedValue`` via
    the shared codec, so the full ProximaValue type system round-trips.
    """

    collection_id: str
    id: str = ""
    props: Dict[str, Any] = field(default_factory=dict)
    version: int = 0
    schema_id: Optional[str] = None
    document_type: Optional[str] = None
    updated_at_ms: int = 0

    @classmethod
    def from_pb(cls, pb_document: Any) -> "Document":
        """Create Document from protobuf message.

        Props are decoded via the shared codec (the same path the sync gRPC
        client uses), so rich types survive.
        """
        from proximadb_sdk.protocols.grpc_sync import ProximaDBSyncGrpcClient

        document = cls(
            collection_id=pb_document.collection_id,
            id=pb_document.id,
            version=pb_document.version,
            updated_at_ms=pb_document.updated_at_ms,
        )

        if pb_document.HasField("schema_id"):
            document.schema_id = pb_document.schema_id
        if pb_document.HasField("document_type"):
            document.document_type = pb_document.document_type

        # Decode props (map<string, TypedValue>) via the shared codec.
        for key, typed_value in pb_document.props.items():
            document.props[key] = ProximaDBSyncGrpcClient._v2_typed_value_to_python(
                None, typed_value
            )

        return document

    def to_dict(self) -> Dict[str, Any]:
        """Convert document to dictionary."""
        return {
            "collection_id": self.collection_id,
            "id": self.id,
            "props": self.props,
            "version": self.version,
            "schema_id": self.schema_id,
            "document_type": self.document_type,
            "updated_at_ms": self.updated_at_ms,
        }


@dataclass
class CreateDocumentResponse:
    """Response from create_document operation."""

    document: Document


@dataclass
class QueryDocumentsResponse:
    """Response from query_documents operation."""

    documents: List[Document]
    total_count: Optional[int] = None
    query_time_ms: int = 0


@dataclass
class AggregateDocumentsResponse:
    """Response from aggregate_documents operation.

    ``results`` is a list of ``{field: python_value}`` dicts — each result
    document's fields are decoded from ``TypedValue`` via the shared codec.
    """

    results: List[Dict[str, Any]] = field(default_factory=list)
    query_time_ms: int = 0


def _operation_to_enum(document_pb2_module: Any, operation: Any) -> int:
    """Map an update operation string (or int) to the ``DocumentUpdateOperation``
    enum value. Accepts the enum name suffix (e.g. ``"set"`` ->
    ``DOCUMENT_UPDATE_OPERATION_SET``), a full name, or an existing int value.
    Defaults to SET.
    """
    if isinstance(operation, int):
        return operation
    name = str(operation).strip().upper()
    if not name.startswith("DOCUMENT_UPDATE_OPERATION_"):
        name = f"DOCUMENT_UPDATE_OPERATION_{name}"
    return getattr(
        document_pb2_module,
        name,
        document_pb2_module.DOCUMENT_UPDATE_OPERATION_SET,
    )


class DocumentServiceClient:
    """
    Client for ProximaDB Document v2 gRPC service.

    This client provides methods for document CRUD, query, and aggregation.
    All value encoding/decoding goes through the shared ``TypedValue`` codec
    so the full ProximaValue type system round-trips without loss.
    """

    def __init__(self, grpc_client: Any):
        """
        Initialize DocumentServiceClient.

        Args:
            grpc_client: ProximaDBSyncGrpcClient instance
        """
        self._grpc_client = grpc_client

    def create_document(
        self,
        collection_id: str,
        props: Optional[Dict[str, Any]] = None,
        document_id: Optional[str] = None,
        document: Optional[Document] = None,
        schema_id: Optional[str] = None,
        document_type: Optional[str] = None,
    ) -> CreateDocumentResponse:
        """
        Create a document.

        Args:
            collection_id: Collection ID
            props: Document body as a ``{field: value}`` map (optional if
                ``document`` is given)
            document_id: Optional client-supplied id; the server generates a
                UUID when empty
            document: Document object (optional if using ``props``)
            schema_id: Optional schema id
            document_type: Optional document type

        Returns:
            CreateDocumentResponse with the stored document (including the
            server-assigned id when one was generated)
        """
        from proximadb.v2 import document_pb2 as v2_document_pb2  # type: ignore

        if document is not None:
            collection_id = document.collection_id
            document_id = document_id or document.id
            props = document.props
            schema_id = schema_id or document.schema_id
            document_type = document_type or document.document_type

        request = v2_document_pb2.CreateDocumentRequest(
            collection_id=collection_id,
            id=document_id or "",
        )
        if schema_id:
            request.schema_id = schema_id
        if document_type:
            request.document_type = document_type

        # Encode props via the shared TypedValue codec.
        for key, value in (props or {}).items():
            request.props[key].CopyFrom(
                self._grpc_client._python_to_v2_typed_value(value)
            )

        response = self._grpc_client._execute_document_with_pool(
            "create_document",
            lambda stub: stub.CreateDocument(
                request, timeout=self._grpc_client.timeout
            ),
        )

        return CreateDocumentResponse(document=Document.from_pb(response.document))

    def get_document(self, collection_id: str, document_id: str) -> Optional[Document]:
        """
        Get a document by ID.

        Args:
            collection_id: Collection ID
            document_id: Document ID

        Returns:
            Document object or None if not found
        """
        from proximadb.v2 import document_pb2 as v2_document_pb2  # type: ignore

        request = v2_document_pb2.GetDocumentRequest(
            collection_id=collection_id,
            id=document_id,
        )

        try:
            response = self._grpc_client._execute_document_with_pool(
                "get_document",
                lambda stub: stub.GetDocument(
                    request, timeout=self._grpc_client.timeout
                ),
            )

            if response.document:
                return Document.from_pb(response.document)
            return None
        except Exception as e:
            if "not found" in str(e).lower():
                return None
            raise

    def update_document(
        self,
        collection_id: str,
        document_id: str,
        updates: List[Dict[str, Any]],
        expected_version: Optional[int] = None,
    ) -> Document:
        """
        Update a document with field operations.

        Args:
            collection_id: Collection ID
            document_id: Document ID
            updates: List of update operations, each a dict with keys:
                ``operation`` (str/int enum, default "set"), ``path`` (dotted
                JSON path), and ``value`` (for SET/INC/PUSH/PULL)
            expected_version: Optional optimistic-lock version

        Returns:
            Updated Document
        """
        from proximadb.v2 import document_pb2 as v2_document_pb2  # type: ignore

        pb_updates = []
        for upd in updates:
            field_update = v2_document_pb2.DocumentFieldUpdate(
                operation=_operation_to_enum(
                    v2_document_pb2, upd.get("operation", "set")
                ),
                path=upd.get("path", ""),
            )
            if "value" in upd and upd["value"] is not None:
                field_update.value.CopyFrom(
                    self._grpc_client._python_to_v2_typed_value(upd["value"])
                )
            pb_updates.append(field_update)

        request = v2_document_pb2.UpdateDocumentRequest(
            collection_id=collection_id,
            id=document_id,
            updates=pb_updates,
        )
        if expected_version is not None:
            request.expected_version = expected_version

        response = self._grpc_client._execute_document_with_pool(
            "update_document",
            lambda stub: stub.UpdateDocument(
                request, timeout=self._grpc_client.timeout
            ),
        )

        return Document.from_pb(response.document)

    def query_documents(
        self,
        collection_id: str,
        projection: Optional[List[str]] = None,
        sort: Optional[List[Dict[str, Any]]] = None,
        limit: int = 0,
        offset: int = 0,
        include_count: bool = False,
    ) -> QueryDocumentsResponse:
        """
        Query (scan) documents in a collection.

        First slice: value-free — projection, sort, and pagination only.
        Predicate filters are deferred server-side until a ProximaValue-native
        filter exists.

        Args:
            collection_id: Collection ID
            projection: Fields to include (empty = all)
            sort: List of ``{"path": ..., "order": "asc"|"desc"}`` dicts
            limit: Max results (0 = server default)
            offset: Pagination offset
            include_count: Request the total match count (slower)

        Returns:
            QueryDocumentsResponse with matching documents
        """
        from proximadb.v2 import document_pb2 as v2_document_pb2  # type: ignore

        request = v2_document_pb2.QueryDocumentsRequest(
            collection_id=collection_id,
            projection=projection or [],
            limit=limit,
            offset=offset,
            include_count=include_count,
        )

        for sort_field in sort or []:
            order = str(sort_field.get("order", "asc")).lower()
            order_enum = (
                v2_document_pb2.DOCUMENT_SORT_DESC
                if order == "desc"
                else v2_document_pb2.DOCUMENT_SORT_ASC
            )
            request.sort.add(path=sort_field.get("path", ""), order=order_enum)

        response = self._grpc_client._execute_document_with_pool(
            "query_documents",
            lambda stub: stub.QueryDocuments(
                request, timeout=self._grpc_client.timeout
            ),
        )

        documents = [Document.from_pb(doc) for doc in response.documents]

        total_count = None
        if response.HasField("total_count"):
            total_count = response.total_count

        return QueryDocumentsResponse(
            documents=documents,
            total_count=total_count,
            query_time_ms=response.query_time_ms,
        )

    def aggregate_documents(
        self,
        collection_id: str,
        pipeline: List[Dict[str, Any]],
    ) -> AggregateDocumentsResponse:
        """
        Run an aggregation pipeline over a collection.

        Each ``pipeline`` entry is a stage dict with a single stage key, e.g.
        ``{"group": {"key": "category", "aggregations": [...]}}``. Supported
        stage keys: ``group``, ``project``, ``sort``, ``limit``, ``skip``.

        Args:
            collection_id: Collection ID
            pipeline: Aggregation pipeline stages

        Returns:
            AggregateDocumentsResponse with result documents
        """
        from proximadb.v2 import document_pb2 as v2_document_pb2  # type: ignore

        request = v2_document_pb2.AggregateDocumentsRequest(
            collection_id=collection_id,
        )

        for stage in pipeline or []:
            pb_stage = request.pipeline.add()
            if "group" in stage:
                group = stage["group"]
                pb_group = pb_stage.group
                pb_group.key = group.get("key", "_id")
                for agg in group.get("aggregations", []):
                    pb_agg = pb_group.aggregations.add(
                        output_field=agg.get("output_field", ""),
                        input_path=agg.get("input_path", ""),
                    )
                    if "type" in agg:
                        pb_agg.type = _aggregation_type_to_enum(
                            v2_document_pb2, agg["type"]
                        )
            elif "project" in stage:
                project = stage["project"]
                for field_name, include in (project.get("fields") or {}).items():
                    pb_stage.project.fields[field_name] = bool(include)
                for alias, expr in (project.get("computed") or {}).items():
                    pb_stage.project.computed[alias] = str(expr)
            elif "sort" in stage:
                for sort_field in stage["sort"]:
                    pb_stage.sort.sort.add(
                        path=sort_field.get("path", ""),
                        order=int(sort_field.get("order", 1)),
                    )
            elif "limit" in stage:
                pb_stage.limit.limit = int(stage["limit"])
            elif "skip" in stage:
                pb_stage.skip.skip = int(stage["skip"])

        response = self._grpc_client._execute_document_with_pool(
            "aggregate_documents",
            lambda stub: stub.AggregateDocuments(
                request, timeout=self._grpc_client.timeout
            ),
        )

        results: List[Dict[str, Any]] = []
        for result in response.results:
            decoded: Dict[str, Any] = {}
            for field_name, typed_value in result.fields.items():
                decoded[field_name] = self._grpc_client._v2_typed_value_to_python(
                    typed_value
                )
            results.append(decoded)

        return AggregateDocumentsResponse(
            results=results,
            query_time_ms=response.query_time_ms,
        )

    def delete_document(self, collection_id: str, document_id: str) -> bool:
        """
        Delete a document by ID.

        Args:
            collection_id: Collection ID
            document_id: Document ID

        Returns:
            True if deleted, False if not found
        """
        from proximadb.v2 import document_pb2 as v2_document_pb2  # type: ignore

        request = v2_document_pb2.DeleteDocumentRequest(
            collection_id=collection_id,
            id=document_id,
        )

        try:
            response = self._grpc_client._execute_document_with_pool(
                "delete_document",
                lambda stub: stub.DeleteDocument(
                    request, timeout=self._grpc_client.timeout
                ),
            )
            return response.deleted
        except Exception as e:
            if "not found" in str(e).lower():
                return False
            raise


def _aggregation_type_to_enum(document_pb2_module: Any, agg_type: Any) -> int:
    """Map an aggregation type string (or int) to the ``AggregationType`` enum
    value. Accepts the enum name suffix (e.g. ``"sum"`` ->
    ``AGGREGATION_TYPE_SUM``), a full name, or an existing int value. Defaults
    to COUNT.
    """
    if isinstance(agg_type, int):
        return agg_type
    name = str(agg_type).strip().upper()
    if not name.startswith("AGGREGATION_TYPE_"):
        name = f"AGGREGATION_TYPE_{name}"
    return getattr(
        document_pb2_module,
        name,
        document_pb2_module.AGGREGATION_TYPE_COUNT,
    )
