"""Offline unit tests for proximadb_sdk.code_knowledge.

Fully offline: no network, no server, no tree-sitter parsing. The builder's
real CodeChunkingStrategy chunker is replaced per-test with a stub that returns
crafted TextChunk objects, and the ProximaDB client is a MagicMock whose async
methods are AsyncMocks returning crafted responses.
"""

import asyncio
from pathlib import Path
from unittest.mock import AsyncMock, MagicMock

import pytest

from proximadb_sdk.chunking_strategies.base import TextChunk
from proximadb_sdk.code_knowledge import (
    CodeIndexConfig,
    CodeKnowledgeBuilder,
    CodeSearchResult,
    IndexingResult,
    create_code_knowledge_store,
)


def run(coro):
    # Use a fresh, isolated event loop per call rather than
    # asyncio.get_event_loop(): when run in the same process as other test
    # files, the shared default loop may already be closed (raising
    # "Event loop is closed"), which previously caused 55 cross-file failures.
    loop = asyncio.new_event_loop()
    try:
        return loop.run_until_complete(coro)
    finally:
        loop.close()


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def make_chunk(
    chunk_id="c1",
    text="def foo():\n    return 1",
    symbol_id=None,
    symbol_type="FUNCTION",
    fqn="mod.foo",
    simple_name="foo",
    extra=None,
):
    meta = {
        "symbol_type": symbol_type,
        "fully_qualified_name": fqn,
        "simple_name": simple_name,
        "start_line": 1,
        "end_line": 2,
    }
    if symbol_id is not None:
        meta["symbol_id"] = symbol_id
    if extra:
        meta.update(extra)
    return TextChunk(
        text=text, start_pos=0, end_pos=len(text), chunk_id=chunk_id, metadata=meta
    )


def make_client():
    """A MagicMock client whose collection/graph methods are async."""
    client = MagicMock()
    client.list_collections = AsyncMock(return_value=[])
    client.create_collection = AsyncMock(return_value=None)
    client.list_graphs = AsyncMock(return_value=[])
    client.create_graph = AsyncMock(return_value=None)

    collection = MagicMock()
    collection.insert_records = AsyncMock(return_value=None)
    collection.insert = AsyncMock(return_value=None)
    collection.search = AsyncMock(return_value=[])
    collection.delete = AsyncMock(return_value=None)
    client.get_collection = AsyncMock(return_value=collection)

    graph = MagicMock()
    graph.insert_node = AsyncMock(return_value=None)
    graph.insert_edge = AsyncMock(return_value=None)
    graph.traverse = AsyncMock(return_value=[])
    client.get_graph = AsyncMock(return_value=graph)

    return client, collection, graph


def make_builder(client=None, config=None, embedding_provider=None):
    if client is None:
        client, _, _ = make_client()
    return CodeKnowledgeBuilder(
        client, config=config, embedding_provider=embedding_provider
    )


def _stub_chunker(builder, chunks):
    builder._chunker.chunk = MagicMock(return_value=chunks)


# ---------------------------------------------------------------------------
# Dataclass / config tests
# ---------------------------------------------------------------------------


def test_config_defaults():
    cfg = CodeIndexConfig()
    assert cfg.vector_collection_name == "code_symbols"
    assert cfg.vector_dimension == 1536
    assert cfg.graph_name == "code_graph"
    assert "*.pyc" in cfg.exclude_patterns
    assert cfg.include_patterns == ["*"]
    assert cfg.hash_algorithm == "sha256"


def test_indexing_result_defaults():
    r = IndexingResult()
    assert r.files_processed == 0
    assert r.errors == []
    assert r.file_hashes == {}


def test_code_search_result_defaults():
    r = CodeSearchResult(
        symbol_id="s",
        symbol_type="FUNCTION",
        fully_qualified_name="m.f",
        simple_name="f",
        source_code="code",
        file_path="/a.py",
        start_line=1,
        end_line=2,
        language="python",
        score=0.9,
    )
    assert r.callers == []
    assert r.callees == []
    assert r.parent_symbols == []
    assert r.documentation is None


