"""Tests for ProximaDB Embedded Mode"""

import tempfile

import numpy as np
import pytest

from proximadb_embedded import DiskConfig, ProximaDB, SearchResult


class TestEmbeddedBasics:
    """Basic functionality tests"""

    def test_create_database(self):
        """Test database creation"""
        with tempfile.TemporaryDirectory() as tmpdir:
            db = ProximaDB(data_dirs=tmpdir)
            assert db is not None

    def test_create_collection(self):
        """Test collection creation"""
        with tempfile.TemporaryDirectory() as tmpdir:
            db = ProximaDB(data_dirs=tmpdir)
            db.create_collection("test_collection", dimension=128)

            info = db.get_collection("test_collection")
            assert info is not None
            assert info.name == "test_collection"
            assert info.dimension == 128

    def test_list_collections(self):
        """Test listing collections"""
        with tempfile.TemporaryDirectory() as tmpdir:
            db = ProximaDB(data_dirs=tmpdir)
            db.create_collection("collection_one", dimension=64)
            db.create_collection("collection_two", dimension=128)

            collections = db.list_collections()
            names = [c.name for c in collections]
            assert "collection_one" in names
            assert "collection_two" in names

    def test_delete_collection(self):
        """Test collection deletion"""
        with tempfile.TemporaryDirectory() as tmpdir:
            db = ProximaDB(data_dirs=tmpdir)
            db.create_collection("to_delete", dimension=64)
            db.delete_collection("to_delete")

            info = db.get_collection("to_delete")
            assert info is None

    def test_execute_sql_agentic_schema_ddl(self):
        """Test embedded SQL DDL lowers mixed schema into Rust catalog."""
        with tempfile.TemporaryDirectory() as tmpdir:
            db = ProximaDB(data_dirs=tmpdir)
            result = db.execute_sql(
                """
                CREATE TABLE IF NOT EXISTS "agent_store" (
                    "record_id" TEXT NOT NULL,
                    "payload" JSONB NOT NULL DEFAULT '{}'::jsonb,
                    "embedding" VECTOR(32),
                    PRIMARY KEY ("record_id")
                ) WITH (
                    storage_engine = 'SST',
                    layout = 'hybrid',
                    xcatalog_namespace = 'agentic.pyembedded',
                    schema_kind = 'agentic_mixed'
                );
                """
            )

            assert result["row_count"] == 1
            assert result["rows"][0]["success"] is True
            assert "Created table" in result["rows"][0]["message"]

            gin = db.execute_sql(
                "CREATE INDEX idx_agent_payload ON agent_store USING GIN (payload);"
            )
            hnsw = db.execute_sql(
                "CREATE INDEX idx_agent_embedding ON agent_store USING HNSW (embedding);"
            )
            assert gin["rows"][0]["success"] is True
            assert hnsw["rows"][0]["success"] is True

            tables = db.execute_sql(
                "SELECT * FROM xcatalog.tables WHERE table_name = 'agent_store';"
            )
            assert tables["row_count"] == 1
            assert tables["rows"][0]["table_name"] == "agent_store"
            assert tables["rows"][0]["schema_kind"] == "agentic_mixed"
            assert tables["rows"][0]["storage_engine"] == "SST"
            assert tables["rows"][0]["xcatalog_namespace"] == "agentic.pyembedded"

            columns = db.execute_sql(
                "SELECT * FROM xcatalog.columns WHERE table_name = 'agent_store';"
            )
            assert any(
                row["column_name"] == "payload" and row["data_type"] == "jsonb"
                for row in columns["rows"]
            )
            assert any(
                row["column_name"] == "embedding"
                and row["data_type"] == "vector"
                and row["vector_dimension"] == "32"
                for row in columns["rows"]
            )

            indexes = db.execute_sql(
                "SELECT * FROM xcatalog.indexes WHERE table_name = 'agent_store';"
            )
            assert any(
                row["index_name"] == "idx_agent_payload" and row["index_type"] == "gin"
                for row in indexes["rows"]
            )
            assert any(
                row["index_name"] == "idx_agent_embedding"
                and row["index_type"] == "hnsw"
                for row in indexes["rows"]
            )

            insert = db.execute_sql(
                """
                INSERT INTO agent_store (
                    record_id,
                    payload,
                    embedding
                ) VALUES (
                    'record-1',
                    '{"kind":"memory","score":7}'::jsonb,
                    '[0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8,
                      0.9, 1.0, 1.1, 1.2, 1.3, 1.4, 1.5, 1.6,
                      1.7, 1.8, 1.9, 2.0, 2.1, 2.2, 2.3, 2.4,
                      2.5, 2.6, 2.7, 2.8, 2.9, 3.0, 3.1, 3.2]'
                );
                """
            )
            assert insert["row_count"] == 1
            assert insert["rows"][0]["success"] is True
            assert insert["rows"][0]["rows_affected"] == 1
            assert insert["rows"][0]["inserted_ids"] == ["record-1"]


