"""Offline unit tests for proximadb_sdk.embedded_multi.

All transports/adapters are mocked. No real DB boot, no network, no model.
"""

from __future__ import annotations

from pathlib import Path
from types import SimpleNamespace

import pytest

from proximadb_sdk import embedded_multi
from proximadb_sdk.embedded_multi import EmbeddedMultiModelProvider

# ---------------------------------------------------------------------------
# Fakes
# ---------------------------------------------------------------------------


class FakeSearchResult:
    def __init__(self, score, metadata):
        self.score = score
        self.metadata = metadata


class FakeAdapter:
    """Hand fake for EmbeddedProtocolAdapter."""

    def __init__(self, *args, **kwargs):
        self._db = SimpleNamespace()
        self.created_collections = []
        self.created_doc_collections = []
        self.created_ts_collections = []
        self.inserted_records = []
        self.inserted_documents = []
        self.created_nodes = []
        self.ingested_points = []
        self.closed = False
        self.search_results = []
        self.graph_results = {"results": []}
        self.existing_collections = set()
        self.raise_on_doc_collection = False
        self.raise_on_ts_collection = False
        self.raise_on_create_node = False
        self.raise_on_ingest = False

    def get_collection(self, name):
        return name if name in self.existing_collections else None

    def create_collection(self, name, config=None):
        self.created_collections.append((name, config))
        self.existing_collections.add(name)

    def create_document_collection(self, name, config=None):
        if self.raise_on_doc_collection:
            raise RuntimeError("exists")
        self.created_doc_collections.append((name, config))

    def create_timeseries_collection(self, name, config=None):
        if self.raise_on_ts_collection:
            raise RuntimeError("exists")
        self.created_ts_collections.append((name, config))

    def insert_records(self, collection_name, records):
        self.inserted_records.append((collection_name, records))

    def insert_document(self, collection_name, document, id):
        self.inserted_documents.append((collection_name, document, id))

    def create_node(self, graph, node_id, labels, properties):
        if self.raise_on_create_node:
            raise RuntimeError("node exists")
        self.created_nodes.append((graph, node_id, labels, properties))

    def ingest_timeseries(self, collection_name, points):
        if self.raise_on_ingest:
            raise RuntimeError("ts down")
        self.ingested_points.append((collection_name, points))

    def search(self, collection_id, query_vector, top_k, include_metadata):
        return self.search_results

    def execute_graph_query(self, graph, query):
        return self.graph_results

    def close(self):
        self.closed = True


def make_provider(adapter=None, initialized=True):
    p = EmbeddedMultiModelProvider(data_dir="~/.proximadb/test", workspace="ws")
    if adapter is None:
        adapter = FakeAdapter()
    p._adapter = adapter
    p._is_initialized = initialized
    return p, adapter


# ---------------------------------------------------------------------------
# Construction
# ---------------------------------------------------------------------------


def test_init_defaults():
    p = EmbeddedMultiModelProvider()
    assert p.embedding_model == "all-MiniLM-L6-v2"
    assert p.workspace == "default_workspace"
    assert p._vector_collection == "default_workspace_vectors"
    assert p._document_collection == "default_workspace_documents"
    assert p._graph_collection == "default_workspace_graph"
    assert p._timeseries_collection == "default_workspace_metrics"
    assert p._adapter is None
    assert p._is_initialized is False
    assert "~" not in p.data_dir


def test_init_custom():
    p = EmbeddedMultiModelProvider(
        data_dir="/tmp/x",
        workspace="proj",
        embedding_model="custom-model",
        config={"k": "v"},
    )
    assert p.data_dir == "/tmp/x"
    assert p.embedding_model == "custom-model"
    assert p.config == {"k": "v"}
    assert p._vector_collection == "proj_vectors"


# ---------------------------------------------------------------------------
# initialize / shutdown
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_initialize_already_done():
    p, adapter = make_provider(initialized=True)
    await p.initialize()
    assert p._adapter is adapter


@pytest.mark.asyncio
async def test_initialize_creates_adapter_and_collections(monkeypatch):
    made = {}
    started = {"val": False}

    class StartableDB:
        async def start(self):
            started["val"] = True

    class FakeAdapterWithDB(FakeAdapter):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, **kwargs)
            self._db = StartableDB()
            made["instance"] = self

    monkeypatch.setattr(embedded_multi, "EmbeddedProtocolAdapter", FakeAdapterWithDB)

    p = EmbeddedMultiModelProvider(workspace="ws")
    await p.initialize()

    assert p._is_initialized is True
    assert started["val"] is True
    adapter = made["instance"]
    names = [n for n, _ in adapter.created_collections]
    assert "ws_vectors" in names
    assert "ws_graph" in names
    assert adapter.created_doc_collections
    assert adapter.created_ts_collections


