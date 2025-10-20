"""
Tests for ProximaDB batching functionality with real server

Uses real ProximaDB server connections to test request batching,
dynamic batch sizing, and performance optimizations.

NOTE: Moved from tests/unit/ to tests/integration/ - these are integration tests
requiring a running ProximaDB server.
"""

import pytest
import time
import numpy as np
from pathlib import Path
import sys
from typing import List, Dict, Any
from concurrent.futures import ThreadPoolExecutor, as_completed

# Add utils to path
sys.path.insert(0, str(Path(__file__).parent.parent))
from utils.base_test import BaseProximaDBTest
from utils.server_utils import ensure_server_running

from proximadb.batching_unified import (
    BatchStrategy,
    BatchOperationType, 
    BatchConfig,
    BatchRequest,
    UnifiedBatchManager,
    BatchMetrics,
    create_vector_batcher,
    batch_insert_vectors
)
from proximadb.models import VectorRecord


class TestBatchConfig(BaseProximaDBTest):
    """Test batch configuration validation"""
    
    def test_default_config(self):
        """Test default configuration values"""
        config = BatchConfig()
        
        assert config.max_batch_size == 1000
        assert config.max_wait_time_ms == 100.0
        assert config.strategy == BatchStrategy.HYBRID
        assert config.min_batch_size == 10
        assert config.max_concurrent_batches == 10
        assert config.max_memory_mb == 50.0
        assert config.enable_compression == True
    
    def test_custom_config(self):
        """Test custom configuration"""
        config = BatchConfig(
            max_batch_size=2000,
            max_wait_time_ms=200.0,
            strategy=BatchStrategy.SIZE_BASED,
            min_batch_size=50,
            max_memory_mb=100.0
        )
        
        assert config.max_batch_size == 2000
        assert config.max_wait_time_ms == 200.0
        assert config.strategy == BatchStrategy.SIZE_BASED
        assert config.min_batch_size == 50
        assert config.max_memory_mb == 100.0


