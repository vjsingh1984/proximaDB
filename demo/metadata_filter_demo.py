#!/usr/bin/env python3
"""
ProximaDB Metadata Filter Demo - Tests AND/OR operators in REST and gRPC APIs
"""

import time
import numpy as np
from proximadb import connect_rest, connect_grpc
from proximadb.models import CollectionConfig, DistanceMetric, VectorRecord, StorageEngine

def test_metadata_filters(client, collection_name, protocol="gRPC"):
    """Test various metadata filter combinations"""
    print(f"\n🔍 Testing Metadata Filters via {protocol}")
    print("=" * 60)
    
    # Generate query vector
    query_vector = np.random.rand(128).astype(np.float32).tolist()
    
    # Test 1: Simple equality filter
    print("\n📌 1. Simple Equality Filter")
    print("-" * 40)
    
    try:
        start_time = time.time()
        results = client.search(
            collection_id=collection_name,
            vector=query_vector,
            top_k=5,
            metadata_filter={"category": "electronics"},
            include_metadata=True
        )
        elapsed = (time.time() - start_time) * 1000
        
        print(f"✅ Filter: category = 'electronics'")
        print(f"   Time: {elapsed:.2f}ms")
        print(f"   Found: {len(results)} results")
        for r in results[:3]:
            print(f"   - {r.id}: {r.metadata.get('name')} ({r.metadata.get('brand')})")
    except Exception as e:
        print(f"❌ Error: {e}")
    
    # Test 2: AND operator (multiple conditions)
    print("\n📌 2. AND Operator Test")
    print("-" * 40)
    
    try:
        # Try different filter formats to see what works
        filter_formats = [
            # Format 1: Nested dict (might work)
            {
                "operator": "AND",
                "conditions": [
                    {"field": "category", "value": "electronics"},
                    {"field": "status", "value": "active"}
                ]
            },
            # Format 2: Simple dict (implied AND)
            {
                "category": "electronics",
                "status": "active"
            },
            # Format 3: Protobuf-style structure
            {
                "$and": [
                    {"category": "electronics"},
                    {"status": "active"}
                ]
            }
        ]
        
        for i, filter_dict in enumerate(filter_formats):
            try:
                print(f"\n   Trying format {i+1}: {filter_dict}")
                start_time = time.time()
                results = client.search(
                    collection_id=collection_name,
                    vector=query_vector,
                    top_k=5,
                    metadata_filter=filter_dict,
                    include_metadata=True
                )
                elapsed = (time.time() - start_time) * 1000
                
                print(f"   ✅ Success! Time: {elapsed:.2f}ms, Found: {len(results)} results")
                for r in results[:2]:
                    print(f"      - {r.id}: {r.metadata}")
                break
            except Exception as e:
                print(f"   ❌ Failed: {str(e)[:100]}")
    except Exception as e:
        print(f"❌ AND operator test failed: {e}")
    
    # Test 3: OR operator
    print("\n📌 3. OR Operator Test")
    print("-" * 40)
    
    try:
        filter_formats = [
            # Format 1: Protobuf-style
            {
                "$or": [
                    {"category": "electronics"},
                    {"category": "books"}
                ]
            },
            # Format 2: Operator field
            {
                "operator": "OR",
                "conditions": [
                    {"field": "category", "value": "electronics"},
                    {"field": "category", "value": "books"}
                ]
            }
        ]
        
        for i, filter_dict in enumerate(filter_formats):
            try:
                print(f"\n   Trying format {i+1}: {filter_dict}")
                start_time = time.time()
                results = client.search(
                    collection_id=collection_name,
                    vector=query_vector,
                    top_k=5,
                    metadata_filter=filter_dict,
                    include_metadata=True
                )
                elapsed = (time.time() - start_time) * 1000
                
                print(f"   ✅ Success! Time: {elapsed:.2f}ms, Found: {len(results)} results")
                categories = set(r.metadata.get('category') for r in results[:5])
                print(f"      Categories found: {categories}")
                break
            except Exception as e:
                print(f"   ❌ Failed: {str(e)[:100]}")
    except Exception as e:
        print(f"❌ OR operator test failed: {e}")
    
    # Test 4: IN operator
    print("\n📌 4. IN Operator Test")
    print("-" * 40)
    
    try:
        filter_formats = [
            # Format 1: List value
            {"brand": ["BrandA", "BrandC"]},
            # Format 2: Explicit IN
            {"brand": {"$in": ["BrandA", "BrandC"]}},
            # Format 3: Operator style
            {
                "field": "brand",
                "operation": "IN",
                "value": ["BrandA", "BrandC"]
            }
        ]
        
        for i, filter_dict in enumerate(filter_formats):
            try:
                print(f"\n   Trying format {i+1}: {filter_dict}")
                start_time = time.time()
                results = client.search(
                    collection_id=collection_name,
                    vector=query_vector,
                    top_k=5,
                    metadata_filter=filter_dict,
                    include_metadata=True
                )
                elapsed = (time.time() - start_time) * 1000
                
                print(f"   ✅ Success! Time: {elapsed:.2f}ms, Found: {len(results)} results")
                brands = set(r.metadata.get('brand') for r in results[:5])
                print(f"      Brands found: {brands}")
                break
            except Exception as e:
                print(f"   ❌ Failed: {str(e)[:100]}")
    except Exception as e:
        print(f"❌ IN operator test failed: {e}")
    
    # Test 5: Comparison operators
    print("\n📌 5. Comparison Operators Test")
    print("-" * 40)
    
    try:
        filter_formats = [
            # Format 1: Direct comparison
            {"item_id": {"$gt": "item_0250"}},
            # Format 2: Range query
            {"item_id": {"$gte": "item_0200", "$lt": "item_0300"}}
        ]
        
        for i, filter_dict in enumerate(filter_formats):
            try:
                print(f"\n   Trying format {i+1}: {filter_dict}")
                start_time = time.time()
                results = client.search(
                    collection_id=collection_name,
                    vector=query_vector,
                    top_k=5,
                    metadata_filter=filter_dict,
                    include_metadata=True
                )
                elapsed = (time.time() - start_time) * 1000
                
                print(f"   ✅ Success! Time: {elapsed:.2f}ms, Found: {len(results)} results")
                for r in results[:3]:
                    print(f"      - {r.id}: item_id={r.metadata.get('item_id')}")
                break
            except Exception as e:
                print(f"   ❌ Failed: {str(e)[:100]}")
    except Exception as e:
        print(f"❌ Comparison operators test failed: {e}")

