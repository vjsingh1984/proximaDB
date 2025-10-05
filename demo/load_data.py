#!/usr/bin/env python3
"""
Load pre-generated datasets into ProximaDB
This script is run during Docker container startup
Updated for new gRPC/REST API
"""

import json
import os
import sys
import time
from pathlib import Path

# Add SDK to Python path
sdk_path = str(Path(__file__).parent.parent / "clients" / "python" / "src")
if sdk_path not in sys.path:
    sys.path.insert(0, sdk_path)

try:
    from proximadb.protocols.grpc_sync import ProximaDBSyncGrpcClient
except ImportError as e:
    print(f"❌ Failed to import ProximaDB client: {e}")
    print("Please ensure the Python SDK is installed")
    sys.exit(1)

# Configuration
PROXIMADB_GRPC_URL = os.getenv("PROXIMADB_GRPC_URL", "localhost:5679")
PRE_DIR = Path("/app/pre") if Path("/app/pre").exists() else Path("./demo/pre")

print("🚀 Loading pre-generated data into ProximaDB...")
print("=" * 60)

# Wait for ProximaDB to be ready
client = None
for i in range(30):
    try:
        client = ProximaDBSyncGrpcClient(
            PROXIMADB_GRPC_URL,
            enable_compression=False
        )
        # Try a simple operation to verify connection
        # Since list_collections might not be implemented, just check connection
        print("✅ Connected to ProximaDB")
        break
    except Exception as e:
        if i < 29:
            print(f"⏳ Waiting for ProximaDB... ({i+1}/30)")
            time.sleep(2)
        else:
            print(f"❌ Failed to connect to ProximaDB: {e}")
            sys.exit(1)

if client is None:
    print("❌ Failed to initialize client")
    sys.exit(1)

# Check if data already exists
marker_file = Path("/data/.demo_data_loaded")
if marker_file.exists():
    print("✅ Demo data already loaded, skipping...")
    sys.exit(0)

# Load e-commerce data
ecommerce_file = PRE_DIR / "ecommerce_data.json"
if ecommerce_file.exists():
    print("\n📦 Loading e-commerce data...")
    with open(ecommerce_file) as f:
        ecommerce_data = json.load(f)
    
    # Create collection
    try:
        client.create_collection(
            name="ecommerce_demo",
            dimension=768,
            distance_metric=1,  # 1 = cosine
            storage_engine=0    # 0 = auto-select
        )
        print("✅ Created ecommerce_demo collection")
    except Exception as e:
        print(f"⚠️ Collection might already exist: {e}")
    
    # Insert vectors in batches
    batch_size = 100
    for i in range(0, len(ecommerce_data), batch_size):
        batch = ecommerce_data[i:i + batch_size]
        vectors = []
        for item in batch:
            vectors.append({
                "id": item["id"],
                "vector": item["vector"],
                "metadata": {k: v for k, v in item.items() if k not in ["id", "vector"]}
            })
        
        try:
            client.insert_vectors("ecommerce_demo", vectors)
            print(f"  Inserted batch {i//batch_size + 1}/{(len(ecommerce_data) + batch_size - 1)//batch_size}")
        except Exception as e:
            print(f"  ❌ Failed to insert batch: {e}")
    
    print(f"✅ Loaded {len(ecommerce_data)} e-commerce products")

# Load SEC EDGAR data
sec_file = PRE_DIR / "sec_edgar_data.json"
if sec_file.exists():
    print("\n📄 Loading SEC EDGAR data...")
    with open(sec_file) as f:
        sec_data = json.load(f)
    
    # Create collection
    try:
        client.create_collection(
            name="sec_edgar_large_filings",
            dimension=768,
            distance_metric=1,  # 1 = cosine
            storage_engine=0    # 0 = auto-select
        )
        print("✅ Created sec_edgar_large_filings collection")
    except Exception as e:
        print(f"⚠️ Collection might already exist: {e}")
    
    # Insert vectors in batches
    batch_size = 100
    for i in range(0, len(sec_data), batch_size):
        batch = sec_data[i:i + batch_size]
        vectors = []
        for item in batch:
            vectors.append({
                "id": item["id"],
                "vector": item["vector"],
                "metadata": {k: v for k, v in item.items() if k not in ["id", "vector"]}
            })
        
        try:
            client.insert_vectors("sec_edgar_large_filings", vectors)
            if i % 1000 == 0:
                print(f"  Inserted {i + len(batch)}/{len(sec_data)} chunks")
        except Exception as e:
            print(f"  ❌ Failed to insert batch: {e}")
    
    print(f"✅ Loaded {len(sec_data)} SEC EDGAR chunks")

# Load knowledge base data
kb_file = PRE_DIR / "knowledge_base_data.json"
if kb_file.exists():
    print("\n📚 Loading knowledge base data...")
    with open(kb_file) as f:
        kb_data = json.load(f)
    
    # Create collection
    try:
        client.create_collection(
            name="knowledge_base",
            dimension=768,
            distance_metric=1,  # 1 = cosine
            storage_engine=0    # 0 = auto-select
        )
        print("✅ Created knowledge_base collection")
    except Exception as e:
        print(f"⚠️ Collection might already exist: {e}")
    
    # Insert all vectors
    vectors = []
    for item in kb_data:
        vectors.append({
            "id": item["id"],
            "vector": item["vector"],
            "metadata": {k: v for k, v in item.items() if k not in ["id", "vector"]}
        })
    
    try:
        client.insert_vectors("knowledge_base", vectors)
        print(f"✅ Loaded {len(kb_data)} knowledge base chunks")
    except Exception as e:
        print(f"❌ Failed to insert knowledge base data: {e}")

# Create marker file
try:
    marker_file.parent.mkdir(parents=True, exist_ok=True)
    marker_file.touch()
    print("\n✅ All demo data loaded successfully!")
except:
    print("⚠️ Could not create marker file, data might be reloaded on restart")

# Close client connection
client.close()

print("=" * 60)