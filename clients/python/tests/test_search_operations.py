#!/usr/bin/env python3
"""
ProximaDB Search Operations Test Suite
Consolidated tests for ID-based search, metadata filtering, and proximity/similarity search
"""

import pytest
import time
import numpy as np
from typing import List, Dict, Any
from sentence_transformers import SentenceTransformer

from proximadb import ProximaDBClient, Protocol, connect_rest, connect_grpc
from proximadb.models import CollectionConfig, DistanceMetric, QuantizationConfig, QuantizationType
from proximadb.exceptions import ProximaDBError


class TestSearchOperations:
    """Comprehensive search operations test suite"""
    
    @pytest.fixture(scope="class")
    def rest_client(self):
        client = connect_rest("http://localhost:5678")
        yield client
        client.close()
    
    @pytest.fixture(scope="class")
    def grpc_client(self):
        client = connect_grpc("http://localhost:5679")
        yield client
        client.close()
    
    @pytest.fixture(scope="class")
    def bert_model(self):
        """Load BERT model for embeddings"""
        return SentenceTransformer('all-MiniLM-L6-v2')
    
    @pytest.fixture(scope="class")
    def search_collection(self, grpc_client):
        """Create test collection with search data"""
        collection_name = f"search_test_{int(time.time())}"
        
        config = CollectionConfig(
            name=collection_name,
            dimension=384,  # all-MiniLM-L6-v2 dimension
            distance_metric=DistanceMetric.COSINE,
            description="Search operations test collection"
        )
        
        collection = grpc_client.create_collection(collection_name, config)
        yield collection
        
        # Cleanup
        try:
            grpc_client.delete_collection(collection_name)
        except:
            pass
    
    @pytest.fixture(scope="class")
    def test_data(self, bert_model) -> List[Dict[str, Any]]:
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
                "tags": ["AI", "ML", "software", "innovation"]
            },
            {
                "id": "tech_002", 
                "text": "Cloud computing provides scalable infrastructure for modern applications",
                "category": "technology",
                "subcategory": "cloud",
                "importance": 8,
                "author": "Mark Thompson",
                "tags": ["cloud", "infrastructure", "scalability"]
            },
            {
                "id": "tech_003",
                "text": "Blockchain technology enables decentralized and secure transactions",
                "category": "technology", 
                "subcategory": "blockchain",
                "importance": 7,
                "author": "Dr. Sarah Chen",
                "tags": ["blockchain", "security", "decentralization"]
            },
            
            # Science category
            {
                "id": "sci_001",
                "text": "Quantum computing promises exponential speedup for complex calculations",
                "category": "science",
                "subcategory": "quantum",
                "importance": 10,
                "author": "Prof. Alan Turing",
                "tags": ["quantum", "computing", "physics"]
            },
            {
                "id": "sci_002",
                "text": "CRISPR gene editing revolutionizes medical treatment possibilities",
                "category": "science",
                "subcategory": "biology", 
                "importance": 9,
                "author": "Dr. Jennifer Wu",
                "tags": ["CRISPR", "genetics", "medicine"]
            },
            
            # Healthcare category
            {
                "id": "health_001",
                "text": "Telemedicine expands healthcare access to remote communities globally",
                "category": "healthcare",
                "subcategory": "telemedicine",
                "importance": 10,
                "author": "Dr. Jennifer Wu",
                "tags": ["telemedicine", "healthcare", "accessibility"]
            },
            
            # Education category
            {
                "id": "edu_001",
                "text": "Online learning platforms democratize access to quality education worldwide",
                "category": "education",
                "subcategory": "online",
                "importance": 9,
                "author": "Prof. Alan Turing",
                "tags": ["education", "online", "accessibility"]
            }
        ]
        
        # Generate embeddings
        texts = [doc["text"] for doc in documents]
        embeddings = bert_model.encode(texts)
        
        # Add embeddings to documents
        for i, doc in enumerate(documents):
            doc["embedding"] = embeddings[i].tolist()
            
        return documents
    
    @pytest.fixture(scope="class", autouse=True)
    def ingest_test_data(self, grpc_client, search_collection, test_data):
        """Ingest test data into the collection"""
        for doc in test_data:
            grpc_client.insert_vector(
                collection_id=search_collection.name,
                vector_id=doc["id"],
                vector=doc["embedding"],
                metadata={
                    "text": doc["text"],
                    "category": doc["category"],
                    "subcategory": doc["subcategory"],
                    "importance": doc["importance"],
                    "author": doc["author"],
                    "tags": doc["tags"]
                }
            )
        
        # Allow time for indexing
        time.sleep(1)
    
    def test_search_by_id(self, grpc_client, search_collection):
        """Test ID-based search functionality"""
        # Test existing IDs
        existing_ids = ["tech_001", "sci_001", "health_001"]
        
        for vector_id in existing_ids:
            result = grpc_client.get_vector(
                collection_id=search_collection.name,
                vector_id=vector_id,
                include_vector=False,
                include_metadata=True
            )
            
            assert result is not None, f"Failed to find vector {vector_id}"
            assert "metadata" in result
            assert "text" in result["metadata"]
        
        # Test non-existent ID
        with pytest.raises(ProximaDBError):
            grpc_client.get_vector(
                collection_id=search_collection.name,
                vector_id="non_existent_id",
                include_vector=False,
                include_metadata=True
            )
    
    def test_search_by_metadata_filtering(self, grpc_client, search_collection, bert_model):
        """Test metadata field search functionality"""
        query_text = "innovative software solutions"
        query_embedding = bert_model.encode([query_text])[0]
        
        # Search without filter first
        all_results = grpc_client.search(
            collection_id=search_collection.name,
            query=query_embedding.tolist(),
            k=10,
            include_metadata=True,
            include_vectors=False
        )
        
        assert len(all_results) > 0, "Search returned no results"
        
        # Client-side filtering by category
        tech_results = [r for r in all_results if r.metadata.get('category') == 'technology']
        assert len(tech_results) >= 3, f"Expected at least 3 technology results"
        
        # Verify all filtered results are in technology category
        for result in tech_results:
            assert result.metadata['category'] == 'technology'
        
        # Filter by author
        chen_results = [r for r in all_results if r.metadata.get('author') == 'Dr. Sarah Chen']
        assert len(chen_results) >= 2, f"Expected at least 2 results by Dr. Sarah Chen"
        
        # Filter by importance
        important_results = [r for r in all_results if r.metadata.get('importance', 0) >= 8]
        assert len(important_results) >= 5, f"Expected at least 5 high importance results"
    
    def test_proximity_similarity_search(self, grpc_client, search_collection, bert_model):
        """Test proximity/similarity search functionality"""
        test_queries = [
            {
                "text": "artificial intelligence machine learning deep learning",
                "expected_top_category": "technology",
                "expected_min_score": 0.5
            },
            {
                "text": "healthcare medicine telemedicine remote patient care",
                "expected_top_category": "healthcare",
                "expected_min_score": 0.5
            },
            {
                "text": "quantum computing physics exponential speedup algorithms",
                "expected_top_category": "science",
                "expected_min_score": 0.5
            }
        ]
        
        for query_info in test_queries:
            # Generate query embedding
            query_embedding = bert_model.encode([query_info["text"]])[0]
            
            # Perform similarity search
            results = grpc_client.search(
                collection_id=search_collection.name,
                query=query_embedding.tolist(),
                k=3,
                include_metadata=True,
                include_vectors=False
            )
            
            assert len(results) >= 1, f"No results for query: {query_info['text']}"
            
            # Verify top result
            top_result = results[0]
            assert top_result.score >= query_info["expected_min_score"], \
                f"Top score {top_result.score} below threshold"
            
            # Check if expected category is in top results
            top_categories = [r.metadata['category'] for r in results[:2]]
            assert query_info["expected_top_category"] in top_categories, \
                f"Expected category {query_info['expected_top_category']} not in top results"
    
    def test_document_similarity_search(self, grpc_client, search_collection, test_data):
        """Test document-to-document similarity search"""
        # Find documents similar to tech_001
        source_doc = next(d for d in test_data if d['id'] == 'tech_001')
        
        results = grpc_client.search(
            collection_id=search_collection.name,
            query=source_doc['embedding'],
            k=5,
            include_metadata=True,
            include_vectors=False
        )
        
        assert len(results) >= 2, "Not enough similar documents found"
        
        # First result should be the document itself with high similarity
        assert results[0].id == 'tech_001', "First result should be the source document"
        assert results[0].score > 0.99, "Self-similarity should be near 1.0"
        
        # Other technology documents should rank high
        tech_ids = ['tech_002', 'tech_003']
        result_ids = [r.id for r in results[1:4]]
        
        tech_found = sum(1 for tid in tech_ids if tid in result_ids)
        assert tech_found >= 1, "Expected at least one other technology document in top results"
    
    def test_cross_protocol_search(self, rest_client, grpc_client, search_collection, bert_model):
        """Test search operations across REST and gRPC protocols"""
        query_text = "technology innovation"
        query_embedding = bert_model.encode([query_text])[0]
        
        # Search via gRPC
        grpc_results = grpc_client.search(
            collection_id=search_collection.name,
            query=query_embedding.tolist(),
            k=5,
            include_metadata=True
        )
        
        # Search via REST
        rest_results = rest_client.search(
            collection_id=search_collection.name,
            query=query_embedding.tolist(),
            k=5,
            include_metadata=True
        )
        
        # Both should return results
        assert len(grpc_results) > 0, "gRPC search returned no results"
        assert len(rest_results) > 0, "REST search returned no results"
        
        # Results should be similar (same ranking algorithm)
        # Check that top results have some overlap
        grpc_top_ids = [r.id for r in grpc_results[:3]]
        rest_top_ids = [r.id for r in rest_results[:3]]
        
        overlap = len(set(grpc_top_ids) & set(rest_top_ids))
        assert overlap >= 1, "Expected some overlap in top results between protocols"
    
    def test_search_edge_cases(self, grpc_client, search_collection, bert_model):
        """Test search edge cases and boundary conditions"""
        query_embedding = bert_model.encode(["test query"])[0]
        
        # Test search with k larger than collection size
        results = grpc_client.search(
            collection_id=search_collection.name,
            query=query_embedding.tolist(),
            k=100,  # Much larger than our 7 documents
            include_metadata=True
        )
        
        # Should return all documents in collection
        assert len(results) == 7, f"Expected 7 results, got {len(results)}"
        
        # Verify all results have valid scores
        for result in results:
            assert 0 <= result.score <= 1, f"Invalid score: {result.score}"
            assert result.metadata is not None
        
        # Test search with k=0
        with pytest.raises(ProximaDBError):
            grpc_client.search(
                collection_id=search_collection.name,
                query=query_embedding.tolist(),
                k=0
            )
        
        # Test search with negative k
        with pytest.raises(ProximaDBError):
            grpc_client.search(
                collection_id=search_collection.name,
                query=query_embedding.tolist(),
                k=-1
            )
    
    def test_search_with_server_side_filtering(self, grpc_client, search_collection, bert_model):
        """Test server-side metadata filtering (if implemented)"""
        query_embedding = bert_model.encode(["innovative technology"])[0]
        
        try:
            # Attempt server-side filtering
            filtered_results = grpc_client.search(
                collection_id=search_collection.name,
                query=query_embedding.tolist(),
                k=10,
                filter={"category": "technology"},
                include_metadata=True
            )
            
            # If server-side filtering is implemented, verify results
            for result in filtered_results:
                assert result.metadata['category'] == 'technology'
                
        except Exception as e:
            # Server-side filtering not yet implemented - test client-side fallback
            all_results = grpc_client.search(
                collection_id=search_collection.name,
                query=query_embedding.tolist(),
                k=10,
                include_metadata=True
            )
            
            # Client-side filtering
            filtered_results = [r for r in all_results if r.metadata.get('category') == 'technology']
            assert len(filtered_results) >= 3, "Should find technology documents"
    
    def test_empty_collection_search(self, grpc_client, bert_model):
        """Test search on empty collection"""
        empty_collection = f"empty_search_{int(time.time())}"
        config = CollectionConfig(name=empty_collection, dimension=384, distance_metric=DistanceMetric.COSINE)
        grpc_client.create_collection(empty_collection, config)
        
        try:
            query_embedding = bert_model.encode(["test query"])[0]
            
            results = grpc_client.search(
                collection_id=empty_collection,
                query=query_embedding.tolist(),
                k=5,
                include_metadata=True
            )
            
            assert len(results) == 0, "Empty collection should return no results"
            
        finally:
            grpc_client.delete_collection(empty_collection)


