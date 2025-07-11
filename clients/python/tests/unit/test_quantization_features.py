"""
Test quantization features and search optimization hints

This test file validates the new quantization and search optimization
features added to ProximaDB.
"""

import pytest
import numpy as np
from proximadb.models import (
    QuantizationType,
    QuantizationConfig,
    SearchOptimizationHints,
    CollectionConfig,
    DistanceMetric,
)
from proximadb.rest_client import ProximaDBRestClient
from proximadb import proximadb_pb2


class TestQuantizationModels:
    """Test quantization configuration models"""
    
    def test_quantization_config_defaults(self):
        """Test default quantization configuration"""
        config = QuantizationConfig()
        assert config.enabled is False
        assert config.type == QuantizationType.NONE
        assert config.progressive_quantization is False
        
    def test_quantization_config_pq(self):
        """Test product quantization configuration"""
        config = QuantizationConfig(
            enabled=True,
            type=QuantizationType.PRODUCT,
            bits_per_subvector=8,
            num_subvectors=16,
            compression_ratio_target=4.0
        )
        assert config.enabled is True
        assert config.type == QuantizationType.PRODUCT
        assert config.bits_per_subvector == 8
        assert config.num_subvectors == 16
        assert config.compression_ratio_target == 4.0
        
    def test_quantization_config_scalar(self):
        """Test scalar quantization configuration"""
        config = QuantizationConfig(
            enabled=True,
            type=QuantizationType.SCALAR,
            bits_per_vector=8,
            accuracy_threshold=0.95
        )
        assert config.enabled is True
        assert config.type == QuantizationType.SCALAR
        assert config.bits_per_vector == 8
        assert config.accuracy_threshold == 0.95
        
    def test_quantization_config_validation(self):
        """Test quantization configuration validation"""
        # Test invalid bits_per_vector
        with pytest.raises(ValueError):
            QuantizationConfig(bits_per_vector=0)
            
        with pytest.raises(ValueError):
            QuantizationConfig(bits_per_vector=33)
            
        # Test invalid accuracy_threshold
        with pytest.raises(ValueError):
            QuantizationConfig(accuracy_threshold=-0.1)
            
        with pytest.raises(ValueError):
            QuantizationConfig(accuracy_threshold=1.1)


class TestSearchOptimizationHints:
    """Test search optimization hints model"""
    
    def test_search_hints_defaults(self):
        """Test default search optimization hints"""
        hints = SearchOptimizationHints()
        assert hints.enable_two_stage_search is False
        assert hints.quantization_hint is None
        assert hints.candidate_multiplier == 3.0
        assert hints.enable_clustering_optimization is True
        assert hints.enable_parallel_search is True
        
    def test_search_hints_two_stage(self):
        """Test two-stage search configuration"""
        hints = SearchOptimizationHints(
            enable_two_stage_search=True,
            quantization_hint="PQ8",
            candidate_multiplier=5.0,
            min_candidates=100,
            max_candidates=1000
        )
        assert hints.enable_two_stage_search is True
        assert hints.quantization_hint == "PQ8"
        assert hints.candidate_multiplier == 5.0
        assert hints.min_candidates == 100
        assert hints.max_candidates == 1000
        
    def test_search_hints_custom(self):
        """Test custom search hints"""
        hints = SearchOptimizationHints(
            custom_hints={
                "use_simd": "true",
                "prefetch_size": "64"
            }
        )
        assert hints.custom_hints["use_simd"] == "true"
        assert hints.custom_hints["prefetch_size"] == "64"
        
    def test_search_hints_validation(self):
        """Test search hints validation"""
        # Test invalid candidate_multiplier
        with pytest.raises(ValueError):
            SearchOptimizationHints(candidate_multiplier=0.5)
            
        with pytest.raises(ValueError):
            SearchOptimizationHints(candidate_multiplier=101.0)
            
        # Test invalid timeout
        with pytest.raises(ValueError):
            SearchOptimizationHints(timeout_ms=0)


