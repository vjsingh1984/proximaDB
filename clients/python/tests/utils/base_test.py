"""
Base test class for ProximaDB Python SDK tests

Provides common functionality for tests that use real ProximaDB server.
"""

import pytest
import time
import uuid
from typing import Optional, List, Dict, Any
from pathlib import Path
import sys

# Add SDK to path
sys.path.insert(0, str(Path(__file__).parent.parent.parent / "src"))

from proximadb import ProximaDBClient
from proximadb.config import ClientConfig
from proximadb.models import VectorRecord
from proximadb.config import Protocol

from .server_utils import ensure_server_running, create_test_collection, cleanup_test_collections


class BaseProximaDBTest:
    """
    Base class for ProximaDB tests using real server
    
    Provides:
    - Automatic server health check
    - Test collection management
    - Common test utilities
    """
    
    @classmethod
    def setup_class(cls):
        """Ensure server is running before tests"""
        ensure_server_running()
        
        # Create clients
        cls.rest_client = ProximaDBClient(url="http://localhost:5678")
        cls.grpc_client = ProximaDBClient(url="grpc://localhost:5679")
        
        # Clean up any leftover test collections
        cleanup_test_collections(cls.rest_client)
        cleanup_test_collections(cls.grpc_client)
    
    @classmethod
    def teardown_class(cls):
        """Clean up after all tests"""
        cleanup_test_collections(cls.rest_client)
        cleanup_test_collections(cls.grpc_client)

        # CRITICAL: Close clients to release file descriptors
        try:
            cls.rest_client.close()
        except Exception:
            pass
        try:
            cls.grpc_client.close()
        except Exception:
            pass
    
    def setup_method(self):
        """Setup for each test method"""
        self.test_collection_name = f"test_{uuid.uuid4().hex[:8]}"
        self.created_collections = []
    
    def teardown_method(self):
        """Cleanup after each test method"""
        # Clean up created collections
        for collection_name in self.created_collections:
            try:
                self.rest_client.delete_collection(collection_name)
            except:
                pass
            try:
                self.grpc_client.delete_collection(collection_name)
            except:
                pass
    
    def create_collection(
        self, 
        client: Optional[ProximaDBClient] = None,
        name: Optional[str] = None,
        dimension: int = 384,
        engine: str = "viper"
    ) -> str:
        """
        Create a test collection
        
        Args:
            client: Client to use (defaults to rest_client)
            name: Collection name (auto-generated if None)
            dimension: Vector dimension
            engine: Storage engine
            
        Returns:
            Collection name
        """
        client = client or self.rest_client
        name = name or f"test_{uuid.uuid4().hex[:8]}"
        
        create_test_collection(client, name, dimension, engine)
        self.created_collections.append(name)
        
        return name
    
    def insert_test_vectors(
        self,
        client: ProximaDBClient,
        collection_name: str,
        count: int = 10,
        dimension: int = 384,
        metadata_template: Optional[Dict[str, Any]] = None
    ) -> List[VectorRecord]:
        """
        Insert test vectors into collection
        
        Args:
            client: Client to use
            collection_name: Collection to insert into
            count: Number of vectors to insert
            dimension: Vector dimension
            metadata_template: Base metadata for vectors
            
        Returns:
            List of inserted vector records
        """
        from ..embedding_utils import embed_seed
        
        vectors = []
        for i in range(count):
            # Deterministic realistic embedding based on index
            vector = embed_seed(i, dimension)
            
            metadata = metadata_template.copy() if metadata_template else {}
            metadata.update({
                "index": i,
                "test": True,
                "category": f"cat_{i % 3}",
                "value": float(i)
            })
            
            record = VectorRecord(
                id=f"vec_{i:04d}",
                vector=vector,
                metadata=metadata
            )
            vectors.append(record)
        
        # Insert in batch
        response = client.insert_vectors(collection_name, records=vectors)
        
        # Brief pause for indexing
        time.sleep(0.5)
        
        return vectors
    
    def verify_search_results(
        self,
        results: List[Dict[str, Any]],
        expected_count: int,
        check_scores: bool = True
    ):
        """
        Verify search results are valid
        
        Args:
            results: Search results
            expected_count: Expected number of results
            check_scores: Whether to verify scores are descending
        """
        assert len(results) == expected_count, f"Expected {expected_count} results, got {len(results)}"
        
        if check_scores and len(results) > 1:
            # Verify scores are in descending order
            scores = [r.get("score", 0) for r in results]
            assert scores == sorted(scores, reverse=True), "Scores not in descending order"
        
        # Verify each result has required fields
        for result in results:
            assert "id" in result
            assert "score" in result
            assert isinstance(result["score"], (int, float))
    
    def wait_for_indexing(self, duration: float = 1.0):
        """Wait for vectors to be indexed"""
        time.sleep(duration)
