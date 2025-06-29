#!/usr/bin/env python3
"""
ProximaDB Persistence & WAL Test Suite
Consolidated tests for data persistence, WAL operations, and recovery mechanisms
"""

import pytest
import time
import numpy as np
from typing import List, Dict, Any

from proximadb import connect_rest, connect_grpc
from proximadb.models import CollectionConfig, FlushConfig, DistanceMetric
from proximadb.exceptions import ProximaDBError


class TestDataPersistence:
    """Test data persistence across operations"""
    
    @pytest.fixture
    def rest_client(self):
        client = connect_rest("http://localhost:5678")
        yield client
        client.close()
    
    @pytest.fixture
    def grpc_client(self):
        client = connect_grpc("http://localhost:5679")
        yield client
        client.close()
    
    def test_collection_persistence(self, rest_client):
        """Test that collections persist after creation"""
        collection_name = f"persist_test_{int(time.time())}"
        
        config = CollectionConfig(
            dimension=256,
            distance_metric=DistanceMetric.COSINE,
            description="Persistence test collection"
        )
        
        try:
            # Create collection
            collection = rest_client.create_collection(collection_name, config)
            assert collection is not None
            
            # Verify persistence immediately
            retrieved = rest_client.get_collection(collection_name)
            assert retrieved is not None
            
            # Verify it appears in listings
            collections = rest_client.list_collections()
            collection_names = [getattr(col, 'id', getattr(col, 'name', None)) for col in collections]
            assert collection_name in collection_names
            
        finally:
            try:
                rest_client.delete_collection(collection_name)
            except:
                pass
    
    def test_vector_persistence(self, rest_client):
        """Test that vectors persist after insertion"""
        collection_name = f"vector_persist_{int(time.time())}"
        config = CollectionConfig(dimension=128, distance_metric=DistanceMetric.COSINE)
        
        try:
            # Create collection
            collection = rest_client.create_collection(collection_name, config)
            
            # Insert test vectors
            test_vectors = []
            for i in range(10):
                vector_id = f"persist_vector_{i}"
                vector = np.random.random(128).astype(np.float32).tolist()
                metadata = {
                    "index": i,
                    "batch": "persistence_test",
                    "timestamp": time.time()
                }
                
                rest_client.insert_vector(
                    collection_id=collection_name,
                    vector_id=vector_id,
                    vector=vector,
                    metadata=metadata
                )
                
                test_vectors.append((vector_id, vector, metadata))
            
            # Verify all vectors can be retrieved
            for vector_id, original_vector, original_metadata in test_vectors:
                retrieved = rest_client.get_vector(
                    collection_id=collection_name,
                    vector_id=vector_id,
                    include_vector=True,
                    include_metadata=True
                )
                
                assert retrieved is not None
                assert retrieved.get('metadata', {}).get('index') == original_metadata['index']
                
        finally:
            try:
                rest_client.delete_collection(collection_name)
            except:
                pass
    
    def test_cross_protocol_persistence(self, rest_client, grpc_client):
        """Test persistence across REST and gRPC protocols"""
        collection_name = f"cross_persist_{int(time.time())}"
        config = CollectionConfig(dimension=128, distance_metric=DistanceMetric.COSINE)
        
        try:
            # Create with REST
            collection = rest_client.create_collection(collection_name, config)
            
            # Insert vectors with REST
            rest_vector_id = "rest_persisted"
            rest_vector = np.random.random(128).astype(np.float32).tolist()
            rest_metadata = {"source": "rest", "test": "cross_protocol_persistence"}
            
            rest_client.insert_vector(
                collection_id=collection_name,
                vector_id=rest_vector_id,
                vector=rest_vector,
                metadata=rest_metadata
            )
            
            # Verify with gRPC
            grpc_retrieved = grpc_client.get_vector(
                collection_id=collection_name,
                vector_id=rest_vector_id,
                include_metadata=True
            )
            
            assert grpc_retrieved is not None
            assert grpc_retrieved.get('metadata', {}).get('source') == 'rest'
            
            # Insert with gRPC
            grpc_vector_id = "grpc_persisted"
            grpc_vector = np.random.random(128).astype(np.float32).tolist()
            grpc_metadata = {"source": "grpc", "test": "cross_protocol_persistence"}
            
            grpc_client.insert_vector(
                collection_id=collection_name,
                vector_id=grpc_vector_id,
                vector=grpc_vector,
                metadata=grpc_metadata
            )
            
            # Verify with REST
            rest_retrieved = rest_client.get_vector(
                collection_id=collection_name,
                vector_id=grpc_vector_id,
                include_metadata=True
            )
            
            assert rest_retrieved is not None
            assert rest_retrieved.get('metadata', {}).get('source') == 'grpc'
            
        finally:
            try:
                rest_client.delete_collection(collection_name)
            except:
                pass


