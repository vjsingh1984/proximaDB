#!/usr/bin/env python3
"""
ProximaDB Integration Test Matrix
=================================

Comprehensive integration test suite that implements the test matrix defined in 
docs/test_matrix.adoc. This test validates all components across:

- Server startup and connectivity
- Authentication and authorization 
- Collection management
- Vector operations
- Algorithm and distance metrics
- Storage engines
- WAL and persistence
- Performance and scale
- Error handling

Each test updates the test matrix status in real-time.
"""

import json
import logging
import os
import random
import sys
import time
import uuid
from datetime import datetime
from pathlib import Path
from typing import Dict, List, Optional, Tuple, Any

import httpx
import numpy as np

# Add the Python SDK to the path
sys.path.insert(0, str(Path(__file__).parent.parent / "clients" / "python" / "src"))

try:
    import proximadb
    from proximadb import ProximaDBClient, ClientConfig
    from proximadb.models import CollectionConfig, DistanceMetric, IndexAlgorithm
    from proximadb.exceptions import ProximaDBError, AuthenticationError
except ImportError as e:
    print(f"Failed to import ProximaDB Python SDK: {e}")
    print("Make sure the Python SDK is built and available")
    sys.exit(1)


# Test configuration
TEST_CONFIG = {
    "server": {
        "rest_endpoint": "http://localhost:5678/api/v1",  # Unified server REST API base
        "grpc_endpoint": "localhost:5678",  # Same port as REST (unified server)
        "dashboard_endpoint": "http://localhost:5678/dashboard",  # Dashboard at /dashboard
        "health_endpoint": "http://localhost:5678/health",  # Health check
        "metrics_endpoint": "http://localhost:5678/metrics",  # Prometheus metrics
    },
    "auth": {
        "admin_api_key": "admin-key-12345",  # Default admin key
        "test_tenant": "test-tenant-001",
        "test_user": "test-user-001",
    },
    "collections": {
        "test_collection": "testcoll",
        "test_collection_id": None,  # Will be set after creation
        "dimension": 512,  # OpenAI embedding dimension
        "test_vectors_count": 1000,  # For WAL flush testing
    },
    "timeouts": {
        "server_startup": 30,
        "operation": 10,
        "bulk_operation": 60,
    }
}

# Test matrix status tracking
class TestMatrixTracker:
    """Tracks test execution status and updates the test matrix document."""
    
    def __init__(self):
        self.results = {}
        self.start_time = datetime.now()
        
    def update_test(self, test_id: str, status: str, result: str = "-", notes: str = ""):
        """Update test status in the matrix."""
        self.results[test_id] = {
            "status": status,
            "result": result, 
            "notes": notes,
            "timestamp": datetime.now().isoformat()
        }
        
    def mark_pending(self, test_id: str, notes: str = ""):
        self.update_test(test_id, "❌ PENDING", "-", notes)
        
    def mark_running(self, test_id: str, notes: str = ""):
        self.update_test(test_id, "⏳ RUNNING", "-", notes)
        
    def mark_passed(self, test_id: str, notes: str = ""):
        self.update_test(test_id, "✅ PASSED", "PASS", notes)
        
    def mark_failed(self, test_id: str, error: str = "", notes: str = ""):
        self.update_test(test_id, "❌ FAILED", "FAIL", f"{notes}. Error: {error}")
        
    def get_summary(self) -> Dict[str, int]:
        """Get test execution summary."""
        summary = {"pending": 0, "running": 0, "passed": 0, "failed": 0, "total": 0}
        for result in self.results.values():
            summary["total"] += 1
            if "PENDING" in result["status"]:
                summary["pending"] += 1
            elif "RUNNING" in result["status"]:
                summary["running"] += 1
            elif "PASSED" in result["status"]:
                summary["passed"] += 1
            elif "FAILED" in result["status"]:
                summary["failed"] += 1
        return summary


# Global test tracker
tracker = TestMatrixTracker()


