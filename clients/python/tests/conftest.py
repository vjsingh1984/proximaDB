#!/usr/bin/env python3
"""
ProximaDB Python SDK Test Configuration
Shared fixtures and configuration for all test modules
"""

import pytest
import logging
import time
import sys
import os
from typing import Generator, Dict, Any

# Add the SDK source to the path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'src'))

from proximadb import ProximaDBClient, connect_rest, connect_grpc
from proximadb.models import CollectionConfig, DistanceMetric
from proximadb.exceptions import ProximaDBError


# Configure logging for tests
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s [%(levelname)s] %(name)s: %(message)s',
    datefmt='%Y-%m-%d %H:%M:%S'
)

# Test configuration
TEST_CONFIG = {
    "rest_endpoint": "http://localhost:5678",
    "grpc_endpoint": "http://localhost:5679",
    "default_timeout": 30.0,
    "max_retry_attempts": 3,
    "test_collection_prefix": "pytest_",
    "cleanup_on_failure": True
}


@pytest.fixture(scope="session")
def test_config() -> Dict[str, Any]:
    """Test configuration fixture"""
    return TEST_CONFIG.copy()


@pytest.fixture(scope="session")
def verify_server_running(test_config):
    """Verify ProximaDB server is running before tests"""
    try:
        rest_client = connect_rest(test_config["rest_endpoint"])
        
        # Try to connect and get health status
        try:
            health = rest_client.health()
            logging.info(f"✅ ProximaDB REST server is healthy: {health}")
        except Exception as e:
            logging.warning(f"⚠️ Health check failed, but server appears to be running: {e}")
        
        # Try basic operation
        collections = rest_client.list_collections()
        logging.info(f"✅ ProximaDB server responding - found {len(collections)} collections")
        
        return True
        
    except Exception as e:
        pytest.fail(f"❌ ProximaDB server not accessible at {test_config['rest_endpoint']}: {e}")


@pytest.fixture(scope="class")
def rest_client(verify_server_running, test_config) -> Generator[ProximaDBClient, None, None]:
    """Shared REST client fixture for test classes"""
    client = connect_rest(test_config["rest_endpoint"])
    yield client
    
    if hasattr(client, 'close'):
        client.close()


@pytest.fixture(scope="class")
def grpc_client(verify_server_running, test_config) -> Generator[ProximaDBClient, None, None]:
    """Shared gRPC client fixture for test classes"""
    client = connect_grpc(test_config["grpc_endpoint"])
    yield client
    
    if hasattr(client, 'close'):
        client.close()


@pytest.fixture
def unique_collection_name(test_config) -> str:
    """Generate unique collection name for each test"""
    timestamp = int(time.time() * 1000)  # Millisecond precision
    test_name = os.environ.get('PYTEST_CURRENT_TEST', 'unknown').split('::')[-1].split('[')[0]
    return f"{test_config['test_collection_prefix']}{test_name}_{timestamp}"


@pytest.fixture
def basic_collection_config() -> CollectionConfig:
    """Basic collection configuration for tests"""
    return CollectionConfig(
        dimension=128,
        distance_metric=DistanceMetric.COSINE,
        description="Test collection created by pytest"
    )


@pytest.fixture
def advanced_collection_config() -> CollectionConfig:
    """Advanced collection configuration for tests"""
    return CollectionConfig(
        dimension=384,
        distance_metric=DistanceMetric.COSINE,
        description="Advanced test collection with BERT dimensions",
        storage_layout="viper"
    )


@pytest.fixture
def test_collection(rest_client, unique_collection_name, basic_collection_config):
    """Create and manage a test collection with automatic cleanup"""
    collection = rest_client.create_collection(unique_collection_name, basic_collection_config)
    yield collection
    
    # Cleanup
    try:
        rest_client.delete_collection(unique_collection_name)
        logging.debug(f"Cleaned up test collection: {unique_collection_name}")
    except Exception as e:
        logging.warning(f"Failed to cleanup collection {unique_collection_name}: {e}")


class TestCollectionManager:
    """Helper class for managing test collections"""
    
    def __init__(self, client: ProximaDBClient, config: Dict[str, Any]):
        self.client = client
        self.config = config
        self.created_collections = []
    
    def create_test_collection(self, name_suffix: str = "", config: CollectionConfig = None) -> str:
        """Create a test collection with automatic tracking"""
        if config is None:
            config = CollectionConfig(dimension=128, distance_metric=DistanceMetric.COSINE)
        
        timestamp = int(time.time() * 1000)
        collection_name = f"{self.config['test_collection_prefix']}{name_suffix}_{timestamp}"
        
        collection = self.client.create_collection(collection_name, config)
        self.created_collections.append(collection_name)
        
        return collection_name
    
    def cleanup_all(self):
        """Clean up all created collections"""
        for collection_name in self.created_collections:
            try:
                self.client.delete_collection(collection_name)
                logging.debug(f"Cleaned up collection: {collection_name}")
            except Exception as e:
                logging.warning(f"Failed to cleanup {collection_name}: {e}")
        
        self.created_collections.clear()


