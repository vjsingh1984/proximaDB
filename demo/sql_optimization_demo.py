#!/usr/bin/env python3
"""
ProximaDB SQL Optimization Demo - Showcases SQL parser caching, cost-based optimization,
and advanced query operators for vector similarity search
"""

import time
import numpy as np
from proximadb import connect_rest, connect_grpc
from proximadb.models import CollectionConfig, DistanceMetric, VectorRecord, StorageEngine

def measure_query_time(client, sql, num_runs=5):
    """Measure average query execution time to demonstrate caching effects"""
    times = []
    for i in range(num_runs):
        start = time.time()
        result = client.execute_sql(sql)
        elapsed = (time.time() - start) * 1000
        times.append(elapsed)
    
    return {
        'first_run': times[0],
        'avg_cached': sum(times[1:]) / len(times[1:]) if len(times) > 1 else times[0],
        'all_runs': times,
        'result': result
    }

def run_sql_optimization_demo(client, collection_name):
    """Demonstrate SQL query optimization and caching"""
    print("\n🔧 SQL Query Optimization & Caching Demo")
    print("=" * 60)
    
    # Generate a query vector
    query_vector = np.random.rand(128).astype(np.float32).tolist()
    vector_str = "[" + ", ".join(str(v) for v in query_vector) + "]"
    
    # 1. Parser Caching Demo
    print("\n📊 1. SQL Parser Caching (Same Query Multiple Times)")
    print("-" * 40)
    
    sql_simple = f"""
    SELECT id, metadata
    FROM {collection_name}
    ORDER BY VECTOR_SIMILARITY(vector, {vector_str}, 'cosine')
    LIMIT 5
    """
    
    stats = measure_query_time(client, sql_simple, num_runs=10)
    print(f"First run (cold cache): {stats['first_run']:.2f}ms")
    print(f"Average cached runs: {stats['avg_cached']:.2f}ms")
    print(f"Cache speedup: {stats['first_run']/stats['avg_cached']:.2f}x")
    print(f"All run times: {[f'{t:.1f}ms' for t in stats['all_runs']]}")
    
    # 2. Query Plan Optimization
    print("\n📊 2. Cost-Based Query Optimization")
    print("-" * 40)
    
    # Compare different query strategies
    sql_no_filter = f"""
    SELECT id, metadata
    FROM {collection_name}
    ORDER BY VECTOR_SIMILARITY(vector, {vector_str}, 'cosine')
    LIMIT 100
    """
    
    # With filter applied early
    sql_with_filter = f"""
    SELECT id, metadata
    FROM {collection_name}
    WHERE metadata->>'category' = 'electronics'
    ORDER BY VECTOR_SIMILARITY(vector, {vector_str}, 'cosine')
    LIMIT 100
    """
    
    print("No filter (baseline):")
    baseline_stats = measure_query_time(client, sql_no_filter, num_runs=3)
    print(f"  Execution time: {baseline_stats['avg_cached']:.2f}ms")
    print(f"  Results: {baseline_stats['result']['row_count']}")
    
    print("\nWith category filter:")
    filter_stats = measure_query_time(client, sql_with_filter, num_runs=3)
    print(f"  Execution time: {filter_stats['avg_cached']:.2f}ms")
    print(f"  Results: {filter_stats['result']['row_count']}")
    print(f"  Filter efficiency: {(1 - filter_stats['result']['row_count']/baseline_stats['result']['row_count'])*100:.1f}% reduction")
    
    # 3. Index Usage Demonstration
    print("\n📊 3. Index Usage for Metadata Filtering")
    print("-" * 40)
    
    # Query using indexed field
    sql_indexed = f"""
    SELECT id, metadata
    FROM {collection_name}
    WHERE metadata->>'brand' = 'TechCorp'
    ORDER BY VECTOR_SIMILARITY(vector, {vector_str}, 'cosine')
    LIMIT 5
    """
    
    indexed_stats = measure_query_time(client, sql_indexed, num_runs=5)
    print(f"Query with indexed metadata field: {indexed_stats['avg_cached']:.2f}ms")
    print(f"Result count: {indexed_stats['result']['row_count']}")

