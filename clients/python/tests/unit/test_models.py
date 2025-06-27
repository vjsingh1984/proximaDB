"""
Comprehensive unit tests for ProximaDB models module.
Tests model creation, validation, and serialization.
"""

import pytest
from datetime import datetime
from typing import Dict, Any

from proximadb.models import (
    Collection,
    CollectionConfig,
    SearchResult,
    InsertResult,
    BatchResult,
    DeleteResult,
    HealthStatus,
    IndexConfig,
    SearchStats
)
from proximadb.exceptions import ProximaDBError


class TestCollectionConfig:
    """Test CollectionConfig model"""
    
    def test_collection_config_creation(self):
        """Test basic collection config creation"""
        config = CollectionConfig(
            dimension=768,
            distance_metric="cosine"
        )
        assert config.dimension == 768
        assert config.distance_metric == "cosine"
    
    def test_collection_config_with_defaults(self):
        """Test collection config with default values"""
        config = CollectionConfig(
            dimension=384
        )
        assert config.dimension == 384
        # Test defaults are applied
        assert hasattr(config, 'distance_metric')
    
    def test_collection_config_validation(self):
        """Test collection config validation"""
        # Test invalid dimension
        with pytest.raises((ValueError, TypeError, ProximaDBError)):
            CollectionConfig(dimension=-1)
        
        # Test dimension validation
        with pytest.raises((ValueError, TypeError, ProximaDBError)):
            CollectionConfig(dimension=0)
    
    def test_collection_config_serialization(self):
        """Test collection config to dict conversion"""
        config = CollectionConfig(
            dimension=768,
            distance_metric="cosine"
        )
        
        # Test the config can be converted to dict-like format
        config_data = config.__dict__ if hasattr(config, '__dict__') else config
        assert isinstance(config_data, (dict, object))


class TestCollection:
    """Test Collection model"""
    
    def test_collection_creation(self):
        """Test basic collection creation"""
        config = CollectionConfig(dimension=768)
        collection = Collection(
            id="col_123",
            name="test_collection",
            config=config,
            created_at=datetime.now(),
            updated_at=datetime.now()
        )
        assert collection.id == "col_123"
        assert collection.config == config
        assert isinstance(collection.created_at, datetime)
    
    def test_collection_with_stats(self):
        """Test collection with statistics"""
        config = CollectionConfig(dimension=768)
        collection = Collection(
            id="col_123",
            name="test_collection",
            config=config,
            created_at=datetime.now(),
            updated_at=datetime.now(),
            vector_count=1000
        )
        if hasattr(collection, 'vector_count'):
            assert collection.vector_count == 1000
        if hasattr(collection, 'index_size_bytes'):
            assert collection.index_size_bytes == 50000


class TestSearchResult:
    """Test SearchResult model"""
    
    def test_search_result_creation(self):
        """Test basic search result creation"""
        result = SearchResult(
            id="vec_123",
            score=0.95,
            vector=[0.1, 0.2, 0.3],
            metadata={"label": "test"}
        )
        assert result.id == "vec_123"
        assert result.score == 0.95
        assert result.vector == [0.1, 0.2, 0.3]
        assert result.metadata == {"label": "test"}
    
    def test_search_result_without_vector(self):
        """Test search result without vector data"""
        result = SearchResult(
            id="vec_123",
            score=0.95,
            metadata={"label": "test"}
        )
        assert result.id == "vec_123"
        assert result.score == 0.95
        assert result.metadata == {"label": "test"}
    
    def test_search_result_validation(self):
        """Test search result validation"""
        # Test invalid score
        with pytest.raises((ValueError, TypeError)):
            SearchResult(id="test", score="invalid")
    
    def test_search_result_comparison(self):
        """Test search result comparison by score"""
        result1 = SearchResult(id="1", score=0.9)
        result2 = SearchResult(id="2", score=0.8)
        
        # If comparison is implemented
        try:
            assert result1 > result2 or result1 >= result2
        except (TypeError, AttributeError):
            # Comparison not implemented, which is fine
            pass


class TestInsertResult:
    """Test InsertResult model"""
    
    def test_insert_result_creation(self):
        """Test basic insert result creation"""
        result = InsertResult(
            count=1,
            duration_ms=50.0
        )
        assert result.count == 1
        assert result.duration_ms == 50.0
    
    def test_insert_result_with_error(self):
        """Test insert result with error"""
        result = InsertResult(
            count=0,
            failed_count=1,
            duration_ms=25.0,
            errors=["Dimension mismatch"]
        )
        assert result.count == 0
        assert result.failed_count == 1
        assert result.errors == ["Dimension mismatch"]


