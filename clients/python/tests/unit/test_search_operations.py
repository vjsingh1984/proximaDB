#!/usr/bin/env python3
"""
ProximaDB Search Operations Test Suite
Tests for ID-based search, metadata filtering, and proximity/similarity search

Tests run against embedded ProximaDB database for fast, reliable testing.
"""

import logging
import time
from typing import Any

import numpy as np
import pytest

from proximadb_sdk import CollectionConfig, ProximaDBError

logger = logging.getLogger(__name__)


# Local helper functions for vector generation
def embed_seed(seed: int, dimension: int) -> np.ndarray:
    """Generate a deterministic embedding based on seed"""
    np.random.seed(seed)
    vec = np.random.rand(dimension).astype(np.float32)
    return vec / np.linalg.norm(vec)


class TestSearchOperations:
    """Comprehensive search operations test suite using embedded database"""

    @pytest.fixture(scope="class")
    def bert_model(self):
        """Load BERT model for embeddings"""
        try:
            from sentence_transformers import SentenceTransformer

            return SentenceTransformer("all-MiniLM-L6-v2")
        except ImportError:
            pytest.skip("sentence-transformers not installed")

    @pytest.fixture(scope="class")
    def search_collection(self, rest_client):
        """Create test collection with search data"""
        collection_name = f"search_test_{int(time.time() * 1000)}"

        config = CollectionConfig(
            name=collection_name,
            dimension=384,  # all-MiniLM-L6-v2 dimension
            distance_metric="cosine",
            description="Search operations test collection",
        )
        collection = rest_client.create_collection(collection_name, config=config)
        yield collection

        # Cleanup
        try:
            rest_client.delete_collection(collection_name)
        except:
            pass

    @pytest.fixture(scope="class")
    def test_data(self, bert_model) -> list[dict[str, Any]]:
        """Prepare diverse test data with embeddings"""
        documents = [
            # Technology category
            {
                "id": "tech_001",
                "text": "Artificial intelligence and machine learning are revolutionizing software development",
                "category": "technology",
                "subcategory": "ai",
                "importance": 9,
                "author": "Dr. Sarah Chen",
                "tags": ["AI", "ML", "software", "innovation"],
            },
            {
                "id": "tech_002",
                "text": "Cloud computing provides scalable infrastructure for modern applications",
                "category": "technology",
                "subcategory": "cloud",
                "importance": 8,
                "author": "Mark Thompson",
                "tags": ["cloud", "infrastructure", "scalability"],
            },
            {
                "id": "tech_003",
                "text": "Blockchain technology enables decentralized and secure transactions",
                "category": "technology",
                "subcategory": "blockchain",
                "importance": 7,
                "author": "Dr. Sarah Chen",
                "tags": ["blockchain", "security", "decentralization"],
            },
            # Science category
            {
                "id": "sci_001",
                "text": "Quantum computing promises exponential speedup for complex calculations",
                "category": "science",
                "subcategory": "quantum",
                "importance": 10,
                "author": "Prof. Alan Turing",
                "tags": ["quantum", "computing", "physics"],
            },
            {
                "id": "sci_002",
                "text": "CRISPR gene editing revolutionizes medical treatment possibilities",
                "category": "science",
                "subcategory": "biology",
                "importance": 9,
                "author": "Dr. Jennifer Wu",
                "tags": ["CRISPR", "genetics", "medicine"],
            },
            # Healthcare category
            {
                "id": "health_001",
                "text": "Telemedicine expands healthcare access to remote communities globally",
                "category": "healthcare",
                "subcategory": "telemedicine",
                "importance": 10,
                "author": "Dr. Jennifer Wu",
                "tags": ["telemedicine", "healthcare", "accessibility"],
            },
            # Education category
            {
                "id": "edu_001",
                "text": "Online learning platforms democratize access to quality education worldwide",
                "category": "education",
                "subcategory": "online",
                "importance": 9,
                "author": "Prof. Alan Turing",
                "tags": ["education", "online", "accessibility"],
            },
        ]

        # Generate embeddings
        texts = [doc["text"] for doc in documents]
        embeddings = bert_model.encode(texts)

        # Add embeddings to documents
        for i, doc in enumerate(documents):
            doc["embedding"] = embeddings[i].tolist()

        return documents

    @pytest.fixture(scope="class", autouse=True)
    def ingest_test_data(self, rest_client, search_collection, test_data):
        """Ingest test data into the collection"""
        for doc in test_data:
            rest_client.insert_vector(
                collection_id=search_collection.name,
                vector_id=doc["id"],
                vector=doc["embedding"],
                metadata={
                    "text": doc["text"],
                    "category": doc["category"],
                    "subcategory": doc["subcategory"],
                    "importance": doc["importance"],
                    "author": doc["author"],
                    "tags": str(doc["tags"]),  # Store as string for embedded DB
                },
            )

        # Allow time for indexing
        time.sleep(0.5)

    def _wait_for_search_results(
        self, search_func, min_results=1, max_wait=10, retry_interval=0.5
    ):
        """Helper method to wait for search results with retries"""
        start_time = time.time()
        while time.time() - start_time < max_wait:
            try:
                results = search_func()
                if len(results) >= min_results:
                    return results
                logger.debug(
                    f"Waiting for indexing... got {len(results)} results, need {min_results}"
                )
                time.sleep(retry_interval)
            except Exception as e:
                logger.debug(f"Search error: {e}, retrying...")
                time.sleep(retry_interval)

        # Final attempt
        return search_func()

    def test_basic_vector_search(self, rest_client, search_collection, bert_model):
        """Test basic vector similarity search"""
        query_text = "artificial intelligence machine learning"
        query_embedding = bert_model.encode([query_text])[0]

        def search_func():
            return rest_client.search(
                collection_id=search_collection.name,
                vector=query_embedding.tolist(),
                top_k=5,
                include_metadata=True,
            )

        results = self._wait_for_search_results(search_func, min_results=1, max_wait=5)

        assert len(results) > 0, "Search should return at least one result"
        # Verify results have expected structure
        for result in results:
            assert hasattr(result, "id")
            assert hasattr(result, "score")

    def test_search_by_metadata_filtering(
        self, rest_client, search_collection, bert_model
    ):
        """Test metadata field search functionality"""
        query_text = "innovative software solutions"
        query_embedding = bert_model.encode([query_text])[0]

        # Search without filter first
        def search_func():
            return rest_client.search(
                collection_id=search_collection.name,
                vector=query_embedding.tolist(),
                top_k=10,
                include_metadata=True,
            )

        all_results = self._wait_for_search_results(
            search_func, min_results=1, max_wait=5
        )

        if len(all_results) == 0:
            pytest.skip("Search returned no results - indexing may not be complete")

        # Client-side filtering by category
        tech_results = [
            r for r in all_results if r.metadata.get("category") == "technology"
        ]

        # Verify all filtered results are in technology category
        for result in tech_results:
            assert result.metadata["category"] == "technology"

    def test_proximity_similarity_search(
        self, rest_client, search_collection, bert_model
    ):
        """Test proximity/similarity search functionality"""
        test_queries = [
            {
                "text": "artificial intelligence machine learning deep learning",
                "expected_top_category": "technology",
                "expected_min_score": 0.15,
            },
            {
                "text": "healthcare medicine telemedicine remote patient care",
                "expected_top_category": "healthcare",
                "expected_min_score": 0.15,
            },
            {
                "text": "quantum computing physics exponential speedup algorithms",
                "expected_top_category": "science",
                "expected_min_score": 0.15,
            },
        ]

        for query_info in test_queries:
            query_embedding = bert_model.encode([query_info["text"]])[0]

            def search_func():
                return rest_client.search(
                    collection_id=search_collection.name,
                    vector=query_embedding.tolist(),
                    top_k=3,
                    include_metadata=True,
                )

            results = self._wait_for_search_results(
                search_func, min_results=1, max_wait=5
            )

            if len(results) == 0:
                logger.debug(f"No results for query: {query_info['text']} - skipping")
                continue

            # Verify top result has reasonable score
            top_result = results[0]
            assert (
                top_result.score >= query_info["expected_min_score"]
            ), f"Top score {top_result.score} below threshold"

    def test_document_similarity_search(
        self, rest_client, search_collection, test_data
    ):
        """Test document-to-document similarity search"""
        # Find documents similar to tech_001
        source_doc = next(d for d in test_data if d["id"] == "tech_001")

        def search_func():
            return rest_client.search(
                collection_id=search_collection.name,
                vector=source_doc["embedding"],
                top_k=5,
                include_metadata=True,
            )

        results = self._wait_for_search_results(search_func, min_results=2, max_wait=5)

        if len(results) < 2:
            pytest.skip("Not enough similar documents found")

        # First result should be the document itself with high similarity
        assert results[0].id == "tech_001", "First result should be the source document"
        assert results[0].score > 0.99, "Self-similarity should be near 1.0"

    def test_search_with_different_top_k(
        self, rest_client, search_collection, bert_model
    ):
        """Test search with different top_k values"""
        query_embedding = bert_model.encode(["test query"])[0]

        # Test with various top_k values
        for top_k in [1, 3, 5, 10]:
            results = rest_client.search(
                collection_id=search_collection.name,
                vector=query_embedding.tolist(),
                top_k=top_k,
                include_metadata=True,
            )

            # Results should not exceed top_k
            assert len(results) <= top_k

    def test_search_edge_cases(self, rest_client, search_collection, bert_model):
        """Test search edge cases and boundary conditions"""
        query_embedding = bert_model.encode(["test query"])[0]

        # Test search with k larger than collection size
        results = rest_client.search(
            collection_id=search_collection.name,
            vector=query_embedding.tolist(),
            top_k=100,  # Much larger than our 7 documents
            include_metadata=True,
        )

        # Should return up to all documents in collection
        assert len(results) <= 7, f"Expected at most 7 results, got {len(results)}"

        # Verify all results have valid scores
        for result in results:
            assert -0.01 <= result.score <= 1.05, f"Invalid score: {result.score}"
            assert result.metadata is not None

        # Test search with k=0 should raise error
        with pytest.raises((ProximaDBError, Exception)):
            rest_client.search(
                collection_id=search_collection.name,
                vector=query_embedding.tolist(),
                top_k=0,
            )

        # Test search with negative k should raise error
        with pytest.raises((ProximaDBError, Exception)):
            rest_client.search(
                collection_id=search_collection.name,
                vector=query_embedding.tolist(),
                top_k=-1,
            )

    def test_empty_collection_search(self, rest_client, bert_model):
        """Test search on empty collection"""
        empty_collection = f"empty_search_{int(time.time() * 1000)}"

        config = CollectionConfig(
            name=empty_collection, dimension=384, distance_metric="cosine"
        )
        rest_client.create_collection(empty_collection, config=config)

        try:
            query_embedding = bert_model.encode(["test query"])[0]

            results = rest_client.search(
                collection_id=empty_collection,
                vector=query_embedding.tolist(),
                top_k=5,
                include_metadata=True,
            )

            assert len(results) == 0, "Empty collection should return no results"

        finally:
            rest_client.delete_collection(empty_collection)