@pytest.mark.asyncio
async def test_initialize_no_start_method(monkeypatch):
    monkeypatch.setattr(embedded_multi, "EmbeddedProtocolAdapter", FakeAdapter)
    p = EmbeddedMultiModelProvider(workspace="ws")
    await p.initialize()
    assert p._is_initialized is True


@pytest.mark.asyncio
async def test_ensure_collections_skips_existing_and_handles_errors():
    adapter = FakeAdapter()
    adapter.existing_collections = {"ws_vectors", "ws_graph"}
    adapter.raise_on_doc_collection = True
    adapter.raise_on_ts_collection = True
    p, _ = make_provider(adapter=adapter)
    await p._ensure_collections()
    assert adapter.created_collections == []
    assert adapter.created_doc_collections == []
    assert adapter.created_ts_collections == []


@pytest.mark.asyncio
async def test_shutdown_with_stop():
    stopped = {"val": False}

    class StoppableDB:
        async def stop(self):
            stopped["val"] = True

    adapter = FakeAdapter()
    adapter._db = StoppableDB()
    p, _ = make_provider(adapter=adapter)
    await p.shutdown()
    assert stopped["val"] is True
    assert adapter.closed is True
    assert p._is_initialized is False


@pytest.mark.asyncio
async def test_shutdown_without_stop_method():
    adapter = FakeAdapter()
    p, _ = make_provider(adapter=adapter)
    await p.shutdown()
    assert adapter.closed is True
    assert p._is_initialized is False


@pytest.mark.asyncio
async def test_shutdown_no_adapter():
    p = EmbeddedMultiModelProvider()
    p._adapter = None
    await p.shutdown()
    assert p._is_initialized is False


# ---------------------------------------------------------------------------
# _chunk_code
# ---------------------------------------------------------------------------


def test_chunk_code_empty_lines_split():
    p, _ = make_provider()
    content = "line1\nline2\n\nline3\nline4"
    chunks = p._chunk_code(content)
    assert len(chunks) >= 2
    assert all("content" in c and "start_line" in c for c in chunks)
    assert chunks[0]["start_line"] == 1


def test_chunk_code_no_trailing_empty():
    p, _ = make_provider()
    chunks = p._chunk_code("a\nb\nc")
    assert len(chunks) == 1
    assert chunks[0]["line_count"] == 3
    assert chunks[0]["end_line"] == 3


def test_chunk_code_chunk_size_limit():
    p, _ = make_provider()
    content = "\n".join(f"x{i}" for i in range(20))
    chunks = p._chunk_code(content, chunk_size=5)
    assert len(chunks) > 1


# ---------------------------------------------------------------------------
# _extract_code_metrics
# ---------------------------------------------------------------------------


def test_extract_code_metrics():
    p, _ = make_provider()
    content = (
        "# comment\n"
        "import os\n"
        "def foo():\n"
        "    if x:\n"
        "        return 1\n"
        "class Bar:\n"
        "    pass\n"
    )
    metrics = p._extract_code_metrics(content, "python")
    names = {m["name"]: m["value"] for m in metrics}
    assert names["function_count"] == 1
    assert names["class_count"] == 1
    assert names["lines_of_code"] >= 5
    assert "max_nesting_depth" in names
    assert all(m["language"] == "python" for m in metrics)


def test_extract_code_metrics_braces_nesting():
    p, _ = make_provider()
    content = "function f() {\n  if (x) {\n    y;\n  }\n}\n"
    metrics = p._extract_code_metrics(content, "javascript")
    depth = next(m["value"] for m in metrics if m["name"] == "max_nesting_depth")
    assert depth == 2


# ---------------------------------------------------------------------------
# _index_code_as_graph
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_index_code_as_graph_counts():
    adapter = FakeAdapter()
    p, _ = make_provider(adapter=adapter)
    content = (
        "import os\n"
        "from sys import path\n"
        "def foo():\n"
        "    pass\n"
        "async def bar():\n"
        "    pass\n"
        "class Baz(object):\n"
        "    pass\n"
    )
    info = await p._index_code_as_graph("f.py", content, "python", {"file_hash": "h1"})
    assert info["functions"] == 2
    assert info["classes"] == 1
    assert info["imports"] == 2
    assert info["calls"] == 0
    assert len(adapter.created_nodes) == 3


