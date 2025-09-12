"""
Pytest configuration and fixtures for ProximaDB tests.
"""

import pytest
import sys
import os
import time
import tempfile
import shutil
from pathlib import Path

# Add Python SDK to path
sys.path.insert(0, str(Path(__file__).parent.parent.parent / "clients/python/src"))

from proximadb import ProximaDBClient
from proximadb.config import ClientConfig, CompressionConfig


@pytest.fixture(scope="session")
def rest_server_url():
    """ProximaDB REST server URL for testing."""
    return "http://localhost:5678"


@pytest.fixture(scope="session")
def grpc_server_url():
    """ProximaDB gRPC server URL for testing."""
    return "grpc://localhost:5679"


@pytest.fixture(scope="session")
def client(rest_server_url):
    """ProximaDB REST client instance with compression disabled."""
    config = ClientConfig(
        url=rest_server_url,
        compression=CompressionConfig(enabled=False)
    )
    return ProximaDBClient(config=config)


@pytest.fixture(scope="session")
def grpc_client(grpc_server_url):
    """ProximaDB gRPC client instance with compression disabled."""
    config = ClientConfig(
        url=grpc_server_url,
        compression=CompressionConfig(enabled=False)
    )
    return ProximaDBClient(config=config)


@pytest.fixture
def test_collection_name():
    """Generate a unique collection name for testing."""
    import uuid
    return f"test_collection_{uuid.uuid4().hex[:8]}"


@pytest.fixture
def cleanup_collection(client, test_collection_name):
    """Cleanup collection after test."""
    yield test_collection_name
    try:
        client.delete_collection(test_collection_name)
    except:
        pass  # Collection might not exist


@pytest.fixture(scope="session")
def bert_service():
    """BERT embedding service for tests."""
    integration_path = Path(__file__).parent / "integration"
    sys.path.insert(0, str(integration_path))
    try:
        from bert_embedding_service import BERTEmbeddingService
        return BERTEmbeddingService("all-MiniLM-L6-v2")
    except ImportError:
        pytest.skip("BERT embedding service not available")


@pytest.fixture(scope="session")
def corpus_data():
    """Load 10MB corpus data for testing."""
    # Try loading actual data files from demo/pre directory
    demo_pre_path = Path(__file__).parent.parent.parent.parent.parent / "demo/pre"
    
    # Try ecommerce_data.json first (21MB)
    ecommerce_path = demo_pre_path / "ecommerce_data.json"
    if ecommerce_path.exists():
        import json
        with open(ecommerce_path, 'r') as f:
            data = json.load(f)
            # Convert to expected format if needed
            if isinstance(data, list) and data and isinstance(data[0], dict):
                # If it's already in the right format, return as is
                if 'text' in data[0]:
                    return data
                # Otherwise convert from ecommerce format
                corpus = []
                for item in data[:1000]:  # Limit to 1000 items for testing
                    doc = {
                        "text": f"{item.get('name', '')} {item.get('description', '')}",
                        "category": item.get('category', 'unknown'),
                        "title": item.get('name', f"Product {item.get('id', '')}"),
                        "doc_type": "product",
                        "year": 2024,
                        "author": item.get('brand', 'Unknown'),
                        "length": len(item.get('description', '')),
                        "importance": int(item.get('rating', 5))
                    }
                    corpus.append(doc)
                return corpus
            return data
    
    # Fallback to knowledge_base_data.json (4.6MB)
    kb_path = demo_pre_path / "knowledge_base_data.json"
    if kb_path.exists():
        import json
        with open(kb_path, 'r') as f:
            data = json.load(f)
            if isinstance(data, list) and data:
                return data[:1000]  # Limit for testing
    
    return []


@pytest.fixture(scope="session")
def cached_embeddings(corpus_data, bert_service):
    """Load cached embeddings for testing."""
    # First try to load cached embeddings
    embeddings_path = Path(__file__).parent.parent.parent / "embedding_cache/embeddings_10.0mb.npy"
    if embeddings_path.exists():
        import numpy as np
        return np.load(embeddings_path).tolist()
    
    # If no cached embeddings and we have corpus data, generate them
    if corpus_data and bert_service:
        texts = []
        for doc in corpus_data[:100]:  # Limit to 100 for testing
            if isinstance(doc, dict) and 'text' in doc:
                texts.append(doc['text'])
            elif isinstance(doc, str):
                texts.append(doc)
        
        if texts:
            # Generate embeddings in batches
            embeddings = []
            batch_size = 32
            for i in range(0, len(texts), batch_size):
                batch = texts[i:i+batch_size]
                batch_embeddings = bert_service.encode(batch)
                embeddings.extend(batch_embeddings.tolist())
            return embeddings
    
    return None


@pytest.fixture
def temp_dir():
    """Create temporary directory for tests."""
    temp_dir = tempfile.mkdtemp()
    yield temp_dir
    shutil.rmtree(temp_dir, ignore_errors=True)


def pytest_configure(config):
    """Configure pytest with custom markers."""
    config.addinivalue_line(
        "markers", "integration: marks tests as integration tests (requires running server)"
    )
    config.addinivalue_line(
        "markers", "slow: marks tests as slow running"
    )
    config.addinivalue_line(
        "markers", "embedding: marks tests that require BERT embeddings"
    )