# ---------------------------------------------------------------------------
# init
# ---------------------------------------------------------------------------


def test_init_with_defaults():
    b = make_builder()
    assert isinstance(b.config, CodeIndexConfig)
    assert b.embedding_provider is None
    assert b._file_hashes == {}
    assert b._vector_collection_ready is False
    assert b._graph_ready is False


def test_init_with_custom_config():
    cfg = CodeIndexConfig(vector_collection_name="custom", vector_dimension=8)
    b = make_builder(config=cfg)
    assert b.config.vector_collection_name == "custom"


# ---------------------------------------------------------------------------
# initialize / _ensure_vector_collection / _ensure_graph
# ---------------------------------------------------------------------------


def test_initialize_creates_resources():
    client, _, _ = make_client()
    b = make_builder(client)
    run(b.initialize())
    client.create_collection.assert_awaited_once()
    client.create_graph.assert_awaited_once()
    assert b._vector_collection_ready
    assert b._graph_ready


def test_initialize_idempotent():
    client, _, _ = make_client()
    b = make_builder(client)
    run(b.initialize())
    run(b.initialize())
    client.create_collection.assert_awaited_once()
    client.create_graph.assert_awaited_once()


def test_ensure_vector_collection_skips_when_exists():
    client, _, _ = make_client()
    existing = MagicMock()
    existing.name = "code_symbols"
    client.list_collections = AsyncMock(return_value=[existing])
    b = make_builder(client)
    run(b._ensure_vector_collection())
    client.create_collection.assert_not_awaited()
    assert b._vector_collection_ready


def test_ensure_vector_collection_string_names():
    client, _, _ = make_client()
    client.list_collections = AsyncMock(return_value=["other"])
    b = make_builder(client)
    run(b._ensure_vector_collection())
    client.create_collection.assert_awaited_once()


def test_ensure_vector_collection_already_ready_noop():
    client, _, _ = make_client()
    b = make_builder(client)
    b._vector_collection_ready = True
    run(b._ensure_vector_collection())
    client.list_collections.assert_not_awaited()


def test_ensure_vector_collection_error_wrapped():
    client, _, _ = make_client()
    client.list_collections = AsyncMock(side_effect=ValueError("boom"))
    b = make_builder(client)
    with pytest.raises(RuntimeError, match="Failed to initialize vector collection"):
        run(b._ensure_vector_collection())


def test_ensure_graph_skips_when_exists():
    client, _, _ = make_client()
    g = MagicMock()
    g.name = "code_graph"
    client.list_graphs = AsyncMock(return_value=[g])
    b = make_builder(client)
    run(b._ensure_graph())
    client.create_graph.assert_not_awaited()


def test_ensure_graph_already_ready_noop():
    client, _, _ = make_client()
    b = make_builder(client)
    b._graph_ready = True
    run(b._ensure_graph())
    client.list_graphs.assert_not_awaited()


def test_ensure_graph_string_names():
    client, _, _ = make_client()
    client.list_graphs = AsyncMock(return_value=["other_graph"])
    b = make_builder(client)
    run(b._ensure_graph())
    client.create_graph.assert_awaited_once()


def test_ensure_graph_error_wrapped():
    client, _, _ = make_client()
    client.list_graphs = AsyncMock(side_effect=ValueError("boom"))
    b = make_builder(client)
    with pytest.raises(RuntimeError, match="Failed to initialize graph"):
        run(b._ensure_graph())


# ---------------------------------------------------------------------------
# hashing / pattern matching / placeholder embedding
# ---------------------------------------------------------------------------


def test_compute_hash_sha256():
    b = make_builder()
    assert len(b._compute_hash("hello")) == 64


def test_compute_hash_md5():
    b = make_builder(config=CodeIndexConfig(hash_algorithm="md5"))
    assert len(b._compute_hash("hello")) == 32


