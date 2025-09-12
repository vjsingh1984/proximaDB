#!/usr/bin/env python3
"""
Path utilities for ProximaDB demos

Provides consistent path resolution that works across:
- Local development environments
- Docker containers
- Different operating systems
"""

import os
import sys
from pathlib import Path
from typing import Optional

def get_project_root() -> Path:
    """
    Get the ProximaDB project root directory
    
    Returns:
        Path to project root (contains Cargo.toml)
    """
    # Start from current file location
    current = Path(__file__).parent.parent.parent
    
    # Look for project markers
    markers = ['Cargo.toml', 'pyproject.toml', '.git']
    
    while current != current.parent:
        for marker in markers:
            if (current / marker).exists():
                return current
        current = current.parent
    
    # Fallback to relative path
    return Path(__file__).parent.parent.parent

def get_python_sdk_path() -> Path:
    """
    Get the ProximaDB Python SDK path
    
    Works in both:
    - Development: PROJECT_ROOT/clients/python/src
    - Container: /app/proximadb-sdk/src
    """
    # Check container path first
    container_path = Path("/app/proximadb-sdk/src")
    if container_path.exists():
        return container_path
    
    # Development path
    project_root = get_project_root()
    dev_path = project_root / "clients" / "python" / "src"
    
    if dev_path.exists():
        return dev_path
    
    # Try relative to demo directory
    demo_relative = Path(__file__).parent.parent.parent / "clients" / "python" / "src"
    if demo_relative.exists():
        return demo_relative
    
    raise RuntimeError("Could not locate ProximaDB Python SDK")

def setup_python_path():
    """Add ProximaDB SDK to Python path if not already present"""
    sdk_path = str(get_python_sdk_path())
    if sdk_path not in sys.path:
        sys.path.insert(0, sdk_path)

def get_embedding_cache_dir() -> Path:
    """
    Get the embedding model cache directory
    
    Priority order:
    1. Environment variable EMBEDDING_CACHE_DIR
    2. Container path /app/embedding_cache
    3. User cache directory ~/.cache/proximadb/embeddings
    4. Local directory ./embedding_cache
    """
    # Environment variable
    if env_cache := os.getenv('EMBEDDING_CACHE_DIR'):
        return Path(env_cache)
    
    # Container path
    container_cache = Path("/app/embedding_cache")
    if container_cache.exists() and os.access(container_cache, os.W_OK):
        return container_cache
    
    # User cache directory
    user_cache = Path.home() / ".cache" / "proximadb" / "embeddings"
    try:
        user_cache.mkdir(parents=True, exist_ok=True)
        if os.access(user_cache, os.W_OK):
            return user_cache
    except (OSError, PermissionError):
        pass
    
    # Local directory fallback
    local_cache = Path("./embedding_cache")
    local_cache.mkdir(parents=True, exist_ok=True)
    return local_cache

def setup_cache_directories():
    """
    Setup all cache directories with proper permissions
    
    Sets up:
    - Embedding model cache
    - HuggingFace cache
    - Torch cache
    """
    cache_dir = get_embedding_cache_dir()
    
    # Create cache directory
    cache_dir.mkdir(parents=True, exist_ok=True)
    
    # Set environment variables
    cache_str = str(cache_dir)
    os.environ['TRANSFORMERS_CACHE'] = cache_str
    os.environ['HF_HOME'] = cache_str
    os.environ['TORCH_HOME'] = cache_str
    os.environ['EMBEDDING_CACHE_DIR'] = cache_str
    
    # Try to set permissions (may fail in some environments)
    try:
        os.chmod(cache_dir, 0o755)
    except (OSError, PermissionError):
        pass
    
    return cache_dir

def get_demo_results_dir() -> Path:
    """
    Get directory for demo results
    
    Returns:
        Path to demo results directory
    """
    # Check if we're in container
    if Path("/app/results").exists():
        results_dir = Path("/app/results")
    else:
        # Local development - use new location
        results_dir = Path("./demo/results")
    
    results_dir.mkdir(parents=True, exist_ok=True)
    return results_dir

def get_config_path(config_name: str) -> Path:
    """
    Get path to a configuration file
    
    Args:
        config_name: Name of config file (e.g., "docker-config.toml")
        
    Returns:
        Path to config file
    """
    # Check common locations
    locations = [
        Path(".") / config_name,
        Path(__file__).parent.parent / config_name,
        get_project_root() / "demo" / config_name,
        Path("/opt/proximadb/config") / config_name,
    ]
    
    for location in locations:
        if location.exists():
            return location
    
    # Return first option as fallback
    return locations[0]

# Convenience function for backward compatibility
def setup_demo_environment():
    """Setup complete demo environment (paths and caches)"""
    setup_python_path()
    cache_dir = setup_cache_directories()
    return {
        'sdk_path': get_python_sdk_path(),
        'cache_dir': cache_dir,
        'results_dir': get_demo_results_dir(),
        'project_root': get_project_root()
    }