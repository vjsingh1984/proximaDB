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


@pytest.fixture(scope="session")
def server_url():
    """ProximaDB server URL for testing."""
    return "http://localhost:5678"


@pytest.fixture(scope="session")
def client(server_url):
    """ProximaDB client instance."""
    return ProximaDBClient(server_url)


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
    sys.path.insert(0, str(Path(__file__).parent.parent.parent))
    from bert_embedding_service import BERTEmbeddingService
    return BERTEmbeddingService("all-MiniLM-L6-v2")


@pytest.fixture(scope="session")
def corpus_data():
    """Load 10MB corpus data for testing."""
    corpus_path = Path(__file__).parent.parent.parent / "embedding_cache/corpus_10.0mb.json"
    if corpus_path.exists():
        import json
        with open(corpus_path, 'r') as f:
            return json.load(f)
    return []


@pytest.fixture(scope="session")
def cached_embeddings():
    """Load cached embeddings for testing."""
    embeddings_path = Path(__file__).parent.parent.parent / "embedding_cache/embeddings_10.0mb.npy"
    if embeddings_path.exists():
        import numpy as np
        return np.load(embeddings_path)
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