def test_compute_hash_unknown_falls_back_sha256():
    b = make_builder(config=CodeIndexConfig(hash_algorithm="crc32"))
    assert len(b._compute_hash("hello")) == 64


def test_matches_patterns():
    b = make_builder()
    assert b._matches_patterns("a.pyc", ["*.pyc"])
    assert not b._matches_patterns("a.py", ["*.pyc"])
    assert b._matches_patterns("anything", ["*"])
    assert not b._matches_patterns("x", [])


def test_placeholder_embedding_dimension():
    b = make_builder(config=CodeIndexConfig(vector_dimension=16))
    emb = b._generate_placeholder_embedding("some text")
    assert len(emb) == 16
    assert all(-1.0 <= v <= 1.0 for v in emb)


def test_placeholder_embedding_deterministic():
    b = make_builder(config=CodeIndexConfig(vector_dimension=8))
    assert b._generate_placeholder_embedding("x") == b._generate_placeholder_embedding(
        "x"
    )


def test_placeholder_embedding_padding_large_dim():
    b = make_builder(config=CodeIndexConfig(vector_dimension=2048))
    emb = b._generate_placeholder_embedding("short")
    assert len(emb) == 2048
    assert emb[-1] == 0.0


# ---------------------------------------------------------------------------
# embeddings generation
# ---------------------------------------------------------------------------


def test_generate_embeddings_with_provider():
    provider = MagicMock()
    provider.embed_batch = AsyncMock(return_value=[[1.0, 2.0], [3.0, 4.0]])
    b = make_builder(embedding_provider=provider)
    chunks = [make_chunk("a"), make_chunk("b")]
    out = run(b._generate_embeddings(chunks))
    assert out == [[1.0, 2.0], [3.0, 4.0]]
    provider.embed_batch.assert_awaited_once()


def test_generate_embeddings_default_placeholder():
    b = make_builder(config=CodeIndexConfig(vector_dimension=8))
    chunks = [make_chunk("a"), make_chunk("b")]
    out = run(b._generate_embeddings(chunks))
    assert len(out) == 2
    assert all(len(e) == 8 for e in out)


def test_generate_query_embedding_with_provider():
    provider = MagicMock()
    provider.embed_batch = AsyncMock(return_value=[[9.0]])
    b = make_builder(embedding_provider=provider)
    assert run(b._generate_query_embedding("q")) == [9.0]


def test_generate_query_embedding_default():
    b = make_builder(config=CodeIndexConfig(vector_dimension=4))
    assert len(run(b._generate_query_embedding("q"))) == 4


# ---------------------------------------------------------------------------
# _prepare_text_for_embedding branches
# ---------------------------------------------------------------------------


def test_prepare_text_full_metadata():
    b = make_builder(config=CodeIndexConfig(max_content_length=10))
    chunk = make_chunk(
        text="x" * 50,
        extra={
            "fully_qualified_name": "m.f",
            "documentation": "d" * 600,
            "signature": "def f()",
        },
    )
    out = b._prepare_text_for_embedding(chunk)
    assert "Symbol: m.f" in out
    assert "Documentation:" in out
    assert "Signature: def f()" in out
    assert out.endswith("...")


def test_prepare_text_minimal():
    b = make_builder()
    chunk = make_chunk(text="short")
    chunk.metadata.pop("fully_qualified_name", None)
    out = b._prepare_text_for_embedding(chunk)
    assert "Code:" in out
    assert "Symbol:" not in out


# ---------------------------------------------------------------------------
# _insert_records
# ---------------------------------------------------------------------------


