#!/usr/bin/env python3
"""
ProximaDB Python SDK - Bulk Operations Performance Benchmark
Extracted from test suite to run as standalone performance benchmark
"""

import time
import sys
import os
import numpy as np
from typing import List, Dict, Any

# Add SDK to path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', '..', '..', 'clients', 'python', 'src'))

from proximadb import ProximaDBClient, connect_rest, connect_grpc
from proximadb.models import CollectionConfig, StorageEngine

class BulkOperationsBenchmark:
    """Standalone bulk operations performance benchmark"""
    
    def __init__(self, rest_url="http://localhost:5678", grpc_url="http://localhost:5679"):
        self.rest_client = connect_rest(rest_url)
        self.grpc_client = connect_grpc(grpc_url)
        
    def setup_test_collection(self, name: str, dimension: int = 512, engine: StorageEngine = StorageEngine.VIPER) -> str:
        """Create test collection optimized for bulk operations"""
        config = CollectionConfig(
            name=name,
            dimension=dimension,
            distance_metric="cosine",
            description=f"Bulk operations benchmark - {engine.value} engine",
            storage_engine=engine
        )
        
        collection = self.rest_client.create_collection(name, config)
        return name
    
    def benchmark_bulk_insert(self, collection_name: str, total_vectors: int, batch_size: int, dimension: int, protocol: str = "rest"):
        """Benchmark bulk insert operations"""
        print(f"\n📥 Bulk Insert Benchmark - {protocol.upper()}")
        print(f"  Total vectors: {total_vectors}")
        print(f"  Batch size: {batch_size}")
        print(f"  Dimension: {dimension}")
        
        client = self.rest_client if protocol == "rest" else self.grpc_client
        
        total_time = 0
        successful_inserts = 0
        
        for batch_start in range(0, total_vectors, batch_size):
            batch_end = min(batch_start + batch_size, total_vectors)
            batch_vectors = []
            batch_ids = []
            batch_metadata = []
            
            # Generate batch data
            for i in range(batch_start, batch_end):
                vector = np.random.normal(0, 1, dimension).astype(np.float32).tolist()
                batch_vectors.append(vector)
                batch_ids.append(f"bulk_vector_{i}")
                batch_metadata.append({
                    "index": i,
                    "batch": batch_start // batch_size,
                    "category": f"group_{i % 20}",
                    "benchmark": "bulk_insert"
                })
            
            # Time the batch insert
            start_time = time.time()
            try:
                result = client.insert_vectors(collection_name, batch_vectors, batch_ids, batch_metadata)
                batch_time = time.time() - start_time
                total_time += batch_time
                successful_inserts += len(batch_vectors)
                
                if (batch_start // batch_size) % 10 == 0:
                    rate = len(batch_vectors) / batch_time if batch_time > 0 else 0
                    print(f"  Batch {batch_start//batch_size + 1}: {len(batch_vectors)} vectors in {batch_time:.3f}s ({rate:.0f} vec/s)")
                    
            except Exception as e:
                print(f"  ❌ Batch {batch_start//batch_size + 1} failed: {e}")
        
        # Calculate final statistics
        overall_rate = successful_inserts / total_time if total_time > 0 else 0
        
        print(f"✅ {protocol.upper()} Bulk Insert Summary:")
        print(f"  Successfully inserted: {successful_inserts}/{total_vectors} vectors")
        print(f"  Total time: {total_time:.2f}s") 
        print(f"  Average rate: {overall_rate:.0f} vectors/second")
        
        return {
            'protocol': protocol,
            'successful_inserts': successful_inserts,
            'total_time': total_time,
            'vectors_per_second': overall_rate
        }
    
    def benchmark_upsert_performance(self, collection_name: str, vector_count: int, dimension: int):
        """Benchmark upsert (update) performance"""
        print(f"\n🔄 Upsert Performance Benchmark")
        print(f"  Updating {vector_count} existing vectors")
        
        # Generate updated vectors
        updated_vectors = []
        updated_ids = []
        updated_metadata = []
        
        for i in range(vector_count):
            vector = np.random.normal(0, 1, dimension).astype(np.float32).tolist()
            updated_vectors.append(vector)
            updated_ids.append(f"bulk_vector_{i}")
            updated_metadata.append({
                "index": i,
                "version": 2,
                "updated": True,
                "benchmark": "upsert"
            })
        
        # REST upsert benchmark
        rest_start = time.time()
        try:
            rest_result = self.rest_client.insert_vectors(
                collection_name, 
                updated_vectors, 
                updated_ids, 
                updated_metadata
            )
            rest_time = time.time() - rest_start
            rest_rate = vector_count / rest_time if rest_time > 0 else 0
            print(f"  REST upsert: {vector_count} vectors in {rest_time:.2f}s ({rest_rate:.0f} vec/s)")
        except Exception as e:
            print(f"  ❌ REST upsert failed: {e}")
            rest_time = 0
            rest_rate = 0
        
        # gRPC upsert benchmark (new vectors to avoid conflicts)
        grpc_vectors = []
        grpc_ids = []
        grpc_metadata = []
        
        for i in range(vector_count):
            vector = np.random.normal(0, 1, dimension).astype(np.float32).tolist()
            grpc_vectors.append(vector)
            grpc_ids.append(f"bulk_grpc_vector_{i}")
            grpc_metadata.append({
                "index": i + vector_count,
                "version": 1,
                "protocol": "grpc",
                "benchmark": "upsert"
            })
        
        grpc_start = time.time()
        try:
            grpc_result = self.grpc_client.insert_vectors(
                collection_name,
                grpc_vectors,
                grpc_ids, 
                grpc_metadata
            )
            grpc_time = time.time() - grpc_start
            grpc_rate = vector_count / grpc_time if grpc_time > 0 else 0
            print(f"  gRPC insert: {vector_count} vectors in {grpc_time:.2f}s ({grpc_rate:.0f} vec/s)")
        except Exception as e:
            print(f"  ❌ gRPC insert failed: {e}")
            grpc_time = 0
            grpc_rate = 0
        
        return {
            'rest_upsert_rate': rest_rate,
            'grpc_insert_rate': grpc_rate,
            'performance_ratio': grpc_rate / rest_rate if rest_rate > 0 else 0
        }
    
    def benchmark_storage_engines(self, dimension: int = 384, vector_count: int = 2000):
        """Compare performance across storage engines"""
        print(f"\n🏗️ Storage Engine Performance Comparison")
        
        engines = [StorageEngine.VIPER, StorageEngine.SST]
        results = {}
        
        for engine in engines:
            collection_name = f"storage_bench_{engine.value}_{int(time.time())}"
            print(f"\n📊 Testing {engine.value} engine...")
            
            try:
                # Setup collection
                self.setup_test_collection(collection_name, dimension, engine)
                
                # Benchmark insert performance
                insert_result = self.benchmark_bulk_insert(
                    collection_name, 
                    vector_count, 
                    batch_size=100, 
                    dimension=dimension, 
                    protocol="rest"
                )
                
                # Small delay for indexing
                time.sleep(1)
                
                # Benchmark search performance
                search_times = []
                for _ in range(20):
                    query_vector = np.random.normal(0, 1, dimension).astype(np.float32).tolist()
                    start_time = time.time()
                    self.rest_client.search_vectors(collection_name, query_vector, top_k=10)
                    search_times.append(time.time() - start_time)
                
                avg_search_time = np.mean(search_times) * 1000  # Convert to ms
                
                results[engine.value] = {
                    'insert_rate': insert_result['vectors_per_second'],
                    'avg_search_time_ms': avg_search_time
                }
                
                print(f"  {engine.value} Results:")
                print(f"    Insert rate: {insert_result['vectors_per_second']:.0f} vectors/s")
                print(f"    Avg search time: {avg_search_time:.2f}ms")
                
            except Exception as e:
                print(f"  ❌ {engine.value} benchmark failed: {e}")
                results[engine.value] = {'insert_rate': 0, 'avg_search_time_ms': float('inf')}
            
            finally:
                try:
                    self.rest_client.delete_collection(collection_name)
                except:
                    pass
        
        # Compare results
        print(f"\n🏆 Storage Engine Comparison:")
        for engine_name, metrics in results.items():
            print(f"  {engine_name}:")
            print(f"    Insert: {metrics['insert_rate']:.0f} vec/s")
            print(f"    Search: {metrics['avg_search_time_ms']:.2f}ms")
        
        return results
    
    def cleanup(self, collection_name: str):
        """Clean up test collection"""
        try:
            self.rest_client.delete_collection(collection_name)
            print(f"🧹 Cleaned up collection: {collection_name}")
        except Exception as e:
            print(f"⚠️ Cleanup warning: {e}")
    
    def run_full_benchmark(self):
        """Run complete bulk operations benchmark"""
        print("🚀 ProximaDB Python SDK - Bulk Operations Benchmark")
        print("=" * 60)
        
        collection_name = f"bulk_ops_benchmark_{int(time.time())}"
        dimension = 512
        vector_count = 2000
        batch_size = 200
        
        try:
            # Setup
            self.setup_test_collection(collection_name, dimension)
            
            # REST bulk insert benchmark
            rest_results = self.benchmark_bulk_insert(
                collection_name, vector_count, batch_size, dimension, "rest"
            )
            
            # gRPC bulk insert benchmark (new collection to avoid conflicts)
            grpc_collection = f"bulk_ops_grpc_{int(time.time())}"
            self.setup_test_collection(grpc_collection, dimension)
            
            grpc_results = self.benchmark_bulk_insert(
                grpc_collection, vector_count, batch_size, dimension, "grpc"
            )
            
            # Upsert performance (using REST collection)
            upsert_results = self.benchmark_upsert_performance(collection_name, vector_count//2, dimension)
            
            # Storage engine comparison
            storage_results = self.benchmark_storage_engines(dimension, vector_count//2)
            
            # Final summary
            print(f"\n🎯 Bulk Operations Benchmark Summary:")
            print(f"Dataset: {vector_count} vectors, {dimension}D, batch size {batch_size}")
            print(f"REST insert rate: {rest_results['vectors_per_second']:.0f} vectors/s")
            print(f"gRPC insert rate: {grpc_results['vectors_per_second']:.0f} vectors/s")
            print(f"gRPC advantage: {grpc_results['vectors_per_second']/rest_results['vectors_per_second']:.2f}x")
            
            # Cleanup
            self.cleanup(grpc_collection)
            
        finally:
            self.cleanup(collection_name)

if __name__ == "__main__":
    try:
        benchmark = BulkOperationsBenchmark()
        benchmark.run_full_benchmark()
    except KeyboardInterrupt:
        print("\n⏹️ Benchmark interrupted by user")
    except Exception as e:
        print(f"❌ Benchmark failed: {e}")
        import traceback
        traceback.print_exc()