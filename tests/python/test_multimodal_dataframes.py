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
    db.flush()

    yield db


def test_dataframe_vector_search(db):
    session = db.dataframe_session()
    session.refresh_tables()

    # Create a vector search dataframe
    # Expected schema from vector_search UDTF is (id: Utf8, score: Float32)
    matches = session.vector_search("test_users", [0.1, 0.2, 0.3, 0.4], 2)
    
    # Collect vector search results directly
    results = matches.collect()
    assert len(results) == 2
    for r in results:
        assert "id" in r
        assert "score" in r
        assert isinstance(r["score"], float)

    # Now join the vector search results with the full relational table
    # This demonstrates the "Zero-ETL Multimodal HTAP" capability in Python
    table_df = session.table("test_users")
    
    # We join `matches.id` with `table_df.test_users.oid`
    # Note: To avoid column ambiguity, we can select/alias first, or just join 
    # if the column names don't conflict (they don't: 'id' vs 'test_users.oid').
    # We'll use a direct filter approach since PyDataFrame.join takes string column names
    # and currently requires them to match. For the prototype, we can use SQL for the join.
    
    # A cleaner test using SQL to join:
    joined_df = session.sql("""
        SELECT u.id, v.score 
        FROM test_users u 
        JOIN vector_search('test_users', '[0.1, 0.2, 0.3, 0.4]', 2) v 
        ON u.id = v.id
    """)
    
    joined_results = joined_df.collect()
    assert len(joined_results) == 2
    for r in joined_results:
        assert "oid" in r
        assert "score" in r

if __name__ == "__main__":
    import sys

    pytest.main(sys.argv)
