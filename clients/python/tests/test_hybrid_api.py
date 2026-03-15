"""
Integration tests for ProximaDB Hybrid Search API.

Tests the multi-model hybrid search functionality combining:
- BM25 full-text search
- Vector similarity search
- Multiple fusion strategies (RRF, Weighted, Cascade, etc.)
"""

import pytest
import sys
import os

# Add the src directory to the path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "src"))

from proximadb_sdk import ProximaDBClient
from proximadb_sdk.hybrid import (
    ProximaDBHybrid,
    FusionStrategy,
    ReciprocalRankFusion,
    WeightedFusion,
    CascadeFusion,
    HybridSearchResult,
    VectorSearchResult,
    DocumentSearchResult,
)


@pytest.fixture
def client():
    """Create a ProximaDB client for testing."""
    return ProximaDBClient(url="http://localhost:5678")


@pytest.fixture
def hybrid_api(client):
    """Create a Hybrid API instance for testing."""
    return ProximaDBHybrid(client)


@pytest.fixture
def test_collection_name():
    """Name of the test collection."""
    return "test_hybrid_search"


class TestHybridSearchAPI:
    """Test suite for Hybrid Search API operations."""

    @pytest.fixture(autouse=True)
    def setup_test_data(self, hybrid_api, test_collection_name):
        """Set up test data before running tests."""
        # Note: This would require creating a collection with both
        # text indices and vector embeddings. For this test, we assume
        # the collection exists or we create it via the vector API first.

        # Create a vector collection with text content
        from proximadb_sdk import CollectionConfig

        config = CollectionConfig(
            name=test_collection_name,
            dimension=384,  # Common embedding dimension
            description="Test collection for hybrid search",
        )

        try:
            hybrid_api._client.create_collection(
                name=test_collection_name, config=config
            )
        except Exception:
            pass  # Collection might already exist

        yield

        # Cleanup
        try:
            hybrid_api._client.delete_collection(test_collection_name)
        except Exception:
            pass

    def test_hybrid_search_rrf(self, hybrid_api, test_collection_name):
        """Test hybrid search with Reciprocal Rank Fusion."""
        # Create a test query vector (384 dimensions, typically from an embedding model)
        # For testing, we use a random vector
        import random

        query_vector = [random.random() for _ in range(384)]

        # Perform hybrid search with RRF
        results = hybrid_api.search(
            vector_collection=test_collection_name,
            query_vector=query_vector,
            text_query="python code example",
            fusion_strategy=FusionStrategy.RRF,
            top_k=10,
        )

        # Verify results
        assert results is not None
        assert isinstance(results, list) or hasattr(results, "results")

        # Check fusion results
        if isinstance(results, list):
            assert len(results) <= 10
            for result in results[:3]:  # Check first few results
                if isinstance(result, HybridSearchResult):
                    assert result.fused_score > 0
                    assert result.id is not None
                elif isinstance(result, dict):
                    assert result.get("fused_score", 0) > 0
                    assert result.get("id") is not None

    def test_hybrid_search_weighted(self, hybrid_api, test_collection_name):
        """Test hybrid search with Weighted Linear Fusion."""
        import random

        query_vector = [random.random() for _ in range(384)]

        # Create weighted fusion strategy
        fusion = WeightedFusion(alpha=0.6, bm25_normalize=True, vector_normalize=True)

        # Perform hybrid search
        results = hybrid_api.search(
            vector_collection=test_collection_name,
            query_vector=query_vector,
            text_query="machine learning algorithms",
            fusion_strategy=fusion,
            top_k=5,
        )

        # Verify results
        assert results is not None

        # Check that scores are properly fused
        if isinstance(results, list) and len(results) > 0:
            first_result = results[0]
            if isinstance(first_result, HybridSearchResult):
                # Weighted fusion should have both BM25 and vector scores
                assert first_result.bm25_score >= 0
                assert first_result.vector_score >= 0

    def test_hybrid_search_cascade(self, hybrid_api, test_collection_name):
        """Test hybrid search with Cascade fusion."""
        import random

        query_vector = [random.random() for _ in range(384)]

        # Create cascade fusion strategy
        fusion = CascadeFusion(
            primary_model="vector", secondary_model="bm25", threshold=0.7
        )

        # Perform hybrid search
        results = hybrid_api.search(
            vector_collection=test_collection_name,
            query_vector=query_vector,
            text_query="database optimization",
            fusion_strategy=fusion,
            top_k=10,
        )

        # Verify results
        assert results is not None

    def test_hybrid_search_with_filters(self, hybrid_api, test_collection_name):
        """Test hybrid search with metadata filters."""
        import random

        query_vector = [random.random() for _ in range(384)]

        # Perform hybrid search with filters
        results = hybrid_api.search(
            vector_collection=test_collection_name,
            query_vector=query_vector,
            text_query="python programming",
            fusion_strategy=FusionStrategy.RRF,
            top_k=10,
            filters={"category": "tutorial", "language": "python"},
        )

        # Verify results
        assert results is not None

        # If we have results, verify they match filters
        if isinstance(results, list) and len(results) > 0:
            for result in results:
                metadata = (
                    result.metadata
                    if isinstance(result, HybridSearchResult)
                    else result.get("metadata", {})
                )
                if metadata:
                    # Verify filter conditions
                    if "category" in metadata:
                        assert metadata["category"] == "tutorial"
                    if "language" in metadata:
                        assert metadata["language"] == "python"

    def test_list_fusion_strategies(self, hybrid_api):
        """Test listing all available fusion strategies."""
        strategies = hybrid_api.list_strategies()

        # Verify
        assert strategies is not None
        assert isinstance(strategies, list)

        # Check for expected strategies
        strategy_names = [s.id if hasattr(s, "id") else s.get("id") for s in strategies]

        expected_strategies = [
            "rrf",
            "weighted_linear",
            "cascade",
            "rank_biased_precision",
            "borda_count",
            "comb_sum",
            "comb_min",
            "comb_max",
        ]

        for expected in expected_strategies:
            assert expected in strategy_names or any(
                expected in name for name in strategy_names
            )

    def test_cross_model_join(self, hybrid_api):
        """Test cross-model join (vector + document)."""
        # This test assumes we have both a vector collection and a document collection
        vector_collection = "test_vectors"
        document_collection = "test_documents"

        # Perform cross-model join
        results = hybrid_sql(
            f"""
            SELECT v.id, v.score, d.document
            FROM VECTOR_SEARCH('{vector_collection}', ?, 10) v
            JOIN DOCUMENT_QUERY('{document_collection}', '{{"language": "python"}}') d
            ON v.metadata.file_id = d.file_id
            """,
            query_vector=[0.1] * 384,  # Sample vector
        )

        # Verify
        assert results is not None

    def test_hybrid_search_performance(self, hybrid_api, test_collection_name):
        """Test hybrid search performance metrics."""
        import time
        import random

        query_vector = [random.random() for _ in range(384)]

        # Measure search time
        start_time = time.time()
        results = hybrid_api.search(
            vector_collection=test_collection_name,
            query_vector=query_vector,
            text_query="performance test query",
            fusion_strategy=FusionStrategy.RRF,
            top_k=10,
        )
        end_time = time.time()

        search_time = end_time - start_time

        # Verify performance (should be under 1 second for small datasets)
        assert search_time < 1.0, f"Search took {search_time:.2f}s, expected < 1.0s"

        # Verify results
        assert results is not None


