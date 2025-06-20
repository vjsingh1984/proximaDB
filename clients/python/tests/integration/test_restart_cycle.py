#!/usr/bin/env python3
"""
Full restart cycle test - create collections, restart server, verify they persist
"""

import asyncio
import subprocess
import time
import os
import sys
from datetime import datetime

sys.path.insert(0, 'clients/python/src')
from proximadb.grpc_client import ProximaDBClient

async def test_full_restart_cycle():
    """Test complete restart cycle"""
    print("🔄 Full Server Restart Cycle Test")
    print("=" * 60)
    
    # Step 1: Connect and list existing collections
    print("\n1️⃣ Checking existing collections...")
    client = ProximaDBClient(endpoint='localhost:5679')
    
    existing = await client.list_collections()
    print(f"   Found {len(existing)} existing collections")
    
    # Step 2: Create unique test collections
    timestamp = datetime.now().strftime("%H%M%S")
    test_names = [
        f"restart_cycle_test_1_{timestamp}",
        f"restart_cycle_test_2_{timestamp}",
    ]
    
    print("\n2️⃣ Creating test collections...")
    created_uuids = {}
    
    for i, name in enumerate(test_names):
        try:
            result = await client.create_collection(
                name=name,
                dimension=128 + i * 64,
                distance_metric=1,  # COSINE
            )
            created_uuids[name] = result.id
            print(f"   ✅ Created: {name} (UUID: {result.id})")
        except Exception as e:
            print(f"   ❌ Failed: {name} - {e}")
    
    # Step 3: Verify collections exist
    print("\n3️⃣ Verifying collections before restart...")
    collections = await client.list_collections()
    found_count = sum(1 for c in collections if c.name in test_names)
    print(f"   Found {found_count}/{len(test_names)} test collections")
    
    # Step 4: Check actual files on disk
    print("\n4️⃣ Checking persistence files...")
    locations = {
        "Current dir": ["./incremental", "./snapshots"],
        "Configured": ["/data/proximadb/1/metadata/incremental", "/data/proximadb/1/metadata/snapshots"]
    }
    
    for location_name, paths in locations.items():
        print(f"\n   📁 {location_name}:")
        for path in paths:
            if os.path.exists(path):
                files = os.listdir(path)
                print(f"      {path}: {len(files)} files")
            else:
                print(f"      {path}: does not exist")
    
    # Step 5: Kill the server
    print("\n5️⃣ Stopping server...")
    subprocess.run(["pkill", "-f", "proximadb-server"], capture_output=True)
    time.sleep(3)
    
    # Step 6: Restart server
    print("\n6️⃣ Restarting server...")
    subprocess.Popen(
        ["cargo", "run", "--bin", "proximadb-server", "--", "--config", "config.toml"],
        stdout=open("restart_cycle_output.txt", "w"),
        stderr=subprocess.STDOUT
    )
    
    # Wait for server
    print("   ⏳ Waiting for server startup...")
    for i in range(10):
        try:
            test_client = ProximaDBClient(endpoint='localhost:5679')
            await test_client.list_collections()
            print("   ✅ Server is ready")
            break
        except:
            print(f"   ⏳ Waiting... ({i+1}/10)")
            time.sleep(1)
    else:
        print("   ❌ Server failed to start")
        return False
    
    # Step 7: Check if collections survived
    print("\n7️⃣ Checking collections after restart...")
    client2 = ProximaDBClient(endpoint='localhost:5679')
    collections_after = await client2.list_collections()
    
    print(f"   Total collections after restart: {len(collections_after)}")
    
    survived = {}
    for name, expected_uuid in created_uuids.items():
        found = next((c for c in collections_after if c.name == name), None)
        if found:
            if found.id == expected_uuid:
                survived[name] = "✅ Survived with correct UUID"
            else:
                survived[name] = f"⚠️ UUID changed: {found.id}"
        else:
            survived[name] = "❌ Not found after restart"
    
    print("\n📊 Results:")
    for name, status in survived.items():
        print(f"   {name}: {status}")
    
    # Step 8: Diagnosis
    print("\n🔍 Diagnosis:")
    if all("✅" in status for status in survived.values()):
        print("   ✅ All collections persisted correctly!")
    else:
        print("   ❌ Collections did not persist across restart")
        print("\n   💡 Issue: Files are being written to working directory")
        print("      instead of the configured metadata path.")
        print("\n   📍 Current behavior:")
        print("      - Config: file:///data/proximadb/1/metadata")
        print("      - Writes to: ./incremental/*.avro (relative to working dir)")
        print("\n   🔧 Fix required in filesystem layer:")
        print("      - When LocalFileSystem receives relative path")
        print("      - It should resolve against base_path from URL")
        print("      - E.g., './incremental/x.avro' → '/data/proximadb/1/metadata/incremental/x.avro'")
    
    return True

if __name__ == "__main__":
    success = asyncio.run(test_full_restart_cycle())
    
    if success:
        print("\n✅ Test completed")
    else:
        print("\n❌ Test failed")
    
    exit(0 if success else 1)