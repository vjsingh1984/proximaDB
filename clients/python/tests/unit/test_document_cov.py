"""Offline unit tests for proximadb_sdk.document.

Fully offline: the backend client is a MagicMock / hand fake; no network,
no server, no model downloads. Exercises the data models, filter builder,
query result, repository CRUD/query/aggregate/index/cache paths, and the
high-level ProximaDBDocument facade.
"""

from __future__ import annotations

import asyncio
from datetime import datetime

import pytest
from unittest.mock import MagicMock

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
    """The repository keeps class-level shared dicts; isolate each test."""
    DocumentRepository._shared_batch_buffer.clear()
    DocumentRepository._shared_collections.clear()
    DocumentRepository._shared_documents.clear()
    yield
    DocumentRepository._shared_batch_buffer.clear()
    DocumentRepository._shared_collections.clear()
    DocumentRepository._shared_documents.clear()


def make_client(**overrides):
    """A MagicMock backend client with sensible default return shapes."""
    client = MagicMock()
    client.create_document_collection.return_value = {"collection_id": "coll1"}
    # Default: server echoes no id, so the code keeps the caller-supplied doc_id.
    client.insert_document.return_value = {}
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
    assert DocIndexType.FULLTEXT.value == "fulltext"
    assert CompressionAlgorithm.ZSTD.value == "zstd"
    assert QueryStrategy.AUTO.value == "auto"
    assert QueryStrategy.INDEX_ONLY.value == "index_only"


# =============================================================================
# IndexDefinition / DocumentCollectionConfig
# =============================================================================


def test_index_definition_to_dict_autoname():
    idx = IndexDefinition(path="$.user.email", type=DocIndexType.HASH, unique=True)
    d = idx.to_dict()
    assert d["index_type"] == "hash"
    assert d["unique"] is True
    assert d["path"] == "$.user.email"
    # name derived from path: "$.user.email" -> ".user.email" -> "_user_email"
    assert d["name"] == "idx__user_email"


def test_index_definition_explicit_name():
    idx = IndexDefinition(name="my_idx", path="$.x")
    assert idx.to_dict()["name"] == "my_idx"


def test_collection_config_to_dict():
    cfg = DocumentCollectionConfig(
        name="files",
        json_schema='{"type":"object"}',
        indexes=[IndexDefinition(path="$.lang", type=DocIndexType.HASH)],
        enable_fulltext=True,
        fulltext_paths=["$.content"],
        ttl_seconds=60,
        compression=CompressionAlgorithm.ZSTD,
    )
    d = cfg.to_dict()
    assert d["name"] == "files"
    assert d["enable_fulltext"] is True
    assert d["fulltext_paths"] == ["$.content"]
    assert d["ttl_seconds"] == 60
    assert d["compression"] == "zstd"
    assert len(d["indexes"]) == 1


# =============================================================================
# Document model
# =============================================================================


def test_document_to_dict_minimal():
    doc = Document(id="x", content={"a": 1})
    d = doc.to_dict()
    assert d == {"id": "x", "document": {"a": 1}}


def test_document_to_dict_full():
    now = datetime(2024, 1, 1, 12, 0, 0)
    doc = Document(
        id="x",
        content={"a": 1},
        created_at=now,
        updated_at=now,
        metadata={"m": 1},
    )
    d = doc.to_dict()
    assert d["created_at"] == now.isoformat()
    assert d["updated_at"] == now.isoformat()
    assert d["metadata"] == {"m": 1}


def test_document_from_dict_roundtrip():
    now = datetime(2024, 1, 1, 12, 0, 0)
    data = {
        "id": "x",
        "document": {"a": 1},
        "version": 5,
        "created_at": now.isoformat(),
        "updated_at": now.isoformat(),
        "metadata": {"m": 2},
    }
    doc = Document.from_dict(data)
    assert doc.id == "x"
    assert doc.version == 5
    assert doc.created_at == now
    assert doc.metadata == {"m": 2}


def test_document_from_dict_no_timestamps():
    doc = Document.from_dict({"id": "x", "document": {}})
    assert doc.created_at is None
    assert doc.updated_at is None
    assert doc.version == 1


# =============================================================================
# DocumentFilter builder
# =============================================================================