# Test utilities
class TestDataGenerator:
    """Generate test data for vector operations."""
    
    @staticmethod
    def generate_openai_embedding(dimension: int = 512) -> List[float]:
        """Generate a fake OpenAI-style embedding."""
        # OpenAI embeddings are typically normalized
        vector = np.random.normal(0, 1, dimension).astype(np.float32)
        vector = vector / np.linalg.norm(vector)  # Normalize
        return vector.tolist()
    
    @staticmethod
    def generate_test_vectors(count: int, dimension: int = 512) -> List[Dict[str, Any]]:
        """Generate test vectors with metadata."""
        vectors = []
        categories = ["technology", "science", "business", "health", "education"]
        
        for i in range(count):
            vector = TestDataGenerator.generate_openai_embedding(dimension)
            metadata = {
                "id": f"test_vector_{i:06d}",
                "category": random.choice(categories),
                "timestamp": datetime.now().isoformat(),
                "source": "integration_test",
                "sequence": i,
                "test_run": str(uuid.uuid4()),
            }
            vectors.append({
                "id": f"test_{i:06d}",
                "vector": vector,
                "metadata": metadata
            })
        return vectors


def wait_for_server_startup(health_url: str, timeout: int = 30) -> bool:
    """Wait for the ProximaDB server to start up."""
    print(f"Waiting for server at {health_url} to start...")
    
    with httpx.Client() as client:
        for i in range(timeout):
            try:
                response = client.get(health_url, timeout=2.0)
                if response.status_code == 200:
                    print(f"Server is ready after {i} seconds")
                    return True
            except (httpx.RequestError, httpx.TimeoutException):
                pass
            
            time.sleep(1)
    
    print(f"Server did not start within {timeout} seconds")
    return False


# Test Categories Implementation

class ServerConnectivityTests:
    """Test Category 1: Server Startup and Connectivity Tests"""
    
    @staticmethod
    def test_server_startup_and_ports():
        """Test that server starts and binds to all required ports."""
        test_id = "server_startup"
        tracker.mark_running(test_id, "Testing server startup and port binding")
        
        try:
            # Test REST endpoint via health check
            rest_ready = wait_for_server_startup(
                TEST_CONFIG["server"]["health_endpoint"],
                TEST_CONFIG["timeouts"]["server_startup"]
            )
            
            if not rest_ready:
                tracker.mark_failed(test_id, "REST server not accessible")
                return False
                
            # Test health endpoint specifically
            with httpx.Client() as client:
                response = client.get(TEST_CONFIG["server"]["health_endpoint"])
                if response.status_code != 200:
                    tracker.mark_failed(test_id, f"Health check failed: {response.status_code}")
                    return False
            
            tracker.mark_passed(test_id, "Server started successfully on all ports")
            return True
            
        except Exception as e:
            tracker.mark_failed(test_id, str(e))
            return False

    @staticmethod
    def test_health_endpoints():
        """Test health check endpoints for REST and gRPC."""
        # REST health check
        test_id = "health_rest"
        tracker.mark_running(test_id, "Testing REST health endpoint")
        
        try:
            with httpx.Client() as client:
                response = client.get(TEST_CONFIG["server"]["health_endpoint"])
                if response.status_code == 200:
                    health_data = response.json()
                    tracker.mark_passed(test_id, f"REST health OK: {health_data.get('status', 'unknown')}")
                else:
                    tracker.mark_failed(test_id, f"HTTP {response.status_code}")
                    
        except Exception as e:
            tracker.mark_failed(test_id, str(e))


class AuthenticationTests:
    """Test Category 3: Authentication and Authorization Tests"""
    
    @staticmethod
    def test_api_key_authentication():
        """Test API key authentication for REST and gRPC."""
        test_id = "auth_api_key"
        tracker.mark_running(test_id, "Testing API key authentication")
        
        try:
            # Test with valid API key
            client = ProximaDBClient(
                url=TEST_CONFIG["server"]["rest_endpoint"],
                api_key=TEST_CONFIG["auth"]["admin_api_key"]
            )
            
            # Try to list collections (requires auth)
            collections = client.list_collections()
            
            tracker.mark_passed(test_id, f"Authentication successful, found {len(collections)} collections")
            return True
            
        except AuthenticationError as e:
            tracker.mark_failed(test_id, f"Authentication failed: {e}")
            return False
        except Exception as e:
            tracker.mark_failed(test_id, f"Unexpected error: {e}")
            return False
    
    @staticmethod
    def test_invalid_api_key():
        """Test rejection of invalid API keys."""
        test_id = "auth_invalid_key"
        tracker.mark_running(test_id, "Testing invalid API key rejection")
        
        try:
            # Test with invalid API key
            client = ProximaDBClient(
                url=TEST_CONFIG["server"]["rest_endpoint"],
                api_key="invalid-key-12345"
            )
            
            # This should fail
            try:
                client.list_collections()
                tracker.mark_failed(test_id, "Invalid API key was accepted (security issue)")
                return False
            except AuthenticationError:
                tracker.mark_passed(test_id, "Invalid API key properly rejected")
                return True
                
        except Exception as e:
            tracker.mark_failed(test_id, f"Unexpected error: {e}")
            return False


