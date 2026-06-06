"""Offline unit tests for proximadb_sdk.embedded_multi.

Mocks the underlying EmbeddedProtocolAdapter so no real embedded DB boots.
Covers multi-model routing: vector/document/graph/time-series indexing,
chunking, metrics extraction, hybrid search ranking/filtering, repo scanning.
"""

from __future__ import annotations

from datetime import datetime, timezone
from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock

import pytest

from proximadb_sdk import embedded_multi
from proximadb_sdk.embedded_multi import EmbeddedMultiModelProvider


def make_adapter(get_collection_return=None):
    """Build a MagicMock adapter standing in for EmbeddedProtocolAdapter."""
    adapter = MagicMock()
    # _db with no start/stop unless overridden (use a plain object)
    adapter._db = SimpleNamespace()
    adapter.get_collection.return_value = get_collection_return
    adapter.create_collection.return_value = {"name": "c"}
    adapter.create_document_collection.return_value = {"name": "d"}
    adapter.create_timeseries_collection.return_value = {"name": "ts"}
    adapter.insert_document.return_value = {"ok": True}
    adapter.create_node.return_value = {"ok": True}
    adapter.ingest_timeseries.return_value = {"ok": True}
    adapter.insert_records.return_value = {"inserted": 1}
    adapter.search.return_value = []
    adapter.execute_graph_query.return_value = {"results": []}
    adapter.close.return_value = None
    return adapter


def attach(provider, adapter):
    """Pre-initialize a provider with a mocked adapter (skip real init)."""
    provider._adapter = adapter
    provider._is_initialized = True
    return provider


# ---------------------------------------------------------------------------
# Construction
# ---------------------------------------------------------------------------


def test_init_defaults():
    p = EmbeddedMultiModelProvider()
    assert p.workspace == "default_workspace"
    assert p.embedding_model == "all-MiniLM-L6-v2"
    assert p._vector_collection == "default_workspace_vectors"
    assert p._document_collection == "default_workspace_documents"
    assert p._graph_collection == "default_workspace_graph"
    assert p._timeseries_collection == "default_workspace_metrics"
    assert p._adapter is None
    assert p._is_initialized is False


def test_init_custom():
    p = EmbeddedMultiModelProvider(
        data_dir="~/foo",
        workspace="ws",
        embedding_model="custom-model",
        config={"x": 1},
    )
    assert "~" not in p.data_dir  # expanduser applied
    assert p.workspace == "ws"
    assert p.embedding_model == "custom-model"
    assert p.config == {"x": 1}
    assert p._vector_collection == "ws_vectors"


# ---------------------------------------------------------------------------
# initialize / shutdown
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_initialize_creates_collections(monkeypatch):
    adapter = make_adapter(get_collection_return=None)

    class FakeDB:
        def __init__(self):
            self.started = False

        async def start(self):
            self.started = True

    adapter._db = FakeDB()

    def fake_ctor(data_dir, config):
        return adapter

    monkeypatch.setattr(embedded_multi, "EmbeddedProtocolAdapter", fake_ctor)

    p = EmbeddedMultiModelProvider(workspace="ws")
    await p.initialize()

    assert p._is_initialized is True
    assert adapter._db.started is True
    # vector + graph created via create_collection (get_collection returned None)
    assert adapter.create_collection.call_count == 2
    adapter.create_document_collection.assert_called_once()
    adapter.create_timeseries_collection.assert_called_once()

    # idempotent: second call returns early
    adapter.create_collection.reset_mock()
    await p.initialize()
    adapter.create_collection.assert_not_called()


@pytest.mark.asyncio
async def test_initialize_existing_collections_no_create(monkeypatch):
    adapter = make_adapter(get_collection_return={"exists": True})
    monkeypatch.setattr(
        embedded_multi, "EmbeddedProtocolAdapter", lambda data_dir, config: adapter
    )
    p = EmbeddedMultiModelProvider()
    await p.initialize()
    # collections already exist -> create_collection not called
    adapter.create_collection.assert_not_called()


