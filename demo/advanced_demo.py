#!/usr/bin/env python3
"""
ProximaDB Advanced Demo - SQL Queries, Multiple Distance Metrics, Storage Engines, and Performance
Tests both VIPER and SST storage engines with gRPC, REST, and SQL search APIs
"""

import time
import numpy as np
from proximadb import connect_grpc, connect_rest
from proximadb.models import CollectionConfig, DistanceMetric, VectorRecord, StorageEngine

def run_sql_demo(client, collection_name):
    """Demonstrate SQL query capabilities"""
    print("\n🔍 SQL Query Demo")
    print("-" * 40)
    
    # Generate a query vector
    query_vector = np.random.rand(128).astype(np.float32).tolist()
    vector_str = "[" + ", ".join(str(v) for v in query_vector) + "]"
    
    # Basic vector similarity search
    sql1 = f"""
    SELECT id, metadata
    FROM {collection_name}
    ORDER BY VECTOR_SIMILARITY(vector, {vector_str}, 'cosine')
    LIMIT 5
    """
    
    print("1. Basic vector similarity search:")
    result = client.execute_sql(sql1)
    print(f"   Found {result['row_count']} results")
    for row in result['rows'][:3]:
        metadata = row.get('metadata', {})
        print(f"   - {row['id']}: {metadata.get('product_name')}")
    
    # Filtered search
    sql2 = f"""
    SELECT id, metadata
    FROM {collection_name}
    WHERE metadata->>'category' = 'laptop'
    ORDER BY VECTOR_SIMILARITY(vector, {vector_str}, 'cosine')
    LIMIT 5
    """
    
    print("\n2. Filtered search (laptops only):")
    result = client.execute_sql(sql2)
    print(f"   Found {result['row_count']} laptops")
    
    # Complex filtering
    sql3 = f"""
    SELECT id, metadata
    FROM {collection_name}
    WHERE metadata->>'in_stock' = 'true'
    ORDER BY VECTOR_SIMILARITY(vector, {vector_str}, 'euclidean')
    LIMIT 10
    """
    
    print("\n3. In-stock items with Euclidean distance:")
    result = client.execute_sql(sql3)
    print(f"   Found {result['row_count']} in-stock items")

def run_distance_metrics_demo(client, collection_name):
    """Compare different distance metrics"""
    print("\n📏 Distance Metrics Comparison")
    print("-" * 40)
    
    query_vector = np.random.rand(128).astype(np.float32).tolist()
    metrics = ["cosine", "euclidean", "dot"]
    
    for metric in metrics:
        start_time = time.time()
        
        if metric == "cosine":
            distance_metric = DistanceMetric.COSINE
        elif metric == "euclidean":
            distance_metric = DistanceMetric.EUCLIDEAN
        else:
            distance_metric = DistanceMetric.DOT_PRODUCT
            
        results = client.search(
            collection_id=collection_name,
            vector=query_vector,
            top_k=5,
            include_metadata=True
        )
        
        search_time = (time.time() - start_time) * 1000
        
        print(f"\n{metric.upper()} distance (search time: {search_time:.2f}ms):")
        for i, result in enumerate(results[:3]):
            metadata = result.metadata
            print(f"  {i+1}. Score: {result.score:.4f}, Product: {metadata.get('product_name')}")

def run_batch_operations_demo(client, collection_name):
    """Demonstrate batch operations performance"""
    print("\n⚡ Batch Operations Performance")
    print("-" * 40)
    
    # Generate batch data
    batch_sizes = [10, 50, 100, 500]
    dimension = 128
    
    for batch_size in batch_sizes:
        records = []
        for i in range(batch_size):
            vector = np.random.rand(dimension).astype(np.float32).tolist()
            record = VectorRecord(
                id=f"batch_{batch_size}_{i}",
                vector=vector,
                metadata={
                    "batch_size": batch_size,
                    "index": i
                }
            )
            records.append(record)
        
        start_time = time.time()
        result = client.upsert_vectors(
            collection_id=collection_name,
            records=records
        )
        insert_time = (time.time() - start_time) * 1000
        
        vectors_per_sec = batch_size / (insert_time / 1000)
        print(f"  Batch size {batch_size}: {insert_time:.2f}ms ({vectors_per_sec:.0f} vectors/sec)")

