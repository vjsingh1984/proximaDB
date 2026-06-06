"""Offline unit tests for proximadb_sdk.code_knowledge.

Everything is mocked: the client, its collections and graphs, and the chunker.
No network, no tree-sitter parsing, no real embeddings.
"""

import hashlib

import pytest

from proximadb_sdk.chunking_strategies.base import TextChunk
from proximadb_sdk.code_knowledge import (
    CodeIndexConfig,
    CodeKnowledgeBuilder,
    CodeSearchResult,
    IndexingResult,
    create_code_knowledge_store,
)


# ---------------------------------------------------------------------------
# Test doubles
# ---------------------------------------------------------------------------


class FakeCollection:
    def __init__(self, search_results=None, use_insert_records=True):
        self.inserted = []
        self.deleted_filters = []
        self._search_results = search_results or []
        self._use_insert_records = use_insert_records

    async def insert_records(self, records):
        if not self._use_insert_records:
            raise AttributeError("no insert_records")
        self.inserted.extend(records)

    async def insert(self, records):
        self.inserted.extend(records)

    async def search(self, query_vector, top_k, filter=None):
        self.last_filter = filter
        self.last_top_k = top_k
        return self._search_results

    async def delete(self, filter=None):
        self.deleted_filters.append(filter)


class CollectionNoInsertRecords:
    """Collection that only has .insert (no insert_records attribute)."""

    def __init__(self):
        self.inserted = []

    async def insert(self, records):
        self.inserted.extend(records)


class FakeGraph:
    def __init__(self, traverse_map=None, raise_on_node=False):
        self.nodes = []
        self.edges = []
        self._traverse_map = traverse_map or {}
        self._raise_on_node = raise_on_node

    async def insert_node(self, node):
        if self._raise_on_node:
            raise RuntimeError("graph node insert failed")
        self.nodes.append(node)

    async def insert_edge(self, edge):
        self.edges.append(edge)

    async def traverse(self, start_node_id, edge_type, direction, max_depth):
        key = (edge_type, direction)
        return self._traverse_map.get(key, [])


class FakeClient:
    def __init__(
        self,
        collection=None,
        graph=None,
        existing_collections=None,
        existing_graphs=None,
    ):
        self.collection = collection or FakeCollection()
        self.graph = graph or FakeGraph()
        self.created_collections = []
        self.created_graphs = []
        self._existing_collections = existing_collections or []
        self._existing_graphs = existing_graphs or []

    async def list_collections(self):
        return self._existing_collections

    async def create_collection(self, **kwargs):
        self.created_collections.append(kwargs)

    async def list_graphs(self):
        return self._existing_graphs

    async def create_graph(self, **kwargs):
        self.created_graphs.append(kwargs)

    async def get_collection(self, name):
        return self.collection

    async def get_graph(self, name):
        return self.graph


class NamedThing:
    def __init__(self, name):
        self.name = name


def make_chunk(
    chunk_id,
    text="def foo():\n    pass",
    metadata=None,
):
    md = {} if metadata is None else dict(metadata)
    return TextChunk(
        text=text,
        start_pos=0,
        end_pos=len(text),
        chunk_id=chunk_id,
        metadata=md,
    )


class FakeChunker:
    """Replaces CodeChunkingStrategy so no tree-sitter is needed."""

    def __init__(self, chunks=None):
        self._chunks = chunks
        self.calls = []

    def chunk(self, text, source_id, metadata):
        self.calls.append((text, source_id, metadata))
        if self._chunks is None:
            return [make_chunk("c1")]
        return self._chunks


def build_builder(client=None, config=None, chunks=None, embedding_provider=None):
    client = client or FakeClient()
    builder = CodeKnowledgeBuilder(
        client=client, config=config, embedding_provider=embedding_provider
    )
    builder._chunker = FakeChunker(chunks=chunks)
    return builder


# ---------------------------------------------------------------------------
# Dataclasses
# ---------------------------------------------------------------------------


