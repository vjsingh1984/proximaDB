"""Offline unit tests for proximadb_sdk.document.

All transport is mocked via a hand fake backend client. No network, no server.
"""

from __future__ import annotations

import asyncio
from datetime import datetime

import pytest

from proximadb_sdk.document import (
    CompressionAlgorithm,
    DocIndexType,
    Document,
    DocumentCollectionConfig,
    DocumentFilter,
    DocumentQueryResponse,
    DocumentQueryResult,
    DocumentRepository,
    IndexDefinition,
    ProximaDBDocument,
    QueryStrategy,
    create_document_api,
)
from proximadb_sdk.exceptions import ProximaDBError


# ---------------------------------------------------------------------------
# Fake backend client
# ---------------------------------------------------------------------------


class FakeClient:
    """Hand fake backend the repository wraps. Records calls; returns dicts."""

    def __init__(self, *, fail=None):
        self.calls = []
        self._fail = fail or set()
        self.stored = {}

    def create_document_collection(self, name, config):
        self.calls.append(("create_document_collection", name, config))
        if "create" in self._fail:
            raise RuntimeError("boom create")
        return {"collection_id": name}

    def insert_document(self, collection_name, document, id):
        self.calls.append(("insert_document", collection_name, document, id))
        if "insert" in self._fail:
            raise RuntimeError("boom insert")
        return {"id": id}

    def get_document(self, collection_name, doc_id, projection=None):
        self.calls.append(("get_document", collection_name, doc_id, projection))
        if "get" in self._fail:
            raise RuntimeError("boom get")
        if doc_id == "missing":
            return None
        return {"id": doc_id, "data": {"k": "v"}}

    def query_documents(self, collection_name, filter, projection=None, limit=100):
        self.calls.append(
            ("query_documents", collection_name, filter, projection, limit)
        )
        if "query" in self._fail:
            raise RuntimeError("boom query")
        return {
            "documents": [
                {"id": "d1", "data": {"language": "python", "loc": 50}},
                {"id": "d2", "data": {"language": "rust", "loc": 100}},
            ],
            "total_count": 2,
            "has_more": False,
        }


@pytest.fixture(autouse=True)
def _clear_shared_state():
    """Repository keeps process-wide shared dicts; reset around each test."""
    DocumentRepository._shared_batch_buffer.clear()
    DocumentRepository._shared_collections.clear()
    DocumentRepository._shared_documents.clear()
    yield
    DocumentRepository._shared_batch_buffer.clear()
    DocumentRepository._shared_collections.clear()
    DocumentRepository._shared_documents.clear()


# ---------------------------------------------------------------------------
# Data models
# ---------------------------------------------------------------------------


def test_index_definition_to_dict_autoname():
    idx = IndexDefinition(path="$.user.email", type=DocIndexType.HASH, unique=True)
    d = idx.to_dict()
    assert d["index_type"] == "hash"
    assert d["unique"] is True
    assert d["path"] == "$.user.email"
    assert d["name"] == "idx__user_email"


def test_index_definition_explicit_name():
    idx = IndexDefinition(name="my_idx", path="$.x")
    assert idx.to_dict()["name"] == "my_idx"


def test_collection_config_to_dict():
    cfg = DocumentCollectionConfig(
        name="c",
        indexes=[IndexDefinition(path="$.a")],
        enable_fulltext=True,
        fulltext_paths=["$.content"],
        ttl_seconds=60,
        compression=CompressionAlgorithm.ZSTD,
    )
    d = cfg.to_dict()
    assert d["name"] == "c"
    assert d["enable_fulltext"] is True
    assert d["compression"] == "zstd"
    assert len(d["indexes"]) == 1


def test_document_to_dict_and_from_dict_roundtrip():
    now = datetime(2024, 1, 1, 12, 0, 0)
    doc = Document(
        id="x",
        content={"a": 1},
        created_at=now,
        updated_at=now,
        metadata={"m": 1},
    )
    d = doc.to_dict()
    assert d["id"] == "x"
    assert d["document"] == {"a": 1}
    assert "created_at" in d and "updated_at" in d and "metadata" in d

    back = Document.from_dict(
        {
            "id": "x",
            "document": {"a": 1},
            "version": 3,
            "created_at": now.isoformat(),
            "updated_at": now.isoformat(),
            "metadata": {"m": 1},
        }
    )
    assert back.id == "x" and back.version == 3
    assert back.created_at == now