def test_filter_all_operators():
    f = DocumentFilter()
    f.eq("a", 1).ne("b", 2).gt("c", 3).gte("d", 4).lt("e", 5).lte("f", 6)
    f.contains("g", "x").fulltext("h", "y").starts_with("i", "z")
    f.ends_with("j", "w").in_list("k", [1, 2]).exists("l")
    d = f.to_dict()
    ops = [c["op"] for c in d["conditions"]]
    assert ops == [
        "eq", "ne", "gt", "gte", "lt", "lte",
        "contains", "fulltext", "starts_with", "ends_with", "in", "exists",
    ]
    assert d["logic"] == "AND"


def test_filter_and_or_switch():
    f = DocumentFilter().eq("a", 1).or_().eq("b", 2)
    assert f.to_dict()["logic"] == "OR"
    f.and_()
    assert f.to_dict()["logic"] == "AND"


def test_filter_group_nesting():
    inner = DocumentFilter().eq("x", 1)
    f = DocumentFilter().eq("y", 2).group(inner)
    d = f.to_dict()
    assert len(d["groups"]) == 1
    assert d["groups"][0]["conditions"][0]["path"] == "x"


def test_filter_or_operator_overload():
    a = DocumentFilter().eq("x", 1)
    b = DocumentFilter().eq("y", 2)
    combined = a | b
    d = combined.to_dict()
    assert d["logic"] == "OR"
    assert len(d["groups"]) == 2


def test_filter_and_operator_overload():
    a = DocumentFilter().eq("x", 1)
    b = DocumentFilter().eq("y", 2)
    combined = a & b
    d = combined.to_dict()
    assert d["logic"] == "AND"
    assert len(d["groups"]) == 2


# =============================================================================
# DocumentQueryResult (lazy loading, async)
# =============================================================================


def test_query_result_basic_props():
    docs = [Document(id="1", content={}), Document(id="2", content={})]
    r = DocumentQueryResult(documents=docs, total_count=2)
    assert r.documents == docs
    assert r.total_count == 2
    assert r.has_more is False
    assert len(r) == 2
    assert [d.id for d in r] == ["1", "2"]


def test_query_result_fetch_next_no_more():
    r = DocumentQueryResult(documents=[], total_count=0, has_more=False)
    assert asyncio.run(r.fetch_next_batch()) == []


def test_query_result_fetch_next_and_all():
    calls = {"n": 0}

    async def fetch_fn():
        calls["n"] += 1
        return [Document(id="new", content={})]

    r = DocumentQueryResult(
        documents=[Document(id="0", content={})],
        total_count=10,
        has_more=True,
        fetch_fn=fetch_fn,
        batch_size=100,
    )

    async def run():
        batch = await r.fetch_next_batch()
        assert len(batch) == 1
        assert r.has_more is False
        return await r.to_list()

    out = asyncio.run(run())
    assert len(out) == 2
    assert calls["n"] == 1


# =============================================================================
# DocumentQueryResponse
# =============================================================================


def test_query_response_dict_access():
    resp = DocumentQueryResponse(
        documents=[{"id": "1"}, {"id": "2"}], total_count=2, has_more=True
    )
    assert len(resp) == 2
    assert list(resp) == [{"id": "1"}, {"id": "2"}]
    assert resp.get("total_count") == 2
    assert resp.get("missing", "def") == "def"
    assert resp.to_dict()["has_more"] is True


# =============================================================================
# DocumentRepository — helpers
# =============================================================================


def test_normalize_path_variants():
    assert DocumentRepository._normalize_path("$.a.b") == "a.b"
    assert DocumentRepository._normalize_path("$a") == "a"
    assert DocumentRepository._normalize_path("plain") == "plain"


def test_get_value_nested_and_missing():
    repo = DocumentRepository(make_client())
    doc = {"user": {"email": "x@y.com"}}
    assert repo._get_value(doc, "$.user.email") == "x@y.com"
    assert repo._get_value(doc, "$.user.missing") is None
    assert repo._get_value({"a": 5}, "$.a.b") is None


def test_matches_condition_every_op():
    repo = DocumentRepository(make_client())
    doc = {"n": 5, "s": "hello", "lst": [1, 2]}
    assert repo._matches_condition(doc, {"path": "n", "op": "eq", "value": 5})
    assert repo._matches_condition(doc, {"path": "n", "op": "ne", "value": 4})
    assert repo._matches_condition(doc, {"path": "n", "op": "gt", "value": 1})
    assert repo._matches_condition(doc, {"path": "n", "op": "gte", "value": 5})
    assert repo._matches_condition(doc, {"path": "n", "op": "lt", "value": 9})
    assert repo._matches_condition(doc, {"path": "n", "op": "lte", "value": 5})
    assert repo._matches_condition(doc, {"path": "s", "op": "contains", "value": "ELL"})
    assert repo._matches_condition(doc, {"path": "s", "op": "starts_with", "value": "he"})
    assert repo._matches_condition(doc, {"path": "s", "op": "ends_with", "value": "lo"})
    assert repo._matches_condition(doc, {"path": "n", "op": "in", "value": [5, 6]})
    assert repo._matches_condition(doc, {"path": "n", "op": "exists"})
    assert repo._matches_condition(doc, {"path": "s", "op": "fulltext", "value": "hello"})
    assert repo._matches_condition(doc, {"path": "n", "op": "weird", "value": 1})
    assert not repo._matches_condition(doc, {"path": "missing", "op": "gt", "value": 1})


