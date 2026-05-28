"""Embedded Victor-style provider backed by ProximaDB agentic IO helpers."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
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
        self._repo_root: Path | None = None
        self._initialized = False

    async def initialize(self, repo_root: Path | str | None = None) -> None:
        if repo_root is not None:
            self._repo_root = Path(repo_root)
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

    def make_symbol_id(self, rel_path: str, symbol_name: str) -> str:
        return f"symbol:{rel_path}:{symbol_name}"

    def make_file_id(self, rel_path: str) -> str:
        return f"file:{rel_path}"

    def parse_id(self, unified_id: str) -> Any:
        try:
            from victor.storage.unified.protocol import UnifiedId

            return UnifiedId.from_string(unified_id)
        except Exception:
            parts = unified_id.split(":", 2)
            if len(parts) == 3:
                return {"type": parts[0], "path": parts[1], "name": parts[2]}
            if len(parts) == 2:
                return {"type": parts[0], "path": parts[1], "name": ""}
            raise ValueError(f"Invalid unified ID format: {unified_id}") from None

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

    async def index_symbol(self, symbol: Any, embedding_text: str) -> None:
        vector = _deterministic_vector(embedding_text)
        await self.upsert_symbol(_symbol_record_from_unified(symbol), vector=vector)

    async def index_symbols_batch(
        self,
        symbols: list[tuple[Any, str]],
        batch_size: int = 500,
    ) -> int:
        del batch_size
        for symbol, embedding_text in symbols:
            await self.index_symbol(symbol, embedding_text)
        return len(symbols)

    async def index_edge(self, edge: Any) -> None:
        await self.initialize()
        src_id = str(edge.src_id)
        dst_id = str(edge.dst_id)
        await self._ensure_graph_node(src_id)
        await self._ensure_graph_node(dst_id)
        await self.link_symbols(
            src_id,
            str(edge.type),
            dst_id,
            properties=dict(getattr(edge, "metadata", {}) or {}),
        )

    async def index_edges_batch(self, edges: list[Any]) -> int:
        for edge in edges:
            await self.index_edge(edge)
        return len(edges)

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

    async def search_semantic(
        self,
        query: str,
        limit: int = 20,
        threshold: float = 0.25,
    ) -> list[Any]:
        del threshold
        symbols = await self.semantic_search(_deterministic_vector(query), top_k=limit)
        return [
            _unified_search_result(symbol, score=1.0, match_type="semantic")
            for symbol in symbols
        ]

    async def search_keyword(
        self,
        query: str,
        limit: int = 20,
        symbol_types: list[str] | None = None,
    ) -> list[Any]:
        await self.initialize()
        candidates = await self.find_symbols(limit=10_000)
        lowered = query.lower()
        results = []
        for symbol in candidates:
            if symbol_types and symbol.kind not in symbol_types:
                continue
            if (
                lowered in symbol.name.lower()
                or lowered in (symbol.content or "").lower()
            ):
                results.append(
                    _unified_search_result(symbol, score=1.0, match_type="keyword")
                )
            if len(results) >= limit:
                break
        return results

    async def search(self, params: Any) -> list[Any]:
        mode = str(getattr(params, "mode", "hybrid")).lower()
        query = str(params.query)
        limit = int(getattr(params, "limit", 20))
        symbol_types = getattr(params, "symbol_types", None)
        if "semantic" in mode:
            return await self.search_semantic(query, limit=limit)
        return await self.search_keyword(query, limit=limit, symbol_types=symbol_types)

    async def get_symbol(self, unified_id: str) -> Any | None:
        await self.initialize()
        symbol = self.session.get(
            VictorSymbolRecord,
            unified_id,
            collection=self.symbol_collection,
        )
        return _unified_symbol_from_record(symbol) if symbol else None

    async def get_symbols_in_file(self, rel_path: str) -> list[Any]:
        await self.initialize()
        symbols = await self.find_symbols(file_path=rel_path, limit=10_000)
        return [_unified_symbol_from_record(symbol) for symbol in symbols]

    async def stats(self) -> dict[str, Any]:
        await self.initialize()
        symbols = await self.find_symbols(limit=100_000)
        return {
            "workspace": self.workspace,
            "symbol_count": len(symbols),
            "event_count": len(
                self.events.read_stream(self.event_stream, limit=100_000)
            ),
        }

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

    async def _ensure_graph_node(self, node_id: str) -> None:
        get_node = getattr(self.adapter, "get_node", None)
        if callable(get_node):
            try:
                if get_node(node_id=node_id, graph=self.graph):
                    return
            except Exception:
                pass

        create_node = getattr(self.adapter, "create_node", None)
        if not callable(create_node):
            return
        try:
            create_node(
                graph=self.graph,
                node_id=node_id,
                labels=["Symbol"],
                properties={"id": node_id, "placeholder": True},
            )
        except Exception:
            pass


def _symbol_record_from_unified(symbol: Any) -> VictorSymbolRecord:
    return VictorSymbolRecord(
        id=str(symbol.unified_id),
        name=str(symbol.name),
        kind=str(symbol.type),
        file_path=str(symbol.file_path),
        language=str(getattr(symbol, "lang", None) or ""),
        line=getattr(symbol, "line", None),
        signature=getattr(symbol, "signature", None),
        content=getattr(symbol, "docstring", None),
        metadata=dict(getattr(symbol, "metadata", {}) or {}),
    )


def _unified_symbol_from_record(symbol: VictorSymbolRecord) -> Any:
    try:
        from victor.storage.unified.protocol import UnifiedSymbol

        return UnifiedSymbol(
            unified_id=symbol.id,
            name=symbol.name,
            type=symbol.kind,
            file_path=symbol.file_path,
            line=symbol.line,
            lang=symbol.language,
            signature=symbol.signature,
            docstring=symbol.content,
            metadata=dict(symbol.metadata or {}),
        )
    except Exception:
        return symbol


def _unified_search_result(
    symbol: VictorSymbolRecord, *, score: float, match_type: str
) -> Any:
    unified_symbol = _unified_symbol_from_record(symbol)
    try:
        from victor.storage.unified.protocol import UnifiedSearchResult

        return UnifiedSearchResult(
            symbol=unified_symbol,
            score=score,
            match_type=match_type,
            semantic_score=score if match_type == "semantic" else None,
            keyword_score=score if match_type == "keyword" else None,
            matched_content=symbol.content,
        )
    except Exception:
        return {
            "symbol": unified_symbol,
            "score": score,
            "match_type": match_type,
            "matched_content": symbol.content,
        }


def _deterministic_vector(text: str, dimensions: int = 4) -> list[float]:
    values = [0.0] * dimensions
    for index, byte in enumerate(text.encode("utf-8")):
        values[index % dimensions] += float(byte % 31) / 31.0
    return values