def test_document_to_dict_minimal_and_from_dict_no_dates():
    doc = Document(id="y", content={})
    d = doc.to_dict()
    assert "created_at" not in d and "metadata" not in d
    back = Document.from_dict({"id": "y", "document": {}})
    assert back.created_at is None and back.updated_at is None


# ---------------------------------------------------------------------------
# DocumentFilter builder
# ---------------------------------------------------------------------------


def test_filter_all_condition_builders():
    f = (
        DocumentFilter()
        .eq("a", 1)
        .ne("b", 2)
        .gt("c", 3)
        .gte("d", 4)
        .lt("e", 5)
        .lte("f", 6)
        .contains("g", "x")
        .fulltext("h", "y")
        .starts_with("i", "p")
        .ends_with("j", "q")
        .in_list("k", [1, 2])
        .exists("l")
    )
    d = f.to_dict()
    assert d["logic"] == "AND"
    ops = {c["op"] for c in d["conditions"]}
    assert ops == {
        "eq", "ne", "gt", "gte", "lt", "lte",
        "contains", "fulltext", "starts_with", "ends_with", "in", "exists",
    }


def test_filter_logic_switch_and_group():
    inner = DocumentFilter().eq("status", "active")
    f = DocumentFilter().eq("a", 1).or_().group(inner).and_()
    d = f.to_dict()
    assert d["logic"] == "AND"  # last switch wins
    assert len(d["groups"]) == 1


def test_filter_or_and_operators():
    a = DocumentFilter().eq("x", 1)
    b = DocumentFilter().eq("y", 2)
    ored = a | b
    anded = a & b
    assert ored.to_dict()["logic"] == "OR"
    assert anded.to_dict()["logic"] == "AND"
    assert len(ored.to_dict()["groups"]) == 2
    assert len(anded.to_dict()["groups"]) == 2


# ---------------------------------------------------------------------------
# DocumentQueryResponse
# ---------------------------------------------------------------------------


def test_query_response_dict_iter_len_get():
    resp = DocumentQueryResponse(
        documents=[{"id": "a"}, {"id": "b"}], total_count=2, has_more=True
    )
    assert len(resp) == 2
    assert list(resp) == [{"id": "a"}, {"id": "b"}]
    assert resp.get("total_count") == 2
    assert resp.get("missing", "def") == "def"
    assert resp.to_dict()["has_more"] is True


# ---------------------------------------------------------------------------
# DocumentQueryResult (lazy/async)
# ---------------------------------------------------------------------------


def test_query_result_basic_props():
    qr = DocumentQueryResult(documents=[1, 2, 3], total_count=10, has_more=False)
    assert qr.documents == [1, 2, 3]
    assert qr.total_count == 10
    assert qr.has_more is False
    assert len(qr) == 3
    assert list(qr) == [1, 2, 3]


def test_query_result_fetch_no_more():
    qr = DocumentQueryResult(documents=[1], total_count=1, has_more=False)
    assert asyncio.run(qr.fetch_next_batch()) == []


def test_query_result_fetch_next_keeps_has_more_when_full_batch():
    async def fetch():
        return [10, 11]

    qr = DocumentQueryResult(
        documents=[1], total_count=5, has_more=True, fetch_fn=fetch, batch_size=2
    )
    batch = asyncio.run(qr.fetch_next_batch())
    assert batch == [10, 11]
    assert qr.documents == [1, 10, 11]
    # len(batch) == batch_size, so has_more remains True
    assert qr.has_more is True


def test_query_result_fetch_all_stops_on_short_batch():
    async def fetch_short():
        return [99]

    qr = DocumentQueryResult(
        documents=[], total_count=5, has_more=True, fetch_fn=fetch_short, batch_size=5
    )
    out = asyncio.run(qr.fetch_all())
    assert out == [99]
    assert qr.has_more is False


