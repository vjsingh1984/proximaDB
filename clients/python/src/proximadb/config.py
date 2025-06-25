"""
ProximaDB Python Client - Configuration

Copyright 2024 Vijaykumar Singh

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
"""

import os
from typing import Dict, Optional, Union
from urllib.parse import urlparse
from enum import Enum

from pydantic import BaseModel, Field, field_validator, ConfigDict


class Protocol(str, Enum):
    """Supported communication protocols"""
    AUTO = "auto"      # Auto-select best available
    AVRO = "avro"      # AvroRPC (highest performance)
    GRPC = "grpc"      # gRPC (proven performance)
    REST = "rest"      # REST (web compatibility)


class LogLevel(str, Enum):
    """Logging levels"""
    DEBUG = "DEBUG"
    INFO = "INFO"
    WARNING = "WARNING"
    ERROR = "ERROR"
    CRITICAL = "CRITICAL"


class RetryConfig(BaseModel):
    """Retry configuration"""
    max_retries: int = Field(default=3, ge=0, le=10)
    backoff_factor: float = Field(default=2.0, ge=1.0, le=10.0)
    max_backoff: float = Field(default=60.0, ge=1.0)
    retry_on_timeout: bool = True
    retry_on_connection_error: bool = True
    retry_on_server_error: bool = True
    retry_status_codes: list = Field(default_factory=lambda: [429, 500, 502, 503, 504])


class ConnectionConfig(BaseModel):
    """Connection pool configuration"""
    pool_size: int = Field(default=10, ge=1, le=100)
    pool_maxsize: int = Field(default=100, ge=1, le=1000)
    keepalive_timeout: float = Field(default=30.0, ge=1.0)
    read_timeout: float = Field(default=30.0, ge=1.0)
    connect_timeout: float = Field(default=10.0, ge=1.0)
    total_timeout: float = Field(default=60.0, ge=1.0)


class CompressionConfig(BaseModel):
    """Compression configuration"""
    enabled: bool = True
    algorithm: str = "gzip"  # gzip, deflate, br
    threshold_bytes: int = Field(default=1024, ge=0)
    level: Optional[int] = Field(default=None, ge=1, le=9)


class TLSConfig(BaseModel):
    """TLS/SSL configuration"""
    verify: bool = True
    ca_bundle: Optional[str] = None
    cert_file: Optional[str] = None
    key_file: Optional[str] = None
    sni_hostname: Optional[str] = None


