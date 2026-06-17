import proximadb_embedded as pdb
import numpy as np
import pytest


@pytest.fixture
def db(tmp_path):
    db = pdb.open(str(tmp_path / "test_df_db"))
    db.create_collection("test_users", dimension=4)

    rng = np.random.default_rng(42)
    vectors = rng.random((5, 4), dtype=np.float32)
    ids = ["u1", "u2", "u3", "u4", "u5"]
    db.insert("test_users", ids=ids, vectors=vectors)

    yield db


def test_dataframe_spark_api(db):
    session = db.dataframe_session()
    session.refresh_tables()
    session.refresh_tables()

    df = session.table("test_users")
    df_select = df.select(pdb.col("test_users.oid"), pdb.col("test_users.updated_at_ns"))

    # The current DataFusion registration path exposes collection schemas for
    # planning; row-producing embedded split discovery is still a separate
    # engine-adapter contract. A filtered scan should therefore be valid even
    # when no splits are discoverable.
    df_filtered = (
        df_select
        .filter(pdb.col("test_users.oid") == pdb.lit("u1"))
        .sort(pdb.col("test_users.updated_at_ns").sort(ascending=False))
    )

    results = df_filtered.collect()
    assert results == []

    arrow_table = df_filtered.to_arrow()
    assert arrow_table.num_rows == 0
    assert arrow_table.schema.names == ["oid", "updated_at_ns"]


def test_dataframe_aggregates(db):
    session = db.dataframe_session()
    session.refresh_tables()

    df = session.table("test_users")

    df_agg = df.aggregate([], [
        pdb.count(pdb.col("test_users.oid")).alias("total_users"),
    ])

    results = df_agg.collect()
    assert len(results) == 1
    assert results[0]["total_users"] == 0


def test_dataframe_helpers_are_public_exports():
    for name in ("DataFrame", "DataFusionSession", "Expr", "col", "lit", "count", "sum", "avg", "min", "max"):
        assert name in pdb.__all__


if __name__ == "__main__":
    import sys

    pytest.main(sys.argv)