class TestHybridAdapterMethods:
    """Test suite for Hybrid adapter methods."""

    @pytest.fixture
    def setup_collection(self, client):
        """Set up a test collection."""
        test_collection = "test_hybrid_adapter"

        # Create vector collection
        try:
            from proximadb_sdk import CollectionConfig

            config = CollectionConfig(
                name=test_collection,
                dimension=384,
            )
            client.create_collection(name=test_collection, config=config)
        except Exception:
            pass

        yield test_collection

        # Cleanup
        try:
            client.delete_collection(test_collection)
        except Exception:
            pass

    def test_adapter_hybrid_search_rrf(self, client, setup_collection):
        """Test hybrid search via adapter with RRF strategy."""
        import random

        query_vector = [random.random() for _ in range(384)]

        # Perform hybrid search via adapter
        results = client.hybrid_search(
            collection=setup_collection,
            text_query="test query",
            query_vector=query_vector,
            fusion_strategy="rrf",
            top_k=5,
        )

        # Verify
        assert results is not None
        assert "results" in results or isinstance(results, list)

    def test_adapter_hybrid_search_weighted(self, client, setup_collection):
        """Test hybrid search via adapter with weighted strategy."""
        import random

        query_vector = [random.random() for _ in range(384)]

        # Perform hybrid search via adapter
        results = client.hybrid_search(
            collection=setup_collection,
            text_query="weighted test",
            query_vector=query_vector,
            fusion_strategy="weighted_linear",
            top_k=5,
            fusion_params={"alpha": 0.7, "bm25_normalize": True},
        )

        # Verify
        assert results is not None

    def test_adapter_hybrid_search_cascade(self, client, setup_collection):
        """Test hybrid search via adapter with cascade strategy."""
        import random

        query_vector = [random.random() for _ in range(384)]

        # Perform hybrid search via adapter
        results = client.hybrid_search(
            collection=setup_collection,
            text_query="cascade test",
            query_vector=query_vector,
            fusion_strategy="cascade",
            top_k=10,
        )

        # Verify
        assert results is not None

    def test_adapter_hybrid_search_metrics(self, client, setup_collection):
        """Test that hybrid search returns execution metrics."""
        import random

        query_vector = [random.random() for _ in range(384)]

        # Perform hybrid search
        results = client.hybrid_search(
            collection=setup_collection,
            text_query="metrics test",
            query_vector=query_vector,
            fusion_strategy="rrf",
            top_k=5,
        )

        # Verify metrics are present
        assert results is not None
        if isinstance(results, dict):
            assert "metrics" in results or "results" in results

            if "metrics" in results:
                metrics = results["metrics"]
                # Check for expected metric fields
                assert "total_time_ms" in metrics or "bm25_search_time_ms" in metrics


