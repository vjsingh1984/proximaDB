"""Insert builder record-native behavior tests."""

import numpy as np

from proximadb_sdk.builders.insert import InsertBuilder, batch_insert, from_numpy
from proximadb_sdk.models import VectorRecord
from proximadb_sdk.models_v2 import ProximaRecord, TypedValue


def test_insert_builder_builds_record_dicts():
    records, options = (
        InsertBuilder()
        .add_record(
            ProximaRecord(
                id="r1",
                vector=[1.0, 2.0],
                typed_fields={"price": TypedValue.float_(9.99)},
                flexible_fields={"kind": "note"},
            )
        )
        .add_metadata_field("tenant", "acme")
        .build()
    )

    assert options["batch_size"] == 1000
    assert records[0]["id"] == "r1"
    assert records[0]["vector"] == [1.0, 2.0]
    assert records[0]["props"] == {"kind": "note", "tenant": "acme"}
    assert records[0]["typed_fields"]["price"]["value"] == 9.99


def test_insert_builder_vector_methods_are_compat_aliases():
    builder = InsertBuilder().add_vector("v1", [3.0, 4.0], {"kind": "legacy"})

    records = builder.build_records()
    vectors = builder.build_vectors()

    assert records == [{"id": "v1", "vector": [3.0, 4.0], "props": {"kind": "legacy"}}]
    assert isinstance(vectors[0], VectorRecord)
    assert vectors[0].metadata == {"kind": "legacy"}


def test_insert_builder_from_numpy_returns_records():
    records, options = from_numpy(
        ["a", "b"],
        np.array([[1.0, 0.0], [0.0, 1.0]], dtype=np.float32),
        [{"label": "A"}, {"label": "B"}],
        batch_size=1,
    )

    assert options["batch_size"] == 1
    assert records == [
        {"id": "a", "vector": [1.0, 0.0], "props": {"label": "A"}},
        {"id": "b", "vector": [0.0, 1.0], "props": {"label": "B"}},
    ]


def test_batch_insert_uses_records():
    records, _options = batch_insert([{"id": "r1", "vector": [1.0], "props": {}}])

    assert records == [{"id": "r1", "vector": [1.0], "props": {}}]