@pytest.fixture
def collection_manager(rest_client, test_config):
    """Collection manager fixture with automatic cleanup"""
    manager = TestCollectionManager(rest_client, test_config)
    yield manager
    
    # Cleanup all collections created by this manager
    manager.cleanup_all()


# Test markers
def pytest_configure(config):
    """Configure custom pytest markers"""
    config.addinivalue_line(
        "markers", "slow: marks tests as slow (may take > 10 seconds)"
    )
    config.addinivalue_line(
        "markers", "integration: marks tests as integration tests requiring running server"
    )
    config.addinivalue_line(
        "markers", "storage: marks tests related to storage layer functionality"
    )
    config.addinivalue_line(
        "markers", "search: marks tests related to search and similarity operations"
    )
    config.addinivalue_line(
        "markers", "large_data: marks tests that work with large datasets"
    )


# Pytest hooks
def pytest_collection_modifyitems(config, items):
    """Modify test collection to add markers and skip conditions"""
    for item in items:
        # Add integration marker to all tests by default
        if not any(marker.name == "unit" for marker in item.iter_markers()):
            item.add_marker(pytest.mark.integration)
        
        # Mark slow tests
        if "large" in item.name or "stress" in item.name or "compaction" in item.name:
            item.add_marker(pytest.mark.slow)
        
        # Mark storage tests
        if "storage" in item.name or "wal" in item.name or "flush" in item.name:
            item.add_marker(pytest.mark.storage)
        
        # Mark search tests
        if "search" in item.name or "similarity" in item.name or "proximity" in item.name:
            item.add_marker(pytest.mark.search)


def pytest_runtest_setup(item):
    """Setup for each test item"""
    # Skip slow tests if running in fast mode
    if item.config.getoption("--fast", default=False):
        if any(marker.name == "slow" for marker in item.iter_markers()):
            pytest.skip("Skipping slow test in fast mode")


def pytest_addoption(parser):
    """Add custom command line options"""
    parser.addoption(
        "--fast",
        action="store_true",
        default=False,
        help="Run only fast tests, skip slow/large data tests"
    )
    parser.addoption(
        "--rest-endpoint",
        action="store",
        default="http://localhost:5678",
        help="ProximaDB REST endpoint for testing"
    )
    parser.addoption(
        "--grpc-endpoint", 
        action="store",
        default="http://localhost:5679",
        help="ProximaDB gRPC endpoint for testing"
    )


@pytest.fixture(scope="session", autouse=True)
def configure_test_endpoints(request):
    """Configure test endpoints from command line options"""
    rest_endpoint = request.config.getoption("--rest-endpoint")
    grpc_endpoint = request.config.getoption("--grpc-endpoint")
    
    TEST_CONFIG["rest_endpoint"] = rest_endpoint
    TEST_CONFIG["grpc_endpoint"] = grpc_endpoint
    
    logging.info(f"Test configuration: REST={rest_endpoint}, gRPC={grpc_endpoint}")


# Exception handling helpers
class ProximaDBTestError(Exception):
    """Custom exception for test-specific errors"""
    pass


def assert_proximadb_error(exc_info, expected_message_fragment: str = None):
    """Helper to assert ProximaDB errors with optional message checking"""
    assert issubclass(exc_info.type, ProximaDBError), f"Expected ProximaDBError, got {exc_info.type}"
    
    if expected_message_fragment:
        assert expected_message_fragment.lower() in str(exc_info.value).lower(), \
            f"Expected '{expected_message_fragment}' in error message: {exc_info.value}"


# Performance measurement helpers
@pytest.fixture
def performance_monitor():
    """Fixture for monitoring test performance"""
    class PerformanceMonitor:
        def __init__(self):
            self.timings = {}
            self.start_times = {}
        
        def start_timer(self, operation: str):
            self.start_times[operation] = time.time()
        
        def end_timer(self, operation: str) -> float:
            if operation in self.start_times:
                duration = time.time() - self.start_times[operation]
                self.timings[operation] = duration
                del self.start_times[operation]
                return duration
            return 0.0
        
        def get_timings(self) -> Dict[str, float]:
            return self.timings.copy()
        
        def assert_performance(self, operation: str, max_seconds: float):
            assert operation in self.timings, f"No timing recorded for {operation}"
            actual = self.timings[operation]
            assert actual <= max_seconds, f"{operation} took {actual:.3f}s, expected <= {max_seconds}s"
    
    return PerformanceMonitor()


# Test data generators
def generate_test_vectors(count: int, dimension: int) -> list:
    """Generate test vectors for use in tests"""
    import numpy as np
    return [np.random.random(dimension).astype(np.float32).tolist() for _ in range(count)]


def generate_test_metadata(count: int, categories: list = None) -> list:
    """Generate test metadata for use in tests"""
    if categories is None:
        categories = ["technology", "science", "healthcare", "education", "business"]
    
    metadata_list = []
    for i in range(count):
        metadata = {
            "index": i,
            "category": categories[i % len(categories)],
            "importance": (i % 10) + 1,
            "test_timestamp": time.time(),
            "test_generated": True
        }
        metadata_list.append(metadata)
    
    return metadata_list