"""Offline unit tests for proximadb_sdk.document.

Fully offline: a MagicMock backend client is injected. No network, no server,
no model downloads. The shared class-level state on DocumentRepository is reset
before every test to keep tests isolated.
"""

from __future__ import annotations

from datetime import datetime
from unittest.mock import MagicMock

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


@pytest.fixture(autouse=True)
def _reset_shared_state():
    """The repository uses class-level shared dicts; reset for isolation."""
    DocumentRepository._shared_batch_buffer.clear()
    DocumentRepository._shared_collections.clear()
    DocumentRepository._shared_documents.clear()
    yield
    DocumentRepository._shared_batch_buffer.clear()
    DocumentRepository._shared_collections.clear()
    DocumentRepository._shared_documents.clear()


def make_client(**overrides):
    """Build a MagicMock backend client with sensible defaults."""
    client = MagicMock()
    client.create_document_collection.return_value = {"collection_id": "col1"}
    client.insert_document.return_value = {"id": "doc1"}
    client.get_document.return_value = {"id": "doc1", "data": {"a": 1}}
    client.query_documents.return_value = {
        "documents": [{"id": "doc1", "data": {"a": 1}}],
        "total_count": 1,
        "has_more": False,
    }
    for k, v in overrides.items():
        getattr(client, k).return_value = v
    return client


# =============================================================================
# Enums
# =============================================================================


def test_enums_values():
    assert DocIndexType.BTREE.value == "btree"
    assert DocIndexType.HASH.value == "hash"
    assert DocIndexType.INVERTED.value == "inverted"
    assert DocIndexType.FULLTEXT.value == "fulltext"
    assert DocIndexType.GEO.value == "geo"
    assert CompressionAlgorithm.NONE.value == "none"
    assert CompressionAlgorithm.ZSTD.value == "zstd"
    assert QueryStrategy.AUTO.value == "auto"
    assert QueryStrategy.INDEX_ONLY.value == "index_only"
    assert QueryStrategy.FULL_SCAN.value == "full_scan"
    assert QueryStrategy.CACHED.value == "cached"


# =============================================================================
# IndexDefinition
# =============================================================================


def test_index_definition_to_dict_autoname():
    idx = IndexDefinition(path="$.user.email", type=DocIndexType.HASH, unique=True)
    d = idx.to_dict()
    assert d["path"] == "$.user.email"
    assert d["index_type"] == "hash"
    assert d["unique"] is True
    assert d["sparse"] is False
    assert d["name"] == "idx__user_email"


def test_index_definition_explicit_name():
    idx = IndexDefinition(name="myidx", path="$.x")
    assert idx.to_dict()["name"] == "myidx"
    assert idx.to_dict()["index_type"] == "btree"


# =============================================================================
# DocumentCollectionConfig
# =============================================================================


def test_collection_config_to_dict():
    cfg = DocumentCollectionConfig(
        name="c",
        json_schema='{"type":"object"}',
        indexes=[IndexDefinition(path="$.a")],
        enable_fulltext=True,
        fulltext_paths=["$.content"],
        ttl_seconds=60,
        compression=CompressionAlgorithm.ZSTD,
    )
    d = cfg.to_dict()
    assert d["name"] == "c"
    assert d["enable_fulltext"] is True
    assert d["fulltext_paths"] == ["$.content"]
    assert d["ttl_seconds"] == 60
    assert d["compression"] == "zstd"
    assert len(d["indexes"]) == 1


# =============================================================================
# Document model
# =============================================================================


def test_document_to_dict_minimal():
    doc = Document(id="d", content={"x": 1})
    d = doc.to_dict()
    assert d == {"id": "d", "document": {"x": 1}}


def test_document_to_dict_full():
    now = datetime(2026, 1, 1, 12, 0, 0)
    doc = Document(
        id="d",
        content={"x": 1},
        created_at=now,
        updated_at=now,
        metadata={"m": True},
    )
    d = doc.to_dict()
    assert d["created_at"] == now.isoformat()
    assert d["updated_at"] == now.isoformat()
    assert d["metadata"] == {"m": True}


