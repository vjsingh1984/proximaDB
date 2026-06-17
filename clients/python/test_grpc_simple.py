#!/usr/bin/env python3
"""
Simple test to debug gRPC insert issues

Usage:
    PYTHONPATH=src python test_grpc_simple.py

Note: Set PYTHONPATH environment variable to include the 'src' directory
instead of modifying sys.path. This is the recommended approach.
"""

import sys
import os
import logging
import numpy as np

# Setup logging
logging.basicConfig(
    level=logging.INFO, format="%(asctime)s - %(name)s - %(levelname)s - %(message)s"
)
logger = logging.getLogger(__name__)

# Recommended: Use PYTHONPATH=src instead of this sys.path modification
# Example: PYTHONPATH=src python test_grpc_simple.py
if "PYTHONPATH" not in os.environ:
    logger.warning("Recommendation: Set PYTHONPATH=src environment variable")
    logger.warning("Example: PYTHONPATH=src python test_grpc_simple.py")
    logger.warning("Falling back to sys.path modification...")
    sys.path.insert(0, os.path.join(os.path.dirname(__file__), "src"))

from proximadb.client_v1 import ProximaDBClientV1 as ProximaDB
from proximadb.models import VectorRecord


def main():
    # Initialize clients
    grpc_client = ProximaDB(url="grpc://localhost:5679")
    rest_client = ProximaDB(url="http://localhost:5678")

    try:
        # Delete collection if it exists
        try:
            grpc_client.delete_collection("test_grpc")
            logger.info("Deleted existing collection")
        except:
            pass

        # Create collection via gRPC
        logger.info("1. Creating collection via gRPC...")
        collection = grpc_client.create_collection(
            name="test_grpc",
            dimension=128,
            distance_metric="cosine",
            storage_engine="sst",
        )
        logger.info(f"✅ Collection created: {collection.name} (ID: {collection.id})")

        # Insert ONE vector via gRPC
        logger.info("2. Inserting ONE vector via gRPC...")

        # Create vector using VectorRecord model
        vector = VectorRecord(
            id="test_vec_1",
            vector=np.random.rand(128).tolist(),
            metadata={"source": "grpc", "index": 1},
        )

        result = grpc_client.insert_vectors(collection_id="test_grpc", vectors=[vector])
        logger.info(f"✅ Insert result: {result}")
        logger.info(f"   Success: {result.success}")
        logger.info(f"   Vector IDs: {result.vector_ids}")

        # Try to get the vector via gRPC
        logger.info("3. Getting vector via gRPC...")
        try:
            vec_result = grpc_client.get_vector(
                collection_id="test_grpc", vector_id="test_vec_1"
            )
            logger.info(f"✅ gRPC get: Found vector")
            logger.info(f"   Response: {vec_result}")
        except Exception as e:
            logger.error(f"❌ gRPC get failed: {e}")

        # Try to get the vector via REST
        logger.info("4. Getting vector via REST...")
        try:
            vec_result = rest_client.get_vector(
                collection_id="test_grpc", vector_id="test_vec_1"
            )
            logger.info(f"✅ REST get: Found vector")
            logger.info(f"   Response: {vec_result}")
        except Exception as e:
            logger.error(f"❌ REST get failed: {e}")

        # Check debug endpoint
        logger.info("5. Checking debug endpoint...")
        import httpx

        with httpx.Client() as client:
            response = client.get("http://localhost:5678/debug/vectors/test_grpc")
            if response.status_code == 200:
                debug_data = response.json()
                logger.info(f"✅ Debug info:")
                logger.info(
                    f"   Unflushed vectors: {debug_data.get('unflushed_vector_count', 0)}"
                )
                logger.info(
                    f"   Vectors: {debug_data.get('vectors', [])[:1]}..."
                )  # Show first vector
            else:
                logger.error(f"❌ Debug endpoint failed: {response.status_code}")

    except Exception as e:
        logger.info(f"\n❌ Error: {e}")
        import traceback

        traceback.print_exc()


if __name__ == "__main__":
    main()
