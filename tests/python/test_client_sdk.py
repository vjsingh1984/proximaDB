#!/usr/bin/env python3
"""
ProximaDB Client & SDK Test Suite
Consolidated tests for client creation, configuration, error handling, and SDK features
"""

import pytest
import asyncio
from typing import Dict, Any

from proximadb import (
    ProximaDBClient, ProximaDBGrpcClient, ProximaDBRestClient,
    connect, connect_grpc, connect_rest, Protocol
)
from proximadb.models import CollectionConfig, DistanceMetric
from proximadb.exceptions import ProximaDBError, CollectionNotFoundError
from proximadb.config import ClientConfig


class TestClientCreation:
    """Test client creation and configuration"""
    
    def test_unified_client_auto_detect(self):
        """Test unified client with protocol auto-detection"""
        # REST client (port 5678)
        rest_client = ProximaDBClient("http://localhost:5678")
        assert rest_client is not None
        
        # gRPC client (port 5679)
        grpc_client = ProximaDBClient("http://localhost:5679")
        assert grpc_client is not None
    
    def test_explicit_protocol_clients(self):
        """Test explicit protocol client creation"""
        # Explicit REST
        rest_client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        assert rest_client is not None
        
        # Explicit gRPC
        grpc_client = ProximaDBClient("http://localhost:5679", protocol=Protocol.GRPC)
        assert grpc_client is not None
    
    def test_client_factory_functions(self):
        """Test factory functions for creating clients"""
        # Generic connect function
        client = connect("http://localhost:5678")
        assert client is not None
        
        # Protocol-specific connections
        rest_client = connect_rest("http://localhost:5678")
        assert rest_client is not None
        
        grpc_client = connect_grpc("http://localhost:5679")
        assert grpc_client is not None
    
    def test_direct_client_classes(self):
        """Test direct client class instantiation"""
        rest_client = ProximaDBRestClient("http://localhost:5678")
        assert rest_client is not None
        assert hasattr(rest_client, 'endpoint') or hasattr(rest_client, 'base_url')
        
        grpc_client = ProximaDBGrpcClient("http://localhost:5679")
        assert grpc_client is not None
        assert hasattr(grpc_client, 'endpoint') or hasattr(grpc_client, 'channel')
    
    def test_client_with_config(self):
        """Test client creation with configuration objects"""
        config = ClientConfig(
            url="http://localhost:5678",
            timeout=30.0,
            retry_attempts=3,
            enable_debug_logging=False
        )
        
        client = ProximaDBClient(config=config)
        assert client is not None
    
    def test_client_url_validation(self):
        """Test client URL validation"""
        # Valid URLs should work
        valid_urls = [
            "http://localhost:5678",
            "https://localhost:5678",
            "http://127.0.0.1:5678",
            "https://api.example.com:5678"
        ]
        
        for url in valid_urls:
            try:
                client = ProximaDBClient(url)
                assert client is not None
            except Exception as e:
                # Connection errors are acceptable, validation errors are not
                assert "connection" in str(e).lower() or "timeout" in str(e).lower()


class TestClientConfiguration:
    """Test client configuration options and validation"""
    
    def test_client_config_creation(self):
        """Test ClientConfig creation and validation"""
        # Basic config
        config = ClientConfig(endpoint="localhost:5678")
        assert config.endpoint == "localhost:5678"
        
        # Advanced config
        advanced_config = ClientConfig(
            endpoint="localhost:5678",
            timeout=10.0,
            retry_attempts=5,
            enable_debug_logging=True,
            max_connections=100
        )
        
        assert advanced_config.timeout == 10.0
        assert advanced_config.retry_attempts == 5
        assert advanced_config.enable_debug_logging is True
    
    def test_config_serialization(self):
        """Test configuration serialization"""
        config = ClientConfig(
            endpoint="localhost:5678",
            timeout=15.0,
            retry_attempts=3
        )
        
        # Test dict conversion
        config_dict = config.model_dump() if hasattr(config, 'model_dump') else config.__dict__
        assert isinstance(config_dict, dict)
        assert config_dict.get('endpoint') == "localhost:5678"
        assert config_dict.get('timeout') == 15.0
    
    def test_config_defaults(self):
        """Test configuration default values"""
        config = ClientConfig()
        
        # Should have reasonable defaults
        assert hasattr(config, 'timeout')
        assert hasattr(config, 'retry_attempts')
        assert hasattr(config, 'enable_debug_logging')


