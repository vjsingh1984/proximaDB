#!/usr/bin/env python3
import time
import random

from proximadb_sdk import connect_rest

base_url = "http://localhost:5678"
client = connect_rest(base_url)

print("=== Testing Vector Persistence with REST SDK ===")
print()

# 1. Use the recovery_test_collection that already exists
collection_name = "recovery_test_collection"
print(f"1. Using existing collection '{collection_name}'")

# 2. Insert records using the OpenAPI v2 REST SDK surface
print("\n2. Inserting records via REST SDK:")
records = []
for i in range(5):
    records.append(
        {
            "id": f"persist_test_{i}_{int(time.time())}",
            "vector": [random.random() for _ in range(128)],
            "props": {"test": "persistence", "index": i, "timestamp": time.time()},
        }
    )

result = client.insert_records(collection_name, records)
print(f"   Insert success: {result.success}")
print(f"   Inserted: {result.success}/{result.total}")

print()

# 3. Try different vector endpoints to verify insertion
print("3. Checking vector operations:")

# Try search
print("   a) OpenAPI v2 search endpoint via SDK:")
search = client.search_envelope(collection_name, [0.5] * 128, top_k=10)
print(f"      Results: {len(search.items)}")

# Try the query facade
print("   b) OpenAPI v2 UQL query facade via SDK:")
query_result = client.execute_uql(
    f"SEARCH {collection_name} VECTOR SEARCH embedding NEAR {[0.5] * 4} TOP 1 RETURN id",
    collection=collection_name,
    limit=1,
)
print(f"      Query keys: {sorted(query_result.keys())}")

print()

# 4. Check collection stats again
print("4. Checking collection stats after insert:")
col = client.get_collection(collection_name)
print("   Collection response received")
print(f"     - Name: {getattr(col, 'name', collection_name)}")

print()

# 5. Check WAL directory
print("5. Checking WAL files:")
import os

wal_dir = "./lsm_wal"
if os.path.exists(wal_dir):
    files = os.listdir(wal_dir)
    print(f"   WAL directory contains {len(files)} files")
    for f in files[:5]:
        print(f"   - {f}")
else:
    print(f"   WAL directory not found at {wal_dir}")

print()
print("=== Test Complete ===")
print("Vectors have been inserted. Now you can:")
print("1. Stop the server (kill the process)")
print("2. Restart the server")
print("3. Run this script again to see if vectors persist")
client.close()