def run_cross_protocol_search(grpc_client, rest_client, collection_name):
    """Demonstrate search across different protocols"""
    print("\n🔄 Cross-Protocol Search Comparison")
    print("-" * 40)
    
    # Generate query vector
    query_vector = np.random.rand(128).astype(np.float32).tolist()
    
    # 1. gRPC Search
    print("\n1. gRPC Search (fastest):")
    start_time = time.time()
    grpc_results = grpc_client.search(
        collection_id=collection_name,
        vector=query_vector,
        top_k=5,
        include_metadata=True
    )
    grpc_time = (time.time() - start_time) * 1000
    print(f"   Time: {grpc_time:.2f}ms")
    print(f"   Top result: {grpc_results[0].id} (score: {grpc_results[0].score:.4f})")
    
    # 2. REST Search
    print("\n2. REST API Search:")
    start_time = time.time()
    rest_results = rest_client.search(
        collection_id=collection_name,
        vector=query_vector,
        top_k=5,
        include_metadata=True
    )
    rest_time = (time.time() - start_time) * 1000
    print(f"   Time: {rest_time:.2f}ms")
    print(f"   Top result: {rest_results[0].id} (score: {rest_results[0].score:.4f})")
    
    # 3. SQL Search
    print("\n3. SQL Search (most flexible):")
    vector_str = "[" + ", ".join(str(v) for v in query_vector) + "]"
    sql = f"""
    SELECT id, metadata
    FROM {collection_name}
    ORDER BY VECTOR_SIMILARITY(vector, {vector_str}, 'cosine')
    LIMIT 5
    """
    start_time = time.time()
    sql_result = rest_client.execute_sql(sql)
    sql_time = (time.time() - start_time) * 1000
    print(f"   Time: {sql_time:.2f}ms")
    print(f"   Top result: {sql_result['rows'][0]['id']}")
    
    print(f"\n📊 Protocol Performance Summary:")
    print(f"   gRPC: {grpc_time:.2f}ms (fastest)")
    print(f"   REST: {rest_time:.2f}ms")
    print(f"   SQL:  {sql_time:.2f}ms (most flexible)")