@pytest.mark.asyncio
async def test_initialize_collection_exceptions_swallowed(monkeypatch):
    adapter = make_adapter(get_collection_return=None)
    adapter.create_document_collection.side_effect = Exception("dup")
    adapter.create_timeseries_collection.side_effect = Exception("dup")
    monkeypatch.setattr(
        embedded_multi, "EmbeddedProtocolAdapter", lambda data_dir, config: adapter
    )
    p = EmbeddedMultiModelProvider()
    await p.initialize()  # must not raise
    assert p._is_initialized is True


@pytest.mark.asyncio
async def test_initialize_db_without_start(monkeypatch):
    adapter = make_adapter(get_collection_return=None)
    adapter._db = SimpleNamespace()  # no .start attr
    monkeypatch.setattr(
        embedded_multi, "EmbeddedProtocolAdapter", lambda data_dir, config: adapter
    )
    p = EmbeddedMultiModelProvider()
    await p.initialize()
    assert p._is_initialized is True


@pytest.mark.asyncio
async def test_shutdown_with_stop():
    adapter = make_adapter()

    class FakeDB:
        def __init__(self):
            self.stopped = False

        async def stop(self):
            self.stopped = True

    adapter._db = FakeDB()
    p = attach(EmbeddedMultiModelProvider(), adapter)
    await p.shutdown()
    assert adapter._db.stopped is True
    adapter.close.assert_called_once()
    assert p._is_initialized is False


@pytest.mark.asyncio
async def test_shutdown_without_stop():
    adapter = make_adapter()
    adapter._db = SimpleNamespace()  # no stop
    p = attach(EmbeddedMultiModelProvider(), adapter)
    await p.shutdown()
    adapter.close.assert_called_once()
    assert p._is_initialized is False


@pytest.mark.asyncio
async def test_shutdown_no_adapter():
    p = EmbeddedMultiModelProvider()
    p._adapter = None
    await p.shutdown()  # must not raise
    assert p._is_initialized is False


# ---------------------------------------------------------------------------
# _chunk_code
# ---------------------------------------------------------------------------


def test_chunk_code_basic():
    p = EmbeddedMultiModelProvider()
    content = "def a():\n    pass\n\ndef b():\n    pass\n"
    chunks = p._chunk_code(content)
    assert len(chunks) >= 1
    for c in chunks:
        assert "content" in c
        assert c["start_line"] >= 1
        assert c["end_line"] >= 1
        assert c["line_count"] >= 1


def test_chunk_code_no_trailing_remainder():
    p = EmbeddedMultiModelProvider()
    # content ends with empty line -> remainder flushed during loop
    chunks = p._chunk_code("a\nb\n\n")
    assert chunks  # at least one chunk produced


def test_chunk_code_remainder_path():
    p = EmbeddedMultiModelProvider()
    # no empty lines at all -> remainder added after loop
    chunks = p._chunk_code("line1\nline2\nline3")
    assert len(chunks) == 1
    assert chunks[0]["content"] == "line1\nline2\nline3"


# ---------------------------------------------------------------------------
# _extract_code_metrics
# ---------------------------------------------------------------------------


def test_extract_code_metrics():
    p = EmbeddedMultiModelProvider()
    content = (
        "# comment\n"
        "import os\n"
        "def foo():\n"
        "    if x:\n"
        "        pass\n"
        "class Bar:\n"
        "    def baz(self): { }\n"
    )
    metrics = p._extract_code_metrics(content, "python")
    names = {m["name"]: m["value"] for m in metrics}
    assert names["lines_of_code"] >= 1
    assert names["function_count"] == 2
    assert names["class_count"] == 1
    assert "max_nesting_depth" in names
    for m in metrics:
        assert m["language"] == "python"


def test_extract_code_metrics_nesting_braces():
    p = EmbeddedMultiModelProvider()
    content = "{\n  {\n    x\n  }\n}\n"
    metrics = p._extract_code_metrics(content, "js")
    depth = next(m["value"] for m in metrics if m["name"] == "max_nesting_depth")
    assert depth == 2


