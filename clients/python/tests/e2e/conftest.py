"""
Pytest configuration for end-to-end tests
Provides shared fixtures and test configuration for comprehensive SDK testing.
"""

import pytest
import logging
import time
import sys
import os
from sentence_transformers import SentenceTransformer

# Add Python SDK to path
sys.path.insert(0, '/workspace/clients/python/src')

from proximadb import ProximaDBClient, Protocol
from proximadb.models import CollectionConfig

# Configure logging for tests
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s [%(levelname)s] %(name)s: %(message)s',
    datefmt='%Y-%m-%dT%H:%M:%S'
)

logger = logging.getLogger('e2e_conftest')


def pytest_configure(config):
    """Configure pytest with custom markers"""
    config.addinivalue_line(
        "markers", "integration: mark test as integration test requiring running server"
    )
    config.addinivalue_line(
        "markers", "performance: mark test as performance test that may take longer"
    )
    config.addinivalue_line(
        "markers", "large_scale: mark test as large-scale test with significant data"
    )
    config.addinivalue_line(
        "markers", "grpc: mark test as gRPC-specific test"
    )
    config.addinivalue_line(
        "markers", "rest: mark test as REST-specific test"
    )


def pytest_addoption(parser):
    """Add custom command line options"""
    parser.addoption(
        "--grpc-port",
        action="store",
        default="5679",
        help="gRPC server port (default: 5679)"
    )
    parser.addoption(
        "--rest-port", 
        action="store",
        default="5678",
        help="REST server port (default: 5678)"
    )
    parser.addoption(
        "--server-host",
        action="store",
        default="localhost",
        help="Server host (default: localhost)"
    )
    parser.addoption(
        "--skip-server-check",
        action="store_true",
        help="Skip server availability check"
    )


@pytest.fixture(scope="session")
def server_config(request):
    """Server configuration for tests"""
    return {
        "host": request.config.getoption("--server-host"),
        "grpc_port": request.config.getoption("--grpc-port"),
        "rest_port": request.config.getoption("--rest-port"),
        "skip_check": request.config.getoption("--skip-server-check")
    }


@pytest.fixture(scope="session")
def bert_model_session():
    """Session-scoped BERT model to avoid reloading"""
    logger.info("🤖 Loading BERT model for test session...")
    model = SentenceTransformer('all-MiniLM-L6-v2')
    logger.info(f"✅ BERT model loaded: {model.get_sentence_embedding_dimension()}D")
    return model


@pytest.fixture
def server_health_check(server_config):
    """Check server health before tests"""
    if server_config["skip_check"]:
        return True
    
    host = server_config["host"]
    grpc_port = server_config["grpc_port"]
    rest_port = server_config["rest_port"]
    
    # Test both protocols
    protocols_to_test = [
        (Protocol.GRPC, f"{host}:{grpc_port}"),
        (Protocol.REST, f"{host}:{rest_port}")
    ]
    
    available_protocols = []
    
    for protocol, url in protocols_to_test:
        try:
            client = ProximaDBClient(url, protocol=protocol)
            
            # Try to connect with retries
            max_retries = 10
            for i in range(max_retries):
                try:
                    health = client.health()
                    if health:
                        available_protocols.append(protocol)
                        logger.info(f"✅ {protocol.value} server available at {url}")
                        break
                except Exception as e:
                    if i == max_retries - 1:
                        logger.warning(f"⚠️ {protocol.value} server not available at {url}: {e}")
                    time.sleep(0.5)
            
            client.close()
            
        except Exception as e:
            logger.warning(f"⚠️ Failed to check {protocol.value} server: {e}")
    
    if not available_protocols:
        pytest.skip("No ProximaDB servers available for testing")
    
    return available_protocols


@pytest.fixture
def sample_collection_config():
    """Standard collection configuration for tests"""
    return CollectionConfig(
        dimension=384,
        distance_metric="cosine",
        index_config={
            "index_type": "hnsw",
            "ef_construction": 200,
            "max_connections": 16
        }
    )


@pytest.fixture
def sample_texts():
    """Sample text data for testing"""
    return [
        "Artificial intelligence is transforming modern technology",
        "Machine learning algorithms process vast amounts of data",
        "Neural networks enable sophisticated pattern recognition", 
        "Vector databases store high-dimensional embeddings efficiently",
        "Similarity search finds nearest neighbors in vector space",
        "Natural language processing understands human communication",
        "Deep learning models achieve state-of-the-art performance",
        "Computer vision interprets and analyzes visual information",
        "Reinforcement learning optimizes decision-making processes",
        "Transformer architectures revolutionize sequence modeling"
    ]