def test_query_result_to_list():
    async def fetch():
        return [7]

    qr = DocumentQueryResult(
        documents=[], total_count=1, has_more=True, fetch_fn=fetch, batch_size=10
    )
    assert asyncio.run(qr.to_list()) == [7]


# ---------------------------------------------------------------------------
# Repository: collection management
# ---------------------------------------------------------------------------


def test_repo_create_get_list_delete_collection():
    client = FakeClient()
    repo = DocumentRepository(client)
    cfg = DocumentCollectionConfig(name="c1", indexes=[IndexDefinition(path="$.a")])
    cid = repo.create_collection(cfg)
    assert cid == "c1"

    info = repo.get_collection("c1")
    assert info["name"] == "c1"
    assert info["document_count"] == 0
    assert "storage_size_bytes" in info

    assert repo.get_collection("nope") is None

    cols = repo.list_collections()
    assert any(c["id"] == "c1" for c in cols)

    assert repo.delete_collection("c1") is True
    assert repo.get_collection("c1") is None


def test_repo_create_collection_failure_wraps_error():
    repo = DocumentRepository(FakeClient(fail={"create"}))
    with pytest.raises(ProximaDBError):
        repo.create_collection(DocumentCollectionConfig(name="x"))


def test_repo_create_collection_uses_returned_id():
    class IdClient(FakeClient):
        def create_document_collection(self, name, config):
            return {"collection_id": "server-id"}

    repo = DocumentRepository(IdClient())
    assert repo.create_collection(DocumentCollectionConfig(name="local")) == "server-id"


# ---------------------------------------------------------------------------
# Repository: CRUD
# ---------------------------------------------------------------------------


def test_repo_insert_and_cache():
    client = FakeClient()
    repo = DocumentRepository(client)
    doc = repo.insert("c", {"a": 1}, id="d1")
    assert doc.id == "d1"
    assert repo._cache["c:d1"] is doc
    assert repo._documents["c"]["d1"] is doc


def test_repo_insert_autogen_id():
    repo = DocumentRepository(FakeClient())
    doc = repo.insert("c", {"a": 1})
    assert doc.id.startswith("doc:")


def test_repo_insert_failure_wraps_error():
    repo = DocumentRepository(FakeClient(fail={"insert"}))
    with pytest.raises(ProximaDBError):
        repo.insert("c", {"a": 1}, id="d1")


def test_repo_insert_batch():
    repo = DocumentRepository(FakeClient())
    docs = repo.insert_batch("c", [{"a": 1}, {"b": 2}], ids=["x", "y"])
    assert [d.id for d in docs] == ["x", "y"]
    assert repo._documents["c"]["x"].content == {"a": 1}


def test_repo_insert_batch_autogen_and_mismatch():
    repo = DocumentRepository(FakeClient())
    docs = repo.insert_batch("c", [{"a": 1}])
    assert docs[0].id.startswith("doc:")
    with pytest.raises(ValueError):
        repo.insert_batch("c", [{"a": 1}], ids=["x", "y"])


def test_repo_get_from_server_and_cache_hit():
    repo = DocumentRepository(FakeClient())
    # Seed the instance cache directly to exercise the cache-hit branch
    # deterministically (no cross-test shared-state coupling).
    cached = Document(id="d1", content={"cached": True})
    repo._update_cache("c:d1", cached)
    assert repo.get("c", "d1") is cached  # cache-hit branch

    # Cache miss -> server path returns server data.
    miss = repo.get("c", "dnew")
    assert miss.content == {"k": "v"}


def test_repo_get_none_when_server_returns_none():
    repo = DocumentRepository(FakeClient())
    assert repo.get("c", "missing") is None


def test_repo_get_no_cache():
    repo = DocumentRepository(FakeClient(), enable_cache=False)
    doc = repo.get("c", "d1", use_cache=False)
    assert doc is not None


def test_repo_get_fallback_to_local_storage():
    repo = DocumentRepository(FakeClient(fail={"get"}))
    repo._ensure_collection("c")
    local = Document(id="d1", content={"local": True})
    repo._documents["c"]["d1"] = local
    got = repo.get("c", "d1", use_cache=False)
    assert got is local


