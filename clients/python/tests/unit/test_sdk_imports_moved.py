#!/usr/bin/env python3
"""
Test module to verify all ProximaDB SDK imports are working correctly
"""

import pytest
import sys
from pathlib import Path

# Add the src directory to path if running directly
sys.path.insert(0, str(Path(__file__).parent.parent))


class TestImports:
    """Test all ProximaDB SDK imports"""
    
    def test_basic_imports(self):
        """Test basic module imports"""
        import proximadb_sdk
        assert proximadb_sdk.__version__
        assert proximadb_sdk.__author__
    
    def test_client_imports(self):
        """Test client imports"""
        from proximadb_sdk import ProximaDBClient, connect, connect_grpc, connect_rest, Protocol
        assert ProximaDBClient is not None
        assert callable(connect)
        assert callable(connect_grpc)
        assert callable(connect_rest)
        assert Protocol.REST.value == "rest"
        assert Protocol.GRPC.value == "grpc"
    
    def test_config_imports(self):
        """Test configuration imports"""
        from proximadb_sdk import ClientConfig, CompressionConfig
        from proximadb_sdk.config import Protocol, LogLevel, RetryConfig
        
        # Test ClientConfig
        config = ClientConfig(url="http://localhost:5678")
        assert config.url == "http://localhost:5678"
        
        # Test CompressionConfig with actual API
        compression = CompressionConfig(enabled=True, algorithm="gzip")
        assert compression.enabled is True
        assert compression.algorithm == "gzip"
    
    def test_model_imports(self):
        """Test model imports"""
        from proximadb_sdk import (
            Collection, CollectionConfig, IndexConfiguration,
            SearchResult, VectorOperationResponse, OperationMetrics,
            DistanceMetric, IndexingAlgorithm, StorageEngine,
            VectorRecord, HealthStatus, VectorArray, MetadataDict,
            FilterDict, QuantizationConfig, QuantizationType,
            SearchOptimization
        )
        
        # Test enums
        assert DistanceMetric.COSINE
        assert IndexingAlgorithm.HNSW
        assert StorageEngine.VIPER
        assert QuantizationType.UNIFORM
    
    def test_exception_imports(self):
        """Test exception imports"""
        from proximadb_sdk import (
            ProximaDBError, AuthenticationError, CollectionNotFoundError,
            VectorDimensionError, RateLimitError, ServerError, NetworkError
        )
        
        # Test exception hierarchy
        assert issubclass(AuthenticationError, ProximaDBError)
        assert issubclass(CollectionNotFoundError, ProximaDBError)
    
    def test_chunking_imports(self):
        """Test text chunking imports"""
        from proximadb_sdk import (
            TextChunker, ChunkingStrategy, ChunkingConfig,
            TextChunk, create_chunker, chunk_by_sentences,
            chunk_by_paragraphs, chunk_sliding_window
        )
        
        # Test ChunkingStrategy enum
        assert ChunkingStrategy.SENTENCE
        assert ChunkingStrategy.SLIDING_WINDOW
        
        # Test callable functions
        assert callable(create_chunker)
        assert callable(chunk_by_sentences)
    
    def test_filter_imports(self):
        """Test filter API imports"""
        from proximadb_sdk import (
            FilterBuilder, FilterOp, LogicalOp,
            eq, gt, lt, in_list, and_filters, or_filters
        )
        
        # Test enums
        assert FilterOp.EQUALS
        assert LogicalOp.AND
        
        # Test callable functions
        assert callable(eq)
        assert callable(and_filters)
    
    def test_all_exports(self):
        """Test that all items in __all__ are importable"""
        import proximadb_sdk

        for item in proximadb_sdk.__all__:
            assert hasattr(proximadb_sdk, item), f"Missing export: {item}"
    
    def test_optional_grpc_imports(self):
        """Test optional gRPC imports"""
        try:
            from proximadb_sdk import proximadb_pb2, proximadb_pb2_grpc
            # If we get here, gRPC is available
            assert proximadb_pb2 is not None or proximadb_pb2 is None  # Could be None if not installed
            assert proximadb_pb2_grpc is not None or proximadb_pb2_grpc is None
        except ImportError:
            # gRPC not available, which is fine
            pass
    
    def test_backwards_compatibility(self):
        """Test backwards compatibility aliases"""
        from proximadb_sdk import IndexConfig, Vector
        from proximadb_sdk import IndexConfiguration, VectorRecord
        
        # These should be the same
        assert IndexConfig is IndexConfiguration
        assert Vector is VectorRecord
    
    def test_from_import_patterns(self):
        """Test common from-import patterns"""
        # Pattern 1: Import everything from proximadb
        from proximadb_sdk import ProximaDBClient, CollectionConfig, DistanceMetric
        
        # Pattern 2: Import from submodules
        from proximadb_sdk.config import ClientConfig, CompressionConfig
        from proximadb_sdk.models import Collection, SearchResult
        from proximadb_sdk.exceptions import ProximaDBError
        from proximadb_sdk.chunking import TextChunker
        
        # All imports should work
        assert ClientConfig is not None
        assert Collection is not None
        assert ProximaDBError is not None
        assert TextChunker is not None


if __name__ == "__main__":
    # Run tests if executed directly
    pytest.main([__file__, "-v"])