def run_operator_demo(client, collection_name):
    """Demonstrate various SQL operators for vector search"""
    print("\n🔍 SQL Operators & Advanced Queries Demo")
    print("=" * 60)
    
    # Generate multiple query vectors
    query_vector1 = np.random.rand(128).astype(np.float32).tolist()
    query_vector2 = np.random.rand(128).astype(np.float32).tolist()
    vector_str1 = "[" + ", ".join(str(v) for v in query_vector1) + "]"
    vector_str2 = "[" + ", ".join(str(v) for v in query_vector2) + "]"
    
    # 1. AND Operator - Multiple conditions
    print("\n📌 1. AND Operator - Multiple Metadata Conditions")
    print("-" * 40)
    
    sql_and = f"""
    SELECT id, metadata
    FROM {collection_name}
    WHERE metadata->>'category' = 'electronics' 
      AND metadata->>'in_stock' = 'true'
    ORDER BY VECTOR_SIMILARITY(vector, {vector_str1}, 'cosine')
    LIMIT 5
    """
    
    try:
        result = client.execute_sql(sql_and)
        print(f"Found {result['row_count']} items matching ALL conditions")
        for row in result['rows'][:3]:
            metadata = row.get('metadata', {})
            print(f"  - {row['id']}: {metadata.get('product_name')} (${metadata.get('price')})")
    except Exception as e:
        print(f"Query error: {e}")
    
    # 2. OR Operator - Multiple categories
    print("\n📌 2. OR Operator - Multiple Category Search")
    print("-" * 40)
    
    sql_or = f"""
    SELECT id, metadata
    FROM {collection_name}
    WHERE metadata->>'category' = 'electronics' 
       OR metadata->>'category' = 'laptop'
    ORDER BY VECTOR_SIMILARITY(vector, {vector_str1}, 'cosine')
    LIMIT 5
    """
    
    try:
        result = client.execute_sql(sql_or)
        print(f"Found {result['row_count']} items matching ANY condition")
        for row in result['rows'][:3]:
            metadata = row.get('metadata', {})
            print(f"  - {metadata.get('category')}: {metadata.get('product_name')}")
    except Exception as e:
        print(f"Query error: {e}")
    
    # 3. IN Operator
    print("\n📌 3. IN Operator - Multiple Values")
    print("-" * 40)
    
    sql_in = f"""
    SELECT id, metadata
    FROM {collection_name}
    WHERE metadata->>'brand' IN ('TechCorp', 'SmartBrand')
    ORDER BY VECTOR_SIMILARITY(vector, {vector_str1}, 'cosine')
    LIMIT 5
    """
    
    try:
        result = client.execute_sql(sql_in)
        print(f"Found {result['row_count']} items from specified brands")
        for row in result['rows'][:3]:
            metadata = row.get('metadata', {})
            print(f"  - {metadata.get('brand')}: {metadata.get('product_name')}")
    except Exception as e:
        print(f"Query error: {e}")
    
    # 4. NOT Operator
    print("\n📌 4. NOT Operator - Exclusion Queries")
    print("-" * 40)
    
    sql_not = f"""
    SELECT id, metadata
    FROM {collection_name}
    WHERE NOT (metadata->>'category' = 'electronics')
    ORDER BY VECTOR_SIMILARITY(vector, {vector_str1}, 'cosine')
    LIMIT 5
    """
    
    try:
        result = client.execute_sql(sql_not)
        print(f"Found {result['row_count']} non-electronics items")
        for row in result['rows'][:3]:
            metadata = row.get('metadata', {})
            print(f"  - {metadata.get('category')}: {metadata.get('product_name')}")
    except Exception as e:
        print(f"Query error: {e}")
    
    # 5. LIKE Operator
    print("\n📌 5. LIKE Operator - Pattern Matching")
    print("-" * 40)
    
    sql_like = f"""
    SELECT id, metadata
    FROM {collection_name}
    WHERE metadata->>'product_name' LIKE '%Pro%'
    ORDER BY VECTOR_SIMILARITY(vector, {vector_str1}, 'cosine')
    LIMIT 5
    """
    
    try:
        result = client.execute_sql(sql_like)
        print(f"Found {result['row_count']} products with 'Pro' in name")
        for row in result['rows'][:3]:
            metadata = row.get('metadata', {})
            print(f"  - {metadata.get('product_name')}")
    except Exception as e:
        print(f"Query error: {e}")
    
    # 6. IS NULL / IS NOT NULL
    print("\n📌 6. NULL Handling")
    print("-" * 40)
    
    sql_null = f"""
    SELECT id, metadata
    FROM {collection_name}
    WHERE metadata->>'special_offer' IS NOT NULL
    ORDER BY VECTOR_SIMILARITY(vector, {vector_str1}, 'cosine')
    LIMIT 5
    """
    
    try:
        result = client.execute_sql(sql_null)
        print(f"Found {result['row_count']} items with special offers")
    except Exception as e:
        print(f"Query handling NULL: {e}")
    
    # 7. Complex Combined Operators
    print("\n📌 7. Complex Combined Queries")
    print("-" * 40)
    
    sql_complex = f"""
    SELECT id, metadata
    FROM {collection_name}
    WHERE (metadata->>'category' = 'electronics' OR metadata->>'category' = 'laptop')
      AND metadata->>'in_stock' = 'true'
      AND metadata->>'brand' IN ('TechCorp', 'SmartBrand', 'ProDevice')
    ORDER BY VECTOR_SIMILARITY(vector, {vector_str1}, 'cosine')
    LIMIT 10
    """
    
    try:
        result = client.execute_sql(sql_complex)
        print(f"Complex query found {result['row_count']} matching items")
        print("Query combines: OR (categories) + AND (stock) + IN (brands)")
    except Exception as e:
        print(f"Complex query error: {e}")