def test_document_from_dict_full_and_minimal():
    now = datetime(2026, 1, 1, 12, 0, 0)
    data = {
        "id": "d",
        "document": {"x": 1},
        "version": 5,
        "created_at": now.isoformat(),
        "updated_at": now.isoformat(),
        "metadata": {"m": 1},
    }
    doc = Document.from_dict(data)
    assert doc.id == "d"
    assert doc.version == 5
    assert doc.created_at == now
    assert doc.metadata == {"m": 1}

    minimal = Document.from_dict({"id": "e", "document": {}})
    assert minimal.version == 1
    assert minimal.created_at is None
    assert minimal.updated_at is None
    assert minimal.metadata is None


# =============================================================================
# DocumentFilter builder
# =============================================================================


def test_filter_all_conditions():
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
    ops = [c["op"] for c in d["conditions"]]
    assert ops == [
        "eq", "ne", "gt", "gte", "lt", "lte", "contains",
        "fulltext", "starts_with", "ends_with", "in", "exists",
    ]
    assert d["logic"] == "AND"
    assert d["groups"] == []


def test_filter_and_or_logic_switch():
    f = DocumentFilter().eq("a", 1).or_()
    assert f.to_dict()["logic"] == "OR"
    f.and_()
    assert f.to_dict()["logic"] == "AND"


def test_filter_group():
    inner = DocumentFilter().eq("status", "active")
    f = DocumentFilter().eq("a", 1).group(inner)
    d = f.to_dict()
    assert len(d["groups"]) == 1
    assert d["groups"][0]["conditions"][0]["path"] == "status"


def test_filter_or_operator():
    f = DocumentFilter().eq("a", 1) | DocumentFilter().eq("b", 2)
    d = f.to_dict()
    assert d["logic"] == "OR"
    assert len(d["groups"]) == 2


def test_filter_and_operator():
    f = DocumentFilter().eq("a", 1) & DocumentFilter().eq("b", 2)
    d = f.to_dict()
    assert d["logic"] == "AND"
    assert len(d["groups"]) == 2


# =============================================================================
# DocumentQueryResult (lazy loading)
# =============================================================================


def test_query_result_basic():
    docs = [Document(id="a", content={}), Document(id="b", content={})]
    r = DocumentQueryResult(documents=docs, total_count=2)
    assert r.documents == docs
    assert r.total_count == 2
    assert r.has_more is False
    assert len(r) == 2
    assert [d.id for d in r] == ["a", "b"]


@pytest.mark.asyncio
async def test_query_result_fetch_next_no_more():
    r = DocumentQueryResult(documents=[], total_count=0, has_more=False)
    assert await r.fetch_next_batch() == []
    assert await r.fetch_all() == []
    assert await r.to_list() == []


@pytest.mark.asyncio
async def test_query_result_fetch_with_fn():
    calls = {"n": 0}

    async def fetch():
        calls["n"] += 1
        # First batch full (batch_size=2), second short -> stops.
        if calls["n"] == 1:
            return [Document(id="x", content={}), Document(id="y", content={})]
        return [Document(id="z", content={})]

    r = DocumentQueryResult(
        documents=[],
        total_count=3,
        has_more=True,
        fetch_fn=fetch,
        batch_size=2,
    )
    batch1 = await r.fetch_next_batch()
    assert len(batch1) == 2
    assert r.has_more is True
    all_docs = await r.fetch_all()
    assert len(all_docs) == 3
    assert r.has_more is False


# =============================================================================
# DocumentQueryResponse
# =============================================================================


def test_query_response():
    resp = DocumentQueryResponse(
        documents=[{"id": "a"}, {"id": "b"}], total_count=2, has_more=True
    )
    assert len(resp) == 2
    assert list(resp) == [{"id": "a"}, {"id": "b"}]
    assert resp.to_dict()["total_count"] == 2
    assert resp.get("has_more") is True
    assert resp.get("missing", "def") == "def"


