#!/usr/bin/env python3
"""
ProximaDB Basic SQL Demo - Demonstrates currently working SQL features
"""

import time
import numpy as np
from proximadb import connect_rest, connect_grpc
from proximadb.models import CollectionConfig, DistanceMetric, VectorRecord, StorageEngine

def main():
    print("🚀 ProximaDB Basic SQL Demo - Currently Supported Features")
    print("=" * 60)
    
    # Connect via REST for SQL queries
    print("\n📡 Connecting via REST API for SQL queries...")
    rest_client = connect_rest(url="http://localhost:5678")
    
    # Also connect via gRPC for data setup
    print("📡 Connecting via gRPC for data operations...")
    grpc_client = connect_grpc(url="http://localhost:5679")
    
    # Create test collection
    collection_name = "sql_basic_demo"
    dimension = 128
    
    print(f"\n📦 Creating collection '{collection_name}'...")
    
    try:
        collection = grpc_client.create_collection(
            name=collection_name,
            dimension=dimension,
            distance_metric=DistanceMetric.COSINE,
            storage_engine=StorageEngine.VIPER,
            filterable_metadata_fields=["category", "brand", "status"]
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
            "name": f"Product {i}"
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
    
    # Demo 1: Basic vector search (no filter)
    print("\n📊 1. Basic Vector Search (No Filter)")
    print("-" * 40)
    
    query_vector = np.random.rand(dimension).astype(np.float32).tolist()
    vector_str = "[" + ", ".join(str(v) for v in query_vector) + "]"
    
    sql = f"""
    SELECT id, metadata
    FROM {collection_name}
    ORDER BY VECTOR_SIMILARITY(vector, {vector_str}, 'cosine')
    LIMIT 5
    """
    
    start_time = time.time()
    result = rest_client.execute_sql(sql)
    elapsed = (time.time() - start_time) * 1000
    
    print(f"Query time: {elapsed:.2f}ms")
    print(f"Found {result['row_count']} results:")
    for row in result['rows']:
        metadata = row.get('metadata', {})
        print(f"  - {row['id']}: {metadata.get('name')} ({metadata.get('category')})")
    
    # Demo 2: Simple equality filter
    print("\n📊 2. Equality Filter (metadata->>'field' = 'value')")
    print("-" * 40)
    
    sql_filtered = f"""
    SELECT id, metadata
    FROM {collection_name}
    WHERE metadata->>'category' = 'electronics'
    ORDER BY VECTOR_SIMILARITY(vector, {vector_str}, 'cosine')
    LIMIT 5
    """
    
    start_time = time.time()
    result = rest_client.execute_sql(sql_filtered)
    elapsed = (time.time() - start_time) * 1000
    
    print(f"Query time: {elapsed:.2f}ms")
    print(f"Found {result['row_count']} electronics items:")
    for row in result['rows']:
        metadata = row.get('metadata', {})
        print(f"  - {row['id']}: {metadata.get('name')} (Brand: {metadata.get('brand')})")
    
    # Demo 3: IN operator
    print("\n📊 3. IN Operator for Multiple Values")
    print("-" * 40)
    
    sql_in = f"""
    SELECT id, metadata
    FROM {collection_name}
    WHERE metadata->>'brand' IN ('BrandA', 'BrandC')
    ORDER BY VECTOR_SIMILARITY(vector, {vector_str}, 'cosine')
    LIMIT 5
    """
    
    start_time = time.time()
    result = rest_client.execute_sql(sql_in)
    elapsed = (time.time() - start_time) * 1000
    
    print(f"Query time: {elapsed:.2f}ms")
    print(f"Found {result['row_count']} items from BrandA or BrandC:")
    for row in result['rows']:
        metadata = row.get('metadata', {})
        print(f"  - {row['id']}: {metadata.get('brand')} - {metadata.get('name')}")
    
    # Demo 4: Different distance metrics
    print("\n📊 4. Different Distance Metrics")
    print("-" * 40)
    
    metrics = ['cosine', 'euclidean', 'dot']
    for metric in metrics:
        sql_metric = f"""
        SELECT id, metadata
        FROM {collection_name}
        WHERE metadata->>'status' = 'featured'
        ORDER BY VECTOR_SIMILARITY(vector, {vector_str}, '{metric}')
        LIMIT 3
        """
        
        try:
            start_time = time.time()
            result = rest_client.execute_sql(sql_metric)
            elapsed = (time.time() - start_time) * 1000
            print(f"\n{metric.upper()} distance ({elapsed:.2f}ms):")
            for row in result['rows'][:2]:
                metadata = row.get('metadata', {})
                print(f"  - {metadata.get('name')} ({metadata.get('category')})")
        except Exception as e:
            print(f"\n{metric.upper()} distance: Error - {e}")
    
    # Demo 5: Query caching effect
    print("\n📊 5. Query Caching Performance")
    print("-" * 40)
    
    # Run same query multiple times
    times = []
    for i in range(5):
        start_time = time.time()
        result = rest_client.execute_sql(sql_filtered)
        elapsed = (time.time() - start_time) * 1000
        times.append(elapsed)
    
    print(f"First run: {times[0]:.2f}ms")
    print(f"Subsequent runs: {[f'{t:.2f}ms' for t in times[1:]]}")
    print(f"Average cached: {sum(times[1:])/len(times[1:]):.2f}ms")
    print(f"Cache speedup: {times[0]/np.mean(times[1:]):.1f}x")
    
    # Demo 6: Show what doesn't work
    print("\n❌ Currently Unsupported Operations")
    print("-" * 40)
    
    unsupported_queries = [
        ("AND operator", f"""
            SELECT id FROM {collection_name}
            WHERE metadata->>'category' = 'electronics' 
              AND metadata->>'status' = 'active'
            LIMIT 5
        """),
        ("OR operator", f"""
            SELECT id FROM {collection_name}
            WHERE metadata->>'category' = 'electronics'
               OR metadata->>'category' = 'books'
            LIMIT 5
        """),
        ("LIKE operator", f"""
            SELECT id FROM {collection_name}
            WHERE metadata->>'name' LIKE '%Product 1%'
            LIMIT 5
        """),
        ("Comparison operators", f"""
            SELECT id FROM {collection_name}
            WHERE metadata->>'item_id' > 'item_0100'
            LIMIT 5
        """)
    ]
    
    for name, query in unsupported_queries:
        try:
            result = rest_client.execute_sql(query)
            print(f"✅ {name}: Unexpectedly worked!")
        except Exception as e:
            error_msg = str(e)
            if "Complex conditions not supported" in error_msg:
                print(f"❌ {name}: Complex conditions not supported yet")
            elif "Operator" in error_msg and "not supported" in error_msg:
                print(f"❌ {name}: Operator not supported")
            else:
                print(f"❌ {name}: {error_msg}")
    
    print("\n📊 Summary of SQL Support")
    print("=" * 60)
    print("✅ Currently Working:")
    print("  - Basic SELECT with field selection")
    print("  - WHERE with simple equality (=)")
    print("  - IN operator for value lists")
    print("  - metadata->>'field' JSON extraction")
    print("  - ORDER BY VECTOR_SIMILARITY()")
    print("  - LIMIT and OFFSET")
    print("  - Query plan caching (2-5x speedup)")
    
    print("\n❌ Not Yet Implemented:")
    print("  - AND, OR, NOT logical operators")
    print("  - Comparison operators (!=, <, >, <=, >=)")
    print("  - LIKE pattern matching")
    print("  - IS NULL, IS NOT NULL")
    print("  - Subqueries, UNION, JOIN")
    print("  - Aggregations (COUNT, AVG, etc.)")
    
    print(f"\n✅ Demo complete! Collection '{collection_name}' retained for testing.")

if __name__ == "__main__":
    main()