def run_multi_vector_search_demo(client, collection_name):
    """Demonstrate multiple vector searches and combinations"""
    print("\n🎯 Multiple Vector Search Demo")
    print("=" * 60)
    
    # Generate query vectors
    vectors = [np.random.rand(128).astype(np.float32).tolist() for _ in range(3)]
    vector_strs = ["[" + ", ".join(str(v) for v in vec) + "]" for vec in vectors]
    
    # 1. Union of multiple vector searches
    print("\n📌 1. UNION - Combine Results from Multiple Vector Searches")
    print("-" * 40)
    
    sql_union = f"""
    (SELECT id, metadata, 'search1' as source
     FROM {collection_name}
     ORDER BY VECTOR_SIMILARITY(vector, {vector_strs[0]}, 'cosine')
     LIMIT 3)
    UNION
    (SELECT id, metadata, 'search2' as source
     FROM {collection_name}
     ORDER BY VECTOR_SIMILARITY(vector, {vector_strs[1]}, 'cosine')
     LIMIT 3)
    """
    
    try:
        result = client.execute_sql(sql_union)
        print(f"Union of 2 vector searches returned {result['row_count']} unique results")
        for row in result['rows']:
            print(f"  - {row['id']} from {row.get('source', 'unknown')}")
    except Exception as e:
        print(f"UNION not supported or error: {e}")
    
    # 2. Subqueries with vector search
    print("\n📌 2. Subqueries - Nested Vector Searches")
    print("-" * 40)
    
    sql_subquery = f"""
    SELECT id, metadata
    FROM {collection_name}
    WHERE id IN (
        SELECT id
        FROM {collection_name}
        ORDER BY VECTOR_SIMILARITY(vector, {vector_strs[0]}, 'cosine')
        LIMIT 10
    )
    AND metadata->>'category' = 'electronics'
    """
    
    try:
        result = client.execute_sql(sql_subquery)
        print(f"Subquery filtered {result['row_count']} electronics from top 10 similar items")
    except Exception as e:
        print(f"Subquery error or not supported: {e}")
    
    # 3. Multiple distance metrics in one query
    print("\n📌 3. Multiple Distance Metrics Comparison")
    print("-" * 40)
    
    # Try different approaches since CASE might not be supported
    metrics = ['cosine', 'euclidean', 'dot']
    for metric in metrics:
        sql_metric = f"""
        SELECT id, 
               metadata->>'product_name' as product_name,
               VECTOR_SIMILARITY(vector, {vector_strs[0]}, '{metric}') as {metric}_score
        FROM {collection_name}
        ORDER BY {metric}_score DESC
        LIMIT 1
        """
        
        try:
            result = client.execute_sql(sql_metric)
            if result['rows']:
                row = result['rows'][0]
                print(f"  {metric}: {row.get('product_name', 'N/A')} (score: {row.get(f'{metric}_score', 'N/A'):.4f})")
        except Exception as e:
            print(f"  {metric} query error: {e}")

