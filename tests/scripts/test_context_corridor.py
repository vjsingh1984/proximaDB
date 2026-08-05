from __future__ import annotations

import hashlib
import importlib.util
import json
import random
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "context_corridor", ROOT / "scripts/bench/context_corridor.py"
)
assert SPEC and SPEC.loader
CONTEXT = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = CONTEXT
SPEC.loader.exec_module(CONTEXT)


class FakeAdapter:
    system_id = "fake"

    def __init__(self, *, fail_prepare: bool = False) -> None:
        self.fail_prepare = fail_prepare
        self.prepared_dimension = None
        self.inserted = 0
        self.max_batch = 0
        self.search_calls = 0
        self.closed = False

    def prepare(self, dimension: int) -> None:
        if self.fail_prepare:
            raise RuntimeError("expected preparation failure")
        self.prepared_dimension = dimension

    def insert_batch(self, records: list[dict]) -> None:
        self.inserted += len(records)
        self.max_batch = max(self.max_batch, len(records))

    def finish_ingest(self) -> None:
        return None

    def search(self, record: dict, *, filtered: bool) -> tuple[list[str], float]:
        self.search_calls += 1
        return [record["id"]], 2.0 if filtered else 1.0

    def signals(self) -> dict:
        return {"fake": True}

    def environment(self) -> dict:
        return {"adapter": "fake"}

    def close(self, *, keep_data: bool) -> None:
        self.closed = True