# =============================================================================
# DocumentRepository helpers
# =============================================================================


def make_repo(client=None, **kw):
    return DocumentRepository(client=client or make_client(), **kw)


def test_normalize_path():
    assert DocumentRepository._normalize_path("$.a.b") == "a.b"
    assert DocumentRepository._normalize_path("$a") == "a"
    assert DocumentRepository._normalize_path("plain") == "plain"


def test_get_value_nested_and_missing():
    repo = make_repo()
    doc = {"user": {"email": "x@y.com"}, "n": 5}
    assert repo._get_value(doc, "$.user.email") == "x@y.com"
    assert repo._get_value(doc, "$.n") == 5
    assert repo._get_value(doc, "$.user.missing") is None
    # traversing into a non-dict returns None
    assert repo._get_value(doc, "$.n.deep") is None


def test_matches_condition_all_ops():
    repo = make_repo()
    doc = {"a": 5, "s": "hello world", "lst_field": "b"}
    assert repo._matches_condition(doc, {"path": "a", "op": "eq", "value": 5})
    assert repo._matches_condition(doc, {"path": "a", "op": "ne", "value": 6})
    assert repo._matches_condition(doc, {"path": "a", "op": "gt", "value": 4})
    assert repo._matches_condition(doc, {"path": "a", "op": "gte", "value": 5})
    assert repo._matches_condition(doc, {"path": "a", "op": "lt", "value": 6})
    assert repo._matches_condition(doc, {"path": "a", "op": "lte", "value": 5})
    assert repo._matches_condition(doc, {"path": "s", "op": "contains", "value": "WORLD"})
    assert repo._matches_condition(doc, {"path": "s", "op": "starts_with", "value": "hello"})
    assert repo._matches_condition(doc, {"path": "s", "op": "ends_with", "value": "world"})
    assert repo._matches_condition(doc, {"path": "lst_field", "op": "in", "value": ["a", "b"]})
    assert repo._matches_condition(doc, {"path": "a", "op": "exists", "value": True})
    assert repo._matches_condition(doc, {"path": "s", "op": "fulltext", "value": "hello"})
    # unknown op -> True
    assert repo._matches_condition(doc, {"path": "a", "op": "weird", "value": 1})
    # None value short-circuits for numeric/string ops
    assert not repo._matches_condition(doc, {"path": "missing", "op": "gt", "value": 1})
    assert not repo._matches_condition(doc, {"path": "missing", "op": "exists", "value": True})
    # in with empty list
    assert not repo._matches_condition(doc, {"path": "a", "op": "in", "value": None})


def test_matches_filter_variants():
    repo = make_repo()
    doc = {"lang": "python", "loc": 200}
    # None filter
    assert repo._matches_filter(doc, None)
    # empty dict
    assert repo._matches_filter(doc, {})
    # plain dict (no conditions/groups keys) - simple equality match
    assert repo._matches_filter(doc, {"lang": "python"})
    assert not repo._matches_filter(doc, {"lang": "rust"})
    # DocumentFilter AND
    f_and = DocumentFilter().eq("lang", "python").gte("loc", 100)
    assert repo._matches_filter(doc, f_and)
    # DocumentFilter OR with one false
    f_or = DocumentFilter().or_().eq("lang", "rust").gte("loc", 100)
    assert repo._matches_filter(doc, f_or)
    # OR with all false
    f_or_false = DocumentFilter().or_().eq("lang", "rust").gte("loc", 9999)
    assert not repo._matches_filter(doc, f_or_false)
    # filter dict with empty conditions/groups -> True
    assert repo._matches_filter(doc, {"conditions": [], "groups": [], "logic": "AND"})


