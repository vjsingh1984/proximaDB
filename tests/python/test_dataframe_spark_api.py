import proximadb_embedded as pdb
import numpy as np
import pytest
import os
import shutil

@pytest.fixture
def db():
    path = "./test_df_db"
    if os.path.exists(path):
        shutil.rmtree(path)
    
    db = pdb.open(path)
    
    # Create a collection with some data
    db.create_collection("test_users", dimension=4)
    
    # Insert some data
    vectors = np.random.rand(5, 4).astype(np.float32)
    ids = ["u1", "u2", "u3", "u4", "u5"]
    props = [
        {"name": "Alice", "age": 25, "active": True},
        {"name": "Bob", "age": 30, "active": False},
        {"name": "Charlie", "age": 35, "active": True},
        {"name": "David", "age": 40, "active": False},
        {"name": "Eve", "age": 45, "active": True},
    ]
    db.insert("test_users", ids=ids, vectors=vectors, metadata=props)
    
    yield db
    
    if os.path.exists(path):
        shutil.rmtree(path)

def test_dataframe_spark_api(db):
    session = db.dataframe_session()
    session.refresh_tables()
    
    # Test col() and select()
    df = session.table("test_users")
    df_select = df.select(pdb.col("name"), pdb.col("age"))
    
    # Test filter()
    df_filtered = df_select.filter(pdb.col("age") > 30)
    
    # Test sort()
    df_sorted = df_filtered.sort(pdb.col("age").sort(ascending=False, nulls_first=False))
    
    # Test limit()
    df_limited = df_sorted.limit(2)
    
    # Collect results
    results = df_limited.collect()
    
    assert len(results) <= 2
    for r in results:
        assert r["age"] > 30
        assert "name" in r
        assert "age" in r
        assert "active" not in r # because we selected name and age only

def test_dataframe_aggregates(db):
    session = db.dataframe_session()
    session.refresh_tables()
    
    df = session.table("test_users")
    
    # Test count, sum, avg, min, max
    # Note: in DataFusion, aggregate requires group expressions (can be empty)
    df_agg = df.aggregate([], [
        pdb.count(pdb.col("name")).alias("total_test_users"),
        pdb.avg(pdb.col("age")).alias("avg_age"),
        pdb.max(pdb.col("age")).alias("max_age")
    ])
    
    results = df_agg.collect()
    assert len(results) == 1
    assert results[0]["total_test_users"] == 5
    assert results[0]["avg_age"] == 35.0
    assert results[0]["max_age"] == 45

def test_dataframe_with_column(db):
    session = db.dataframe_session()
    session.refresh_tables()
    
    df = session.table("test_users")
    
    # Test with_column (age + 10)
    df_new = df.with_column("age_plus_10", pdb.col("age") + pdb.lit(10))
    
    results = df_new.collect()
    for r in results:
        assert r["age_plus_10"] == r["age"] + 10

if __name__ == "__main__":
    # Manual run if needed
    import sys
    pytest.main(sys.argv)