def test_config_defaults():
    cfg = CodeIndexConfig()
    assert cfg.vector_collection_name == "code_symbols"
    assert cfg.vector_dimension == 1536
    assert cfg.graph_name == "code_graph"
    assert "*.pyc" in cfg.exclude_patterns
    assert cfg.include_patterns == ["*"]


def test_indexing_result_defaults():
    r = IndexingResult()
    assert r.files_processed == 0
    assert r.errors == []
    assert r.file_hashes == {}


def test_code_search_result_defaults():
    r = CodeSearchResult(
        symbol_id="s",
        symbol_type="FUNCTION",
        fully_qualified_name="m.foo",
        simple_name="foo",
        source_code="x",
        file_path="f.py",
        start_line=1,
        end_line=2,
        language="python",
        score=0.9,
    )
    assert r.callers == []
    assert r.documentation is None


# ---------------------------------------------------------------------------
# Pure helpers
# ---------------------------------------------------------------------------


def test_compute_hash_sha256():
    b = build_builder()
    h = b._compute_hash("hello")
    assert h == hashlib.sha256(b"hello").hexdigest()


def test_compute_hash_md5():
    b = build_builder(config=CodeIndexConfig(hash_algorithm="md5"))
    h = b._compute_hash("hello")
    assert h == hashlib.md5(b"hello").hexdigest()


def test_compute_hash_unknown_falls_back_to_sha256():
    b = build_builder(config=CodeIndexConfig(hash_algorithm="weird"))
    h = b._compute_hash("hello")
    assert h == hashlib.sha256(b"hello").hexdigest()


def test_matches_patterns():
    b = build_builder()
    assert b._matches_patterns("a/b.pyc", ["*.pyc"]) is True
    assert b._matches_patterns("a/b.py", ["*.pyc"]) is False


def test_generate_placeholder_embedding_dimension_and_range():
    b = build_builder(config=CodeIndexConfig(vector_dimension=8))
    emb = b._generate_placeholder_embedding("some text")
    assert len(emb) == 8
    assert all(-1.0 <= v <= 1.0 for v in emb)


def test_generate_placeholder_embedding_pads_short_hash():
    # Large dimension forces the padding-with-zeros branch.
    b = build_builder(config=CodeIndexConfig(vector_dimension=200))
    emb = b._generate_placeholder_embedding("x")
    assert len(emb) == 200
    assert emb[-1] == 0.0


def test_prepare_text_for_embedding_full_metadata():
    b = build_builder()
    chunk = make_chunk(
        "c1",
        text="code body",
        metadata={
            "fully_qualified_name": "mod.foo",
            "documentation": "docs here",
            "signature": "foo() -> int",
        },
    )
    text = b._prepare_text_for_embedding(chunk)
    assert "Symbol: mod.foo" in text
    assert "Documentation: docs here" in text
    assert "Signature: foo() -> int" in text
    assert "Code:\ncode body" in text


def test_prepare_text_for_embedding_truncates_long_code():
    cfg = CodeIndexConfig(max_content_length=10)
    b = build_builder(config=cfg)
    chunk = make_chunk("c1", text="x" * 100, metadata={})
    text = b._prepare_text_for_embedding(chunk)
    assert text.endswith("...")


def test_get_indexed_files_and_hash():
    b = build_builder()
    assert b.get_indexed_files() == []
    b._file_hashes["a.py"] = "deadbeef"
    assert b.get_indexed_files() == ["a.py"]
    assert b.get_file_hash("a.py") == "deadbeef"
    assert b.get_file_hash("missing.py") is None


# ---------------------------------------------------------------------------
# initialize / ensure
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_initialize_creates_collection_and_graph():
    client = FakeClient()
    b = build_builder(client=client)
    await b.initialize()
    assert client.created_collections
    assert client.created_graphs
    assert b._vector_collection_ready
    assert b._graph_ready


