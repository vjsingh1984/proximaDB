"""
Example: Using SQL queries with ProximaDB

This example demonstrates how to use SQL queries for vector similarity search
with metadata filtering.
"""

import numpy as np
from proximadb import connect
from proximadb.models import CollectionConfig, StorageEngine


def main():
    # Connect to ProximaDB using REST
    client = connect(url="http://localhost:5678", protocol="rest")
    
    # Create a collection
    collection_name = "products_demo"  # Minimum 8 characters required
    config = CollectionConfig(
        name=collection_name,
        dimension=384,  # Common embedding dimension
        storage_engine=StorageEngine.VIPER
    )
    
    try:
        client.delete_collection(collection_name)
    except:
        pass
    
    collection = client.create_collection(collection_name, dimension=384, storage_engine=StorageEngine.VIPER)
    print(f"Created collection: {collection_name}")
    
    # Insert sample product data
    products = [
        {
            "id": "laptop_001",
            "vector": np.random.rand(384).tolist(),
            "metadata": {
                "name": "ProBook Laptop",
                "category": "electronics",
                "brand": "TechCorp",
                "price": 899.99,
                "rating": 4.5,
                "in_stock": True,
                "features": ["16GB RAM", "512GB SSD", "Intel i7"]
            }
        },
        {
            "id": "laptop_002",
            "vector": np.random.rand(384).tolist(),
            "metadata": {
                "name": "UltraBook Pro",
                "category": "electronics",
                "brand": "CompuTech",
                "price": 1299.99,
                "rating": 4.8,
                "in_stock": True,
                "features": ["32GB RAM", "1TB SSD", "Intel i9"]
            }
        },
        {
            "id": "phone_001",
            "vector": np.random.rand(384).tolist(),
            "metadata": {
                "name": "SmartPhone X",
                "category": "electronics",
                "brand": "PhoneCorp",
                "price": 699.99,
                "rating": 4.3,
                "in_stock": False,
                "features": ["5G", "128GB", "Triple Camera"]
            }
        },
        {
            "id": "book_001",
            "vector": np.random.rand(384).tolist(),
            "metadata": {
                "name": "Python Programming",
                "category": "books",
                "brand": "TechBooks",
                "price": 49.99,
                "rating": 4.7,
                "in_stock": True,
                "features": ["Beginner Friendly", "500 pages"]
            }
        },
        {
            "id": "book_002",
            "vector": np.random.rand(384).tolist(),
            "metadata": {
                "name": "Machine Learning Guide",
                "category": "books",
                "brand": "DataBooks",
                "price": 59.99,
                "rating": 4.6,
                "in_stock": True,
                "features": ["Advanced Topics", "700 pages"]
            }
        }
    ]
    
    # Extract vectors, ids, and metadata separately
    vectors = [p["vector"] for p in products]
    ids = [p["id"] for p in products]
    metadata = [p["metadata"] for p in products]
    
    response = client.insert_vectors(collection_name, vectors, ids, metadata)
    print(f"Inserted {len(products)} products")
    
    # Example 1: Basic vector similarity search
    print("\n=== Example 1: Basic Vector Similarity Search ===")
    query_vector = np.random.rand(384).tolist()
    # Format vector as [0.1, 0.2, ...] for SQL parser
    vector_str = "[" + ", ".join(str(v) for v in query_vector) + "]"
    
    sql = f"""
    SELECT id, metadata
    FROM {collection_name}
    ORDER BY VECTOR_SIMILARITY(vector, {vector_str}, 'cosine')
    LIMIT 3
    """
    
    try:
        result = client.execute_sql(sql)
        print(f"Found {result['row_count']} similar products:")
        for row in result['rows']:
            metadata = row.get('metadata', {})
            print(f"  - {row['id']}: {metadata.get('name', 'N/A')} (${metadata.get('price', 'N/A')})")
    except Exception as e:
        print(f"SQL Error: {e}")
        # Try without metadata fields to see if basic SQL works
        sql_basic = f"""
        SELECT id
        FROM {collection_name}
        ORDER BY VECTOR_SIMILARITY(vector, {vector_str}, 'cosine')
        LIMIT 3
        """
        print("\nTrying basic SQL without metadata fields...")
        try:
            result = client.execute_sql(sql_basic)
            print(f"Basic SQL worked! Found {result['row_count']} results")
            for row in result['rows']:
                print(f"  - ID: {row['id']}")
        except Exception as e2:
            print(f"Basic SQL also failed: {e2}")
    
    # Example 2: Filtered vector search
    print("\n=== Example 2: Vector Search with Category Filter ===")
    sql_filtered = f"""
    SELECT id, metadata
    FROM {collection_name}
    WHERE metadata->>'category' = 'electronics'
    ORDER BY VECTOR_SIMILARITY(vector, {vector_str}, 'cosine')
    LIMIT 3
    """
    
    result = client.execute_sql(sql_filtered)
    print(f"Found {result['row_count']} similar electronics:")
    for row in result['rows']:
        metadata = row.get('metadata', {})
        print(f"  - {metadata.get('name', 'N/A')}: ${metadata.get('price', 'N/A')} (Rating: {metadata.get('rating', 'N/A')})")
    
    # Example 3: Complex filtering with price range
    print("\n=== Example 3: Price Range Filter ===")
    sql_price = f"""
    SELECT id, metadata
    FROM {collection_name}
    WHERE metadata->>'in_stock' = 'true'
    ORDER BY VECTOR_SIMILARITY(vector, {vector_str}, 'cosine')
    LIMIT 5
    """
    
    result = client.execute_sql(sql_price)
    print(f"Found {result['row_count']} products in stock:")
    for row in result['rows']:
        metadata = row.get('metadata', {})
        stock = "In Stock" if metadata.get('in_stock') == True else "Out of Stock"
        print(f"  - {metadata.get('name', 'N/A')}: ${metadata.get('price', 'N/A')} ({stock})")
    
    # Example 4: Different distance metrics
    print("\n=== Example 4: Different Distance Metrics ===")
    metrics = ['cosine', 'euclidean', 'manhattan', 'dot']
    
    for metric in metrics:
        sql_metric = f"""
        SELECT id, metadata
        FROM {collection_name}
        ORDER BY VECTOR_SIMILARITY(vector, {vector_str}, '{metric}')
        LIMIT 1
        """
        
        result = client.execute_sql(sql_metric)
        if result['rows']:
            metadata = result['rows'][0].get('metadata', {})
            print(f"  {metric}: {metadata.get('name', 'N/A')}")
    
    # Example 5: Select all fields including vector
    print("\n=== Example 5: Select All Fields ===")
    sql_all = f"""
    SELECT *
    FROM {collection_name}
    ORDER BY VECTOR_SIMILARITY(vector, {vector_str}, 'cosine')
    LIMIT 2
    """
    
    result = client.execute_sql(sql_all)
    print(f"Found {result['row_count']} high-rated products with all fields")
    for i, row in enumerate(result['rows']):
        print(f"  Product {i+1}:")
        print(f"    ID: {row['id']}")
        print(f"    Vector dimension: {len(row.get('vector', []))}")
        if 'metadata' in row:
            print(f"    Metadata fields: {list(row['metadata'].keys())}")
    
    # Example 6: Pagination with OFFSET
    print("\n=== Example 6: Pagination ===")
    page_size = 2
    for page in range(3):
        offset = page * page_size
        sql_page = f"""
        SELECT id, metadata.name
        FROM {collection_name}
        ORDER BY VECTOR_SIMILARITY(vector, {query_vector}, 'cosine')
        LIMIT {page_size} OFFSET {offset}
        """
        
        result = client.execute_sql(sql_page)
        print(f"  Page {page + 1}: {[row['metadata.name'] for row in result['rows']]}")
    
    # Cleanup
    client.delete_collection(collection_name)
    print(f"\nDeleted collection: {collection_name}")


if __name__ == "__main__":
    main()