class ClientConfig(BaseModel):
    """Complete client configuration"""
    
    # Connection settings
    url: str = Field(..., description="ProximaDB server URL")
    api_key: Optional[str] = Field(default=None, description="API key for authentication")
    protocol: Protocol = Field(default=Protocol.AUTO, description="Communication protocol")
    
    # Timeouts
    timeout: float = Field(default=30.0, ge=0.1, le=300.0, description="Default request timeout")
    
    # Retry configuration
    retry: RetryConfig = Field(default_factory=RetryConfig)
    
    # Connection configuration
    connection: ConnectionConfig = Field(default_factory=ConnectionConfig)
    
    # Compression
    compression: CompressionConfig = Field(default_factory=CompressionConfig)
    
    # TLS/SSL
    tls: TLSConfig = Field(default_factory=TLSConfig)
    
    # Headers and metadata
    user_agent: Optional[str] = Field(default=None, description="Custom User-Agent header")
    custom_headers: Dict[str, str] = Field(default_factory=dict)
    
    # Logging
    log_level: LogLevel = Field(default=LogLevel.INFO)
    enable_debug_logging: bool = False
    
    # Performance tuning
    enable_keepalive: bool = True
    enable_http2: bool = True
    max_concurrent_requests: int = Field(default=100, ge=1, le=1000)
    
    # Client behavior
    validate_inputs: bool = True
    auto_convert_numpy: bool = True
    default_batch_size: int = Field(default=1000, ge=1, le=10000)
    
    model_config = ConfigDict(use_enum_values=True)
    
    @field_validator('url')
    def validate_url(cls, v: str) -> str:
        """Validate and normalize URL"""
        if not v:
            raise ValueError("URL cannot be empty")
        
        parsed = urlparse(v)
        if not parsed.scheme:
            # Add https as default scheme
            v = f"https://{v}"
            parsed = urlparse(v)
        
        if parsed.scheme not in ('http', 'https'):
            raise ValueError("URL must use http or https scheme")
        
        if not parsed.netloc:
            raise ValueError("URL must include hostname")
        
        return v
    
    @field_validator('api_key')
    def validate_api_key(cls, v: Optional[str]) -> Optional[str]:
        """Validate API key format"""
        if v and len(v) < 10:
            raise ValueError("API key appears to be too short")
        return v
    
    @classmethod
    def from_env(cls, **overrides) -> "ClientConfig":
        """Create configuration from environment variables"""
        config_dict = {}
        
        # Basic settings
        if url := os.getenv("PROXIMADB_URL"):
            config_dict["url"] = url
        if api_key := os.getenv("PROXIMADB_API_KEY"):
            config_dict["api_key"] = api_key
        if protocol := os.getenv("PROXIMADB_PROTOCOL"):
            config_dict["protocol"] = protocol
        if timeout := os.getenv("PROXIMADB_TIMEOUT"):
            config_dict["timeout"] = float(timeout)
        
        # Retry settings
        retry_config = {}
        if max_retries := os.getenv("PROXIMADB_MAX_RETRIES"):
            retry_config["max_retries"] = int(max_retries)
        if backoff_factor := os.getenv("PROXIMADB_BACKOFF_FACTOR"):
            retry_config["backoff_factor"] = float(backoff_factor)
        if retry_config:
            config_dict["retry"] = RetryConfig(**retry_config)
        
        # Connection settings
        connection_config = {}
        if pool_size := os.getenv("PROXIMADB_POOL_SIZE"):
            connection_config["pool_size"] = int(pool_size)
        if read_timeout := os.getenv("PROXIMADB_READ_TIMEOUT"):
            connection_config["read_timeout"] = float(read_timeout)
        if connection_config:
            config_dict["connection"] = ConnectionConfig(**connection_config)
        
        # TLS settings
        tls_config = {}
        if verify := os.getenv("PROXIMADB_TLS_VERIFY"):
            tls_config["verify"] = verify.lower() in ("true", "1", "yes")
        if ca_bundle := os.getenv("PROXIMADB_CA_BUNDLE"):
            tls_config["ca_bundle"] = ca_bundle
        if cert_file := os.getenv("PROXIMADB_CERT_FILE"):
            tls_config["cert_file"] = cert_file
        if key_file := os.getenv("PROXIMADB_KEY_FILE"):
            tls_config["key_file"] = key_file
        if tls_config:
            config_dict["tls"] = TLSConfig(**tls_config)
        
        # Logging
        if log_level := os.getenv("PROXIMADB_LOG_LEVEL"):
            config_dict["log_level"] = log_level.upper()
        if debug := os.getenv("PROXIMADB_DEBUG"):
            config_dict["enable_debug_logging"] = debug.lower() in ("true", "1", "yes")
        
        # Performance
        if batch_size := os.getenv("PROXIMADB_BATCH_SIZE"):
            config_dict["default_batch_size"] = int(batch_size)
        if max_concurrent := os.getenv("PROXIMADB_MAX_CONCURRENT"):
            config_dict["max_concurrent_requests"] = int(max_concurrent)
        
        # Apply overrides
        config_dict.update(overrides)
        
        # URL is required
        if "url" not in config_dict:
            raise ValueError("URL must be provided via PROXIMADB_URL environment variable or constructor")
        
        return cls(**config_dict)
    
    def get_base_headers(self) -> Dict[str, str]:
        """Get base headers for requests"""
        headers = {
            "User-Agent": self.user_agent or f"proximadb-python/{self._get_version()}",
            "Accept": "application/json",
            "Content-Type": "application/json",
        }
        
        if self.api_key:
            headers["Authorization"] = f"Bearer {self.api_key}"
        
        # Add custom headers
        headers.update(self.custom_headers)
        
        return headers
    
    def get_grpc_metadata(self) -> list:
        """Get gRPC metadata for requests"""
        metadata = []
        
        if self.api_key:
            metadata.append(("authorization", f"Bearer {self.api_key}"))
        
        metadata.append(("user-agent", self.user_agent or f"proximadb-python/{self._get_version()}"))
        
        # Add custom headers as metadata
        for key, value in self.custom_headers.items():
            metadata.append((key.lower(), value))
        
        return metadata
    
    def _get_version(self) -> str:
        """Get client version"""
        try:
            from . import __version__
            return __version__
        except ImportError:
            return "unknown"
    
    def is_secure(self) -> bool:
        """Check if connection should use TLS"""
        parsed = urlparse(self.url)
        return parsed.scheme == "https"
    
    def get_host_port(self) -> tuple:
        """Get host and port from URL"""
        parsed = urlparse(self.url)
        host = parsed.hostname or "localhost"
        
        if parsed.port:
            port = parsed.port
        else:
            port = 443 if self.is_secure() else 80
        
        return host, port
    
    def should_use_grpc(self) -> bool:
        """Determine if gRPC should be used"""
        if self.protocol == Protocol.GRPC:
            return True
        elif self.protocol == Protocol.REST:
            return False
        else:  # AUTO
            # Use gRPC by default for better performance
            return True
    
    def get_protocol_url(self, target_protocol: Protocol) -> str:
        """Get URL for specific protocol with correct port"""
        parsed = urlparse(self.url)
        host = parsed.hostname or "localhost"
        scheme = parsed.scheme or "http"
        
        # ProximaDB standard port allocation
        if target_protocol == Protocol.REST:
            port = 5679 if scheme == "https" else 5678
        elif target_protocol == Protocol.GRPC:
            port = 5681 if scheme == "https" else 5680  
        elif target_protocol == Protocol.AVRO:
            port = 5683 if scheme == "https" else 5682
        else:
            # Keep original port for AUTO or unknown protocols
            port = parsed.port or (443 if scheme == "https" else 80)
        
        # For gRPC, don't include scheme in the endpoint
        if target_protocol == Protocol.GRPC:
            return f"{host}:{port}"
        else:
            return f"{scheme}://{host}:{port}"


