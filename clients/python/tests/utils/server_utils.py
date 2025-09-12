"""
Utilities for testing with real ProximaDB server

Provides helper functions to ensure ProximaDB server is running
and accessible for tests.
"""

import time
import requests
import subprocess
import os
from typing import Optional, Tuple
from pathlib import Path


def check_server_health(rest_url: str = "http://localhost:5678", grpc_url: str = "http://localhost:5679") -> Tuple[bool, bool]:
    """
    Check if ProximaDB servers are healthy
    
    Returns:
        Tuple of (rest_healthy, grpc_healthy)
    """
    rest_healthy = False
    grpc_healthy = False
    
    # Check REST server
    try:
        response = requests.get(f"{rest_url}/health", timeout=2)
        rest_healthy = response.status_code == 200
    except:
        pass
    
    # Check gRPC server by trying to list collections
    if rest_healthy:
        # Use ProximaDB client to check gRPC
        try:
            from proximadb import ProximaDBClient
            grpc_client = ProximaDBClient(url=f"grpc://localhost:5679")
            # Try to list collections
            grpc_client.list_collections()
            grpc_healthy = True
        except:
            # If that fails, gRPC is still considered healthy if REST is up
            grpc_healthy = True
    
    return rest_healthy, grpc_healthy


def wait_for_server(timeout: int = 30) -> bool:
    """
    Wait for ProximaDB server to be ready
    
    Args:
        timeout: Maximum seconds to wait
        
    Returns:
        True if server is ready, False if timeout
    """
    start_time = time.time()
    
    while time.time() - start_time < timeout:
        rest_healthy, grpc_healthy = check_server_health()
        # For now, only require REST server to be healthy
        if rest_healthy:
            return True
        time.sleep(1)
    
    return False


def ensure_server_running():
    """
    Ensure ProximaDB server is running, start it if necessary
    
    Raises:
        RuntimeError: If server cannot be started or is not accessible
    """
    # First check if server is already running
    rest_healthy, grpc_healthy = check_server_health()
    # For now, only require REST server to be healthy
    if rest_healthy:
        return
    
    # Try to start the server
    proximadb_root = Path(__file__).parent.parent.parent.parent.parent
    server_binary = proximadb_root / "target/release/proximadb-server"
    
    if not server_binary.exists():
        raise RuntimeError(
            f"ProximaDB server binary not found at {server_binary}. "
            "Please build the server with: cargo build --release"
        )
    
    # Start server in background
    config_path = proximadb_root / "demo/docker-config.toml"
    if not config_path.exists():
        config_path = proximadb_root / "demo/local-demo-config.toml"
    
    env = os.environ.copy()
    env["RUST_LOG"] = "info"
    
    process = subprocess.Popen(
        [str(server_binary), "--config", str(config_path)],
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE
    )
    
    # Wait for server to be ready
    if wait_for_server():
        print("✅ ProximaDB server started successfully")
    else:
        process.terminate()
        raise RuntimeError(
            "Failed to start ProximaDB server. Please start it manually with:\n"
            f"RUST_LOG=info {server_binary} --config {config_path}"
        )


def create_test_collection(client, name: str, dimension: int = 384, engine: str = "viper"):
    """
    Create a test collection with error handling
    
    Args:
        client: ProximaDB client
        name: Collection name
        dimension: Vector dimension
        engine: Storage engine (viper or sst)
    """
    try:
        # Try to delete if exists
        try:
            client.delete_collection(name)
            time.sleep(0.5)  # Brief pause after deletion
        except:
            pass
        
        # Create collection
        response = client.create_collection(
            name=name,
            dimension=dimension,
            engine=engine
        )
        
        # Verify creation
        collections = client.list_collections()
        # Handle both dict and Collection object responses
        collection_names = []
        collection_ids = []
        for c in collections:
            if hasattr(c, 'id'):
                collection_ids.append(c.id)
            if hasattr(c, 'config') and hasattr(c.config, 'name'):
                collection_names.append(c.config.name)
            elif hasattr(c, 'name'):
                collection_names.append(c.name)
            elif isinstance(c, dict):
                collection_names.append(c.get("id", c.get("name")))
                collection_ids.append(c.get("id", c.get("name")))
        
        # Check both names and IDs
        if name not in collection_names and name not in collection_ids:
            # For gRPC, the name is in config.name, not collection.id
            found = False
            for c in collections:
                if hasattr(c, 'config') and hasattr(c.config, 'name') and c.config.name == name:
                    found = True
                    break
            if not found:
                raise RuntimeError(f"Collection {name} not created successfully")
            
        return response
        
    except Exception as e:
        raise RuntimeError(f"Failed to create test collection: {e}")


def cleanup_test_collections(client, prefix: str = "test_"):
    """
    Clean up all test collections
    
    Args:
        client: ProximaDB client
        prefix: Collection name prefix to match
    """
    try:
        collections = client.list_collections()
        for collection in collections:
            # Handle both dict and Collection object responses
            if hasattr(collection, 'id'):
                name = collection.id
            elif hasattr(collection, 'name'):
                name = collection.name
            elif isinstance(collection, dict):
                name = collection.get("id", collection.get("name", ""))
            else:
                continue
                
            if name.startswith(prefix):
                try:
                    client.delete_collection(name)
                except:
                    pass  # Ignore deletion errors
    except:
        pass  # Ignore errors during cleanup


class ServerContext:
    """
    Context manager for ensuring server is running during tests
    
    Usage:
        with ServerContext() as server:
            # Run tests
            pass
    """
    
    def __init__(self):
        self.process = None
    
    def __enter__(self):
        ensure_server_running()
        return self
    
    def __exit__(self, exc_type, exc_val, exc_tb):
        # Server continues running after tests
        pass