def test_matches_filter_none_and_empty():
    repo = DocumentRepository(make_client())
    assert repo._matches_filter({"a": 1}, None) is True
    assert repo._matches_filter({"a": 1}, {}) is True


def test_matches_filter_plain_dict():
    repo = DocumentRepository(make_client())
    assert repo._matches_filter({"a": 1, "b": 2}, {"a": 1}) is True
    assert repo._matches_filter({"a": 1}, {"a": 9}) is False


def test_matches_filter_and_or_logic():
    repo = DocumentRepository(make_client())
    doc = {"a": 1, "b": 2}
    f_and = DocumentFilter().eq("a", 1).eq("b", 2)
    assert repo._matches_filter(doc, f_and) is True
    f_and_fail = DocumentFilter().eq("a", 1).eq("b", 99)
    assert repo._matches_filter(doc, f_and_fail) is False
    f_or = DocumentFilter().eq("a", 99).or_().eq("b", 2)
    assert repo._matches_filter(doc, f_or) is True


def test_matches_filter_empty_conditions_key():
    repo = DocumentRepository(make_client())
    assert repo._matches_filter({"a": 1}, {"conditions": [], "groups": []}) is True


def test_project_document():
    repo = DocumentRepository(make_client())
    doc = {"user": {"email": "x"}, "name": "n", "ignored": "z"}
    assert repo._project_document(doc, None) == doc
    proj = repo._project_document(doc, ["$.user.email", "$.name"])
    assert proj == {"email": "x", "name": "n"}
    proj2 = repo._project_document(doc, ["$.nope"])
    assert proj2 == {}


def test_apply_updates_dict():
    repo = DocumentRepository(make_client())
    out = repo._apply_updates({"a": 1}, {"a": 2, "b": 3})
    assert out == {"a": 2, "b": 3}


def test_apply_updates_set_and_push():
    repo = DocumentRepository(make_client())
    base = {"meta": {}}
    out = repo._apply_updates(
        base,
        [
            {"operation": "SET", "path": "$.meta.x", "value": 5},
            {"operation": "PUSH", "path": "$.tags", "value": "a"},
            {"operation": "PUSH", "path": "$.tags", "value": "b"},
            {"operation": "SET", "path": "", "value": 1},
        ],
    )
    assert out["meta"]["x"] == 5
    assert out["tags"] == ["a", "b"]


def test_apply_updates_push_existing_scalar():
    repo = DocumentRepository(make_client())
    out = repo._apply_updates(
        {"tags": "first"},
        [{"operation": "PUSH", "path": "$.tags", "value": "second"}],
    )
    assert out["tags"] == ["first", "second"]


# =============================================================================
# DocumentRepository — collection management
# =============================================================================


def test_create_collection_success():
    client = make_client()
    repo = DocumentRepository(client)
    cfg = DocumentCollectionConfig(name="c", indexes=[IndexDefinition(path="$.x")])
    cid = repo.create_collection(cfg)
    assert cid == "coll1"
    client.create_document_collection.assert_called_once()
    assert "coll1" in repo._collections


def test_create_collection_default_id():
    client = make_client(create_document_collection={})
    repo = DocumentRepository(client)
    cfg = DocumentCollectionConfig(name="named")
    cid = repo.create_collection(cfg)
    assert cid == "named"


def test_create_collection_error():
    client = MagicMock()
    client.create_document_collection.side_effect = RuntimeError("boom")
    repo = DocumentRepository(client)
    with pytest.raises(ProximaDBError):
        repo.create_collection(DocumentCollectionConfig(name="c"))