class TestRealServerBatching(BaseProximaDBTest):
    """Test real server batch operations"""
    
    def test_batch_vector_insertion_helper(self):
        """Test batching vector insertions using helper function"""
        collection_name = self.create_collection()
        
        # Create test vectors
        vectors = []
        for i in range(100):
            np.random.seed(i)
            vector = np.random.randn(384).astype(np.float32)
            vector = vector / np.linalg.norm(vector)
            
            record = VectorRecord(
                id=f"batch_vec_{i:04d}",
                vector=vector.tolist(),
                metadata={"batch_test": True, "index": i}
            )
            vectors.append(record)
        
        # Batch insert using helper
        results = batch_insert_vectors(
            client=self.rest_client,
            collection_id=collection_name,
            vectors=vectors,
            batch_size=25
        )
        
        # Verify results
        assert len(results) == 4  # 100/25 = 4 batches
        assert all(r.success if hasattr(r, "success") else r.get("success", False) for r in results)
        
        # Wait for indexing
        self.wait_for_indexing()
        
        # Verify with search
        query_vector = vectors[0].vector
        search_results = self.rest_client.search(
            collection_name,
            query_vector,
            top_k=10
        )
        
        self.verify_search_results(search_results, 10)
        assert search_results[0]["id"] == "batch_vec_0000"
    
    def test_concurrent_batch_insertions(self):
        """Test concurrent batch processing with real server"""
        collection_name = self.create_collection()
        
        # Track results
        insert_results = []
        
        def worker(worker_id, count):
            """Worker thread to insert vectors"""
            local_results = []
            vectors = []
            
            for i in range(count):
                np.random.seed(worker_id * 1000 + i)
                vector = np.random.randn(384).astype(np.float32)
                vector = vector / np.linalg.norm(vector)
                
                record = VectorRecord(
                    id=f"concurrent_{worker_id}_{i:04d}",
                    vector=vector.tolist(),
                    metadata={"worker": worker_id}
                )
                vectors.append(record)
            
            # Batch insert for this worker
            try:
                batch_results = batch_insert_vectors(
                    client=self.rest_client,
                    collection_id=collection_name,
                    vectors=vectors,
                    batch_size=10
                )
                local_results.extend([(10, True) for _ in batch_results])
            except Exception as e:
                local_results.append((count, False))
            
            return local_results
        
        # Run concurrent workers
        with ThreadPoolExecutor(max_workers=4) as executor:
            futures = []
            for worker_id in range(4):
                future = executor.submit(worker, worker_id, 25)
                futures.append(future)
            
            for future in as_completed(futures):
                insert_results.extend(future.result())
        
        # Verify results
        total_vectors = sum(count for count, _ in insert_results)
        successful = sum(1 for _, success in insert_results if success)
        
        assert total_vectors == 100
        assert successful == len(insert_results)
        
        # Check final state
        self.wait_for_indexing()
        
        # Verify with search
        results = self.rest_client.search(
            collection_name,
            [0.1] * 384,
            top_k=10
        )
        
        assert len(results) > 0
    
    def test_different_batch_sizes(self):
        """Test different batch sizes with real server"""
        collection_name = self.create_collection()
        
        # Test different batch sizes
        test_cases = [
            (50, 10),   # 50 vectors, batch size 10
            (30, 15),   # 30 vectors, batch size 15  
            (25, 25),   # 25 vectors, batch size 25
        ]
        
        for total_vectors, batch_size in test_cases:
            # Create vectors for this test
            vectors = []
            for i in range(total_vectors):
                np.random.seed(i + total_vectors)
                vector = np.random.randn(384).astype(np.float32)
                vector = vector / np.linalg.norm(vector)
                
                record = VectorRecord(
                    id=f"size_test_{batch_size}_{i:04d}",
                    vector=vector.tolist(),
                    metadata={"batch_size": batch_size, "total": total_vectors}
                )
                vectors.append(record)
            
            # Batch insert
            results = batch_insert_vectors(
                client=self.rest_client,
                collection_id=collection_name,
                vectors=vectors,
                batch_size=batch_size
            )
            
            # Verify results
            expected_batches = (total_vectors + batch_size - 1) // batch_size
            assert len(results) == expected_batches
            assert all(r.success if hasattr(r, "success") else r.get("success", False) for r in results)
        
        # Wait for indexing
        self.wait_for_indexing()
        
        # Verify total insertion
        all_results = self.rest_client.search(
            collection_name,
            [0.1] * 384,
            top_k=100  # Get many results
        )
        
        # Should have inserted all vectors from all test cases
        assert len(all_results) >= sum(total for total, _ in test_cases)


class TestBatchMetrics(BaseProximaDBTest):
    """Test batch metrics collection"""
    
    def test_metrics_creation(self):
        """Test that metrics objects are created correctly"""
        config = BatchConfig(
            max_batch_size=20,
            strategy=BatchStrategy.SIZE_BASED
        )
        manager = UnifiedBatchManager(config)
        
        # Get metrics should work
        all_metrics = manager.get_all_metrics()
        assert isinstance(all_metrics, dict)
    
    def test_batch_config_validation(self):
        """Test batch configuration validation"""
        # Test valid configurations
        valid_configs = [
            BatchConfig(strategy=BatchStrategy.SIZE_BASED),
            BatchConfig(strategy=BatchStrategy.HYBRID),
            BatchConfig(max_batch_size=500, min_batch_size=10),
            BatchConfig(max_wait_time_ms=200.0)
        ]
        
        for config in valid_configs:
            assert config.max_batch_size >= config.min_batch_size
            assert config.max_wait_time_ms > 0
            assert config.max_memory_mb > 0