class TestCollectionWithQuantization:
    """Test collection configuration with quantization"""
    
    def test_collection_config_with_quantization(self):
        """Test collection config includes quantization"""
        quantization = QuantizationConfig(
            enabled=True,
            type=QuantizationType.PRODUCT,
            bits_per_subvector=4,
            num_subvectors=32
        )
        
        config = CollectionConfig(
            dimension=768,
            distance_metric=DistanceMetric.COSINE,
            quantization_config=quantization
        )
        
        assert config.quantization_config is not None
        assert config.quantization_config.enabled is True
        assert config.quantization_config.type == QuantizationType.PRODUCT
        
    def test_collection_config_without_quantization(self):
        """Test collection config without quantization"""
        config = CollectionConfig(
            dimension=384,
            distance_metric=DistanceMetric.EUCLIDEAN
        )
        
        assert config.quantization_config is None


class TestProtoQuantizationMessages:
    """Test protobuf quantization message generation"""
    
    def test_quantization_config_proto(self):
        """Test QuantizationConfig protobuf message"""
        # Create a QuantizationConfig proto message
        proto_config = proximadb_pb2.QuantizationConfig()
        proto_config.enabled = True
        proto_config.progressive_quantization = True
        proto_config.compression_ratio_target = 4.0
        
        # Create a QuantizationLevel with PQ
        pq_level = proximadb_pb2.ProductQuantization()
        pq_level.bits_per_code = 8
        pq_level.num_subvectors = 16
        pq_level.adaptive_subvectors = True
        
        proto_config.level.pq.CopyFrom(pq_level)
        
        # Verify fields
        assert proto_config.enabled is True
        assert proto_config.progressive_quantization is True
        assert proto_config.compression_ratio_target == 4.0
        assert proto_config.level.pq.bits_per_code == 8
        assert proto_config.level.pq.num_subvectors == 16
        
    def test_search_optimization_hints_proto(self):
        """Test SearchOptimizationHints protobuf message"""
        # Create SearchOptimizationHints proto message
        hints = proximadb_pb2.SearchOptimizationHints()
        hints.enable_two_stage_search = True
        hints.candidate_multiplier = 5.0
        hints.min_candidates = 100
        hints.max_candidates = 1000
        hints.enable_parallel_search = True
        hints.accuracy_threshold = 0.95
        
        # Set quantization hint
        uniform_quant = proximadb_pb2.UniformQuantization()
        uniform_quant.bits = 8
        hints.quantization_hint.uniform.CopyFrom(uniform_quant)
        
        # Add custom hints
        hints.custom_hints["use_gpu"] = "true"
        hints.custom_hints["batch_size"] = "256"
        
        # Verify fields
        assert hints.enable_two_stage_search is True
        assert hints.candidate_multiplier == 5.0
        assert hints.min_candidates == 100
        assert hints.max_candidates == 1000
        assert hints.quantization_hint.uniform.bits == 8
        assert hints.custom_hints["use_gpu"] == "true"
        
    def test_collection_config_proto_with_quantization(self):
        """Test CollectionConfig proto with quantization"""
        # Create CollectionConfig proto
        config = proximadb_pb2.CollectionConfig()
        config.name = "test_collection"
        config.dimension = 768
        config.distance_metric = proximadb_pb2.DistanceMetric.COSINE
        config.storage_engine = proximadb_pb2.StorageEngine.VIPER
        config.indexing_algorithm = proximadb_pb2.IndexingAlgorithm.HNSW
        
        # Add quantization config
        config.quantization_config.enabled = True
        config.quantization_config.compression_ratio_target = 4.0
        
        # Set binary quantization
        binary_quant = proximadb_pb2.BinaryQuantization()
        binary_quant.threshold = 0.0
        binary_quant.sign_based = True
        config.quantization_config.level.binary.CopyFrom(binary_quant)
        
        # Verify
        assert config.name == "test_collection"
        assert config.dimension == 768
        assert config.quantization_config.enabled is True
        assert config.quantization_config.level.binary.sign_based is True
        
    def test_vector_search_request_with_hints(self):
        """Test VectorSearchRequest with optimization hints"""
        # Create search request
        request = proximadb_pb2.VectorSearchRequest()
        request.collection_id = "test_collection"
        request.top_k = 10
        
        # Add query
        query = proximadb_pb2.SearchQuery()
        query.vector.extend([0.1, 0.2, 0.3, 0.4])
        request.queries.append(query)
        
        # Add optimization hints
        request.optimization_hints.enable_two_stage_search = True
        request.optimization_hints.candidate_multiplier = 3.0
        request.optimization_hints.enable_clustering_optimization = True
        
        # Set quantization hint to PQ
        pq_level = proximadb_pb2.ProductQuantization()
        pq_level.bits_per_code = 4
        pq_level.num_subvectors = 64
        request.optimization_hints.quantization_hint.pq.CopyFrom(pq_level)
        
        # Verify
        assert request.collection_id == "test_collection"
        assert request.top_k == 10
        assert len(request.queries) == 1
        assert request.optimization_hints.enable_two_stage_search is True
        assert request.optimization_hints.quantization_hint.pq.bits_per_code == 4