@pytest.mark.asyncio
async def test_index_code_as_graph_node_error_swallowed():
    adapter = FakeAdapter()
    adapter.raise_on_create_node = True
    p, _ = make_provider(adapter=adapter)
    info = await p._index_code_as_graph("f.py", "def foo():\n    pass\n", "python", {})
    assert info["functions"] == 1
    assert adapter.created_nodes == []


@pytest.mark.asyncio
async def test_index_code_as_graph_no_file_hash_computed():
    adapter = FakeAdapter()
    p, _ = make_provider(adapter=adapter)
    info = await p._index_code_as_graph("f.py", "x = 1\n", "python", {})
    assert info["functions"] == 0


# ---------------------------------------------------------------------------
# _store_metric
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_store_metric_success():
    adapter = FakeAdapter()
    p, _ = make_provider(adapter=adapter)
    await p._store_metric(
        "f.py", {"name": "loc", "value": 10, "language": "python"}, {}
    )
    assert len(adapter.ingested_points) == 1
    coll, points = adapter.ingested_points[0]
    assert coll == "ws_metrics"
    assert points[0]["values"]["value"] == 10
    assert points[0]["tags"]["metric_name"] == "loc"


@pytest.mark.asyncio
async def test_store_metric_error_swallowed():
    adapter = FakeAdapter()
    adapter.raise_on_ingest = True
    p, _ = make_provider(adapter=adapter)
    await p._store_metric("f.py", {"name": "loc", "value": 1}, {})
    assert adapter.ingested_points == []


# ---------------------------------------------------------------------------
# index_code_file
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_index_code_file_full():
    adapter = FakeAdapter()
    p, _ = make_provider(adapter=adapter)
    content = "def foo():\n    return 1\n\nclass C:\n    pass\n"
    results = await p.index_code_file(
        "main.py", content, language="python", metadata={"author": "x"}
    )
    assert results["vectors"] >= 1
    assert results["document"] is True
    assert results["graph"]["functions"] == 1
    assert results["graph"]["classes"] == 1
    assert results["timeseries"] == 4
    assert adapter.inserted_documents
    assert adapter.inserted_records


@pytest.mark.asyncio
async def test_index_code_file_initializes_if_needed(monkeypatch):
    monkeypatch.setattr(embedded_multi, "EmbeddedProtocolAdapter", FakeAdapter)
    p = EmbeddedMultiModelProvider(workspace="ws")
    assert p._is_initialized is False
    results = await p.index_code_file("a.py", "x = 1\n")
    assert p._is_initialized is True
    assert "vectors" in results


@pytest.mark.asyncio
async def test_index_code_file_error_branches():
    class BrokenAdapter(FakeAdapter):
        def insert_records(self, *a, **k):
            raise RuntimeError("vec fail")

        def insert_document(self, *a, **k):
            raise RuntimeError("doc fail")

    adapter = BrokenAdapter()
    p, _ = make_provider(adapter=adapter)
    results = await p.index_code_file("m.py", "def f():\n    pass\n")
    assert "vectors_error" in results
    assert "document_error" in results


@pytest.mark.asyncio
async def test_index_code_file_graph_and_timeseries_errors(monkeypatch):
    adapter = FakeAdapter()
    p, _ = make_provider(adapter=adapter)

    async def graph_boom(*a, **k):
        raise RuntimeError("graph fail")

    async def metric_boom(*a, **k):
        raise RuntimeError("metric fail")

    monkeypatch.setattr(p, "_index_code_as_graph", graph_boom)
    monkeypatch.setattr(p, "_store_metric", metric_boom)
    results = await p.index_code_file("m.py", "def f():\n    pass\n")
    assert "graph_error" in results
    assert "timeseries_error" in results


# ---------------------------------------------------------------------------
# find_similar_functions
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_find_similar_functions_filters():
    adapter = FakeAdapter()
    adapter.search_results = [
        FakeSearchResult(
            0.9,
            {
                "language": "python",
                "content": "def foo(): pass",
                "file_path": "a.py",
                "start_line": 1,
                "end_line": 2,
            },
        ),
        FakeSearchResult(
            0.8, {"language": "go", "content": "def x(): pass", "file_path": "b.go"}
        ),
        FakeSearchResult(
            0.7, {"language": "python", "content": "x = 1", "file_path": "c.py"}
        ),
    ]
    p, _ = make_provider(adapter=adapter)
    out = await p.find_similar_functions("def q(): pass", language="python", top_k=5)
    assert len(out) == 1
    assert out[0]["file_path"] == "a.py"
    assert out[0]["score"] == 0.9