class TestBatchHelpers(BaseProximaDBTest):
    """Test batch helper functions"""
    
    def test_create_vector_batcher(self):
        """Test vector batcher creation helper"""
        collection_name = self.create_collection()
        
        # Create batcher with helper
        batcher = create_vector_batcher(
            client=self.rest_client,
            collection_id=collection_name,
            max_batch_size=30
        )
        
        assert batcher is not None
        assert batcher.config.max_batch_size == 30
        assert hasattr(batcher, 'get_metrics')
        
        # Get metrics should work
        metrics = batcher.get_metrics()
        assert isinstance(metrics, BatchMetrics)
    
    def test_batch_insert_vectors_helper(self):
        """Test batch insert helper function"""
        collection_name = self.create_collection()
        
        # Create test vectors
        vectors = []
        for i in range(75):
            np.random.seed(i)
            vector = np.random.randn(384).tolist()
            
            record = VectorRecord(
                id=f"helper_vec_{i:04d}",
                vector=vector,
                metadata={"helper_test": True}
            )
            vectors.append(record)
        
        # Use helper to batch insert
        results = batch_insert_vectors(
            client=self.rest_client,
            collection_id=collection_name,
            vectors=vectors,
            batch_size=25
        )
        
        # Verify results
        assert len(results) == 3  # 75 vectors / 25 batch size = 3 batches
        assert all(r.success if hasattr(r, "success") else r.get("success", False) for r in results)
        
        # Verify insertion
        self.wait_for_indexing()
        
        search_results = self.rest_client.search(
            collection_name,
            vectors[0].vector,
            top_k=5
        )
        
        assert len(search_results) == 5
        assert search_results[0]["id"] == "helper_vec_0000"


class TestGRPCBatching(BaseProximaDBTest):
    """Test gRPC protocol batching"""
    
    def test_grpc_batch_insertion(self):
        """Test batching with gRPC client"""
        collection_name = self.create_collection(client=self.grpc_client)
        
        # Create test vectors
        vectors = []
        for i in range(50):
            np.random.seed(i)
            vector = np.random.randn(384).astype(np.float32)
            vector = vector / np.linalg.norm(vector)
            
            record = VectorRecord(
                id=f"grpc_batch_{i:04d}",
                vector=vector.tolist(),
                metadata={"protocol": "grpc", "index": i}
            )
            vectors.append(record)
        
        # Batch insert using gRPC
        results = batch_insert_vectors(
            client=self.grpc_client,
            collection_id=collection_name,
            vectors=vectors,
            batch_size=10
        )
        
        # Verify results
        assert len(results) == 5  # 50/10 = 5 batches
        assert all(r.success for r in results)
        
        # Wait for indexing
        self.wait_for_indexing()
        
        # Verify with search
        query_vector = vectors[0].vector
        search_results = self.grpc_client.search(
            collection_name,
            query_vector,
            top_k=10
        )
        
        # gRPC returns different format, adapt verification
        results_data = search_results.results if hasattr(search_results, 'results') else search_results
        
        assert len(results_data) == 10
        assert results_data[0].id == "grpc_batch_0000"
    
    def test_cross_protocol_batching(self):
        """Test batching across different protocols"""
        # Create collections on both protocols
        rest_collection = self.create_collection(client=self.rest_client)
        grpc_collection = self.create_collection(client=self.grpc_client)
        
        # Create vectors for both
        vectors = []
        for i in range(30):
            np.random.seed(i)
            vector = np.random.randn(384).astype(np.float32)
            vector = vector / np.linalg.norm(vector)
            
            record = VectorRecord(
                id=f"cross_proto_{i:04d}",
                vector=vector.tolist(),
                metadata={"test": "cross_protocol"}
            )
            vectors.append(record)
        
        # Insert via REST
        rest_results = batch_insert_vectors(
            client=self.rest_client,
            collection_id=rest_collection,
            vectors=vectors,
            batch_size=15
        )
        
        # Insert via gRPC
        grpc_results = batch_insert_vectors(
            client=self.grpc_client,
            collection_id=grpc_collection,
            vectors=vectors,
            batch_size=15
        )
        
        # Verify both succeeded
        assert len(rest_results) == 2
        assert len(grpc_results) == 2
        assert all(r.success if hasattr(r, "success") else r.get("success", False) for r in rest_results)
        assert all(r.success for r in grpc_results)
        
        # Wait for indexing
        self.wait_for_indexing()
        
        # Verify both collections have the data
        rest_search = self.rest_client.search(
            rest_collection,
            vectors[0].vector,
            top_k=5
        )
        
        grpc_search = self.grpc_client.search(
            grpc_collection,
            vectors[0].vector,
            top_k=5
        )
        
        assert len(rest_search) == 5
        # Handle different gRPC response format
        grpc_results_data = grpc_search.results if hasattr(grpc_search, 'results') else grpc_search
        assert len(grpc_results_data) == 5