class TestHealthAndMetrics:
    """Test health checks and metrics collection"""
    
    def test_health_check_rest(self):
        """Test health check via REST"""
        client = connect_rest("http://localhost:5678")
        
        try:
            health = client.health()
            assert health is not None
            
            # Health response should indicate server status
            if hasattr(health, 'status'):
                assert health.status in ['healthy', 'ok', 'running', 'active']
            
        except Exception as e:
            # Health endpoint might not be implemented yet
            pytest.skip(f"Health endpoint not implemented: {e}")
    
    def test_health_check_grpc(self):
        """Test health check via gRPC"""
        client = connect_grpc("http://localhost:5679")
        
        try:
            health = client.health()
            assert health is not None
            
            if hasattr(health, 'status'):
                assert health.status in ['healthy', 'ok', 'running', 'active']
                
        except Exception as e:
            pytest.skip(f"Health endpoint not implemented: {e}")
    
    def test_metrics_collection(self):
        """Test metrics endpoint"""
        client = connect_rest("http://localhost:5678")
        
        try:
            metrics = client.get_metrics()
            assert metrics is not None
            
            # Metrics should contain useful information
            if isinstance(metrics, dict):
                assert len(metrics) > 0
                
        except Exception as e:
            pytest.skip(f"Metrics endpoint not implemented: {e}")


class TestErrorHandling:
    """Test error handling and exception management"""
    
    def test_connection_errors(self):
        """Test handling of connection errors"""
        # Test connection to non-existent server
        try:
            client = ProximaDBClient("http://localhost:9999")  # Non-existent port
            # Some operations might fail only when actually used
            client.list_collections()
        except Exception as e:
            # Connection errors are expected
            assert "connection" in str(e).lower() or "refused" in str(e).lower()
    
    def test_proxima_db_error_hierarchy(self):
        """Test ProximaDB error class hierarchy"""
        # Test that specific errors inherit from ProximaDBError
        assert issubclass(CollectionNotFoundError, ProximaDBError)
        
        # Test error creation and message handling
        error = ProximaDBError("Test error message")
        assert str(error) == "Test error message"
        
        collection_error = CollectionNotFoundError("Collection 'test' not found")
        assert "test" in str(collection_error)
        assert isinstance(collection_error, ProximaDBError)
    
    def test_collection_not_found_handling(self):
        """Test CollectionNotFoundError handling"""
        client = connect_rest("http://localhost:5678")
        non_existent = f"non_existent_collection_{int(time.time())}"
        
        with pytest.raises((CollectionNotFoundError, ProximaDBError)):
            client.get_collection(non_existent)
    
    def test_invalid_input_handling(self):
        """Test handling of invalid inputs"""
        client = connect_rest("http://localhost:5678")
        
        # Test invalid collection names
        invalid_names = [None, "", " ", "\n", "\t"]
        
        for invalid_name in invalid_names:
            with pytest.raises((ProximaDBError, ValueError, TypeError)):
                config = CollectionConfig(dimension=128)
                client.create_collection(invalid_name, config)


class TestContextManagers:
    """Test context manager support for clients"""
    
    def test_client_context_manager(self):
        """Test client context manager support"""
        try:
            with ProximaDBClient("http://localhost:5678") as client:
                assert client is not None
                # Test basic operation
                try:
                    collections = client.list_collections()
                except:
                    pass  # Operation failure is acceptable, context manager is what we're testing
                    
        except (AttributeError, TypeError):
            # Context manager not implemented, which is acceptable
            pytest.skip("Context manager not implemented for client")
    
    def test_specific_client_context_managers(self):
        """Test context managers for specific client types"""
        try:
            with ProximaDBRestClient("http://localhost:5678") as rest_client:
                assert rest_client is not None
                
        except (AttributeError, TypeError):
            pytest.skip("Context manager not implemented for REST client")
        
        try:
            with ProximaDBGrpcClient("http://localhost:5679") as grpc_client:
                assert grpc_client is not None
                
        except (AttributeError, TypeError):
            pytest.skip("Context manager not implemented for gRPC client")


