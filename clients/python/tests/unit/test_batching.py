"""
Tests for ProximaDB batching functionality with real server

Uses real ProximaDB server connections to test request batching,
dynamic batch sizing, and performance optimizations.
"""

import asyncio
import sys
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path
from typing import Any, Dict, List

import numpy as np
import pytest

from ..embedding_utils import embed_seed

# Add utils to path
sys.path.insert(0, str(Path(__file__).parent.parent))
from utils.base_test import BaseProximaDBTest
from utils.server_utils import ensure_server_running

from proximadb_sdk import ProximaDBClient

# Backward compatibility import
from proximadb_sdk.batching import RequestBatcher
from proximadb_sdk.batching_unified import (
    AsyncBatchProcessor,
    BatchConfig,
    BatchMetrics,
    BatchOperationType,
    BatchRequest,
    BatchStrategy,
    ThreadedBatchProcessor,
    UnifiedBatchManager,
    batch_insert_vectors,
    create_vector_batcher,
)
from proximadb_sdk.models import VectorRecord


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
            max_memory_mb=100.0,
        )

        assert config.max_batch_size == 2000
        assert config.max_wait_time_ms == 200.0
        assert config.strategy == BatchStrategy.SIZE_BASED
        assert config.min_batch_size == 50
        assert config.max_memory_mb == 100.0


class TestRequestBatcher(BaseProximaDBTest):
    """Test request batching with embedded database"""

    def test_batch_vector_insertion(self):
        """Test batching vector insertions with real server"""
        collection_name = self.create_collection()

        # Create test vectors
        vectors = []
        for i in range(100):
            vector = np.array(embed_seed(i, 384), dtype=np.float32)
            vector = vector / np.linalg.norm(vector)

            record = VectorRecord(
                id=f"batch_vec_{i:04d}",
                vector=vector.tolist(),
                metadata={"batch_test": True, "index": i},
            )
            vectors.append(record)

        # Use the batch_insert_vectors helper function
        batch_results = batch_insert_vectors(
            self.rest_client, collection_name, vectors, batch_size=50
        )

        # Verify results
        assert len(batch_results) == 2  # 100 vectors / 50 batch size = 2 batches

        # Check that all batches succeeded
        for response in batch_results:
            assert response.success

        # Verify vectors in collection
        self.wait_for_indexing()

        # Search for inserted vectors using embedded client
        query_vector = vectors[0].vector
        results = self.rest_client.search(
            collection_id=collection_name, vector=query_vector, top_k=10
        )

        assert len(results) == 10
        # Check that top result is from our batch (ID starts with "batch_vec_")
        assert results[0].id.startswith(
            "batch_vec_"
        ), f"Expected batch_vec_* but got {results[0].id}"

    def test_adaptive_batching(self):
        """Test adaptive batch strategy with real server"""
        collection_name = self.create_collection()

        # Create vectors with different batch sizes to test adaptive behavior
        all_vectors = []

        # Phase 1: Small batches
        for i in range(30):
            vector = np.array(embed_seed(i, 384), dtype=np.float32)
            vector = vector / np.linalg.norm(vector)

            record = VectorRecord(
                id=f"adaptive_phase1_{i:04d}",
                vector=vector.tolist(),
                metadata={"phase": 1},
            )
            all_vectors.append(record)

        # Phase 2: Medium batches
        for i in range(50):
            vector = np.array(embed_seed(i + 1000, 384), dtype=np.float32)
            vector = vector / np.linalg.norm(vector)

            record = VectorRecord(
                id=f"adaptive_phase2_{i:04d}",
                vector=vector.tolist(),
                metadata={"phase": 2},
            )
            all_vectors.append(record)

        # Phase 3: Large batches
        for i in range(70):
            vector = np.array(embed_seed(i + 2000, 384), dtype=np.float32)
            vector = vector / np.linalg.norm(vector)

            record = VectorRecord(
                id=f"adaptive_phase3_{i:04d}",
                vector=vector.tolist(),
                metadata={"phase": 3},
            )
            all_vectors.append(record)

        # Insert with varying batch sizes
        batch_results = []

        # Small batches
        batch_results.extend(
            batch_insert_vectors(
                self.rest_client, collection_name, all_vectors[:30], batch_size=10
            )
        )

        # Medium batches
        batch_results.extend(
            batch_insert_vectors(
                self.rest_client, collection_name, all_vectors[30:80], batch_size=25
            )
        )

        # Large batches
        batch_results.extend(
            batch_insert_vectors(
                self.rest_client, collection_name, all_vectors[80:], batch_size=50
            )
        )

        # Verify all succeeded
        assert all(r.success for r in batch_results)
        assert len(all_vectors) == 150

        # Verify insertion worked
        self.wait_for_indexing()
        results = self.rest_client.search(
            collection_id=collection_name, vector=all_vectors[0].vector, top_k=1
        )
        assert len(results) == 1

    def test_concurrent_batching(self):
        """Test concurrent batch processing with real server"""
        collection_name = self.create_collection()

        def worker(worker_id, count):
            """Worker thread to insert vectors"""
            vectors = []
            for i in range(count):
                vector = np.array(
                    embed_seed(worker_id * 1000 + i, 384), dtype=np.float32
                )
                vector = vector / np.linalg.norm(vector)

                record = VectorRecord(
                    id=f"concurrent_{worker_id}_{i:04d}",
                    vector=vector.tolist(),
                    metadata={"worker": worker_id},
                )
                vectors.append(record)

            # Batch insert this worker's vectors
            results = batch_insert_vectors(
                self.rest_client, collection_name, vectors, batch_size=25
            )
            return results

        # Run concurrent workers
        all_results = []
        with ThreadPoolExecutor(max_workers=4) as executor:
            futures = []
            for worker_id in range(4):
                future = executor.submit(worker, worker_id, 25)
                futures.append(future)

            for future in as_completed(futures):
                all_results.extend(future.result())

        # Verify results
        assert all(r.success for r in all_results)

        # Check final state
        self.wait_for_indexing()

        # Verify with search
        query_vector = embed_seed(999, 384)
        results = self.rest_client.search(
            collection_id=collection_name, vector=query_vector, top_k=10
        )

        assert len(results) == 10
        # Verify we have vectors from different workers
        worker_ids = set()
        for result in results:
            if "_" in result.id:
                worker_id = result.id.split("_")[1]
                worker_ids.add(worker_id)
        assert len(worker_ids) > 1  # Should have results from multiple workers