def test_insert_records_uses_insert_records():
    client, collection, _ = make_client()
    b = make_builder(client)
    chunks = [
        make_chunk(
            "c1",
            symbol_id="sid1",
            extra={
                "documentation": "doc",
                "signature": "sig",
                "modifiers": ["pub", "static"],
                "parameters": [{"name": "a"}],
                "return_type": "int",
                "complexity": 5,
            },
        )
    ]
    run(b._insert_records(chunks, [[0.1, 0.2]], Path("/a.py"), "python"))
    collection.insert_records.assert_awaited_once()
    rec = collection.insert_records.await_args.args[0][0]
    assert rec["id"] == "sid1"
    assert rec["props"]["modifiers"] == "pub,static"
    assert rec["props"]["return_type"] == "int"
    assert rec["props"]["complexity"] == "5"
    assert rec["props"]["parameters"] == "[{'name': 'a'}]"


def test_insert_records_falls_back_to_insert():
    client, _, _ = make_client()
    coll = MagicMock(spec=["insert"])
    coll.insert = AsyncMock(return_value=None)
    client.get_collection = AsyncMock(return_value=coll)
    b = make_builder(client)
    run(b._insert_records([make_chunk("c1")], [[0.1]], Path("/a.py"), "python"))
    coll.insert.assert_awaited_once()


def test_insert_records_empty_noop():
    client, collection, _ = make_client()
    b = make_builder(client)
    run(b._insert_records([], [], Path("/a.py"), "python"))
    collection.insert_records.assert_not_awaited()


def test_insert_records_default_symbol_id_uses_chunk_id():
    client, collection, _ = make_client()
    b = make_builder(client)
    run(b._insert_records([make_chunk("chunkid", symbol_id=None)], [[0.1]], Path("/a.py"), "python"))
    rec = collection.insert_records.await_args.args[0][0]
    assert rec["id"] == "chunkid"


# ---------------------------------------------------------------------------
# _insert_graph_data
# ---------------------------------------------------------------------------


def test_insert_graph_data_nodes_edges_containment():
    client, _, graph = make_client()
    b = make_builder(client)
    parent = make_chunk(
        "p",
        symbol_id="pid",
        simple_name="Klass",
        symbol_type="CLASS",
        extra={"documentation": "doc", "signature": "class Klass"},
    )
    child = make_chunk(
        "c",
        symbol_id="cid",
        simple_name="method",
        symbol_type="METHOD",
        extra={
            "relations": [{"to": "other", "type": "CALLS", "confidence": 0.5}],
            "scope_chain": ["Klass"],
        },
    )
    count = run(b._insert_graph_data([parent, child], Path("/a.py"), "python"))
    assert count == 2
    assert graph.insert_node.await_count == 2
    assert graph.insert_edge.await_count == 2


def test_insert_graph_data_relation_without_to_skipped():
    client, _, graph = make_client()
    b = make_builder(client)
    chunk = make_chunk("c", symbol_id="cid", extra={"relations": [{"type": "CALLS"}]})
    count = run(b._insert_graph_data([chunk], Path("/a.py"), "python"))
    assert count == 0
    graph.insert_edge.assert_not_awaited()


def test_insert_graph_data_relation_default_type():
    client, _, graph = make_client()
    b = make_builder(client)
    chunk = make_chunk("c", symbol_id="cid", extra={"relations": [{"to": "z"}]})
    count = run(b._insert_graph_data([chunk], Path("/a.py"), "python"))
    assert count == 1
    edge = graph.insert_edge.await_args.args[0]
    assert edge["edge_type"] == "REFERENCES"
    assert edge["properties"]["confidence"] == 1.0


def test_insert_graph_data_scope_chain_no_match():
    client, _, graph = make_client()
    b = make_builder(client)
    chunk = make_chunk(
        "c", symbol_id="cid", simple_name="method", extra={"scope_chain": ["Missing"]}
    )
    count = run(b._insert_graph_data([chunk], Path("/a.py"), "python"))
    assert count == 0


def test_insert_graph_data_swallows_errors():
    client, _, _ = make_client()
    client.get_graph = AsyncMock(side_effect=RuntimeError("graph down"))
    b = make_builder(client)
    count = run(b._insert_graph_data([make_chunk("c")], Path("/a.py"), "python"))
    assert count == 0


# ---------------------------------------------------------------------------
# index_file
# ---------------------------------------------------------------------------