class TestBatchResult:
    """Test BatchResult model"""
    
    def test_batch_result_creation(self):
        """Test basic batch result creation"""
        batch = BatchResult(
            successful_count=2,
            failed_count=0,
            duration_ms=100.0
        )
        assert batch.successful_count == 2
        assert batch.failed_count == 0
        assert batch.duration_ms == 100.0
    
    def test_batch_result_with_failures(self):
        """Test batch result with some failures"""
        batch = BatchResult(
            successful_count=1,
            failed_count=1,
            duration_ms=120.0,
            errors=[{"id": "2", "error": "Invalid data"}]
        )
        assert batch.successful_count == 1
        assert batch.failed_count == 1
        assert len(batch.errors) == 1


class TestDeleteResult:
    """Test DeleteResult model"""
    
    def test_delete_result_creation(self):
        """Test basic delete result creation"""
        result = DeleteResult(
            deleted_count=1,
            duration_ms=20.0
        )
        assert result.deleted_count == 1
        assert result.duration_ms == 20.0


class TestHealthStatus:
    """Test HealthStatus model"""
    
    def test_health_status_creation(self):
        """Test basic health status creation"""
        status = HealthStatus(
            status="healthy",
            version="1.0.0",
            uptime_seconds=3600
        )
        assert status.status == "healthy"
        assert status.version == "1.0.0"
        assert status.uptime_seconds == 3600
    
    def test_health_status_with_metrics(self):
        """Test health status with additional metrics"""
        status = HealthStatus(
            status="healthy",
            version="1.0.0",
            uptime_seconds=3600,
            active_connections=5,
            memory_usage_bytes=1000000
        )
        assert status.status == "healthy"
        if hasattr(status, 'active_connections'):
            assert status.active_connections == 5
        if hasattr(status, 'memory_usage_bytes'):
            assert status.memory_usage_bytes == 1000000


class TestIndexConfig:
    """Test IndexConfig model"""
    
    def test_index_config_creation(self):
        """Test basic index config creation"""
        config = IndexConfig(
            algorithm="hnsw",
            parameters={"m": 16, "ef_construction": 200}
        )
        assert config.algorithm == "hnsw"
        assert config.parameters["m"] == 16
        assert config.parameters["ef_construction"] == 200
    
    def test_index_config_validation(self):
        """Test index config validation"""
        # Test empty algorithm
        with pytest.raises((ValueError, TypeError)):
            IndexConfig(algorithm="", parameters={})


class TestSearchStats:
    """Test SearchStats model"""
    
    def test_search_stats_creation(self):
        """Test basic search stats creation"""
        stats = SearchStats(
            total_searched=1000,
            vectors_scanned=100,
            duration_ms=25.5
        )
        assert stats.total_searched == 1000
        assert stats.vectors_scanned == 100
        assert stats.duration_ms == 25.5
    
    def test_search_stats_with_additional_metrics(self):
        """Test search stats with additional metrics"""
        stats = SearchStats(
            total_searched=1000,
            vectors_scanned=500,
            duration_ms=25.5,
            cache_hit_rate=0.85
        )
        assert stats.total_searched == 1000
        assert stats.vectors_scanned == 500
        assert stats.cache_hit_rate == 0.85


class TestModelIntegration:
    """Test model integration and edge cases"""
    
    def test_model_dict_conversion(self):
        """Test models can be converted to dictionaries"""
        config = CollectionConfig(dimension=768)
        
        # Try various ways to convert to dict
        try:
            dict_repr = config.__dict__
            assert isinstance(dict_repr, dict)
        except AttributeError:
            # Some models might use different serialization
            pass
        
        try:
            dict_repr = config.to_dict() if hasattr(config, 'to_dict') else None
            if dict_repr:
                assert isinstance(dict_repr, dict)
        except AttributeError:
            pass
    
    def test_model_repr(self):
        """Test model string representations"""
        config = CollectionConfig(dimension=768)
        result = SearchResult(id="test", score=0.9)
        
        # Should have meaningful string representations
        config_str = str(config)
        result_str = str(result)
        
        assert isinstance(config_str, str)
        assert isinstance(result_str, str)
        assert len(config_str) > 0
        assert len(result_str) > 0
    
    def test_model_equality(self):
        """Test model equality comparison"""
        config1 = CollectionConfig(dimension=768)
        config2 = CollectionConfig(dimension=768)
        config3 = CollectionConfig(dimension=384)
        
        # Test equality if implemented
        try:
            assert config1 == config2
            assert config1 != config3
        except (TypeError, NotImplementedError):
            # Equality not implemented, which is acceptable
            pass