@pytest.mark.asyncio
async def test_initialize_skips_when_existing_named_objects():
    client = FakeClient(
        existing_collections=[NamedThing("code_symbols")],
        existing_graphs=[NamedThing("code_graph")],
    )
    b = build_builder(client=client)
    await b.initialize()
    assert client.created_collections == []
    assert client.created_graphs == []


@pytest.mark.asyncio
async def test_initialize_skips_when_existing_plain_strings():
    client = FakeClient(
        existing_collections=["code_symbols"],
        existing_graphs=["code_graph"],
    )
    b = build_builder(client=client)
    await b.initialize()
    assert client.created_collections == []
    assert client.created_graphs == []


@pytest.mark.asyncio
async def test_ensure_idempotent_returns_early():
    client = FakeClient()
    b = build_builder(client=client)
    await b._ensure_vector_collection()
    await b._ensure_graph()
    # Second call should early-return without re-listing.
    await b._ensure_vector_collection()
    await b._ensure_graph()
    assert len(client.created_collections) == 1
    assert len(client.created_graphs) == 1


@pytest.mark.asyncio
async def test_ensure_vector_collection_wraps_errors():
    class Boom(FakeClient):
        async def list_collections(self):
            raise ValueError("boom")

    b = build_builder(client=Boom())
    with pytest.raises(RuntimeError, match="Failed to initialize vector collection"):
        await b._ensure_vector_collection()


@pytest.mark.asyncio
async def test_ensure_graph_wraps_errors():
    class Boom(FakeClient):
        async def list_graphs(self):
            raise ValueError("boom")

    b = build_builder(client=Boom())
    with pytest.raises(RuntimeError, match="Failed to initialize graph"):
        await b._ensure_graph()


# ---------------------------------------------------------------------------
# embeddings
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_generate_embeddings_with_provider():
    class Provider:
        async def embed_batch(self, texts):
            return [[1.0, 2.0] for _ in texts]

    b = build_builder(embedding_provider=Provider())
    chunks = [make_chunk("a"), make_chunk("b")]
    embs = await b._generate_embeddings(chunks)
    assert embs == [[1.0, 2.0], [1.0, 2.0]]


@pytest.mark.asyncio
async def test_generate_embeddings_placeholder():
    b = build_builder(config=CodeIndexConfig(vector_dimension=4))
    chunks = [make_chunk("a"), make_chunk("b")]
    embs = await b._generate_embeddings(chunks)
    assert len(embs) == 2
    assert all(len(e) == 4 for e in embs)


@pytest.mark.asyncio
async def test_generate_query_embedding_with_provider():
    class Provider:
        async def embed_batch(self, texts):
            return [[7.0, 8.0]]

    b = build_builder(embedding_provider=Provider())
    emb = await b._generate_query_embedding("q")
    assert emb == [7.0, 8.0]


@pytest.mark.asyncio
async def test_generate_query_embedding_placeholder():
    b = build_builder(config=CodeIndexConfig(vector_dimension=4))
    emb = await b._generate_query_embedding("q")
    assert len(emb) == 4


# ---------------------------------------------------------------------------
# _insert_records
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_insert_records_rich_metadata():
    coll = FakeCollection()
    b = build_builder(client=FakeClient(collection=coll))
    from pathlib import Path

    chunk = make_chunk(
        "sym1",
        metadata={
            "symbol_id": "sym1",
            "symbol_type": "FUNCTION",
            "fully_qualified_name": "m.foo",
            "simple_name": "foo",
            "start_line": 1,
            "end_line": 5,
            "documentation": "doc",
            "signature": "sig",
            "modifiers": ["pub", "async"],
            "parameters": [{"name": "x"}],
            "return_type": "int",
            "complexity": 3,
        },
    )
    await b._insert_records([chunk], [[0.1, 0.2]], Path("a.py"), "python")
    assert len(coll.inserted) == 1
    rec = coll.inserted[0]
    assert rec["id"] == "sym1"
    assert rec["props"]["modifiers"] == "pub,async"
    assert rec["props"]["documentation"] == "doc"
    assert rec["props"]["complexity"] == "3"


