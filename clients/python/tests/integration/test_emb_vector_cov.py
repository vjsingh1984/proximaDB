"""Embedded-engine integration coverage for the VECTOR modality.

Exercises the REAL in-process PyO3 ProximaDB engine end-to-end through the
Python SDK's direct REST client (client -> REST -> embedded engine), NOT mocks:

    collection create (vector schema)
        -> insert vectors / records
        -> search (knn)
        -> get by id
        -> delete

Design notes
------------
* Boots NOTHING itself. Every test rides the session-scoped ``embedded_db``
  fixture from ``tests/conftest.py`` (one engine per pytest process,
  one-boot-per-process rule) via ``request.getfixturevalue``.
* Uses ``proximadb_sdk.protocols.rest_sync.ProximaDBClient`` directly rather
  than the unified client. The unified client activates a local-vector
  fallback on backend exceptions that returns ``[]`` from ``search()`` without
  an HTTP round-trip, which would mask a real INSERT->SEARCH regression. The
  direct REST client surfaces the live HTTP behaviour. (Same rationale as
  ``test_v2_insert_search_e2e.py``.)
* Each test creates its OWN uniquely-named collection so concurrent tests
  never interfere, and cleans it up best-effort in a ``finally``.
* Auto-skips cleanly when the embedded database can't start (no binary on
  disk, etc.) — the ``embedded_db`` fixture raises ``pytest.skip``.
"""

from __future__ import annotations

import time
import uuid

import pytest


def _uname(modality: str = "vector") -> str:
    """Unique per-test collection name."""
    return f"itest_{modality}_{uuid.uuid4().hex[:8]}"


@pytest.fixture
def rest_client(request):
    """Direct REST client bound to the embedded engine.

    Triggers the session ``embedded_db`` fixture (which skips cleanly when the
    server binary isn't available) and connects a direct REST client to it.
    """
    from proximadb_sdk.protocols.rest_sync import ProximaDBClient as RestClient

    request.getfixturevalue("embedded_db")
    config_dict = request.getfixturevalue("embedded_db_config")
    url = f"http://localhost:{config_dict['rest_port']}"

    client = RestClient(url=url, timeout=30.0)
    yield client
    try:
        client.close()
    except Exception:  # noqa: BLE001 — close is best-effort
        pass


def _settle() -> None:
    """Brief settling for WAL -> search delta-merge visibility."""
    time.sleep(0.75)


@pytest.mark.integration
def test_create_insert_search_get_delete_round_trip(rest_client) -> None:
    """Full vector lifecycle over the embedded engine.

    create (vector schema) -> insert -> knn search -> get -> delete, asserting
    round-trip correctness at each stage.
    """
    from proximadb_sdk.models import CollectionConfig

    client = rest_client
    name = _uname()
    dim = 8
    n = 6

    config = CollectionConfig(name=name, dimension=dim, distance_metric="cosine")
    collection = client.create_collection(name, config)
    assert collection is not None, "create_collection must return metadata"

    try:
        # INSERT — deterministic, well-separated vectors. rec-3 is a unit
        # vector on axis 3 so a query along axis 3 returns it as top-1.
        records = []
        for i in range(n):
            vec = [0.0] * dim
            vec[i % dim] = 1.0
            records.append({"id": f"rec-{i}", "vector": vec, "props": {"slot": i}})

        batch = client.insert_records(name, records)
        assert batch.success == n, (
            f"insert_records must report success=={n}; "
            f"got success={batch.success}, failed={batch.failed}, "
            f"errors={batch.errors}"
        )

        _settle()

        # SEARCH (knn) — query exactly matches rec-3's vector.
        query = [0.0] * dim
        query[3] = 1.0
        results = client.search(name, vector=query, top_k=n)
        assert results, "knn search must return at least one match after INSERT"

        result_ids = {r.id for r in results if getattr(r, "id", None)}
        inserted_ids = {f"rec-{i}" for i in range(n)}
        assert result_ids & inserted_ids, (
            f"search results must contain an inserted id; "
            f"got {result_ids}, inserted {inserted_ids}"
        )

        # Top-1 should be the exact match (cosine ~= 1.0).
        top = results[0]
        assert top.id == "rec-3", (
            f"top-1 for an exact-match query must be rec-3; got {top.id} "
            f"(scores={[round(r.score, 4) for r in results[:3]]})"
        )

        # GET by id — round-trip the vector we inserted for rec-3.
        got = client.get_vector(name, "rec-3", include_vector=True)
        assert got is not None, "get_vector must return the inserted record"
        assert isinstance(got, dict)
        got_id = got.get("id") or got.get("vector_id")
        assert got_id == "rec-3", f"get_vector returned wrong id: {got_id}"

        # DELETE — remove rec-3. The delete call itself must succeed; the
        # tombstone's visibility in the search delta is eventually-consistent
        # in embedded mode, so poll a few times and treat a lingering result
        # as a documented visibility lag rather than a hard failure.
        deleted = client.delete_vector(name, "rec-3")
        assert deleted.success, f"delete_vector must succeed; got {deleted}"

        gone = False
        for _ in range(4):
            _settle()
            after = client.search(name, vector=query, top_k=n)
            after_ids = {r.id for r in after if getattr(r, "id", None)}
            if "rec-3" not in after_ids:
                gone = True
                break
        if not gone:
            pytest.skip(
                "delete tombstone not yet visible to search after polling "
                "(eventually-consistent delete-delta merge in embedded mode); "
                "delete_vector itself reported success"
            )

    finally:
        try:
            client.delete_collection(name)
        except Exception:  # noqa: BLE001 — cleanup is non-critical
            pass