# ---------------------------------------------------------------------------
# _find_code_files / _detect_language
# ---------------------------------------------------------------------------


def test_find_code_files(tmp_path):
    (tmp_path / "a.py").write_text("x")
    (tmp_path / "b.js").write_text("y")
    (tmp_path / "ignore.txt").write_text("z")
    sub = tmp_path / "sub"
    sub.mkdir()
    (sub / "c.rs").write_text("w")

    p = EmbeddedMultiModelProvider()
    files = p._find_code_files(tmp_path)
    suffixes = sorted(f.suffix for f in files)
    assert suffixes == [".js", ".py", ".rs"]


def test_find_code_files_custom_map(tmp_path):
    (tmp_path / "a.py").write_text("x")
    (tmp_path / "b.foo").write_text("y")
    p = EmbeddedMultiModelProvider()
    files = p._find_code_files(tmp_path, language_map={".foo": "foolang"})
    assert [f.suffix for f in files] == [".foo"]


def test_detect_language():
    from pathlib import Path

    p = EmbeddedMultiModelProvider()
    assert p._detect_language(Path("x.py"), {".py": "python"}) == "python"
    assert p._detect_language(Path("x.unknown")) == "unknown"


# ---------------------------------------------------------------------------
# _index_code_as_graph
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_index_code_as_graph_counts():
    adapter = make_adapter()
    p = attach(EmbeddedMultiModelProvider(), adapter)
    content = (
        "import os\n"
        "from sys import path\n"
        "def foo():\n"
        "    pass\n"
        "async def bar():\n"
        "    pass\n"
        "class Baz(Base):\n"
        "    pass\n"
    )
    info = await p._index_code_as_graph("f.py", content, "python", {"file_hash": "h"})
    assert info["functions"] == 2
    assert info["classes"] == 1
    assert info["imports"] == 2
    assert info["calls"] == 0
    # two function nodes + one class node created
    assert adapter.create_node.call_count == 3


@pytest.mark.asyncio
async def test_index_code_as_graph_node_errors_swallowed():
    adapter = make_adapter()
    adapter.create_node.side_effect = Exception("dup")
    p = attach(EmbeddedMultiModelProvider(), adapter)
    info = await p._index_code_as_graph(
        "f.py", "def foo():\n    pass\nclass C:\n    pass\n", "python", {}
    )
    # file_hash defaulted by hashing content; exceptions swallowed
    assert info["functions"] == 1
    assert info["classes"] == 1


# ---------------------------------------------------------------------------
# _store_metric
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_store_metric_ingests():
    adapter = make_adapter()
    p = attach(EmbeddedMultiModelProvider(), adapter)
    await p._store_metric(
        "f.py", {"name": "loc", "value": 10, "language": "python"}, {}
    )
    adapter.ingest_timeseries.assert_called_once()
    kwargs = adapter.ingest_timeseries.call_args.kwargs
    assert kwargs["collection_name"] == p._timeseries_collection
    point = kwargs["points"][0]
    assert point["values"]["value"] == 10
    assert point["tags"]["metric_name"] == "loc"


@pytest.mark.asyncio
async def test_store_metric_error_swallowed():
    adapter = make_adapter()
    adapter.ingest_timeseries.side_effect = Exception("no ts")
    p = attach(EmbeddedMultiModelProvider(), adapter)
    # missing language key -> uses default "unknown"
    await p._store_metric("f.py", {"name": "loc", "value": 1}, {})  # no raise


# ---------------------------------------------------------------------------
# index_code_file (the multi-model fan-out)
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_index_code_file_all_models():
    adapter = make_adapter()
    p = attach(EmbeddedMultiModelProvider(), adapter)
    content = "def foo():\n    return 1\n\nclass C:\n    pass\n"
    res = await p.index_code_file(
        "main.py", content, language="python", metadata={"author": "me"}
    )
    assert res["vectors"] >= 1
    assert res["document"] is True
    assert res["graph"]["functions"] == 1
    assert res["timeseries"] == 4  # loc, func, class, nesting
    # vectors routed via insert_records
    assert adapter.insert_records.called
    adapter.insert_document.assert_called_once()


