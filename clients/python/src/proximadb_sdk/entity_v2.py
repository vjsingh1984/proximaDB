"""
Entity service client for ProximaDB v2.

This module provides the EntityServiceClient for interacting with the
ProximaDB Entity v2 gRPC service.

Copyright 2025 ProximaDB Contributors
Licensed under the Apache License, Version 2.0
"""

from typing import Any, Dict, List, Optional
from dataclasses import dataclass, field


@dataclass
class Entity:
    """Represents an entity in ProximaDB."""

    id: str
    collection_id: str
    flexible_metadata: Dict[str, Any] = field(default_factory=dict)
    embeddings: List[Any] = field(default_factory=list)
    typed_metadata: Optional[Dict[str, Any]] = None
    provenance: Optional[Dict[str, Any]] = None
    relations: List[Any] = field(default_factory=list)
    temporal: Optional[Dict[str, Any]] = None

    @classmethod
    def from_pb(cls, pb_entity: Any) -> "Entity":
        """Create Entity from protobuf message."""
        from proximadb_sdk.protocols.grpc_sync import ProximaDBSyncGrpcClient

        entity = cls(
            id=pb_entity.id,
            collection_id=pb_entity.collection_id,
        )

        # Convert flexible_metadata (map<string, TypedValue>)
        for key, typed_value in pb_entity.flexible_metadata.items():
            entity.flexible_metadata[key] = ProximaDBSyncGrpcClient._v2_typed_value_to_python(
                None, typed_value
            )

        # Convert embeddings
        for embedding in pb_entity.embeddings:
            entity.embeddings.append(
                {
                    "model_id": embedding.model_id,
                    "dimension": embedding.dimension,
                    "vector": list(embedding.vector),
                    "modality": embedding.modality if embedding.modality else None,
                }
            )

        # Convert provenance if present
        if pb_entity.provenance:
            entity.provenance = {
                "source_id": pb_entity.provenance.source_id,
                "chunk_id": pb_entity.provenance.chunk_id,
                "chunk_position": pb_entity.provenance.chunk_position,
                "extraction_method": pb_entity.provenance.extraction_method,
                "extracted_at_ms": pb_entity.provenance.extracted_at_ms,
                "metadata": dict(pb_entity.provenance.metadata),
            }

        # Convert relations. The proto `Relation` uses `properties: map<string,string>`.
        for relation in pb_entity.relations:
            entity.relations.append(
                {
                    "relation_type": relation.relation_type,
                    "source_entity_id": relation.source_entity_id,
                    "target_entity_id": relation.target_entity_id,
                    "weight": relation.weight,
                    "properties": dict(relation.properties),
                }
            )

        # Convert temporal info if present
        if pb_entity.temporal:
            entity.temporal = {
                "created_at_ms": pb_entity.temporal.created_at_ms,
                "valid_from_ms": pb_entity.temporal.valid_from_ms,
                "valid_to_ms": pb_entity.temporal.valid_to_ms,
                "is_current": pb_entity.temporal.is_current,
            }

        return entity

    def to_dict(self) -> Dict[str, Any]:
        """Convert entity to dictionary."""
        return {
            "id": self.id,
            "collection_id": self.collection_id,
            "flexible_metadata": self.flexible_metadata,
            "embeddings": self.embeddings,
            "provenance": self.provenance,
            "relations": self.relations,
            "temporal": self.temporal,
        }


@dataclass
class UpsertEntityResponse:
    """Response from upsert_entity operation."""

    success: bool
    entity_id: str
    message: str


@dataclass
class SearchEntitiesResponse:
    """Response from search_entities operation."""

    entities: List[Entity]
    total_count: int


def _value_to_filter_clause(entity_pb2_module: Any, field: str, value: Any) -> Any:
    """Build a proto ``FilterClause`` (EQ) choosing the right oneof value field
    for the Python value's type. ``bool`` is checked before ``int`` because
    ``bool`` is a subclass of ``int`` in Python.
    """
    kwargs: Dict[str, Any] = {
        "field": field,
        "op": entity_pb2_module.EntityComparisonOp.ENTITY_COMPARISON_EQ,
    }
    if isinstance(value, bool):
        kwargs["bool_value"] = value
    elif isinstance(value, int):
        kwargs["int_value"] = value
    elif isinstance(value, float):
        kwargs["double_value"] = value
    else:
        kwargs["string_value"] = str(value)
    return entity_pb2_module.FilterClause(**kwargs)