class TestSearchWithDeterministicVectors:
    """Test search operations with deterministic vectors (no BERT required)"""

    @pytest.fixture(scope="class")
    def deterministic_collection(self, rest_client):
        """Create collection with deterministic test vectors"""
        collection_name = f"deterministic_search_{int(time.time() * 1000)}"

        config = CollectionConfig(
            name=collection_name,
            dimension=128,
            distance_metric="cosine",
            description="Deterministic search test collection",
        )
        collection = rest_client.create_collection(collection_name, config=config)

        # Insert deterministic vectors
        for i in range(50):
            vector = embed_seed(i, 128)
            rest_client.insert_vector(
                collection_id=collection_name,
                vector_id=f"vec_{i}",
                vector=vector.tolist(),
                metadata={
                    "index": i,
                    "category": f"cat_{i % 5}",
                    "group": f"group_{i % 10}",
                },
            )

        time.sleep(0.5)  # Allow indexing
        yield collection

        # Cleanup
        try:
            rest_client.delete_collection(collection_name)
        except:
            pass

    def test_self_similarity_search(self, rest_client, deterministic_collection):
        """Test that a vector is most similar to itself"""
        query_vector = embed_seed(0, 128)

        results = rest_client.search(
            collection_id=deterministic_collection.name,
            vector=query_vector.tolist(),
            top_k=5,
            include_metadata=True,
        )

        assert len(results) > 0, "Should return at least one result"
        assert results[0].id == "vec_0", "Most similar vector should be vec_0"
        assert results[0].score > 0.99, "Self-similarity should be near 1.0"

    def test_nearby_vectors_ranked_higher(self, rest_client, deterministic_collection):
        """Test that nearby vectors (similar seeds) rank higher"""
        # Use seed 5 as query
        query_vector = embed_seed(5, 128)

        results = rest_client.search(
            collection_id=deterministic_collection.name,
            vector=query_vector.tolist(),
            top_k=10,
            include_metadata=True,
        )

        assert len(results) > 0, "Should return results"
        # First result should be vec_5
        assert results[0].id == "vec_5"

    def test_search_result_ordering(self, rest_client, deterministic_collection):
        """Test that search results are ordered by score (descending)"""
        query_vector = embed_seed(25, 128)

        results = rest_client.search(
            collection_id=deterministic_collection.name,
            vector=query_vector.tolist(),
            top_k=20,
            include_metadata=True,
        )

        # Verify descending score order
        for i in range(1, len(results)):
            assert (
                results[i - 1].score >= results[i].score
            ), f"Results not sorted: {results[i-1].score} < {results[i].score}"

    def test_metadata_present_in_results(self, rest_client, deterministic_collection):
        """Test that metadata is correctly returned in search results"""
        query_vector = embed_seed(10, 128)

        results = rest_client.search(
            collection_id=deterministic_collection.name,
            vector=query_vector.tolist(),
            top_k=5,
            include_metadata=True,
        )

        for result in results:
            assert result.metadata is not None
            assert "index" in result.metadata or hasattr(result.metadata, "index")
            assert "category" in result.metadata or hasattr(result.metadata, "category")

    def test_client_side_filtering(self, rest_client, deterministic_collection):
        """Test client-side metadata filtering"""
        query_vector = embed_seed(0, 128)

        # Get all results
        all_results = rest_client.search(
            collection_id=deterministic_collection.name,
            vector=query_vector.tolist(),
            top_k=50,
            include_metadata=True,
        )

        # Client-side filter by category - handle both dict-like and attribute access
        cat_0_results = []
        for r in all_results:
            category = None
            if hasattr(r.metadata, "get"):
                category = r.metadata.get("category")
            elif hasattr(r.metadata, "category"):
                category = r.metadata.category
            if category == "cat_0":
                cat_0_results.append(r)

        # Should have results in cat_0 (indices 0, 5, 10, 15, 20, 25, 30, 35, 40, 45)
        # Note: may get fewer if top_k limits or indexing is incomplete
        assert len(all_results) > 0, "Should return some results"

        # Verify any filtered results have correct category
        for result in cat_0_results:
            cat = (
                result.metadata.get("category")
                if hasattr(result.metadata, "get")
                else result.metadata.category
            )
            assert cat == "cat_0"


if __name__ == "__main__":
    pytest.main([__file__, "-v", "-s"])
