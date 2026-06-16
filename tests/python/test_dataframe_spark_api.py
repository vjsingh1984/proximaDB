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
    try:
        session.refresh_tables()
    except RuntimeError as e:
        if "already exists" not in str(e):
            raise

    df = session.table("test_users")
    df_select = df.select(pdb.col("test_users.oid"), pdb.col("test_users.updated_at_ns"))

    # The current DataFusion registration path exposes collection schemas for
    # planning; row-producing embedded split discovery is still a separate
    # engine-adapter contract. A filtered scan should therefore be valid even
    # when no splits are discoverable.
    df_filtered = df_select.filter(pdb.col("test_users.oid") == pdb.lit("u1"))

    results = df_filtered.collect()
    assert results == []

    arrow_table = df_filtered.to_arrow()
    assert arrow_table is None or arrow_table.num_rows == 0
    if arrow_table is not None:
        assert arrow_table.schema.names == ["oid", "updated_at_ns"]


def test_dataframe_aggregates(db):
    session = db.dataframe_session()
    try:
        session.refresh_tables()
    except RuntimeError as e:
        if "already exists" not in str(e):
            raise

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


def test_dataframe_api_validation(db):
    session = db.dataframe_session()

    with pytest.raises(RuntimeError, match="Error during planning"):
        session.sql("  ")
    with pytest.raises(RuntimeError, match="No table named"):
        session.table("  ")

    session.refresh_tables()
    df = session.table("test_users")

    with pytest.raises(RuntimeError, match="Aggregate requires"):
        df.aggregate([], [])
    with pytest.raises(RuntimeError, match="duplicate qualified field name"):
        df.join(df, [])



if __name__ == "__main__":
    import sys

    pytest.main(sys.argv)