def _modality_to_enum(entity_pb2_module: Any, modality: Any) -> int:
    """Map a modality string (or int) to the ``EntityModality`` enum value.
    Accepts the enum name suffix (e.g. ``"text"`` → ``ENTITY_MODALITY_TEXT``),
    a full name, or an existing int value. Defaults to TEXT.
    """
    if isinstance(modality, int):
        return modality
    name = str(modality).strip().upper()
    if not name.startswith("ENTITY_MODALITY_"):
        name = f"ENTITY_MODALITY_{name}"
    return getattr(
        entity_pb2_module.EntityModality,
        name,
        entity_pb2_module.EntityModality.ENTITY_MODALITY_TEXT,
    )


class EntityServiceClient:
    """
    Client for ProximaDB Entity v2 gRPC service.

    This client provides methods for entity CRUD operations and search.
    """

    def __init__(self, grpc_client: Any):
        """
        Initialize EntityServiceClient.

        Args:
            grpc_client: ProximaDBSyncGrpcClient instance
        """
        self._grpc_client = grpc_client

    def upsert_entity(
        self,
        collection_id: str,
        entity: Optional[Entity] = None,
        flexible_metadata: Optional[Dict[str, Any]] = None,
        embeddings: Optional[List[Dict[str, Any]]] = None,
        provenance: Optional[Dict[str, Any]] = None,
        relations: Optional[List[Dict[str, Any]]] = None,
        entity_id: Optional[str] = None,
    ) -> UpsertEntityResponse:
        """
        Upsert an entity (create or update).

        Args:
            collection_id: Collection ID
            entity: Entity object (optional if using individual parameters)
            flexible_metadata: Entity metadata as key-value pairs
            embeddings: List of embedding vectors with metadata
            provenance: Provenance information
            relations: List of entity relations
            entity_id: Entity ID (optional for new entities)

        Returns:
            UpsertEntityResponse with operation result
        """
        from proximadb.v2 import entity_pb2 as v2_entity_pb2  # type: ignore

        # Use entity object if provided, otherwise build from parameters
        if entity is None:
            entity = Entity(
                id=entity_id or "",
                collection_id=collection_id,
                flexible_metadata=flexible_metadata or {},
                embeddings=embeddings or [],
                relations=relations or [],
            )

        # Convert to protobuf
        pb_entity = v2_entity_pb2.Entity(
            id=entity.id or "",  # Empty for auto-generated
            collection_id=collection_id,
        )

        # Add flexible metadata
        for key, value in entity.flexible_metadata.items():
            pb_entity.flexible_metadata[key] = self._grpc_client._python_to_v2_typed_value(
                value
            )

        # Add embeddings. `modality` is the EntityModality enum — map the
        # convenience string (default "text") to the enum value.
        for emb in entity.embeddings:
            pb_embedding = v2_entity_pb2.EmbeddingVersion(
                model_id=emb.get("model_id", ""),
                dimension=emb.get("dimension", len(emb.get("vector", []))),
                vector=emb.get("vector", []),
                modality=_modality_to_enum(v2_entity_pb2, emb.get("modality", "text")),
            )
            pb_entity.embeddings.append(pb_embedding)

        # Add provenance if provided
        if provenance or entity.provenance:
            prov = provenance or entity.provenance
            pb_provenance = v2_entity_pb2.Provenance(
                source_id=prov.get("source_id", ""),
                chunk_id=prov.get("chunk_id", ""),
                chunk_position=prov.get("chunk_position", 0),
                extraction_method=prov.get("extraction_method", ""),
                extracted_at_ms=prov.get("extracted_at_ms", 0),
                metadata=prov.get("metadata", {}),
            )
            pb_entity.provenance.CopyFrom(pb_provenance)

        # Add relations. The proto `Relation` uses `properties: map<string,string>`.
        for rel in entity.relations:
            pb_relation = v2_entity_pb2.Relation(
                relation_type=rel.get("relation_type", ""),
                source_entity_id=rel.get("source_entity_id", ""),
                target_entity_id=rel.get("target_entity_id", ""),
                weight=rel.get("weight", 0.0),
            )
            for k, v in (rel.get("properties") or {}).items():
                pb_relation.properties[k] = str(v)
            pb_entity.relations.append(pb_relation)

        # Execute the RPC
        request = v2_entity_pb2.UpsertEntityRequest(
            collection_id=collection_id,
            entity=pb_entity,
        )

        response = self._grpc_client._execute_entity_with_pool(
            "upsert_entity", lambda stub: stub.UpsertEntity(request, timeout=self._grpc_client.timeout)
        )

        return UpsertEntityResponse(
            success=response.success,
            entity_id=response.entity_id,
            message=response.message,
        )

    def get_entity(self, collection_id: str, entity_id: str) -> Optional[Entity]:
        """
        Get an entity by ID.

        Args:
            collection_id: Collection ID
            entity_id: Entity ID

        Returns:
            Entity object or None if not found
        """
        from proximadb.v2 import entity_pb2 as v2_entity_pb2  # type: ignore

        request = v2_entity_pb2.GetEntityRequest(
            collection_id=collection_id,
            entity_id=entity_id,
        )

        try:
            response = self._grpc_client._execute_entity_with_pool(
                "get_entity", lambda stub: stub.GetEntity(request, timeout=self._grpc_client.timeout)
            )

            if response.entity:
                return Entity.from_pb(response.entity)
            return None
        except Exception as e:
            if "not found" in str(e).lower():
                return None
            raise

    def delete_entity(self, collection_id: str, entity_id: str) -> bool:
        """
        Delete an entity by ID.

        Args:
            collection_id: Collection ID
            entity_id: Entity ID

        Returns:
            True if deleted, False if not found
        """
        from proximadb.v2 import entity_pb2 as v2_entity_pb2  # type: ignore

        request = v2_entity_pb2.DeleteEntityRequest(
            collection_id=collection_id,
            entity_id=entity_id,
        )

        try:
            response = self._grpc_client._execute_entity_with_pool(
                "delete_entity", lambda stub: stub.DeleteEntity(request, timeout=self._grpc_client.timeout)
            )
            return response.success
        except Exception as e:
            if "not found" in str(e).lower():
                return False
            raise

    def search_entities(
        self,
        collection_id: str,
        top_k: int = 10,
        filters: Optional[Dict[str, Any]] = None,
        query_vector: Optional[List[float]] = None,
    ) -> SearchEntitiesResponse:
        """
        Search for entities by metadata filter.

        Vector ANN search (``query_vector``) is not yet implemented server-side;
        it raises ``NotImplementedError`` until the vector-index metadata-filter
        fusion lands. Use ``filters`` (a ``{field: value}`` dict, combined with
        logical AND, equality) for now.

        Args:
            collection_id: Collection ID
            top_k: Maximum number of results
            filters: Equality metadata filters as a ``{field: value}`` dict
            query_vector: Reserved for future ANN search (not yet supported)

        Returns:
            SearchEntitiesResponse with matching entities
        """
        from proximadb.v2 import entity_pb2 as v2_entity_pb2  # type: ignore

        if query_vector:
            raise NotImplementedError(
                "Vector ANN entity search is not yet supported by the server. "
                "Use metadata filters instead."
            )

        request = v2_entity_pb2.SearchEntitiesRequest(
            collection_id=collection_id,
            top_k=top_k,
        )

        # Build a MetadataFilter from the simple {field: value} dict using EQ clauses.
        if filters:
            clauses = [
                _value_to_filter_clause(v2_entity_pb2, field, value)
                for field, value in filters.items()
            ]
            request.filters.CopyFrom(
                v2_entity_pb2.MetadataFilter(
                    clauses=clauses,
                    op=v2_entity_pb2.EntityLogicalOp.ENTITY_LOGICAL_AND,
                )
            )

        response = self._grpc_client._execute_entity_with_pool(
            "search_entities",
            lambda stub: stub.SearchEntities(request, timeout=self._grpc_client.timeout),
        )

        entities = [Entity.from_pb(res.entity) for res in response.results if res.entity]

        return SearchEntitiesResponse(
            entities=entities,
            total_count=response.total,
        )