@pytest.mark.asyncio
async def test_insert_records_falls_back_to_insert():
    coll = CollectionNoInsertRecords()
    b = build_builder(client=FakeClient(collection=coll))
    from pathlib import Path

    chunk = make_chunk("c1", metadata={})
    await b._insert_records([chunk], [[0.1]], Path("a.py"), "python")
    assert len(coll.inserted) == 1


@pytest.mark.asyncio
async def test_insert_records_empty_noop():
    coll = FakeCollection()
    b = build_builder(client=FakeClient(collection=coll))
    from pathlib import Path

    await b._insert_records([], [], Path("a.py"), "python")
    assert coll.inserted == []


# ---------------------------------------------------------------------------
# _insert_graph_data
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_insert_graph_data_nodes_edges_and_containment():
    graph = FakeGraph()
    b = build_builder(client=FakeClient(graph=graph))
    from pathlib import Path

    parent = make_chunk(
        "pid",
        metadata={
            "symbol_id": "pid",
            "simple_name": "MyClass",
            "symbol_type": "CLASS",
            "documentation": "cls doc",
            "signature": "class MyClass",
        },
    )
    child = make_chunk(
        "cid",
        metadata={
            "symbol_id": "cid",
            "simple_name": "method",
            "symbol_type": "METHOD",
            "scope_chain": ["MyClass"],
            "relations": [
                {"to": "other", "type": "CALLS", "confidence": 0.5},
                {"type": "CALLS"},  # missing 'to' -> skipped
            ],
        },
    )
    count = await b._insert_graph_data([parent, child], Path("a.py"), "python")
    # one relation edge (CALLS) + one containment edge (CONTAINS)
    assert count == 2
    assert len(graph.nodes) == 2
    edge_types = {e["edge_type"] for e in graph.edges}
    assert "CALLS" in edge_types
    assert "CONTAINS" in edge_types


@pytest.mark.asyncio
async def test_insert_graph_data_swallows_errors():
    graph = FakeGraph(raise_on_node=True)
    b = build_builder(client=FakeClient(graph=graph))
    from pathlib import Path

    chunk = make_chunk("c1", metadata={"symbol_id": "c1"})
    count = await b._insert_graph_data([chunk], Path("a.py"), "python")
    assert count == 0


# ---------------------------------------------------------------------------
# index_file
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_index_file_with_content_success():
    coll = FakeCollection()
    graph = FakeGraph()
    client = FakeClient(collection=coll, graph=graph)
    chunk = make_chunk("c1", metadata={"symbol_id": "c1"})
    b = build_builder(client=client, chunks=[chunk])
    res = await b.index_file("a.py", content="def f(): pass")
    assert res.files_processed == 1
    assert res.symbols_indexed == 1
    assert "a.py" in res.file_hashes


@pytest.mark.asyncio
async def test_index_file_read_failure(tmp_path):
    b = build_builder()
    missing = tmp_path / "does_not_exist.py"
    res = await b.index_file(str(missing))
    assert res.files_failed == 1
    assert res.errors and "Failed to read file" in res.errors[0]["error"]


@pytest.mark.asyncio
async def test_index_file_incremental_skip():
    chunk = make_chunk("c1", metadata={"symbol_id": "c1"})
    b = build_builder(chunks=[chunk])
    content = "def f(): pass"
    b._file_hashes["a.py"] = b._compute_hash(content)
    res = await b.index_file("a.py", content=content)
    assert res.files_skipped == 1


@pytest.mark.asyncio
async def test_index_file_force_overrides_incremental():
    coll = FakeCollection()
    client = FakeClient(collection=coll)
    chunk = make_chunk("c1", metadata={"symbol_id": "c1"})
    b = build_builder(client=client, chunks=[chunk])
    content = "def f(): pass"
    b._file_hashes["a.py"] = b._compute_hash(content)
    res = await b.index_file("a.py", content=content, force=True)
    assert res.files_processed == 1


