#!/usr/bin/env python3
"""
Test gRPC workflow: create collection, insert vectors, search

Usage:
    PYTHONPATH=src python test_grpc_workflow.py
"""

import sys
import os
import logging
import numpy as np

logging.basicConfig(
    level=logging.INFO, format="%(asctime)s - %(levelname)s - %(message)s"
)
logger = logging.getLogger(__name__)

if "PYTHONPATH" not in os.environ:
    sys.path.insert(0, os.path.join(os.path.dirname(__file__), "src"))

from proximadb.client_v1 import ProximaDBClientV1 as ProximaDB
from proximadb.models import VectorRecord


def main():
    # Use localhost:5679 directly - the client will detect port 5679 as gRPC
    client = ProximaDB(url="localhost:5679", protocol="grpc")
    import time

    collection_name = f"test_grpc_workflow_{int(time.time())}"

    try:
        # 1. Clean up existing collection
        try:
            client.delete_collection(collection_name)
            logger.info(f"Deleted existing collection '{collection_name}'")
        except:
            pass

        # 2. Create collection
        logger.info("Step 1: Creating collection...")
        collection = client.create_collection(
            name=collection_name,
            dimension=128,
            distance_metric="cosine",
            storage_engine="sst",
        )
        logger.info(f"✅ Collection created: {collection.name} (ID: {collection.id})")

        # 3. Insert vectors
        logger.info("Step 2: Inserting 10 vectors...")
        vectors = [
            VectorRecord(
                id=f"vec_{i}",
                vector=np.random.rand(128).tolist(),
                metadata={"category": "test", "index": i},
            )
            for i in range(10)
        ]

        result = client.insert_vectors(collection_id=collection_name, vectors=vectors)
        vector_ids = (
            result["vector_ids"] if isinstance(result, dict) else result.vector_ids
        )
        logger.info(f"✅ Inserted {len(vector_ids)} vectors")
        logger.info(f"   Vector IDs: {vector_ids[:3]}...")

        # 4. Search
        logger.info("Step 3: Searching for similar vectors...")
        query_vector = np.random.rand(128).tolist()

        search_results = client.search_vectors(
            collection_id=collection_name, vector=query_vector, top_k=5
        )

        logger.info(f"✅ Search completed!")
        logger.info(f"   Found {len(search_results)} results")
        for i, result in enumerate(search_results):
            logger.info(f"   [{i+1}] ID: {result.id}, Score: {result.score:.4f}")
            if result.metadata:
                logger.info(f"       Metadata: {result.metadata}")

        logger.info("\n🎉 All tests passed!")

    except Exception as e:
        logger.error(f"\n❌ Error: {e}")
        import traceback

        traceback.print_exc()
        sys.exit(1)


if __name__ == "__main__":
    main()