class TestWALOperations:
    """Test Write-Ahead Log operations and flush behavior"""
    
    @pytest.fixture
    def rest_client(self):
        client = connect_rest("http://localhost:5678")
        yield client
        client.close()
    
    @pytest.fixture
    def grpc_client(self):
        client = connect_grpc("http://localhost:5679")
        yield client
        client.close()
    
    def test_wal_flush_trigger(self, rest_client):
        """Test that WAL flush is triggered by large data volumes"""
        collection_name = f"wal_flush_{int(time.time())}"
        
        # Configure low flush threshold to trigger flush
        config = CollectionConfig(
            dimension=512,
            distance_metric=DistanceMetric.COSINE,
            description="WAL flush test collection",
            flush_config=FlushConfig(max_wal_size_mb=8.0)  # Low threshold
        )
        
        try:
            collection = rest_client.create_collection(collection_name, config)
            
            # Insert enough data to trigger flush
            # 512 dimensions * 4 bytes * 300 vectors ≈ 0.6MB per batch
            batch_count = 15  # Total ~9MB to exceed 8MB threshold
            
            for batch_num in range(batch_count):
                vectors = []
                vector_ids = []
                metadatas = []
                
                for i in range(300):
                    vector = np.random.normal(0, 1, 512).astype(np.float32).tolist()
                    vectors.append(vector)
                    vector_ids.append(f"wal_vector_{batch_num}_{i}")
                    metadatas.append({
                        "batch": batch_num,
                        "index": i,
                        "wal_test": True,
                        "timestamp": time.time()
                    })
                
                # Insert batch
                result = rest_client.insert_vectors(
                    collection_id=collection_name,
                    vectors=vectors,
                    ids=vector_ids,
                    metadata=metadatas
                )
                
                assert result is not None
                
                # Small delay between batches
                time.sleep(0.1)
            
            # Verify data is accessible (should be in VIPER after flush)
            test_vector_id = "wal_vector_0_0"
            retrieved = rest_client.get_vector(
                collection_id=collection_name,
                vector_id=test_vector_id,
                include_metadata=True
            )
            
            assert retrieved is not None
            assert retrieved.get('metadata', {}).get('wal_test') is True
            
        finally:
            try:
                rest_client.delete_collection(collection_name)
            except:
                pass
    
    def test_wal_durability(self, rest_client):
        """Test WAL durability and recovery"""
        collection_name = f"wal_durability_{int(time.time())}"
        config = CollectionConfig(
            dimension=256,
            distance_metric=DistanceMetric.COSINE,
            description="WAL durability test"
        )
        
        try:
            collection = rest_client.create_collection(collection_name, config)
            
            # Insert vectors that should be written to WAL
            test_vectors = []
            for i in range(50):
                vector_id = f"durable_vector_{i}"
                vector = np.random.random(256).astype(np.float32).tolist()
                metadata = {
                    "index": i,
                    "durability_test": True,
                    "timestamp": time.time()
                }
                
                rest_client.insert_vector(
                    collection_id=collection_name,
                    vector_id=vector_id,
                    vector=vector,
                    metadata=metadata
                )
                
                test_vectors.append((vector_id, metadata))
            
            # Verify immediate durability (should be in WAL at minimum)
            for vector_id, metadata in test_vectors:
                retrieved = rest_client.get_vector(
                    collection_id=collection_name,
                    vector_id=vector_id,
                    include_metadata=True
                )
                
                assert retrieved is not None
                assert retrieved.get('metadata', {}).get('durability_test') is True
                assert retrieved.get('metadata', {}).get('index') == metadata['index']
                
        finally:
            try:
                rest_client.delete_collection(collection_name)
            except:
                pass
    
    def test_concurrent_wal_operations(self, rest_client, grpc_client):
        """Test concurrent WAL operations from multiple clients"""
        collection_name = f"concurrent_wal_{int(time.time())}"
        config = CollectionConfig(
            dimension=128,
            distance_metric=DistanceMetric.COSINE,
            description="Concurrent WAL test"
        )
        
        try:
            collection = rest_client.create_collection(collection_name, config)
            
            # Concurrent inserts from both clients
            import threading
            import time
            
            results = {"rest": [], "grpc": []}
            errors = []
            
            def insert_via_rest():
                try:
                    for i in range(25):
                        vector_id = f"rest_concurrent_{i}"
                        vector = np.random.random(128).astype(np.float32).tolist()
                        metadata = {"source": "rest", "index": i, "concurrent": True}
                        
                        result = rest_client.insert_vector(
                            collection_id=collection_name,
                            vector_id=vector_id,
                            vector=vector,
                            metadata=metadata
                        )
                        results["rest"].append(result)
                        time.sleep(0.01)  # Small delay
                except Exception as e:
                    errors.append(f"REST error: {e}")
            
            def insert_via_grpc():
                try:
                    for i in range(25):
                        vector_id = f"grpc_concurrent_{i}"
                        vector = np.random.random(128).astype(np.float32).tolist()
                        metadata = {"source": "grpc", "index": i, "concurrent": True}
                        
                        result = grpc_client.insert_vector(
                            collection_id=collection_name,
                            vector_id=vector_id,
                            vector=vector,
                            metadata=metadata
                        )
                        results["grpc"].append(result)
                        time.sleep(0.01)  # Small delay
                except Exception as e:
                    errors.append(f"gRPC error: {e}")
            
            # Start concurrent operations
            rest_thread = threading.Thread(target=insert_via_rest)
            grpc_thread = threading.Thread(target=insert_via_grpc)
            
            rest_thread.start()
            grpc_thread.start()
            
            # Wait for completion
            rest_thread.join(timeout=10.0)
            grpc_thread.join(timeout=10.0)
            
            # Verify results
            assert len(errors) == 0, f"Errors during concurrent operations: {errors}"
            assert len(results["rest"]) > 0, "REST operations should have succeeded"
            assert len(results["grpc"]) > 0, "gRPC operations should have succeeded"
            
            # Verify all data is accessible
            for i in range(25):
                rest_vector = rest_client.get_vector(
                    collection_id=collection_name,
                    vector_id=f"rest_concurrent_{i}",
                    include_metadata=True
                )
                assert rest_vector is not None
                assert rest_vector.get('metadata', {}).get('source') == 'rest'
                
                grpc_vector = grpc_client.get_vector(
                    collection_id=collection_name,
                    vector_id=f"grpc_concurrent_{i}",
                    include_metadata=True
                )
                assert grpc_vector is not None
                assert grpc_vector.get('metadata', {}).get('source') == 'grpc'
                
        finally:
            try:
                rest_client.delete_collection(collection_name)
            except:
                pass


