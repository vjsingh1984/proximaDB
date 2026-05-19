"""ProximaRecord normalization tests for embedded Python."""

from decimal import Decimal

import numpy as np
import pytest

from proximadb_embedded import (
    ProximaRecord,
    insert_proxima_records,
    insert_records,
    proxima_value,
    upsert_records,
)
from proximadb_embedded.records import (
    normalize_document,
    normalize_graph_node,
    normalize_observability_event,
    normalize_record,
    normalize_records,
)


class RecordingDb:
    def __init__(self):
        self.insert_calls = []
        self.insert_numpy_calls = []
        self.upsert_calls = []

    def insert(self, collection, ids, vectors, metadata):
        self.insert_calls.append((collection, ids, vectors, metadata))
        return len(ids)

    def insert_numpy(self, collection, ids, vectors, metadata):
        self.insert_numpy_calls.append((collection, ids, vectors, metadata))
        return len(ids)

    def upsert(self, collection, ids, vectors, metadata):
        self.upsert_calls.append((collection, ids, vectors, metadata))
        return len(ids), 0


class NativeRecordingDb(RecordingDb):
    def __init__(self):
        super().__init__()
        self.native_insert_calls = []
        self.native_batch_insert_calls = []

    def _insert_proxima_records_native(self, collection, records):
        self.native_insert_calls.append((collection, records))
        return len(records)

    def _insert_proxima_record_batch_native(self, collection, ids, vectors, props):
        self.native_batch_insert_calls.append((collection, ids, vectors, props))
        return len(ids)


def test_mapping_normalizes_to_v2_record_shape():
    record = normalize_record(
        {
            "id": "product-1",
            "vector": np.array([0.1, 0.2], dtype=np.float32),
            "props": {"price": Decimal("19.99"), "payload": {"sku": "A1"}},
            "description": "lightweight shoe",
        },
        text_columns=["description"],
    )

    assert record["id"] == "product-1"
    assert record["vector"] == pytest.approx([0.1, 0.2])
    assert record["props"]["price"] == {"type": "decimal", "value": "19.99"}
    assert record["props"]["payload"] == {"type": "jsonb", "value": {"sku": "A1"}}
    assert record["text_fields"] == [
        {
            "name": "description",
            "content": "lightweight shoe",
            "storage_hint": "adaptive",
        }
    ]


def test_legacy_metadata_lowers_to_canonical_props():
    record = normalize_record(
        {
            "id": "legacy-1",
            "vector": [1, 2],
            "metadata": {"tenant": "acme"},
            "flexible_fields": {"status": "active"},
        }
    )

    assert record["props"] == {"tenant": "acme", "status": "active"}
    assert "metadata" not in record["props"]
    assert "flexible_fields" not in record["props"]


def test_proxima_record_and_value_are_primary_embedded_api_shape():
    record = normalize_record(
        ProximaRecord(
            id="typed-1",
            vector=[0.1, 0.2],
            props={
                "price": proxima_value("decimal", "19.99"),
                "payload": {"sku": "A1"},
            },
            text_fields=[{"name": "body", "content": "hello"}],
            source="inline",
            schema_id="catalog.schema.v1",
        )
    )

    assert record["id"] == "typed-1"
    assert record["props"]["price"] == {"type": "decimal", "value": "19.99"}
    assert record["props"]["payload"] == {"type": "jsonb", "value": {"sku": "A1"}}
    assert record["text_fields"] == [{"name": "body", "content": "hello"}]
    assert record["source"] == "inline"
    assert record["schema_id"] == "catalog.schema.v1"


def test_numpy_matrix_normalizes_with_ids_and_props():
    records = normalize_records(
        np.array([[1.0, 2.0], [3.0, 4.0]], dtype=np.float32),
        ids=["a", "b"],
        props=[{"tenant": "acme"}, {"tenant": "beta"}],
    )

    assert [record["id"] for record in records] == ["a", "b"]
    assert records[0]["vector"] == [1.0, 2.0]
    assert records[1]["props"]["tenant"] == "beta"


def test_pandas_dataframe_normalizes_when_available():
    pd = pytest.importorskip("pandas")

    frame = pd.DataFrame(
        [
            {"id": "r1", "embedding": [0.1, 0.2], "score": 7, "payload": {"k": "v"}},
            {"id": "r2", "embedding": [0.3, 0.4], "score": 8, "payload": {"k": "w"}},
        ]
    )

    records = normalize_records(
        frame,
        vector_field="embedding",
        typed_columns={"score": "int64"},
    )

    assert records[0]["id"] == "r1"
    assert records[0]["props"]["score"] == {"type": "int64", "value": 7}
    assert records[1]["props"]["payload"] == {"type": "jsonb", "value": {"k": "w"}}


def test_pyarrow_table_normalizes_when_available():
    pa = pytest.importorskip("pyarrow")

    table = pa.table(
        {
            "id": ["r1"],
            "vector": [[0.1, 0.2]],
            "category": ["doc"],
        }
    )

    records = normalize_records(table)

    assert records[0]["id"] == "r1"
    assert records[0]["vector"] == pytest.approx([0.1, 0.2])
    assert records[0]["props"] == {"category": "doc"}