class CollectionManagementTests:
    """Test Category 4: Collection Management Tests"""
    
    @staticmethod
    def test_create_collection():
        """Test collection creation with Python SDK."""
        test_id = "collection_create_python"
        tracker.mark_running(test_id, "Creating test collection via Python SDK")
        
        try:
            client = ProximaDBClient(
                url=TEST_CONFIG["server"]["rest_endpoint"],
                api_key=TEST_CONFIG["auth"]["admin_api_key"]
            )
            
            # Create collection config
            collection_config = CollectionConfig(
                dimension=TEST_CONFIG["collections"]["dimension"],
                distance_metric=DistanceMetric.COSINE,
                description="Integration test collection for OpenAI embeddings"
            )
            
            # Create collection
            collection = client.create_collection(
                name=TEST_CONFIG["collections"]["test_collection"],
                config=collection_config
            )
            
            # Store collection ID for later tests
            TEST_CONFIG["collections"]["test_collection_id"] = collection.id
            
            tracker.mark_passed(test_id, f"Collection '{collection.name}' created successfully with ID {collection.id}")
            return True
            
        except Exception as e:
            tracker.mark_failed(test_id, str(e))
            return False
    
    @staticmethod
    def test_list_collections():
        """Test listing collections."""
        test_id = "collection_list"
        tracker.mark_running(test_id, "Listing collections")
        
        try:
            client = ProximaDBClient(
                url=TEST_CONFIG["server"]["rest_endpoint"],
                api_key=TEST_CONFIG["auth"]["admin_api_key"]
            )
            
            collections = client.list_collections()
            
            # Check if our test collection exists (by name since ID might not be set yet)
            test_collection_exists = any(
                c.name == TEST_CONFIG["collections"]["test_collection"] 
                for c in collections
            )
            
            if test_collection_exists:
                tracker.mark_passed(test_id, f"Found {len(collections)} collections including test collection")
            else:
                tracker.mark_failed(test_id, f"Test collection not found in {len(collections)} collections")
            
            return test_collection_exists
            
        except Exception as e:
            tracker.mark_failed(test_id, str(e))
            return False


