from __future__ import annotations

import base64
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
    def test_streamed_dataset_builds_hash_and_ground_truth(self) -> None:
        adapter = FakeAdapter()
        queries = [CONTEXT.make_query(i, 4, random.Random(100 + i)) for i in range(2)]
        manifest, ingest_seconds = CONTEXT.ingest_stream(
            adapter,
            record_count=11,
            dimension=4,
            batch_size=3,
            seed=7,
            queries=queries,
        )

        rng = random.Random(7)
        records = [CONTEXT.make_record(index, 4, rng) for index in range(11)]
        expected_hash = hashlib.sha256(
            json.dumps(records, sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest()

        self.assertEqual(manifest.sha256, expected_hash)
        self.assertEqual(len(manifest.queries), 2)
        # Ground-truth ids are real corpus ids, and the filtered truth stays
        # within the query's own partition.
        corpus_by_id = {record["id"]: record for record in records}
        for ground_truth in manifest.queries:
            self.assertTrue(
                ground_truth.unfiltered_truth.issubset(corpus_by_id.keys())
            )
            partition = ground_truth.query["props"]["partition"]
            for record_id in ground_truth.filtered_truth:
                self.assertEqual(
                    corpus_by_id[record_id]["props"]["partition"], partition
                )
        self.assertEqual(adapter.prepared_dimension, 4)
        self.assertEqual(adapter.inserted, 11)
        self.assertEqual(adapter.max_batch, 3)
        self.assertGreater(ingest_seconds, 0)

    def test_queries_measure_recall_against_ground_truth(self) -> None:
        class StubAdapter:
            system_id = "stub"

            def search(self, record, *, filtered):
                # Unfiltered returns 2 of the 4 true neighbours (0.5 recall);
                # filtered returns exactly the true filtered set (1.0 recall).
                if filtered:
                    return ["a", "b"], 2.0
                return ["a", "b", "x", "y"], 1.0

        queries = [
            CONTEXT.QueryGroundTruth(
                query={
                    "id": "query-0",
                    "vector": [0.0],
                    "props": {"partition": "p0"},
                },
                unfiltered_truth=frozenset({"a", "b", "c", "d"}),
                filtered_truth=frozenset({"a", "b"}),
            )
        ]
        accuracy, latencies = CONTEXT.run_queries(
            StubAdapter(), queries, warmup_count=0
        )

        self.assertEqual(accuracy["recall_at_10"], 0.5)
        self.assertEqual(accuracy["filtered_recall_at_10"], 1.0)
        self.assertEqual(latencies, [2.0])

    def test_ground_truth_accumulator_computes_exact_topk(self) -> None:
        queries = [{"id": "q", "vector": [1.0, 0.0], "props": {"partition": "p0"}}]
        accumulator = CONTEXT.GroundTruthAccumulator(queries, top_k=2)
        records = [
            {"id": "a", "vector": [1.0, 0.0], "props": {"partition": "p0"}},
            {"id": "b", "vector": [0.9, 0.1], "props": {"partition": "p1"}},
            {"id": "c", "vector": [0.5, 0.5], "props": {"partition": "p0"}},
            {"id": "d", "vector": [-1.0, 0.0], "props": {"partition": "p0"}},
        ]
        for record in records:
            accumulator.observe(record)
        (unfiltered, filtered), = accumulator.finalize()

        # Top-2 by cosine overall = a (1.0), b (0.994); within p0 = a, c.
        self.assertEqual(unfiltered, frozenset({"a", "b"}))
        self.assertEqual(filtered, frozenset({"a", "c"}))

    def test_recall_at_k_measures_intersection(self) -> None:
        truth = frozenset({"a", "b", "c", "d"})
        self.assertEqual(CONTEXT.recall_at_k(["a", "b", "c", "d"], truth), 1.0)
        self.assertEqual(CONTEXT.recall_at_k(["a", "b"], truth), 0.5)
        self.assertEqual(CONTEXT.recall_at_k(["x", "y"], truth), 0.0)
        # Empty truth (e.g. an empty partition) is vacuously satisfied.
        self.assertEqual(CONTEXT.recall_at_k([], frozenset()), 1.0)

    def test_make_query_structure_and_partition_cycle(self) -> None:
        query = CONTEXT.make_query(3, 4, random.Random(7))
        self.assertEqual(query["id"], "query-3")
        self.assertEqual(query["props"]["partition"], "p3")
        self.assertEqual(len(query["vector"]), 4)
        self.assertEqual(
            CONTEXT.make_query(8, 4, random.Random(7))["props"]["partition"], "p0"
        )

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

    def test_milvus_collection_name_is_path_safe(self) -> None:
        with self.assertRaises(ValueError):
            CONTEXT.MilvusAdapter(
                "http://milvus.example",
                "private-token",
                30.0,
                "bad-collection",
                "3.0.0",
            )

    def test_milvus_prepare_and_insert_contract(self) -> None:
        calls: list[tuple] = []

        def fake_request(*args):
            calls.append(args)
            if args[2] == "/v2/vectordb/entities/insert":
                return {"code": 0, "data": {"insertCount": 1}}, 0.5
            return {"code": 0, "data": {}}, 0.5

        adapter = CONTEXT.MilvusAdapter(
            "http://milvus.example",
            "private-token",
            12.0,
            "context_corridor",
            "3.0.0",
        )
        record = {
            "id": "rec-7",
            "vector": [0.25, -0.5],
            "props": {"partition": "p7", "ordinal": 7},
        }
        with patch.object(CONTEXT, "request", side_effect=fake_request):
            adapter.prepare(2)
            adapter.insert_batch([record])

        self.assertEqual(calls[0][2], "/v2/vectordb/collections/create")
        create = calls[0][3]
        self.assertEqual(create["consistencyLevel"], "Strong")
        self.assertFalse(create["schema"]["enableDynamicField"])
        self.assertEqual(
            create["indexParams"],
            [
                {
                    "fieldName": "embedding",
                    "indexName": "embedding_hnsw_idx",
                    "metricType": "COSINE",
                    "params": {
                        "index_type": "HNSW",
                        "M": 16,
                        "efConstruction": 100,
                    },
                },
                {
                    "fieldName": "partition",
                    "indexName": "partition_bitmap_idx",
                    "params": {"index_type": "BITMAP"},
                },
            ],
        )
        self.assertEqual(
            calls[1][3]["data"],
            [
                {
                    "id": "rec-7",
                    "embedding": [0.25, -0.5],
                    "partition": "p7",
                    "ordinal": 7,
                }
            ],
        )
        self.assertEqual(calls[0][5]["Authorization"], "Bearer private-token")

    def test_milvus_filtered_search_contract_and_result_mapping(self) -> None:
        calls: list[tuple] = []

        def fake_request(*args):
            calls.append(args)
            return {"code": 0, "data": [{"id": "rec-7", "distance": 1.0}]}, 1.25

        adapter = CONTEXT.MilvusAdapter(
            "http://milvus.example",
            "private-token",
            12.0,
            "context_corridor",
            "3.0.0",
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
        self.assertEqual(calls[0][2], "/v2/vectordb/entities/search")
        search = calls[0][3]
        self.assertEqual(search["filter"], 'partition == "p7"')
        self.assertEqual(
            search["searchParams"],
            {"metricType": "COSINE", "params": {"ef": 40}},
        )

    def test_milvus_ingest_waits_for_load_and_both_indexes(self) -> None:
        adapter = CONTEXT.MilvusAdapter(
            "http://milvus.example",
            "private-token",
            12.0,
            "context_corridor",
            "3.0.0",
        )
        finished_index = {
            "data": [
                {
                    "indexState": "Finished",
                    "pendingRows": 0,
                }
            ]
        }
        responses = [
            ({"data": {}}, 0.5),
            (
                {
                    "data": {
                        "loadState": "LoadStateLoaded",
                        "loadProgress": 100,
                    }
                },
                0.5,
            ),
            (finished_index, 0.5),
            (finished_index, 0.5),
        ]
        with patch.object(adapter, "_request", side_effect=responses) as mocked:
            adapter.finish_ingest()

        self.assertEqual(mocked.call_count, 4)
        self.assertGreaterEqual(adapter.readiness_wait_seconds, 0.0)

    def test_milvus_environment_does_not_disclose_token(self) -> None:
        adapter = CONTEXT.MilvusAdapter(
            "http://milvus.example",
            "private-token",
            12.0,
            "context_corridor",
            "3.0.0",
        )
        environment = adapter.environment()
        self.assertNotIn("private-token", json.dumps(environment))
        self.assertEqual(environment["hnsw_ef_search"], 40)
        self.assertEqual(environment["server_version"], "3.0.0")

    def test_elasticsearch_index_name_is_path_safe(self) -> None:
        with self.assertRaises(ValueError):
            CONTEXT.ElasticsearchAdapter(
                "http://es.example", "", 30.0, "Bad-Index"
            )

    def test_elasticsearch_prepare_and_bulk_insert_contract(self) -> None:
        calls: list[tuple] = []

        def fake_request(*args, **kwargs):
            calls.append((args, kwargs))
            if args[1:4] == ("GET", "/", None):
                return {"version": {"number": "9.5.0"}}, 0.5
            if args[2].endswith("/_bulk"):
                return {"errors": False, "items": []}, 0.5
            return {"acknowledged": True}, 0.5

        adapter = CONTEXT.ElasticsearchAdapter(
            "http://es.example", "private-key", 12.0, "context_bench_1"
        )
        record = {
            "id": "rec-7",
            "vector": [0.25, -0.5],
            "props": {"partition": "p7", "ordinal": 7},
        }
        with patch.object(CONTEXT, "request", side_effect=fake_request):
            adapter.prepare(2)
            adapter.insert_batch([record])

        self.assertEqual(adapter.version, "9.5.0")
        put_args, _ = calls[1]
        self.assertEqual(put_args[1:3], ("PUT", "/context_bench_1"))
        embedding = put_args[3]["mappings"]["properties"]["embedding"]
        self.assertEqual(embedding["type"], "dense_vector")
        self.assertEqual(embedding["dims"], 2)
        self.assertEqual(embedding["similarity"], "cosine")
        self.assertEqual(
            embedding["index_options"],
            {"type": "hnsw", "m": 16, "ef_construction": 100},
        )
        self.assertEqual(
            put_args[3]["mappings"]["properties"]["partition"]["type"],
            "keyword",
        )
        bulk_args, bulk_kwargs = calls[2]
        self.assertEqual(bulk_args[2], "/context_bench_1/_bulk")
        self.assertEqual(bulk_kwargs["content_type"], "application/x-ndjson")
        lines = bulk_kwargs["raw_body"].decode().splitlines()
        self.assertEqual(json.loads(lines[0]), {"index": {"_id": "rec-7"}})
        self.assertEqual(
            json.loads(lines[1]),
            {
                "record_id": "rec-7",
                "embedding": [0.25, -0.5],
                "partition": "p7",
                "ordinal": 7,
            },
        )
        self.assertEqual(adapter.expected_docs, 1)
        self.assertEqual(bulk_args[5], {"Authorization": "ApiKey private-key"})

    def test_elasticsearch_bulk_errors_are_surfaced(self) -> None:
        adapter = CONTEXT.ElasticsearchAdapter(
            "http://es.example", "", 12.0, "context_bench_1"
        )
        error_response = {
            "errors": True,
            "items": [{"index": {"error": {"reason": "mapper_parsing"}}}],
        }
        record = {
            "id": "rec-7",
            "vector": [0.25, -0.5],
            "props": {"partition": "p7", "ordinal": 7},
        }
        with patch.object(CONTEXT, "request", return_value=(error_response, 0.5)):
            with self.assertRaises(RuntimeError):
                adapter.insert_batch([record])

    def test_elasticsearch_filtered_knn_contract_and_result_mapping(self) -> None:
        calls: list[tuple] = []

        def fake_request(*args, **kwargs):
            calls.append((args, kwargs))
            return (
                {"hits": {"hits": [{"_id": "rec-7", "fields": {"record_id": ["rec-7"]}}]}},
                1.25,
            )

        adapter = CONTEXT.ElasticsearchAdapter(
            "http://es.example", "", 12.0, "context_bench_1"
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
        args, _ = calls[0]
        self.assertEqual(args[2], "/context_bench_1/_search")
        knn = args[3]["knn"]
        self.assertEqual(knn["field"], "embedding")
        self.assertEqual(knn["k"], 10)
        self.assertEqual(knn["num_candidates"], 40)
        self.assertEqual(knn["filter"], {"term": {"partition": "p7"}})
        self.assertFalse(args[3]["_source"])

    def test_elasticsearch_ingest_waits_for_green_and_matching_count(self) -> None:
        adapter = CONTEXT.ElasticsearchAdapter(
            "http://es.example", "", 12.0, "context_bench_1"
        )
        adapter.expected_docs = 1
        responses = [
            ({}, 0.5),  # refresh
            ({"status": "yellow"}, 0.5),  # health (not green)
            ({"count": 0}, 0.5),  # count (mismatch)
            ({}, 0.5),  # refresh
            ({"status": "green"}, 0.5),  # health
            ({"count": 1}, 0.5),  # count (match)
        ]
        with (
            patch.object(CONTEXT, "request", side_effect=responses) as mocked,
            patch.object(CONTEXT.time, "sleep") as mocked_sleep,
        ):
            adapter.finish_ingest()

        self.assertEqual(mocked.call_count, 6)
        mocked_sleep.assert_called_once_with(0.25)
        self.assertGreaterEqual(adapter.readiness_wait_seconds, 0.0)

    def test_elasticsearch_environment_does_not_disclose_api_key(self) -> None:
        adapter = CONTEXT.ElasticsearchAdapter(
            "http://es.example", "private-key", 12.0, "context_bench_1"
        )
        environment = adapter.environment()
        self.assertNotIn("private-key", json.dumps(environment))
        self.assertEqual(environment["hnsw_num_candidates"], 40)
        self.assertEqual(environment["query_endpoint"], "/_search (knn)")

    def _surrealdb_adapter(self, password: str = ""):
        return CONTEXT.SurrealDBAdapter(
            "http://surreal.example",
            "root",
            password,
            "benchmark",
            "context_corridor",
            12.0,
            "context_bench_1",
            "3.2.0",
        )

    def test_surrealdb_table_name_is_path_safe(self) -> None:
        with self.assertRaises(ValueError):
            CONTEXT.SurrealDBAdapter(
                "http://surreal.example",
                "root",
                "",
                "benchmark",
                "context_corridor",
                30.0,
                "Bad-Table",
                "3.2.0",
            )

    def test_surrealdb_prepare_and_insert_contract(self) -> None:
        calls: list[tuple] = []

        def fake_request(*args, **kwargs):
            calls.append((args, kwargs))
            return [{"status": "OK", "result": None}], 0.5

        adapter = self._surrealdb_adapter("secret-pass")
        record = {
            "id": "rec-7",
            "vector": [0.25, -0.5],
            "props": {"partition": "p7", "ordinal": 7},
        }
        with patch.object(CONTEXT, "request", side_effect=fake_request):
            adapter.prepare(2)
            adapter.insert_batch([record])

        define_surql = calls[0][1]["raw_body"].decode()
        self.assertIn(
            "HNSW DIMENSION 2 DIST COSINE M 16 EFC 100", define_surql
        )
        self.assertIn("DEFINE INDEX partition_idx", define_surql)
        headers = calls[0][0][5]
        self.assertEqual(
            base64.b64decode(headers["Authorization"].split()[1]).decode(),
            "root:secret-pass",
        )
        self.assertEqual(headers["Surreal-NS"], "benchmark")
        self.assertEqual(headers["Surreal-DB"], "context_corridor")
        insert_surql = calls[1][1]["raw_body"].decode()
        self.assertIn("INSERT INTO context_bench_1", insert_surql)
        self.assertIn("record_id: 'rec-7'", insert_surql)
        self.assertIn("partition: 'p7'", insert_surql)
        self.assertIn("embedding: [0.25, -0.5]", insert_surql)
        self.assertIn("ordinal: 7", insert_surql)
        self.assertEqual(adapter.expected_docs, 1)

    def test_surrealdb_statement_error_is_surfaced(self) -> None:
        adapter = self._surrealdb_adapter()
        with patch.object(
            CONTEXT, "request", return_value=([{"status": "ERR", "result": "boom"}], 0.5)
        ):
            with self.assertRaises(RuntimeError):
                adapter.insert_batch(
                    [
                        {
                            "id": "rec-1",
                            "vector": [0.1],
                            "props": {"partition": "p1", "ordinal": 1},
                        }
                    ]
                )

    def test_surrealdb_filtered_knn_contract_and_result_mapping(self) -> None:
        calls: list[tuple] = []

        def fake_request(*args, **kwargs):
            calls.append((args, kwargs))
            return [{"status": "OK", "result": [{"record_id": "rec-7"}]}], 1.25

        adapter = self._surrealdb_adapter()
        record = {
            "id": "rec-7",
            "vector": [0.25, -0.5],
            "props": {"partition": "p7", "ordinal": 7},
        }
        with patch.object(CONTEXT, "request", side_effect=fake_request):
            found, latency = adapter.search(record, filtered=True)

        self.assertEqual(found, ["rec-7"])
        self.assertEqual(latency, 1.25)
        surql = calls[0][1]["raw_body"].decode()
        self.assertIn("SELECT record_id FROM context_bench_1", surql)
        self.assertIn("embedding <|10,40|> [0.25, -0.5]", surql)
        self.assertIn("AND partition = 'p7'", surql)

    def test_surrealdb_unfiltered_search_has_no_partition_filter(self) -> None:
        calls: list[tuple] = []

        def fake_request(*args, **kwargs):
            calls.append((args, kwargs))
            return [{"status": "OK", "result": [{"record_id": "rec-7"}]}], 1.0

        adapter = self._surrealdb_adapter()
        record = {
            "id": "rec-7",
            "vector": [0.25, -0.5],
            "props": {"partition": "p7", "ordinal": 7},
        }
        with patch.object(CONTEXT, "request", side_effect=fake_request):
            adapter.search(record, filtered=False)

        surql = calls[0][1]["raw_body"].decode()
        self.assertIn("embedding <|10,40|>", surql)
        self.assertNotIn("partition", surql)

    def test_surrealdb_ingest_waits_for_full_count(self) -> None:
        adapter = self._surrealdb_adapter()
        adapter.expected_docs = 2
        responses = [
            ([{"status": "OK", "result": [{"count": 1}]}], 0.5),  # mismatch
            ([{"status": "OK", "result": [{"count": 2}]}], 0.5),  # match
        ]
        with (
            patch.object(CONTEXT, "request", side_effect=responses) as mocked,
            patch.object(CONTEXT.time, "sleep") as mocked_sleep,
        ):
            adapter.finish_ingest()

        self.assertEqual(mocked.call_count, 2)
        mocked_sleep.assert_called_once_with(0.25)
        self.assertGreaterEqual(adapter.readiness_wait_seconds, 0.0)

    def test_surrealdb_environment_does_not_disclose_password(self) -> None:
        adapter = self._surrealdb_adapter("secret-pass")
        environment = adapter.environment()
        self.assertNotIn("secret-pass", json.dumps(environment))
        self.assertEqual(environment["hnsw_ef_search"], 40)
        self.assertEqual(environment["server_version"], "3.2.0")

    def test_failure_text_does_not_persist_a_dsn_or_keyword_password(self) -> None:
        dsn = "postgresql://alice:secret@db.example/test"
        api_key = "qdrant-private-key"
        milvus_token = "milvus-private-token"
        es_api_key = "elasticsearch-private-key"
        surreal_pass = "surrealdb-private-pass"
        error = RuntimeError(
            f"could not use {dsn}; password=second-secret; api-key={api_key}; "
            f"token={milvus_token}; es={es_api_key}; surreal={surreal_pass}"
        )
        sanitized = CONTEXT.sanitized_error(
            error, dsn, api_key, milvus_token, es_api_key, surreal_pass
        )
        self.assertNotIn("secret", sanitized)
        self.assertNotIn(api_key, sanitized)
        self.assertNotIn(milvus_token, sanitized)
        self.assertNotIn(es_api_key, sanitized)
        self.assertNotIn(surreal_pass, sanitized)
        self.assertIn("<redacted>", sanitized)


if __name__ == "__main__":
    unittest.main()
