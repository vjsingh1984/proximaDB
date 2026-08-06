"""Collection accessors must read the v2 payload shape the server actually sends.

`list_collections()` assumed every descriptor nested its fields under `config`.
The v2 REST API returns them flat, so it raised `KeyError('config')` and
`get_collection()` returned None even on a 200 — which in turn made
`create_collection` -> COLLECTION_EXISTS unrecoverable for callers, since the
natural fallback (fetch the existing one) could never succeed.
"""

import pytest

from proximadb_sdk.embedded import EmbeddedConfig, EmbeddedProximaDB, _collection_field

V2_FLAT = {
    "collection_id": "1",
    "name": "proxima_codegraph_records",
    "dimension": 384,
    "engine": "helix",
    "record_count": None,
}
LEGACY_NESTED = {"config": {"name": "legacy_docs", "dimension": 128}}


def test_collection_field_reads_flat_v2_shape():
    assert _collection_field(V2_FLAT, "name") == "proxima_codegraph_records"
    assert _collection_field(V2_FLAT, "dimension") == 384


def test_collection_field_still_reads_legacy_nested_shape():
    assert _collection_field(LEGACY_NESTED, "name") == "legacy_docs"
    assert _collection_field(LEGACY_NESTED, "dimension") == 128


def test_collection_field_returns_none_for_unknown_or_malformed():
    assert _collection_field(V2_FLAT, "nope") is None
    assert _collection_field({}, "name") is None
    assert _collection_field(None, "name") is None
    assert _collection_field("not-a-dict", "name") is None


def _db(monkeypatch, entries):
    db = EmbeddedProximaDB(config=EmbeddedConfig(data_dir="/tmp/does-not-matter"))
    db._started = True

    async def fake_entries():
        return entries

    monkeypatch.setattr(db, "_collection_entries", fake_entries)
    return db


@pytest.mark.asyncio
async def test_list_collections_parses_flat_entries(monkeypatch):
    db = _db(monkeypatch, [V2_FLAT, LEGACY_NESTED])
    assert await db.list_collections() == ["proxima_codegraph_records", "legacy_docs"]


@pytest.mark.asyncio
async def test_list_collections_skips_entries_without_a_name(monkeypatch):
    db = _db(monkeypatch, [V2_FLAT, {"collection_id": "2"}])
    assert await db.list_collections() == ["proxima_codegraph_records"]


@pytest.mark.asyncio
async def test_get_collection_resolves_an_existing_collection(monkeypatch):
    db = _db(monkeypatch, [V2_FLAT])
    coll = await db.get_collection("proxima_codegraph_records")
    assert coll is not None
    assert coll.dimension == 384


@pytest.mark.asyncio
async def test_get_collection_returns_none_for_a_missing_collection(monkeypatch):
    """Must not hand back a phantom handle.

    GET /api/v2/collections/{name} answers 200 for a name that does not exist,
    with dimension 0 and a created_at equal to the request instant. Resolving
    through LIST — which reports only real collections — avoids adopting a
    collection that silently writes nowhere.
    """
    db = _db(monkeypatch, [V2_FLAT])
    assert await db.get_collection("never_created") is None


@pytest.mark.asyncio
async def test_get_collection_prefers_the_process_cache(monkeypatch):
    db = _db(monkeypatch, [V2_FLAT])
    first = await db.get_collection("proxima_codegraph_records")
    monkeypatch.setattr(db, "_collection_entries", None)  # would raise if consulted
    assert await db.get_collection("proxima_codegraph_records") is first


if __name__ == "__main__":  # pragma: no cover
    raise SystemExit(pytest.main([__file__, "-v"]))