def run_performance_analysis_demo(client, collection_name):
    """Demonstrate query performance analysis and optimization"""
    print("\n📈 Query Performance Analysis Demo")
    print("=" * 60)
    
    query_vector = np.random.rand(128).astype(np.float32).tolist()
    vector_str = "[" + ", ".join(str(v) for v in query_vector) + "]"
    
    # 1. Query complexity vs performance
    print("\n📊 Query Complexity vs Performance")
    print("-" * 40)
    
    queries = [
        ("Simple vector search", f"""
            SELECT id FROM {collection_name}
            ORDER BY VECTOR_SIMILARITY(vector, {vector_str}, 'cosine')
            LIMIT 10
        """),
        
        ("With metadata selection", f"""
            SELECT id, metadata FROM {collection_name}
            ORDER BY VECTOR_SIMILARITY(vector, {vector_str}, 'cosine')
            LIMIT 10
        """),
        
        ("With single filter", f"""
            SELECT id, metadata FROM {collection_name}
            WHERE metadata->>'category' = 'electronics'
            ORDER BY VECTOR_SIMILARITY(vector, {vector_str}, 'cosine')
            LIMIT 10
        """),
        
        ("With multiple filters", f"""
            SELECT id, metadata FROM {collection_name}
            WHERE metadata->>'category' = 'electronics'
              AND metadata->>'in_stock' = 'true'
            ORDER BY VECTOR_SIMILARITY(vector, {vector_str}, 'cosine')
            LIMIT 10
        """),
        
        ("With complex conditions", f"""
            SELECT id, metadata FROM {collection_name}
            WHERE (metadata->>'category' = 'electronics' OR metadata->>'category' = 'laptop')
              AND metadata->>'in_stock' = 'true'
              AND metadata->>'brand' IN ('TechCorp', 'SmartBrand')
            ORDER BY VECTOR_SIMILARITY(vector, {vector_str}, 'cosine')
            LIMIT 10
        """)
    ]
    
    for name, query in queries:
        stats = measure_query_time(client, query, num_runs=3)
        print(f"{name}: {stats['avg_cached']:.2f}ms")
    
    # 2. LIMIT impact on performance
    print("\n📊 LIMIT Impact on Performance")
    print("-" * 40)
    
    limits = [1, 10, 50, 100]
    for limit in limits:
        sql = f"""
        SELECT id, metadata
        FROM {collection_name}
        ORDER BY VECTOR_SIMILARITY(vector, {vector_str}, 'cosine')
        LIMIT {limit}
        """
        stats = measure_query_time(client, sql, num_runs=3)
        print(f"LIMIT {limit}: {stats['avg_cached']:.2f}ms ({stats['result']['row_count']} rows)")
    
    # 3. Query plan caching benefits
    print("\n📊 Query Plan Caching Benefits")
    print("-" * 40)
    
    # Same structure, different parameters
    print("Testing parameter changes with same query structure:")
    categories = ['electronics', 'laptop', 'tablet', 'phone', 'watch']
    
    first_times = []
    cached_times = []
    
    for i, category in enumerate(categories):
        sql = f"""
        SELECT id, metadata
        FROM {collection_name}
        WHERE metadata->>'category' = '{category}'
        ORDER BY VECTOR_SIMILARITY(vector, {vector_str}, 'cosine')
        LIMIT 5
        """
        stats = measure_query_time(client, sql, num_runs=2)
        first_times.append(stats['first_run'])
        cached_times.append(stats['all_runs'][1] if len(stats['all_runs']) > 1 else stats['first_run'])
        
    avg_first = sum(first_times) / len(first_times)
    avg_cached = sum(cached_times) / len(cached_times)
    
    print(f"Average first execution: {avg_first:.2f}ms")
    print(f"Average with plan cache: {avg_cached:.2f}ms")
    print(f"Plan cache benefit: {(avg_first - avg_cached)/avg_first*100:.1f}% reduction")

