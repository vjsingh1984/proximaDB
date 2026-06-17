"""Notebook/procedural facade tests."""

import tempfile

import numpy as np

from proximadb_embedded import ProximaSession, col


class _NativePlanDb:
    def __init__(self):
        self.seen_plan = None

    def explain_notebook_plan(self, plan):
        self.seen_plan = plan
        return {
            "source_surface": "python_notebook",
            "execution_scope": "local_process",
            "workers": plan["session"]["workers"],
            "effective_parallelism": 1,
            "partition_plan": {"effective_read_partitions": 1},
            "compiled_sql": "SELECT * FROM native_seen",
        }


def test_session_builder_tracks_local_pseudo_distributed_config():
    with tempfile.TemporaryDirectory() as tmpdir:
        session = (
            ProximaSession.builder()
            .data_dir(tmpdir)
            .master("proxima-local[3]")
            .memory_limit("2g")
            .batch_size(512)
            .get_or_create()
        )

        assert session.worker_count == 3
        assert session.memory_limit == "2g"
        assert session.batch_size == 512


def test_frame_compiles_dataframe_style_sql_and_explain():
    with tempfile.TemporaryDirectory() as tmpdir:
        session = ProximaSession.builder().data_dir(tmpdir).get_or_create()

        frame = (
            session.table("events")
            .where((col("tenant_id") == "acme") & (col("score") >= 7))
            .select("id", "message")
            .limit(5)
        )

        assert (
            frame.compile_sql() == 'SELECT "id", "message" FROM events WHERE '
            '(("tenant_id" = \'acme\') AND ("score" >= 7)) LIMIT 5'
        )

        plan = frame.explain()
        assert plan["source_surface"] == "python_notebook"
        assert plan["execution_scope"] == "local_process"
        assert plan["authority_mode"] == "ProximaAuthoritative"
        assert plan["compute_route"] == "DataFusionLocal"
        assert plan["compiled_sql"] == frame.compile_sql()
        assert plan["partition_plan"]["requested_partitions"] == 1
        assert plan["partition_plan"]["effective_read_partitions"] == 1
        assert plan["effective_parallelism"] == 1


def test_frame_explain_prefers_native_plan_boundary_when_available():
    db = _NativePlanDb()
    session = ProximaSession(db, master="proxima-local[2]", batch_size=128)

    plan = session.table("items").where(col("kind") == "note").explain()

    assert plan["workers"] == 2
    assert plan["compiled_sql"] == "SELECT * FROM native_seen"
    assert db.seen_plan["source_surface"] == "python_notebook"
    assert db.seen_plan["session"]["batch_size"] == 128
    assert db.seen_plan["plan"]["source"] == "items"
    assert db.seen_plan["plan"]["operations"][0]["type"] == "where"


def test_frame_explain_reports_safe_parallelism_cap():
    with tempfile.TemporaryDirectory() as tmpdir:
        session = (
            ProximaSession.builder()
            .data_dir(tmpdir)
            .master("proxima-local[4]")
            .get_or_create()
        )

        plan = session.table("events").explain()

        assert plan["workers"] == 4
        assert plan["partition_plan"]["requested_partitions"] == 4
        assert plan["partition_plan"]["effective_read_partitions"] == 1
        assert plan["effective_parallelism"] == 1
        assert "rejected_parallelism_reason" in plan["partition_plan"]


def test_sql_frame_collects_through_embedded_rust_sql():
    with tempfile.TemporaryDirectory() as tmpdir:
        session = ProximaSession.builder().data_dir(tmpdir).get_or_create()

        session.db.execute_sql("""
            CREATE TABLE notebook_items (
                id TEXT NOT NULL,
                kind TEXT NOT NULL,
                score INT,
                embedding VECTOR(4),
                PRIMARY KEY (id)
            );
            """)
        session.db.execute_sql("""
            INSERT INTO notebook_items (id, kind, score, embedding)
            VALUES
                ('a', 'keep', 9, '[0.1, 0.2, 0.3, 0.4]'),
                ('b', 'drop', 3, '[0.4, 0.3, 0.2, 0.1]');
            """)

        rows = (
            session.table("xcatalog.tables")
            .where(col("table_name") == "notebook_items")
            .select("table_name", "storage_engine")
            .collect()
        )

        assert len(rows) == 1
        assert rows[0]["table_name"] == "notebook_items"
        assert rows[0]["schema_kind"] == "relational_vector"


def test_vector_search_collects_through_native_rust_search():
    with tempfile.TemporaryDirectory() as tmpdir:
        session = ProximaSession.builder().data_dir(tmpdir).get_or_create()
        session.db.create_collection("notebook_vectors", dimension=4)
        vectors = np.array(
            [
                [0.1, 0.2, 0.3, 0.4],
                [0.9, 0.8, 0.7, 0.6],
                [0.1, 0.2, 0.3, 0.5],
            ],
            dtype=np.float32,
        )
        session.db.insert(
            "notebook_vectors",
            ids=["a", "b", "c"],
            vectors=vectors,
            metadata=[
                {"kind": "near"},
                {"kind": "far"},
                {"kind": "near"},
            ],
        )

        rows = (
            session.table("notebook_vectors")
            .vector_search(column="embedding", query=vectors[0], top_k=3)
            .select("id", "score", "kind")
            .limit(2)
            .collect()
        )

        assert len(rows) <= 2
        assert rows[0]["id"] == "a" or rows[0]["score"] < 0.001
        assert set(rows[0]) == {"id", "score", "kind"}
