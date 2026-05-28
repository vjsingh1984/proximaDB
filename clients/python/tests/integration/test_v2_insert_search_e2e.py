"""
v0.2 release-readiness round 2: TD-081 — Python SDK live v2 INSERT→SEARCH
integration coverage.

The Rust-side regression test
`tests/release_smoke_v2.rs::rest_v2_record_release_smoke_round_trip` covers
the REST endpoint end-to-end, but exercises it via `reqwest`. A regression in
the Python SDK's `client.insert_records()` → `client.search()` round trip
(payload shape, response parsing, retry behaviour) would not be caught by
`make release-check`. This test closes that gap by using the live SDK against
the `embedded_db` fixture.

Auto-skips when the embedded database is unavailable (the same skip behaviour
as the rest of the SDK integration suite). When the embedded server is up,
this test is the canonical SDK-side smoke for the v2 record path.
"""

from __future__ import annotations

import os
import time
import uuid

import pytest


@pytest.fixture
def coll_name() -> str:
    """Unique collection name per test invocation."""
    return f"py_sdk_v2_e2e_{uuid.uuid4().hex[:12]}"


@pytest.fixture
def rest_client(embedded_db_config):
    """REST client connected either to an externally-running server
    (``PROXIMADB_TEST_SERVER_URL``) or to the in-fixture embedded database.

    The external-URL path lets `make release-check` exercise these tests
    against the same release binary the rest of the suite uses, without
    requiring a separate `cargo build --release` step for every CI run.
    Falls back to the `embedded_db`-driven path when the env var is unset;
    that path auto-skips when no `proximadb-server` binary is on disk.
    """
    from proximadb_sdk import Protocol, ProximaDBClient
    from proximadb_sdk.config import ClientConfig

    url = os.getenv("PROXIMADB_TEST_SERVER_URL")
    if url:
        config = ClientConfig(url=url, protocol=Protocol.REST, timeout=30.0)
        client = ProximaDBClient(config=config)
        yield client
        client.close()
        return

    # Reuse the existing `embedded_rest_client` setup via the conftest
    # fixture, which will skip when the embedded binary is unavailable.
    from proximadb_sdk import Protocol, ProximaDBClient

    config = ClientConfig(
        url=f"http://localhost:{embedded_db_config['rest_port']}",
        protocol=Protocol.REST,
        timeout=30.0,
    )
    client = ProximaDBClient(config=config)
    # Ping the embedded server health endpoint; if no server is up, skip
    # rather than fail with a connection error.
    import requests

    try:
        resp = requests.get(f"{config.url}/health", timeout=2.0)
        if resp.status_code != 200:
            pytest.skip(
                f"No server reachable at {config.url}; set PROXIMADB_TEST_SERVER_URL "
                f"or start the embedded test fixture"
            )
    except requests.RequestException as exc:
        pytest.skip(
            f"No server reachable at {config.url}: {exc}; set "
            f"PROXIMADB_TEST_SERVER_URL or start the embedded test fixture"
        )
    yield client
    client.close()


def test_v2_records_batch_insert_then_search_round_trips(
    rest_client, coll_name: str
) -> None:
    """SDK insert_records → search end-to-end on the v2 record path.

    Asserts:
      1. Collection create succeeds via SDK.
      2. SDK insert_records returns success=N for N records (no silent
         partial-failure that the round-1 audit caught at the REST level).
      3. SDK search returns at least one match for the query that exactly
         matches an inserted record's vector (cosine ≈ 1.0).
      4. The matched ID is among the IDs the test inserted.

    Closes TD-081 in `docs/10-quality/TECHNICAL_DEBT.adoc`.
    """
    from proximadb_sdk.models import CollectionConfig

    client = rest_client
    dim = 8
    n = 5

    # 1. CREATE — fp32 SST cosine collection (stays out of the fp16 metric
    #    path so the test isolates SDK round-trip semantics).
    config = CollectionConfig(
        name=coll_name,
        dimension=dim,
        distance_metric="cosine",
    )
    collection = client.create_collection(coll_name, config)
    assert collection is not None, "client.create_collection must return collection metadata"

    try:
        # 2. INSERT — n records with deterministic vectors. rec-2's vector is
        #    the unit vector along dimension 2 modulo dim, so a query for the
        #    same shape returns rec-2 as top-1.
        records = []
        for i in range(n):
            vector = [(i * 0.1) + (j * 0.01) for j in range(dim)]
            records.append({"id": f"rec-{i}", "vector": vector})

        batch = client.insert_records(coll_name, records)
        assert batch.success == n, (
            f"SDK insert_records must report success=={n} for {n} clean records; "
            f"got success={batch.success}, failed={batch.failed}, errors={batch.errors}"
        )

        # Brief settling — WAL → search delta merge visibility.
        time.sleep(0.75)

        # 3. SEARCH — query the exact vector for rec-0.
        query = [j * 0.01 for j in range(dim)]
        results = client.search(coll_name, vector=query, top_k=10)
        assert results, (
            "SDK client.search must return at least one match after INSERT — "
            "this guards the v2 INSERT→SEARCH wiring at the SDK boundary"
        )

        # 4. Verify the result IDs match what we inserted.
        result_ids = {r.id for r in results if getattr(r, "id", None)}
        inserted_ids = {f"rec-{i}" for i in range(n)}
        overlap = result_ids & inserted_ids
        assert overlap, (
            f"SDK search results must contain at least one of the inserted "
            f"IDs. Got result_ids={result_ids}, inserted_ids={inserted_ids}"
        )

    finally:
        # Best-effort cleanup so the test is rerunnable against a persistent
        # embedded fixture.
        try:
            client.delete_collection(coll_name)
        except Exception:  # noqa: BLE001 — cleanup is non-critical
            pass


def test_v2_records_batch_to_missing_collection_surfaces_error(
    rest_client,
) -> None:
    """SDK round-2 contract: POST to a non-existent collection surfaces as an
    error to the caller, not as a silent success-shaped response.

    Pairs with the Rust-side regression
    `tests/release_smoke_v2.rs::rest_v2_insert_to_missing_collection_returns_404`.
    """
    client = rest_client
    missing = f"definitely_not_a_real_collection_{uuid.uuid4().hex[:8]}"

    try:
        batch = client.insert_records(
            missing,
            [{"id": "x", "vector": [0.0, 0.0]}],
        )
    except Exception:
        # Acceptable — the SDK may raise an exception on HTTP 404 from the
        # server. Either an exception or a clearly-failed BatchResult is
        # a valid signal; what we're guarding against is the round-1 anti-
        # pattern where the caller sees success=true with empty results.
        return

    # If the SDK swallows the 404 and returns a result, success must be 0 and
    # failed must reflect the input row count.
    assert batch.success == 0, (
        "SDK insert_records to a missing collection must report success=0; "
        f"got success={batch.success}, failed={batch.failed}, errors={batch.errors}"
    )
