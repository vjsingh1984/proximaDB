"""Embedded Victor-style provider backed by ProximaDB agentic IO helpers."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from proximadb_sdk.integrations.agentic_io import (
    ProximaEventStore,
    ProximaMapperSession,
)


@dataclass
class VictorSymbolRecord:
    """Portable symbol record for Victor and coding-agent indexes."""

    id: str
    name: str
    kind: str
    file_path: str
    language: str
    line: int | None = None
    signature: str | None = None
    content: str | None = None
    metadata: dict[str, Any] | None = None


class ProximaDBEmbeddedVictorProvider:
    """Small embedded-first provider for Victor-style code knowledge.

    The provider intentionally composes the generic agentic IO contracts rather
    than adding a separate persistence path:

    - symbols are stored through ``ProximaMapperSession``
    - relationships are stored through graph APIs
    - indexing activity is recorded through ``ProximaEventStore``
    """

    def __init__(
        self,
        adapter: Any,
        *,
        workspace: str = "victor",
        graph: str | None = None,
    ) -> None:
        self.adapter = adapter
        self.workspace = workspace
        self.symbol_collection = f"{workspace}_symbols"
        self.event_stream = f"{workspace}:events"
        self.graph = graph or f"{workspace}_graph"
        self.session = ProximaMapperSession(adapter, default_graph=self.graph)
        self.events = ProximaEventStore(
            adapter,
            collection=f"{workspace}_events",
        )
        self._initialized = False

    async def initialize(self) -> None:
        if self._initialized:
            return
        self.session.register(
            VictorSymbolRecord,
            collection=self.symbol_collection,
            indexed_paths=["$.name", "$.kind", "$.file_path", "$.language"],
        )
        self.events.setup()
        create_graph = getattr(self.adapter, "create_graph", None)
        if callable(create_graph):
            try:
                create_graph(self.graph)
            except Exception:
                pass
        self._initialized = True

    async def upsert_symbol(
        self,
        symbol: VictorSymbolRecord,
        *,
        vector: list[float] | None = None,
    ) -> str:
        await self.initialize()
        doc_id = self.session.upsert(
            symbol,
            collection=self.symbol_collection,
            vector=vector,
            source=symbol.content,
        )
        create_node = getattr(self.adapter, "create_node", None)
        if callable(create_node):
            try:
                create_node(
                    graph=self.graph,
                    node_id=doc_id,
                    labels=[symbol.kind, symbol.language],
                    properties={
                        "name": symbol.name,
                        "kind": symbol.kind,
                        "file_path": symbol.file_path,
                        "language": symbol.language,
                        "line": symbol.line,
                        "signature": symbol.signature,
                        "metadata": symbol.metadata or {},
                    },
                )
            except Exception:
                pass
        self.events.append(
            self.event_stream,
            "SymbolUpserted",
            {
                "symbol_id": doc_id,
                "name": symbol.name,
                "kind": symbol.kind,
                "file_path": symbol.file_path,
            },
        )
        return doc_id

    async def find_symbols(
        self,
        *,
        name: str | None = None,
        kind: str | None = None,
        file_path: str | None = None,
        language: str | None = None,
        limit: int = 100,
    ) -> list[VictorSymbolRecord]:
        await self.initialize()
        filters = {
            key: value
            for key, value in {
                "name": name,
                "kind": kind,
                "file_path": file_path,
                "language": language,
            }.items()
            if value is not None
        }
        return (
            self.session.query(VictorSymbolRecord, collection=self.symbol_collection)
            .where(**filters)
            .limit(limit)
            .all()
        )

    async def link_symbols(
        self,
        src_id: str,
        relation: str,
        dst_id: str,
        *,
        properties: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        await self.initialize()
        edge = self.session.link(
            src_id,
            relation,
            dst_id,
            graph=self.graph,
            properties=properties or {},
        )
        self.events.append(
            self.event_stream,
            "SymbolLinked",
            {
                "src_id": src_id,
                "dst_id": dst_id,
                "relation": relation,
                "properties": properties or {},
            },
        )
        return edge

    async def semantic_search(
        self,
        vector: list[float],
        *,
        top_k: int = 10,
    ) -> list[VictorSymbolRecord]:
        await self.initialize()
        hits = self.session.vector_search(
            VictorSymbolRecord,
            vector,
            collection=self.symbol_collection,
            top_k=top_k,
        )
        return [hit.item for hit in hits]

    async def event_history(self) -> list[dict[str, Any]]:
        await self.initialize()
        return [
            {
                "version": event.version,
                "event_type": event.event_type,
                "data": event.data,
            }
            for event in self.events.read_stream(self.event_stream, limit=100_000)
        ]