def test_get_collection_and_list():
    client = make_client()
    repo = DocumentRepository(client)
    assert repo.get_collection("nope") is None
    cfg = DocumentCollectionConfig(name="c", indexes=[IndexDefinition(path="$.x")])
    repo.create_collection(cfg)
    info = repo.get_collection("coll1")
    assert info["name"] == "c"
    assert info["document_count"] == 0
    assert len(repo.list_collections()) == 1


def test_delete_collection():
    client = make_client()
    repo = DocumentRepository(client)
    repo.create_collection(DocumentCollectionConfig(name="c"))
    repo.insert("coll1", {"a": 1}, id="d1")
    assert repo.delete_collection("coll1") is True
    assert repo.get_collection("coll1") is None


# =============================================================================
# DocumentRepository — CRUD
# =============================================================================


def test_insert_success_uses_server_id():
    client = make_client(insert_document={"id": "server-id"})
    repo = DocumentRepository(client)
    doc = repo.insert("coll1", {"a": 1}, id="given")
    assert doc.id == "server-id"
    assert repo._documents["coll1"]["server-id"] is doc


def test_insert_autogenerated_id():
    client = make_client(insert_document={})
    repo = DocumentRepository(client)
    doc = repo.insert("coll1", {"a": 1})
    assert doc.id.startswith("doc:")


def test_insert_error():
    client = MagicMock()
    client.insert_document.side_effect = RuntimeError("nope")
    repo = DocumentRepository(client)
    with pytest.raises(ProximaDBError):
        repo.insert("coll1", {"a": 1})


def test_insert_batch_and_id_mismatch():
    repo = DocumentRepository(make_client())
    docs = repo.insert_batch("coll1", [{"a": 1}, {"a": 2}], ids=["x", "y"])
    assert [d.id for d in docs] == ["x", "y"]
    auto = repo.insert_batch("coll1", [{"b": 1}])
    assert auto[0].id.startswith("doc:")
    with pytest.raises(ValueError):
        repo.insert_batch("coll1", [{"a": 1}], ids=["x", "y"])


def test_get_from_cache():
    client = make_client()
    repo = DocumentRepository(client)
    repo.insert("coll1", {"a": 1}, id="d1")
    client.get_document.reset_mock()
    got = repo.get("coll1", "d1", use_cache=True)
    assert got is not None
    client.get_document.assert_not_called()


def test_get_from_server():
    client = make_client(get_document={"id": "d1", "data": {"a": 9}})
    repo = DocumentRepository(client)
    got = repo.get("coll1", "d1", use_cache=False)
    assert got.content == {"a": 9}


def test_get_returns_none_when_server_none():
    client = make_client(get_document=None)
    repo = DocumentRepository(client)
    assert repo.get("coll1", "missing", use_cache=False) is None


def test_get_fallback_to_local_storage():
    client = make_client()
    repo = DocumentRepository(client)
    repo.insert("coll1", {"a": 1}, id="d1")
    client.get_document.side_effect = RuntimeError("down")
    got = repo.get("coll1", "d1", use_cache=False)
    assert got is not None


def test_get_raises_when_no_local_fallback():
    client = make_client()
    client.get_document.side_effect = RuntimeError("down")
    repo = DocumentRepository(client)
    with pytest.raises(ProximaDBError):
        repo.get("coll1", "absent", use_cache=False)


def test_query_from_server():
    client = make_client()
    repo = DocumentRepository(client)
    res = repo.query("coll1", filter=DocumentFilter().eq("a", 1))
    assert res.total_count == 1
    assert res.documents[0].id == "doc1"


def test_query_fallback_local():
    client = make_client()
    client.query_documents.side_effect = RuntimeError("down")
    repo = DocumentRepository(client)
    repo.insert("coll1", {"lang": "py", "n": 1}, id="d1")
    repo.insert("coll1", {"lang": "go", "n": 2}, id="d2")
    res = repo.query(
        "coll1",
        filter=DocumentFilter().eq("lang", "py"),
        projection=["$.lang"],
        limit=10,
        offset=0,
    )
    assert res.total_count == 1
    assert res.documents[0].content == {"lang": "py"}


def test_query_fallback_pagination_has_more():
    client = make_client()
    client.query_documents.side_effect = RuntimeError("down")
    repo = DocumentRepository(client)
    for i in range(5):
        repo.insert("coll1", {"n": i}, id=f"d{i}")
    res = repo.query("coll1", limit=2, offset=0)
    assert res.total_count == 5
    assert res.has_more is True
    assert len(res.documents) == 2


