#!/usr/bin/env python3
"""
Integration tests for embedded Document and Observability APIs.

Tests the following APIs:
- Document Storage: create_document_collection, insert_document, get_document, query_documents, delete_document
- Observability: create_observability_namespace, ingest_logs, query_logs, ingest_metrics
"""

import os
import sys
import tempfile
import time
import shutil

# Add the Python SDK to the path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'clients', 'python', 'src'))

def test_document_storage_api():
    """Test the Document Storage API."""
    print("\n" + "=" * 60)
    print("Testing Document Storage API")
    print("=" * 60)

    import proximadb

    # Create a temporary directory for the test
    test_dir = tempfile.mkdtemp(prefix="proximadb_doc_test_")
    print(f"Using test directory: {test_dir}")

    try:
        # Initialize the database
        db = proximadb.ProximaDB(test_dir)
        print("Database initialized")

        # Test 1: Create document collection
        print("\n1. Creating document collection...")
        db.create_document_collection("users", indexed_paths=["$.email", "$.profile.name"])
        print("   Created collection 'users' with indexed paths")

        # Test 2: Insert documents
        print("\n2. Inserting documents...")

        doc1 = {
            "name": "John Doe",
            "email": "john@example.com",
            "profile": {
                "age": 30,
                "city": "New York"
            },
            "tags": ["developer", "python"]
        }
        doc1_id, doc1_version = db.insert_document("users", doc1)
        print(f"   Inserted doc1: id={doc1_id}, version={doc1_version}")

        doc2 = {
            "name": "Jane Smith",
            "email": "jane@example.com",
            "profile": {
                "age": 28,
                "city": "San Francisco"
            },
            "tags": ["designer", "ux"]
        }
        doc2_id, doc2_version = db.insert_document("users", doc2, doc_id="user_jane")
        print(f"   Inserted doc2: id={doc2_id}, version={doc2_version}")

        # Test 3: Get document by ID
        print("\n3. Getting document by ID...")
        retrieved_doc = db.get_document("users", doc2_id)
        if retrieved_doc:
            print(f"   Retrieved: name={retrieved_doc.get('name')}, email={retrieved_doc.get('email')}")
            assert retrieved_doc.get("name") == "Jane Smith", "Document name mismatch"
        else:
            print("   ERROR: Document not found!")
            return False

        # Test 4: Query documents
        print("\n4. Querying documents...")
        results = db.query_documents("users", limit=10)
        print(f"   Found {len(results)} documents")
        for doc_id, doc in results:
            print(f"     - {doc_id}: {doc.get('name')}")

        # Test 5: Delete document
        print("\n5. Deleting document...")
        deleted = db.delete_document("users", doc1_id)
        print(f"   Deleted doc1: {deleted}")

        # Verify deletion
        deleted_doc = db.get_document("users", doc1_id)
        if deleted_doc is None:
            print("   Verified: Document no longer exists")
        else:
            print("   WARNING: Document still exists after deletion")

        # Test 6: List document collections
        print("\n6. Listing document collections...")
        collections = db.list_document_collections()
        print(f"   Collections: {collections}")

        # Cleanup
        db.flush()
        db.close()
        print("\nDocument Storage API tests PASSED!")
        return True

    except Exception as e:
        print(f"\nERROR: {e}")
        import traceback
        traceback.print_exc()
        return False
    finally:
        # Cleanup test directory
        shutil.rmtree(test_dir, ignore_errors=True)


