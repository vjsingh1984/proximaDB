import os
import uuid
import numpy as np
import pytest

from proximadb_sdk.unified_client import ProximaDBClient
from proximadb_sdk.models import VectorRecord


def rest_available(url: str) -> bool:
    import httpx

    try:
        r = httpx.get(url.rstrip("/") + "/api/v1/health", timeout=2.0)
        return r.status_code < 500
    except Exception:
        return False


@pytest.mark.integration
def test_unified_client_end_to_end():
    base_url = os.getenv("PROXIMADB_URL", "http://localhost:5678")
    if not rest_available(base_url):
        pytest.skip(
            "ProximaDB REST server not available; set PROXIMADB_URL and start server to run integration tests."
        )

    # Force REST path for this integration test (gRPC covered separately)
    client = ProximaDBClient(url=base_url, protocol="rest", sks_warmup_collection=None)

    coll = f"py_sdk_uc_{uuid.uuid4().hex[:8]}"
    try:
        # Create collection via unified client
        client.create_collection(coll, dimension=4)

        # Insert vectors (new API with VectorRecord)
        records = [
            VectorRecord(id="a", vector=[0.1, 0.2, 0.3, 0.4]),
            VectorRecord(id="b", vector=[0.11, 0.21, 0.31, 0.39]),
        ]
        ins = client.insert_vectors(coll, records=records)
        assert ins.metrics.successful_count >= 1

        # Search single
        res = client.search_single(coll, vector=[0.1, 0.2, 0.3, 0.4], top_k=2)
        assert isinstance(res, list)
        assert len(res) >= 1
        assert res[0].id

        # Delete one vector
        delres = client.delete_vector(coll, "a")
        assert delres.success is True

    finally:
        try:
            client.delete_collection(coll)
        except Exception:
            pass