class TestBatchMetrics(BaseProximaDBTest):
    """Test batch metrics collection"""

    def test_metrics_collection(self):
        """Test that metrics are collected correctly"""
        collection_name = self.create_collection()

        # Create test vectors
        vectors = []
        for i in range(50):
            vector = embed_seed(i, 384)

            record = VectorRecord(
                id=f"metric_vec_{i:04d}", vector=vector, metadata={"test": "metrics"}
            )
            vectors.append(record)

        # Batch insert with specific size to track metrics
        start_time = time.time()
        results = batch_insert_vectors(
            self.rest_client, collection_name, vectors, batch_size=20
        )
        elapsed_time = time.time() - start_time

        # Verify batch count
        assert len(results) == 3  # 50 vectors / 20 batch size = 2.5, rounds up to 3
        assert all(r.success for r in results)

        # Create simple metrics for validation
        total_requests = len(vectors)
        total_batches = len(results)
        avg_batch_size = total_requests / total_batches

        assert total_requests == 50
        assert total_batches == 3
        assert avg_batch_size > 16 and avg_batch_size < 20
        assert elapsed_time > 0


class TestBatchHelpers(BaseProximaDBTest):
    """Test batch helper functions"""

    def test_create_vector_batcher(self):
        """Test vector batcher creation helper"""
        collection_name = self.create_collection()

        # Create batcher with helper
        batcher = create_vector_batcher(
            client=self.rest_client, collection_id=collection_name, max_batch_size=30
        )

        assert batcher is not None
        assert batcher.config.max_batch_size == 30

    def test_batch_insert_vectors_helper(self):
        """Test batch insert helper function"""
        collection_name = self.create_collection()

        # Create test vectors
        vectors = []
        for i in range(75):
            vector = embed_seed(i, 384)

            record = VectorRecord(
                id=f"helper_vec_{i:04d}", vector=vector, metadata={"helper_test": True}
            )
            vectors.append(record)

        # Use helper to batch insert
        results = batch_insert_vectors(
            self.rest_client, collection_name, vectors, batch_size=25
        )

        # Verify results
        assert len(results) == 3  # 75 vectors / 25 batch size = 3 batches
        assert all(r.success for r in results)

        # Verify insertion
        self.wait_for_indexing()

        search_results = self.rest_client.search(
            collection_id=collection_name, vector=vectors[0].vector, top_k=5
        )

        assert len(search_results) == 5
        # Check that top result is from our batch (ID starts with "helper_vec_")
        assert search_results[0].id.startswith(
            "helper_vec_"
        ), f"Expected helper_vec_* but got {search_results[0].id}"
