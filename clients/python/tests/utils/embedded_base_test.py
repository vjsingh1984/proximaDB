"""
Base test class for ProximaDB Python SDK tests using embedded database

Provides common functionality for tests using the embedded ProximaDB
(PyO3/maturin-based), allowing tests to run without a separate server process.

Note: The SDK is now named `proximadb_sdk` to avoid conflict with the
native `proximadb` module (PyO3/maturin-based embedded database).
"""

import pytest
import time
import uuid
import tempfile
import shutil
from typing import Optional, List, Dict, Any
from pathlib import Path
import numpy as np

# Import native ProximaDB module directly
# SDK is now proximadb_sdk, so no namespace conflict
from proximadb import ProximaDB


class EmbeddedProximaDBTest:
    """
    Base class for ProximaDB tests using embedded database

    Provides:
    - Automatic embedded database setup
    - Test collection management
    - Common test utilities
    - No external server required
    """

    # Class-level database for reuse across tests (optional)
    _shared_db = None
    _shared_data_dir = None

    @classmethod
    def setup_class(cls):
        """Initialize embedded database for tests"""
        # ProximaDB is imported at module level from native proximadb package

        # Create temporary directory for test data
        cls._shared_data_dir = tempfile.mkdtemp(prefix="proximadb_test_")

        # Create embedded database
        cls._shared_db = ProximaDB(
            data_dirs=cls._shared_data_dir,
            cache_size_mb=256,
            default_engine="sst",
            enable_wal=True,
        )

        # Also expose as instance attribute for compatibility
        cls.db = cls._shared_db

    @classmethod
    def teardown_class(cls):
        """Clean up embedded database"""
        if cls._shared_db is not None:
            try:
                cls._shared_db.flush()
            except Exception:
                pass
            cls._shared_db = None

        # Clean up temporary directory
        if cls._shared_data_dir and Path(cls._shared_data_dir).exists():
            try:
                shutil.rmtree(cls._shared_data_dir)
            except Exception:
                pass

    def setup_method(self):
        """Setup for each test method"""
        self.test_collection_name = f"test_{uuid.uuid4().hex[:8]}"
        self.created_collections = []
        self.db = self._shared_db

    def teardown_method(self):
        """Cleanup after each test method"""
        # Clean up created collections
        for collection_name in self.created_collections:
            try:
                self.db.delete_collection(collection_name)
            except Exception:
                pass

    def create_collection(
        self, name: Optional[str] = None, dimension: int = 384, engine: str = "sst"
    ) -> str:
        """
        Create a test collection

        Args:
            name: Collection name (auto-generated if None)
            dimension: Vector dimension
            engine: Storage engine

        Returns:
            Collection name
        """
        name = name or f"test_{uuid.uuid4().hex[:8]}"
        self.db.create_collection(name, dimension, engine)
        self.created_collections.append(name)
        return name

    def insert_test_vectors(
        self,
        collection_name: str,
        count: int = 10,
        dimension: int = 384,
        metadata_template: Optional[Dict[str, Any]] = None,
    ) -> List[Dict[str, Any]]:
        """
        Insert test vectors into collection

        Args:
            collection_name: Collection to insert into
            count: Number of vectors to insert
            dimension: Vector dimension
            metadata_template: Base metadata for vectors

        Returns:
            List of inserted vector info (ids and metadata)
        """
        # Generate deterministic vectors
        np.random.seed(42)
        vectors = np.random.rand(count, dimension).astype(np.float32)
        # Normalize vectors
        norms = np.linalg.norm(vectors, axis=1, keepdims=True)
        vectors = vectors / norms

        ids = [f"vec_{i:04d}" for i in range(count)]

        metadata_list = []
        for i in range(count):
            metadata = metadata_template.copy() if metadata_template else {}
            metadata.update(
                {
                    "index": i,
                    "test": True,
                    "category": f"cat_{i % 3}",
                    "value": float(i),
                }
            )
            metadata_list.append(metadata)

        # Insert vectors
        inserted = self.db.insert(collection_name, ids, vectors, metadata_list)

        # Brief pause for indexing
        time.sleep(0.1)

        return [{"id": ids[i], "metadata": metadata_list[i]} for i in range(count)]

    def search(
        self,
        collection_name: str,
        query_vector: np.ndarray,
        top_k: int = 10,
        filter_expr: Optional[str] = None,
    ) -> List[Dict[str, Any]]:
        """
        Search for similar vectors

        Args:
            collection_name: Collection to search
            query_vector: Query vector
            top_k: Number of results
            filter_expr: Optional filter expression

        Returns:
            Search results
        """
        results = self.db.search(
            collection_name, query_vector, top_k=top_k, filter=filter_expr
        )

        # Convert to list of dicts for compatibility
        return [
            {
                "id": r.id,
                "score": r.score,
                "metadata": r.metadata if hasattr(r, "metadata") else {},
            }
            for r in results
        ]

    def verify_search_results(
        self,
        results: List[Dict[str, Any]],
        expected_count: int,
        check_scores: bool = True,
    ):
        """
        Verify search results are valid

        Args:
            results: Search results
            expected_count: Expected number of results
            check_scores: Whether to verify scores are descending
        """
        assert (
            len(results) == expected_count
        ), f"Expected {expected_count} results, got {len(results)}"

        if check_scores and len(results) > 1:
            # Verify scores are in descending order (higher similarity = higher score)
            scores = [r["score"] for r in results]
            assert scores == sorted(
                scores, reverse=True
            ), f"Scores not in descending order: {scores}"

        # Verify each result has required fields
        for result in results:
            assert "id" in result, "Result missing 'id'"
            assert "score" in result, "Result missing 'score'"
            assert isinstance(
                result["score"], (int, float)
            ), f"Score is not numeric: {result['score']}"

    def wait_for_indexing(self, duration: float = 0.1):
        """Wait for vectors to be indexed (shorter for embedded)"""
        time.sleep(duration)

    def get_random_query_vector(self, dimension: int = 384) -> np.ndarray:
        """Generate a random normalized query vector"""
        vec = np.random.rand(dimension).astype(np.float32)
        return vec / np.linalg.norm(vec)


# Fixture for pytest
@pytest.fixture(scope="session")
def embedded_db():
    """Session-scoped embedded database fixture"""
    # ProximaDB is imported at module level from native proximadb package

    data_dir = tempfile.mkdtemp(prefix="proximadb_fixture_")

    db = ProximaDB(
        data_dirs=data_dir,
        cache_size_mb=256,
        default_engine="sst",
        enable_wal=True,
    )

    yield db

    # Cleanup
    db.flush()
    shutil.rmtree(data_dir, ignore_errors=True)


@pytest.fixture
def test_collection(embedded_db):
    """Create a temporary test collection"""
    name = f"test_{uuid.uuid4().hex[:8]}"
    embedded_db.create_collection(name, dimension=384, engine="sst")

    yield name, embedded_db

    # Cleanup
    try:
        embedded_db.delete_collection(name)
    except Exception:
        pass
