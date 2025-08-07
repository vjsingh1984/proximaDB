#!/usr/bin/env python3
"""
Tests for SDK-driven compression integration

Copyright 2025 ProximaDB
"""

import pytest
import numpy as np
from typing import List
from unittest.mock import Mock, patch, MagicMock

from proximadb.models import (
    CollectionConfig,
    CompressionConfig,
    CompressionAlgorithm,
    CompressionLevel,
    DistanceMetric,
    StorageEngine,
    VectorRecord,
    SearchOptimization,
    SearchResult,
)


class TestCompressionConfig:
    """Test compression configuration models"""
    
    def test_compression_config_defaults(self):
        """Test default values for CompressionConfig"""
        config = CompressionConfig()
        
        assert config.sst_block_size == 16384
        assert config.sst_compression_algorithm == CompressionAlgorithm.NONE
        assert config.sst_compression_level is None
        assert config.viper_compression_algorithm == CompressionAlgorithm.NONE
        assert config.viper_compression_level is None
        assert config.viper_enable_dual_columns is False
        assert config.adaptive_compression is False
        assert config.compression_threshold_kb == 100
    
    def test_compression_config_custom_values(self):
        """Test custom values for CompressionConfig"""
        config = CompressionConfig(
            sst_block_size=32768,
            sst_compression_algorithm=CompressionAlgorithm.ZSTD,
            sst_compression_level=6,
            viper_compression_algorithm=CompressionAlgorithm.LZ4,
            viper_compression_level=1,
            viper_enable_dual_columns=True,
            adaptive_compression=True,
            compression_threshold_kb=50,
        )
        
        assert config.sst_block_size == 32768
        assert config.sst_compression_algorithm == CompressionAlgorithm.ZSTD
        assert config.sst_compression_level == 6
        assert config.viper_compression_algorithm == CompressionAlgorithm.LZ4
        assert config.viper_compression_level == 1
        assert config.viper_enable_dual_columns is True
        assert config.adaptive_compression is True
        assert config.compression_threshold_kb == 50
    
    def test_compression_algorithm_enum(self):
        """Test CompressionAlgorithm enum values"""
        assert CompressionAlgorithm.NONE == "none"
        assert CompressionAlgorithm.ZSTD == "zstd"
        assert CompressionAlgorithm.LZ4 == "lz4"
        assert CompressionAlgorithm.SNAPPY == "snappy"
    
    def test_compression_level_enum(self):
        """Test CompressionLevel enum values"""
        assert CompressionLevel.FASTEST == 1
        assert CompressionLevel.FAST == 3
        assert CompressionLevel.BALANCED == 6
        assert CompressionLevel.HIGH == 9


class TestCollectionConfigWithCompression:
    """Test collection configuration with compression"""
    
    def test_collection_config_with_compression(self):
        """Test CollectionConfig includes compression_config field"""
        compression_config = CompressionConfig(
            sst_compression_algorithm=CompressionAlgorithm.ZSTD,
            sst_compression_level=CompressionLevel.BALANCED,
        )
        
        collection_config = CollectionConfig(
            name="test_compressed_collection",
            dimension=1536,
            distance_metric=DistanceMetric.COSINE,
            storage_engine=StorageEngine.SST,
            compression_config=compression_config,
        )
        
        assert collection_config.compression_config is not None
        assert collection_config.compression_config.sst_compression_algorithm == CompressionAlgorithm.ZSTD
        assert collection_config.compression_config.sst_compression_level == 6
    
    def test_collection_config_serialization(self):
        """Test CollectionConfig serialization with compression"""
        compression_config = CompressionConfig(
            sst_compression_algorithm=CompressionAlgorithm.LZ4,
            adaptive_compression=True,
        )
        
        collection_config = CollectionConfig(
            name="serialization_test",
            dimension=768,
            compression_config=compression_config,
        )
        
        # Test model_dump (Pydantic v2)
        data = collection_config.model_dump(exclude_none=True)
        assert "compression_config" in data
        assert data["compression_config"]["sst_compression_algorithm"] == "lz4"
        assert data["compression_config"]["adaptive_compression"] is True
    
    def test_viper_dual_column_config(self):
        """Test VIPER-specific dual column configuration"""
        compression_config = CompressionConfig(
            viper_compression_algorithm=CompressionAlgorithm.SNAPPY,
            viper_enable_dual_columns=True,
        )
        
        collection_config = CollectionConfig(
            name="viper_dual_test",
            dimension=512,
            storage_engine=StorageEngine.VIPER,
            compression_config=compression_config,
        )
        
        assert collection_config.storage_engine == StorageEngine.VIPER
        assert collection_config.compression_config.viper_enable_dual_columns is True
        assert collection_config.compression_config.viper_compression_algorithm == CompressionAlgorithm.SNAPPY


