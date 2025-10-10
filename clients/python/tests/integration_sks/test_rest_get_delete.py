import os
import uuid
import numpy as np
import pytest

from proximadb.protocols.rest_sync import ProximaDBClient
from proximadb.models import CollectionConfig


def server_available(url: str) -> bool:
    import httpx
    try:
        r = httpx.get(url.rstrip("/") + "/api/v1/health", timeout=2.0)
        return r.status_code < 500
    except Exception:
        return False


@pytest.mark.integration
def test_rest_get_and_delete_batch_sks_or_legacy():
    base_url = os.getenv("PROXIMADB_URL", "http://localhost:5678")
    if not server_available(base_url):
        pytest.skip("ProximaDB REST server not available; set PROXIMADB_URL and start server to run integration tests.")

    client = ProximaDBClient(url=base_url)
    coll = f"py_sdk_rest_{uuid.uuid4().hex[:8]}"
    try:
        # Create collection
        cfg = CollectionConfig(name=coll, dimension=4)
        client.create_collection(name=coll, config=cfg)

        # Insert 2 vectors
        vectors = np.array([[0.2, 0.1, 0.0, 0.3], [0.25, 0.05, 0.02, 0.29]], dtype=np.float32)
        ids = ["gid1", "gid2"]
        ins = client.insert_vectors(coll, vectors, ids, upsert=True)
        assert ins.success >= 1

        # Get single vector (SKS-first or legacy fallback)
        got = client.get_vector(coll, "gid1", include_vector=False, include_metadata=True)
        assert isinstance(got, dict)
        assert got.get("id") == "gid1" or got.get("vector_id") == "gid1" or got.get("entity_id") == "gid1"

        # Delete batch
        delres = client.delete_vectors(coll, ids)
        assert delres.success is True
        assert delres.deleted_count >= 1

    finally:
        try:
            client.delete_collection(coll)
        except Exception:
            pass