@pytest.fixture
def sample_embeddings(bert_model_session, sample_texts):
    """Pre-computed embeddings for sample texts"""
    return [bert_model_session.encode(text).tolist() for text in sample_texts]


@pytest.fixture
def large_text_corpus():
    """Large text corpus for performance testing"""
    themes = [
        "artificial intelligence and machine learning",
        "cloud computing and distributed systems",
        "vector databases and similarity search", 
        "natural language processing and transformers",
        "computer vision and image recognition",
        "blockchain technology and cryptocurrencies",
        "quantum computing and quantum algorithms",
        "cybersecurity and privacy protection",
        "software engineering and development",
        "data science and analytics"
    ]
    
    variations = [
        "is revolutionizing the tech industry",
        "has significant applications in business",
        "requires advanced mathematical concepts",
        "is becoming increasingly important",
        "faces challenges in implementation",
        "shows promising future developments",
        "needs careful ethical considerations",
        "demands skilled professionals",
        "transforms traditional workflows",
        "enables innovative solutions"
    ]
    
    corpus = []
    for i in range(1000):  # Generate 1000 documents
        theme = themes[i % len(themes)]
        variation = variations[i % len(variations)]
        text = f"Document {i+1}: {theme.title()} {variation} in modern applications."
        corpus.append(text)
    
    return corpus


class ClientManager:
    """Helper class to manage multiple clients for testing"""
    
    def __init__(self, server_config):
        self.server_config = server_config
        self.clients = {}
    
    def get_client(self, protocol: Protocol):
        """Get or create client for specified protocol"""
        if protocol in self.clients:
            return self.clients[protocol]
        
        host = self.server_config["host"]
        if protocol == Protocol.GRPC:
            url = f"{host}:{self.server_config['grpc_port']}"
        else:
            url = f"{host}:{self.server_config['rest_port']}"
        
        client = ProximaDBClient(url, protocol=protocol)
        
        # Verify connection
        max_retries = 30
        for i in range(max_retries):
            try:
                health = client.health()
                if health:
                    self.clients[protocol] = client
                    return client
            except Exception as e:
                if i == max_retries - 1:
                    client.close()
                    raise Exception(f"Failed to connect to {protocol.value} server: {e}")
                time.sleep(1)
        
        client.close()
        raise Exception(f"Timeout connecting to {protocol.value} server")
    
    def close_all(self):
        """Close all managed clients"""
        for client in self.clients.values():
            try:
                client.close()
            except Exception as e:
                logger.warning(f"⚠️ Error closing client: {e}")
        self.clients.clear()


@pytest.fixture
def client_manager(server_config, server_health_check):
    """Client manager for tests requiring multiple clients"""
    manager = ClientManager(server_config)
    yield manager
    manager.close_all()


def pytest_collection_modifyitems(config, items):
    """Modify test collection to add markers based on test names"""
    for item in items:
        # Add integration marker to all e2e tests
        if "e2e" in str(item.fspath):
            item.add_marker(pytest.mark.integration)
        
        # Add specific markers based on test names
        if "grpc" in item.name.lower():
            item.add_marker(pytest.mark.grpc)
        
        if "rest" in item.name.lower():
            item.add_marker(pytest.mark.rest)
        
        if "performance" in item.name.lower() or "large" in item.name.lower():
            item.add_marker(pytest.mark.performance)
        
        if "10mb" in item.name.lower() or "large_scale" in item.name.lower():
            item.add_marker(pytest.mark.large_scale)


def pytest_report_header(config):
    """Add custom header to pytest report"""
    return [
        "ProximaDB Python SDK End-to-End Test Suite",
        f"Testing gRPC server: {config.getoption('--server-host')}:{config.getoption('--grpc-port')}",
        f"Testing REST server: {config.getoption('--server-host')}:{config.getoption('--rest-port')}"
    ]


def pytest_sessionstart(session):
    """Session start hook"""
    logger.info("🚀 Starting ProximaDB SDK End-to-End Test Suite")


def pytest_sessionfinish(session, exitstatus):
    """Session finish hook"""
    if exitstatus == 0:
        logger.info("✅ All tests completed successfully")
    else:
        logger.info(f"❌ Tests completed with exit status: {exitstatus}")


# Custom markers for pytest
pytest_plugins = []