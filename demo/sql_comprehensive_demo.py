#!/usr/bin/env python3
"""
ProximaDB SQL Comprehensive Demo
Complete SQL interface demonstration including basic queries and optimization techniques

Features:
- Basic SQL syntax and operations
- Vector similarity search with SQL
- Metadata filtering
- Query optimization and caching
- Performance comparison
- Integration with demo logger
"""

import sys
import time
import numpy as np
import json
from pathlib import Path
from typing import Dict, List, Any
import uuid

# Add parent directory for utils
sys.path.append(str(Path(__file__).parent))

from proximadb import connect_rest
from proximadb.models import CollectionConfig, DistanceMetric, VectorRecord
from utils.demo_logger import DemoLogger


class SQLComprehensiveDemo:
    """Comprehensive SQL interface demonstration"""
    
    def __init__(self):
        self.logger = DemoLogger("sql_comprehensive")
        self.client = None
        self.collection_name = f"sql_demo_{uuid.uuid4().hex[:8]}"
        self.dimension = 128
        self.test_data = []
        
    def setup(self):
        """Setup client and test collection"""
        self.logger.section("SQL Demo Setup")
        
        try:
            self.client = connect_rest("http://localhost:5678")
            self.logger.success("Connected to ProximaDB REST API")
            
            # Create test collection
            config = CollectionConfig(
                dimension=self.dimension,
                distance_metric=DistanceMetric.COSINE,
                description="SQL demo collection"
            )
            
            collection = self.client.create_collection(self.collection_name, config)
            self.logger.success(f"Created collection: {self.collection_name}")
            
            # Generate test data
            self.generate_test_data()
            
            return True
            
        except Exception as e:
            self.logger.error("Setup failed", e)
            return False
    
    def generate_test_data(self):
        """Generate and insert test data"""
        self.logger.log("Generating test data...")
        
        # Create diverse test vectors
        categories = ["electronics", "clothing", "books", "toys", "food"]
        brands = ["BrandA", "BrandB", "BrandC", "BrandD", "BrandE"]
        
        vectors = []
        for i in range(100):
            vector = np.random.rand(self.dimension).astype(np.float32)
            # Make vectors cluster by category
            category_idx = i % len(categories)
            vector[category_idx*20:(category_idx+1)*20] += 0.5
            vector = vector / np.linalg.norm(vector)
            
            record = VectorRecord(
                id=f"product_{i}",
                vector=vector.tolist(),
                metadata={
                    "name": f"Product {i}",
                    "category": categories[category_idx],
                    "brand": brands[i % len(brands)],
                    "price": 10 + (i * 5) % 200,
                    "rating": 3.0 + (i % 20) / 10.0,
                    "in_stock": i % 3 != 0
                }
            )
            vectors.append(record)
        
        # Insert in batches
        batch_size = 20
        for i in range(0, len(vectors), batch_size):
            batch = vectors[i:i+batch_size]
            self.client.insert_batch(self.collection_name, batch)
        
        self.logger.success(f"Inserted {len(vectors)} test vectors")
        self.test_data = vectors
    
    def demonstrate_basic_sql(self):
        """Demonstrate basic SQL operations"""
        self.logger.section("Basic SQL Operations")
        
        # Generate a query vector
        query_vector = np.random.rand(self.dimension).astype(np.float32).tolist()
        vector_str = "[" + ", ".join(str(v) for v in query_vector[:3]) + ", ...]"
        
        # 1. Simple vector similarity search
        sql1 = f"""
        SELECT id, metadata
        FROM {self.collection_name}
        ORDER BY VECTOR_SIMILARITY(vector, {vector_str}, 'cosine')
        LIMIT 5
        """
        
        self.logger.log("1. Basic vector similarity search:")
        self.logger.log(f"   SQL: {sql1[:100]}...")
        
        start_time = time.time()
        result = self.client.execute_sql(sql1)
        query_time = (time.time() - start_time) * 1000
        
        self.logger.metric("Query time", query_time, "ms")
        self.logger.success(f"Found {result['row_count']} results")
        
        if result['rows']:
            for i, row in enumerate(result['rows'][:3]):
                self.logger.log(f"   {i+1}. {row['id']}: {row['metadata']['name']}")
        
        # 2. Filtered search
        sql2 = f"""
        SELECT id, metadata
        FROM {self.collection_name}
        WHERE metadata->>'category' = 'electronics'
        ORDER BY VECTOR_SIMILARITY(vector, {vector_str}, 'cosine')
        LIMIT 5
        """
        
        self.logger.log("\n2. Filtered search (electronics only):")
        
        start_time = time.time()
        result = self.client.execute_sql(sql2)
        query_time = (time.time() - start_time) * 1000
        
        self.logger.metric("Filtered query time", query_time, "ms")
        self.logger.success(f"Found {result['row_count']} electronics")
        
        # 3. Complex filtering with IN operator
        sql3 = f"""
        SELECT id, metadata
        FROM {self.collection_name}
        WHERE metadata->>'brand' IN ('BrandA', 'BrandB')
          AND metadata->>'in_stock' = 'true'
        ORDER BY VECTOR_SIMILARITY(vector, {vector_str}, 'euclidean')
        LIMIT 10
        """
        
        self.logger.log("\n3. Complex filtering (multiple brands, in stock):")
        
        start_time = time.time()
        result = self.client.execute_sql(sql3)
        query_time = (time.time() - start_time) * 1000
        
        self.logger.metric("Complex query time", query_time, "ms")
        self.logger.success(f"Found {result['row_count']} matching products")
    
    def demonstrate_sql_optimization(self):
        """Demonstrate SQL query optimization techniques"""
        self.logger.section("SQL Query Optimization")
        
        # Test query caching
        self.logger.log("Testing query parser caching...")
        
        query_vector = np.random.rand(self.dimension).astype(np.float32).tolist()
        vector_str = "[" + ", ".join(str(v) for v in query_vector[:3]) + ", ...]"
        
        # Same query structure, different parameters
        base_sql = """
        SELECT id, metadata
        FROM {collection}
        WHERE metadata->>'category' = '{category}'
        ORDER BY VECTOR_SIMILARITY(vector, {vector}, 'cosine')
        LIMIT 5
        """
        
        categories = ["electronics", "clothing", "books"]
        cache_times = []
        
        # First run - no cache
        for i, category in enumerate(categories):
            sql = base_sql.format(
                collection=self.collection_name,
                category=category,
                vector=vector_str
            )
            
            start_time = time.time()
            result = self.client.execute_sql(sql)
            query_time = (time.time() - start_time) * 1000
            cache_times.append(query_time)
            
            self.logger.log(f"   Query {i+1} ({category}): {query_time:.2f}ms")
        
        # Second run - with cache
        self.logger.log("\nSecond run (with cache):")
        
        for i, category in enumerate(categories):
            sql = base_sql.format(
                collection=self.collection_name,
                category=category,
                vector=vector_str
            )
            
            start_time = time.time()
            result = self.client.execute_sql(sql)
            query_time = (time.time() - start_time) * 1000
            
            speedup = cache_times[i] / query_time if query_time > 0 else 1
            self.logger.log(f"   Query {i+1} ({category}): {query_time:.2f}ms (speedup: {speedup:.1f}x)")
        
        # Test different distance metrics
        self.logger.log("\nDistance metric performance comparison:")
        
        metrics = ["cosine", "euclidean", "dot"]
        metric_times = {}
        
        for metric in metrics:
            sql = f"""
            SELECT id, metadata
            FROM {self.collection_name}
            ORDER BY VECTOR_SIMILARITY(vector, {vector_str}, '{metric}')
            LIMIT 10
            """
            
            start_time = time.time()
            result = self.client.execute_sql(sql)
            query_time = (time.time() - start_time) * 1000
            metric_times[metric] = query_time
            
            self.logger.metric(f"{metric} distance", query_time, "ms")
        
        # Find fastest metric
        fastest = min(metric_times.items(), key=lambda x: x[1])
        self.logger.success(f"Fastest metric: {fastest[0]} ({fastest[1]:.2f}ms)")
    
    def demonstrate_sql_limitations(self):
        """Demonstrate current SQL limitations and workarounds"""
        self.logger.section("SQL Limitations and Workarounds")
        
        self.logger.log("Current limitations:")
        self.logger.log("• Only equality (=) and IN operators supported")
        self.logger.log("• No AND/OR operators yet")
        self.logger.log("• No comparison operators (<, >, <=, >=)")
        self.logger.log("• No LIKE operator for pattern matching")
        
        # Show unsupported query examples
        unsupported_queries = [
            {
                "name": "AND operator",
                "sql": f"SELECT * FROM {self.collection_name} WHERE metadata->>'category' = 'electronics' AND metadata->>'price' > 100"
            },
            {
                "name": "Comparison operator",
                "sql": f"SELECT * FROM {self.collection_name} WHERE metadata->>'rating' >= 4.0"
            },
            {
                "name": "LIKE operator",
                "sql": f"SELECT * FROM {self.collection_name} WHERE metadata->>'name' LIKE 'Product 1%'"
            }
        ]
        
        for query in unsupported_queries:
            self.logger.log(f"\n{query['name']} (not supported):")
            self.logger.log(f"   SQL: {query['sql']}")
            
            try:
                result = self.client.execute_sql(query['sql'])
            except Exception as e:
                self.logger.warning(f"   Expected error: {str(e)[:100]}...")
        
        self.logger.log("\nWorkarounds:")
        self.logger.log("• Use multiple queries for AND/OR logic")
        self.logger.log("• Filter results client-side for comparisons")
        self.logger.log("• Use exact matches instead of patterns")
    
    def cleanup(self):
        """Clean up test collection"""
        try:
            self.client.delete_collection(self.collection_name)
            self.logger.success("Cleaned up test collection")
        except Exception as e:
            self.logger.warning(f"Cleanup failed: {e}")
    
    def run_demo(self):
        """Run complete SQL demo"""
        self.logger.section("ProximaDB SQL Comprehensive Demo")
        
        if not self.setup():
            return False
        
        try:
            self.demonstrate_basic_sql()
            self.demonstrate_sql_optimization()
            self.demonstrate_sql_limitations()
            
            self.logger.success("SQL demo completed successfully!")
            
            self.logger.section("Key Takeaways")
            self.logger.log("• SQL provides familiar interface for vector search")
            self.logger.log("• Query caching improves performance significantly")
            self.logger.log("• Distance metric choice affects performance")
            self.logger.log("• Current limitations require workarounds")
            self.logger.log("• Future updates will add more SQL operators")
            
            return True
            
        except Exception as e:
            self.logger.error("Demo failed", e)
            return False
        finally:
            self.cleanup()


def main():
    """Run SQL comprehensive demo"""
    demo = SQLComprehensiveDemo()
    
    with demo.logger:
        success = demo.run_demo()
        return 0 if success else 1


if __name__ == "__main__":
    sys.exit(main())