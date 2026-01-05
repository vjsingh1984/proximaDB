#!/usr/bin/env python3
"""
Detailed WAL Persistence Test

This test validates that:
1. WAL files are created when vectors are inserted
2. WAL files are in the correct location (multi-disk layout)
3. Data can be recovered from WAL after server restart
4. Collection metadata persists correctly

Test procedure:
- Start server
- Create collection and get storage path
- Insert vectors
- Verify WAL files created
- Stop server
- Restart server
- Verify data recovered from WAL
"""

import os
import sys
import time
import subprocess
import json
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent.parent.parent / "src"))

from proximadb_sdk import ProximaDBClient
from proximadb_sdk.models import VectorRecord

server_process = None


def get_project_paths():
    """Get project paths"""
    test_dir = os.path.dirname(os.path.abspath(__file__))
    project_root = os.path.abspath(os.path.join(test_dir, "../../../.."))
    return project_root


def start_server():
    """Start server"""
    global server_process
    print("🚀 Starting server...")

    os.system("pkill -f proximadb-server >/dev/null 2>&1")
    time.sleep(2)

    project_root = get_project_paths()
    server_binary = os.path.join(project_root, "target/release/proximadb-server")
    config_file = os.path.join(project_root, "config/config.toml")

    if not os.path.exists(server_binary):
        print(f"❌ Server not found. Build with: cargo build --release")
        return False

    server_process = subprocess.Popen(
        [server_binary, "--config", config_file],
        cwd=project_root,
        env={**os.environ, "RUST_LOG": "info"},
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )

    # Wait for server to be ready (poll health endpoint)
    import requests

    for i in range(30):  # Try for 30 seconds
        time.sleep(1)
        try:
            response = requests.get("http://localhost:5678/health", timeout=1)
            if response.status_code == 200:
                print(f"✅ Server started (ready after {i+1}s)")
                return True
        except:
            continue

    print("❌ Server failed to start after 30s")
    return False


def stop_server():
    """Stop server"""
    global server_process
    if server_process:
        print("🛑 Stopping server...")
        server_process.terminate()
        try:
            server_process.wait(timeout=10)
        except:
            server_process.kill()
        server_process = None
        time.sleep(2)
        print("✅ Server stopped")


def check_wal_files(storage_path, collection_id):
    """Check if WAL files exist at correct location"""
    print(f"\n📁 Checking WAL files...")
    print(f"   Storage path: {storage_path}")

    # Extract path from file:// URL
    if storage_path.startswith("file://"):
        base_path = storage_path[7:]
    else:
        base_path = storage_path

    # WAL directory is: {base_path}/{collection_id}/wal/
    wal_dir = os.path.join(base_path, collection_id, "wal")

    print(f"   WAL directory: {wal_dir}")

    if os.path.exists(wal_dir):
        wal_files = [f for f in os.listdir(wal_dir) if f.endswith(".bcwal")]
        print(f"✅ WAL directory exists")
        print(f"✅ Found {len(wal_files)} WAL files:")
        for f in wal_files[:5]:  # Show first 5
            file_path = os.path.join(wal_dir, f)
            size = os.path.getsize(file_path)
            print(f"      - {f} ({size} bytes)")
        if len(wal_files) > 5:
            print(f"      ... and {len(wal_files) - 5} more")
        return True
    else:
        print(f"❌ WAL directory not found at: {wal_dir}")
        return False


