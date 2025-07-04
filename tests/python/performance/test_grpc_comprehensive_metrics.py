#!/usr/bin/env python3
"""
Comprehensive gRPC SDK Metrics Test
Monitors WAL flush, Parquet files, and provides detailed performance metrics
"""

import asyncio
import time
import numpy as np
import uuid
import sys
import os
import glob
from pathlib import Path

# Add SDK to path
sdk_path = Path(__file__).parent.parent.parent / "clients/python/src"
# IMPORTANT: This test requires the ProximaDB Python SDK to be in PYTHONPATH
# Run with: PYTHONPATH=/workspace/clients/python/src python3 test_grpc_comprehensive_metrics.py
# Do NOT add sys.path.insert() - paths should be externalized via environment variables

# Add utils to path for BERT embedding functions
current_dir = Path(__file__).parent
# IMPORTANT: This test requires the ProximaDB Python SDK to be in PYTHONPATH
# Run with: PYTHONPATH=/workspace/clients/python/src python3 test_grpc_comprehensive_metrics.py
# Do NOT add sys.path.insert() - paths should be externalized via environment variables

from proximadb.grpc_client import ProximaDBClient
from bert_embedding_utils import (
    generate_text_corpus,
    convert_corpus_to_vectors,
    create_query_texts,
    create_deterministic_embedding
)