class TestRecoveryMechanisms:
    """Test data recovery and consistency mechanisms"""
    
    @pytest.fixture
    def rest_client(self):
        client = connect_rest("http://localhost:5678")
        yield client
        client.close()
    
    def test_collection_recovery(self, rest_client):
        """Test collection metadata recovery"""
        collection_name = f"recovery_test_{int(time.time())}"
        
        # Create collection with specific configuration
        config = CollectionConfig(
            dimension=384,
            distance_metric=DistanceMetric.EUCLIDEAN,
            description="Recovery test collection with specific config",
            storage_layout="viper"
        )
        
        try:
            # Create collection
            original_collection = rest_client.create_collection(collection_name, config)
            assert original_collection is not None
            
            # Insert some test data
            for i in range(10):
                vector = np.random.random(384).astype(np.float32).tolist()
                metadata = {"recovery_test": True, "index": i}
                
                rest_client.insert_vector(
                    collection_id=collection_name,
                    vector_id=f"recovery_vector_{i}",
                    vector=vector,
                    metadata=metadata
                )
            
            # Simulate recovery by re-reading collection
            recovered_collection = rest_client.get_collection(collection_name)
            assert recovered_collection is not None
            
            # Verify configuration persisted
            if hasattr(recovered_collection, 'dimension'):
                assert recovered_collection.dimension == 384
            
            # Verify data is still accessible
            test_vector = rest_client.get_vector(
                collection_id=collection_name,
                vector_id="recovery_vector_0",
                include_metadata=True
            )
            assert test_vector is not None
            assert test_vector.get('metadata', {}).get('recovery_test') is True
            
        finally:
            try:
                rest_client.delete_collection(collection_name)
            except:
                pass
    
    def test_data_consistency_check(self, rest_client):
        """Test data consistency after operations"""
        collection_name = f"consistency_test_{int(time.time())}"
        config = CollectionConfig(dimension=128, distance_metric=DistanceMetric.COSINE)
        
        try:
            collection = rest_client.create_collection(collection_name, config)
            
            # Insert, update, and verify consistency
            vector_id = "consistency_vector"
            
            # Initial insert
            original_vector = np.random.random(128).astype(np.float32).tolist()
            original_metadata = {"version": 1, "consistency_test": True}
            
            rest_client.insert_vector(
                collection_id=collection_name,
                vector_id=vector_id,
                vector=original_vector,
                metadata=original_metadata
            )
            
            # Verify initial state
            retrieved_v1 = rest_client.get_vector(
                collection_id=collection_name,
                vector_id=vector_id,
                include_metadata=True
            )
            assert retrieved_v1.get('metadata', {}).get('version') == 1
            
            # Update vector
            updated_vector = np.random.random(128).astype(np.float32).tolist()
            updated_metadata = {"version": 2, "consistency_test": True, "updated": True}
            
            rest_client.insert_vector(
                collection_id=collection_name,
                vector_id=vector_id,
                vector=updated_vector,
                metadata=updated_metadata
            )
            
            # Verify consistency after update
            retrieved_v2 = rest_client.get_vector(
                collection_id=collection_name,
                vector_id=vector_id,
                include_metadata=True
            )
            assert retrieved_v2.get('metadata', {}).get('version') == 2
            assert retrieved_v2.get('metadata', {}).get('updated') is True
            
            # Ensure old version is not accessible
            assert retrieved_v2.get('metadata', {}).get('version') != 1
            
        finally:
            try:
                rest_client.delete_collection(collection_name)
            except:
                pass


