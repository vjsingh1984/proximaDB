"""
Unit tests for ProximaDB configuration module.
Tests configuration classes and environment variable loading.
"""

import pytest
import os
from unittest.mock import patch
from proximadb.config import (
    ClientConfig, RetryConfig, ConnectionConfig, TLSConfig,
    CompressionConfig, Protocol, LogLevel, load_config
)
from proximadb.exceptions import ConfigurationError


class TestRetryConfig:
    """Test RetryConfig class"""
    
    def test_default_retry_config(self):
        """Test default retry configuration"""
        config = RetryConfig()
        assert config.max_retries == 3
        assert config.backoff_factor == 2.0
        assert config.retry_status_codes == [429, 500, 502, 503, 504]
        assert config.retry_on_timeout is True
    
    def test_custom_retry_config(self):
        """Test custom retry configuration"""
        config = RetryConfig(
            max_retries=5,
            backoff_factor=3.0,
            retry_status_codes=[500, 503],
            retry_on_timeout=False
        )
        assert config.max_retries == 5
        assert config.backoff_factor == 3.0
        assert config.retry_status_codes == [500, 503]
        assert config.retry_on_timeout is False
    
    def test_retry_config_validation(self):
        """Test retry configuration validation"""
        # Test negative max_retries
        with pytest.raises(ValueError):
            RetryConfig(max_retries=-1)
        
        # Test negative backoff_factor
        with pytest.raises(ValueError):
            RetryConfig(backoff_factor=-0.5)


class TestConnectionConfig:
    """Test ConnectionConfig class"""
    
    def test_default_connection_config(self):
        """Test default connection configuration"""
        config = ConnectionConfig()
        assert config.pool_size == 10
        assert config.pool_maxsize == 100
        assert config.read_timeout == 30.0
        assert config.connect_timeout == 10.0
        assert config.keepalive_timeout == 30.0
    
    def test_custom_connection_config(self):
        """Test custom connection configuration"""
        config = ConnectionConfig(
            pool_size=20,
            pool_maxsize=200,
            read_timeout=60.0,
            connect_timeout=15.0,
            keepalive_timeout=45.0
        )
        assert config.pool_size == 20
        assert config.pool_maxsize == 200
        assert config.read_timeout == 60.0
        assert config.connect_timeout == 15.0
        assert config.keepalive_timeout == 45.0
    
    def test_connection_config_validation(self):
        """Test connection configuration validation"""
        # Test negative pool_size
        with pytest.raises(ValueError):
            ConnectionConfig(pool_size=0)
        
        # Test negative timeout values
        with pytest.raises(ValueError):
            ConnectionConfig(read_timeout=-1.0)


class TestTLSConfig:
    """Test TLSConfig class"""
    
    def test_default_tls_config(self):
        """Test default TLS configuration"""
        config = TLSConfig()
        assert config.verify is True
        assert config.ca_bundle is None
        assert config.cert_file is None
        assert config.key_file is None
    
    def test_custom_tls_config(self):
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


class TestCompressionConfig:
    """Test CompressionConfig class"""
    
    def test_default_compression_config(self):
        """Test default compression configuration"""
        config = CompressionConfig()
        assert hasattr(config, 'enabled')
        assert hasattr(config, 'algorithm')
    
    def test_custom_compression_config(self):
        """Test custom compression configuration"""
        config = CompressionConfig(enabled=True)
        assert config.enabled is True