def test_repo_get_raises_when_no_fallback():
    repo = DocumentRepository(FakeClient(fail={"get"}))
    with pytest.raises(ProximaDBError):
        repo.get("c", "d1", use_cache=False)


def test_repo_query_server_path():
    repo = DocumentRepository(FakeClient())
    result = repo.query(
        "c", filter=DocumentFilter().eq("language", "python"), projection=["language"]
    )
    assert result.total_count == 2
    assert {d.id for d in result.documents} == {"d1", "d2"}


def test_repo_query_fallback_local_with_projection_and_offset():
    repo = DocumentRepository(FakeClient(fail={"query"}))
    repo._ensure_collection("c")
    for i in range(5):
        repo._documents["c"][f"d{i}"] = Document(
            id=f"d{i}", content={"language": "python", "loc": i}
        )
    repo._documents["c"]["dr"] = Document(id="dr", content={"language": "rust"})
    result = repo.query(
        "c",
        filter=DocumentFilter().eq("language", "python"),
        projection=["language"],
        limit=2,
        offset=1,
    )
    assert result.total_count == 5  # 5 python docs match
    assert len(result.documents) == 2
    assert result.has_more is True
    assert "language" in result.documents[0].content


def test_repo_search_delegates_to_query():
    repo = DocumentRepository(FakeClient())
    docs = repo.search("c", "hello", limit=5)
    assert isinstance(docs, list)
    assert len(docs) == 2


def test_repo_update_dict_and_oplist():
    repo = DocumentRepository(FakeClient())
    repo._ensure_collection("c")
    repo._documents["c"]["d1"] = Document(id="d1", content={"a": 1, "nested": {}})

    upd = repo.update("c", "d1", {"a": 2})
    assert upd.content["a"] == 2
    assert upd.version == 2

    upd2 = repo.update(
        "c",
        "d1",
        [
            {"operation": "SET", "path": "$.nested.x", "value": 9},
            {"operation": "PUSH", "path": "$.arr", "value": 1},
            {"operation": "PUSH", "path": "$.arr", "value": 2},
            {"operation": "SET", "path": ""},  # skipped (empty path)
        ],
    )
    assert upd2.content["nested"]["x"] == 9
    assert upd2.content["arr"] == [1, 2]


def test_repo_update_push_scalar_into_list():
    repo = DocumentRepository(FakeClient())
    repo._ensure_collection("c")
    repo._documents["c"]["d1"] = Document(id="d1", content={"arr": "single"})
    upd = repo.update("c", "d1", [{"operation": "PUSH", "path": "$.arr", "value": 2}])
    assert upd.content["arr"] == ["single", 2]


def test_repo_update_missing_returns_none():
    repo = DocumentRepository(FakeClient())
    assert repo.update("c", "nope", {"a": 1}) is None


def test_repo_delete_and_delete_by_filter():
    repo = DocumentRepository(FakeClient())
    repo._ensure_collection("c")
    repo._documents["c"]["d1"] = Document(id="d1", content={})
    repo._update_cache("c:d1", repo._documents["c"]["d1"])
    assert repo.delete("c", "d1") is True
    assert repo.delete("c", "d1") is False
    assert repo.delete_by_filter("c", DocumentFilter().eq("a", 1)) == 0


def test_repo_flush_batch():
    repo = DocumentRepository(FakeClient())
    assert repo.flush_batch("empty") == {"success": True, "flushed": 0}
    repo.insert("c", {"a": 1}, id="d1")
    res = repo.flush_batch("c")
    assert res["flushed"] == 1
    assert repo.flush_batch("c")["flushed"] == 0  # buffer now empty


def test_repo_index_stubs():
    repo = DocumentRepository(FakeClient())
    assert repo.create_index("c", IndexDefinition(path="$.a")) is True
    assert repo.drop_index("c", "idx") is True
    assert repo.list_indexes("c") == []