def test_search_delegates_to_query():
    client = make_client(
        query_documents={
            "documents": [{"id": "d1", "data": {"content": "json parser"}}],
            "total_count": 1,
            "has_more": False,
        }
    )
    repo = DocumentRepository(client)
    out = repo.search("coll1", "json", limit=5)
    assert out[0].id == "d1"


def test_update_existing_and_missing():
    repo = DocumentRepository(make_client())
    repo.insert("coll1", {"a": 1}, id="d1")
    updated = repo.update("coll1", "d1", {"a": 2})
    assert updated.content["a"] == 2
    assert updated.version == 2
    assert repo.update("coll1", "absent", {"a": 1}) is None


def test_delete_existing_and_missing():
    repo = DocumentRepository(make_client())
    repo.insert("coll1", {"a": 1}, id="d1")
    assert repo.delete("coll1", "d1") is True
    assert repo.delete("coll1", "d1") is False


def test_delete_by_filter_clears_cache_returns_zero():
    repo = DocumentRepository(make_client())
    repo.insert("coll1", {"a": 1}, id="d1")
    assert repo.delete_by_filter("coll1", DocumentFilter().eq("a", 1)) == 0


# =============================================================================
# DocumentRepository — batch / index / cache
# =============================================================================


def test_flush_batch_empty_and_populated():
    repo = DocumentRepository(make_client())
    assert repo.flush_batch("none")["flushed"] == 0
    repo.insert("coll1", {"a": 1}, id="d1")
    res = repo.flush_batch("coll1")
    assert res["flushed"] == 1
    assert res["success"] is True
    assert repo.flush_batch("coll1")["flushed"] == 0


def test_index_methods_stub():
    repo = DocumentRepository(make_client())
    assert repo.create_index("coll1", IndexDefinition(path="$.x")) is True
    assert repo.drop_index("coll1", "idx") is True
    assert repo.list_indexes("coll1") == []


def test_cache_eviction_lru():
    repo = DocumentRepository(make_client(), cache_size=2)
    repo._update_cache("a", Document(id="a", content={}))
    repo._update_cache("b", Document(id="b", content={}))
    repo._update_cache("c", Document(id="c", content={}))
    assert "a" not in repo._cache
    assert "b" in repo._cache and "c" in repo._cache


def test_clear_cache_targeted_and_all():
    repo = DocumentRepository(make_client())
    repo._update_cache("coll1:1", Document(id="1", content={}))
    repo._update_cache("coll2:2", Document(id="2", content={}))
    repo.clear_cache("coll1")
    assert "coll1:1" not in repo._cache
    assert "coll2:2" in repo._cache
    repo.clear_cache()
    assert repo._cache == {}


def test_get_cache_stats():
    repo = DocumentRepository(make_client(), cache_size=10)
    repo._update_cache("k", Document(id="k", content={}))
    stats = repo.get_cache_stats()
    assert stats["size"] == 1
    assert stats["capacity"] == 10
    assert stats["hit_rate"] == 0.0


def test_cache_disabled_get_path():
    client = make_client(get_document={"id": "d1", "data": {"a": 1}})
    repo = DocumentRepository(client, enable_cache=False)
    repo.insert("coll1", {"a": 1}, id="d1")
    got = repo.get("coll1", "d1")
    assert got is not None


# =============================================================================
# ProximaDBDocument facade
# =============================================================================


def test_facade_create_collection_by_args():
    client = make_client()
    docs = ProximaDBDocument(client)
    cid = docs.create_collection(
        name="files",
        indexes=[IndexDefinition(path="$.lang")],
        enable_fulltext=True,
        fulltext_paths=["$.content"],
        json_schema=None,
    )
    assert cid == "coll1"


def test_facade_create_collection_by_config_returns_dict():
    client = make_client()
    docs = ProximaDBDocument(client)
    out = docs.create_collection(config=DocumentCollectionConfig(name="c"))
    assert out == {"success": True, "collection_id": "coll1"}


def test_facade_create_collection_requires_name():
    docs = ProximaDBDocument(make_client())
    with pytest.raises(ValueError):
        docs.create_collection()


def test_facade_insert_and_get():
    client = make_client(insert_document={"id": "d1"})
    docs = ProximaDBDocument(client)
    created = docs.insert("coll1", {"a": 1}, id="d1")
    assert created.id == "d1"
    got = docs.get("coll1", "d1")
    assert got is not None


def test_facade_insert_batch():
    docs = ProximaDBDocument(make_client())
    out = docs.insert_batch("coll1", [{"a": 1}, {"a": 2}], ids=["x", "y"])
    assert len(out) == 2