def test_document_graph_and_observability_helpers_emit_canonical_props():
    document = normalize_document(
        "doc-1",
        {"title": "Spec", "body": "Canonical records"},
        [0.1, 0.2],
        text_columns=["body"],
    )
    graph_node = normalize_graph_node("node-1", ["Person"], {"name": "Ada"}, [0.3, 0.4])
    log = normalize_observability_event(
        "log-1",
        {"severity": "INFO", "service": "api"},
        [0.5, 0.6],
        event_type="log",
    )

    assert document.props["_modality"] == "document"
    assert document.text_fields == [
        {
            "name": "body",
            "content": "Canonical records",
            "storage_hint": "adaptive",
        }
    ]
    assert graph_node.props["_modality"] == "graph_node"
    assert graph_node.props["labels"] == ["Person"]
    assert log.props["_modality"] == "observability"
    assert log.props["event_type"] == "log"


def test_insert_records_routes_modern_shape_to_embedded_batch_boundary():
    db = RecordingDb()

    count = insert_records(
        db,
        "records",
        [
            {
                "id": "r1",
                "vector": [0.1, 0.2],
                "props": {"kind": "note", "payload": {"x": 1}},
            }
        ],
    )

    assert count == 1
    collection, ids, vectors, metadata = db.insert_calls[0]
    assert collection == "records"
    assert ids == ["r1"]
    assert vectors == [[0.1, 0.2]]
    assert metadata == [{"kind": "note", "payload": {"type": "jsonb", "value": {"x": 1}}}]


def test_insert_proxima_records_preserves_record_extras_at_current_native_boundary():
    db = RecordingDb()

    count = insert_proxima_records(
        db,
        "records",
        ProximaRecord(
            id="r1",
            vector=[0.1, 0.2],
            props={"kind": "note"},
            text_fields=[{"name": "body", "content": "hello"}],
            source="inline",
            schema_id="schema-v1",
        ),
    )

    assert count == 1
    collection, ids, vectors, metadata = db.insert_calls[0]
    assert collection == "records"
    assert ids == ["r1"]
    assert vectors == [[0.1, 0.2]]
    assert metadata == [
        {
            "kind": "note",
            "_text_fields": [{"name": "body", "content": "hello"}],
            "_source": "inline",
            "_schema_id": "schema-v1",
        }
    ]


def test_insert_proxima_records_uses_numpy_transport_for_dense_record_batch():
    db = RecordingDb()
    vectors = np.array([[0.1, 0.2], [0.3, 0.4]], dtype=np.float32)

    count = insert_proxima_records(
        db,
        "records",
        [
            ProximaRecord(id="r1", vector=vectors[0], props={"kind": "note"}),
            ProximaRecord(id="r2", vector=vectors[1], props={"kind": "note"}),
        ],
    )

    assert count == 2
    assert db.insert_calls == []
    collection, ids, matrix, metadata = db.insert_numpy_calls[0]
    assert collection == "records"
    assert ids == ["r1", "r2"]
    assert matrix.dtype == np.float32
    assert matrix.shape == (2, 2)
    np.testing.assert_allclose(matrix, vectors)
    assert metadata == [{"kind": "note"}, {"kind": "note"}]


def test_insert_proxima_records_prefers_native_record_boundary_when_available():
    db = NativeRecordingDb()
    records = [
        normalize_graph_node("node-1", ["Person"], {"name": "Ada"}, [0.1, 0.2]),
        normalize_observability_event(
            "log-1",
            {"severity": "INFO"},
            [0.3, 0.4],
            event_type="log",
        ),
    ]

    count = insert_proxima_records(db, "records", records)

    assert count == 2
    assert db.insert_calls == []
    assert db.insert_numpy_calls == []
    collection, native_records = db.native_insert_calls[0]
    assert collection == "records"
    assert native_records == records


def test_insert_proxima_records_prefers_columnar_native_boundary_for_dense_records():
    db = NativeRecordingDb()
    vectors = np.array([[0.1, 0.2], [0.3, 0.4]], dtype=np.float32)
    records = [
        ProximaRecord(id="r1", vector=vectors[0], props={"kind": "note"}),
        ProximaRecord(id="r2", vector=vectors[1], props={"kind": "note"}),
    ]

    count = insert_proxima_records(db, "records", records)

    assert count == 2
    assert db.insert_calls == []
    assert db.insert_numpy_calls == []
    assert db.native_insert_calls == []
    collection, ids, matrix, props = db.native_batch_insert_calls[0]
    assert collection == "records"
    assert ids == ["r1", "r2"]
    np.testing.assert_allclose(matrix, vectors)
    assert props == [{"kind": "note"}, {"kind": "note"}]


def test_upsert_records_accepts_numpy_matrix_with_props():
    db = RecordingDb()

    inserted, updated = upsert_records(
        db,
        "records",
        np.array([[1.0, 2.0]], dtype=np.float32),
        ids=["r1"],
        props=[{"kind": "metric"}],
    )

    assert (inserted, updated) == (1, 0)
    assert db.upsert_calls[0] == (
        "records",
        ["r1"],
        [[1.0, 2.0]],
        [{"kind": "metric"}],
    )