class TestClientConfig:
    """Test ClientConfig class"""
    
    def test_minimal_client_config(self):
        """Test minimal client configuration"""
        config = ClientConfig(url="http://localhost:5678")
        assert config.url == "http://localhost:5678"
        assert config.api_key is None
        assert config.timeout == 30.0
        assert isinstance(config.retry, RetryConfig)
        assert isinstance(config.connection, ConnectionConfig)
        assert isinstance(config.tls, TLSConfig)
        assert isinstance(config.compression, CompressionConfig)
    
    def test_full_client_config(self):
        """Test complete client configuration"""
        retry_config = RetryConfig(max_retries=5)
        connection_config = ConnectionConfig(pool_size=20)
        tls_config = TLSConfig(verify=False)
        compression_config = CompressionConfig(enabled=True)
        
        config = ClientConfig(
            url="https://api.proximadb.com",
            api_key="test-key-1234567890",
            timeout=60.0,
            retry=retry_config,
            connection=connection_config,
            tls=tls_config,
            compression=compression_config,
            default_batch_size=100,
            max_concurrent_requests=5,
            log_level=LogLevel.DEBUG,
            enable_debug_logging=True
        )
        
        assert config.url == "https://api.proximadb.com"
        assert config.api_key == "test-key-1234567890"
        assert config.timeout == 60.0
        assert config.retry.max_retries == 5
        assert config.connection.pool_size == 20
        assert config.tls.verify is False
        assert config.compression.enabled is True
        assert config.default_batch_size == 100
        assert config.max_concurrent_requests == 5
        assert config.log_level == LogLevel.DEBUG
        assert config.enable_debug_logging is True
    
    def test_client_config_validation(self):
        """Test client configuration validation"""
        # Test missing URL
        with pytest.raises(ValueError):
            ClientConfig(url="")
        
        # Test invalid timeout
        with pytest.raises(ValueError):
            ClientConfig(url="http://localhost", timeout=-1.0)
        
        # Test invalid batch size
        with pytest.raises(ValueError):
            ClientConfig(url="http://localhost", default_batch_size=0)
    
    def test_from_env_minimal(self):
        """Test creating config from environment variables - minimal"""
        with patch.dict(os.environ, {"PROXIMADB_URL": "http://localhost:5678"}, clear=True):
            config = ClientConfig.from_env()
            assert config.url == "http://localhost:5678"
    
    def test_from_env_comprehensive(self):
        """Test creating config from environment variables - comprehensive"""
        env_vars = {
            "PROXIMADB_URL": "https://api.proximadb.com",
            "PROXIMADB_API_KEY": "test-key-1234567890",
            "PROXIMADB_TIMEOUT": "60.0",
            "PROXIMADB_MAX_RETRIES": "5",
            "PROXIMADB_BACKOFF_FACTOR": "2.0",
            "PROXIMADB_POOL_SIZE": "20",
            "PROXIMADB_READ_TIMEOUT": "45.0",
            "PROXIMADB_TLS_VERIFY": "false",
            "PROXIMADB_CA_BUNDLE": "/path/to/ca.pem",
            "PROXIMADB_LOG_LEVEL": "DEBUG",
            "PROXIMADB_DEBUG": "true",
            "PROXIMADB_BATCH_SIZE": "100",
            "PROXIMADB_MAX_CONCURRENT": "10"
        }
        
        with patch.dict(os.environ, env_vars, clear=True):
            config = ClientConfig.from_env()
            
            assert config.url == "https://api.proximadb.com"
            assert config.api_key == "test-key-1234567890"
            assert config.timeout == 60.0
            assert config.retry.max_retries == 5
            assert config.retry.backoff_factor == 2.0
            assert config.connection.pool_size == 20
            assert config.connection.read_timeout == 45.0
            assert config.tls.verify is False
            assert config.tls.ca_bundle == "/path/to/ca.pem"
            assert config.log_level == LogLevel.DEBUG
            assert config.enable_debug_logging is True
            assert config.default_batch_size == 100
            assert config.max_concurrent_requests == 10
    
    def test_from_env_missing_url(self):
        """Test error when URL is missing from environment"""
        with patch.dict(os.environ, {}, clear=True):
            with pytest.raises(ValueError, match="URL must be provided"):
                ClientConfig.from_env()
    
    def test_from_env_with_overrides(self):
        """Test environment config with constructor overrides"""
        with patch.dict(os.environ, {"PROXIMADB_URL": "http://localhost:5678"}, clear=True):
            config = ClientConfig.from_env(timeout=120.0, api_key="override-key")
            assert config.url == "http://localhost:5678"
            assert config.timeout == 120.0
            assert config.api_key == "override-key"


class TestLoadConfig:
    """Test load_config function"""
    
    def test_load_config_basic(self):
        """Test basic config loading"""
        config = load_config(url="http://localhost:5678")
        assert isinstance(config, ClientConfig)
        assert config.url == "http://localhost:5678"
    
    def test_load_config_with_api_key(self):
        """Test config loading with API key"""
        config = load_config(url="http://localhost:5678", api_key="test-key-1234567890")
        assert config.url == "http://localhost:5678"
        assert config.api_key == "test-key-1234567890"
    
    def test_load_config_with_kwargs(self):
        """Test config loading with additional kwargs"""
        config = load_config(
            url="http://localhost:5678",
            timeout=45.0,
            default_batch_size=50
        )
        assert config.url == "http://localhost:5678"
        assert config.timeout == 45.0
        assert config.default_batch_size == 50
    
    @patch.dict(os.environ, {"PROXIMADB_URL": "http://env-url:5678"}, clear=True)
    def test_load_config_from_env(self):
        """Test config loading from environment when no URL provided"""
        config = load_config()
        assert config.url == "http://env-url:5678"
    
    def test_load_config_url_override(self):
        """Test that explicit URL overrides environment"""
        with patch.dict(os.environ, {"PROXIMADB_URL": "http://env-url:5678"}, clear=True):
            config = load_config(url="http://override:5678")
            assert config.url == "http://override:5678"


class TestConfigHelpers:
    """Test configuration helper functions and edge cases"""
    
    def test_protocol_detection(self):
        """Test protocol detection from URL"""
        # HTTP
        config = ClientConfig(url="http://localhost:5678")
        assert "http" in config.url
        
        # HTTPS
        config = ClientConfig(url="https://secure.proximadb.com")
        assert "https" in config.url
    
    def test_default_values_consistency(self):
        """Test that default values are consistent across configs"""
        config1 = ClientConfig(url="http://localhost:5678")
        config2 = ClientConfig(url="http://localhost:5678")
        
        assert config1.timeout == config2.timeout
        assert config1.retry.max_retries == config2.retry.max_retries
        assert config1.connection.pool_size == config2.connection.pool_size
    
    def test_config_immutability_after_creation(self):
        """Test that config objects can be modified after creation"""
        config = ClientConfig(url="http://localhost:5678")
        original_timeout = config.timeout
        
        # Configs should be mutable (Pydantic allows this)
        config.timeout = 60.0
        assert config.timeout == 60.0
        assert config.timeout != original_timeout
    
    def test_nested_config_objects(self):
        """Test that nested config objects are properly created"""
        config = ClientConfig(url="http://localhost:5678")
        
        # Check that nested objects exist and have correct types
        assert hasattr(config, 'retry')
        assert isinstance(config.retry, RetryConfig)
        assert hasattr(config, 'connection')
        assert isinstance(config.connection, ConnectionConfig)
        assert hasattr(config, 'tls')
        assert isinstance(config.tls, TLSConfig)
        assert hasattr(config, 'compression')
        assert isinstance(config.compression, CompressionConfig)