def test_index_file_with_content():
    client, collection, _ = make_client()
    b = make_builder(client, config=CodeIndexConfig(vector_dimension=8))
    _stub_chunker(b, [make_chunk("c1", symbol_id="sid")])
    res = run(b.index_file("/some/file.py", content="def foo(): pass"))
    assert res.files_processed == 1
    assert res.symbols_indexed == 1
    assert b.get_file_hash("/some/file.py") is not None
    collection.insert_records.assert_awaited()


def test_index_file_path_object():
    client, _, _ = make_client()
    b = make_builder(client, config=CodeIndexConfig(vector_dimension=8))
    _stub_chunker(b, [make_chunk("c1", symbol_id="sid")])
    res = run(b.index_file(Path("/some/file.py"), content="x"))
    assert res.files_processed == 1


def test_index_file_read_failure():
    b = make_builder(config=CodeIndexConfig(vector_dimension=8))
    res = run(b.index_file("/no/such/path/xyz123.py"))
    assert res.files_failed == 1
    assert res.errors and "Failed to read file" in res.errors[0]["error"]


def test_index_file_unsupported_extension_skipped():
    b = make_builder()
    res = run(b.index_file("/a/file.unknownext", content="data"))
    assert res.files_skipped == 1


def test_index_file_incremental_skip():
    b = make_builder(config=CodeIndexConfig(vector_dimension=8))
    _stub_chunker(b, [make_chunk("c1", symbol_id="sid")])
    assert run(b.index_file("/f.py", content="abc")).files_processed == 1
    assert run(b.index_file("/f.py", content="abc")).files_skipped == 1


def test_index_file_force_reindex():
    b = make_builder(config=CodeIndexConfig(vector_dimension=8))
    _stub_chunker(b, [make_chunk("c1", symbol_id="sid")])
    run(b.index_file("/f.py", content="abc"))
    assert run(b.index_file("/f.py", content="abc", force=True)).files_processed == 1


def test_index_file_no_chunks_skipped():
    b = make_builder(config=CodeIndexConfig(vector_dimension=8))
    _stub_chunker(b, [])
    assert run(b.index_file("/f.py", content="abc")).files_skipped == 1


def test_index_file_exception_during_processing():
    b = make_builder(config=CodeIndexConfig(vector_dimension=8))
    b._chunker.chunk = MagicMock(side_effect=RuntimeError("parse error"))
    res = run(b.index_file("/f.py", content="abc"))
    assert res.files_failed == 1
    assert "parse error" in res.errors[0]["error"]


# ---------------------------------------------------------------------------
# index_directory + _collect_files
# ---------------------------------------------------------------------------


def test_index_directory_aggregates(tmp_path):
    (tmp_path / "a.py").write_text("def a(): pass")
    (tmp_path / "b.py").write_text("def b(): pass")
    (tmp_path / "skip.txt").write_text("not code")
    sub = tmp_path / "sub"
    sub.mkdir()
    (sub / "c.py").write_text("def c(): pass")

    b = make_builder(config=CodeIndexConfig(vector_dimension=8))
    _stub_chunker(b, [make_chunk("c1", symbol_id="sid")])

    seen = []
    res = run(
        b.index_directory(
            tmp_path,
            recursive=True,
            progress_callback=lambda p, cur, tot: seen.append((p, cur, tot)),
        )
    )
    assert res.files_processed == 3
    assert len(seen) == 3
    assert seen[0][2] == 3


def test_index_directory_non_recursive(tmp_path):
    (tmp_path / "a.py").write_text("def a(): pass")
    sub = tmp_path / "sub"
    sub.mkdir()
    (sub / "c.py").write_text("def c(): pass")

    b = make_builder(config=CodeIndexConfig(vector_dimension=8))
    _stub_chunker(b, [make_chunk("c1", symbol_id="sid")])
    assert run(b.index_directory(tmp_path, recursive=False)).files_processed == 1