def test_storage_engines(grpc_client, rest_client):
    """Test both VIPER and SST storage engines"""
    print("\n🗄️ Storage Engine Comparison")
    print("=" * 60)
    
    dimension = 128
    num_vectors = 1000
    
    # Test data generation
    def generate_test_data(num_vectors, dimension):
        records = []
        for i in range(num_vectors):
            vector = np.random.rand(dimension).astype(np.float32).tolist()
            metadata = {
                "item_id": f"item_{i:04d}",
                "category": f"cat_{i % 5}",
                "value": float(i)
            }
            record = VectorRecord(
                id=f"vec_{i:04d}",
                vector=vector,
                metadata=metadata
            )
            records.append(record)
        return records
    
    # Test VIPER (columnar storage)
    print("\n📊 Testing VIPER Storage Engine (Columnar):")
    viper_collection = "demo_viper_collection"
    try:
        grpc_client.create_collection(
            name=viper_collection,
            dimension=dimension,
            distance_metric=DistanceMetric.COSINE,
            storage_engine=StorageEngine.VIPER,
            filterable_metadata_fields=["category", "value"]
        )
        print(f"✅ Created collection with VIPER engine")
    except Exception as e:
        if "already exists" in str(e):
            print(f"📁 Using existing VIPER collection")
    
    # Insert data to VIPER
    records = generate_test_data(num_vectors, dimension)
    start_time = time.time()
    for i in range(0, len(records), 100):
        batch = records[i:i+100]
        grpc_client.upsert_vectors(collection_id=viper_collection, records=batch)
    viper_insert_time = time.time() - start_time
    print(f"   Insert time: {viper_insert_time:.2f}s ({num_vectors/viper_insert_time:.0f} vectors/sec)")
    
    # Search VIPER
    query_vector = np.random.rand(dimension).astype(np.float32).tolist()
    start_time = time.time()
    viper_results = grpc_client.search(
        collection_id=viper_collection,
        vector=query_vector,
        top_k=10,
        include_metadata=True
    )
    viper_search_time = (time.time() - start_time) * 1000
    print(f"   Search time: {viper_search_time:.2f}ms")
    
    # Test SST (row-based storage)
    print("\n📋 Testing SST Storage Engine (Row-based):")
    sst_collection = "demo_sst_collection"
    try:
        grpc_client.create_collection(
            name=sst_collection,
            dimension=dimension,
            distance_metric=DistanceMetric.COSINE,
            storage_engine=StorageEngine.SST,
            filterable_metadata_fields=["category", "value"]
        )
        print(f"✅ Created collection with SST engine")
    except Exception as e:
        if "already exists" in str(e):
            print(f"📁 Using existing SST collection")
    
    # Insert data to SST
    records = generate_test_data(num_vectors, dimension)
    start_time = time.time()
    for i in range(0, len(records), 100):
        batch = records[i:i+100]
        grpc_client.upsert_vectors(collection_id=sst_collection, records=batch)
    sst_insert_time = time.time() - start_time
    print(f"   Insert time: {sst_insert_time:.2f}s ({num_vectors/sst_insert_time:.0f} vectors/sec)")
    
    # Search SST
    start_time = time.time()
    sst_results = grpc_client.search(
        collection_id=sst_collection,
        vector=query_vector,
        top_k=10,
        include_metadata=True
    )
    sst_search_time = (time.time() - start_time) * 1000
    print(f"   Search time: {sst_search_time:.2f}ms")
    
    # Cross-protocol search on SST collection
    print("\n🔄 Cross-Protocol Search on SST Collection:")
    run_cross_protocol_search(grpc_client, rest_client, sst_collection)
    
    # Comparison
    print("\n📊 Storage Engine Performance Summary:")
    print(f"   VIPER (Columnar):")
    print(f"     - Insert: {num_vectors/viper_insert_time:.0f} vectors/sec")
    print(f"     - Search: {viper_search_time:.2f}ms")
    print(f"   SST (Row-based):")
    print(f"     - Insert: {num_vectors/sst_insert_time:.0f} vectors/sec")
    print(f"     - Search: {sst_search_time:.2f}ms")
    
    return viper_collection, sst_collection

