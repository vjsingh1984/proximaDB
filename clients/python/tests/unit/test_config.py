"""
Test suite for ProximaDB configuration
"""
import os
import pytest
from pydantic import ValidationError
from proximadb.config import (
    Protocol,
    LogLevel,
    RetryConfig,
    ConnectionConfig,
    CompressionConfig,
    TLSConfig,
    ClientConfig,
    load_config,
    load_config_file
)


class TestProtocolEnum:
    """Test Protocol enum"""
    
    def test_protocol_values(self):
        """Test protocol enum values"""
        assert Protocol.AUTO.value == "auto"
        assert Protocol.GRPC.value == "grpc"
        assert Protocol.REST.value == "rest"


class TestLogLevelEnum:
    """Test LogLevel enum"""
    
    def test_log_level_values(self):
        """Test log level enum values"""
        assert LogLevel.DEBUG.value == "DEBUG"
        assert LogLevel.INFO.value == "INFO"
        assert LogLevel.WARNING.value == "WARNING"
        assert LogLevel.ERROR.value == "ERROR"
        assert LogLevel.CRITICAL.value == "CRITICAL"


class TestRetryConfig:
    """Test RetryConfig model"""
    
    def test_retry_config_defaults(self):
        """Test default retry configuration"""
        config = RetryConfig()
        assert config.max_retries == 3
        assert config.backoff_factor == 2.0
        assert config.max_backoff == 60.0
        assert config.retry_on_timeout is True
        assert config.retry_on_connection_error is True
        assert config.retry_on_server_error is True
        assert config.retry_status_codes == [429, 500, 502, 503, 504]
    
    def test_retry_config_custom(self):
        """Test custom retry configuration"""
        config = RetryConfig(
            max_retries=5,
            backoff_factor=3.0,
            max_backoff=120.0,
            retry_on_timeout=False
        )
        assert config.max_retries == 5
        assert config.backoff_factor == 3.0
        assert config.max_backoff == 120.0
        assert config.retry_on_timeout is False
    
    def test_retry_config_validation(self):
        """Test retry config validation"""
        # Test max_retries bounds
        with pytest.raises(ValidationError):
            RetryConfig(max_retries=-1)
        
        with pytest.raises(ValidationError):
            RetryConfig(max_retries=11)
        
        # Test backoff_factor bounds
        with pytest.raises(ValidationError):
            RetryConfig(backoff_factor=0.5)


class TestConnectionConfig:
    """Test ConnectionConfig model"""
    
    def test_connection_config_defaults(self):
        """Test default connection configuration"""
        config = ConnectionConfig()
        assert config.pool_size == 10
        assert config.pool_maxsize == 100
        assert config.keepalive_timeout == 30.0
        assert config.read_timeout == 30.0
        assert config.connect_timeout == 10.0
        assert config.total_timeout == 60.0
    
    def test_connection_config_custom(self):
        """Test custom connection configuration"""
        config = ConnectionConfig(
            pool_size=20,
            pool_maxsize=200,
            read_timeout=60.0
        )
        assert config.pool_size == 20
        assert config.pool_maxsize == 200
        assert config.read_timeout == 60.0
    
    def test_connection_config_validation(self):
        """Test connection config validation"""
        # Test pool_size bounds
        with pytest.raises(ValidationError):
            ConnectionConfig(pool_size=0)
        
        with pytest.raises(ValidationError):
            ConnectionConfig(pool_size=101)
        
        # Test timeout bounds
        with pytest.raises(ValidationError):
            ConnectionConfig(read_timeout=0.5)


class TestCompressionConfig:
    """Test CompressionConfig model"""
    
    def test_compression_config_defaults(self):
        """Test default compression configuration"""
        config = CompressionConfig()
        assert config.enabled is True
        assert config.algorithm == "gzip"
        assert config.threshold_bytes == 1024
        assert config.level is None
    
    def test_compression_config_custom(self):
        """Test custom compression configuration"""
        config = CompressionConfig(
            enabled=False,
            algorithm="deflate",
            threshold_bytes=2048,
            level=6
        )
        assert config.enabled is False
        assert config.algorithm == "deflate"
        assert config.threshold_bytes == 2048
        assert config.level == 6
    
    def test_compression_config_validation(self):
        """Test compression config validation"""
        # Test level bounds
        with pytest.raises(ValidationError):
            CompressionConfig(level=0)
        
        with pytest.raises(ValidationError):
            CompressionConfig(level=10)


