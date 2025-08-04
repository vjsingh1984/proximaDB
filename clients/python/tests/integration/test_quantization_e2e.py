#!/usr/bin/env python3
"""
End-to-end integration test for quantization features

This test verifies that the complete quantization flow works correctly
from client SDK through to storage engines.
"""

import pytest
import time
import numpy as np
from typing import List, Dict, Any

from proximadb import connect_rest, connect_grpc
from proximadb.models import (
    CollectionConfig,
    DistanceMetric,
    QuantizationConfig,
    QuantizationType,
    SearchOptimization as SearchOptimizationHints,
    QuantizationHint,
    VectorRecord
)
from proximadb import proximadb_pb2


class TestQuantizationE2E:
    """End-to-end quantization tests"""
    
    @pytest.fixture
    def rest_client(self):
        """Create REST client"""
        client = connect_rest("http://localhost:5678")
        yield client
        # Cleanup collections created during tests
        try:
            collections = client.list_collections()
            for col in collections:
                if col.name.startswith("test_quant_e2e_"):
                    client.delete_collection(col.name)
        except:
            pass
    
    @pytest.fixture
    def grpc_client(self):
        """Create gRPC client"""
        client = connect_grpc("grpc://localhost:5679")
        yield client
        # Cleanup
        try:
            collections = client.list_collections()
            for col in collections:
                if col.name.startswith("test_quant_e2e_"):
                    client.delete_collection(col.name)
        except:
            pass
    
    def test_proto_quantization_fields_exist(self):
        """Verify proto messages have quantization fields"""
        # Check CollectionConfig has quantization_config
        config = proximadb_pb2.CollectionConfig()
        assert hasattr(config, 'quantization_config')
        
        # Check VectorSearchRequest has search_optimization
        search_req = proximadb_pb2.VectorSearchRequest()
        assert hasattr(search_req, 'search_optimization')
        
        # Check QuantizationConfig message exists
        quant_config = proximadb_pb2.QuantizationConfig()
        assert hasattr(quant_config, 'enabled')
        # QuantizationConfig has different structure - check storage_quantization 
        assert hasattr(quant_config, 'storage_quantization')
        
        # Check StorageQuantizationConfig has progressive_quantization
        storage_config = proximadb_pb2.StorageQuantizationConfig()
        assert hasattr(storage_config, 'progressive_quantization')
        
        # Check SearchParams exists (the actual proto message for search optimization)
        search_params = proximadb_pb2.SearchParams()
        assert hasattr(search_params, 'enable_two_stage')
        assert hasattr(search_params, 'enable_clustering_hint')
        assert hasattr(search_params, 'no_quantization')  # quantization_hint oneof field
    
    def test_create_collection_with_quantization_rest(self, rest_client):
        """Test creating collection with quantization via REST"""
        collection_name = f"test_quant_e2e_{int(time.time())}"
        
        config = CollectionConfig(
            name=collection_name,
            dimension=128,
            distance_metric=DistanceMetric.COSINE,
            quantization_config=QuantizationConfig(
                enabled=True,
                type=QuantizationType.SCALAR,
                bits_per_vector=8,
                accuracy_threshold=0.95
            )
        )
        
        # Create collection
        collection = rest_client.create_collection(collection_name, config)
        assert collection.name == collection_name
        
        # Verify it exists
        collections = rest_client.list_collections()
        assert any(c.name == collection_name for c in collections)
        
        # Insert vectors
        vectors = np.random.rand(10, 128).astype(np.float32)
        ids = [f"vec_{i}" for i in range(10)]
        
        rest_client.insert_vectors(collection_name, vectors, ids)
        
        # Search without hints
        query = np.random.rand(128).astype(np.float32)
        results = rest_client.search(collection_name, query, top_k=5)
        assert len(results) == 5
    
    def test_search_with_optimization_hints_rest(self, rest_client):
        """Test search with optimization hints via REST"""
        collection_name = f"test_quant_e2e_hints_{int(time.time())}"
        
        # Create collection
        config = CollectionConfig(name=collection_name, dimension=256)
        rest_client.create_collection(collection_name, config)
        
        # Insert test data
        num_vectors = 100
        vectors = []
        ids = []
        metadata = []
        
        for i in range(num_vectors):
            vec = np.random.randn(256).astype(np.float32)
            vec = vec / np.linalg.norm(vec)  # Normalize
            vectors.append(vec)
            ids.append(f"doc_{i}")
            metadata.append({"index": i, "category": f"cat_{i % 5}"})
        
        rest_client.insert_vectors(collection_name, vectors, ids, metadata)
        
        # Test different optimization hints
        query = np.random.randn(256).astype(np.float32)
        query = query / np.linalg.norm(query)
        
        # No optimization
        start = time.time()
        results_baseline = rest_client.search(
            collection_name, 
            query, 
            top_k=10,
            optimization_hints=None
        )
        time_baseline = time.time() - start
        
        # With optimization
        start = time.time()
        results_optimized = rest_client.search(
            collection_name,
            query,
            top_k=10,
            optimization_hints=SearchOptimizationHints(
                enable_two_stage=True,
                quantization_hint=QuantizationHint(hint_type="scalar", parameters={"bits": 8})
            )
        )
        time_optimized = time.time() - start
        
        # Verify results
        assert len(results_baseline) == 10
        assert len(results_optimized) == 10
        
        # Check if optimization provides speedup (may vary based on data size)
        print(f"Baseline time: {time_baseline*1000:.2f}ms")
        print(f"Optimized time: {time_optimized*1000:.2f}ms")
        
        # Verify result quality - top results should be similar
        baseline_ids = [r.id for r in results_baseline[:5]]
        optimized_ids = [r.id for r in results_optimized[:5]]
        overlap = len(set(baseline_ids) & set(optimized_ids))
        assert overlap >= 3, f"Low overlap in top-5 results: {overlap}/5"
    
    def test_grpc_quantization_hints(self, grpc_client):
        """Test gRPC client with quantization hints"""
        collection_name = f"test_quant_e2e_grpc_{int(time.time())}"
        
        # Create collection via gRPC
        config = CollectionConfig(
            name=collection_name,
            dimension=384,
            distance_metric=DistanceMetric.COSINE
        )
        grpc_client.create_collection(collection_name, config)
        
        # Insert vectors
        vectors = []
        for i in range(10):
            vec = VectorRecord(
                id=f"grpc_vec_{i}",
                vector=np.random.randn(384).astype(np.float32).tolist(),
                metadata={"index": i}
            )
            vectors.append(vec)
        
        grpc_client.insert_vectors(collection_name, vectors)
        
        # Search with optimization hints
        query = np.random.randn(384).astype(np.float32).tolist()
        
        hints = SearchOptimizationHints(
            enable_two_stage=True,
            quantization_hint=QuantizationHint(hint_type="product", parameters={"bits": 8}),
            accuracy_threshold=0.9,
            custom_hints={
                "algorithm": "hnsw",
                "ef_search": "100"
            }
        )
        
        results = grpc_client.search(
            collection_id=collection_name,
            vector=query,
            top_k=10,
            optimization_hints=hints
        )
        
        assert len(results) == 10
        assert all(hasattr(r, 'score') for r in results)
        
        # Verify scores are sorted
        scores = [r.score for r in results]
        assert scores == sorted(scores, reverse=True)
    
    def test_quantization_types(self, rest_client):
        """Test different quantization types"""
        base_name = "test_quant_e2e_types"
        dimension = 512
        
        quantization_configs = [
            ("scalar", QuantizationType.SCALAR, {"bits_per_vector": 8}),
            ("product", QuantizationType.PRODUCT, {"bits_per_subvector": 4, "num_subvectors": 64}),
            ("binary", QuantizationType.BINARY, {}),
            ("uniform", QuantizationType.UNIFORM, {"bits_per_vector": 16})
        ]
        
        for type_name, quant_type, extra_params in quantization_configs:
            collection_name = f"{base_name}_{type_name}_{int(time.time())}"
            
            # Create quantization config
            quant_config = QuantizationConfig(
                enabled=True,
                type=quant_type,
                compression_ratio_target=4.0,
                accuracy_threshold=0.9,
                **extra_params
            )
            
            config = CollectionConfig(
                name=collection_name,
                dimension=dimension,
                distance_metric=DistanceMetric.EUCLIDEAN,
                quantization_config=quant_config
            )
            
            try:
                # Create collection
                collection = rest_client.create_collection(collection_name, config)
                print(f"✓ Created collection with {type_name} quantization")
                
                # Insert a few vectors
                vectors = np.random.rand(5, dimension).astype(np.float32)
                ids = [f"{type_name}_{i}" for i in range(5)]
                rest_client.insert_vectors(collection_name, vectors, ids)
                
                # Search
                query = np.random.rand(dimension).astype(np.float32)
                results = rest_client.search(collection_name, query, top_k=3)
                
                assert len(results) <= 3
                print(f"  Found {len(results)} results")
                
            except Exception as e:
                pytest.fail(f"Failed with {type_name} quantization: {e}")
    
    def test_progressive_quantization(self, rest_client):
        """Test progressive quantization feature"""
        collection_name = f"test_quant_e2e_progressive_{int(time.time())}"
        
        config = CollectionConfig(
            name=collection_name,
            dimension=128,
            distance_metric=DistanceMetric.COSINE,
            quantization_config=QuantizationConfig(
                enabled=True,
                type=QuantizationType.UNIFORM,
                bits_per_vector=8,
                progressive_quantization=True,
                compression_ratio_target=4.0,
                accuracy_threshold=0.95
            )
        )
        
        rest_client.create_collection(collection_name, config)
        
        # Insert vectors in batches to trigger progressive quantization
        batch_sizes = [10, 20, 50, 100]
        total_inserted = 0
        
        for batch_size in batch_sizes:
            vectors = np.random.rand(batch_size, 128).astype(np.float32)
            ids = [f"prog_{total_inserted + i}" for i in range(batch_size)]
            
            rest_client.insert_vectors(collection_name, vectors, ids)
            total_inserted += batch_size
            
            print(f"Inserted {total_inserted} vectors total")
            
            # Search after each batch
            query = np.random.rand(128).astype(np.float32)
            results = rest_client.search(
                collection_name,
                query,
                top_k=5,
                optimization_hints=SearchOptimizationHints(
                    enable_two_stage=True,
                    quantization_hint=QuantizationHint(hint_type="uniform", parameters={"bits": 8})
                )
            )
            
            assert len(results) <= 5
            print(f"  Search found {len(results)} results")
    
    def test_search_hints_model(self):
        """Test SearchOptimizationHints model"""
        hints = SearchOptimizationHints(
            enable_two_stage=True,
            quantization_hint=QuantizationHint(hint_type="product", parameters={"bits": 4}),
            accuracy_threshold=0.95,
            timeout_ms=100,
            custom_hints={"gpu": "true", "batch_size": "64"}
        )
        
        # Convert to dict for API
        hints_dict = hints.model_dump(exclude_none=True)
        
        assert hints_dict["enable_two_stage"] is True
        assert hints_dict["quantization_hint"]["hint_type"] == "product"
        assert hints_dict["quantization_hint"]["parameters"]["bits"] == 4
        assert hints_dict["custom_hints"]["gpu"] == "true"
        
        # Test validation - remove invalid field tests since model structure changed


if __name__ == "__main__":
    pytest.main([__file__, "-v", "-s"])