class ContextCorridorTest(unittest.TestCase):
    def test_streamed_dataset_is_bounded_and_hash_compatible(self) -> None:
        adapter = FakeAdapter()
        manifest, ingest_seconds = CONTEXT.ingest_stream(
            adapter,
            record_count=11,
            dimension=4,
            batch_size=3,
            seed=7,
            query_indexes=[0, 7, 7, 10],
        )

        rng = random.Random(7)
        records = [CONTEXT.make_record(index, 4, rng) for index in range(11)]
        expected_hash = hashlib.sha256(
            json.dumps(records, sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest()

        self.assertEqual(manifest.sha256, expected_hash)
        self.assertEqual(
            [record["id"] for record in manifest.query_records],
            ["rec-0", "rec-7", "rec-7", "rec-10"],
        )
        self.assertEqual(adapter.prepared_dimension, 4)
        self.assertEqual(adapter.inserted, 11)
        self.assertEqual(adapter.max_batch, 3)
        self.assertGreater(ingest_seconds, 0)

    def test_queries_measure_filtered_and_unfiltered_target_recall(self) -> None:
        adapter = FakeAdapter()
        records = [
            {
                "id": f"rec-{index}",
                "vector": [float(index)],
                "props": {"partition": "p0"},
            }
            for index in range(4)
        ]
        accuracy, latencies = CONTEXT.run_queries(adapter, records, warmup_count=1)

        self.assertEqual(accuracy["recall_at_10"], 1.0)
        self.assertEqual(accuracy["filtered_recall_at_10"], 1.0)
        self.assertEqual(latencies, [2.0, 2.0, 2.0])
        self.assertEqual(adapter.search_calls, 8)

    def test_report_has_the_complete_metric_contract(self) -> None:
        adapter = FakeAdapter()
        report = CONTEXT.build_report(
            adapter,
            root=ROOT,
            manifest=CONTEXT.DatasetManifest("abc", []),
            record_count=100,
            dimension=8,
            seed=9,
            query_count=3,
            warmup_count=1,
            ingest_seconds=2.0,
            accuracy={"recall_at_10": 1.0, "filtered_recall_at_10": 1.0},
            latencies=[1.0, 2.0, 3.0],
        )

        self.assertEqual(set(report["metrics"]), set(CONTEXT.REQUIRED_METRICS))
        self.assertEqual(report["metrics"]["ingest_records_per_second"], 50.0)
        self.assertFalse(report["publication_eligible"])
        self.assertIn("object_gets", report["metric_scope"]["unavailable"])

    def test_failed_run_is_written_and_cleanup_runs(self) -> None:
        adapter = FakeAdapter(fail_prepare=True)
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "failure.json"
            argv = [
                "context_corridor.py",
                "--records",
                "1",
                "--dimension",
                "1",
                "--queries",
                "1",
                "--warmup",
                "0",
                "--output",
                str(output),
            ]
            with (
                patch.object(CONTEXT, "make_adapter", return_value=adapter),
                patch.object(sys, "argv", argv),
            ):
                result = CONTEXT.main()

            payload = json.loads(output.read_text(encoding="utf-8"))
        self.assertEqual(result, 1)
        self.assertEqual(payload["status"], "failed")
        self.assertEqual(payload["error_type"], "RuntimeError")
        self.assertTrue(adapter.closed)

    def test_pgvector_identifiers_and_vector_literals_are_strict(self) -> None:
        with self.assertRaises(ValueError):
            CONTEXT.PgvectorAdapter("ignored", 'bad";drop table x')
        self.assertEqual(CONTEXT.vector_literal([1.0, -0.25]), "[1,-0.25]")

    def test_pgvector_environment_discloses_filtered_ann_settings(self) -> None:
        adapter = CONTEXT.PgvectorAdapter("ignored", "context_corridor")
        environment = adapter.environment()
        self.assertEqual(environment["hnsw_ef_search"], 40)
        self.assertEqual(environment["hnsw_iterative_scan"], "strict_order")

    def test_qdrant_collection_name_is_path_safe(self) -> None:
        with self.assertRaises(ValueError):
            CONTEXT.QdrantAdapter(
                "http://qdrant.example", "", 30.0, "bad/collection"
            )

    def test_qdrant_prepare_and_insert_contract(self) -> None:
        calls: list[tuple] = []

        def fake_request(*args):
            calls.append(args)
            if args[1:4] == ("GET", "/", None):
                return {"version": "1.18.2"}, 0.5
            return {"status": "ok"}, 0.5

        adapter = CONTEXT.QdrantAdapter(
            "http://qdrant.example", "private-key", 12.0, "context_corridor"
        )
        record = {
            "id": "rec-7",
            "vector": [0.25, -0.5],
            "props": {"partition": "p7", "ordinal": 7},
        }
        with patch.object(CONTEXT, "request", side_effect=fake_request):
            adapter.prepare(2)
            adapter.insert_batch([record])

        self.assertEqual(adapter.version, "1.18.2")
        self.assertEqual(
            calls[1][1:4],
            (
                "PUT",
                "/collections/context_corridor",
                {
                    "vectors": {"size": 2, "distance": "Cosine"},
                    "hnsw_config": {"m": 16, "ef_construct": 100},
                },
            ),
        )
        self.assertEqual(
            calls[2][1:4],
            (
                "PUT",
                "/collections/context_corridor/index?wait=true",
                {"field_name": "partition", "field_schema": "keyword"},
            ),
        )
        self.assertEqual(calls[3][2], "/collections/context_corridor/points?wait=true")
        self.assertEqual(
            calls[3][3]["points"],
            [
                {
                    "id": 7,
                    "vector": [0.25, -0.5],
                    "payload": {
                        "record_id": "rec-7",
                        "partition": "p7",
                        "ordinal": 7,
                    },
                }
            ],
        )
        self.assertEqual(calls[0][5], {"api-key": "private-key"})

    def test_qdrant_filtered_query_contract_and_usage_signals(self) -> None:
        calls: list[tuple] = []

        def fake_request(*args):
            calls.append(args)
            return (
                {
                    "usage": {
                        "hardware": {"cpu": 2, "payload_index_io_read": 1}
                    },
                    "result": {
                        "points": [
                            {"id": 7, "payload": {"record_id": "rec-7"}}
                        ]
                    },
                },
                1.25,
            )

        adapter = CONTEXT.QdrantAdapter(
            "http://qdrant.example", "", 12.0, "context_corridor"
        )
        record = {
            "id": "rec-7",
            "vector": [0.25, -0.5],
            "props": {"partition": "p7", "ordinal": 7},
        }
        with patch.object(CONTEXT, "request", side_effect=fake_request):
            found, latency = adapter.search(record, filtered=True)

        self.assertEqual(found, ["rec-7"])
        self.assertEqual(latency, 1.25)
        self.assertEqual(calls[0][2], "/collections/context_corridor/points/query")
        query = calls[0][3]
        self.assertEqual(query["params"], {"hnsw_ef": 40, "exact": False})
        self.assertEqual(query["with_payload"], ["record_id"])
        self.assertEqual(
            query["filter"],
            {"must": [{"key": "partition", "match": {"value": "p7"}}]},
        )
        self.assertEqual(
            adapter.query_usage, {"cpu": 2.0, "payload_index_io_read": 1.0}
        )

    def test_qdrant_ingest_waits_for_optimizer_readiness(self) -> None:
        responses = [
            {
                "result": {
                    "status": "yellow",
                    "update_queue": {"length": 3},
                }
            },
            {
                "result": {
                    "status": "green",
                    "update_queue": {"length": 0},
                }
            },
        ]
        adapter = CONTEXT.QdrantAdapter(
            "http://qdrant.example", "", 12.0, "context_corridor"
        )
        with (
            patch.object(
                CONTEXT,
                "request",
                side_effect=[(payload, 0.5) for payload in responses],
            ) as mocked_request,
            patch.object(CONTEXT.time, "sleep") as mocked_sleep,
        ):
            adapter.finish_ingest()

        self.assertEqual(mocked_request.call_count, 2)
        mocked_sleep.assert_called_once_with(0.25)
        self.assertGreaterEqual(adapter.readiness_wait_seconds, 0.0)

    def test_qdrant_environment_does_not_disclose_api_key(self) -> None:
        adapter = CONTEXT.QdrantAdapter(
            "http://qdrant.example", "private-key", 12.0, "context_corridor"
        )
        self.assertNotIn("private-key", json.dumps(adapter.environment()))
        self.assertEqual(adapter.environment()["hnsw_ef_search"], 40)

    def test_failure_text_does_not_persist_a_dsn_or_keyword_password(self) -> None:
        dsn = "postgresql://alice:secret@db.example/test"
        api_key = "qdrant-private-key"
        error = RuntimeError(
            f"could not use {dsn}; password=second-secret; api-key={api_key}"
        )
        sanitized = CONTEXT.sanitized_error(error, dsn, api_key)
        self.assertNotIn("secret", sanitized)
        self.assertNotIn(api_key, sanitized)
        self.assertIn("<redacted>", sanitized)


if __name__ == "__main__":
    unittest.main()