def test_project_document():
    repo = make_repo()
    doc = {"user": {"email": "x@y.com"}, "name": "bob", "age": 3}
    # no projection -> copy
    assert repo._project_document(doc, None) == doc
    # specific fields, nested last segment used as key
    proj = repo._project_document(doc, ["$.user.email", "$.name", "$.missing"])
    assert proj == {"email": "x@y.com", "name": "bob"}


def test_apply_updates_dict():
    repo = make_repo()
    out = repo._apply_updates({"a": 1}, {"a": 2, "b": 3})
    assert out == {"a": 2, "b": 3}


def test_apply_updates_oplist_set_and_push():
    repo = make_repo()
    doc = {"meta": {"count": 1}, "tags": ["x"]}
    updates = [
        {"operation": "SET", "path": "$.meta.count", "value": 9},
        {"operation": "SET", "path": "$.new.deep", "value": "v"},
        {"operation": "PUSH", "path": "$.tags", "value": "y"},
        {"operation": "PUSH", "path": "$.scalar", "value": "z"},
        {"operation": "SET", "path": "", "value": "skip"},  # empty path skipped
    ]
    out = repo._apply_updates(doc, updates)
    assert out["meta"]["count"] == 9
    assert out["new"]["deep"] == "v"
    assert out["tags"] == ["x", "y"]
    assert out["scalar"] == ["z"]


def test_apply_updates_push_to_scalar_existing():
    repo = make_repo()
    # existing leaf is a scalar, PUSH should wrap it into a list
    out = repo._apply_updates({"k": "single"}, [
        {"operation": "PUSH", "path": "$.k", "value": "more"}
    ])
    assert out["k"] == ["single", "more"]


# =============================================================================
# Collection management
# =============================================================================


def test_create_collection_success():
    client = make_client()
    repo = make_repo(client)
    cfg = DocumentCollectionConfig(name="c", indexes=[IndexDefinition(path="$.a")])
    cid = repo.create_collection(cfg)
    assert cid == "col1"
    assert cid in repo._collections
    client.create_document_collection.assert_called_once()


def test_create_collection_default_id_from_name():
    client = make_client(create_document_collection={})  # no collection_id key
    repo = make_repo(client)
    cfg = DocumentCollectionConfig(name="cname")
    cid = repo.create_collection(cfg)
    assert cid == "cname"


def test_create_collection_error_wraps():
    client = make_client()
    client.create_document_collection.side_effect = RuntimeError("boom")
    repo = make_repo(client)
    with pytest.raises(ProximaDBError) as ei:
        repo.create_collection(DocumentCollectionConfig(name="c"))
    assert "Failed to create document collection" in str(ei.value)


def test_get_list_delete_collection():
    client = make_client()
    repo = make_repo(client)
    cfg = DocumentCollectionConfig(name="c", indexes=[IndexDefinition(path="$.a")])
    cid = repo.create_collection(cfg)
    info = repo.get_collection(cid)
    assert info["name"] == "c"
    assert info["document_count"] == 0
    assert info["id"] == cid
    # unknown collection
    assert repo.get_collection("nope") is None
    # list
    listing = repo.list_collections()
    assert any(c["id"] == cid for c in listing)
    # delete
    assert repo.delete_collection(cid) is True
    assert repo.get_collection(cid) is None


def test_delete_collection_clears_cache():
    client = make_client()
    repo = make_repo(client)
    repo.create_collection(DocumentCollectionConfig(name="c"))
    repo.insert("col1", {"x": 1}, id="d1")
    assert any(k.startswith("col1:") for k in repo._cache)
    repo.delete_collection("col1")
    assert not any(k.startswith("col1:") for k in repo._cache)


# =============================================================================
# CRUD
# =============================================================================


def test_insert_success_and_cache():
    client = make_client()
    repo = make_repo(client)
    doc = repo.insert("col1", {"a": 1}, id="d1")
    assert doc.id == "doc1"  # server returns id "doc1"
    assert repo._documents["col1"]["doc1"] is doc
    assert "col1:doc1" in repo._cache