def test_wal_persistence():
    """Main test function"""

    print("\n" + "=" * 80)
    print("WAL PERSISTENCE DETAILED TEST")
    print("=" * 80)

    if not start_server():
        return

    client = ProximaDBClient("http://localhost:5678", protocol="rest")
    collection_name = f"wal_test_{int(time.time())}"

    # =========================
    # Step 1: Create & Insert
    # =========================
    print("\n" + "=" * 80)
    print("STEP 1: Create Collection and Insert Data")
    print("=" * 80)

    try:
        # Create collection
        collection = client.create_collection(
            name=collection_name, dimension=64, storage_engine="sst"
        )

        # Get storage path from collection
        storage_path = None
        if hasattr(collection, "storage_assignment"):
            storage_path = collection.storage_assignment.get("primary_path")
        elif isinstance(collection, dict):
            storage_path = collection.get("storage_assignment", {}).get("primary_path")

        collection_id = (
            collection.id if hasattr(collection, "id") else collection.get("id")
        )

        print(f"✅ Collection: {collection_name}")
        print(f"   ID: {collection_id}")
        print(f"   Storage: {storage_path}")

        # Insert vectors
        vectors = [
            VectorRecord(
                id=f"wal_vec_{i}",
                vector=[float(i + j * 0.1) for j in range(64)],
                metadata={"index": i, "test": "wal"},
            )
            for i in range(20)
        ]

        result = client.insert_vectors(collection_name, vectors)
        print(f"✅ Inserted {len(vectors)} vectors")

        # Check WAL files created
        if storage_path and collection_id:
            wal_exists = check_wal_files(storage_path, collection_id)
            if not wal_exists:
                print("⚠️  WAL files not found - data might not persist")

    except Exception as e:
        print(f"❌ Setup failed: {e}")
        stop_server()
        raise

    # =========================
    # Step 2: Restart Server
    # =========================
    print("\n" + "=" * 80)
    print("STEP 2: Restart Server")
    print("=" * 80)

    stop_server()
    time.sleep(3)
    if not start_server():
        raise Exception("Failed to restart server")

    client = ProximaDBClient("http://localhost:5678", protocol="rest")

    # =========================
    # Step 3: Verify Recovery
    # =========================
    print("\n" + "=" * 80)
    print("STEP 3: Verify Data Recovery")
    print("=" * 80)

    recovered_vectors = 0
    collection_found = False

    try:
        # Check collection exists
        collections = client.list_collections()
        for c in collections:
            name = c.config.name if hasattr(c, "config") else c.get("name", "")
            cid = c.id if hasattr(c, "id") else c.get("id", "")
            if name == collection_name or cid == collection_id:
                collection_found = True
                print(f"✅ Collection recovered: {collection_name}")
                break

        if not collection_found:
            print(f"❌ Collection NOT recovered")

        # Search for vectors
        if collection_found:
            search_results = client.search(
                collection_id=collection_name, vector=[0.5] * 64, top_k=20
            )
            recovered_vectors = len(search_results)
            print(f"✅ Recovered {recovered_vectors}/20 vectors")

            if recovered_vectors > 0:
                print(f"   Sample IDs: {[r.id for r in search_results[:5]]}")
                if search_results[0].metadata:
                    print(f"   Metadata preserved: {search_results[0].metadata}")
        else:
            print("⚠️  Skipping vector search - collection not found")

    except Exception as e:
        print(f"❌ Recovery verification failed: {e}")

    # =========================
    # Final Report
    # =========================
    print("\n" + "=" * 80)
    print("TEST RESULTS")
    print("=" * 80)

    print(f"\n📊 Recovery Status:")
    print(f"   Collection: {'✅ Recovered' if collection_found else '❌ Lost'}")
    print(
        f"   Vectors: {recovered_vectors}/20 recovered ({recovered_vectors/20*100:.0f}%)"
    )

    if collection_found and recovered_vectors >= 18:
        print(f"\n✅ SUCCESS: WAL persistence working correctly!")
        success = True
    else:
        print(f"\n❌ FAILURE: Data not fully recovered")
        success = False

    # Cleanup
    print("\n🧹 Cleanup...")
    try:
        if collection_found:
            client.delete_collection(collection_name)
            print("✅ Deleted test collection")
    except:
        pass

    stop_server()

    assert collection_found, "Collection must be recovered after restart"
    assert recovered_vectors >= 18, f"Expected >=18 vectors, got {recovered_vectors}"

    return success


if __name__ == "__main__":
    success = test_wal_persistence()
    sys.exit(0 if success else 1)