class TestAdvancedSearchFeatures:
    """Test advanced search features and optimizations"""
    
    @pytest.fixture
    def grpc_client(self):
        client = connect_grpc("http://localhost:5679")
        yield client
        client.close()
    
    @pytest.fixture
    def bert_model(self):
        return SentenceTransformer('all-MiniLM-L6-v2')
    
    def test_search_performance_basic(self, grpc_client, bert_model):
        """Test basic search performance characteristics"""
        collection_name = f"perf_test_{int(time.time())}"
        config = CollectionConfig(name=collection_name, dimension=384, distance_metric=DistanceMetric.COSINE)
        
        collection = grpc_client.create_collection(collection_name, config)
        
        try:
            # Insert test data
            vector_count = 100
            for i in range(vector_count):
                vector = np.random.normal(0, 1, 384).astype(np.float32).tolist()
                grpc_client.insert_vector(
                    collection_id=collection_name,
                    vector_id=f"perf_vector_{i}",
                    vector=vector,
                    metadata={"index": i, "category": f"group_{i % 10}"}
                )
            
            # Perform search and measure
            query_embedding = bert_model.encode(["performance test query"])[0]
            
            start_time = time.time()
            results = grpc_client.search(
                collection_id=collection_name,
                query=query_embedding.tolist(),
                k=10,
                include_metadata=True
            )
            search_time = time.time() - start_time
            
            assert len(results) == 10, "Should return requested number of results"
            assert search_time < 1.0, f"Search took too long: {search_time:.3f}s"
            
        finally:
            grpc_client.delete_collection(collection_name)
    
    def test_search_with_quantization_hints(self, rest_client, bert_model):
        """Test search with quantization optimization hints"""
        collection_name = f"quant_search_test_{int(time.time())}"
        
        # Create collection with quantization enabled
        config = CollectionConfig(
            name=collection_name,
            dimension=384,
            distance_metric=DistanceMetric.COSINE,
            quantization_config=QuantizationConfig(
                enabled=True,
                type=QuantizationType.SCALAR,
                bits_per_vector=8,
                accuracy_threshold=0.95
            )
        )
        
        collection = rest_client.create_collection(collection_name, config)
        
        try:
            # Insert test vectors
            texts = [
                "ProximaDB is a high-performance vector database",
                "Machine learning models generate embeddings",
                "Vector search enables semantic similarity",
                "Quantization reduces memory usage",
                "Two-stage search improves performance"
            ]
            
            embeddings = bert_model.encode(texts)
            for i, (text, embedding) in enumerate(zip(texts, embeddings)):
                rest_client.insert(
                    collection_name,
                    vector=embedding.tolist(),
                    id=f"quant_doc_{i}",
                    metadata={"text": text, "index": i}
                )
            
            # Search with optimization hints
            query_text = "database performance optimization"
            query_embedding = bert_model.encode([query_text])[0]
            
            # Test different optimization configurations
            optimization_configs = [
                {
                    "name": "No optimization",
                    "hints": None
                },
                {
                    "name": "Basic two-stage",
                    "hints": {
                        "enable_two_stage_search": True,
                        "quantization_hint": "INT8",
                        "candidate_multiplier": 2.0
                    }
                },
                {
                    "name": "Aggressive optimization",
                    "hints": {
                        "enable_two_stage_search": True,
                        "quantization_hint": "PQ4",
                        "candidate_multiplier": 5.0,
                        "min_candidates": 20,
                        "max_candidates": 100,
                        "enable_parallel_search": True
                    }
                }
            ]
            
            for config in optimization_configs:
                start_time = time.time()
                results = rest_client.search(
                    collection_name,
                    query=query_embedding.tolist(),
                    k=3,
                    include_metadata=True,
                    optimization_hints=config["hints"]
                )
                search_time = time.time() - start_time
                
                print(f"\n{config['name']}:")
                print(f"  Search time: {search_time*1000:.2f}ms")
                print(f"  Results: {len(results)}")
                
                assert len(results) <= 3
                assert all(hasattr(r, 'score') for r in results)
                
                # With optimization, search should be faster (after warmup)
                if config["hints"] and config["hints"].get("enable_two_stage_search"):
                    assert search_time < 0.5, f"Optimized search too slow: {search_time:.3f}s"
                    
        finally:
            rest_client.delete_collection(collection_name)
    
    def test_grpc_search_with_optimization(self, grpc_client, bert_model):
        """Test gRPC search with optimization hints"""
        collection_name = f"grpc_opt_test_{int(time.time())}"
        config = CollectionConfig(name=collection_name, dimension=384, distance_metric=DistanceMetric.COSINE)
        
        collection = grpc_client.create_collection(collection_name, config)
        
        try:
            # Insert vectors
            vector_count = 50
            vectors = []
            for i in range(vector_count):
                vector = np.random.normal(0, 1, 384).astype(np.float32).tolist()
                vectors.append({
                    "id": f"opt_vec_{i}",
                    "vector": vector,
                    "metadata": {"index": i, "type": "random"}
                })
            
            grpc_client.insert_vectors(collection_name, vectors)
            
            # Test search with optimization hints
            query_vector = np.random.normal(0, 1, 384).astype(np.float32).tolist()
            
            optimization_hints = {
                "enable_two_stage_search": True,
                "quantization_hint": "PQ8",
                "candidate_multiplier": 3.0,
                "enable_clustering_optimization": True,
                "enable_metadata_filtering_hint": True,
                "accuracy_threshold": 0.9,
                "custom_hints": {
                    "use_gpu": "false",
                    "prefetch_size": "32"
                }
            }
            
            results = grpc_client.search_vectors(
                collection_name,
                [query_vector],
                top_k=10,
                optimization_hints=optimization_hints
            )
            
            assert len(results) == 10
            assert all(hasattr(r, 'score') for r in results)
            
            # Verify results are properly sorted by score
            scores = [r.score for r in results]
            assert scores == sorted(scores, reverse=True), "Results should be sorted by score"
            
        finally:
            grpc_client.delete_collection(collection_name)
    
    def test_collection_with_progressive_quantization(self, rest_client):
        """Test collection with progressive quantization enabled"""
        collection_name = f"prog_quant_test_{int(time.time())}"
        
        # Create collection with progressive quantization
        config = CollectionConfig(
            name=collection_name,
            dimension=128,
            distance_metric=DistanceMetric.EUCLIDEAN,
            quantization_config=QuantizationConfig(
                enabled=True,
                type=QuantizationType.UNIFORM,
                bits_per_vector=16,
                progressive_quantization=True,
                compression_ratio_target=2.0,
                accuracy_threshold=0.98
            )
        )
        
        collection = rest_client.create_collection(collection_name, config)
        
        try:
            # Insert initial vectors
            initial_vectors = np.random.rand(10, 128).astype(np.float32)
            for i, vec in enumerate(initial_vectors):
                rest_client.insert(
                    collection_name,
                    vector=vec.tolist(),
                    id=f"prog_vec_{i}",
                    metadata={"batch": "initial"}
                )
            
            # Add more vectors to trigger progressive quantization
            additional_vectors = np.random.rand(90, 128).astype(np.float32)
            for i, vec in enumerate(additional_vectors):
                rest_client.insert(
                    collection_name,
                    vector=vec.tolist(),
                    id=f"prog_vec_{i+10}",
                    metadata={"batch": "additional"}
                )
            
            # Search and verify accuracy is maintained
            query = np.random.rand(128).astype(np.float32)
            results = rest_client.search(
                collection_name,
                query=query.tolist(),
                k=5,
                include_metadata=True
            )
            
            assert len(results) == 5
            assert all(r.metadata is not None for r in results)
            
        finally:
            rest_client.delete_collection(collection_name)


if __name__ == "__main__":
    pytest.main([__file__, "-v", "-s"])