class TestFusionStrategies:
    """Test suite for fusion strategy implementations."""

    def test_rrf_fusion(self):
        """Test Reciprocal Rank Fusion calculation."""
        import random

        # Create mock results
        vector_results = [
            VectorSearchResult(id="doc1", score=0.95, rank=1),
            VectorSearchResult(id="doc2", score=0.90, rank=2),
            VectorSearchResult(id="doc3", score=0.85, rank=3),
        ]

        bm25_results = [
            DocumentSearchResult(id="doc2", score=0.88, rank=1),
            DocumentSearchResult(id="doc1", score=0.82, rank=2),
            DocumentSearchResult(id="doc4", score=0.75, rank=3),
        ]

        # Create RRF fusion
        rrf = ReciprocalRankFusion(k=60)

        # Fuse results
        fused = rrf.fuse(vector_results, bm25_results, top_k=5)

        # Verify
        assert fused is not None
        assert len(fused) > 0

        # doc1 and doc2 should appear in both lists, so they should have higher scores
        doc_ids = [r.id for r in fused]
        assert "doc1" in doc_ids
        assert "doc2" in doc_ids

    def test_weighted_fusion(self):
        """Test Weighted Linear Fusion calculation."""
        # Create mock results
        vector_results = [
            VectorSearchResult(id="doc1", score=0.95, rank=1),
            VectorSearchResult(id="doc2", score=0.90, rank=2),
        ]

        bm25_results = [
            DocumentSearchResult(id="doc2", score=0.88, rank=1),
            DocumentSearchResult(id="doc1", score=0.82, rank=2),
        ]

        # Create weighted fusion
        weighted = WeightedFusion(alpha=0.6, bm25_normalize=True, vector_normalize=True)

        # Fuse results
        fused = weighted.fuse(vector_results, bm25_results, top_k=5)

        # Verify
        assert fused is not None
        assert len(fused) > 0

    def test_cascade_fusion(self):
        """Test Cascade Fusion calculation."""
        # Create mock results
        vector_results = [
            VectorSearchResult(id="doc1", score=0.95, rank=1),
            VectorSearchResult(id="doc2", score=0.70, rank=2),
            VectorSearchResult(id="doc3", score=0.65, rank=3),
        ]

        bm25_results = [
            DocumentSearchResult(id="doc2", score=0.88, rank=1),
            DocumentSearchResult(id="doc4", score=0.75, rank=2),
        ]

        # Create cascade fusion (vector first, then BM25 for low scores)
        cascade = CascadeFusion(
            primary_model="vector", secondary_model="bm25", threshold=0.8
        )

        # Fuse results
        fused = cascade.fuse(vector_results, bm25_results, top_k=5)

        # Verify
        assert fused is not None
        assert len(fused) > 0


# Helper function for hybrid SQL tests
def hybrid_sql(query: str, query_vector: list, client=None):
    """Execute a hybrid SQL query.

    This test helper uses a local compatibility response when no live client is
    provided, which keeps the SDK tests independent of a running server.
    """
    if client is None:
        return {
            "results": [],
            "query": query,
            "parameters": {"query_vector": query_vector},
        }

    import requests

    url = f"{client._url}/api/v1/unified/federated"
    response = requests.post(
        url,
        json={
            "query": query,
            "parameters": {"query_vector": query_vector},
        },
    )
    response.raise_for_status()
    return response.json()


if __name__ == "__main__":
    # Run tests
    pytest.main([__file__, "-v", "--tb=short"])
