#!/usr/bin/env python3
"""
Simple End-to-End Test for ProximaDB
Tests basic connectivity and operations without heavy dependencies
"""

import sys
import time

import requests

SERVER_URL = "http://localhost:5678"


def test_health_check():
    """Test health endpoint"""
    print("🏥 Testing health check...")
    response = requests.get(f"{SERVER_URL}/health")
    print(f"   Response status: {response.status_code}")
    if response.status_code == 200:
        print("✅ Health check passed")
        print(f"   Response: {response.json()}")
    assert response.status_code == 200, f"Health check failed: {response.status_code}"


def test_create_collection():
    """Test collection creation"""
    print("📦 Testing collection creation...")

    # Enum values matching server expectations
    collection_config = {
        "name": "test_collection",
        "dimension": 384,
        "distance_metric": 1,  # COSINE enum value
        "storage_engine": 1,  # VIPER enum value
        "tags": [],
        "filterable_columns": [],
        "index_configs": [
            {
                "index_name": "test_collection_primary",
                "algorithm": 1,  # HNSW enum value
                "parameters": {},
                "enabled": True,
                "update_mode": 0,
                "enable_background_optimization": True,
                "build_concurrency": 4,
                "memory_limit_mb": 512,
                "checkpoint_interval_ms": 60000,
                "is_primary": True,
                "use_cases": [],
                "selectivity_threshold": 0.5,
                "use_quantization": False,
                "queue_representation": "vector",
            }
        ],
        "primary_index": "test_collection_primary",
        "auto_index_selection": True,
        "embedding_models": [],
    }

    request_data = {
        "operation": 1,  # COLLECTION_CREATE enum value
        "collection_config": collection_config,
        "query_params": {},
        "options": {},
        "migration_config": {},
    }

    response = requests.post(
        f"{SERVER_URL}/api/v1/collections",
        json=request_data,
        headers={"Content-Type": "application/json"},
    )

    print(f"   Response status: {response.status_code}")
    if response.status_code in [200, 201]:
        print("✅ Collection created successfully")
        result = response.json()
        collection_id = result.get("data", result.get("id", "test_collection"))
        print(f"   Collection ID: {collection_id}")
    else:
        print(f"❌ Collection creation failed: {response.status_code}")
        print(f"   Response: {response.text}")

    assert response.status_code in [
        200,
        201,
    ], f"Collection creation failed: {response.status_code} - {response.text}"


def test_list_collections():
    """Test listing collections"""
    print("📋 Testing collection listing...")

    # Use GET request to /collections (plural) endpoint
    response = requests.get(f"{SERVER_URL}/api/v1/collections")

    print(f"   Response status: {response.status_code}")
    if response.status_code == 200:
        print("✅ Collections listed successfully")
        result = response.json()
        # Handle different response formats: either direct array in 'data' or nested 'collections'
        collections = result.get("data", [])
        if isinstance(collections, dict) and "collections" in collections:
            collections = collections["collections"]
        elif not isinstance(collections, list):
            collections = result.get("collections", [])
        print(f"   Found {len(collections)} collections")
    else:
        print(f"❌ Collection listing failed: {response.status_code}")
        print(f"   Response: {response.text}")

    assert (
        response.status_code == 200
    ), f"Collection listing failed: {response.status_code} - {response.text}"


# Note: test_vector_operations is not a proper pytest test since it takes parameters
# Vector operations are tested in other test files with proper pytest structure


def main():
    """Run all tests"""
    print("🚀 Starting ProximaDB End-to-End Test")
    print("=" * 50)

    # Give server time to start
    print("⏱️ Waiting for server startup...")
    time.sleep(2)

    results = []

    # Test 1: Health check
    results.append(test_health_check())

    # Test 2: Create collection
    collection_id = test_create_collection()
    results.append(collection_id is not None)

    # Test 3: List collections
    results.append(test_list_collections())

    # Test 4: Vector operations
    results.append(test_vector_operations(collection_id))

    # Summary
    print("\n" + "=" * 50)
    print("📊 Test Summary:")
    passed = sum(results)
    total = len(results)
    print(f"   ✅ Passed: {passed}/{total}")
    print(f"   ❌ Failed: {total - passed}/{total}")

    if passed == total:
        print("🎉 All tests passed!")
        return 0
    else:
        print("💥 Some tests failed!")
        return 1


if __name__ == "__main__":
    sys.exit(main())
