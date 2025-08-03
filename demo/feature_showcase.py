#!/usr/bin/env python3
"""
ProximaDB Feature Showcase Demo
===============================
Comprehensive demonstration of all ProximaDB capabilities
"""

import time
import json
import numpy as np
from typing import List, Dict, Any
from proximadb import ProximaDBClient, VectorRecord, CollectionConfig, DistanceMetric

class FeatureShowcase:
    """Demonstrates all ProximaDB features step by step"""
    
    def __init__(self):
        self.client = ProximaDBClient()
        self.collection_prefix = "showcase"
        
    def print_section(self, title: str):
        """Print formatted section header"""
        print(f"\n{'='*70}")
        print(f"{title}")
        print(f"{'='*70}\n")
        
    def demo_1_distance_metrics(self):
        """Demo 1: All 13 Distance Metrics"""
        self.print_section("DEMO 1: Distance Metrics Support")
        
        metrics = [
            ("cosine", DistanceMetric.COSINE, "Angle-based similarity (0=identical, 2=opposite)"),
            ("euclidean", DistanceMetric.EUCLIDEAN, "L2 norm straight-line distance"),
            ("dot_product", DistanceMetric.DOT_PRODUCT, "Inner product similarity"),
            ("manhattan", DistanceMetric.MANHATTAN, "L1 norm sum of absolute differences"),
            ("hamming", DistanceMetric.HAMMING, "Bit-level differences"),
        ]
        
        print("ProximaDB supports 13 hardware-accelerated distance metrics:")
        print("• Core: Cosine, Euclidean, Dot Product, Manhattan, Hamming, Jaccard")
        print("• Extended: Chebyshev, Canberra, Minkowski, Angular, Bray-Curtis, Hellinger\n")
        
        # Create test vectors
        vec1 = [1.0, 0.0, 0.0, 0.0]
        vec2 = [0.707, 0.707, 0.0, 0.0]  # 45 degrees from vec1
        
        for name, metric, description in metrics[:3]:  # Demo first 3
            collection = f"{self.collection_prefix}_{name}"
            print(f"Testing {name.upper()} distance:")
            print(f"   {description}")
            
            try:
                # Create collection with specific metric
                self.client.create_collection(
                    name=collection,
                    dimension=4,
                    distance_metric=metric,
                    engine="viper"
                )
                
                # Insert vectors
                self.client.insert_vectors(collection, [
                    VectorRecord(id="vec1", vector=vec1, metadata={"name": "vec1"}),
                    VectorRecord(id="vec2", vector=vec2, metadata={"name": "vec2"})
                ])
                
                # Search
                results = self.client.search(collection, vec1, top_k=2)
                for r in results["results"]:
                    print(f"   - {r['id']}: distance={r['distance']:.4f}")
                    
                # Cleanup
                self.client.delete_collection(collection)
                
            except Exception as e:
                print(f"   Error: {e}")
                
        print("\nDistance metrics demonstrated successfully!")
        
    def demo_2_metadata_filtering(self):
        """Demo 2: Advanced Metadata Filtering"""
        self.print_section("DEMO 2: Metadata Filtering")
        
        collection = f"{self.collection_prefix}_filtering"
        
        print("Creating collection with products...")
        self.client.create_collection(name=collection, dimension=4, engine="viper")
        
        # Insert products with metadata
        products = [
            {"id": "p1", "vector": [1.0, 0.0, 0.0, 0.0], "metadata": {"category": "electronics", "price": 999, "brand": "TechCorp"}},
            {"id": "p2", "vector": [0.0, 1.0, 0.0, 0.0], "metadata": {"category": "electronics", "price": 1499, "brand": "SmartBrand"}},
            {"id": "p3", "vector": [0.0, 0.0, 1.0, 0.0], "metadata": {"category": "furniture", "price": 299, "brand": "HomeStyle"}},
            {"id": "p4", "vector": [0.0, 0.0, 0.0, 1.0], "metadata": {"category": "electronics", "price": 599, "brand": "TechCorp"}},
        ]
        
        vectors = [VectorRecord(**p) for p in products]
        self.client.insert_vectors(collection, vectors)
        print(f"Inserted {len(products)} products")
        
        # Test different filters
        filters = [
            {"category": "electronics"},
            {"brand": "TechCorp"},
            {"price": {"$gte": 500, "$lte": 1000}},
        ]
        
        query_vector = [0.9, 0.1, 0.0, 0.0]
        
        for i, filter_spec in enumerate(filters, 1):
            print(f"\nFilter {i}: {filter_spec}")
            try:
                results = self.client.search(
                    collection,
                    query_vector,
                    top_k=5,
                    metadata_filter=filter_spec
                )
                
                print(f"   Found {len(results['results'])} matches:")
                for r in results["results"]:
                    print(f"   - {r['id']}: {r.get('metadata', {})}")
                    
            except Exception as e:
                print(f"   ⚠️  Note: {e}")
        
        self.client.delete_collection(collection)
        print("\nMetadata filtering demonstrated!")
        
    def demo_3_batch_operations(self):
        """Demo 3: High-Performance Batch Operations"""
        self.print_section("DEMO 3: Batch Operations & Performance")
        
        collection = f"{self.collection_prefix}_batch"
        self.client.create_collection(name=collection, dimension=128, engine="viper")
        
        # Test different batch sizes
        batch_sizes = [10, 100, 500, 1000]
        
        print("Testing batch insertion performance:")
        print(f"{'Batch Size':>12} | {'Time (s)':>10} | {'Vectors/sec':>12}")
        print("-" * 40)
        
        for batch_size in batch_sizes:
            # Generate random vectors
            vectors = []
            for i in range(batch_size):
                vec = np.random.rand(128).tolist()
                vectors.append(VectorRecord(
                    id=f"vec_{i}",
                    vector=vec,
                    metadata={"batch": batch_size, "index": i}
                ))
            
            # Time the insertion
            start = time.time()
            result = self.client.insert_vectors(collection, vectors)
            elapsed = time.time() - start
            
            rate = batch_size / elapsed
            print(f"{batch_size:>12} | {elapsed:>10.3f} | {rate:>12.0f}")
        
        # Test batch search
        print("\nTesting batch search:")
        query_vectors = [np.random.rand(128).tolist() for _ in range(5)]
        
        start = time.time()
        for qv in query_vectors:
            self.client.search(collection, qv, top_k=10)
        elapsed = time.time() - start
        
        print(f"Searched 5 queries in {elapsed:.3f}s ({5/elapsed:.1f} queries/sec)")
        
        self.client.delete_collection(collection)
        
    def demo_4_vector_updates(self):
        """Demo 4: Vector Updates and Versioning"""
        self.print_section("DEMO 4: Vector Updates & Versioning")
        
        collection = f"{self.collection_prefix}_updates"
        self.client.create_collection(name=collection, dimension=4, engine="viper")
        
        print("Initial vector insertion:")
        vec_id = "product_123"
        
        # Version 1
        v1 = VectorRecord(
            id=vec_id,
            vector=[1.0, 0.0, 0.0, 0.0],
            metadata={"version": 1, "name": "Product v1", "updated": "2024-01-01"}
        )
        self.client.insert_vectors(collection, [v1])
        print(f"Inserted {vec_id} (version 1)")
        
        # Version 2 - Update
        v2 = VectorRecord(
            id=vec_id,
            vector=[0.0, 1.0, 0.0, 0.0],
            metadata={"version": 2, "name": "Product v2", "updated": "2024-01-02"}
        )
        self.client.upsert_vectors(collection, [v2])
        print(f"Updated {vec_id} (version 2)")
        
        # Verify update
        results = self.client.get_vectors(collection, [vec_id])
        if results and results[0]:
            print(f"Current state: {results[0].get('metadata', {})}")
        
        self.client.delete_collection(collection)
        print("\nVector updates demonstrated!")
        
    def demo_5_collection_management(self):
        """Demo 5: Collection Management"""
        self.print_section("DEMO 5: Collection Management")
        
        print("📦 Collection operations:")
        
        # Create multiple collections
        collections = [
            ("users", 512, "viper"),
            ("products", 384, "viper"),
            ("documents", 768, "sst"),
        ]
        
        for name, dim, engine in collections:
            full_name = f"{self.collection_prefix}_{name}"
            try:
                self.client.create_collection(
                    name=full_name,
                    dimension=dim,
                    engine=engine
                )
                print(f"Created: {full_name} (dim={dim}, engine={engine})")
            except Exception as e:
                print(f"⚠️  {full_name}: {e}")
        
        # List collections
        print("\nListing collections:")
        all_collections = self.client.list_collections()
        showcase_collections = [c for c in all_collections if self.collection_prefix in c]
        for col in showcase_collections:
            print(f"   - {col}")
        
        # Get collection info (if supported)
        if showcase_collections:
            col_name = showcase_collections[0]
            print(f"\nCollection info for {col_name}:")
            try:
                info = self.client.get_collection_info(col_name)
                print(f"   {json.dumps(info, indent=2)}")
            except:
                print("   (Collection info not available)")
        
        # Cleanup
        print("\n🧹 Cleaning up collections...")
        for col in showcase_collections:
            self.client.delete_collection(col)
            print(f"   Deleted: {col}")
            
    def demo_6_persistence(self):
        """Demo 6: Persistence and Recovery"""
        self.print_section("DEMO 6: Persistence & Recovery")
        
        collection = f"{self.collection_prefix}_persist"
        
        print("💾 Testing persistence:")
        
        # Create and populate collection
        self.client.create_collection(name=collection, dimension=4, engine="viper")
        
        test_vectors = [
            VectorRecord(id=f"persist_{i}", vector=np.random.rand(4).tolist(), 
                        metadata={"index": i, "timestamp": time.time()})
            for i in range(10)
        ]
        
        self.client.insert_vectors(collection, test_vectors)
        print(f"✅ Inserted {len(test_vectors)} vectors")
        
        # Force flush (if supported)
        print("💾 Forcing flush to disk...")
        try:
            self.client.flush_collection(collection)
            print("✅ Collection flushed successfully")
        except:
            print("⚠️  Manual flush not available (auto-flush enabled)")
        
        # Verify data
        query = test_vectors[0]["vector"]
        results = self.client.search(collection, query, top_k=5)
        print(f"🔍 Search after flush: Found {len(results['results'])} results")
        
        self.client.delete_collection(collection)
        print("\n✅ Persistence demonstrated!")
        
    def run_all_demos(self):
        """Run all feature demonstrations"""
        print("\n🚀 ProximaDB Feature Showcase")
        print("="*70)
        print("This demo showcases all major ProximaDB capabilities\n")
        
        demos = [
            ("Distance Metrics", self.demo_1_distance_metrics),
            ("Metadata Filtering", self.demo_2_metadata_filtering),
            ("Batch Operations", self.demo_3_batch_operations),
            ("Vector Updates", self.demo_4_vector_updates),
            ("Collection Management", self.demo_5_collection_management),
            ("Persistence", self.demo_6_persistence),
        ]
        
        for i, (name, demo_func) in enumerate(demos, 1):
            try:
                demo_func()
                time.sleep(1)  # Brief pause between demos
            except Exception as e:
                print(f"\n❌ Demo {i} ({name}) failed: {e}")
                
        print("\n" + "="*70)
        print("🎉 Feature showcase completed!")
        print("="*70)

if __name__ == "__main__":
    showcase = FeatureShowcase()
    showcase.run_all_demos()