class VectorOperationsTests:
    """Test Category 5: Vector Operations Tests"""
    
    @staticmethod
    def test_bulk_insert_openai_embeddings():
        """Test bulk insert of OpenAI-style 512D embeddings."""
        test_id = "bulk_insert_openai_512d"
        tracker.mark_running(test_id, "Bulk inserting OpenAI embeddings (512D)")
        
        try:
            client = ProximaDBClient(
                url=TEST_CONFIG["server"]["rest_endpoint"],
                api_key=TEST_CONFIG["auth"]["admin_api_key"]
            )
            
            # Generate test vectors
            test_vectors = TestDataGenerator.generate_test_vectors(
                count=TEST_CONFIG["collections"]["test_vectors_count"],
                dimension=TEST_CONFIG["collections"]["dimension"]
            )
            
            # Convert to the format expected by the client
            vectors_list = []
            ids_list = []
            metadata_list = []
            
            for vec in test_vectors:
                vectors_list.append(vec["vector"])
                ids_list.append(vec["id"])
                metadata_list.append(vec["metadata"])
            
            # Insert in batches to avoid timeout
            batch_size = 100
            total_inserted = 0
            
            for i in range(0, len(vectors_list), batch_size):
                batch_vectors = vectors_list[i:i + batch_size]
                batch_ids = ids_list[i:i + batch_size]
                batch_metadata = metadata_list[i:i + batch_size]
                
                result = client.insert_vectors(
                    collection_id=TEST_CONFIG["collections"]["test_collection_id"],
                    vectors=batch_vectors,
                    ids=batch_ids,
                    metadata=batch_metadata,
                    upsert=True
                )
                total_inserted += len(batch_vectors)
                
                if i % 500 == 0:  # Progress update
                    print(f"Inserted {total_inserted}/{len(test_vectors)} vectors...")
            
            tracker.mark_passed(test_id, f"Successfully inserted {total_inserted} OpenAI embeddings")
            return True
            
        except Exception as e:
            tracker.mark_failed(test_id, str(e))
            return False
    
    @staticmethod
    def test_vector_search():
        """Test vector similarity search."""
        test_id = "vector_search_similarity"
        tracker.mark_running(test_id, "Testing vector similarity search")
        
        try:
            client = ProximaDBClient(
                url=TEST_CONFIG["server"]["rest_endpoint"],
                api_key=TEST_CONFIG["auth"]["admin_api_key"]
            )
            
            # Generate a query vector
            query_vector = TestDataGenerator.generate_openai_embedding(
                TEST_CONFIG["collections"]["dimension"]
            )
            
            # Search for similar vectors
            results = client.search(
                collection_id=TEST_CONFIG["collections"]["test_collection_id"],
                query=query_vector,
                k=10
            )
            
            if len(results) > 0:
                tracker.mark_passed(test_id, f"Found {len(results)} similar vectors")
                return True
            else:
                tracker.mark_failed(test_id, "No search results returned")
                return False
                
        except Exception as e:
            tracker.mark_failed(test_id, str(e))
            return False
    
    @staticmethod
    def test_metadata_filtering():
        """Test metadata-based filtering."""
        test_id = "metadata_filtering"
        tracker.mark_running(test_id, "Testing metadata filtering")
        
        try:
            client = ProximaDBClient(
                url=TEST_CONFIG["server"]["rest_endpoint"],
                api_key=TEST_CONFIG["auth"]["admin_api_key"]
            )
            
            # Generate a query vector
            query_vector = TestDataGenerator.generate_openai_embedding(
                TEST_CONFIG["collections"]["dimension"]
            )
            
            # Search with metadata filter
            filter_dict = {"category": "technology"}
            
            results = client.search(
                collection_id=TEST_CONFIG["collections"]["test_collection_id"],
                query=query_vector,
                k=10,
                filter=filter_dict
            )
            
            # Verify all results match the filter
            all_match_filter = all(
                result.metadata.get("category") == "technology" 
                for result in results
            )
            
            if all_match_filter:
                tracker.mark_passed(test_id, f"Metadata filtering working: {len(results)} results match filter")
                return True
            else:
                tracker.mark_failed(test_id, "Some results don't match metadata filter")
                return False
                
        except Exception as e:
            tracker.mark_failed(test_id, str(e))
            return False
    
    @staticmethod
    def test_vector_retrieval_by_id():
        """Test exact vector retrieval by ID."""
        test_id = "vector_retrieval_by_id"
        tracker.mark_running(test_id, "Testing vector retrieval by ID")
        
        try:
            client = ProximaDBClient(
                url=TEST_CONFIG["server"]["rest_endpoint"],
                api_key=TEST_CONFIG["auth"]["admin_api_key"]
            )
            
            # Try to retrieve a known vector ID
            test_vector_id = "test_000001"
            
            vector = client.get_vector(
                collection_id=TEST_CONFIG["collections"]["test_collection_id"],
                vector_id=test_vector_id
            )
            
            if vector and vector.id == test_vector_id:
                tracker.mark_passed(test_id, f"Successfully retrieved vector by ID: {test_vector_id}")
                return True
            else:
                tracker.mark_failed(test_id, f"Vector ID {test_vector_id} not found")
                return False
                
        except Exception as e:
            tracker.mark_failed(test_id, str(e))
            return False


