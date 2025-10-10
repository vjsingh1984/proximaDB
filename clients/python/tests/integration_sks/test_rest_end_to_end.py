import os
import time
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
def test_rest_end_to_end_sks_or_legacy():
    base_url = os.getenv("PROXIMADB_URL", "http://localhost:5678")
    if not server_available(base_url):
        pytest.skip("ProximaDB REST server not available; set PROXIMADB_URL and start server to run integration tests.")

    client = ProximaDBClient(url=base_url)

    # Unique collection
    coll = f"py_sdk_it_{uuid.uuid4().hex[:8]}"
    try:
        # Create collection (dimension=4 for simplicity)
        cfg = CollectionConfig(name=coll, dimension=4)
        client.create_collection(name=coll, config=cfg)

        # Insert a few vectors
        vectors = np.array([[0.1, 0.2, 0.3, 0.4], [0.11, 0.21, 0.29, 0.41]], dtype=np.float32)
        ids = [f"vec_{i}" for i in range(len(vectors))]
        res = client.insert_vectors(coll, vectors, ids, upsert=True)
        assert res.success >= 1

        # Search (uses SKS if supported, otherwise legacy)
        q = [0.1, 0.2, 0.3, 0.4]
        results = client.search(coll, q, top_k=2, include_metadata=True)
        assert isinstance(results, list)
        assert len(results) >= 1
        assert results[0].id
        assert results[0].score >= 0.0

    finally:
        # Cleanup
        try:
            client.delete_collection(coll)
        except Exception:
            pass