def test_collect_files_exclude_pattern(tmp_path):
    (tmp_path / "a.py").write_text("x")
    nm = tmp_path / "node_modules"
    nm.mkdir()
    (nm / "dep.py").write_text("x")
    b = make_builder()
    names = [f.name for f in b._collect_files(tmp_path, recursive=True)]
    assert "a.py" in names
    assert "dep.py" not in names


def test_collect_files_include_pattern_filter(tmp_path):
    (tmp_path / "keep.py").write_text("x")
    (tmp_path / "drop.py").write_text("x")
    b = make_builder(config=CodeIndexConfig(include_patterns=["keep*"]))
    names = [f.name for f in b._collect_files(tmp_path, recursive=True)]
    assert names == ["keep.py"]


def test_collect_files_skips_directories_and_unsupported(tmp_path):
    (tmp_path / "a.py").write_text("x")
    (tmp_path / "readme.md").write_text("x")  # .md is supported? exclude via ext check
    (tmp_path / "data.bin").write_text("x")  # unsupported
    sub = tmp_path / "pkg"
    sub.mkdir()  # a directory should be skipped
    b = make_builder()
    files = b._collect_files(tmp_path, recursive=False)
    names = [f.name for f in files]
    assert "a.py" in names
    assert "data.bin" not in names


# ---------------------------------------------------------------------------
# search_code
# ---------------------------------------------------------------------------


def _search_hit(symbol_id="sid", **md):
    metadata = {
        "symbol_id": symbol_id,
        "symbol_type": "FUNCTION",
        "fully_qualified_name": "m.f",
        "simple_name": "f",
        "source_code": "def f(): pass",
        "file_path": "/a.py",
        "start_line": 1,
        "end_line": 2,
        "language": "python",
        "documentation": "doc",
        "signature": "def f()",
    }
    metadata.update(md)
    return {"metadata": metadata, "score": 0.88}


def test_search_code_basic():
    client, collection, _ = make_client()
    collection.search = AsyncMock(return_value=[_search_hit()])
    b = make_builder(client, config=CodeIndexConfig(vector_dimension=8))
    out = run(b.search_code("find f", top_k=5))
    assert len(out) == 1
    assert isinstance(out[0], CodeSearchResult)
    assert out[0].symbol_id == "sid"
    assert out[0].score == 0.88
    assert collection.search.await_args.kwargs["filter"] is None
    assert collection.search.await_args.kwargs["top_k"] == 5


def test_search_code_missing_metadata_defaults():
    client, collection, _ = make_client()
    collection.search = AsyncMock(return_value=[{}])  # no metadata, no score
    b = make_builder(client, config=CodeIndexConfig(vector_dimension=8))
    out = run(b.search_code("q"))
    assert out[0].symbol_id == ""
    assert out[0].score == 0.0
    assert out[0].language == ""


def test_search_code_with_filters():
    client, collection, _ = make_client()
    collection.search = AsyncMock(return_value=[])
    b = make_builder(client, config=CodeIndexConfig(vector_dimension=8))
    run(
        b.search_code(
            "q", filter_language="python", filter_symbol_types=["FUNCTION", "CLASS"]
        )
    )
    flt = collection.search.await_args.kwargs["filter"]
    assert flt["language"] == "python"
    assert flt["symbol_type"] == {"$in": ["FUNCTION", "CLASS"]}


def test_search_code_with_context():
    client, collection, graph = make_client()
    collection.search = AsyncMock(return_value=[_search_hit("sid")])
    graph.traverse = AsyncMock(
        side_effect=[[{"id": "caller1"}], [{"id": "callee1"}], [{"id": "parent1"}]]
    )
    b = make_builder(client, config=CodeIndexConfig(vector_dimension=8))
    out = run(b.search_code("q", include_context=True))
    assert out[0].callers == ["caller1"]
    assert out[0].callees == ["callee1"]
    assert out[0].parent_symbols == ["parent1"]


