#!/usr/bin/env python3
"""
Unit tests for ProximaDB configuration module
"""
import os
import pytest
from unittest.mock import patch
from proximadb.config import (
    ClientConfig, 
    RetryConfig, 
    ConnectionConfig, 
    TLSConfig,
    load_config,
    Protocol
)
from proximadb.exceptions import ConfigurationError


class TestRetryConfig:
    """Test RetryConfig class"""
    
    def test_retry_config_defaults(self):
        """Test default retry configuration"""
        config = RetryConfig()
        assert config.max_retries == 3
        assert config.backoff_factor == 2.0
        assert config.max_backoff == 60.0
        
    def test_retry_config_custom(self):
        """Test custom retry configuration"""
        config = RetryConfig(
            max_retries=5,
            backoff_factor=3.0,
            max_backoff=120.0
        )
        assert config.max_retries == 5
        assert config.backoff_factor == 3.0
        assert config.max_backoff == 120.0


class TestConnectionConfig:
    """Test ConnectionConfig class"""
    
    def test_connection_config_defaults(self):
        """Test default connection configuration"""
        config = ConnectionConfig()
        assert config.pool_size == 10
        assert config.read_timeout == 30.0
        assert config.connect_timeout == 10.0
        
    def test_connection_config_custom(self):
        """Test custom connection configuration"""
        config = ConnectionConfig(
            pool_size=20,
            read_timeout=60.0,
            connect_timeout=15.0
        )
        assert config.pool_size == 20
        assert config.read_timeout == 60.0
        assert config.connect_timeout == 15.0


class TestTLSConfig:
    """Test TLSConfig class"""
    
    def test_tls_config_defaults(self):
        """Test default TLS configuration"""
        config = TLSConfig()
        assert config.verify is True
        assert config.ca_bundle is None
        assert config.cert_file is None
        assert config.key_file is None
        
    def test_tls_config_custom(self):
        """Test custom TLS configuration"""
        config = TLSConfig(
            verify=False,
            ca_bundle="/path/to/ca.pem",
            cert_file="/path/to/cert.pem",
            key_file="/path/to/key.pem"
        )
        assert config.verify is False
        assert config.ca_bundle == "/path/to/ca.pem"
        assert config.cert_file == "/path/to/cert.pem"
        assert config.key_file == "/path/to/key.pem"


class TestClientConfig:
    """Test ClientConfig class"""
    
    def test_client_config_minimal(self):
        """Test minimal client configuration"""
        config = ClientConfig(url="http://localhost:5678")
        assert config.url == "http://localhost:5678"
        assert config.api_key is None
        assert config.protocol == Protocol.AUTO
        assert config.timeout == 30.0
        
    def test_client_config_full(self):
        """Test full client configuration"""
        retry_config = RetryConfig(max_retries=5)
        connection_config = ConnectionConfig(pool_size=20)
        tls_config = TLSConfig(verify=False)
        
        config = ClientConfig(
            url="https://api.proximadb.com",
            api_key="test-key-1234567890",  # Make it long enough
            protocol=Protocol.REST,
            timeout=60.0,
            retry=retry_config,
            connection=connection_config,
            tls=tls_config,
            log_level="DEBUG",
            enable_debug_logging=True,
            default_batch_size=1000,
            max_concurrent_requests=50
        )
        
        assert config.url == "https://api.proximadb.com"
        assert config.api_key == "test-key-1234567890"
        assert config.protocol == Protocol.REST
        assert config.timeout == 60.0
        assert config.retry.max_retries == 5
        assert config.connection.pool_size == 20
        assert config.tls.verify is False
        assert config.log_level == "DEBUG"
        assert config.enable_debug_logging is True
        assert config.default_batch_size == 1000
        assert config.max_concurrent_requests == 50
        
    def test_client_config_invalid_url(self):
        """Test invalid URL validation"""
        with pytest.raises(ValueError, match="URL must use http or https scheme"):
            ClientConfig(url="ftp://invalid.com")
            
        with pytest.raises(ValueError, match="URL must use http or https scheme"):
            ClientConfig(url="localhost:5678")
            
    def test_client_config_url_validation(self):
        """Test URL validation"""
        # Valid URLs
        valid_urls = [
            "http://localhost:5678",
            "https://api.proximadb.com",
            "http://127.0.0.1:8080",
            "https://subdomain.domain.com:443"
        ]
        
        for url in valid_urls:
            config = ClientConfig(url=url)
            assert config.url == url
            
    def test_client_config_from_env_empty(self):
        """Test from_env with no environment variables"""
        with pytest.raises(ValueError, match="URL must be provided"):
            ClientConfig.from_env()
            
    @patch.dict(os.environ, {
        'PROXIMADB_URL': 'http://localhost:5678',
        'PROXIMADB_API_KEY': 'test-key-1234567890',
        'PROXIMADB_PROTOCOL': 'rest',
        'PROXIMADB_TIMEOUT': '60.0'
    })
    def test_client_config_from_env_basic(self):
        """Test from_env with basic environment variables"""
        config = ClientConfig.from_env()
        assert config.url == "http://localhost:5678"
        assert config.api_key == "test-key-1234567890"
        assert config.protocol == "rest"
        assert config.timeout == 60.0
        
    @patch.dict(os.environ, {
        'PROXIMADB_URL': 'https://api.proximadb.com',
        'PROXIMADB_MAX_RETRIES': '5',
        'PROXIMADB_BACKOFF_FACTOR': '2.5',
        'PROXIMADB_POOL_SIZE': '20',
        'PROXIMADB_READ_TIMEOUT': '45.0',
        'PROXIMADB_TLS_VERIFY': 'false',
        'PROXIMADB_CA_BUNDLE': '/path/to/ca.pem',
        'PROXIMADB_LOG_LEVEL': 'DEBUG',
        'PROXIMADB_DEBUG': 'true',
        'PROXIMADB_BATCH_SIZE': '1000',
        'PROXIMADB_MAX_CONCURRENT': '50'
    })
    def test_client_config_from_env_full(self):
        """Test from_env with all environment variables"""
        config = ClientConfig.from_env()
        assert config.url == "https://api.proximadb.com"
        assert config.retry.max_retries == 5
        assert config.retry.backoff_factor == 2.5
        assert config.connection.pool_size == 20
        assert config.connection.read_timeout == 45.0
        assert config.tls.verify is False
        assert config.tls.ca_bundle == "/path/to/ca.pem"
        assert config.log_level == "DEBUG"
        assert config.enable_debug_logging is True
        assert config.default_batch_size == 1000
        assert config.max_concurrent_requests == 50
        
    def test_client_config_from_env_with_overrides(self):
        """Test from_env with overrides"""
        with patch.dict(os.environ, {'PROXIMADB_URL': 'http://localhost:5678'}):
            config = ClientConfig.from_env(
                url="https://override.com",
                api_key="override-key-1234567890",
                timeout=120.0
            )
            assert config.url == "https://override.com"
            assert config.api_key == "override-key-1234567890"
            assert config.timeout == 120.0