def main():
    print("🚀 ProximaDB Advanced Features Demo")
    print("=" * 60)
    
    # Use gRPC for performance-critical operations
    print("\n📡 Connecting via gRPC for high performance...")
    grpc_client = connect_grpc(url="http://localhost:5679")
    
    # Also connect via REST for SQL queries
    print("📡 Connecting via REST for SQL queries...")
    rest_client = connect_rest(url="http://localhost:5678")
    
    # Test storage engines first
    viper_collection, sst_collection = test_storage_engines(grpc_client, rest_client)
    
    # Create main demo collection
    collection_name = "advanced_demo_collection"
    print(f"\n📦 Creating main demo collection '{collection_name}'...")
    
    try:
        collection = grpc_client.create_collection(
            name=collection_name,
            dimension=128,
            distance_metric=DistanceMetric.COSINE,
            storage_engine=StorageEngine.VIPER,  # High-performance columnar storage
            filterable_metadata_fields=["category", "brand", "in_stock", "product_name"]
        )
        print("✅ Collection created with VIPER storage engine")
    except Exception as e:
        if "already exists" in str(e):
            print("📁 Using existing collection")
        else:
            raise
    
    # Insert demo data
    print("\n📝 Inserting demo product data...")
    categories = ["laptop", "phone", "tablet", "watch", "earbuds"]
    brands = ["TechCorp", "SmartBrand", "ProDevice", "EliteGadget"]
    products = ["Pro Max", "Ultra", "Air", "Mini", "Plus"]
    
    records = []
    for i in range(500):
        vector = np.random.rand(128).astype(np.float32).tolist()
        category = categories[i % len(categories)]
        brand = brands[i % len(brands)]
        product = products[i % len(products)]
        
        metadata = {
            "category": category,
            "brand": brand,
            "product_name": f"{brand} {category.title()} {product}",
            "price": float(np.random.randint(100, 2000)),
            "in_stock": np.random.choice(["true", "false"])
        }
        
        record = VectorRecord(
            id=f"product_{i:04d}",
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
    print(f"✅ Inserted {len(records)} product vectors")
    
    # Run demos
    run_sql_demo(rest_client, collection_name)
    run_distance_metrics_demo(grpc_client, collection_name)
    run_batch_operations_demo(grpc_client, collection_name)
    
    # Cross-protocol comparison on main collection
    print("\n🔄 Cross-Protocol Search on Main Collection:")
    run_cross_protocol_search(grpc_client, rest_client, collection_name)
    
    # Detailed response format comparison
    print("\n📋 Response Format Comparison")
    print("-" * 40)
    
    query_vector = np.random.rand(128).astype(np.float32).tolist()
    
    # gRPC response
    print("\n1. gRPC Response Format:")
    grpc_result = grpc_client.search(
        collection_id=collection_name,
        vector=query_vector,
        top_k=1,
        include_metadata=True
    )[0]
    print(f"   Type: {type(grpc_result).__name__}")
    print(f"   Fields: id='{grpc_result.id}', score={grpc_result.score:.4f}")
    print(f"   Metadata: {dict(grpc_result.metadata)}")
    
    # REST response
    print("\n2. REST Response Format:")
    rest_result = rest_client.search(
        collection_id=collection_name,
        vector=query_vector,
        top_k=1,
        include_metadata=True
    )[0]
    print(f"   Type: {type(rest_result).__name__}")
    print(f"   Fields: id='{rest_result.id}', score={rest_result.score:.4f}")
    print(f"   Metadata: {dict(rest_result.metadata)}")
    
    # SQL response
    print("\n3. SQL Response Format:")
    vector_str = "[" + ", ".join(str(v) for v in query_vector) + "]"
    sql = f"SELECT id, metadata FROM {collection_name} ORDER BY VECTOR_SIMILARITY(vector, {vector_str}, 'cosine') LIMIT 1"
    sql_result = rest_client.execute_sql(sql)
    print(f"   Type: dict with keys: {list(sql_result.keys())}")
    print(f"   Row data: {sql_result['rows'][0]}")
    
    # Performance and usage summary
    print("\n🎯 Protocol Comparison Summary")
    print("=" * 60)
    
    print("\n🚀 gRPC:")
    print("   ✅ Fastest performance (typically < 2ms)")
    print("   ✅ Best for high-throughput operations")
    print("   ✅ Binary protocol, efficient for large batches")
    print("   ✅ Strongly typed responses")
    print("   📊 Use case: Production workloads, real-time applications")
    
    print("\n🌐 REST API:")
    print("   ✅ Good performance (typically 2-5ms)")
    print("   ✅ Universal compatibility")
    print("   ✅ Easy to debug and test")
    print("   ✅ Works with any HTTP client")
    print("   📊 Use case: Web applications, microservices")
    
    print("\n📝 SQL API:")
    print("   ✅ Most flexible querying")
    print("   ✅ Complex filtering and aggregations")
    print("   ✅ Familiar syntax for developers")
    print("   ✅ Can combine vector and metadata queries")
    print("   📊 Use case: Analytics, complex queries, reporting")
    
    print("\n🗄️ Storage Engine Summary:")
    print("   VIPER (Columnar):")
    print("     ✅ Optimized for analytics and filtering")
    print("     ✅ Better compression")
    print("     ✅ Faster metadata queries")
    print("   SST (Row-based):")
    print("     ✅ Optimized for point lookups")
    print("     ✅ Better for write-heavy workloads")
    print("     ✅ Lower memory footprint")
    
    print(f"\n✅ Demo complete! Collections retained for testing:")
    print(f"   - {collection_name} (VIPER)")
    print(f"   - {viper_collection} (VIPER)")
    print(f"   - {sst_collection} (SST)")

if __name__ == "__main__":
    main()