def test_repo_cache_eviction_and_clear_and_stats():
    repo = DocumentRepository(FakeClient(), cache_size=2)
    for i in range(3):
        repo._update_cache(f"c:{i}", Document(id=str(i), content={}))
    assert len(repo._cache) == 2  # capacity 2 -> oldest evicted
    assert "c:0" not in repo._cache

    repo._update_cache("other:z", Document(id="z", content={}))
    repo.clear_cache("c")
    assert all(not k.startswith("c:") for k in repo._cache)

    repo.clear_cache()
    assert repo._cache == {}

    stats = repo.get_cache_stats()
    assert stats["capacity"] == 2
    assert "hit_rate" in stats


def test_repo_get_value_and_normalize_path():
    repo = DocumentRepository(FakeClient())
    assert repo._normalize_path("$.a.b") == "a.b"
    assert repo._normalize_path("$x") == "x"
    assert repo._normalize_path("plain") == "plain"
    doc = {"a": {"b": 5}}
    assert repo._get_value(doc, "$.a.b") == 5
    assert repo._get_value(doc, "$.a.b.c") is None  # descends past scalar


def test_repo_matches_filter_dict_shortcuts():
    repo = DocumentRepository(FakeClient())
    assert repo._matches_filter({"a": 1}, None) is True
    assert repo._matches_filter({"a": 1}, {}) is True
    assert repo._matches_filter({"a": 1}, {"a": 1}) is True  # plain dict equality
    assert repo._matches_filter({"a": 1}, {"a": 2}) is False
    assert repo._matches_filter({"a": 1}, {"conditions": [], "groups": []}) is True


def test_repo_matches_condition_all_ops():
    repo = DocumentRepository(FakeClient())
    doc = {"n": 5, "s": "hello"}

    def m(op, path, value):
        return repo._matches_condition(doc, {"op": op, "path": path, "value": value})

    assert m("eq", "n", 5)
    assert m("ne", "n", 6)
    assert m("gt", "n", 4)
    assert m("gte", "n", 5)
    assert m("lt", "n", 6)
    assert m("lte", "n", 5)
    assert m("contains", "s", "ELL")
    assert m("starts_with", "s", "he")
    assert m("ends_with", "s", "lo")
    assert m("in", "n", [5, 6])
    assert m("exists", "n", True)
    assert m("fulltext", "s", "hell")
    assert not m("in", "missing", [1])
    assert not m("exists", "missing", True)
    assert m("weird", "n", 1)  # unknown op -> True
    assert not m("gt", "missing", 1)  # value is None -> False


def test_repo_matches_filter_or_logic():
    repo = DocumentRepository(FakeClient())
    f = DocumentFilter().eq("a", 1).or_().eq("b", 99)
    assert repo._matches_filter({"a": 1, "b": 2}, f) is True


def test_repo_project_document():
    repo = DocumentRepository(FakeClient())
    doc = {"a": {"b": 1}, "c": 2}
    assert repo._project_document(doc, None) == doc
    proj = repo._project_document(doc, ["$.a.b", "$.missing"])
    assert proj == {"b": 1}


# ---------------------------------------------------------------------------
# High-level ProximaDBDocument
# ---------------------------------------------------------------------------


def test_highlevel_create_collection_by_name():
    docs = ProximaDBDocument(FakeClient())
    cid = docs.create_collection(name="c", enable_fulltext=True)
    assert cid == "c"


def test_highlevel_create_collection_by_config():
    docs = ProximaDBDocument(FakeClient())
    out = docs.create_collection(config=DocumentCollectionConfig(name="c2"))
    assert out == {"success": True, "collection_id": "c2"}


def test_highlevel_create_collection_requires_name():
    docs = ProximaDBDocument(FakeClient())
    with pytest.raises(ValueError):
        docs.create_collection()


def test_highlevel_insert_and_get_and_query():
    docs = ProximaDBDocument(FakeClient())
    created = docs.insert("c", {"a": 1}, id="d1")
    assert created.id == "d1"

    got = docs.get("c", "d1")
    assert got is not None

    resp = docs.query("c", filter=DocumentFilter().eq("a", 1), limit=10)
    assert isinstance(resp, DocumentQueryResponse)
    assert resp.total_count == 2


def test_highlevel_insert_batch_and_search():
    docs = ProximaDBDocument(FakeClient())
    out = docs.insert_batch("c", [{"a": 1}], ids=["x"])
    assert out[0].id == "x"
    found = docs.search("c", "query text", limit=3)
    assert isinstance(found, list)


