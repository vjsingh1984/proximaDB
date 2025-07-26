"""
Example: Using SQL queries with ProximaDB

This example demonstrates how to use SQL queries for vector similarity search
with metadata filtering.
"""

import numpy as np
from proximadb import connect
from proximadb.models import CollectionConfig, StorageEngine


def main():
    # Connect to ProximaDB
    client = connect(url="http://localhost:5678")
    
    # Create a collection
    collection_name = "products"
    config = CollectionConfig(
        name=collection_name,
        dimension=384,  # Common embedding dimension
        storage_engine=StorageEngine.LSM
    )
    
    try:
        client.delete_collection(collection_name)
    except:
        pass
    
    client.create_collection(config)
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
    
    response = client.insert_vectors(collection_name, products)
    print(f"Inserted {len(products)} products")
    
    # Example 1: Basic vector similarity search
    print("\n=== Example 1: Basic Vector Similarity Search ===")
    query_vector = np.random.rand(384).tolist()
    
    sql = f"""
    SELECT id, metadata.name, metadata.category, metadata.price
    FROM {collection_name}
    ORDER BY VECTOR_SIMILARITY(vector, {query_vector}, 'cosine')
    LIMIT 3
    """
    
    result = client.execute_sql(sql)
    print(f"Found {result['row_count']} similar products:")
    for row in result['rows']:
        print(f"  - {row['id']}: {row['metadata.name']} (${row['metadata.price']})")
    
    # Example 2: Filtered vector search
    print("\n=== Example 2: Vector Search with Category Filter ===")
    sql_filtered = f"""
    SELECT id, metadata.name, metadata.price, metadata.rating
    FROM {collection_name}
    WHERE metadata.category = 'electronics'
    ORDER BY VECTOR_SIMILARITY(vector, {query_vector}, 'cosine')
    LIMIT 3
    """
    
    result = client.execute_sql(sql_filtered)
    print(f"Found {result['row_count']} similar electronics:")
    for row in result['rows']:
        print(f"  - {row['metadata.name']}: ${row['metadata.price']} (Rating: {row['metadata.rating']})")
    
    # Example 3: Complex filtering with price range
    print("\n=== Example 3: Price Range Filter ===")
    sql_price = f"""
    SELECT id, metadata.name, metadata.price, metadata.in_stock
    FROM {collection_name}
    WHERE metadata.price BETWEEN 50 AND 1000
      AND metadata.in_stock = true
    ORDER BY VECTOR_SIMILARITY(vector, {query_vector}, 'cosine')
    LIMIT 5
    """
    
    result = client.execute_sql(sql_price)
    print(f"Found {result['row_count']} products in price range:")
    for row in result['rows']:
        stock = "In Stock" if row.get('metadata.in_stock') else "Out of Stock"
        print(f"  - {row['metadata.name']}: ${row['metadata.price']} ({stock})")
    
    # Example 4: Different distance metrics
    print("\n=== Example 4: Different Distance Metrics ===")
    metrics = ['cosine', 'euclidean', 'manhattan', 'dot']
    
    for metric in metrics:
        sql_metric = f"""
        SELECT id, metadata.name
        FROM {collection_name}
        ORDER BY VECTOR_SIMILARITY(vector, {query_vector}, '{metric}')
        LIMIT 1
        """
        
        result = client.execute_sql(sql_metric)
        if result['rows']:
            print(f"  {metric}: {result['rows'][0]['metadata.name']}")
    
    # Example 5: Select all fields including vector
    print("\n=== Example 5: Select All Fields ===")
    sql_all = f"""
    SELECT *
    FROM {collection_name}
    WHERE metadata.rating > 4.5
    ORDER BY VECTOR_SIMILARITY(vector, {query_vector}, 'cosine')
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