def test_insert_no_cache():
    client = make_client()
    repo = make_repo(client, enable_cache=False)
    doc = repo.insert("col1", {"a": 1}, id="d1")
    assert doc.id == "doc1"
    assert repo._cache == {}


def test_insert_error_wraps():
    client = make_client()
    client.insert_document.side_effect = ValueError("nope")
    repo = make_repo(client)
    with pytest.raises(ProximaDBError) as ei:
        repo.insert("col1", {"a": 1})
    assert "Failed to insert document" in str(ei.value)


def test_insert_batch():
    client = make_client()
    repo = make_repo(client)
    docs = repo.insert_batch("col1", [{"a": 1}, {"b": 2}], ids=["i1", "i2"])
    assert [d.id for d in docs] == ["i1", "i2"]
    assert "col1:i1" in repo._cache


def test_insert_batch_autoids():
    repo = make_repo()
    docs = repo.insert_batch("col1", [{"a": 1}])
    assert docs[0].id.startswith("doc:")


def test_insert_batch_id_mismatch():
    repo = make_repo()
    with pytest.raises(ValueError):
        repo.insert_batch("col1", [{"a": 1}], ids=["x", "y"])


def test_get_from_cache():
    client = make_client()
    repo = make_repo(client)
    inserted = repo.insert("col1", {"a": 1}, id="d1")  # caches under col1:doc1
    client.get_document.reset_mock()
    fetched = repo.get("col1", "doc1")
    assert fetched is inserted
    client.get_document.assert_not_called()


def test_get_from_server():
    client = make_client()
    repo = make_repo(client)
    doc = repo.get("col1", "doc1", use_cache=False)
    assert doc.id == "doc1"
    assert doc.content == {"a": 1}


def test_get_server_returns_none():
    client = make_client(get_document=None)
    repo = make_repo(client)
    assert repo.get("col1", "missing", use_cache=False) is None


def test_get_error_falls_back_to_local():
    client = make_client()
    repo = make_repo(client)
    # seed local storage
    repo.insert("col1", {"a": 1}, id="d1")  # stored under doc1
    client.get_document.side_effect = RuntimeError("down")
    fetched = repo.get("col1", "doc1", use_cache=False)
    assert fetched is not None
    assert fetched.id == "doc1"


def test_get_error_no_local_raises():
    client = make_client()
    client.get_document.side_effect = RuntimeError("down")
    repo = make_repo(client)
    with pytest.raises(ProximaDBError) as ei:
        repo.get("col1", "ghost", use_cache=False)
    assert "Failed to get document" in str(ei.value)


def test_query_from_server():
    client = make_client()
    repo = make_repo(client)
    f = DocumentFilter().eq("a", 1)
    res = repo.query("col1", filter=f, projection=["a"], limit=5)
    assert res.total_count == 1
    assert res.documents[0].id == "doc1"
    assert res.has_more is False


def test_query_no_filter():
    client = make_client()
    repo = make_repo(client)
    res = repo.query("col1")
    assert res.total_count == 1


def test_query_fallback_local():
    client = make_client()
    repo = make_repo(client)
    # populate local docs directly
    repo.insert_batch(
        "col1",
        [{"lang": "python", "n": 10}, {"lang": "rust", "n": 20}],
        ids=["a", "b"],
    )
    client.query_documents.side_effect = RuntimeError("server down")
    res = repo.query(
        "col1",
        filter=DocumentFilter().eq("lang", "python"),
        projection=["lang"],
        limit=10,
        offset=0,
    )
    assert res.total_count == 1
    assert res.documents[0].content == {"lang": "python"}


def test_query_fallback_pagination_has_more():
    client = make_client()
    repo = make_repo(client)
    repo.insert_batch(
        "col1",
        [{"n": i} for i in range(5)],
        ids=[f"k{i}" for i in range(5)],
    )
    client.query_documents.side_effect = RuntimeError("down")
    res = repo.query("col1", limit=2, offset=0)
    assert res.total_count == 5
    assert len(res.documents) == 2
    assert res.has_more is True