class TestTLSConfig:
    """Test TLSConfig model"""
    
    def test_tls_config_defaults(self):
        """Test default TLS configuration"""
        config = TLSConfig()
        assert config.verify is True
        assert config.ca_bundle is None
        assert config.cert_file is None
        assert config.key_file is None
        assert config.sni_hostname is None
    
    def test_tls_config_custom(self):
        """Test custom TLS configuration"""
        config = TLSConfig(
            verify=False,
            ca_bundle="/path/to/ca.pem",
            cert_file="/path/to/cert.pem",
            key_file="/path/to/key.pem",
            sni_hostname="example.com"
        )
        assert config.verify is False
        assert config.ca_bundle == "/path/to/ca.pem"
        assert config.cert_file == "/path/to/cert.pem"
        assert config.key_file == "/path/to/key.pem"
        assert config.sni_hostname == "example.com"


class TestClientConfig:
    """Test ClientConfig model"""
    
    def test_client_config_defaults(self):
        """Test default client configuration"""
        config = ClientConfig(url="http://localhost:5678")
        assert config.api_key is None
        assert config.url == "http://localhost:5678"
        assert config.protocol == Protocol.AUTO
        assert config.log_level == LogLevel.INFO
        assert config.user_agent is None
        assert isinstance(config.retry, RetryConfig)
        assert isinstance(config.connection, ConnectionConfig)
        assert isinstance(config.compression, CompressionConfig)
        assert isinstance(config.tls, TLSConfig)
        assert config.enable_debug_logging is False
        assert config.validate_inputs is True
    
    def test_client_config_custom(self):
        """Test custom client configuration"""
        config = ClientConfig(
            url="https://api.proximadb.com",
            api_key="test_api_key",
            protocol=Protocol.GRPC,
            log_level=LogLevel.DEBUG,
            enable_debug_logging=True,
            validate_inputs=False
        )
        assert config.api_key == "test_api_key"
        assert config.url == "https://api.proximadb.com"
        assert config.protocol == Protocol.GRPC
        assert config.log_level == LogLevel.DEBUG
        assert config.enable_debug_logging is True
        assert config.validate_inputs is False
    
    def test_client_config_nested(self):
        """Test client config with nested configurations"""
        config = ClientConfig(
            url="http://localhost:5678",
            retry=RetryConfig(max_retries=5),
            connection=ConnectionConfig(pool_size=20),
            compression=CompressionConfig(enabled=False)
        )
        assert config.retry.max_retries == 5
        assert config.connection.pool_size == 20
        assert config.compression.enabled is False
    
    def test_client_config_from_dict(self):
        """Test creating client config from dict"""
        config_dict = {
            "url": "grpc://localhost:5679",
            "api_key": "test_key_12345",
            "protocol": "grpc",
            "log_level": "DEBUG",
            "retry": {
                "max_retries": 5,
                "backoff_factor": 3.0
            },
            "connection": {
                "pool_size": 20,
                "read_timeout": 60.0
            }
        }
        config = ClientConfig(**config_dict)
        assert config.api_key == "test_key_12345"
        assert config.url == "grpc://localhost:5679"
        assert config.protocol == Protocol.GRPC
        assert config.log_level == LogLevel.DEBUG
        assert config.retry.max_retries == 5
        assert config.connection.pool_size == 20