class TestLoadConfig:
    """Test load_config function"""
    
    def test_load_config_basic(self):
        """Test basic load_config functionality"""
        config = load_config(url="http://localhost:5678")
        assert isinstance(config, ClientConfig)
        assert config.url == "http://localhost:5678"
        
    def test_load_config_with_params(self):
        """Test load_config with additional parameters"""
        config = load_config(
            url="https://api.proximadb.com",
            api_key="test-key-1234567890",
            timeout=60.0,
            protocol=Protocol.GRPC
        )
        assert config.url == "https://api.proximadb.com"
        assert config.api_key == "test-key-1234567890"
        assert config.timeout == 60.0
        assert config.protocol == Protocol.GRPC
        
    def test_load_config_invalid_url(self):
        """Test load_config with invalid URL"""
        with pytest.raises(ValueError):
            load_config(url="ftp://invalid-url")
            
    @patch.dict(os.environ, {'PROXIMADB_URL': 'http://localhost:5678'})
    def test_load_config_from_env(self):
        """Test load_config using environment variables"""
        config = load_config()
        assert config.url == "http://localhost:5678"


class TestProtocol:
    """Test Protocol enum"""
    
    def test_protocol_values(self):
        """Test Protocol enum values"""
        assert Protocol.AUTO == "auto"
        assert Protocol.REST == "rest"
        assert Protocol.GRPC == "grpc"
        
    def test_protocol_detection(self):
        """Test protocol detection logic"""
        # This would be tested in the actual client code
        # but we can test the enum values here
        protocols = [Protocol.AUTO, Protocol.REST, Protocol.GRPC]
        assert len(protocols) == 3
        assert all(isinstance(p.value, str) for p in protocols)


class TestConfigErrors:
    """Test configuration error handling"""
    
    def test_invalid_retry_config(self):
        """Test invalid retry configuration"""
        with pytest.raises(ValueError):
            RetryConfig(max_retries=-1)
            
    def test_invalid_connection_config(self):
        """Test invalid connection configuration"""
        with pytest.raises(ValueError):
            ConnectionConfig(pool_size=0)
            
        with pytest.raises(ValueError):
            ConnectionConfig(read_timeout=-1.0)
            
    def test_invalid_timeout(self):
        """Test invalid timeout values"""
        with pytest.raises(ValueError):
            ClientConfig(url="http://localhost:5678", timeout=-1.0)
            
    def test_invalid_batch_size(self):
        """Test invalid batch size"""
        with pytest.raises(ValueError):
            ClientConfig(
                url="http://localhost:5678",
                default_batch_size=0
            )
            
    def test_invalid_concurrent_requests(self):
        """Test invalid concurrent requests"""
        with pytest.raises(ValueError):
            ClientConfig(
                url="http://localhost:5678",
                max_concurrent_requests=0
            )


class TestConfigSerialization:
    """Test configuration serialization"""
    
    def test_config_dict_conversion(self):
        """Test converting config to dict"""
        config = ClientConfig(
            url="http://localhost:5678",
            api_key="test-key-1234567890",
            timeout=60.0
        )
        
        # Test that config can be serialized
        config_dict = config.model_dump()
        assert config_dict["url"] == "http://localhost:5678"
        assert config_dict["api_key"] == "test-key-1234567890"
        assert config_dict["timeout"] == 60.0
        
    def test_config_json_serialization(self):
        """Test JSON serialization of config"""
        config = ClientConfig(
            url="http://localhost:5678",
            protocol=Protocol.REST
        )
        
        json_str = config.model_dump_json()
        assert "http://localhost:5678" in json_str
        assert "rest" in json_str