def test_observability_api():
    """Test the Observability API (Logs and Metrics)."""
    print("\n" + "=" * 60)
    print("Testing Observability API")
    print("=" * 60)

    import proximadb

    # Create a temporary directory for the test
    test_dir = tempfile.mkdtemp(prefix="proximadb_obs_test_")
    print(f"Using test directory: {test_dir}")

    try:
        # Initialize the database
        db = proximadb.ProximaDB(test_dir)
        print("Database initialized")

        # Test 1: Create observability namespace
        print("\n1. Creating observability namespace...")
        db.create_observability_namespace("production", retention_days=30)
        print("   Created namespace 'production' with 30 day retention")

        # Test 2: Ingest logs
        print("\n2. Ingesting logs...")
        now_ns = int(time.time() * 1e9)
        logs = [
            {
                "timestamp_ns": now_ns - 60_000_000_000,  # 1 minute ago
                "severity": "INFO",
                "message": "Server started successfully",
                "source": "main.rs",
                "service": "api-gateway",
                "fields": {"version": "1.0.0", "env": "production"}
            },
            {
                "timestamp_ns": now_ns - 30_000_000_000,  # 30 seconds ago
                "severity": "WARN",
                "message": "High memory usage detected",
                "source": "monitor.rs",
                "service": "api-gateway",
                "fields": {"memory_pct": "85", "threshold": "80"}
            },
            {
                "timestamp_ns": now_ns,
                "severity": "ERROR",
                "message": "Database connection failed",
                "source": "db.rs",
                "service": "api-gateway",
                "fields": {"retry_count": "3", "error_code": "CONN_REFUSED"}
            }
        ]

        ingested = db.ingest_logs("production", logs)
        print(f"   Ingested {ingested} logs")

        # Test 3: Query logs
        print("\n3. Querying logs...")
        start_time = now_ns - 120_000_000_000  # 2 minutes ago
        end_time = now_ns + 1_000_000_000  # 1 second in future

        queried_logs = db.query_logs("production", start_time, end_time, limit=10)
        print(f"   Found {len(queried_logs)} logs")
        for log in queried_logs:
            print(f"     [{log.get('severity')}] {log.get('message')}")

        # Test 4: Ingest metrics
        print("\n4. Ingesting metrics...")
        samples = [
            {
                "metric_name": "http_request_duration_seconds",
                "timestamp_ns": now_ns - 60_000_000_000,
                "value": 0.125,
                "labels": {"endpoint": "/api/search", "method": "GET", "status": "200"}
            },
            {
                "metric_name": "http_request_duration_seconds",
                "timestamp_ns": now_ns - 30_000_000_000,
                "value": 0.089,
                "labels": {"endpoint": "/api/search", "method": "GET", "status": "200"}
            },
            {
                "metric_name": "cpu_usage_percent",
                "timestamp_ns": now_ns,
                "value": 65.5,
                "labels": {"host": "server-1", "core": "0"}
            },
            {
                "metric_name": "memory_usage_bytes",
                "timestamp_ns": now_ns,
                "value": 4294967296.0,  # 4GB
                "labels": {"host": "server-1"}
            }
        ]

        ingested_metrics = db.ingest_metrics("production", samples)
        print(f"   Ingested {ingested_metrics} metric samples")

        # Cleanup
        db.flush()
        db.close()
        print("\nObservability API tests PASSED!")
        return True

    except Exception as e:
        print(f"\nERROR: {e}")
        import traceback
        traceback.print_exc()
        return False
    finally:
        # Cleanup test directory
        shutil.rmtree(test_dir, ignore_errors=True)


def main():
    """Run all integration tests."""
    print("=" * 60)
    print("ProximaDB Embedded Document/Observability Integration Tests")
    print("=" * 60)

    results = []

    # Run document storage tests
    try:
        doc_result = test_document_storage_api()
        results.append(("Document Storage API", doc_result))
    except Exception as e:
        print(f"Document Storage API test failed with exception: {e}")
        results.append(("Document Storage API", False))

    # Run observability tests
    try:
        obs_result = test_observability_api()
        results.append(("Observability API", obs_result))
    except Exception as e:
        print(f"Observability API test failed with exception: {e}")
        results.append(("Observability API", False))

    # Print summary
    print("\n" + "=" * 60)
    print("Test Summary")
    print("=" * 60)

    all_passed = True
    for name, passed in results:
        status = "PASSED" if passed else "FAILED"
        print(f"  {name}: {status}")
        if not passed:
            all_passed = False

    if all_passed:
        print("\nAll tests PASSED!")
        return 0
    else:
        print("\nSome tests FAILED!")
        return 1


if __name__ == "__main__":
    sys.exit(main())