def test_search_delegates_to_query():
    client = make_client()
    repo = make_repo(client)
    docs = repo.search("col1", "hello", limit=3)
    assert isinstance(docs, list)
    # the filter passed should be a fulltext filter
    call = client.query_documents.call_args
    assert call.kwargs["filter"]["conditions"][0]["op"] == "fulltext"


def test_update_existing_and_missing():
    client = make_client()
    repo = make_repo(client)
    repo.insert("col1", {"a": 1}, id="d1")  # stored under doc1
    updated = repo.update("col1", "doc1", {"a": 99})
    assert updated.content["a"] == 99
    assert updated.version == 2
    assert "col1:doc1" in repo._cache
    # missing doc
    assert repo.update("col1", "ghost", {"a": 1}) is None


def test_delete_existing_and_missing():
    client = make_client()
    repo = make_repo(client)
    repo.insert("col1", {"a": 1}, id="d1")  # doc1
    assert repo.delete("col1", "doc1") is True
    assert repo.delete("col1", "doc1") is False


def test_delete_by_filter_clears_cache():
    client = make_client()
    repo = make_repo(client)
    repo.insert("col1", {"a": 1}, id="d1")
    count = repo.delete_by_filter("col1", DocumentFilter().eq("a", 1))
    assert count == 0
    assert not any(k.startswith("col1:") for k in repo._cache)


# =============================================================================
# Batch + index + cache management
# =============================================================================


def test_flush_batch_empty_and_populated():
    client = make_client()
    repo = make_repo(client)
    # collection with no buffer entry
    assert repo.flush_batch("none") == {"success": True, "flushed": 0}
    repo.insert("col1", {"a": 1}, id="d1")
    out = repo.flush_batch("col1")
    assert out["success"] is True
    assert out["flushed"] >= 1
    # now buffer empty
    assert repo.flush_batch("col1") == {"success": True, "flushed": 0}


def test_index_stub_methods():
    repo = make_repo()
    assert repo.create_index("col1", IndexDefinition(path="$.a")) is True
    assert repo.drop_index("col1", "idx") is True
    assert repo.list_indexes("col1") == []


def test_cache_lru_eviction():
    client = make_client()
    repo = make_repo(client, cache_size=2)
    repo._update_cache("k1", Document(id="1", content={}))
    repo._update_cache("k2", Document(id="2", content={}))
    repo._update_cache("k3", Document(id="3", content={}))
    assert "k1" not in repo._cache  # evicted
    assert "k3" in repo._cache
    assert len(repo._cache) == 2


def test_clear_cache_specific_and_all():
    repo = make_repo()
    repo._update_cache("col1:a", Document(id="a", content={}))
    repo._update_cache("col2:b", Document(id="b", content={}))
    repo.clear_cache("col1")
    assert "col1:a" not in repo._cache
    assert "col2:b" in repo._cache
    repo.clear_cache()
    assert repo._cache == {}
    assert repo._cache_keys == []


def test_get_cache_stats():
    repo = make_repo(cache_size=50)
    stats = repo.get_cache_stats()
    assert stats["capacity"] == 50
    assert stats["hit_rate"] == 0.0
    assert stats["size"] == 0


# =============================================================================
# ProximaDBDocument high-level API
# =============================================================================


def test_highlevel_create_collection_name():
    client = make_client()
    api = ProximaDBDocument(client)
    cid = api.create_collection(name="c", indexes=[IndexDefinition(path="$.a")])
    assert cid == "col1"


def test_highlevel_create_collection_requires_name():
    api = ProximaDBDocument(make_client())
    with pytest.raises(ValueError):
        api.create_collection()


def test_highlevel_create_collection_with_config():
    client = make_client()
    api = ProximaDBDocument(client)
    cfg = DocumentCollectionConfig(name="c")
    out = api.create_collection(config=cfg)
    assert out == {"success": True, "collection_id": "col1"}


