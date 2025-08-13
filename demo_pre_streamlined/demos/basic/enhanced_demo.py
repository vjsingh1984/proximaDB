#!/usr/bin/env python3
"""
Enhanced ProximaDB Demo using Python SDK
Demonstrates all major features step by step
"""

import sys
import os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), 'src'))

import time
import random
import numpy as np
from proximadb import connect_rest, CollectionConfig, DistanceMetric
import json

class EnhancedProximaDBDemo:
    """Enhanced demo showcasing ProximaDB features"""
    
    def __init__(self, server_url="http://localhost:5678"):
        self.server_url = server_url
        self.client = None
        self.collection_name = f"enhanced_demo_{int(time.time())}"
        self.demo_vectors = []
        self.demo_metadata = []
        
    def step_1_connect_and_health(self):
        """Step 1: Connect to ProximaDB and check health"""
        print("🔗 Step 1: Connecting to ProximaDB...")
        
        try:
            self.client = connect_rest(self.server_url)
            print("✅ Connected to ProximaDB successfully")
            
            # Try health check if available
            try:
                health = self.client.health()
                print(f"💚 Server health: {health}")
            except:
                print("ℹ️  Health endpoint not available (using gRPC interface)")
            
            return True
        except Exception as e:
            print(f"❌ Failed to connect: {e}")
            return False
    
    def step_2_explore_collections(self):
        """Step 2: Explore existing collections"""
        print("\n📋 Step 2: Exploring existing collections...")
        
        try:
            collections = self.client.list_collections()
            print(f"📊 Found {len(collections)} existing collections:")
            
            for i, collection in enumerate(collections[:5]):  # Show first 5
                if hasattr(collection, 'name'):
                    print(f"   {i+1}. {collection.name}")
                else:
                    print(f"   {i+1}. {collection}")
            
            if len(collections) > 5:
                print(f"   ... and {len(collections) - 5} more")
            
            return True
        except Exception as e:
            print(f"❌ Failed to list collections: {e}")
            return False
    
    def step_3_create_demo_collection(self):
        """Step 3: Create a demo collection with specific configuration"""
        print(f"\n🗂️  Step 3: Creating demo collection '{self.collection_name}'...")
        
        try:
            config = CollectionConfig(
                dimension=256,  # Larger dimension for more realistic demo
                distance_metric=DistanceMetric.COSINE,
                description="Enhanced demo collection showcasing ProximaDB features"
            )
            
            collection = self.client.create_collection(self.collection_name, config)
            print(f"✅ Collection created successfully:")
            print(f"   📝 Name: {collection.name}")
            print(f"   📏 Dimension: {collection.dimension}")
            print(f"   📐 Distance metric: {collection.metric}")
            print(f"   📄 Description: {config.description}")
            
            return True
        except Exception as e:
            print(f"❌ Failed to create collection: {e}")
            return False
    
    def step_4_generate_realistic_data(self, num_vectors=50):
        """Step 4: Generate realistic demo data with categories"""
        print(f"\n🎲 Step 4: Generating {num_vectors} realistic demo vectors...")
        
        categories = ["electronics", "books", "clothing", "sports", "home", "toys"]
        brands = ["BrandA", "BrandB", "BrandC", "BrandD", "BrandE"]
        
        self.demo_vectors = []
        self.demo_metadata = []
        
        for i in range(num_vectors):
            # Generate vector with some structure based on category
            category = random.choice(categories)
            category_seed = hash(category) % 1000
            
            # Create vector with category-specific patterns
            vector = np.random.randn(256).astype(np.float32)
            
            # Add category signature to first 10 dimensions
            vector[:10] += (category_seed / 1000.0)
            
            # Add some noise and normalize
            vector += np.random.normal(0, 0.1, 256)
            vector = vector / np.linalg.norm(vector)
            
            # Generate realistic metadata
            metadata = {
                "category": category,
                "price": round(random.uniform(10.0, 2000.0), 2),
                "rating": round(random.uniform(1.0, 5.0), 1),
                "brand": random.choice(brands),
                "name": f"{category.title()} Product {i}",
                "description": f"High-quality {category} item from our collection",
                "in_stock": random.choice([True, False]),
                "discount": random.randint(0, 50),
                "reviews": random.randint(0, 1000),
                "weight": round(random.uniform(0.1, 10.0), 2),
                "created_date": f"2024-{random.randint(1,12):02d}-{random.randint(1,28):02d}"
            }
            
            self.demo_vectors.append(vector.tolist())
            self.demo_metadata.append(metadata)
        
        print(f"✅ Generated {len(self.demo_vectors)} vectors across {len(categories)} categories")
        print(f"📊 Sample categories: {list(set(m['category'] for m in self.demo_metadata[:10]))}")
        
        return True
    
    def step_5_batch_insert_vectors(self):
        """Step 5: Insert vectors in batches for optimal performance"""
        print(f"\n📤 Step 5: Inserting {len(self.demo_vectors)} vectors in batches...")
        
        try:
            # Generate IDs
            ids = [f"demo_vec_{i}" for i in range(len(self.demo_vectors))]
            
            # Insert in batches for better performance
            batch_size = 20
            total_inserted = 0
            start_time = time.time()
            
            for i in range(0, len(self.demo_vectors), batch_size):
                batch_vectors = self.demo_vectors[i:i+batch_size]
                batch_ids = ids[i:i+batch_size]
                batch_metadata = self.demo_metadata[i:i+batch_size]
                
                # Insert batch
                result = self.client.insert_vectors(
                    self.collection_name, 
                    batch_vectors, 
                    batch_ids,
                    metadata=batch_metadata
                )
                
                total_inserted += result.successful_count
                
                if (i // batch_size + 1) % 3 == 0:  # Progress update every 3 batches
                    progress = min(100, ((i + batch_size) / len(self.demo_vectors)) * 100)
                    print(f"   📈 Progress: {progress:.1f}% ({total_inserted} vectors inserted)")
            
            duration = time.time() - start_time
            print(f"✅ Successfully inserted {total_inserted} vectors")
            print(f"⚡ Performance: {total_inserted/duration:.0f} vectors/second")
            print(f"⏱️  Total time: {duration:.2f} seconds")
            
            return True
        except Exception as e:
            print(f"❌ Failed to insert vectors: {e}")
            return False
    
    def step_6_basic_similarity_search(self):
        """Step 6: Perform basic similarity search"""
        print("\n🔍 Step 6: Performing basic similarity search...")
        
        try:
            # Use a random vector from our dataset as query
            query_idx = random.randint(0, len(self.demo_vectors) - 1)
            query_vector = self.demo_vectors[query_idx]
            query_metadata = self.demo_metadata[query_idx]
            
            print(f"🎯 Searching for vectors similar to '{query_metadata['name']}'")
            print(f"   Category: {query_metadata['category']}")
            print(f"   Price: ${query_metadata['price']}")
            
            start_time = time.time()
            results = self.client.search(self.collection_name, query_vector, k=10)
            search_time = time.time() - start_time
            
            print(f"✅ Found {len(results)} similar items in {search_time*1000:.2f}ms")
            print(f"\n🎯 Top 5 most similar items:")
            
            for i, result in enumerate(results[:5]):
                metadata = getattr(result, 'metadata', {}) or {}
                print(f"   {i+1}. {result.id}")
                print(f"      📊 Similarity: {result.score:.4f}")
                print(f"      🏷️  Category: {metadata.get('category', 'N/A')}")
                print(f"      💰 Price: ${metadata.get('price', 'N/A')}")
                print(f"      ⭐ Rating: {metadata.get('rating', 'N/A')}/5.0")
                print()
            
            return True
        except Exception as e:
            print(f"❌ Search failed: {e}")
            return False
    
    def step_7_category_filtering_search(self):
        """Step 7: Search with category filtering"""
        print("\n🏷️  Step 7: Performing category-filtered search...")
        
        try:
            # Search for electronics specifically
            target_category = "electronics"
            query_vector = self.demo_vectors[10]  # Use different query
            
            # Get all electronics first to show filtering
            electronics_items = [m for m in self.demo_metadata if m['category'] == target_category]
            print(f"📊 Dataset contains {len(electronics_items)} {target_category} items")
            
            start_time = time.time()
            results = self.client.search(self.collection_name, query_vector, k=10)
            search_time = time.time() - start_time
            
            # Filter results manually (since SDK might not support metadata filtering yet)
            filtered_results = [
                r for r in results 
                if getattr(r, 'metadata', {}) and 
                getattr(r, 'metadata', {}).get('category') == target_category
            ]
            
            print(f"✅ Search completed in {search_time*1000:.2f}ms")
            print(f"🎯 Found {len(filtered_results)} {target_category} items:")
            
            for i, result in enumerate(filtered_results[:3]):
                metadata = getattr(result, 'metadata', {}) or {}
                print(f"   {i+1}. {metadata.get('name', result.id)}")
                print(f"      📊 Similarity: {result.score:.4f}")
                print(f"      💰 Price: ${metadata.get('price', 'N/A')}")
                print(f"      🏪 Brand: {metadata.get('brand', 'N/A')}")
                print()
            
            return True
        except Exception as e:
            print(f"❌ Filtered search failed: {e}")
            return False
    
    def step_8_price_range_analysis(self):
        """Step 8: Analyze vectors by price range"""
        print("\n💰 Step 8: Analyzing vectors by price range...")
        
        try:
            # Analyze price distribution in our dataset
            prices = [m['price'] for m in self.demo_metadata]
            categories = list(set(m['category'] for m in self.demo_metadata))
            
            print(f"📊 Price Analysis:")
            print(f"   💵 Min price: ${min(prices):.2f}")
            print(f"   💵 Max price: ${max(prices):.2f}")
            print(f"   💵 Average price: ${sum(prices)/len(prices):.2f}")
            
            # Show price breakdown by category
            print(f"\n🏷️  Price by Category:")
            for category in sorted(categories):
                cat_prices = [m['price'] for m in self.demo_metadata if m['category'] == category]
                avg_price = sum(cat_prices) / len(cat_prices)
                print(f"   {category.title()}: ${avg_price:.2f} avg ({len(cat_prices)} items)")
            
            # Search for premium items (top 20% price range)
            price_threshold = sorted(prices, reverse=True)[len(prices)//5]  # Top 20%
            premium_items = [m for m in self.demo_metadata if m['price'] >= price_threshold]
            
            print(f"\n💎 Premium Items Analysis (≥${price_threshold:.2f}):")
            print(f"   Found {len(premium_items)} premium items")
            
            # Show premium categories
            premium_categories = {}
            for item in premium_items:
                cat = item['category']
                premium_categories[cat] = premium_categories.get(cat, 0) + 1
            
            for cat, count in sorted(premium_categories.items(), key=lambda x: x[1], reverse=True):
                print(f"   {cat.title()}: {count} premium items")
            
            return True
        except Exception as e:
            print(f"❌ Price analysis failed: {e}")
            return False
    
    def step_9_performance_benchmark(self):
        """Step 9: Run performance benchmarks"""
        print("\n⚡ Step 9: Running performance benchmarks...")
        
        try:
            # Benchmark different k values
            k_values = [1, 5, 10, 20]
            benchmark_results = {}
            
            print("📊 Benchmarking search performance with different k values:")
            
            for k in k_values:
                times = []
                query_vector = self.demo_vectors[0]
                
                # Run multiple searches for average
                for _ in range(5):
                    start_time = time.time()
                    results = self.client.search(self.collection_name, query_vector, k=k)
                    search_time = time.time() - start_time
                    times.append(search_time * 1000)  # Convert to ms
                
                avg_time = sum(times) / len(times)
                benchmark_results[k] = avg_time
                
                print(f"   k={k:2d}: {avg_time:6.2f}ms average ({len(results)} results)")
            
            # Test with multiple random queries
            print(f"\n🎯 Batch Query Performance Test:")
            num_queries = 10
            start_time = time.time()
            
            for i in range(num_queries):
                query_vector = self.demo_vectors[i % len(self.demo_vectors)]
                results = self.client.search(self.collection_name, query_vector, k=5)
            
            total_time = time.time() - start_time
            qps = num_queries / total_time
            
            print(f"   Executed {num_queries} queries in {total_time:.2f}s")
            print(f"   ⚡ Throughput: {qps:.1f} queries per second")
            print(f"   📊 Average latency: {(total_time/num_queries)*1000:.2f}ms per query")
            
            return True
        except Exception as e:
            print(f"❌ Performance benchmark failed: {e}")
            return False
    
    def step_10_collection_management(self):
        """Step 10: Demonstrate collection management"""
        print("\n🗂️  Step 10: Collection management operations...")
        
        try:
            # Get collection info
            collection_info = self.client.get_collection(self.collection_name)
            print(f"📋 Collection Information:")
            print(f"   🆔 ID: {collection_info.id}")
            print(f"   📝 Name: {collection_info.name}")
            print(f"   📏 Dimension: {collection_info.dimension}")
            print(f"   📐 Distance Metric: {collection_info.metric}")
            print(f"   📊 Status: {collection_info.status}")
            
            # Collection statistics (if available)
            print(f"\n📊 Collection Statistics:")
            print(f"   📦 Vectors inserted: {len(self.demo_vectors)}")
            print(f"   🏷️  Categories: {len(set(m['category'] for m in self.demo_metadata))}")
            print(f"   💰 Price range: ${min(m['price'] for m in self.demo_metadata):.2f} - ${max(m['price'] for m in self.demo_metadata):.2f}")
            
            return True
        except Exception as e:
            print(f"❌ Collection management failed: {e}")
            return False
    
    def cleanup(self):
        """Clean up demo resources"""
        print(f"\n🧹 Cleaning up demo resources...")
        
        try:
            # Delete the demo collection
            self.client.delete_collection(self.collection_name)
            print(f"✅ Demo collection '{self.collection_name}' deleted")
            return True
        except Exception as e:
            print(f"⚠️  Cleanup failed: {e}")
            return False
    
    def run_complete_demo(self):
        """Run the complete enhanced demo"""
        print("🎭 ProximaDB Enhanced Feature Demo")
        print("=" * 60)
        print("This demo showcases:")
        print("• Vector operations with realistic data")
        print("• Similarity search and filtering")
        print("• Performance benchmarking")
        print("• Collection management")
        print("• Data analysis capabilities")
        print("=" * 60)
        
        steps = [
            self.step_1_connect_and_health,
            self.step_2_explore_collections,
            self.step_3_create_demo_collection,
            lambda: self.step_4_generate_realistic_data(50),
            self.step_5_batch_insert_vectors,
            self.step_6_basic_similarity_search,
            self.step_7_category_filtering_search,
            self.step_8_price_range_analysis,
            self.step_9_performance_benchmark,
            self.step_10_collection_management
        ]
        
        success_count = 0
        
        for i, step in enumerate(steps, 1):
            print(f"\n{'='*60}")
            try:
                success = step()
                if success:
                    success_count += 1
                    print(f"✅ Step {i} completed successfully")
                else:
                    print(f"❌ Step {i} failed")
                    break
            except Exception as e:
                print(f"💥 Step {i} crashed: {e}")
                break
            
            if i < len(steps):
                print(f"\n⏳ Moving to next step in 2 seconds...")
                time.sleep(2)
        
        # Cleanup
        self.cleanup()
        
        print(f"\n{'='*60}")
        print(f"🎉 Demo Summary")
        print(f"✅ Completed {success_count}/{len(steps)} steps successfully")
        
        if success_count == len(steps):
            print(f"🏆 All ProximaDB features demonstrated successfully!")
            print(f"💡 Key achievements:")
            print(f"   • Created and managed vector collection")
            print(f"   • Inserted {len(self.demo_vectors)} vectors with metadata")
            print(f"   • Performed similarity search and filtering")
            print(f"   • Analyzed data by categories and price ranges")
            print(f"   • Benchmarked search performance")
        else:
            print(f"⚠️  Demo partially completed")
        
        return success_count == len(steps)

def main():
    """Main entry point"""
    print("🚀 Starting ProximaDB Enhanced Demo...")
    
    demo = EnhancedProximaDBDemo()
    success = demo.run_complete_demo()
    
    if success:
        print(f"\n🎊 ProximaDB demo completed successfully!")
    else:
        print(f"\n😞 Demo encountered some issues")
    
    return success

if __name__ == "__main__":
    success = main()
    sys.exit(0 if success else 1)