#!/usr/bin/env python3
"""
Test script for the v1 ProximaDB client to validate imports and basic functionality.

Usage:
    PYTHONPATH=src python test_v1_client.py

Note: Set PYTHONPATH environment variable to include the 'src' directory
instead of modifying sys.path. This is the recommended approach.
"""

import sys
import os
import logging

# Setup logging
logging.basicConfig(
    level=logging.INFO, format="%(asctime)s - %(name)s - %(levelname)s - %(message)s"
)
logger = logging.getLogger(__name__)

# Recommended: Use PYTHONPATH=src instead of this sys.path modification
# Example: PYTHONPATH=src python test_v1_client.py
if "PYTHONPATH" not in os.environ:
    logger.warning("⚠️  Recommendation: Set PYTHONPATH=src environment variable")
    logger.info("   Example: PYTHONPATH=src python test_v1_client.py")
    logger.info("   Falling back to sys.path modification...\n")
    sys.path.insert(0, os.path.join(os.path.dirname(__file__), "src"))


def test_v1_imports():
    """Test that v1 proto and client imports work correctly"""
    logger.info("Testing v1 client imports...")

    try:
        # Test proto imports
        from proximadb.v1 import (
            vector_pb2,
            vector_pb2_grpc,
            collection_pb2,
            collection_pb2_grpc,
            vector_types_pb2,
            collection_types_pb2,
        )

        logger.info("✅ Proto v1 imports successful")

        # Test client import
        from proximadb.client_v1 import ProximaDBClientV1

        logger.info("✅ Client v1 import successful")

        # Test client instantiation
        client = ProximaDBClientV1(url="http://localhost:5678")
        logger.info("✅ Client v1 instantiation successful")
        logger.info(f"   Protocol: {client.protocol}")
        logger.info(f"   Base URL: {client.base_url}")

        # Test model imports
        from proximadb.models import (
            VectorRecord,
            SearchResult,
            Collection,
            DistanceMetric,
            StorageEngine,
        )

        logger.info("✅ Model imports successful")

        # Test VectorRecord creation
        vector = VectorRecord(
            id="test_vector", vector=[0.1, 0.2, 0.3, 0.4], metadata={"test": "data"}
        )
        logger.info("✅ VectorRecord creation successful: %s", vector.id)

        return True

    except ImportError as e:
        logger.error("❌ Import error: %s", e)
        return False
    except Exception as e:
        logger.error("❌ General error: %s", e)
        return False


def test_proto_message_creation():
    """Test that we can create proto messages"""
    logger.info("\nTesting proto message creation...")

    try:
        from proximadb.v1 import vector_types_pb2, collection_types_pb2

        # Test VectorRecord proto creation (metadata as empty dict for proto compatibility)
        vector_proto = vector_types_pb2.VectorRecord(
            id="test_id", vector=[1.0, 2.0, 3.0]
        )
        logger.info("✅ VectorRecord proto created: %s", vector_proto.id)

        # Test CollectionConfig proto creation
        collection_proto = collection_types_pb2.CollectionConfig(
            name="test_collection",
            dimension=128,
            distance_metric="COSINE",
            storage_engine="SST",
        )
        logger.info("✅ CollectionConfig proto created: %s", collection_proto.name)

        # Test VectorBatchRequest proto creation
        batch_request = vector_types_pb2.VectorBatchRequest(
            collection_id="test_collection", vectors=[vector_proto]
        )
        logger.info(
            f"✅ VectorBatchRequest proto created with {len(batch_request.vectors)} vectors"
        )

        return True

    except Exception as e:
        logger.error("❌ Proto message creation error: %s", e)
        return False


def main():
    """Run all tests"""
    logger.info("ProximaDB Python SDK v1 Client Test")
    print("=" * 50)

    success = True

    # Test imports
    success &= test_v1_imports()

    # Test proto message creation
    success &= test_proto_message_creation()

    print("\n" + "=" * 50)
    if success:
        logger.info("🎉 All tests passed! v1 client is ready.")
        return 0
    else:
        logger.error("❌ Some tests failed. Check the output above.")
        return 1


if __name__ == "__main__":
    sys.exit(main())