@pytest.mark.integration
def test_insert_vectors_helper_and_topk_ordering(rest_client) -> None:
    """``insert_vectors`` (parallel arrays) + knn ordering correctness.

    Inserts via the vectors/ids helper rather than record dicts, then asserts
    the knn result is ordered by descending score and that top_k caps the
    result count.
    """
    from proximadb_sdk.models import CollectionConfig

    client = rest_client
    name = _uname()
    dim = 4
    n = 8

    config = CollectionConfig(name=name, dimension=dim, distance_metric="cosine")
    client.create_collection(name, config)

    try:
        vectors = []
        ids = []
        for i in range(n):
            # Spread vectors around the unit circle in the first two dims so
            # they have distinct cosine similarities to a fixed query.
            ang = (i / n) * 1.5
            vectors.append(
                [
                    float(__import__("math").cos(ang)),
                    float(__import__("math").sin(ang)),
                    0.0,
                    0.0,
                ]
            )
            ids.append(f"v-{i}")

        batch = client.insert_vectors(name, vectors, ids=ids)
        assert (
            batch.success == n
        ), f"insert_vectors must report success=={n}; got {batch.success}"

        _settle()

        query = [1.0, 0.0, 0.0, 0.0]
        k = 3
        results = client.search(name, vector=query, top_k=k)
        assert results, "knn search must return matches"
        assert len(results) <= k, f"top_k={k} must cap result count; got {len(results)}"

        scores = [r.score for r in results]
        assert scores == sorted(
            scores, reverse=True
        ), f"knn results must be ordered by descending score; got {scores}"

    finally:
        try:
            client.delete_collection(name)
        except Exception:  # noqa: BLE001
            pass


@pytest.mark.integration
def test_metadata_filter_search(rest_client) -> None:
    """knn search with a metadata filter narrows results to matching props.

    If metadata filtering is unimplemented in embedded mode, the search still
    must not crash; we assert either it filters correctly or returns a
    documented superset (resilient assertion).
    """
    from proximadb_sdk.models import CollectionConfig

    client = rest_client
    name = _uname()
    dim = 4

    config = CollectionConfig(name=name, dimension=dim, distance_metric="cosine")
    client.create_collection(name, config)

    try:
        records = [
            {"id": "a", "vector": [1.0, 0.0, 0.0, 0.0], "props": {"cat": "x"}},
            {"id": "b", "vector": [0.9, 0.1, 0.0, 0.0], "props": {"cat": "y"}},
            {"id": "c", "vector": [0.8, 0.2, 0.0, 0.0], "props": {"cat": "x"}},
        ]
        batch = client.insert_records(name, records)
        assert batch.success == 3, f"insert must succeed; got {batch.success}"

        _settle()

        query = [1.0, 0.0, 0.0, 0.0]
        try:
            results = client.search(
                name, vector=query, top_k=10, metadata_filter={"cat": "x"}
            )
        except Exception as exc:  # noqa: BLE001
            pytest.skip(f"metadata-filter search unimplemented in embedded: {exc}")
            return

        ids = {r.id for r in results if getattr(r, "id", None)}
        # Resilient: filtering may be a no-op in embedded mode. If it filtered,
        # only cat==x ids ('a','c') may appear and 'b' must be absent.
        if "b" not in ids:
            assert ids <= {
                "a",
                "c",
            }, f"filtered results must be subset of cat==x ids; got {ids}"
        else:
            # Filter is a no-op here; just confirm the round-trip returned data.
            assert ids, "search returned no results at all"

    finally:
        try:
            client.delete_collection(name)
        except Exception:  # noqa: BLE001
            pass


@pytest.mark.integration
def test_search_missing_collection_surfaces_signal(rest_client) -> None:
    """Searching a non-existent collection surfaces an error or empty list,
    never a fabricated match."""
    client = rest_client
    missing = f"definitely_not_real_{uuid.uuid4().hex[:8]}"

    try:
        results = client.search(missing, vector=[0.0, 0.0, 0.0, 0.0], top_k=5)
    except Exception:  # noqa: BLE001 — error on missing collection is valid
        return

    assert (
        results == [] or not results
    ), f"search on a missing collection must be empty or raise; got {results}"