class TestConfigFunctions:
    """Test configuration helper functions"""
    
    def test_load_config_defaults(self):
        """Test load_config function with defaults"""
        config = load_config(url="http://localhost:5678")
        assert isinstance(config, ClientConfig)
        assert config.protocol == Protocol.AUTO
        assert config.log_level == LogLevel.INFO
    
    def test_client_config_from_env(self):
        """Test ClientConfig.from_env method"""
        # Save current env
        old_env = dict(os.environ)
        
        try:
            # Set test environment variables
            os.environ["PROXIMADB_URL"] = "grpc://env.host:5679"
            os.environ["PROXIMADB_API_KEY"] = "env_test_key"
            os.environ["PROXIMADB_PROTOCOL"] = "grpc"
            os.environ["PROXIMADB_LOG_LEVEL"] = "DEBUG"
            os.environ["PROXIMADB_DEBUG"] = "true"
            os.environ["PROXIMADB_MAX_RETRIES"] = "5"
            os.environ["PROXIMADB_POOL_SIZE"] = "20"
            os.environ["PROXIMADB_TLS_VERIFY"] = "false"
            
            config = ClientConfig.from_env()
            assert config.url == "grpc://env.host:5679"
            assert config.api_key == "env_test_key"
            assert config.protocol == Protocol.GRPC
            assert config.log_level == LogLevel.DEBUG
            assert config.enable_debug_logging is True
            assert config.retry.max_retries == 5
            assert config.connection.pool_size == 20
            assert config.tls.verify is False
            
        finally:
            # Restore environment
            os.environ.clear()
            os.environ.update(old_env)
    
    def test_load_config_with_parameters(self):
        """Test load_config function with parameters"""
        config = load_config(
            url="https://api.proximadb.com",
            api_key="test_key_12345",
            protocol=Protocol.GRPC,
            log_level=LogLevel.DEBUG
        )
        assert config.url == "https://api.proximadb.com"
        assert config.api_key == "test_key_12345"
        assert config.protocol == Protocol.GRPC
        assert config.log_level == LogLevel.DEBUG
    
    def test_client_config_url_validation(self):
        """Test URL validation in ClientConfig"""
        # Valid URLs
        config = ClientConfig(url="http://localhost:5678")
        assert config.url == "http://localhost:5678"
        
        config = ClientConfig(url="https://api.proximadb.com")
        assert config.url == "https://api.proximadb.com"
        
        config = ClientConfig(url="grpc://localhost:5679")
        assert config.url == "grpc://localhost:5679"
        
        # Host:port format needs scheme
        config = ClientConfig(url="https://localhost:5679")
        assert config.url == "https://localhost:5679"
        
        # Invalid URLs
        with pytest.raises(ValidationError):
            ClientConfig(url="")
        
        with pytest.raises(ValidationError):
            ClientConfig(url="ftp://localhost:21")
    
    def test_client_config_methods(self):
        """Test ClientConfig methods"""
        config = ClientConfig(url="https://api.proximadb.com")
        
        # Test is_secure
        assert config.is_secure() is True
        
        # Test get_host_port
        host, port = config.get_host_port()
        assert host == "api.proximadb.com"
        assert port == 443
        
        # Test should_use_grpc
        config.protocol = Protocol.GRPC
        assert config.should_use_grpc() is True
        
        config.protocol = Protocol.REST
        assert config.should_use_grpc() is False
        
        config.protocol = Protocol.AUTO
        assert config.should_use_grpc() is True  # Default to gRPC
        
        # Test get_protocol_url
        assert config.get_protocol_url(Protocol.REST) == "https://api.proximadb.com:5678"
        assert config.get_protocol_url(Protocol.GRPC) == "api.proximadb.com:5679"
    
    def test_client_config_headers(self):
        """Test ClientConfig header methods"""
        config = ClientConfig(
            url="http://localhost:5678",
            api_key="test_key_12345",
            user_agent="custom-agent/1.0",
            custom_headers={"X-Custom": "value"}
        )
        
        # Test get_base_headers
        headers = config.get_base_headers()
        assert headers["Authorization"] == "Bearer test_key_12345"
        assert headers["User-Agent"] == "custom-agent/1.0"
        assert headers["X-Custom"] == "value"
        assert headers["Accept"] == "application/json"
        assert headers["Content-Type"] == "application/json"
        
        # Test get_grpc_metadata
        metadata = config.get_grpc_metadata()
        assert ("authorization", "Bearer test_key_12345") in metadata
        assert ("user-agent", "custom-agent/1.0") in metadata
        assert ("x-custom", "value") in metadata