def test_highlevel_update_and_delete_and_flush():
    docs = ProximaDBDocument(FakeClient())
    docs.insert("c", {"a": 1}, id="d1")
    upd = docs.update("c", "d1", {"a": 2})
    assert upd["success"] is True
    assert upd["new_version"] == 2

    assert docs.update("c", "nope", {"a": 1}) is None
    assert docs.delete("c", "d1") is True
    assert docs.flush("c")["success"] is True


def test_highlevel_insert_document_and_get_document():
    docs = ProximaDBDocument(FakeClient())
    res = docs.insert_document("c", {"a": 1, "b": 2}, id="d1")
    assert res["id"] == "d1"
    assert res["document"] == {"a": 1, "b": 2}

    got = docs.get_document("c", "d1", projection=["$.a"])
    assert got["found"] is True
    assert got["document"] == {"a": 1}

    assert docs.get_document("c", "missing") is None


def test_highlevel_list_and_delete_collection():
    docs = ProximaDBDocument(FakeClient())
    docs.create_collection(name="c")
    cols = docs.list_collections()
    assert any(c["id"] == "c" for c in cols)
    assert docs.delete_collection("c") is True


def test_highlevel_aggregate_match_and_group():
    docs = ProximaDBDocument(FakeClient())
    repo = docs._repository
    repo._ensure_collection("c")
    repo._documents["c"]["d1"] = Document(id="d1", content={"lang": "py", "loc": 10})
    repo._documents["c"]["d2"] = Document(id="d2", content={"lang": "py", "loc": 20})
    repo._documents["c"]["d3"] = Document(id="d3", content={"lang": "rs", "loc": 5})

    # match stage only -> returns documents
    out = docs.aggregate(
        "c", [{"stage": "match", "filter": DocumentFilter().eq("lang", "py")}]
    )
    assert len(out["results"]) == 2

    # group stage with count/avg/sum
    out2 = docs.aggregate(
        "c",
        [
            {
                "stage": "group",
                "key": "$.lang",
                "aggregations": [
                    {"field": "cnt", "type": "count", "path": "$.loc"},
                    {"field": "avg_loc", "type": "avg", "path": "$.loc"},
                    {"field": "sum_loc", "type": "sum", "path": "$.loc"},
                ],
            }
        ],
    )
    rows = {r["key"]: r for r in out2["results"]}
    assert rows["py"]["cnt"] == 2
    assert rows["py"]["avg_loc"] == 15
    assert rows["py"]["sum_loc"] == 30
    assert rows["rs"]["cnt"] == 1


def test_highlevel_aggregate_no_recognized_stage():
    docs = ProximaDBDocument(FakeClient())
    repo = docs._repository
    repo._ensure_collection("c")
    repo._documents["c"]["d1"] = Document(id="d1", content={"a": 1})
    out = docs.aggregate("c", [{"stage": "unknown"}])
    assert len(out["results"]) == 1


def test_highlevel_aggregate_group_empty_values_avg():
    docs = ProximaDBDocument(FakeClient())
    repo = docs._repository
    repo._ensure_collection("c")
    repo._documents["c"]["d1"] = Document(id="d1", content={"lang": "py"})
    out = docs.aggregate(
        "c",
        [
            {
                "stage": "group",
                "key": "$.lang",
                "aggregations": [
                    {"field": "avg_x", "type": "avg", "path": "$.missing"},
                    {"field": "sum_x", "type": "sum", "path": "$.missing"},
                ],
            }
        ],
    )
    row = out["results"][0]
    assert row["avg_x"] == 0
    assert row["sum_x"] == 0


# ---------------------------------------------------------------------------
# Factory + enums
# ---------------------------------------------------------------------------


def test_factory_and_enums():
    api = create_document_api(FakeClient(), enable_cache=False, cache_size=10)
    assert isinstance(api, ProximaDBDocument)
    assert QueryStrategy.AUTO.value == "auto"
    assert DocIndexType.FULLTEXT.value == "fulltext"
    assert CompressionAlgorithm.LZ4.value == "lz4"