@pytest.mark.integration
class TestQuantizationIntegration:
    """Integration tests for quantization features (requires running server)"""
    
    @pytest.fixture
    def client(self):
        """Create REST client for testing"""
        return ProximaDBRestClient(url="http://localhost:5678")
        
    def test_create_collection_with_quantization(self, client):
        """Test creating collection with quantization enabled"""
        # Skip if server is not running
        try:
            client.health()
        except Exception:
            pytest.skip("ProximaDB server not running")
            
        # Create collection with quantization
        config = CollectionConfig(
            dimension=128,
            distance_metric=DistanceMetric.COSINE,
            quantization_config=QuantizationConfig(
                enabled=True,
                type=QuantizationType.SCALAR,
                bits_per_vector=8,
                accuracy_threshold=0.95
            )
        )
        
        collection_name = f"test_quant_{np.random.randint(1000000)}"
        
        try:
            # Create collection
            collection = client.create_collection(collection_name, config)
            assert collection.name == collection_name
            
            # Verify collection was created
            collections = client.list_collections()
            assert collection_name in [c.name for c in collections]
            
        finally:
            # Cleanup
            try:
                client.delete_collection(collection_name)
            except Exception:
                pass
                
    def test_search_with_optimization_hints(self, client):
        """Test search with optimization hints"""
        # Skip if server is not running
        try:
            client.health()
        except Exception:
            pytest.skip("ProximaDB server not running")
            
        collection_name = f"test_search_hints_{np.random.randint(1000000)}"
        
        try:
            # Create collection
            config = CollectionConfig(dimension=128)
            collection = client.create_collection(collection_name, config)
            
            # Insert test vectors
            vectors = np.random.rand(100, 128).astype(np.float32)
            ids = [f"vec_{i}" for i in range(100)]
            client.insert_vectors(collection_name, vectors, ids)
            
            # Search with optimization hints
            query = np.random.rand(128).astype(np.float32)
            hints = {
                "enable_two_stage_search": True,
                "quantization_hint": "FP32",
                "candidate_multiplier": 2.0,
                "enable_parallel_search": True
            }
            
            results = client.search(
                collection_name,
                query,
                k=10,
                optimization_hints=hints
            )
            
            # Verify results
            assert len(results) <= 10
            for result in results:
                assert hasattr(result, 'id')
                assert hasattr(result, 'score')
                
        finally:
            # Cleanup
            try:
                client.delete_collection(collection_name)
            except Exception:
                pass


if __name__ == "__main__":
    pytest.main([__file__, "-v"])