def test_search_code_context_swallows_error():
    client, collection, graph = make_client()
    collection.search = AsyncMock(return_value=[_search_hit("sid")])
    graph.traverse = AsyncMock(side_effect=RuntimeError("graph error"))
    b = make_builder(client, config=CodeIndexConfig(vector_dimension=8))
    out = run(b.search_code("q", include_context=True))
    assert out[0].callers == []


# ---------------------------------------------------------------------------
# graph traversal helpers
# ---------------------------------------------------------------------------


def test_resolve_symbol_id_as_id():
    b = make_builder(config=CodeIndexConfig(vector_dimension=8))
    assert run(b._resolve_symbol_id("abcd1234efgh5678")) == "abcd1234efgh5678"


def test_resolve_symbol_id_by_search():
    client, collection, _ = make_client()
    collection.search = AsyncMock(return_value=[_search_hit("resolved")])
    b = make_builder(client, config=CodeIndexConfig(vector_dimension=8))
    assert run(b._resolve_symbol_id("my_func")) == "resolved"


def test_resolve_symbol_id_not_found():
    client, collection, _ = make_client()
    collection.search = AsyncMock(return_value=[])
    b = make_builder(client, config=CodeIndexConfig(vector_dimension=8))
    assert run(b._resolve_symbol_id("missing")) is None


def test_find_callers():
    client, collection, graph = make_client()
    collection.search = AsyncMock(return_value=[_search_hit("sid")])
    graph.traverse = AsyncMock(return_value=[{"id": "caller"}])
    b = make_builder(client, config=CodeIndexConfig(vector_dimension=8))
    out = run(b.find_callers("my_func", max_depth=2))
    assert out == [{"id": "caller"}]
    assert graph.traverse.await_args.kwargs["direction"] == "incoming"
    assert graph.traverse.await_args.kwargs["max_depth"] == 2


def test_find_callers_unresolved():
    client, collection, _ = make_client()
    collection.search = AsyncMock(return_value=[])
    b = make_builder(client, config=CodeIndexConfig(vector_dimension=8))
    assert run(b.find_callers("unknown")) == []


def test_find_callees():
    client, collection, graph = make_client()
    collection.search = AsyncMock(return_value=[_search_hit("sid")])
    graph.traverse = AsyncMock(return_value=[{"id": "callee"}])
    b = make_builder(client, config=CodeIndexConfig(vector_dimension=8))
    out = run(b.find_callees("my_func"))
    assert out == [{"id": "callee"}]
    assert graph.traverse.await_args.kwargs["direction"] == "outgoing"


def test_find_callees_unresolved():
    client, collection, _ = make_client()
    collection.search = AsyncMock(return_value=[])
    b = make_builder(client, config=CodeIndexConfig(vector_dimension=8))
    assert run(b.find_callees("unknown")) == []


def test_find_usages():
    client, collection, graph = make_client()
    collection.search = AsyncMock(return_value=[_search_hit("sid")])
    graph.traverse = AsyncMock(return_value=[{"id": "ref"}])
    b = make_builder(client, config=CodeIndexConfig(vector_dimension=8))
    out = run(b.find_usages("my_func"))
    assert out == [{"id": "ref"}]
    assert graph.traverse.await_args.kwargs["edge_type"] == "REFERENCES"


def test_find_usages_unresolved():
    client, collection, _ = make_client()
    collection.search = AsyncMock(return_value=[])
    b = make_builder(client, config=CodeIndexConfig(vector_dimension=8))
    assert run(b.find_usages("unknown")) == []


# ---------------------------------------------------------------------------
# get_impact_analysis
# ---------------------------------------------------------------------------


def test_impact_analysis_not_found():
    client, collection, _ = make_client()
    collection.search = AsyncMock(return_value=[])
    b = make_builder(client, config=CodeIndexConfig(vector_dimension=8))
    assert run(b.get_impact_analysis("missing")) == {"error": "Symbol not found"}


