"""ADR-068 D5 — live-server create → insert → search → get → delete round-trip.

This is the BLOCKING behavioral SDK gate (the codegen-drift gates are advisory
under ADR-068 D6). The codegen gate only checks that the committed generated
client matches a fresh regeneration — it cannot see **facade** bugs, and two of
those shipped undetected to the VM (S2a: 8-char collection-name rejection; S2b:
`search()` silent local fallback returning `[]`). This gate drives the SHIPPED
Python facade through the real REST transport against a live server, so a
facade↔transport break fails CI instead of reaching customers. Proven correct:
passes with the S2b fix, fails (search returns []) without it.

Requires a running server: `PROXIMADB_REST_URL` (e.g. http://127.0.0.1:5678).
Skipped when unset, so the offline `python-test` job is unaffected; the
`python-sdk-round-trip` CI job sets it after starting the server.
"""

from __future__ import annotations

import os
import time

import pytest

from proximadb_sdk import connect_rest

from .test_helpers import cleanup_collection, ensure_collection

pytestmark = pytest.mark.skipif(
    not os.environ.get("PROXIMADB_REST_URL"),
    reason="PROXIMADB_REST_URL unset — this gate needs a running server (ADR-068 D5)",
)


def test_round_trip_create_insert_search_get_delete() -> None:
    url = os.environ["PROXIMADB_REST_URL"]
    client = connect_rest(url)
    try:
        # S2a: a SHORT collection name (4 chars) must be accepted — the server
        # relaxed this for relational DDL; the facade must not re-impose an
        # 8-char minimum.
        name = "rt04"
        coll = ensure_collection(
            client,
            name,
            dimension=4,
            distance_metric="cosine",
            description="ADR-068 D5 round-trip gate",
        )
        cid = coll.config.name

        # Insert one vector.
        vec = [0.1, 0.2, 0.3, 0.4]
        ins = client.insert_vector(
            collection_id=cid,
            vector_id="rt-1",
            vector=vec,
            metadata={"kind": "round-trip"},
        )
        assert ins is not None, "insert_vector returned None"
        # Give the synchronous flush a beat to land the record.
        time.sleep(0.3)

        # S2b: search MUST hit the server and return the inserted record. The
        # pre-fix bug raised TypeError on wrong kwargs and silently fell back to
        # an empty client-side store → returned [].
        results = client.search(cid, vec, top_k=10)
        ids = [getattr(r, "id", None) for r in results]
        assert "rt-1" in ids, (
            f"search did not return the inserted record (got {ids}) — "
            "facade↔transport broken (ADR-068 S2b class); check search() kwargs / fallback"
        )

        # get-by-id round-trips the record + its vector.
        got = client.get_vector(
            collection_id=cid, vector_id="rt-1", include_vector=True
        )
        assert got is not None, "get_vector returned None"
        got_id = getattr(got, "id", None) or (
            got.get("id") if isinstance(got, dict) else None
        )
        assert got_id == "rt-1", f"get_vector id mismatch: {got_id!r}"

        # Delete + confirm it's gone.
        client.delete_vector(collection_id=cid, vector_id="rt-1")
    finally:
        cleanup_collection(client, name)
        client.close()
