import os
import uuid
import numpy as np
import pytest

from proximadb.protocols.grpc_sync import ProximaDBSyncGrpcClient
from proximadb import proximadb_pb2 as pb2


def grpc_available(addr: str) -> bool:
    # Best-effort: attempt Health RPC if possible using a short-lived client
    try:
        client = ProximaDBSyncGrpcClient(addr, timeout=2.0, pool_size=1)
        # Health() returns a dict from our wrapper
        _ = client.health_check()
        client.close()
        return True
    except Exception:
        return False


@pytest.mark.integration
def test_grpc_end_to_end_basic():
    grpc_addr = os.getenv("PROXIMADB_GRPC", "localhost:5679")
    if not grpc_available(grpc_addr):
        pytest.skip("ProximaDB gRPC server not available; set PROXIMADB_GRPC and start server to run integration tests.")

    client = ProximaDBSyncGrpcClient(grpc_addr, timeout=10.0)
    coll = f"py_sdk_grpc_{uuid.uuid4().hex[:8]}"
    try:
        # Create collection with HNSW primary
        ic = pb2.IndexConfig(index_name=f"{coll}_primary", algorithm=pb2.IndexingAlgorithm.HNSW, is_primary=True)
        qcfg = pb2.QuantizationConfig(enabled=False)
        client.create_collection(
            name=coll,
            dimension=4,
            distance_metric=pb2.DistanceMetric.COSINE,
            storage_engine=pb2.StorageEngine.VIPER,
            index_configs=[ic],
            quantization_config=qcfg,
        )

        # Insert vectors via gRPC wrapper (VectorBatch)
        vectors = [
            {"id": "g1", "vector": [0.1, 0.2, 0.3, 0.4]},
            {"id": "g2", "vector": [0.11, 0.21, 0.29, 0.41]},
        ]
        ins = client.insert_vectors(collection_id=coll, vectors=vectors)
        assert ins.success is True

        # Search
        results = client.search_vectors(collection_id=coll, query_vector=[0.1, 0.2, 0.3, 0.4], top_k=2)
        assert isinstance(results, list)
        assert len(results) >= 1
        assert results[0].id

        # Get single vector
        got = client.get_vector(collection_id=coll, vector_id="g1", include_vector=False, include_metadata=True)
        assert isinstance(got, dict)
        assert got.get("id") == "g1"

        # Delete one vector (wrapper uses DeleteVector RPC)
        delres = client.delete_vector(collection_id=coll, vector_id="g1")
        assert delres.get("success", True)

    finally:
        try:
            client.delete_collection(coll)
        except Exception:
            pass
        client.close()