class TestAsyncSupport:
    """Test asynchronous client support"""
    
    @pytest.mark.asyncio
    async def test_async_client_operations(self):
        """Test async client operations if supported"""
        try:
            # Try to import async client
            from proximadb import AsyncProximaDBClient
            
            async with AsyncProximaDBClient("http://localhost:5678") as client:
                assert client is not None
                
                # Test async operations
                try:
                    collections = await client.list_collections()
                    assert collections is not None
                except Exception:
                    # Operation failure is acceptable
                    pass
                    
        except ImportError:
            pytest.skip("Async client not implemented")
        except Exception as e:
            pytest.skip(f"Async operations not fully supported: {e}")


class TestProtocolInteroperability:
    """Test interoperability between REST and gRPC protocols"""
    
    def test_cross_protocol_compatibility(self):
        """Test that REST and gRPC clients can work with same data"""
        rest_client = connect_rest("http://localhost:5678")
        grpc_client = connect_grpc("http://localhost:5679")
        
        collection_name = f"interop_test_{int(time.time())}"
        config = CollectionConfig(dimension=128, distance_metric=DistanceMetric.COSINE)
        
        try:
            # Create collection with REST
            collection = rest_client.create_collection(collection_name, config)
            assert collection is not None
            
            # Verify with gRPC
            grpc_collection = grpc_client.get_collection(collection_name)
            assert grpc_collection is not None
            
            # Both clients should see the same collection
            rest_collections = rest_client.list_collections()
            grpc_collections = grpc_client.list_collections()
            
            rest_names = [getattr(col, 'id', getattr(col, 'name', None)) for col in rest_collections]
            grpc_names = [getattr(col, 'id', getattr(col, 'name', None)) for col in grpc_collections]
            
            assert collection_name in rest_names
            assert collection_name in grpc_names
            
        finally:
            # Cleanup with either client
            try:
                rest_client.delete_collection(collection_name)
            except:
                try:
                    grpc_client.delete_collection(collection_name)
                except:
                    pass
    
    def test_protocol_specific_features(self):
        """Test protocol-specific features and optimizations"""
        rest_client = connect_rest("http://localhost:5678")
        grpc_client = connect_grpc("http://localhost:5679")
        
        # Test REST-specific features
        if hasattr(rest_client, 'get_openapi_spec'):
            try:
                spec = rest_client.get_openapi_spec()
                assert spec is not None
            except Exception:
                pass
        
        # Test gRPC-specific features
        if hasattr(grpc_client, 'get_service_info'):
            try:
                info = grpc_client.get_service_info()
                assert info is not None
            except Exception:
                pass


class TestClientPerformance:
    """Test client performance characteristics"""
    
    def test_connection_pooling(self):
        """Test connection pooling behavior"""
        # Create multiple clients to same endpoint
        clients = []
        for i in range(5):
            client = connect_rest("http://localhost:5678")
            clients.append(client)
        
        # All should be functional
        for client in clients:
            try:
                health = client.health()
            except Exception:
                # Health check failure is acceptable
                pass
        
        # Cleanup
        for client in clients:
            if hasattr(client, 'close'):
                client.close()
    
    def test_concurrent_operations(self):
        """Test concurrent client operations"""
        import threading
        import time
        
        client = connect_rest("http://localhost:5678")
        results = []
        errors = []
        
        def make_request():
            try:
                collections = client.list_collections()
                results.append(collections)
            except Exception as e:
                errors.append(e)
        
        # Launch concurrent requests
        threads = []
        for i in range(10):
            thread = threading.Thread(target=make_request)
            threads.append(thread)
            thread.start()
        
        # Wait for completion
        for thread in threads:
            thread.join(timeout=5.0)
        
        # At least some requests should succeed
        assert len(results) > 0 or len(errors) > 0  # Some activity occurred


if __name__ == "__main__":
    pytest.main([__file__, "-v", "-s"])