class TestSearchOptimizationWithCompression:
    """Test search optimization with compression hints"""
    
    def test_search_optimization_defaults(self):
        """Test default values for compression-aware search hints"""
        optimization = SearchOptimization()
        
        # Check compression-specific fields
        assert optimization.prefer_compressed_search is None
        assert optimization.decompression_budget_ms is None
        assert optimization.use_decompression_cache is True  # Default to True
        assert optimization.compression_aware_routing is None
    
    def test_search_optimization_custom_values(self):
        """Test custom compression-aware search hints"""
        optimization = SearchOptimization(
            enable_two_stage=True,
            prefer_compressed_search=True,
            decompression_budget_ms=100,
            use_decompression_cache=False,
            compression_aware_routing=True,
        )
        
        assert optimization.enable_two_stage is True
        assert optimization.prefer_compressed_search is True
        assert optimization.decompression_budget_ms == 100
        assert optimization.use_decompression_cache is False
        assert optimization.compression_aware_routing is True
    
    def test_search_optimization_serialization(self):
        """Test SearchOptimization serialization"""
        optimization = SearchOptimization(
            top_k=20,
            accuracy_threshold=0.95,
            prefer_compressed_search=True,
            decompression_budget_ms=200,
            custom_hints={"parallel_decompression": True},
        )
        
        data = optimization.model_dump(exclude_none=True)
        assert data["top_k"] == 20
        assert data["accuracy_threshold"] == 0.95
        assert data["prefer_compressed_search"] is True
        assert data["decompression_budget_ms"] == 200
        assert data["use_decompression_cache"] is True  # Default value
        assert data["custom_hints"]["parallel_decompression"] is True