@pytest.mark.asyncio
async def test_index_file_unknown_language_skipped():
    b = build_builder()
    res = await b.index_file("a.unknownext", content="data")
    assert res.files_skipped == 1


@pytest.mark.asyncio
async def test_index_file_no_chunks_skipped():
    b = build_builder(chunks=[])
    res = await b.index_file("a.py", content="def f(): pass")
    assert res.files_skipped == 1


@pytest.mark.asyncio
async def test_index_file_exception_during_processing():
    client = FakeClient()

    async def boom(name):
        raise RuntimeError("collection fetch failed")

    client.get_collection = boom
    chunk = make_chunk("c1", metadata={"symbol_id": "c1"})
    b = build_builder(client=client, chunks=[chunk])
    res = await b.index_file("a.py", content="def f(): pass")
    assert res.files_failed == 1
    assert res.errors


# ---------------------------------------------------------------------------
# index_directory / _collect_files
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_index_directory_recursive(tmp_path):
    (tmp_path / "good.py").write_text("def f(): pass")
    (tmp_path / "skip.pyc").write_text("bytecode")
    sub = tmp_path / "sub"
    sub.mkdir()
    (sub / "nested.py").write_text("def g(): pass")

    coll = FakeCollection()
    client = FakeClient(collection=coll)
    chunk = make_chunk("c1", metadata={"symbol_id": "c1"})
    b = build_builder(client=client, chunks=[chunk])

    seen = []

    def progress(path, cur, total):
        seen.append((path, cur, total))

    res = await b.index_directory(str(tmp_path), recursive=True, progress_callback=progress)
    # two .py files processed, .pyc excluded
    assert res.files_processed == 2
    assert len(seen) == 2


@pytest.mark.asyncio
async def test_index_directory_non_recursive(tmp_path):
    (tmp_path / "top.py").write_text("def f(): pass")
    sub = tmp_path / "sub"
    sub.mkdir()
    (sub / "nested.py").write_text("def g(): pass")

    client = FakeClient()
    chunk = make_chunk("c1", metadata={"symbol_id": "c1"})
    b = build_builder(client=client, chunks=[chunk])
    res = await b.index_directory(str(tmp_path), recursive=False)
    assert res.files_processed == 1


@pytest.mark.asyncio
async def test_collect_files_include_pattern_filter(tmp_path):
    (tmp_path / "keep.py").write_text("x")
    (tmp_path / "drop.py").write_text("y")
    cfg = CodeIndexConfig(include_patterns=["keep.py"])
    b = build_builder(config=cfg)
    from pathlib import Path

    files = b._collect_files(Path(str(tmp_path)), recursive=False)
    assert [f.name for f in files] == ["keep.py"]


# ---------------------------------------------------------------------------
# search_code
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_search_code_basic_no_filter():
    results_payload = [
        {
            "metadata": {
                "symbol_id": "s1",
                "symbol_type": "FUNCTION",
                "fully_qualified_name": "m.foo",
                "simple_name": "foo",
                "source_code": "def foo(): pass",
                "file_path": "a.py",
                "start_line": 1,
                "end_line": 2,
                "language": "python",
            },
            "score": 0.95,
        }
    ]
    coll = FakeCollection(search_results=results_payload)
    b = build_builder(client=FakeClient(collection=coll))
    out = await b.search_code("authentication", top_k=5)
    assert len(out) == 1
    assert out[0].symbol_id == "s1"
    assert out[0].score == 0.95
    assert coll.last_filter is None