@pytest.mark.asyncio
async def test_index_code_file_graph_and_timeseries_errors(monkeypatch):
    adapter = make_adapter()
    p = attach(EmbeddedMultiModelProvider(), adapter)

    async def graph_boom(*a, **k):
        raise RuntimeError("graph build fail")

    def metrics_boom(*a, **k):
        raise RuntimeError("metrics fail")

    monkeypatch.setattr(p, "_index_code_as_graph", graph_boom)
    monkeypatch.setattr(p, "_extract_code_metrics", metrics_boom)
    res = await p.index_code_file("a.py", "def f():\n    pass\n")
    assert "graph_error" in res
    assert res["graph_error"] == "graph build fail"
    assert "timeseries_error" in res
    assert res["timeseries_error"] == "metrics fail"


@pytest.mark.asyncio
async def test_index_code_file_lazy_init(monkeypatch):
    adapter = make_adapter(get_collection_return={"exists": True})
    monkeypatch.setattr(
        embedded_multi, "EmbeddedProtocolAdapter", lambda data_dir, config: adapter
    )
    p = EmbeddedMultiModelProvider()  # not initialized
    res = await p.index_code_file("a.py", "x = 1\n")
    assert p._is_initialized is True
    assert "vectors" in res


@pytest.mark.asyncio
async def test_index_code_file_error_branches():
    adapter = make_adapter()
    adapter.insert_records.side_effect = Exception("v fail")
    adapter.insert_document.side_effect = Exception("d fail")
    adapter.ingest_timeseries.side_effect = Exception("ts fail")
    adapter.create_node.side_effect = Exception("g node fail")
    p = attach(EmbeddedMultiModelProvider(), adapter)
    res = await p.index_code_file("a.py", "def f():\n    pass\n")
    assert "vectors_error" in res
    assert "document_error" in res
    # graph node errors are swallowed inside _index_code_as_graph -> graph succeeds
    assert "graph" in res
    # timeseries errors swallowed in _store_metric -> reports count
    assert res["timeseries"] == 4


# ---------------------------------------------------------------------------
# find_similar_functions
# ---------------------------------------------------------------------------


def _result(metadata, score=0.9):
    return SimpleNamespace(metadata=metadata, score=score)


@pytest.mark.asyncio
async def test_find_similar_functions_filters():
    adapter = make_adapter()
    adapter.search.return_value = [
        _result(
            {
                "language": "python",
                "content": "def parse(): pass",
                "file_path": "a.py",
                "start_line": 1,
                "end_line": 2,
            }
        ),
        _result({"language": "java", "content": "def x(): pass"}),  # wrong lang
        _result({"language": "python", "content": "x = 1"}),  # no def
        _result(None),  # None metadata -> language None != python
    ]
    p = attach(EmbeddedMultiModelProvider(), adapter)
    out = await p.find_similar_functions(code="def y(): ...", language="python", top_k=5)
    assert len(out) == 1
    assert out[0]["file_path"] == "a.py"
    assert out[0]["score"] == 0.9
    assert out[0]["start_line"] == 1


@pytest.mark.asyncio
async def test_find_similar_functions_name_filter():
    adapter = make_adapter()
    adapter.search.return_value = [
        _result({"language": "python", "content": "def parse(): pass"}),
        _result({"language": "python", "content": "def other(): pass"}),
    ]
    p = attach(EmbeddedMultiModelProvider(), adapter)
    out = await p.find_similar_functions(
        code="q", function_name="parse", language="python"
    )
    assert len(out) == 1
    assert "parse" in out[0]["content"]