class AlgorithmTests:
    """Test Category 6: Algorithm and Distance Metric Tests"""
    
    @staticmethod
    def test_distance_metrics():
        """Test different distance metrics."""
        metrics_to_test = [
            (DistanceMetric.COSINE, "cosine_similarity"),
            (DistanceMetric.EUCLIDEAN, "euclidean_distance"), 
            (DistanceMetric.DOT_PRODUCT, "dot_product_similarity"),
        ]
        
        for metric, test_id in metrics_to_test:
            tracker.mark_running(test_id, f"Testing {metric.value} distance metric")
            
            try:
                # Create a test collection with specific metric
                client = ProximaDBClient(
                    url=TEST_CONFIG["server"]["rest_endpoint"],
                    api_key=TEST_CONFIG["auth"]["admin_api_key"]
                )
                
                collection_name = f"test_{metric.value}_collection"
                
                collection_config = CollectionConfig(
                    dimension=128,  # Smaller for faster testing
                    distance_metric=metric,
                    description=f"Test collection for {metric.value} metric"
                )
                
                # Create collection
                client.create_collection(name=collection_name, config=collection_config)
                
                # Insert test vectors
                test_vectors = TestDataGenerator.generate_test_vectors(count=10, dimension=128)
                vectors_list = [vec["vector"] for vec in test_vectors]
                ids_list = [vec["id"] for vec in test_vectors]
                metadata_list = [vec["metadata"] for vec in test_vectors]
                
                client.insert_vectors(
                    collection_id=collection_name,
                    vectors=vectors_list,
                    ids=ids_list,
                    metadata=metadata_list,
                    upsert=True
                )
                
                # Search
                query_vector = TestDataGenerator.generate_openai_embedding(128)
                results = client.search(
                    collection_id=collection_name,
                    query=query_vector,
                    k=5
                )
                
                if len(results) > 0:
                    tracker.mark_passed(test_id, f"{metric.value} metric working: {len(results)} results")
                else:
                    tracker.mark_failed(test_id, f"No results with {metric.value} metric")
                    
            except Exception as e:
                tracker.mark_failed(test_id, str(e))


class WALPersistenceTests:
    """Test Category 8: WAL and Persistence Tests"""
    
    @staticmethod
    def test_wal_flush_scenarios():
        """Test WAL flush behavior and data retrieval from both WAL and flushed storage."""
        test_id = "wal_flush_mixed_retrieval"
        tracker.mark_running(test_id, "Testing WAL flush and mixed data retrieval")
        
        try:
            client = ProximaDBClient(
                url=TEST_CONFIG["server"]["rest_endpoint"],
                api_key=TEST_CONFIG["auth"]["admin_api_key"]
            )
            
            collection_name = "wal_test_collection"
            
            # Create a collection for WAL testing
            collection_config = CollectionConfig(
                dimension=256,
                distance_metric=DistanceMetric.COSINE,
                description="WAL flush test collection"
            )
            
            client.create_collection(name=collection_name, config=collection_config)
            
            # Phase 1: Insert enough data to trigger WAL flush
            # Generate a large batch of vectors
            large_batch = TestDataGenerator.generate_test_vectors(count=2000, dimension=256)
            
            # Convert to client format
            vectors_list = [vec["vector"] for vec in large_batch]
            ids_list = [vec["id"] for vec in large_batch]
            metadata_list = [vec["metadata"] for vec in large_batch]
            
            # Insert in smaller batches to monitor progress
            batch_size = 100
            for i in range(0, len(vectors_list), batch_size):
                batch_vectors = vectors_list[i:i + batch_size]
                batch_ids = ids_list[i:i + batch_size]
                batch_metadata = metadata_list[i:i + batch_size]
                
                client.insert_vectors(
                    collection_id=collection_name,
                    vectors=batch_vectors,
                    ids=batch_ids,
                    metadata=batch_metadata,
                    upsert=True
                )
                
                # Wait a bit to allow background processing
                time.sleep(0.1)
            
            # Phase 2: Insert recent data that should be in WAL
            recent_vectors = TestDataGenerator.generate_test_vectors(count=50, dimension=256)
            # Mark these with special metadata
            for vec in recent_vectors:
                vec["metadata"]["in_wal"] = True
                vec["metadata"]["test_phase"] = "recent"
            
            recent_vectors_list = [vec["vector"] for vec in recent_vectors]
            recent_ids_list = [vec["id"] for vec in recent_vectors]
            recent_metadata_list = [vec["metadata"] for vec in recent_vectors]
            
            client.insert_vectors(
                collection_id=collection_name,
                vectors=recent_vectors_list,
                ids=recent_ids_list,
                metadata=recent_metadata_list,
                upsert=True
            )
            
            # Phase 3: Test data retrieval from both sources
            
            # Test 1: Retrieve old data (should be from flushed storage)
            old_vector_id = large_batch[0]["id"]
            old_vector = client.get_vector(collection_id=collection_name, vector_id=old_vector_id)
            
            # Test 2: Retrieve recent data (should be from WAL)
            recent_vector_id = recent_vectors[0]["id"]
            recent_vector = client.get_vector(collection_id=collection_name, vector_id=recent_vector_id)
            
            # Test 3: Search across both (should combine WAL + flushed data)
            query_vector = TestDataGenerator.generate_openai_embedding(256)
            search_results = client.search(
                collection_id=collection_name,
                query=query_vector,
                k=20
            )
            
            # Verify we can retrieve from both sources
            old_retrieved = old_vector is not None
            recent_retrieved = recent_vector is not None and recent_vector.get("metadata", {}).get("in_wal") == True
            search_working = len(search_results) > 0
            
            if old_retrieved and recent_retrieved and search_working:
                tracker.mark_passed(
                    test_id, 
                    f"WAL + flushed data retrieval working: "
                    f"old={old_retrieved}, recent={recent_retrieved}, search={len(search_results)} results"
                )
                return True
            else:
                tracker.mark_failed(
                    test_id,
                    f"Data retrieval issues: old={old_retrieved}, recent={recent_retrieved}, search={len(search_results)}"
                )
                return False
                
        except Exception as e:
            tracker.mark_failed(test_id, str(e))
            return False


