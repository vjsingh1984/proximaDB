#!/usr/bin/env python3
"""
Debug REST client
"""

import httpx
import json

def test_create_collection():
    """Test collection creation directly"""
    
    url = "http://localhost:5678/api/v1/collection"
    
    request_data = {
        "operation": "create",
        "config": {
            "name": "test_debug",
            "dimension": 128,
            "distance_metric": "cosine",
            "storage_engine": "viper",
            "indexing_algorithm": "hnsw"
        }
    }
    
    response = httpx.post(url, json=request_data)
    
    print(f"Status: {response.status_code}")
    print(f"Response: {json.dumps(response.json(), indent=2)}")

if __name__ == "__main__":
    test_create_collection()