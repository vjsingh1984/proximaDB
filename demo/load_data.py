#!/usr/bin/env python3
"""
Load pre-generated datasets into ProximaDB
This script is run during Docker container startup
"""

import json
import os
import sys
import time
from pathlib import Path

# Add path utilities
sys.path.insert(0, str(Path(__file__).parent))
from utils.path_utils import setup_demo_environment

# Setup environment
env_info = setup_demo_environment()
from proximadb import ProximaDBClient, Protocol
from proximadb import CollectionConfig, DistanceMetric, StorageEngine

# Configuration
PROXIMADB_REST_URL = os.getenv("PROXIMADB_URL", "http://localhost:5678")
PROXIMADB_GRPC_URL = os.getenv("PROXIMADB_GRPC_URL", "http://localhost:5679")
PRE_DIR = Path("/app/pre")

print("🚀 Loading pre-generated data into ProximaDB...")
print("=" * 60)

# Wait for ProximaDB to be ready
for i in range(30):
    try:
        client = ProximaDBClient(
            protocol=Protocol.GRPC,
            url=PROXIMADB_REST_URL,
            grpc_url=PROXIMADB_GRPC_URL
        )
        # Try to list collections to verify connection
        client.list_collections()
        print("✅ Connected to ProximaDB")
        break
    except Exception as e:
        if i < 29:
            print(f"⏳ Waiting for ProximaDB... ({i+1}/30)")
            time.sleep(2)
        else:
            print(f"❌ Failed to connect to ProximaDB: {e}")
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
        collection = client.create_collection(
            name="ecommerce_demo",
            config=CollectionConfig(
                name="ecommerce_demo",
                dimension=768,
                distance_metric=DistanceMetric.COSINE,
                storage_engine=StorageEngine.VIPER
            )
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
        collection = client.create_collection(
            name="sec_edgar_large_filings",
            config=CollectionConfig(
                name="sec_edgar_large_filings",
                dimension=768,
                distance_metric=DistanceMetric.COSINE,
                storage_engine=StorageEngine.VIPER
            )
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
        collection = client.create_collection(
            name="knowledge_base",
            config=CollectionConfig(
                name="knowledge_base",
                dimension=768,
                distance_metric=DistanceMetric.COSINE,
                storage_engine=StorageEngine.SST
            )
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

print("=" * 60)