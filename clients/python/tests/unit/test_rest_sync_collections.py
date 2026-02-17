from proximadb_sdk.protocols.rest_sync import ProximaDBClient


class FakeResponse:
    def __init__(self, data):
        self._data = data

    def json(self):
        return self._data


def test_create_collection_monkeypatched_request(monkeypatch):
    client = ProximaDBClient(url="http://testserver")
    captured = {}

    def fake_make_request(method, endpoint, **kwargs):
        captured["method"] = method
        captured["endpoint"] = endpoint
        captured["json"] = kwargs.get("json", {})
        # Simulate server response for collection creation (unified API format)
        body = {
            "collection": {
                "id": "col-123",
                "config": {
                    "name": captured["json"].get("config", {}).get("name", "documents"),
                    "dimension": captured["json"]
                    .get("config", {})
                    .get("dimension", 128),
                    "distance_metric": captured["json"]
                    .get("config", {})
                    .get("distance_metric", "cosine"),
                },
                "vector_count": 0,
                "created_at": 1,
                "updated_at": 1,
            }
        }
        return FakeResponse(body)

    monkeypatch.setattr(client, "_make_request", fake_make_request)
    coll = client.create_collection("documents", dimension=128)
    assert coll.id == "col-123"
    assert coll.config.name == "documents"
    # Ensure a POST was made
    assert captured["method"] == "POST"


def test_get_collection_parses_response(monkeypatch):
    client = ProximaDBClient(url="http://testserver")

    def fake_make_request(method, endpoint, **kwargs):
        assert method == "GET"
        assert endpoint.startswith("/api/v1/collections/")
        return FakeResponse(
            {
                "id": "col-1",
                "name": "documents",
                "dimension": 768,
                "metric": "cosine",
                "vector_count": 10,
                "created_at": 1,
                "updated_at": 2,
            }
        )

    monkeypatch.setattr(client, "_make_request", fake_make_request)
    c = client.get_collection("documents")
    assert c.id == "col-1"
    assert c.config.dimension == 768
    assert c.stats.vector_count == 10


def test_list_collections_parses_list(monkeypatch):
    client = ProximaDBClient(url="http://testserver")

    def fake_make_request(method, endpoint, **kwargs):
        assert method == "GET" and endpoint == "/api/v1/collections"
        return FakeResponse(
            {
                "collections": [
                    {
                        "id": "c1",
                        "name": "collection_a",
                        "dimension": 128,
                        "metric": "cosine",
                        "vector_count": 0,
                    },
                    {
                        "id": "c2",
                        "name": "collection_b",
                        "dimension": 256,
                        "metric": "cosine",
                        "vector_count": 3,
                    },
                ],
                "total_count": 2,
            }
        )

    monkeypatch.setattr(client, "_make_request", fake_make_request)
    lst = client.list_collections()
    assert len(lst) == 2
    assert lst[1].id == "c2" and lst[1].config.dimension == 256


def test_delete_collection(monkeypatch):
    client = ProximaDBClient(url="http://testserver")

    def fake_make_request(method, endpoint, **kwargs):
        assert method == "DELETE"
        assert endpoint == "/api/v1/collections/documents"
        return FakeResponse({"success": True})

    monkeypatch.setattr(client, "_make_request", fake_make_request)
    ok = client.delete_collection("documents")
    assert ok is True