@pytest.mark.asyncio
async def test_find_similar_functions_lazy_init(monkeypatch):
    adapter = make_adapter(get_collection_return={"exists": True})
    monkeypatch.setattr(
        embedded_multi, "EmbeddedProtocolAdapter", lambda data_dir, config: adapter
    )
    p = EmbeddedMultiModelProvider()
    out = await p.find_similar_functions(code="x")
    assert out == []
    assert p._is_initialized is True


# ---------------------------------------------------------------------------
# trace_function_usage
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_trace_function_usage():
    p = attach(EmbeddedMultiModelProvider(), make_adapter())
    out = await p.trace_function_usage("foo", "f.py", depth=5)
    assert out == {
        "function": "foo",
        "file": "f.py",
        "callers": [],
        "callees": [],
        "depth": 5,
    }


@pytest.mark.asyncio
async def test_trace_function_usage_lazy_init(monkeypatch):
    adapter = make_adapter(get_collection_return={"exists": True})
    monkeypatch.setattr(
        embedded_multi, "EmbeddedProtocolAdapter", lambda data_dir, config: adapter
    )
    p = EmbeddedMultiModelProvider()
    out = await p.trace_function_usage("foo", "f.py")
    assert out["depth"] == 3
    assert p._is_initialized is True


# ---------------------------------------------------------------------------
# index_repository
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_index_repository_missing_path():
    p = attach(EmbeddedMultiModelProvider(), make_adapter())
    with pytest.raises(ValueError):
        await p.index_repository("/nonexistent/path/xyz")


@pytest.mark.asyncio
async def test_index_repository_success(tmp_path):
    (tmp_path / "a.py").write_text("def f():\n    pass\n")
    (tmp_path / "b.py").write_text("def g():\n    pass\n")
    adapter = make_adapter()
    p = attach(EmbeddedMultiModelProvider(), adapter)
    res = await p.index_repository(str(tmp_path))
    assert res["files_processed"] == 2
    assert res["files_failed"] == 0
    assert res["total_functions"] == 2
    assert res["total_chunks"] >= 2


@pytest.mark.asyncio
async def test_index_repository_max_files(tmp_path):
    (tmp_path / "a.py").write_text("x = 1\n")
    (tmp_path / "b.py").write_text("y = 2\n")
    (tmp_path / "c.py").write_text("z = 3\n")
    p = attach(EmbeddedMultiModelProvider(), make_adapter())
    res = await p.index_repository(str(tmp_path), max_files=1)
    assert res["files_processed"] == 1


@pytest.mark.asyncio
async def test_index_repository_file_failure(tmp_path, monkeypatch):
    (tmp_path / "a.py").write_text("x = 1\n")
    p = attach(EmbeddedMultiModelProvider(), make_adapter())

    async def boom(*a, **k):
        raise RuntimeError("index fail")

    monkeypatch.setattr(p, "index_code_file", boom)
    res = await p.index_repository(str(tmp_path))
    assert res["files_processed"] == 0
    assert res["files_failed"] == 1
    assert res["errors"][0]["error"] == "index fail"


@pytest.mark.asyncio
async def test_index_repository_lazy_init(tmp_path, monkeypatch):
    adapter = make_adapter(get_collection_return={"exists": True})
    monkeypatch.setattr(
        embedded_multi, "EmbeddedProtocolAdapter", lambda data_dir, config: adapter
    )
    p = EmbeddedMultiModelProvider()
    res = await p.index_repository(str(tmp_path))  # empty dir
    assert res["files_processed"] == 0
    assert p._is_initialized is True


# ---------------------------------------------------------------------------
# hybrid_search + helpers
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_hybrid_search_vector_and_graph():
    adapter = make_adapter()
    adapter.search.return_value = [
        _result({"content": "vec content", "file_path": "a.py"}, score=0.5),
    ]
    adapter.execute_graph_query.return_value = {
        "results": [
            {
                "score": 0.4,
                "content": "graph content",
                "metadata": {"file_path": "a.py"},
            }
        ]
    }
    p = attach(EmbeddedMultiModelProvider(), adapter)
    out = await p.hybrid_search("query", top_k=10, graph_query="MATCH (n) RETURN n")
    # a.py gets vector(0.5) + graph(0.4*1.2) ; ranked, file_path present
    assert len(out) == 1
    assert out[0]["metadata"]["file_path"] == "a.py"