@pytest.mark.asyncio
async def test_search_code_with_filters_and_context():
    payload = [{"metadata": {"symbol_id": "s1", "language": "python"}, "score": 0.5}]
    coll = FakeCollection(search_results=payload)
    graph = FakeGraph(
        traverse_map={
            ("CALLS", "incoming"): [{"id": "caller1"}],
            ("CALLS", "outgoing"): [{"id": "callee1"}],
            ("CONTAINS", "incoming"): [{"id": "parent1"}],
        }
    )
    b = build_builder(client=FakeClient(collection=coll, graph=graph))
    out = await b.search_code(
        "q",
        top_k=3,
        filter_language="python",
        filter_symbol_types=["FUNCTION", "CLASS"],
        include_context=True,
    )
    assert coll.last_filter == {
        "language": "python",
        "symbol_type": {"$in": ["FUNCTION", "CLASS"]},
    }
    assert out[0].callers == ["caller1"]
    assert out[0].callees == ["callee1"]
    assert out[0].parent_symbols == ["parent1"]


@pytest.mark.asyncio
async def test_enrich_with_graph_context_swallows_errors():
    class BadGraphClient(FakeClient):
        async def get_graph(self, name):
            raise RuntimeError("no graph")

    b = build_builder(client=BadGraphClient())
    result = CodeSearchResult(
        symbol_id="s",
        symbol_type="F",
        fully_qualified_name="m.s",
        simple_name="s",
        source_code="",
        file_path="",
        start_line=0,
        end_line=0,
        language="python",
        score=1.0,
    )
    # Should not raise.
    await b._enrich_with_graph_context(result)
    assert result.callers == []


# ---------------------------------------------------------------------------
# graph traversal helpers
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_resolve_symbol_id_treats_16char_alnum_as_id():
    b = build_builder()
    sid = await b._resolve_symbol_id("a1b2c3d4e5f6g7h8")
    assert sid == "a1b2c3d4e5f6g7h8"


@pytest.mark.asyncio
async def test_resolve_symbol_id_via_search():
    payload = [{"metadata": {"symbol_id": "resolved"}, "score": 0.9}]
    coll = FakeCollection(search_results=payload)
    b = build_builder(client=FakeClient(collection=coll))
    sid = await b._resolve_symbol_id("my_function")
    assert sid == "resolved"


@pytest.mark.asyncio
async def test_resolve_symbol_id_not_found():
    coll = FakeCollection(search_results=[])
    b = build_builder(client=FakeClient(collection=coll))
    sid = await b._resolve_symbol_id("unknown_symbol")
    assert sid is None


@pytest.mark.asyncio
async def test_find_callers_found():
    payload = [{"metadata": {"symbol_id": "sid"}, "score": 1.0}]
    coll = FakeCollection(search_results=payload)
    graph = FakeGraph(traverse_map={("CALLS", "incoming"): [{"id": "c1"}]})
    b = build_builder(client=FakeClient(collection=coll, graph=graph))
    callers = await b.find_callers("foo")
    assert callers == [{"id": "c1"}]


@pytest.mark.asyncio
async def test_find_callers_symbol_not_found_returns_empty():
    coll = FakeCollection(search_results=[])
    b = build_builder(client=FakeClient(collection=coll))
    assert await b.find_callers("nope") == []


@pytest.mark.asyncio
async def test_find_callees_found():
    payload = [{"metadata": {"symbol_id": "sid"}, "score": 1.0}]
    coll = FakeCollection(search_results=payload)
    graph = FakeGraph(traverse_map={("CALLS", "outgoing"): [{"id": "x"}]})
    b = build_builder(client=FakeClient(collection=coll, graph=graph))
    assert await b.find_callees("foo") == [{"id": "x"}]


@pytest.mark.asyncio
async def test_find_callees_not_found():
    coll = FakeCollection(search_results=[])
    b = build_builder(client=FakeClient(collection=coll))
    assert await b.find_callees("nope") == []


@pytest.mark.asyncio
async def test_find_usages_found():
    payload = [{"metadata": {"symbol_id": "sid"}, "score": 1.0}]
    coll = FakeCollection(search_results=payload)
    graph = FakeGraph(traverse_map={("REFERENCES", "incoming"): [{"id": "u"}]})
    b = build_builder(client=FakeClient(collection=coll, graph=graph))
    assert await b.find_usages("foo") == [{"id": "u"}]