def test_facade_query_returns_response():
    client = make_client()
    docs = ProximaDBDocument(client)
    resp = docs.query("coll1", filter=DocumentFilter().eq("a", 1), limit=10)
    assert isinstance(resp, DocumentQueryResponse)
    assert resp.total_count == 1
    assert resp.documents[0]["id"] == "doc1"


def test_facade_search():
    client = make_client(
        query_documents={
            "documents": [{"id": "d1", "data": {"content": "x"}}],
            "total_count": 1,
            "has_more": False,
        }
    )
    docs = ProximaDBDocument(client)
    out = docs.search("coll1", "x", limit=3)
    assert out[0].id == "d1"


def test_facade_update_existing_and_missing():
    docs = ProximaDBDocument(make_client())
    docs.insert("coll1", {"a": 1}, id="d1")
    res = docs.update("coll1", "d1", {"a": 2})
    assert res["success"] is True
    assert res["new_version"] == 2
    assert res["document"]["a"] == 2
    assert docs.update("coll1", "absent", {"a": 1}) is None


def test_facade_delete_and_flush():
    docs = ProximaDBDocument(make_client())
    docs.insert("coll1", {"a": 1}, id="d1")
    assert docs.delete("coll1", "d1") is True
    flush = docs.flush("coll1")
    assert flush["success"] is True


def test_facade_insert_document_wire_shape():
    client = make_client(insert_document={"id": "d1"})
    docs = ProximaDBDocument(client)
    out = docs.insert_document("coll1", {"a": 1}, id="d1")
    assert out == {"id": "d1", "version": 1, "document": {"a": 1}}


def test_facade_get_document_found_and_missing():
    client = make_client(insert_document={"id": "d1"})
    docs = ProximaDBDocument(client)
    docs.insert("coll1", {"a": 1, "b": 2}, id="d1")
    out = docs.get_document("coll1", "d1", projection=["$.a"])
    assert out["found"] is True
    assert out["document"] == {"a": 1}
    client.get_document.return_value = None
    assert docs.get_document("coll1", "absent") is None


def test_facade_list_and_delete_collection():
    client = make_client()
    docs = ProximaDBDocument(client)
    docs.create_collection(name="c")
    assert len(docs.list_collections()) >= 1
    assert docs.delete_collection("coll1") is True


def test_facade_aggregate_match_and_default():
    docs = ProximaDBDocument(make_client())
    docs.insert("coll1", {"lang": "py", "n": 1}, id="d1")
    docs.insert("coll1", {"lang": "go", "n": 2}, id="d2")
    res = docs.aggregate(
        "coll1", [{"stage": "match", "filter": DocumentFilter().eq("lang", "py")}]
    )
    assert len(res["results"]) == 1
    res2 = docs.aggregate("coll1", [])
    assert len(res2["results"]) == 2


def test_facade_aggregate_group():
    docs = ProximaDBDocument(make_client())
    docs.insert("coll1", {"lang": "py", "n": 10}, id="d1")
    docs.insert("coll1", {"lang": "py", "n": 20}, id="d2")
    docs.insert("coll1", {"lang": "go", "n": 5}, id="d3")
    res = docs.aggregate(
        "coll1",
        [
            {
                "stage": "group",
                "key": "$.lang",
                "aggregations": [
                    {"field": "cnt", "type": "count", "path": "$.n"},
                    {"field": "total", "type": "sum", "path": "$.n"},
                    {"field": "mean", "type": "avg", "path": "$.n"},
                ],
            }
        ],
    )
    rows = {r["key"]: r for r in res["results"]}
    assert rows["py"]["cnt"] == 2
    assert rows["py"]["total"] == 30
    assert rows["py"]["mean"] == 15
    assert rows["go"]["total"] == 5


def test_facade_aggregate_group_empty_values():
    docs = ProximaDBDocument(make_client())
    docs.insert("coll1", {"lang": "py"}, id="d1")
    res = docs.aggregate(
        "coll1",
        [
            {
                "stage": "group",
                "key": "$.lang",
                "aggregations": [
                    {"field": "mean", "type": "avg", "path": "$.n"},
                    {"field": "total", "type": "sum", "path": "$.n"},
                ],
            }
        ],
    )
    row = res["results"][0]
    assert row["mean"] == 0
    assert row["total"] == 0


# =============================================================================
# Factory
# =============================================================================


def test_create_document_api_factory():
    api = create_document_api(make_client(), enable_cache=True, cache_size=50)
    assert isinstance(api, ProximaDBDocument)
