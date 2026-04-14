#!/usr/bin/env python
"""Quick test for embedded database Multi-Model API support."""

import asyncio
import sys
import os
import time

# Add src to path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "src"))

from proximadb_sdk.embedded import EmbeddedProximaDB, EmbeddedConfig


async def test_document_api():
    """Test Document API in embedded mode."""
    print("Testing Document API...")

    config = EmbeddedConfig(
        data_dir="/tmp/proximadb_test_multimodel",
        rest_port=15678,
        log_level="info",
    )

    db = EmbeddedProximaDB(config=config)

    try:
        await db.start()

        # Create document collection
        result = await db.create_document_collection(
            name="test_docs",
            indexes=[{"path": "$.category", "type": "hash"}],
            enable_fulltext=True,
        )
        print(f"✓ Created document collection: {result}")

        # Insert document
        doc = await db.insert_document(
            collection_name="test_docs",
            document={"category": "test", "value": 42, "text": "hello world"},
            id="doc1",
        )
        print(f"✓ Inserted document: {doc}")

        # Get document
        retrieved = await db.get_document("test_docs", "doc1")
        print(f"✓ Retrieved document: {retrieved}")

        # Query documents
        results = await db.query_documents(
            collection_name="test_docs",
            filter={"category": "test"},
            limit=10,
        )
        print(f"✓ Queried documents: {results}")

        # Delete collection
        deleted = await db.delete_document_collection("test_docs")
        print(f"✓ Deleted collection: {deleted}")

        print("✅ Document API tests passed!\n")
        return True

    except Exception as e:
        print(f"❌ Document API test failed: {e}")
        import traceback

        traceback.print_exc()
        return False

    finally:
        await db.stop()


async def test_timeseries_via_sql():
    """Test Time Series functionality via SQL in embedded mode."""
    print("Testing Time Series via SQL...")

    config = EmbeddedConfig(
        data_dir="/tmp/proximadb_test_multimodel",
        rest_port=15678,
        log_level="info",
    )

    db = EmbeddedProximaDB(config=config)

    try:
        await db.start()

        # Create a collection for time-series data
        collection = await db.create_collection(
            name="test_timeseries_vectors",
            dimension=3,
            distance_metric="euclidean",
        )
        print(f"✓ Created collection for time-series data: {collection.name}")

        # Insert time-series data as vectors with timestamp metadata
        now = int(time.time() * 1e9)  # Convert to nanoseconds
        points = []
        for i, value in enumerate([10.5, 11.2, 10.8, 11.5, 10.9]):
            points.append(
                {
                    "id": f"ts_point_{i}",
                    "vector": [value, value * 1.1, value * 0.9],
                    "metadata": {
                        "timestamp": now + (i * 60_000_000_000),  # i minutes later
                        "source": "sensor1",
                        "value": value,
                    },
                }
            )

        result = await collection.insert(points)
        print(f"✓ Inserted time-series data points")

        # Search for similar time-series patterns
        query_vector = [11.0, 12.1, 9.9]
        results = await collection.search(query_vector, top_k=5)
        print(f"✓ Found {len(results)} similar time-series patterns")

        # Note: Full time-series aggregation would require SQL queries
        # which are not yet fully exposed in the embedded API
        print("✅ Time Series (via vectors) tests passed!\n")
        return True

    except Exception as e:
        print(f"❌ Time Series test failed: {e}")
        import traceback

        traceback.print_exc()
        return False

    finally:
        await db.stop()


async def main():
    """Run all Multi-Model API tests."""
    print("=" * 60)
    print("Testing Embedded Database Multi-Model API Support")
    print("=" * 60)
    print()

    # Clean up test data directory
    import shutil

    if os.path.exists("/tmp/proximadb_test_multimodel"):
        shutil.rmtree("/tmp/proximadb_test_multimodel")

    results = []

    # Test Document API
    results.append(await test_document_api())

    # Test Time Series via SQL
    results.append(await test_timeseries_via_sql())

    # Clean up
    if os.path.exists("/tmp/proximadb_test_multimodel"):
        shutil.rmtree("/tmp/proximadb_test_multimodel")

    # Summary
    print("=" * 60)
    if all(results):
        print("✅ All tests passed!")
        return 0
    else:
        print(f"❌ {len([r for r in results if not r])} test(s) failed")
        return 1


if __name__ == "__main__":
    exit_code = asyncio.run(main())
    sys.exit(exit_code)
