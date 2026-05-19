"""Batching helper record API tests."""

from proximadb_sdk.batching_unified import batch_insert_records, batch_insert_vectors


class RecordingClient:
    def __init__(self):
        self.record_batches = []
        self.vector_batches = []

    def insert_records(self, collection_id, records):
        self.record_batches.append((collection_id, records))
        return {"success": len(records)}

    def insert_vectors(self, collection_id, records):
        self.vector_batches.append((collection_id, records))
        return {"success": len(records)}


def test_batch_insert_records_uses_record_method():
    client = RecordingClient()

    results = batch_insert_records(
        client,
        "items",
        [{"id": "a", "vector": [1.0]}, {"id": "b", "vector": [2.0]}],
        batch_size=1,
    )

    assert results == [{"success": 1}, {"success": 1}]
    assert [batch[1][0]["id"] for batch in client.record_batches] == ["a", "b"]
    assert client.vector_batches == []


def test_batch_insert_vectors_delegates_to_record_helper():
    client = RecordingClient()

    batch_insert_vectors(client, "items", [{"id": "a", "vector": [1.0]}])

    assert client.record_batches == [("items", [{"id": "a", "vector": [1.0]}])]
    assert client.vector_batches == []
