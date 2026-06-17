#!/usr/bin/env python3
"""
HTTP Integration Test - test_search_simple.py

Usage:
    python test_search_simple.py

This test makes direct HTTP requests to test server endpoints.
"""

import requests
import json
import logging

# Setup logging
logging.basicConfig(
    level=logging.INFO, format="%(asctime)s - %(name)s - %(levelname)s - %(message)s"
)
logger = logging.getLogger(__name__)

base_url = "http://localhost:5678"

logger.info("=== Testing Vector Search - Simplified ===")
logger.info("")

collection_name = "recovery_test_collection"

# 1. Simplest possible search
logger.info("1. Simplest search request:")
search_data = {
    "collection_id": collection_name,
    "queries": [{"vector": [0.5] * 128}],
    "top_k": 10,
}

response = requests.post(f"{base_url}/api/v1/vector/search", json=search_data)
logger.info(f"   Response: {response.status_code}")

if response.status_code == 200:
    results = response.json()
    logger.info(f"   ✓ Search successful!")
    logger.info(f"   Full response: {json.dumps(results, indent=2)}")
else:
    logger.info(f"   ✗ Error: {response.text}")

logger.info("")

# 2. Let's check if WAL has any data now
logger.info("2. Checking storage directories:")
import os

# Check LSM WAL
wal_dir = "./lsm_wal"
if os.path.exists(wal_dir):
    files = os.listdir(wal_dir)
    logger.info(f"   LSM WAL: {len(files)} files")

# Check main data directory
data_dir = "/tmp/proximadb-test"
if os.path.exists(data_dir):
    for root, dirs, files in os.walk(data_dir):
        if files:
            logger.info(f"   {root}: {len(files)} files")
            for f in files[:3]:
                logger.info(f"     - {f}")

logger.info("")
logger.info("=== Test Complete ===")