@pytest.mark.asyncio
async def test_find_usages_not_found():
    coll = FakeCollection(search_results=[])
    b = build_builder(client=FakeClient(collection=coll))
    assert await b.find_usages("nope") == []


# ---------------------------------------------------------------------------
# impact analysis
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_impact_analysis_symbol_not_found():
    coll = FakeCollection(search_results=[])
    b = build_builder(client=FakeClient(collection=coll))
    out = await b.get_impact_analysis("nope")
    assert out == {"error": "Symbol not found"}


@pytest.mark.asyncio
async def test_impact_analysis_with_indirect_and_files():
    payload = [{"metadata": {"symbol_id": "sid"}, "score": 1.0}]
    coll = FakeCollection(search_results=payload)
    direct = [{"id": "d1", "properties": {"file_path": "f1.py"}}]
    indirect = [
        {"id": "d1", "properties": {"file_path": "f1.py"}},  # dup of direct
        {"id": "i1", "properties": {"file_path": "f2.py"}},
    ]

    class DepthGraph(FakeGraph):
        async def traverse(self, start_node_id, edge_type, direction, max_depth):
            if max_depth == 1:
                return direct
            return indirect

    graph = DepthGraph()
    b = build_builder(client=FakeClient(collection=coll, graph=graph))
    out = await b.get_impact_analysis("foo", max_depth=3)
    assert out["symbol"] == "foo"
    assert out["direct_callers"] == direct
    assert out["indirect_callers"] == [
        {"id": "i1", "properties": {"file_path": "f2.py"}}
    ]
    assert set(out["dependent_files"]) == {"f1.py", "f2.py"}
    assert out["total_affected"] == 2


@pytest.mark.asyncio
async def test_impact_analysis_depth_one_only():
    payload = [{"metadata": {"symbol_id": "sid"}, "score": 1.0}]
    coll = FakeCollection(search_results=payload)
    graph = FakeGraph(traverse_map={("CALLS", "incoming"): [{"id": "d1"}]})
    b = build_builder(client=FakeClient(collection=coll, graph=graph))
    out = await b.get_impact_analysis("foo", max_depth=1)
    assert out["indirect_callers"] == []
    assert out["total_affected"] == 1


# ---------------------------------------------------------------------------
# delete_file_index
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_delete_file_index_success():
    coll = FakeCollection()
    client = FakeClient(collection=coll)
    b = build_builder(client=client)
    b._file_hashes["a.py"] = "hash"
    ok = await b.delete_file_index("a.py")
    assert ok is True
    assert coll.deleted_filters == [{"file_path": "a.py"}]
    assert "a.py" not in b._file_hashes


@pytest.mark.asyncio
async def test_delete_file_index_failure_returns_false():
    class BadClient(FakeClient):
        async def get_collection(self, name):
            raise RuntimeError("nope")

    b = build_builder(client=BadClient())
    ok = await b.delete_file_index("a.py")
    assert ok is False


# ---------------------------------------------------------------------------
# convenience function
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_create_code_knowledge_store(tmp_path):
    (tmp_path / "mod.py").write_text("def f(): pass")
    client = FakeClient()
    # Patch the chunker on every builder instance the factory builds by
    # constructing then re-pointing; the factory builds its own builder, so we
    # patch the class chunker via monkeypatching the instance after creation is
    # not possible here. Instead, supply chunks indirectly by replacing chunk.
    import proximadb_sdk.code_knowledge as ck

    orig = ck.CodeChunkingStrategy

    class _Chunker:
        def __init__(self, *a, **k):
            pass

        def chunk(self, text, source_id, metadata):
            return [make_chunk("c1", metadata={"symbol_id": "c1"})]

    ck.CodeChunkingStrategy = _Chunker
    try:
        builder, result = await create_code_knowledge_store(client, str(tmp_path))
    finally:
        ck.CodeChunkingStrategy = orig
    assert isinstance(builder, CodeKnowledgeBuilder)
    assert result.files_processed == 1