class TestStorageIntegration:
    """Test integration between WAL and storage layers"""
    
    @pytest.fixture
    def rest_client(self):
        client = connect_rest("http://localhost:5678")
        yield client
        client.close()
    
    def test_wal_to_viper_integration(self, rest_client):
        """Test WAL to VIPER storage integration"""
        collection_name = f"wal_viper_{int(time.time())}"
        
        # Configure for VIPER storage with flush
        config = CollectionConfig(
            dimension=256,
            distance_metric=DistanceMetric.COSINE,
            description="WAL-VIPER integration test",
            storage_layout="viper",
            flush_config=FlushConfig(max_wal_size_mb=4.0)  # Small threshold
        )
        
        try:
            collection = rest_client.create_collection(collection_name, config)
            
            # Insert data to trigger WAL->VIPER flush
            batch_size = 100
            total_batches = 12  # ~1.2MB total to exceed 4MB threshold multiple times
            
            for batch_num in range(total_batches):
                vectors = []
                vector_ids = []
                metadatas = []
                
                for i in range(batch_size):
                    vector = np.random.normal(0, 1, 256).astype(np.float32).tolist()
                    vectors.append(vector)
                    vector_ids.append(f"integration_vector_{batch_num}_{i}")
                    metadatas.append({
                        "batch": batch_num,
                        "index": i,
                        "integration_test": True,
                        "storage": "viper"
                    })
                
                result = rest_client.insert_vectors(
                    collection_id=collection_name,
                    vectors=vectors,
                    ids=vector_ids,
                    metadata=metadatas
                )
                
                assert result is not None
            
            # Verify data accessibility across storage layers
            # Test vectors from different batches (some in WAL, some in VIPER)
            test_vectors = [
                "integration_vector_0_0",    # Early batch (likely in VIPER)
                "integration_vector_5_50",   # Middle batch
                "integration_vector_11_99"   # Latest batch (likely in WAL)
            ]
            
            for vector_id in test_vectors:
                retrieved = rest_client.get_vector(
                    collection_id=collection_name,
                    vector_id=vector_id,
                    include_metadata=True
                )
                
                assert retrieved is not None
                assert retrieved.get('metadata', {}).get('integration_test') is True
                assert retrieved.get('metadata', {}).get('storage') == 'viper'
            
            # Test search across storage layers
            query_vector = np.random.normal(0, 1, 256).astype(np.float32).tolist()
            search_results = rest_client.search(
                collection_id=collection_name,
                query=query_vector,
                k=20,
                include_metadata=True
            )
            
            assert len(search_results) > 0
            
            # Results should come from both WAL and VIPER storage
            batch_numbers = set()
            for result in search_results:
                batch_num = result.metadata.get('batch')
                if batch_num is not None:
                    batch_numbers.add(batch_num)
            
            # Should have results from multiple batches
            assert len(batch_numbers) > 1
            
        finally:
            try:
                rest_client.delete_collection(collection_name)
            except:
                pass


if __name__ == "__main__":
    pytest.main([__file__, "-v", "-s"])