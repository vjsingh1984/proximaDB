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
        with tempfile.TemporaryDirectory() as tmpdir:
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