# Default configuration instance
DEFAULT_CONFIG = ClientConfig(
    url="http://localhost:5678",
    protocol=Protocol.AUTO,
    timeout=30.0,
)


def load_config(
    url: Optional[str] = None,
    api_key: Optional[str] = None,
    config_file: Optional[str] = None,
    **kwargs
) -> ClientConfig:
    """Load configuration from multiple sources with precedence:
    1. Explicit parameters
    2. Configuration file
    3. Environment variables
    4. Defaults
    """
    config_dict = {}
    
    # Load from config file if provided
    if config_file:
        config_dict.update(load_config_file(config_file))
    
    # Override with explicit parameters
    if url:
        config_dict["url"] = url
    if api_key:
        config_dict["api_key"] = api_key
    
    config_dict.update(kwargs)
    
    # Create from environment with overrides
    return ClientConfig.from_env(**config_dict)


def load_config_file(file_path: str) -> dict:
    """Load configuration from file (JSON, YAML, or TOML)"""
    import json
    from pathlib import Path
    
    path = Path(file_path)
    if not path.exists():
        raise FileNotFoundError(f"Configuration file not found: {file_path}")
    
    content = path.read_text()
    
    if file_path.endswith(('.yml', '.yaml')):
        try:
            import yaml
            return yaml.safe_load(content)
        except ImportError:
            raise ImportError("PyYAML is required to load YAML configuration files")
    
    elif file_path.endswith('.toml'):
        try:
            import tomli
            return tomli.loads(content)
        except ImportError:
            raise ImportError("tomli is required to load TOML configuration files")
    
    else:  # JSON
        return json.loads(content)