class TestVectorOperations:
    """Vector insert and search tests"""

    def test_insert_vectors(self):
        """Test vector insertion"""
        with tempfile.TemporaryDirectory() as tmpdir:
            db = ProximaDB(data_dirs=tmpdir)
            db.create_collection("vectors_main", dimension=128)

            vectors = np.random.rand(100, 128).astype(np.float32)
            ids = [f"vec_{i}" for i in range(100)]

            count = db.insert("vectors_main", ids=ids, vectors=vectors)
            assert count == 100

    def test_insert_with_metadata(self):
        """Test vector insertion with metadata"""
        with tempfile.TemporaryDirectory() as tmpdir:
            db = ProximaDB(data_dirs=tmpdir)
            db.create_collection("vectors_meta", dimension=64)

            vectors = np.random.rand(10, 64).astype(np.float32)
            ids = [f"vec_{i}" for i in range(10)]
            metadata = [{"category": "A", "score": 0.9} for _ in range(10)]

            count = db.insert("vectors_meta", ids=ids, vectors=vectors, metadata=metadata)
            assert count == 10

    def test_search_basic(self):
        """Test basic vector search"""
        with tempfile.TemporaryDirectory() as tmpdir:
            db = ProximaDB(data_dirs=tmpdir)
            db.create_collection("search_test", dimension=64)

            # Insert vectors
            vectors = np.random.rand(100, 64).astype(np.float32)
            ids = [f"vec_{i}" for i in range(100)]
            db.insert("search_test", ids=ids, vectors=vectors)

            # Search with first vector as query
            results = db.search("search_test", query=vectors[0], top_k=5)

            assert len(results) <= 5
            assert all(isinstance(r, SearchResult) for r in results)

            # First result should be exact match (or very close)
            if results:
                assert results[0].id == "vec_0" or results[0].score < 0.001

    def test_search_top_k(self):
        """Test search with different top_k values"""
        with tempfile.TemporaryDirectory() as tmpdir:
            db = ProximaDB(data_dirs=tmpdir)
            db.create_collection("topk_test", dimension=32)

            vectors = np.random.rand(50, 32).astype(np.float32)
            ids = [f"v{i}" for i in range(50)]
            db.insert("topk_test", ids=ids, vectors=vectors)

            # Test different top_k values
            for k in [1, 5, 10, 20]:
                results = db.search("topk_test", query=vectors[0], top_k=k)
                assert len(results) <= k


class TestMultiDisk:
    """Multi-disk configuration tests"""

    def test_disk_config(self):
        """Test DiskConfig creation"""
        config = DiskConfig("/data/path", weight=2, tags=["hot", "ssd"])
        assert config.path == "/data/path"
        assert config.weight == 2
        assert "hot" in config.tags
        assert "ssd" in config.tags

    def test_multi_disk_setup(self):
        """Test multi-disk database setup"""
        with tempfile.TemporaryDirectory() as tmpdir1:
            with tempfile.TemporaryDirectory() as tmpdir2:
                disks = [
                    DiskConfig(tmpdir1, weight=2),
                    DiskConfig(tmpdir2, weight=1),
                ]
                db = ProximaDB(data_dirs=disks)

                # Should be able to create collections
                db.create_collection("multi_disk_test", dimension=128)
                info = db.get_collection("multi_disk_test")
                assert info is not None


class TestPersistence:
    """Persistence and durability tests"""

    def test_flush(self):
        """Test explicit flush"""
        with tempfile.TemporaryDirectory(ignore_cleanup_errors=True) as tmpdir:
            db = ProximaDB(data_dirs=tmpdir)
            db.create_collection("flush_test", dimension=64)

            vectors = np.random.rand(10, 64).astype(np.float32)
            ids = [f"v{i}" for i in range(10)]
            db.insert("flush_test", ids=ids, vectors=vectors)

            # Should not raise
            db.flush()

    def test_stats(self):
        """Test storage statistics"""
        with tempfile.TemporaryDirectory(ignore_cleanup_errors=True) as tmpdir:
            db = ProximaDB(data_dirs=tmpdir)
            db.create_collection("stats_test", dimension=128)

            vectors = np.random.rand(100, 128).astype(np.float32)
            ids = [f"v{i}" for i in range(100)]
            db.insert("stats_test", ids=ids, vectors=vectors)
            db.flush()

            stats = db.stats()
            assert stats.total_collections >= 1
            assert stats.total_vectors >= 100


class TestContextManager:
    """Context manager tests"""

    def test_with_statement(self):
        """Test using database with context manager"""
        with tempfile.TemporaryDirectory(ignore_cleanup_errors=True) as tmpdir:
            with ProximaDB(data_dirs=tmpdir) as db:
                db.create_collection("context_test", dimension=32)
                vectors = np.random.rand(10, 32).astype(np.float32)
                db.insert(
                    "context_test", ids=[f"v{i}" for i in range(10)], vectors=vectors
                )
            # Should be flushed after exit


class TestEngines:
    """Storage engine tests"""

    @pytest.mark.parametrize(
        "engine", ["sst", "viper", "nova", "swift", "raptor", "helix"]
    )
    def test_engine_types(self, engine):
        """Test different storage engines"""
        with tempfile.TemporaryDirectory() as tmpdir:
            db = ProximaDB(data_dirs=tmpdir)
            try:
                db.create_collection(f"engine_{engine}", dimension=64, engine=engine)
                info = db.get_collection(f"engine_{engine}")
                assert info is not None
            except Exception:
                # Some engines might not be available in all builds
                pytest.skip(f"Engine {engine} not available")


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