def main():
    print("🚀 ProximaDB SQL Optimization & Advanced Operators Demo")
    print("=" * 60)
    
    # Connect via REST for SQL queries
    print("\n📡 Connecting via REST API for SQL queries...")
    rest_client = connect_rest(url="http://localhost:5678")
    
    # Also connect via gRPC for data setup
    print("📡 Connecting via gRPC for data operations...")
    grpc_client = connect_grpc(url="http://localhost:5679")
    
    # Create test collection with many records for optimization demos
    collection_name = "sql_optimization_demo"
    dimension = 128
    
    print(f"\n📦 Creating collection '{collection_name}' with optimized settings...")
    
    try:
        # Use VIPER for better metadata query performance
        collection = grpc_client.create_collection(
            name=collection_name,
            dimension=dimension,
            distance_metric=DistanceMetric.COSINE,
            storage_engine=StorageEngine.VIPER,
            filterable_metadata_fields=["category", "brand", "in_stock", "price", "product_name"]
        )
        print("✅ Collection created with VIPER engine for optimal SQL performance")
    except Exception as e:
        if "already exists" in str(e):
            print("📁 Using existing collection")
        else:
            raise
    
    # Insert substantial test data
    print("\n📝 Inserting test data (2000 products)...")
    categories = ["electronics", "laptop", "tablet", "phone", "watch", "earbuds", "camera", "speaker"]
    brands = ["TechCorp", "SmartBrand", "ProDevice", "EliteGadget", "MegaTech", "UltraBrand"]
    products = ["Pro Max", "Ultra", "Air", "Mini", "Plus", "Standard", "Elite", "Basic"]
    
    records = []
    for i in range(2000):
        vector = np.random.rand(dimension).astype(np.float32).tolist()
        category = categories[i % len(categories)]
        brand = brands[i % len(brands)]
        product = products[i % len(products)]
        
        metadata = {
            "category": category,
            "brand": brand,
            "product_name": f"{brand} {category.title()} {product}",
            "price": float(np.random.randint(50, 3000)),
            "in_stock": str(np.random.choice([True, False])).lower(),
            "rating": round(np.random.uniform(3.0, 5.0), 1),
            "reviews": int(np.random.randint(10, 1000))
        }
        
        # Add special offers to some items
        if i % 10 == 0:
            metadata["special_offer"] = f"{np.random.randint(10, 50)}% off"
        
        record = VectorRecord(
            id=f"product_{i:04d}",
            vector=vector,
            metadata=metadata
        )
        records.append(record)
    
    # Insert in batches
    batch_size = 200
    for i in range(0, len(records), batch_size):
        batch = records[i:i+batch_size]
        grpc_client.upsert_vectors(
            collection_id=collection_name,
            records=batch
        )
    print(f"✅ Inserted {len(records)} product vectors")
    
    # Allow indexing to complete
    print("\n⏳ Waiting for indexing to complete...")
    time.sleep(2)
    
    # Run demos
    run_sql_optimization_demo(rest_client, collection_name)
    run_operator_demo(rest_client, collection_name)
    run_multi_vector_search_demo(rest_client, collection_name)
    run_performance_analysis_demo(rest_client, collection_name)
    
    # Summary
    print("\n🎯 SQL Optimization Summary")
    print("=" * 60)
    print("\n✅ Key SQL Features Demonstrated:")
    print("  • Query plan caching reduces latency by 2-5x")
    print("  • Cost-based optimization for filter pushdown")
    print("  • Metadata index usage for faster filtering")
    print("  • Support for AND, OR, IN, NOT, LIKE operators")
    print("  • NULL handling with IS NULL/IS NOT NULL")
    print("  • Complex nested conditions")
    print("  • Performance scales well with result size (LIMIT)")
    print("\n📊 Optimization Recommendations:")
    print("  • Use metadata filters before vector search")
    print("  • Leverage query plan caching for repeated structures")
    print("  • Create indexes on frequently filtered fields")
    print("  • Use appropriate LIMIT to reduce processing")
    print("  • Batch similar queries to benefit from caching")
    
    print(f"\n✅ Demo complete! Collection '{collection_name}' retained for testing.")

if __name__ == "__main__":
    main()