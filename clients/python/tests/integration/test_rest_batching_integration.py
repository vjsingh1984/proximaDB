"""
Integration tests for REST request batching
"""

import pytest
import time
import threading
from unittest.mock import Mock, patch, MagicMock

from proximadb.protocols.rest_sync import ProximaDBClient
from proximadb.batching_unified import BatchConfig, BatchStrategy
from proximadb.config import ClientConfig
from proximadb.exceptions import BatchError


class TestRestBatchingIntegration:
    """Integration tests for REST client with batching enabled"""
    
    @pytest.fixture
    def config(self):
        """Client configuration"""
        return ClientConfig(
            url="http://localhost:5678",
            timeout=30.0
        )
    
    @pytest.fixture
    def batch_config(self):
        """Batch configuration for testing"""
        return BatchConfig(
            max_batch_size=10,
            max_wait_time_ms=100.0,
            strategy=BatchStrategy.HYBRID
        )
    
    @pytest.fixture
    def mock_http_client(self):
        """Mock HTTP client for testing"""
        client = Mock()
        
        # Mock successful responses
        mock_response = Mock()
        mock_response.status_code = 200
        mock_response.json.return_value = {
            "success": True,
            "count": 5,
            "errors": [],
            "duration_ms": 25.0
        }
        
        client.post.return_value = mock_response
        client.get.return_value = mock_response
        client.delete.return_value = mock_response
        client.put.return_value = mock_response
        
        return client
    
    def test_client_initialization_with_batching(self, config, batch_config):
        """Test client initialization with batching enabled"""
        with patch('proximadb.protocols.rest_sync.ProximaDBClient._create_http_client'):
            client = ProximaDBClient(
                config=config,
                enable_batching=True,
                batch_config=batch_config
            )
            
            assert client.enable_batching is True
            assert client._batch_processor is not None
            assert client._batch_processor.config == batch_config
            
            client.close()
    
    def test_client_initialization_without_batching(self, config):
        """Test client initialization without batching"""
        with patch('proximadb.protocols.rest_sync.ProximaDBClient._create_http_client'):
            client = ProximaDBClient(config=config)
            
            assert client.enable_batching is False
            assert client._batch_processor is None
            
            client.close()
    
    def test_batched_insert_vectors(self, config, batch_config, mock_http_client):
        """Test batched vector insertion"""
        with patch('proximadb.protocols.rest_sync.ProximaDBClient._create_http_client', 
                   return_value=mock_http_client):
            
            client = ProximaDBClient(
                config=config,
                enable_batching=True,
                batch_config=batch_config
            )
            
            try:
                callback = Mock()
                
                # Submit batched request
                request_id = client.insert_vectors_batched(
                    collection_id="test_collection",
                    vectors=[[1.0, 2.0], [3.0, 4.0]],
                    ids=["vec_1", "vec_2"],
                    metadata=[{"tag": "test"}, {"tag": "test2"}],
                    callback=callback,
                    priority=1
                )
                
                assert request_id.startswith("req_")
                
                # Wait for processing
                time.sleep(0.5)
                
                # Callback should be called
                callback.assert_called_once()
                
            finally:
                client.close()
    
    def test_batched_upsert_vectors(self, config, batch_config, mock_http_client):
        """Test batched vector upserts"""
        with patch('proximadb.protocols.rest_sync.ProximaDBClient._create_http_client',
                   return_value=mock_http_client):
            
            client = ProximaDBClient(
                config=config,
                enable_batching=True,
                batch_config=batch_config
            )
            
            try:
                callback = Mock()
                
                request_id = client.upsert_vectors_batched(
                    collection_id="test_collection",
                    vectors=[[1.0, 2.0]],
                    ids=["vec_1"],
                    callback=callback
                )
                
                assert request_id.startswith("req_")
                
                # Wait for processing
                time.sleep(0.5)
                
                callback.assert_called_once()
                
            finally:
                client.close()
    
    def test_batched_delete_vectors(self, config, batch_config, mock_http_client):
        """Test batched vector deletions"""
        with patch('proximadb.protocols.rest_sync.ProximaDBClient._create_http_client',
                   return_value=mock_http_client):
            
            client = ProximaDBClient(
                config=config,
                enable_batching=True,
                batch_config=batch_config
            )
            
            try:
                callback = Mock()
                
                request_id = client.delete_vectors_batched(
                    collection_id="test_collection",
                    ids=["vec_1", "vec_2", "vec_3"],
                    callback=callback
                )
                
                assert request_id.startswith("req_")
                
                # Wait for processing
                time.sleep(0.5)
                
                callback.assert_called_once()
                
            finally:
                client.close()
    
    def test_batching_disabled_error(self, config):
        """Test error when trying to use batching when disabled"""
        with patch('proximadb.protocols.rest_sync.ProximaDBClient._create_http_client'):
            client = ProximaDBClient(config=config)  # Batching disabled
            
            try:
                with pytest.raises(RuntimeError, match="Batching is not enabled"):
                    client.insert_vectors_batched(
                        collection_id="test",
                        vectors=[[1.0, 2.0]],
                        ids=["vec_1"]
                    )
                
                with pytest.raises(RuntimeError, match="Batching is not enabled"):
                    client.get_batch_metrics()
                
                with pytest.raises(RuntimeError, match="Batching is not enabled"):
                    client.reset_batch_metrics()
                
            finally:
                client.close()
    
    def test_batch_metrics(self, config, batch_config, mock_http_client):
        """Test batch metrics functionality"""
        with patch('proximadb.protocols.rest_sync.ProximaDBClient._create_http_client',
                   return_value=mock_http_client):
            
            client = ProximaDBClient(
                config=config,
                enable_batching=True,
                batch_config=batch_config
            )
            
            try:
                # Submit some requests
                for i in range(5):
                    client.insert_vectors_batched(
                        collection_id="test",
                        vectors=[[float(i), float(i+1)]],
                        ids=[f"vec_{i}"]
                    )
                
                # Wait for processing
                time.sleep(0.5)
                
                # Get metrics
                metrics = client.get_batch_metrics()
                
                assert isinstance(metrics, dict)
                assert "total_batches" in metrics
                assert "total_requests" in metrics
                assert "avg_batch_size" in metrics
                assert "memory_usage_mb" in metrics
                assert "strategy" in metrics
                
                # Reset metrics
                client.reset_batch_metrics()
                
                # Metrics should be reset
                reset_metrics = client.get_batch_metrics()
                assert reset_metrics["total_batches"] == 0
                assert reset_metrics["total_requests"] == 0
                
            finally:
                client.close()
    
    def test_validation_errors(self, config, batch_config, mock_http_client):
        """Test validation errors in batched operations"""
        with patch('proximadb.protocols.rest_sync.ProximaDBClient._create_http_client',
                   return_value=mock_http_client):
            
            client = ProximaDBClient(
                config=config,
                enable_batching=True,
                batch_config=batch_config
            )
            
            try:
                # Mismatched vectors and IDs
                with pytest.raises(ValueError, match="Number of vectors must match"):
                    client.insert_vectors_batched(
                        collection_id="test",
                        vectors=[[1.0, 2.0], [3.0, 4.0]],
                        ids=["vec_1"]  # Only 1 ID for 2 vectors
                    )
                
                # Mismatched metadata
                with pytest.raises(ValueError, match="Number of metadata items must match"):
                    client.insert_vectors_batched(
                        collection_id="test",
                        vectors=[[1.0, 2.0]],
                        ids=["vec_1"],
                        metadata=[{"tag": "1"}, {"tag": "2"}]  # 2 metadata for 1 vector
                    )
                
            finally:
                client.close()
    
    def test_concurrent_batching(self, config, batch_config, mock_http_client):
        """Test concurrent batching operations"""
        with patch('proximadb.protocols.rest_sync.ProximaDBClient._create_http_client',
                   return_value=mock_http_client):
            
            client = ProximaDBClient(
                config=config,
                enable_batching=True,
                batch_config=batch_config
            )
            
            results = []
            errors = []
            
            def submit_requests():
                try:
                    for i in range(10):
                        request_id = client.insert_vectors_batched(
                            collection_id=f"test_{threading.current_thread().ident}",
                            vectors=[[float(i), float(i+1)]],
                            ids=[f"vec_{i}"]
                        )
                        results.append(request_id)
                except Exception as e:
                    errors.append(e)
            
            try:
                # Run concurrent submissions
                threads = []
                for _ in range(3):
                    thread = threading.Thread(target=submit_requests)
                    threads.append(thread)
                    thread.start()
                
                for thread in threads:
                    thread.join()
                
                # Wait for processing
                time.sleep(1.0)
                
                # Should complete without errors
                assert len(errors) == 0
                assert len(results) == 30  # 3 threads × 10 requests
                
                # All request IDs should be unique
                assert len(set(results)) == len(results)
                
            finally:
                client.close()
    
    def test_context_manager_with_batching(self, config, batch_config, mock_http_client):
        """Test client as context manager with batching"""
        with patch('proximadb.protocols.rest_sync.ProximaDBClient._create_http_client',
                   return_value=mock_http_client):
            
            with ProximaDBClient(
                config=config,
                enable_batching=True,
                batch_config=batch_config
            ) as client:
                assert client.enable_batching is True
                assert client._batch_processor is not None
                
                # Submit request
                request_id = client.insert_vectors_batched(
                    collection_id="test",
                    vectors=[[1.0, 2.0]],
                    ids=["vec_1"]
                )
                
                assert request_id.startswith("req_")
            
            # Should be closed after context manager exit
            assert client._batch_processor is None
    
    def test_different_batch_strategies(self, config, mock_http_client):
        """Test different batching strategies"""
        strategies = [
            BatchStrategy.SIZE_BASED,
            BatchStrategy.TIME_BASED,
            BatchStrategy.ADAPTIVE,
            BatchStrategy.HYBRID
        ]
        
        for strategy in strategies:
            batch_config = BatchConfig(
                max_batch_size=5,
                strategy=strategy
            )
            
            with patch('proximadb.protocols.rest_sync.ProximaDBClient._create_http_client',
                       return_value=mock_http_client):
                
                client = ProximaDBClient(
                    config=config,
                    enable_batching=True,
                    batch_config=batch_config
                )
                
                try:
                    # Submit requests
                    for i in range(3):
                        client.insert_vectors_batched(
                            collection_id="test",
                            vectors=[[float(i)]],
                            ids=[f"vec_{i}"]
                        )
                    
                    # Verify strategy is set correctly
                    metrics = client.get_batch_metrics()
                    assert metrics["strategy"] == strategy.value
                    
                finally:
                    client.close()
    
    def test_priority_handling(self, config, batch_config, mock_http_client):
        """Test priority handling in batched requests"""
        with patch('proximadb.protocols.rest_sync.ProximaDBClient._create_http_client',
                   return_value=mock_http_client):
            
            client = ProximaDBClient(
                config=config,
                enable_batching=True,
                batch_config=batch_config
            )
            
            try:
                # Submit low priority requests
                for i in range(3):
                    client.insert_vectors_batched(
                        collection_id="test",
                        vectors=[[float(i)]],
                        ids=[f"low_{i}"],
                        priority=1
                    )
                
                # Submit high priority request
                high_priority_id = client.insert_vectors_batched(
                    collection_id="test",
                    vectors=[[100.0]],
                    ids=["high_priority"],
                    priority=3
                )
                
                assert high_priority_id.startswith("req_")
                
                # Wait for processing
                time.sleep(0.5)
                
                # Should have processed requests (priority handling tested in unit tests)
                metrics = client.get_batch_metrics()
                assert metrics["total_requests"] > 0
                
            finally:
                client.close()


