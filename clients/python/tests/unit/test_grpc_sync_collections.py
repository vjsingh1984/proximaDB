import types
from proximadb.protocols.grpc_sync import ProximaDBSyncGrpcClient
from proximadb.models import DistanceMetricType, StorageEngineType
import proximadb.protocols.grpc_sync as grpc_mod


class FakeCollectionStub:
    def __init__(self, channel):
        pass

    def CreateCollection(self, req, timeout=None):
        return types.SimpleNamespace(id="col-xyz")

    def GetCollection(self, req, timeout=None):
        return types.SimpleNamespace(id="col-xyz", name="test_docs")

    def ListCollections(self, req, timeout=None):
        return types.SimpleNamespace(collections=[types.SimpleNamespace(id="c1"), types.SimpleNamespace(id="c2")])

    def DeleteCollection(self, req, timeout=None):
        return types.SimpleNamespace(success=True)


def test_grpc_sync_collection_ops(monkeypatch):
    # Patch collection stub and connection pool
    monkeypatch.setattr(grpc_mod, "v1_collection_pb2_grpc", types.SimpleNamespace(CollectionServiceStub=FakeCollectionStub), raising=False)
    # Patch message modules to simple namespaces to avoid import dependence
    monkeypatch.setattr(grpc_mod, "v1_collection_types_pb2", types.SimpleNamespace(CollectionConfig=object, ListCollectionsRequest=object, GetCollectionRequest=object, DeleteCollectionRequest=object), raising=False)

    from proximadb.protocols.connection_pools import GrpcConnectionPool, GrpcChannelContext

    class FakePool:
        def __init__(self, *a, **k):
            pass
    class FakeCtx:
        def __init__(self, pool):
            pass
        def __enter__(self):
            return None
        def __exit__(self, *a):
            return False

    monkeypatch.setattr("proximadb.protocols.connection_pools.GrpcConnectionPool", FakePool)
    monkeypatch.setattr("proximadb.protocols.connection_pools.GrpcChannelContext", FakeCtx)

    client = ProximaDBSyncGrpcClient("localhost:5679")
    client.create_collection_v1(
        name="test_docs",  # Must be >8 chars
        dimension=128,
        distance_metric=DistanceMetricType.COSINE,
        storage_engine=StorageEngineType.VIPER
    )
    client.get_collection_v1("test_docs")
    client.list_collections_v1()
    client.delete_collection_v1("test_docs")