def main():
    print("🚀 ProximaDB Metadata Filter Demo - AND/OR Support Test")
    print("=" * 60)
    
    # Connect via both protocols
    print("\n📡 Connecting to ProximaDB...")
    grpc_client = connect_grpc(url="http://localhost:5679")
    rest_client = connect_rest(url="http://localhost:5678")
    
    # Create test collection
    collection_name = "metadata_filter_demo"
    dimension = 128
    
    print(f"\n📦 Creating collection '{collection_name}'...")
    
    try:
        collection = grpc_client.create_collection(
            name=collection_name,
            dimension=dimension,
            distance_metric=DistanceMetric.COSINE,
            storage_engine=StorageEngine.VIPER,
            filterable_metadata_fields=["category", "brand", "status", "item_id"]
        )
        print("✅ Collection created")
    except Exception as e:
        if "already exists" in str(e):
            print("📁 Using existing collection")
        else:
            raise
    
    # Insert test data
    print("\n📝 Inserting test data...")
    categories = ["electronics", "books", "clothing", "home", "sports"]
    brands = ["BrandA", "BrandB", "BrandC", "BrandD", "BrandE"]
    statuses = ["active", "inactive", "featured"]
    
    records = []
    for i in range(500):
        vector = np.random.rand(dimension).astype(np.float32).tolist()
        metadata = {
            "category": categories[i % len(categories)],
            "brand": brands[i % len(brands)],
            "status": statuses[i % len(statuses)],
            "item_id": f"item_{i:04d}",
            "name": f"Product {i}",
            "price": float(np.random.randint(10, 1000))
        }
        
        record = VectorRecord(
            id=f"vec_{i:04d}",
            vector=vector,
            metadata=metadata
        )
        records.append(record)
    
    # Insert in batches
    batch_size = 100
    for i in range(0, len(records), batch_size):
        batch = records[i:i+batch_size]
        grpc_client.upsert_vectors(
            collection_id=collection_name,
            records=batch
        )
    print(f"✅ Inserted {len(records)} vectors")
    
    # Wait for indexing
    print("\n⏳ Waiting for indexing...")
    time.sleep(2)
    
    # Test metadata filters on both protocols
    test_metadata_filters(grpc_client, collection_name, "gRPC")
    test_metadata_filters(rest_client, collection_name, "REST")
    
    # Summary
    print("\n📊 Summary")
    print("=" * 60)
    print("Based on the proto definition, ProximaDB should support:")
    print("✅ AND, OR, NOT operators for combining conditions")
    print("✅ EQUALS, NOT_EQUALS, GT, LT, GTE, LTE comparison operators")
    print("✅ IN, NOT_IN for set membership")
    print("✅ CONTAINS, STARTS_WITH, ENDS_WITH for string matching")
    print("\nHowever, the actual implementation may vary.")
    print("Check the test results above to see what's currently working.")
    
    print(f"\n✅ Demo complete! Collection '{collection_name}' retained for testing.")

if __name__ == "__main__":
    main()