"""
Tests for REST request batching functionality using unified batching system
"""

import pytest
import time
import threading
from unittest.mock import Mock, patch, MagicMock
from collections import deque

from proximadb.batching_unified import (
    ThreadedBatchProcessor,
    BatchConfig,
    BatchStrategy,
    BatchRequest,
    BatchMetrics,
    BatchOperationType
)
from proximadb.exceptions import BatchError, ProximaDBError
from proximadb.models import VectorRecord


class TestBatchConfig:
    """Test batch configuration"""
    
    def test_default_config(self):
        """Test default configuration values"""
        config = BatchConfig()
        
        assert config.max_batch_size == 1000
        assert config.min_batch_size == 10
        assert config.max_wait_time_ms == 100.0
        assert config.strategy == BatchStrategy.HYBRID
        assert config.max_memory_mb == 50.0
        assert config.enable_compression == True
    
    def test_custom_config(self):
        """Test custom configuration values"""
        config = BatchConfig(
            max_batch_size=500,
            min_batch_size=5,
            max_wait_time_ms=200.0,
            strategy=BatchStrategy.SIZE_BASED,
            max_memory_mb=100.0,
            enable_compression=False
        )
        
        assert config.max_batch_size == 500
        assert config.min_batch_size == 5
        assert config.max_wait_time_ms == 200.0
        assert config.strategy == BatchStrategy.SIZE_BASED
        assert config.max_memory_mb == 100.0
        assert config.enable_compression == False


class TestThreadedBatchProcessor:
    """Test threaded batch processor (used for REST protocol)"""
    
    def setup_method(self):
        """Setup test fixtures"""
        self.mock_execute_fn = Mock()
        self.mock_execute_fn.return_value = {"status": "success"}
        
        self.config = BatchConfig(
            max_batch_size=5,
            max_wait_time_ms=100.0,
            strategy=BatchStrategy.SIZE_BASED
        )
        
        self.processor = ThreadedBatchProcessor(self.config, self.mock_execute_fn)
    
    def teardown_method(self):
        """Cleanup after tests"""
        if hasattr(self, 'processor'):
            self.processor.stop()
    
    def test_processor_creation(self):
        """Test processor can be created"""
        assert self.processor is not None
        assert self.processor.config == self.config
        assert self.processor.execute_batch_fn == self.mock_execute_fn
    
    def test_batch_request_creation(self):
        """Test batch request creation"""
        request = BatchRequest(
            operation=BatchOperationType.INSERT_VECTORS,
            collection_id="test_collection",
            data=[{"vector": [1, 2, 3], "id": "test_1"}],
            priority=1
        )
        
        assert request.operation == BatchOperationType.INSERT_VECTORS
        assert request.collection_id == "test_collection"
        assert request.data is not None
        assert request.priority == 1
        assert request.request_id is not None
        assert len(request.request_id) > 0
    
    def test_batch_metrics(self):
        """Test batch metrics initialization"""
        metrics = BatchMetrics()
        
        assert metrics.total_requests == 0
        assert metrics.batched_requests == 0
        assert metrics.total_batches == 0
        assert metrics.avg_batch_size == 0.0
        assert metrics.total_latency_ms == 0.0
        assert metrics.avg_latency_ms == 0.0
        assert metrics.throughput_qps == 0.0
        assert metrics.cache_hit_ratio == 0.0
        assert metrics.memory_usage_mb == 0.0
    
    def test_processor_start_stop(self):
        """Test processor lifecycle"""
        # Start processor
        self.processor.start()
        assert self.processor._running == True
        
        # Stop processor
        self.processor.stop()
        assert self.processor._running == False
    
    def test_memory_size_estimation(self):
        """Test memory size estimation for requests"""
        # Test with vector record
        vector_record = VectorRecord(
            id="test_1",
            vector=[1.0, 2.0, 3.0, 4.0],
            metadata={"category": "test"}
        )
        
        request = BatchRequest(
            operation=BatchOperationType.INSERT_VECTORS,
            collection_id="test_collection",
            data=[vector_record]
        )
        
        estimated_size = self.processor._estimate_request_size(request)
        assert estimated_size > 0
        assert estimated_size < 1  # Should be small for single vector
    
    def test_batch_strategy_enum_values(self):
        """Test all batch strategy values are available"""
        strategies = [
            BatchStrategy.SIZE_BASED,
            BatchStrategy.TIME_BASED,
            BatchStrategy.ADAPTIVE,
            BatchStrategy.HYBRID,
            BatchStrategy.IMMEDIATE
        ]
        
        for strategy in strategies:
            config = BatchConfig(strategy=strategy)
            assert config.strategy == strategy
    
    def test_batch_operation_types(self):
        """Test batch operation type enum values"""
        operations = [
            BatchOperationType.INSERT_VECTORS,
            BatchOperationType.UPSERT_VECTORS,
            BatchOperationType.DELETE_VECTORS,
            BatchOperationType.SEARCH_VECTORS,
            BatchOperationType.GET_VECTORS,
            BatchOperationType.UPDATE_VECTORS
        ]
        
        for operation in operations:
            request = BatchRequest(operation=operation, collection_id="test")
            assert request.operation == operation


class TestBatchingIntegration:
    """Test integration with unified batching system"""
    
    def test_backward_compatibility(self):
        """Test that old REST batching imports still work"""
        # This should work due to backward compatibility
        from proximadb.batching import RequestBatcher
        
        config = BatchConfig()
        batcher = RequestBatcher(config)
        assert batcher is not None
    
    def test_unified_batch_manager(self):
        """Test unified batch manager can create REST processors"""
        from proximadb.batching_unified import UnifiedBatchManager
        
        config = BatchConfig()
        manager = UnifiedBatchManager(config)
        
        # Mock execute function
        mock_execute = Mock()
        mock_execute.return_value = {"status": "success"}
        
        # Get REST processor (threaded)
        processor = manager.get_processor('rest', mock_execute)
        assert isinstance(processor, ThreadedBatchProcessor)
        
        # Cleanup
        manager.stop_all()


def create_rest_batch_processor(config: BatchConfig, execute_fn):
    """Factory function for backward compatibility"""
    return ThreadedBatchProcessor(config, execute_fn)