@pytest.mark.asyncio
async def test_find_similar_functions_name_filter():
    adapter = FakeAdapter()
    adapter.search_results = [
        FakeSearchResult(0.9, {"language": "python", "content": "def foo(): pass"})
    ]
    p, _ = make_provider(adapter=adapter)
    out = await p.find_similar_functions("x", function_name="bar", language="python")
    assert out == []


@pytest.mark.asyncio
async def test_find_similar_functions_none_metadata():
    adapter = FakeAdapter()
    adapter.search_results = [FakeSearchResult(0.5, None)]
    p, _ = make_provider(adapter=adapter)
    out = await p.find_similar_functions("x", language="python")
    assert out == []


# ---------------------------------------------------------------------------
# trace_function_usage
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_trace_function_usage():
    p, _ = make_provider()
    res = await p.trace_function_usage("foo", "a.py", depth=2)
    assert res["function"] == "foo"
    assert res["file"] == "a.py"
    assert res["depth"] == 2
    assert res["callers"] == []
    assert res["callees"] == []


# ---------------------------------------------------------------------------
# index_repository
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_index_repository_missing_path():
    p, _ = make_provider()
    with pytest.raises(ValueError):
        await p.index_repository("/nonexistent/path/xyz")


@pytest.mark.asyncio
async def test_index_repository_success(tmp_path):
    (tmp_path / "a.py").write_text("def foo():\n    pass\n")
    (tmp_path / "b.py").write_text("def bar():\n    pass\n")
    (tmp_path / "readme.txt").write_text("ignore me")

    adapter = FakeAdapter()
    p, _ = make_provider(adapter=adapter)
    res = await p.index_repository(str(tmp_path))
    assert res["files_processed"] == 2
    assert res["files_failed"] == 0
    assert res["total_functions"] == 2
    assert res["total_chunks"] >= 2


@pytest.mark.asyncio
async def test_index_repository_max_files(tmp_path):
    for i in range(3):
        (tmp_path / f"f{i}.py").write_text("x = 1\n")
    adapter = FakeAdapter()
    p, _ = make_provider(adapter=adapter)
    res = await p.index_repository(str(tmp_path), max_files=1)
    assert res["files_processed"] == 1


@pytest.mark.asyncio
async def test_index_repository_file_failure(tmp_path, monkeypatch):
    (tmp_path / "a.py").write_text("def foo():\n    pass\n")
    p, _ = make_provider()

    async def boom(*a, **k):
        raise RuntimeError("index fail")

    monkeypatch.setattr(p, "index_code_file", boom)
    res = await p.index_repository(str(tmp_path))
    assert res["files_failed"] == 1
    assert res["errors"]
    assert "index fail" in res["errors"][0]["error"]


@pytest.mark.asyncio
async def test_index_repository_initializes(monkeypatch, tmp_path):
    monkeypatch.setattr(embedded_multi, "EmbeddedProtocolAdapter", FakeAdapter)
    (tmp_path / "a.py").write_text("x=1\n")
    p = EmbeddedMultiModelProvider(workspace="ws")
    res = await p.index_repository(str(tmp_path))
    assert p._is_initialized is True
    assert res["files_processed"] == 1


# ---------------------------------------------------------------------------
# _find_code_files / _detect_language
# ---------------------------------------------------------------------------


def test_find_code_files(tmp_path):
    (tmp_path / "a.py").write_text("x")
    (tmp_path / "b.rs").write_text("x")
    (tmp_path / "c.txt").write_text("x")
    sub = tmp_path / "sub"
    sub.mkdir()
    (sub / "d.go").write_text("x")
    p, _ = make_provider()
    files = p._find_code_files(tmp_path)
    suffixes = sorted(f.suffix for f in files)
    assert suffixes == [".go", ".py", ".rs"]


def test_find_code_files_custom_map(tmp_path):
    (tmp_path / "a.foo").write_text("x")
    (tmp_path / "b.py").write_text("x")
    p, _ = make_provider()
    files = p._find_code_files(tmp_path, language_map={".foo": "foolang"})
    assert [f.suffix for f in files] == [".foo"]


def test_detect_language():
    p, _ = make_provider()
    assert p._detect_language(Path("a.py"), {".py": "python"}) == "python"
    assert p._detect_language(Path("a.xyz"), {".py": "python"}) == "unknown"
    assert p._detect_language(Path("a.py")) == "unknown"