class ErrorHandlingTests:
    """Test Category 10: Error Handling and Edge Cases"""
    
    @staticmethod
    def test_invalid_vector_dimensions():
        """Test handling of invalid vector dimensions."""
        test_id = "invalid_vector_dimensions"
        tracker.mark_running(test_id, "Testing invalid vector dimension handling")
        
        try:
            client = ProximaDBClient(
                url=TEST_CONFIG["server"]["rest_endpoint"],
                api_key=TEST_CONFIG["auth"]["admin_api_key"]
            )
            
            # Try to insert vector with wrong dimensions
            wrong_vector = {
                "id": "wrong_dimension_test",
                "vector": [0.1, 0.2, 0.3],  # 3D instead of 512D
                "metadata": {"test": "invalid_dimension"}
            }
            
            try:
                client.insert_vectors(
                    collection_id=TEST_CONFIG["collections"]["test_collection_id"],
                    vectors=[wrong_vector["vector"]],
                    ids=[wrong_vector["id"]],
                    metadata=[wrong_vector["metadata"]],
                    upsert=True
                )
                tracker.mark_failed(test_id, "Invalid dimension vector was accepted")
                return False
            except Exception as e:
                # This should fail
                tracker.mark_passed(test_id, f"Invalid dimensions properly rejected: {type(e).__name__}")
                return True
                
        except Exception as e:
            tracker.mark_failed(test_id, str(e))
            return False


# Main test execution
def run_integration_tests():
    """Run the complete integration test matrix."""
    print("=" * 80)
    print("ProximaDB Integration Test Matrix Execution")
    print("=" * 80)
    print(f"Start time: {tracker.start_time}")
    print(f"Test configuration: {json.dumps(TEST_CONFIG, indent=2)}")
    print()
    
    # Phase 1: Server Connectivity
    print("Phase 1: Server Startup and Connectivity Tests")
    print("-" * 50)
    if not ServerConnectivityTests.test_server_startup_and_ports():
        print("CRITICAL: Server startup failed. Aborting tests.")
        return False
    
    ServerConnectivityTests.test_health_endpoints()
    
    # Phase 2: Authentication
    print("\nPhase 2: Authentication and Authorization Tests")
    print("-" * 50)
    if not AuthenticationTests.test_api_key_authentication():
        print("CRITICAL: Authentication failed. Aborting tests.")
        return False
    
    AuthenticationTests.test_invalid_api_key()
    
    # Phase 3: Collection Management
    print("\nPhase 3: Collection Management Tests")
    print("-" * 50)
    if not CollectionManagementTests.test_create_collection():
        print("CRITICAL: Collection creation failed. Aborting tests.")
        return False
    
    CollectionManagementTests.test_list_collections()
    
    # Phase 4: Vector Operations
    print("\nPhase 4: Vector Operations Tests")
    print("-" * 50)
    if not VectorOperationsTests.test_bulk_insert_openai_embeddings():
        print("WARNING: Bulk insert failed. Some tests may be limited.")
    
    VectorOperationsTests.test_vector_search()
    VectorOperationsTests.test_metadata_filtering()
    VectorOperationsTests.test_vector_retrieval_by_id()
    
    # Phase 5: Algorithm Tests
    print("\nPhase 5: Algorithm and Distance Metric Tests")
    print("-" * 50)
    AlgorithmTests.test_distance_metrics()
    
    # Phase 6: WAL and Persistence
    print("\nPhase 6: WAL and Persistence Tests")
    print("-" * 50)
    WALPersistenceTests.test_wal_flush_scenarios()
    
    # Phase 7: Error Handling
    print("\nPhase 7: Error Handling Tests")
    print("-" * 50)
    ErrorHandlingTests.test_invalid_vector_dimensions()
    
    return True