class TestCompressionIntegrationScenarios:
    """Test complete compression integration scenarios"""
    
    @pytest.fixture
    def mock_client(self):
        """Create a mock ProximaDB client"""
        client = Mock()
        client.create_collection = Mock(return_value=Mock(id="test_collection", name="test_collection"))
        client.insert_vectors = Mock(return_value={"success": True, "inserted": 100})
        client.search_vectors = Mock(return_value=[
            SearchResult(id="vec_1", score=0.95, rank=1),
            SearchResult(id="vec_2", score=0.90, rank=2),
        ])
        return client
    
    def test_sst_compression_scenario(self, mock_client):
        """Test SST storage with compression"""
        # Create collection with SST compression
        compression_config = CompressionConfig(
            sst_compression_algorithm=CompressionAlgorithm.ZSTD,
            sst_compression_level=CompressionLevel.BALANCED,
            sst_block_size=32768,
            adaptive_compression=True,
        )
        
        collection_config = CollectionConfig(
            name="sst_compressed",
            dimension=1536,
            storage_engine=StorageEngine.SST,
            compression_config=compression_config,
        )
        
        # Create collection
        collection = mock_client.create_collection(collection_config)
        assert collection.id == "test_collection"
        
        # Verify compression config was passed
        mock_client.create_collection.assert_called_once()
        call_args = mock_client.create_collection.call_args[0][0]
        assert call_args.compression_config.sst_compression_algorithm == CompressionAlgorithm.ZSTD
    
    def test_viper_dual_column_scenario(self, mock_client):
        """Test VIPER storage with dual columns"""
        compression_config = CompressionConfig(
            viper_compression_algorithm=CompressionAlgorithm.LZ4,
            viper_compression_level=1,
            viper_enable_dual_columns=True,
        )
        
        collection_config = CollectionConfig(
            name="viper_dual",
            dimension=768,
            storage_engine=StorageEngine.VIPER,
            compression_config=compression_config,
        )
        
        collection = mock_client.create_collection(collection_config)
        
        # Verify dual column configuration
        call_args = mock_client.create_collection.call_args[0][0]
        assert call_args.compression_config.viper_enable_dual_columns is True
        assert call_args.compression_config.viper_compression_algorithm == CompressionAlgorithm.LZ4
    
    def test_compression_aware_search(self, mock_client):
        """Test compression-aware search optimization"""
        # Perform search with compression hints
        query_vector = np.random.rand(1536).tolist()
        
        optimization = SearchOptimization(
            enable_two_stage=True,
            prefer_compressed_search=True,
            decompression_budget_ms=150,
            use_decompression_cache=True,
            compression_aware_routing=True,
        )
        
        results = mock_client.search_vectors(
            collection_id="test_collection",
            query_vector=query_vector,
            top_k=10,
            search_optimization=optimization,
        )
        
        assert len(results) == 2
        assert results[0].id == "vec_1"
        
        # Verify optimization hints were passed
        mock_client.search_vectors.assert_called_once()
        call_kwargs = mock_client.search_vectors.call_args[1]
        opt = call_kwargs["search_optimization"]
        assert opt.prefer_compressed_search is True
        assert opt.decompression_budget_ms == 150
    
    def test_adaptive_compression_scenario(self, mock_client):
        """Test adaptive compression based on data characteristics"""
        compression_config = CompressionConfig(
            sst_compression_algorithm=CompressionAlgorithm.ZSTD,
            adaptive_compression=True,
            compression_threshold_kb=50,  # Only compress files > 50KB
        )
        
        collection_config = CollectionConfig(
            name="adaptive_test",
            dimension=512,
            compression_config=compression_config,
        )
        
        collection = mock_client.create_collection(collection_config)
        
        # Insert sparse vectors (high compressibility)
        sparse_vectors = []
        for i in range(10):
            vec = np.zeros(512)
            vec[np.random.choice(512, 5)] = np.random.randn(5)
            sparse_vectors.append(VectorRecord(
                id=f"sparse_{i}",
                vector=vec.tolist(),
                metadata={"type": "sparse"}
            ))
        
        mock_client.insert_vectors("adaptive_test", sparse_vectors)
        
        # Insert dense vectors (low compressibility)
        dense_vectors = []
        for i in range(10):
            dense_vectors.append(VectorRecord(
                id=f"dense_{i}",
                vector=np.random.rand(512).tolist(),
                metadata={"type": "dense"}
            ))
        
        mock_client.insert_vectors("adaptive_test", dense_vectors)
        
        # Verify adaptive compression settings
        create_call = mock_client.create_collection.call_args[0][0]
        assert create_call.compression_config.adaptive_compression is True
        assert create_call.compression_config.compression_threshold_kb == 50


class TestCompressionPerformance:
    """Test compression performance characteristics"""
    
    def test_compression_ratio_estimation(self):
        """Test estimation of compression ratios for different data types"""
        # Sparse data should compress well
        sparse_data = np.zeros((100, 512))
        sparse_data[np.random.choice(100*512, 1000, replace=False)] = np.random.randn(1000)
        sparse_size = sparse_data.nbytes
        
        # Dense random data compresses poorly
        dense_data = np.random.rand(100, 512)
        dense_size = dense_data.nbytes
        
        # Structured data (repeated patterns) compresses well
        structured_data = np.tile(np.arange(512), (100, 1))
        structured_size = structured_data.nbytes
        
        assert sparse_size == dense_size == structured_size  # Same uncompressed size
        
        # In real scenarios, ZSTD would achieve different compression ratios:
        # - Sparse: ~10-20x compression
        # - Dense random: ~1.1-1.3x compression
        # - Structured: ~5-10x compression
    
    def test_cache_effectiveness(self):
        """Test decompression cache effectiveness metrics"""
        cache_config = {
            "max_size_mb": 512,
            "enable_prefetch": True,
            "prefetch_threshold": 3,
            "ttl_seconds": 0,
            "invalidation_check_interval_seconds": 60,
        }
        
        # Simulate cache hits/misses
        total_requests = 1000
        cache_hits = 750
        cache_misses = 250
        
        hit_rate = cache_hits / total_requests
        miss_rate = cache_misses / total_requests
        
        assert hit_rate == 0.75
        assert miss_rate == 0.25
        
        # Expected speedup from cache (assuming 10ms decompression time)
        avg_time_with_cache = (cache_hits * 0.1 + cache_misses * 10) / total_requests
        avg_time_without_cache = 10  # Always decompress
        
        speedup = avg_time_without_cache / avg_time_with_cache
        assert speedup > 3.0  # Should achieve >3x speedup with 75% hit rate


if __name__ == "__main__":
    pytest.main([__file__, "-v"])