@pytest.mark.performance
class TestRestBatchingPerformance:
    """Performance tests for REST batching"""
    
    def test_batching_vs_individual_requests(self):
        """Compare batching performance vs individual requests"""
        config = ClientConfig(url="http://localhost:5678")
        
        # Mock HTTP client with realistic delay
        def slow_post(*args, **kwargs):
            time.sleep(0.01)  # 10ms per request
            mock_response = Mock()
            mock_response.status_code = 200
            mock_response.json.return_value = {"success": True, "count": 1}
            return mock_response
        
        mock_http_client = Mock()
        mock_http_client.post.side_effect = slow_post
        
        # Test individual requests
        with patch('proximadb.protocols.rest_sync.ProximaDBClient._create_http_client',
                   return_value=mock_http_client):
            
            individual_client = ProximaDBClient(config=config)
            
            start_time = time.time()
            
            # Simulate individual requests (can't actually call without full setup)
            for _ in range(10):
                mock_http_client.post("/collections/test/vectors", json={"vectors": []})
            
            individual_time = time.time() - start_time
            individual_client.close()
        
        # Test batched requests
        mock_http_client.reset_mock()
        mock_http_client.post.side_effect = slow_post
        
        with patch('proximadb.protocols.rest_sync.ProximaDBClient._create_http_client',
                   return_value=mock_http_client):
            
            batch_config = BatchConfig(max_batch_size=5, max_wait_time_ms=50.0)
            batched_client = ProximaDBClient(
                config=config,
                enable_batching=True,
                batch_config=batch_config
            )
            
            start_time = time.time()
            
            # Submit batched requests
            for i in range(10):
                batched_client.insert_vectors_batched(
                    collection_id="test",
                    vectors=[[float(i)]],
                    ids=[f"vec_{i}"]
                )
            
            # Wait for processing
            time.sleep(1.0)
            
            batched_time = time.time() - start_time
            batched_client.close()
        
        print(f"Individual requests: {individual_time:.3f}s")
        print(f"Batched requests: {batched_time:.3f}s")
        print(f"HTTP calls - Individual: 10, Batched: {mock_http_client.post.call_count}")
        
        # Batching should result in fewer HTTP calls
        assert mock_http_client.post.call_count < 10
        assert mock_http_client.post.call_count >= 2  # At least 2 batches for 10 items


if __name__ == "__main__":
    # Run basic tests
    pytest.main([__file__, "-v"])