@pytest.mark.asyncio
async def test_hybrid_search_graph_error_swallowed():
    adapter = make_adapter()
    adapter.search.return_value = [
        _result({"content": "c", "file_path": "a.py"}, score=0.5)
    ]
    adapter.execute_graph_query.side_effect = Exception("graph down")
    p = attach(EmbeddedMultiModelProvider(), adapter)
    out = await p.hybrid_search("q", graph_query="bad")
    assert len(out) == 1


@pytest.mark.asyncio
async def test_hybrid_search_no_graph_query():
    adapter = make_adapter()
    adapter.search.return_value = [
        _result({"content": "c", "file_path": "a.py"}, score=0.5)
    ]
    p = attach(EmbeddedMultiModelProvider(), adapter)
    out = await p.hybrid_search("q")
    adapter.execute_graph_query.assert_not_called()
    assert len(out) == 1


@pytest.mark.asyncio
async def test_hybrid_search_document_filter():
    adapter = make_adapter()
    adapter.search.return_value = [
        _result({"content": "c1", "file_path": "a.py", "language": "python"}, 0.9),
        _result({"content": "c2", "file_path": "b.py", "language": "java"}, 0.8),
    ]
    p = attach(EmbeddedMultiModelProvider(), adapter)
    out = await p.hybrid_search("q", document_filter={"language": "python"})
    assert len(out) == 1
    assert out[0]["metadata"]["file_path"] == "a.py"


@pytest.mark.asyncio
async def test_hybrid_search_lazy_init(monkeypatch):
    adapter = make_adapter(get_collection_return={"exists": True})
    monkeypatch.setattr(
        embedded_multi, "EmbeddedProtocolAdapter", lambda data_dir, config: adapter
    )
    p = EmbeddedMultiModelProvider()
    out = await p.hybrid_search("q")
    assert out == []
    assert p._is_initialized is True


# ---------------------------------------------------------------------------
# _filter_hybrid_results / _matches_filter / _rank_hybrid_results
# ---------------------------------------------------------------------------


def test_filter_hybrid_results_no_filter():
    p = EmbeddedMultiModelProvider()
    results = [{"metadata": {"a": 1}}]
    assert p._filter_hybrid_results(results, None) is results


def test_matches_filter():
    p = EmbeddedMultiModelProvider()
    assert p._matches_filter({"a": 1, "b": 2}, {"a": 1}) is True
    assert p._matches_filter({"a": 1}, {"a": 2}) is False
    assert p._matches_filter({"a": 1}, {"missing": 1}) is False


def test_rank_hybrid_results_scoring():
    p = EmbeddedMultiModelProvider()
    results = [
        {"type": "vector", "score": 0.5, "metadata": {"file_path": "a.py"}},
        {"type": "graph", "score": 0.5, "metadata": {"file_path": "a.py"}},
        {"type": "vector", "score": 0.9, "metadata": {"file_path": "b.py"}},
        {"type": "vector", "score": 0.1, "metadata": {}},  # no file_path -> dropped
    ]
    ranked = p._rank_hybrid_results(results, top_k=10)
    paths = [r["metadata"].get("file_path") for r in ranked]
    # a.py: 0.5 + 0.5*1.2 = 1.1 > b.py 0.9
    assert paths == ["a.py", "b.py"]


def test_rank_hybrid_results_top_k_limit():
    p = EmbeddedMultiModelProvider()
    results = [
        {"type": "vector", "score": float(i), "metadata": {"file_path": f"{i}.py"}}
        for i in range(5)
    ]
    ranked = p._rank_hybrid_results(results, top_k=2)
    assert len(ranked) == 2
    # highest scores first
    assert ranked[0]["metadata"]["file_path"] == "4.py"