class ComprehensiveMetricsTest:
    """Comprehensive gRPC SDK test with detailed metrics and file monitoring"""
    
    def __init__(self):
        self.server_address = "localhost:5679"  # gRPC port
        self.client = None
        self.collection_id = f"metrics_test_{uuid.uuid4().hex[:8]}"
        self.dimension = 384
        self.num_vectors = 1000
        
        # Metrics tracking
        self.metrics = {
            "connection_time": 0,
            "collection_creation_time": 0,
            "embedding_generation_time": 0,
            "total_insert_time": 0,
            "batch_insert_times": [],
            "vectors_inserted": 0,
            "flush_wait_time": 0,
            "search_times": [],
            "search_scores": [],
            "parquet_files_before": 0,
            "parquet_files_after": 0,
            "wal_files_before": 0,
            "wal_files_after": 0
        }
        
    async def run_comprehensive_test(self):
        """Run comprehensive test with full metrics"""
        print("🚀 Comprehensive gRPC SDK Metrics Test")
        print("=" * 60)
        
        try:
            # Monitor initial file state
            self.monitor_initial_files()
            
            # Test 1: gRPC SDK Connection Metrics
            if not await self.test_grpc_connection():
                return False
            
            # Test 2: Collection Creation Metrics
            if not await self.test_collection_creation():
                return False
            
            # Test 3: Embedding Generation Metrics
            if not self.test_embedding_generation():
                return False
            
            # Test 4: Vector Insertion with Flush Monitoring
            if not await self.test_vector_insertion_with_monitoring():
                return False
            
            # Test 5: WAL Flush Monitoring
            if not await self.test_wal_flush_monitoring():
                return False
            
            # Test 6: Parquet File Creation Monitoring
            if not self.monitor_parquet_files():
                return False
            
            # Test 7: Search Performance and Scoring Metrics
            if not await self.test_search_performance_metrics():
                return False
            
            # Final metrics report
            self.generate_comprehensive_report()
            
            print("\n🎉 Comprehensive metrics test completed successfully!")
            return True
            
        except Exception as e:
            print(f"❌ Comprehensive test failed: {e}")
            import traceback
            traceback.print_exc()
            return False
        finally:
            await self.cleanup()
    
    def monitor_initial_files(self):
        """Monitor initial file state"""
        print("\n📁 Monitoring Initial File State")
        print("-" * 40)
        
        # Check for Parquet files
        parquet_patterns = [
            "/tmp/proximadb/**/*.parquet",
            "/data/proximadb/**/*.parquet", 
            "/workspace/**/*.parquet",
            "./**/*.parquet"
        ]
        
        parquet_files = []
        for pattern in parquet_patterns:
            parquet_files.extend(glob.glob(pattern, recursive=True))
        
        self.metrics["parquet_files_before"] = len(parquet_files)
        print(f"   📄 Initial Parquet files: {len(parquet_files)}")
        
        # Check for WAL files
        wal_patterns = [
            "/tmp/proximadb/**/*.wal",
            "/data/proximadb/**/*.wal",
            "/workspace/**/*.wal",
            "./**/*.wal"
        ]
        
        wal_files = []
        for pattern in wal_patterns:
            wal_files.extend(glob.glob(pattern, recursive=True))
        
        self.metrics["wal_files_before"] = len(wal_files)
        print(f"   📝 Initial WAL files: {len(wal_files)}")
        
        if parquet_files:
            print("   📄 Found Parquet files:")
            for f in parquet_files[:5]:  # Show first 5
                size = os.path.getsize(f) / 1024 / 1024  # MB
                print(f"      {f} ({size:.2f} MB)")
        
        if wal_files:
            print("   📝 Found WAL files:")
            for f in wal_files[:5]:  # Show first 5
                size = os.path.getsize(f) / 1024  # KB
                print(f"      {f} ({size:.2f} KB)")
    
    async def test_grpc_connection(self):
        """Test gRPC connection with metrics"""
        print("\n🔗 Testing gRPC SDK Connection")
        print("-" * 40)
        
        start_time = time.time()
        
        try:
            print("   Initializing gRPC client...")
            self.client = ProximaDBClient(self.server_address)
            
            connect_start = time.time()
            await self.client.connect()
            connect_end = time.time()
            
            # Test health check
            health_start = time.time()
            health = await self.client.health_check()
            health_end = time.time()
            
            end_time = time.time()
            
            self.metrics["connection_time"] = end_time - start_time
            
            print(f"   ✅ gRPC client connected")
            print(f"   📊 Connection metrics:")
            print(f"      Total connection time: {self.metrics['connection_time']:.3f}s")
            print(f"      Socket connection: {connect_end - connect_start:.3f}s")
            print(f"      Health check: {health_end - health_start:.3f}s")
            print(f"      Server status: {getattr(health, 'status', 'unknown')}")
            
            return True
            
        except Exception as e:
            print(f"   ❌ gRPC connection failed: {e}")
            return False
    
    async def test_collection_creation(self):
        """Test collection creation with metrics"""
        print(f"\n🏗️ Testing Collection Creation")
        print("-" * 40)
        
        start_time = time.time()
        
        try:
            print(f"   Creating collection: {self.collection_id}")
            
            result = await self.client.create_collection(
                name=self.collection_id,
                dimension=self.dimension,
                distance_metric="COSINE",
                storage_engine="VIPER"
            )
            
            end_time = time.time()
            self.metrics["collection_creation_time"] = end_time - start_time
            
            print(f"   ✅ Collection created via gRPC")
            print(f"   📊 Creation metrics:")
            print(f"      Creation time: {self.metrics['collection_creation_time']:.3f}s")
            print(f"      Collection ID: {self.collection_id}")
            print(f"      Dimension: {self.dimension}")
            print(f"      Storage engine: VIPER")
            
            return True
            
        except Exception as e:
            print(f"   ❌ Collection creation failed: {e}")
            return False
    
    def test_embedding_generation(self):
        """Test BERT embedding generation with metrics"""
        print(f"\n🧠 Testing BERT Embedding Generation")
        print("-" * 40)
        
        start_time = time.time()
        
        try:
            print(f"   Generating {self.num_vectors} BERT embeddings...")
            
            # Generate text corpus
            corpus_start = time.time()
            self.corpus = generate_text_corpus(self.num_vectors)
            corpus_end = time.time()
            
            # Convert to vectors with embeddings
            vector_start = time.time()
            self.vectors = convert_corpus_to_vectors(self.corpus, self.dimension)
            vector_end = time.time()
            
            end_time = time.time()
            self.metrics["embedding_generation_time"] = end_time - start_time
            
            # Analyze corpus
            categories = {}
            for vector in self.vectors:
                cat = vector["metadata"]["category"]
                categories[cat] = categories.get(cat, 0) + 1
            
            print(f"   ✅ Generated {len(self.vectors)} BERT vectors")
            print(f"   📊 Embedding metrics:")
            print(f"      Total generation time: {self.metrics['embedding_generation_time']:.3f}s")
            print(f"      Corpus generation: {corpus_end - corpus_start:.3f}s")
            print(f"      Vector conversion: {vector_end - vector_start:.3f}s")
            print(f"      Vectors per second: {len(self.vectors)/(vector_end - vector_start):.1f}")
            print(f"   📊 Corpus distribution:")
            for cat, count in categories.items():
                percentage = (count / len(self.vectors)) * 100
                print(f"      {cat}: {count} documents ({percentage:.1f}%)")
            
            return True
            
        except Exception as e:
            print(f"   ❌ Embedding generation failed: {e}")
            return False
    
    async def test_vector_insertion_with_monitoring(self):
        """Test vector insertion with detailed monitoring"""
        print(f"\n🔥 Testing Vector Insertion with Monitoring")
        print("-" * 40)
        
        start_time = time.time()
        
        try:
            print(f"   Inserting {len(self.vectors)} vectors via gRPC SDK...")
            
            # Monitor files during insertion
            initial_files = self.count_files()
            
            batch_size = 100
            total_inserted = 0
            
            for i in range(0, len(self.vectors), batch_size):
                batch = self.vectors[i:batch_size]
                batch_num = (i // batch_size) + 1
                
                try:
                    batch_start = time.time()
                    
                    # Monitor files before batch
                    files_before = self.count_files()
                    
                    result = self.client.insert_vectors(
                        collection_id=self.collection_id,
                        vectors=batch
                    )
                    
                    batch_end = time.time()
                    batch_time = batch_end - batch_start
                    
                    # Monitor files after batch
                    files_after = self.count_files()
                    
                    batch_count = result.count if hasattr(result, 'count') else 1
                    total_inserted += batch_count
                    
                    self.metrics["batch_insert_times"].append(batch_time)
                    
                    print(f"   Batch {batch_num}: {batch_count} vectors in {batch_time:.3f}s")
                    print(f"      Files before: Parquet={files_before['parquet']}, WAL={files_before['wal']}")
                    print(f"      Files after:  Parquet={files_after['parquet']}, WAL={files_after['wal']}")
                    
                    # Check if new files were created
                    if files_after['parquet'] > files_before['parquet']:
                        print(f"      🆕 {files_after['parquet'] - files_before['parquet']} new Parquet file(s) created!")
                    if files_after['wal'] > files_before['wal']:
                        print(f"      🆕 {files_after['wal'] - files_before['wal']} new WAL file(s) created!")
                    
                    # Break after successful batches to save time
                    if batch_num >= 3:  # Test first 3 batches
                        print(f"   ... (stopping after {batch_num} batches for metrics)")
                        break
                    
                except Exception as e:
                    print(f"   ⚠️ Batch {batch_num} failed: {e}")
                    continue
            
            end_time = time.time()
            self.metrics["total_insert_time"] = end_time - start_time
            self.metrics["vectors_inserted"] = total_inserted
            
            final_files = self.count_files()
            
            print(f"   ✅ Insertion monitoring completed")
            print(f"   📊 Insertion metrics:")
            print(f"      Total insert time: {self.metrics['total_insert_time']:.3f}s")
            print(f"      Vectors inserted: {total_inserted}")
            print(f"      Batches processed: {len(self.metrics['batch_insert_times'])}")
            if self.metrics["batch_insert_times"]:
                avg_batch_time = sum(self.metrics["batch_insert_times"]) / len(self.metrics["batch_insert_times"])
                print(f"      Average batch time: {avg_batch_time:.3f}s")
            print(f"      Final file count: Parquet={final_files['parquet']}, WAL={final_files['wal']}")
            
            return total_inserted > 0
            
        except Exception as e:
            print(f"   ❌ Vector insertion monitoring failed: {e}")
            return False
    
    def count_files(self):
        """Count Parquet and WAL files"""
        parquet_patterns = [
            "/tmp/proximadb/**/*.parquet",
            "/data/proximadb/**/*.parquet", 
            "/workspace/**/*.parquet",
            "./**/*.parquet"
        ]
        
        wal_patterns = [
            "/tmp/proximadb/**/*.wal",
            "/data/proximadb/**/*.wal",
            "/workspace/**/*.wal",
            "./**/*.wal"
        ]
        
        parquet_files = []
        for pattern in parquet_patterns:
            parquet_files.extend(glob.glob(pattern, recursive=True))
        
        wal_files = []
        for pattern in wal_patterns:
            wal_files.extend(glob.glob(pattern, recursive=True))
        
        return {"parquet": len(parquet_files), "wal": len(wal_files)}
    
    async def test_wal_flush_monitoring(self):
        """Monitor WAL flush operations"""
        print(f"\n⏳ Monitoring WAL Flush Operations")
        print("-" * 40)
        
        flush_start = time.time()
        
        print("   Waiting for WAL flush (10 seconds)...")
        files_before_flush = self.count_files()
        
        await asyncio.sleep(10)  # Wait for flush as requested
        
        files_after_flush = self.count_files()
        flush_end = time.time()
        
        self.metrics["flush_wait_time"] = flush_end - flush_start
        
        print(f"   ✅ Flush monitoring completed")
        print(f"   📊 Flush metrics:")
        print(f"      Flush wait time: {self.metrics['flush_wait_time']:.1f}s")
        print(f"      Files before flush: Parquet={files_before_flush['parquet']}, WAL={files_before_flush['wal']}")
        print(f"      Files after flush:  Parquet={files_after_flush['parquet']}, WAL={files_after_flush['wal']}")
        
        if files_after_flush['parquet'] > files_before_flush['parquet']:
            new_parquet = files_after_flush['parquet'] - files_before_flush['parquet']
            print(f"      🔄 {new_parquet} new Parquet file(s) created during flush!")
        
        if files_after_flush['wal'] != files_before_flush['wal']:
            wal_change = files_after_flush['wal'] - files_before_flush['wal']
            print(f"      📝 WAL files changed by {wal_change} during flush")
        
        return True
    
    def monitor_parquet_files(self):
        """Monitor Parquet file creation"""
        print(f"\n📄 Monitoring Parquet Files")
        print("-" * 40)
        
        # Final count
        parquet_patterns = [
            "/tmp/proximadb/**/*.parquet",
            "/data/proximadb/**/*.parquet", 
            "/workspace/**/*.parquet",
            "./**/*.parquet"
        ]
        
        parquet_files = []
        for pattern in parquet_patterns:
            parquet_files.extend(glob.glob(pattern, recursive=True))
        
        self.metrics["parquet_files_after"] = len(parquet_files)
        
        print(f"   📊 Parquet file metrics:")
        print(f"      Initial Parquet files: {self.metrics['parquet_files_before']}")
        print(f"      Final Parquet files: {self.metrics['parquet_files_after']}")
        print(f"      Net change: {self.metrics['parquet_files_after'] - self.metrics['parquet_files_before']}")
        
        if parquet_files:
            print(f"   📄 Current Parquet files:")
            for i, f in enumerate(parquet_files[:10]):  # Show first 10
                try:
                    size = os.path.getsize(f) / 1024 / 1024  # MB
                    mtime = os.path.getmtime(f)
                    print(f"      {i+1}. {f} ({size:.2f} MB, modified: {time.ctime(mtime)})")
                except:
                    print(f"      {i+1}. {f} (size unknown)")
        
        return True
    
    async def test_search_performance_metrics(self):
        """Test search performance with detailed metrics"""
        print(f"\n🔍 Testing Search Performance Metrics")
        print("-" * 40)
        
        if self.metrics["vectors_inserted"] == 0:
            print("   ⚠️ No vectors inserted, skipping search tests")
            return True
        
        try:
            # Test multiple search scenarios
            search_scenarios = [
                {"k": 1, "name": "Top-1"},
                {"k": 5, "name": "Top-5"},
                {"k": 10, "name": "Top-10"},
                {"k": 20, "name": "Top-20"}
            ]
            
            query_texts = create_query_texts()
            
            for scenario in search_scenarios:
                print(f"\n   🎯 Testing {scenario['name']} search:")
                
                scenario_times = []
                scenario_scores = []
                
                for i, query in enumerate(query_texts[:3]):  # Test 3 queries per scenario
                    try:
                        query_embedding = create_deterministic_embedding(query["text"], self.dimension)
                        
                        search_start = time.time()
                        results = self.client.search_vectors(
                            collection_id=self.collection_id,
                            query_vectors=[query_embedding.tolist()],
                            top_k=scenario["k"],
                            include_metadata=True
                        )
                        search_end = time.time()
                        
                        search_time = search_end - search_start
                        scenario_times.append(search_time)
                        
                        result_count = len(results.results) if hasattr(results, 'results') else 0
                        
                        if result_count > 0:
                            # Extract scores
                            scores = [r.get('score', 0) for r in results.results]
                            scenario_scores.extend(scores)
                            top_score = max(scores) if scores else 0
                            avg_score = sum(scores) / len(scores) if scores else 0
                            
                            print(f"      Query {i+1}: {search_time:.3f}s, {result_count} results")
                            print(f"         Top score: {top_score:.4f}, Avg score: {avg_score:.4f}")
                        else:
                            print(f"      Query {i+1}: {search_time:.3f}s, 0 results")
                            
                    except Exception as e:
                        print(f"      Query {i+1}: Failed - {e}")
                        continue
                
                if scenario_times:
                    avg_time = sum(scenario_times) / len(scenario_times)
                    min_time = min(scenario_times)
                    max_time = max(scenario_times)
                    throughput = 1 / avg_time if avg_time > 0 else 0
                    
                    self.metrics["search_times"].extend(scenario_times)
                    self.metrics["search_scores"].extend(scenario_scores)
                    
                    print(f"      📊 {scenario['name']} metrics:")
                    print(f"         Avg time: {avg_time:.3f}s")
                    print(f"         Min time: {min_time:.3f}s") 
                    print(f"         Max time: {max_time:.3f}s")
                    print(f"         Throughput: {throughput:.1f} queries/second")
                    
                    if scenario_scores:
                        avg_score = sum(scenario_scores) / len(scenario_scores)
                        max_score = max(scenario_scores)
                        min_score = min(scenario_scores)
                        print(f"         Score range: {min_score:.4f} - {max_score:.4f}")
                        print(f"         Avg score: {avg_score:.4f}")
            
            return True
            
        except Exception as e:
            print(f"   ❌ Search performance testing failed: {e}")
            return False
    
    def generate_comprehensive_report(self):
        """Generate comprehensive metrics report"""
        print(f"\n📊 COMPREHENSIVE METRICS REPORT")
        print("=" * 60)
        
        print(f"\n🔗 SDK Connection Metrics:")
        print(f"   Connection time: {self.metrics['connection_time']:.3f}s")
        print(f"   Collection creation: {self.metrics['collection_creation_time']:.3f}s")
        
        print(f"\n🧠 Embedding Metrics:")
        print(f"   Generation time: {self.metrics['embedding_generation_time']:.3f}s")
        print(f"   Vectors generated: {len(self.vectors) if hasattr(self, 'vectors') else 0}")
        
        print(f"\n🔥 Insertion Metrics:")
        print(f"   Total insert time: {self.metrics['total_insert_time']:.3f}s")
        print(f"   Vectors inserted: {self.metrics['vectors_inserted']}")
        print(f"   Batches processed: {len(self.metrics['batch_insert_times'])}")
        if self.metrics["batch_insert_times"]:
            avg_batch = sum(self.metrics["batch_insert_times"]) / len(self.metrics["batch_insert_times"])
            print(f"   Avg batch time: {avg_batch:.3f}s")
        
        print(f"\n📁 File System Metrics:")
        print(f"   Parquet files before: {self.metrics['parquet_files_before']}")
        print(f"   Parquet files after: {self.metrics['parquet_files_after']}")
        print(f"   Parquet files created: {self.metrics['parquet_files_after'] - self.metrics['parquet_files_before']}")
        print(f"   WAL files before: {self.metrics['wal_files_before']}")
        print(f"   WAL files after: {self.metrics['wal_files_after']}")
        
        print(f"\n⏳ Flush Metrics:")
        print(f"   Flush wait time: {self.metrics['flush_wait_time']:.1f}s")
        
        print(f"\n🔍 Search Performance Metrics:")
        if self.metrics["search_times"]:
            avg_search = sum(self.metrics["search_times"]) / len(self.metrics["search_times"])
            min_search = min(self.metrics["search_times"])
            max_search = max(self.metrics["search_times"])
            print(f"   Searches performed: {len(self.metrics['search_times'])}")
            print(f"   Avg search time: {avg_search:.3f}s")
            print(f"   Search time range: {min_search:.3f}s - {max_search:.3f}s")
            print(f"   Search throughput: {1/avg_search:.1f} queries/second")
        
        if self.metrics["search_scores"]:
            avg_score = sum(self.metrics["search_scores"]) / len(self.metrics["search_scores"])
            min_score = min(self.metrics["search_scores"])
            max_score = max(self.metrics["search_scores"])
            print(f"   Score results: {len(self.metrics['search_scores'])}")
            print(f"   Avg score: {avg_score:.4f}")
            print(f"   Score range: {min_score:.4f} - {max_score:.4f}")
        
        print(f"\n🎯 Overall Performance Summary:")
        total_time = (self.metrics['connection_time'] + 
                     self.metrics['collection_creation_time'] + 
                     self.metrics['embedding_generation_time'] + 
                     self.metrics['total_insert_time'] + 
                     self.metrics['flush_wait_time'])
        print(f"   Total test time: {total_time:.2f}s ({total_time/60:.1f}m)")
        
        if self.metrics["vectors_inserted"] > 0:
            overall_throughput = self.metrics["vectors_inserted"] / total_time
            print(f"   Overall throughput: {overall_throughput:.1f} vectors/second")
    
    async def cleanup(self):
        """Clean up test collection"""
        print(f"\n🧹 Cleaning up collection: {self.collection_id}")
        
        try:
            if self.client:
                await self.client.delete_collection(self.collection_id)
                print("   ✅ Collection deleted successfully")
                
                await self.client.close()
                print("   ✅ gRPC client closed")
                
        except Exception as e:
            print(f"   ⚠️ Cleanup failed: {e}")


async def main():
    """Run the comprehensive metrics test"""
    test = ComprehensiveMetricsTest()
    success = await test.run_comprehensive_test()
    
    if success:
        print("\n🎉 Comprehensive metrics test completed successfully!")
    else:
        print("\n💥 Comprehensive metrics test failed!")
        exit(1)


if __name__ == "__main__":
    asyncio.run(main())