def test_highlevel_insert_get_query_search():
    client = make_client()
    api = ProximaDBDocument(client)
    doc = api.insert("col1", {"a": 1}, id="d1")
    assert isinstance(doc, Document)
    got = api.get("col1", "doc1")
    assert got is not None
    resp = api.query("col1", filter=DocumentFilter().eq("a", 1), projection=["a"])
    assert isinstance(resp, DocumentQueryResponse)
    assert resp.total_count == 1
    results = api.search("col1", "text", limit=5)
    assert isinstance(results, list)


def test_highlevel_insert_batch():
    api = ProximaDBDocument(make_client())
    docs = api.insert_batch("col1", [{"a": 1}], ids=["x"])
    assert docs[0].id == "x"


def test_highlevel_update_existing_and_missing():
    client = make_client()
    api = ProximaDBDocument(client)
    api.insert("col1", {"a": 1}, id="d1")  # doc1
    out = api.update("col1", "doc1", {"a": 2})
    assert out["success"] is True
    assert out["new_version"] == 2
    assert out["document"]["a"] == 2
    assert api.update("col1", "ghost", {"a": 1}) is None


def test_highlevel_delete_and_flush():
    client = make_client()
    api = ProximaDBDocument(client)
    api.insert("col1", {"a": 1}, id="d1")
    assert api.delete("col1", "doc1") is True
    flushed = api.flush("col1")
    assert flushed["success"] is True


def test_highlevel_insert_document_and_get_document():
    client = make_client()
    api = ProximaDBDocument(client)
    created = api.insert_document("col1", {"a": 1}, id="d1")
    assert created["id"] == "doc1"
    assert created["version"] == 1
    got = api.get_document("col1", "doc1", projection=["a"])
    assert got["found"] is True
    assert got["document"] == {"a": 1}
    # missing -> None
    client.get_document.return_value = None
    assert api.get_document("col1", "absent") is None


def test_highlevel_list_and_delete_collection():
    client = make_client()
    api = ProximaDBDocument(client)
    api.create_collection(name="c")
    assert any(c["id"] == "col1" for c in api.list_collections())
    assert api.delete_collection("col1") is True


def test_highlevel_aggregate_match_and_passthrough():
    client = make_client()
    api = ProximaDBDocument(client)
    api.insert_batch(
        "col1",
        [{"lang": "python"}, {"lang": "rust"}],
        ids=["a", "b"],
    )
    # match stage
    out = api.aggregate(
        "col1",
        [{"stage": "match", "filter": DocumentFilter().eq("lang", "python")}],
    )
    assert len(out["results"]) == 1
    # no recognized stage -> passthrough all docs as dicts
    out2 = api.aggregate("col1", [{"stage": "unknown"}])
    assert len(out2["results"]) == 2
    assert "document" in out2["results"][0]


def test_highlevel_aggregate_group():
    client = make_client()
    api = ProximaDBDocument(client)
    api.insert_batch(
        "col1",
        [
            {"lang": "python", "loc": 10},
            {"lang": "python", "loc": 30},
            {"lang": "rust", "loc": 50},
        ],
        ids=["a", "b", "c"],
    )
    out = api.aggregate(
        "col1",
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
    rows = {r["key"]: r for r in out["results"]}
    assert rows["python"]["cnt"] == 2
    assert rows["python"]["avg_loc"] == 20
    assert rows["python"]["sum_loc"] == 40
    assert rows["rust"]["cnt"] == 1


def test_highlevel_aggregate_group_empty_values():
    client = make_client()
    api = ProximaDBDocument(client)
    api.insert_batch("col1", [{"lang": "go"}], ids=["a"])
    out = api.aggregate(
        "col1",
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


# =============================================================================
# Factory
# =============================================================================


def test_create_document_api_factory():
    client = make_client()
    api = create_document_api(client, enable_cache=False, cache_size=10)
    assert isinstance(api, ProximaDBDocument)
    assert api._repository._enable_cache is False
    assert api._repository._cache_size == 10
