"""
Comprehensive integration tests for ProximaDB functionality.
"""

import pytest
import numpy as np
import time
from typing import List, Dict, Any


@pytest.mark.integration
@pytest.mark.slow
class TestComprehensiveIntegration:
    """Comprehensive integration tests using real data."""
    
    @pytest.fixture(autouse=True)
    def setup_collection(self, client, cleanup_collection):
        """Set up test collection."""
        self.client = client
        self.collection_name = cleanup_collection
        
        # Create collection with comprehensive configuration
        self.client.create_collection(
            name=self.collection_name,
            dimension=384,
            distance_metric="cosine",
            storage_engine="viper",
            indexing_algorithm="hnsw"
        )
    
    @pytest.mark.embedding
    def test_10mb_corpus_integration(self, corpus_data, cached_embeddings, bert_service):
        """Test integration with 10MB corpus data."""
        if not corpus_data or cached_embeddings is None:
            pytest.skip("10MB corpus data not available")
        
        # Use a subset for testing (full corpus is too large for unit tests)
        test_size = min(100, len(corpus_data))
        test_corpus = corpus_data[:test_size]
        test_embeddings = cached_embeddings[:test_size]
        
        # Prepare data for insertion
        vector_ids = [f"corpus_vec_{i:05d}" for i in range(test_size)]
        vectors = [emb.tolist() for emb in test_embeddings]
        metadata_list = []
        
        for i, doc in enumerate(test_corpus):
            metadata = {
                "category": doc["category"],
                "author": doc["author"],
                "doc_type": doc["doc_type"],
                "year": doc["year"],
                "length": doc["length"],
                "title": doc.get("title", f"Document {i}"),
                "source": "corpus_test"
            }
            metadata_list.append(metadata)
        
        # Measure insertion performance
        start_time = time.time()
        
        # Insert in smaller batches
        batch_size = 50
        for i in range(0, test_size, batch_size):
            end_idx = min(i + batch_size, test_size)
            batch_vectors = vectors[i:end_idx]
            batch_ids = vector_ids[i:end_idx]
            batch_metadata = metadata_list[i:end_idx]
            
            result = self.client.insert_vectors(
                self.collection_name,
                batch_vectors,
                batch_ids,
                batch_metadata
            )
            assert result is not None
        
        insert_time = time.time() - start_time
        throughput = test_size / insert_time
        
        print(f"\\n📊 INSERT PERFORMANCE:")
        print(f"   Vectors: {test_size}")
        print(f"   Time: {insert_time:.2f}s")
        print(f"   Throughput: {throughput:.1f} vectors/sec")
        
        # Test various search operations
        self._test_search_operations(bert_service, test_size)
    
    def _test_search_operations(self, bert_service, corpus_size):
        """Test different search operations."""
        print(f"\\n🔍 TESTING SEARCH OPERATIONS:")
        
        # 1. Test ID-based search (metadata filter)
        test_id = "corpus_vec_00001"
        start_time = time.time()
        results = self.client.search(
            self.collection_name,
            [0.1] * 384,  # dummy vector
            k=1,
            filter={"id": test_id}
        )
        search_time = (time.time() - start_time) * 1000
        print(f"   ID search: {search_time:.2f}ms")
        
        # 2. Test metadata filtering
        start_time = time.time()
        results = self.client.search(
            self.collection_name,
            [0.1] * 384,  # dummy vector
            k=10,
            filter={"category": "AI"}
        )
        search_time = (time.time() - start_time) * 1000
        print(f"   Metadata filter: {search_time:.2f}ms")
        
        # 3. Test similarity search
        query_text = "machine learning algorithms"
        query_embedding = bert_service.embed_texts([query_text])[0]
        
        start_time = time.time()
        results = self.client.search(
            self.collection_name,
            query_embedding.tolist(),
            k=10
        )
        search_time = (time.time() - start_time) * 1000
        print(f"   Similarity search: {search_time:.2f}ms")
        
        # 4. Test hybrid search
        start_time = time.time()
        results = self.client.search(
            self.collection_name,
            query_embedding.tolist(),
            k=10,
            filter={"category": "AI"}
        )
        search_time = (time.time() - start_time) * 1000
        print(f"   Hybrid search: {search_time:.2f}ms")
    
    def test_performance_benchmarking(self, bert_service):
        """Test performance benchmarking."""
        # Insert test data
        test_size = 50
        vector_ids = [f"perf_test_{i:03d}" for i in range(test_size)]
        vectors = [np.random.random(384).tolist() for _ in range(test_size)]
        metadata_list = [
            {
                "category": "performance_test",
                "index": i,
                "type": "benchmark"
            }
            for i in range(test_size)
        ]
        
        # Measure insertion time
        start_time = time.time()
        result = self.client.insert_vectors(
            self.collection_name,
            vectors,
            vector_ids,
            metadata_list
        )
        insert_time = time.time() - start_time
        
        assert result is not None
        
        # Measure search performance
        query_vector = np.random.random(384).tolist()
        search_times = []
        
        for _ in range(10):
            start_time = time.time()
            results = self.client.search(
                self.collection_name,
                query_vector,
                k=5
            )
            search_time = (time.time() - start_time) * 1000
            search_times.append(search_time)
        
        avg_search_time = sum(search_times) / len(search_times)
        min_search_time = min(search_times)
        max_search_time = max(search_times)
        
        print(f"\\n📈 PERFORMANCE BENCHMARK:")
        print(f"   Insert time: {insert_time:.3f}s ({test_size/insert_time:.1f} vectors/sec)")
        print(f"   Avg search: {avg_search_time:.2f}ms")
        print(f"   Min search: {min_search_time:.2f}ms") 
        print(f"   Max search: {max_search_time:.2f}ms")
        print(f"   Throughput: {1000/avg_search_time:.1f} searches/sec")
    
    def test_metadata_lifecycle(self):
        """Test metadata lifecycle with unlimited fields."""
        # Insert vector with extensive metadata
        vector_id = "metadata_test_001"
        vector = np.random.random(384).tolist()
        
        # Create extensive metadata (unlimited fields)
        metadata = {
            # Core filterable fields
            "category": "AI",
            "author": "Dr. Smith",
            "doc_type": "research_paper",
            "year": 2024,
            "length": 5000,
            
            # Additional metadata (goes to extra_meta)
            "title": "Advanced Vector Database Research",
            "abstract": "This paper explores vector database optimization...",
            "keywords": ["vector", "database", "search", "optimization"],
            "journal": "Journal of Database Research",
            "volume": 42,
            "issue": 3,
            "pages": "123-145",
            "doi": "10.1000/journal.2024.001",
            "citation_count": 15,
            "download_count": 342,
            "view_count": 1250,
            "language": "english",
            "region": "north_america",
            "institution": "University of Technology",
            "funding_source": "NSF Grant ABC-123",
            "processing_timestamp": "2024-01-15T10:30:00Z",
            "quality_score": 0.95,
            "review_status": "peer_reviewed",
            "access_level": "public",
            "format": "pdf",
            "file_size_mb": 2.3,
            "content_hash": "sha256:abc123def456..."
        }
        
        result = self.client.insert_vectors(
            self.collection_name,
            [vector],
            [vector_id],
            [metadata]
        )
        
        assert result is not None
        
        # Test searching with various metadata filters
        filters_to_test = [
            {"category": "AI"},
            {"author": "Dr. Smith"},
            {"year": 2024},
            {"journal": "Journal of Database Research"},
            {"category": "AI", "year": 2024}  # Multiple filters
        ]
        
        for filter_dict in filters_to_test:
            results = self.client.search(
                self.collection_name,
                [0.1] * 384,  # dummy vector
                k=5,
                filter=filter_dict
            )
            # Just verify the search executed without error
            assert results is not None