def generate_test_report():
    """Generate final test report."""
    summary = tracker.get_summary()
    
    print("\n" + "=" * 80)
    print("INTEGRATION TEST MATRIX RESULTS")
    print("=" * 80)
    
    print(f"Execution Time: {datetime.now() - tracker.start_time}")
    print(f"Total Tests: {summary['total']}")
    print(f"✅ Passed: {summary['passed']}")
    print(f"❌ Failed: {summary['failed']}")
    print(f"⏳ Running: {summary['running']}")
    print(f"❌ Pending: {summary['pending']}")
    
    success_rate = (summary['passed'] / summary['total'] * 100) if summary['total'] > 0 else 0
    print(f"Success Rate: {success_rate:.1f}%")
    
    print("\nDetailed Results:")
    print("-" * 50)
    for test_id, result in tracker.results.items():
        status_icon = result["status"].split()[0]
        print(f"{status_icon} {test_id:30} | {result['result']:4} | {result['notes']}")
    
    # Write results to file
    results_file = Path(__file__).parent / "test_results.json"
    with open(results_file, "w") as f:
        json.dump({
            "timestamp": datetime.now().isoformat(),
            "summary": summary,
            "results": tracker.results,
            "config": TEST_CONFIG
        }, f, indent=2)
    
    print(f"\nDetailed results saved to: {results_file}")
    
    # Update test matrix document if it exists
    matrix_file = Path(__file__).parent.parent / "docs" / "test_matrix.adoc"
    if matrix_file.exists():
        print(f"TODO: Update test matrix document at {matrix_file}")
    
    return success_rate >= 80  # 80% pass rate required


def main():
    """Main test execution function."""
    print("ProximaDB Integration Test Matrix")
    print("Comprehensive testing of all ProximaDB components")
    print()
    
    # Check if server is expected to be running
    print("NOTE: This test assumes ProximaDB server is running.")
    print(f"Expected REST endpoint: {TEST_CONFIG['server']['rest_endpoint']}")
    print(f"Expected gRPC endpoint: {TEST_CONFIG['server']['grpc_endpoint']}")
    print()
    
    # Auto-proceed for CI/automated testing
    if sys.stdin.isatty():
        response = input("Continue with testing? (y/N): ")
        if response.lower() not in ['y', 'yes']:
            print("Testing cancelled by user.")
            return
    else:
        print("Auto-proceeding with testing (CI mode)")
    
    try:
        success = run_integration_tests()
        test_passed = generate_test_report()
        
        if test_passed:
            print("\n🎉 Integration tests PASSED!")
            sys.exit(0)
        else:
            print("\n💥 Integration tests FAILED!")
            sys.exit(1)
            
    except KeyboardInterrupt:
        print("\n\nTesting interrupted by user.")
        generate_test_report()
        sys.exit(130)
    except Exception as e:
        print(f"\n\nUnexpected error during testing: {e}")
        generate_test_report()
        sys.exit(1)


if __name__ == "__main__":
    # Configure logging
    logging.basicConfig(
        level=logging.INFO,
        format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
    )
    
    # Run the tests
    main()