# ---------------------------------------------------------------------------
# hybrid_search
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_hybrid_search_vector_only():
    adapter = FakeAdapter()
    adapter.search_results = [
        FakeSearchResult(0.9, {"content": "abc", "file_path": "a.py"}),
        FakeSearchResult(0.5, {"content": "def", "file_path": "b.py"}),
    ]
    p, _ = make_provider(adapter=adapter)
    res = await p.hybrid_search("query", top_k=5)
    assert len(res) == 2
    assert res[0]["metadata"]["file_path"] == "a.py"


@pytest.mark.asyncio
async def test_hybrid_search_vector_none_metadata():
    # When a vector result has metadata=None, the ranking step dereferences
    # the None metadata -> source raises AttributeError (documented behavior).
    adapter = FakeAdapter()
    adapter.search_results = [FakeSearchResult(0.9, None)]
    p, _ = make_provider(adapter=adapter)
    with pytest.raises(AttributeError):
        await p.hybrid_search("q")


@pytest.mark.asyncio
async def test_hybrid_search_with_graph():
    adapter = FakeAdapter()
    adapter.search_results = [
        FakeSearchResult(0.5, {"content": "vec", "file_path": "a.py"})
    ]
    adapter.graph_results = {
        "results": [{"score": 0.4, "content": "g", "metadata": {"file_path": "a.py"}}]
    }
    p, _ = make_provider(adapter=adapter)
    res = await p.hybrid_search("q", graph_query="MATCH (n) RETURN n")
    assert len(res) == 1


@pytest.mark.asyncio
async def test_hybrid_search_graph_error_swallowed():
    adapter = FakeAdapter()
    adapter.search_results = [
        FakeSearchResult(0.5, {"content": "vec", "file_path": "a.py"})
    ]

    def boom(graph, query):
        raise RuntimeError("graph down")

    adapter.execute_graph_query = boom
    p, _ = make_provider(adapter=adapter)
    res = await p.hybrid_search("q", graph_query="MATCH")
    assert len(res) == 1


@pytest.mark.asyncio
async def test_hybrid_search_document_filter():
    adapter = FakeAdapter()
    adapter.search_results = [
        FakeSearchResult(0.9, {"content": "a", "file_path": "a.py", "lang": "py"}),
        FakeSearchResult(0.8, {"content": "b", "file_path": "b.py", "lang": "go"}),
    ]
    p, _ = make_provider(adapter=adapter)
    res = await p.hybrid_search("q", document_filter={"lang": "py"})
    assert len(res) == 1
    assert res[0]["metadata"]["file_path"] == "a.py"


@pytest.mark.asyncio
async def test_hybrid_search_initializes(monkeypatch):
    monkeypatch.setattr(embedded_multi, "EmbeddedProtocolAdapter", FakeAdapter)
    p = EmbeddedMultiModelProvider(workspace="ws")
    res = await p.hybrid_search("q")
    assert p._is_initialized is True
    assert res == []


# ---------------------------------------------------------------------------
# filter / rank helpers
# ---------------------------------------------------------------------------


def test_filter_hybrid_results_no_filter():
    p, _ = make_provider()
    data = [{"metadata": {"x": 1}}]
    assert p._filter_hybrid_results(data, None) is data


def test_matches_filter():
    p, _ = make_provider()
    assert p._matches_filter({"a": 1, "b": 2}, {"a": 1}) is True
    assert p._matches_filter({"a": 1}, {"a": 2}) is False
    assert p._matches_filter({"a": 1}, {"missing": 1}) is False


def test_rank_hybrid_results_graph_boost():
    p, _ = make_provider()
    results = [
        {"type": "vector", "score": 0.5, "metadata": {"file_path": "a.py"}},
        {"type": "graph", "score": 0.5, "metadata": {"file_path": "b.py"}},
    ]
    ranked = p._rank_hybrid_results(results, top_k=5)
    assert ranked[0]["metadata"]["file_path"] == "b.py"


def test_rank_hybrid_results_skips_no_file_path():
    p, _ = make_provider()
    results = [{"type": "vector", "score": 1.0, "metadata": {}}]
    assert p._rank_hybrid_results(results, top_k=5) == []


def test_rank_hybrid_results_best_result_selection():
    p, _ = make_provider()
    results = [
        {"type": "vector", "score": 0.2, "metadata": {"file_path": "a.py"}, "id": 1},
        {"type": "vector", "score": 0.9, "metadata": {"file_path": "a.py"}, "id": 2},
    ]
    ranked = p._rank_hybrid_results(results, top_k=5)
    assert len(ranked) == 1
    assert ranked[0]["id"] == 2