def test_impact_analysis_with_indirect():
    client, collection, graph = make_client()
    collection.search = AsyncMock(return_value=[_search_hit("sid")])
    direct = [{"id": "d1", "properties": {"file_path": "/d1.py"}}]
    indirect = [
        {"id": "d1", "properties": {"file_path": "/d1.py"}},
        {"id": "i1", "properties": {"file_path": "/i1.py"}},
    ]
    graph.traverse = AsyncMock(side_effect=[direct, indirect])
    b = make_builder(client, config=CodeIndexConfig(vector_dimension=8))
    out = run(b.get_impact_analysis("sym", max_depth=3))
    assert out["direct_callers"] == direct
    assert out["indirect_callers"] == [indirect[1]]
    assert out["total_affected"] == 2
    assert set(out["dependent_files"]) == {"/d1.py", "/i1.py"}


def test_impact_analysis_depth_one():
    client, collection, graph = make_client()
    collection.search = AsyncMock(return_value=[_search_hit("sid")])
    direct = [{"id": "d1", "properties": {"file_path": "/d1.py"}}]
    graph.traverse = AsyncMock(return_value=direct)
    b = make_builder(client, config=CodeIndexConfig(vector_dimension=8))
    out = run(b.get_impact_analysis("sym", max_depth=1))
    assert out["indirect_callers"] == []
    assert out["total_affected"] == 1


def test_impact_analysis_caller_without_file_path():
    client, collection, graph = make_client()
    collection.search = AsyncMock(return_value=[_search_hit("sid")])
    direct = [{"id": "d1", "properties": {}}]
    graph.traverse = AsyncMock(return_value=direct)
    b = make_builder(client, config=CodeIndexConfig(vector_dimension=8))
    out = run(b.get_impact_analysis("sym", max_depth=1))
    assert out["dependent_files"] == []


# ---------------------------------------------------------------------------
# delete_file_index + accessors
# ---------------------------------------------------------------------------


def test_delete_file_index_success():
    client, collection, _ = make_client()
    b = make_builder(client, config=CodeIndexConfig(vector_dimension=8))
    b._file_hashes["/a.py"] = "hash"
    assert run(b.delete_file_index("/a.py")) is True
    collection.delete.assert_awaited_once()
    assert "/a.py" not in b._file_hashes


def test_delete_file_index_path_object():
    client, _, _ = make_client()
    b = make_builder(client, config=CodeIndexConfig(vector_dimension=8))
    assert run(b.delete_file_index(Path("/a.py"))) is True


def test_delete_file_index_failure():
    client, collection, _ = make_client()
    collection.delete = AsyncMock(side_effect=RuntimeError("delete failed"))
    b = make_builder(client, config=CodeIndexConfig(vector_dimension=8))
    assert run(b.delete_file_index("/a.py")) is False


def test_get_indexed_files_and_hash():
    b = make_builder()
    b._file_hashes["/x.py"] = "h1"
    assert b.get_indexed_files() == ["/x.py"]
    assert b.get_file_hash("/x.py") == "h1"
    assert b.get_file_hash("/missing.py") is None


# ---------------------------------------------------------------------------
# convenience function
# ---------------------------------------------------------------------------


def test_create_code_knowledge_store(tmp_path):
    (tmp_path / "a.py").write_text("def a(): pass")
    client, _, _ = make_client()

    chunks = [make_chunk("c1", symbol_id="sid")]
    orig_init = CodeKnowledgeBuilder.__init__

    def patched_init(self, *args, **kwargs):
        orig_init(self, *args, **kwargs)
        self._chunker.chunk = MagicMock(return_value=chunks)

    CodeKnowledgeBuilder.__init__ = patched_init
    try:
        builder, result = run(
            create_code_knowledge_store(
                client, tmp_path, config=CodeIndexConfig(vector_dimension=8)
            )
        )
    finally:
        CodeKnowledgeBuilder.__init__ = orig_init

    assert isinstance(builder, CodeKnowledgeBuilder)
    assert result.files_processed == 1
