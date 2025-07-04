#!/usr/bin/env python3
"""
Comprehensive test with detailed metrics for 1K, 5K, and 25K vectors
"""
import sys
sys.path.insert(0, '/workspace/clients/python/src')

import time
import numpy as np
import uuid
import glob
import os
from datetime import datetime
from proximadb.grpc_client import ProximaDBClient
from tests.python.performance.bert_embedding_utils import (
    generate_text_corpus,
    convert_corpus_to_vectors,
    create_query_texts,
    create_deterministic_embedding
)

class ComprehensiveTest:
    def __init__(self, num_vectors, test_name):
        self.num_vectors = num_vectors
        self.test_name = test_name
        self.client = ProximaDBClient("localhost:5679")
        self.collection_id = f"test_{test_name}_{uuid.uuid4().hex[:8]}"
        self.dimension = 384
        self.metrics = {
            'test_name': test_name,
            'num_vectors': num_vectors,
            'collection_id': self.collection_id,
            'start_time': datetime.now()
        }
        
    def count_parquet_files(self):
        """Count Parquet files for this collection"""
        pattern = f"/workspace/data/disk2/storage/{self.collection_id}/**/*.parquet"
        files = glob.glob(pattern, recursive=True)
        
        total_size = 0
        for f in files:
            try:
                total_size += os.path.getsize(f)
            except:
                pass
                
        return {
            'count': len(files),
            'total_size_mb': total_size / (1024 * 1024),
            'files': files
        }
    
    def run_test(self):
        """Run complete test with metrics"""
        print(f"\n{'='*80}")
        print(f"🚀 Running {self.test_name} Test ({self.num_vectors} vectors)")
        print(f"Collection: {self.collection_id}")
        print(f"{'='*80}")
        
        try:
            # 1. Create collection
            print(f"\n📁 Creating collection...")
            start = time.time()
            self.client.create_collection(
                name=self.collection_id,
                dimension=self.dimension,
                distance_metric=1,  # COSINE
                storage_engine=1    # VIPER
            )
            self.metrics['collection_creation_time'] = time.time() - start
            print(f"✅ Collection created in {self.metrics['collection_creation_time']:.2f}s")
            
            # 2. Generate corpus and embeddings
            print(f"\n📚 Generating {self.num_vectors} documents...")
            start = time.time()
            corpus = generate_text_corpus(self.num_vectors)
            self.metrics['corpus_generation_time'] = time.time() - start
            
            print(f"🧠 Converting to embeddings...")
            start = time.time()
            vectors = convert_corpus_to_vectors(corpus, self.dimension)
            self.metrics['embedding_generation_time'] = time.time() - start
            print(f"✅ Generated {len(vectors)} embeddings in {self.metrics['embedding_generation_time']:.2f}s")
            
            # 3. Insert vectors with monitoring
            print(f"\n🔥 Inserting {len(vectors)} vectors...")
            batch_size = 100 if self.num_vectors <= 5000 else 500
            
            # Monitor initial state
            initial_files = self.count_parquet_files()
            
            start = time.time()
            total_inserted = 0
            batch_times = []
            
            for i in range(0, len(vectors), batch_size):
                batch = vectors[i:i + batch_size]
                batch_start = time.time()
                
                try:
                    result = self.client.insert_vectors(self.collection_id, batch)
                    batch_time = time.time() - batch_start
                    batch_times.append(batch_time)
                    
                    total_inserted += result.count
                    print(f"   Batch {i//batch_size + 1}: {result.count} vectors in {batch_time:.2f}s ({result.count/batch_time:.1f} vec/s)")
                    
                except Exception as e:
                    print(f"   ❌ Batch {i//batch_size + 1} failed: {e}")
                    
            self.metrics['total_insert_time'] = time.time() - start
            self.metrics['vectors_inserted'] = total_inserted
            self.metrics['insert_rate'] = total_inserted / self.metrics['total_insert_time']
            self.metrics['batch_times'] = batch_times
            
            print(f"\n✅ Insertion complete:")
            print(f"   Total vectors: {total_inserted}/{len(vectors)}")
            print(f"   Total time: {self.metrics['total_insert_time']:.2f}s")
            print(f"   Insert rate: {self.metrics['insert_rate']:.1f} vectors/second")
            
            # 4. Monitor flush
            print(f"\n⏳ Waiting for WAL flush (10s)...")
            time.sleep(10)
            
            # Check Parquet files after flush
            final_files = self.count_parquet_files()
            self.metrics['parquet_files_initial'] = initial_files
            self.metrics['parquet_files_final'] = final_files
            
            print(f"\n📊 Parquet File Statistics:")
            print(f"   Initial files: {initial_files['count']}")
            print(f"   Final files: {final_files['count']}")
            print(f"   New files: {final_files['count'] - initial_files['count']}")
            print(f"   Total size: {final_files['total_size_mb']:.2f} MB")
            
            if final_files['count'] > 0:
                print(f"   Avg file size: {final_files['total_size_mb']/final_files['count']:.2f} MB")
                
            # 5. Search performance
            print(f"\n🔍 Testing Search Performance...")
            query_texts = create_query_texts()
            search_times = []
            search_results = []
            
            for i, query in enumerate(query_texts[:5]):
                query_embedding = create_deterministic_embedding(query["text"], self.dimension)
                
                start = time.time()
                try:
                    results = self.client.search_vectors(
                        collection_id=self.collection_id,
                        query_vectors=[query_embedding.tolist()],
                        top_k=10,
                        include_metadata=True
                    )
                    search_time = time.time() - start
                    search_times.append(search_time)
                    
                    result_count = len(results) if results else 0
                    search_results.append(result_count)
                    
                    print(f"   Query {i+1} ({query['expected_category']}): {search_time:.3f}s, {result_count} results")
                    
                    if result_count > 0 and hasattr(results[0], 'score'):
                        print(f"      Top result: score={results[0].score:.4f}")
                        if hasattr(results[0], 'metadata') and results[0].metadata:
                            category = results[0].metadata.get('category', 'unknown')
                            print(f"      Category: {category} (expected: {query['expected_category']})")
                            
                except Exception as e:
                    print(f"   Query {i+1} failed: {e}")
                    search_times.append(0)
                    search_results.append(0)
                    
            self.metrics['search_times'] = search_times
            self.metrics['search_results'] = search_results
            
            if search_times:
                avg_search = sum(search_times) / len(search_times)
                print(f"\n   Average search time: {avg_search:.3f}s")
                print(f"   Search throughput: {1/avg_search:.1f} queries/second")
                
            # 6. Cleanup
            print(f"\n🧹 Cleaning up...")
            self.client.delete_collection(self.collection_id)
            
            self.metrics['end_time'] = datetime.now()
            self.metrics['total_duration'] = (self.metrics['end_time'] - self.metrics['start_time']).total_seconds()
            
            return self.metrics
            
        except Exception as e:
            print(f"\n❌ Test failed: {e}")
            import traceback
            traceback.print_exc()
            self.metrics['error'] = str(e)
            return self.metrics

def main():
    """Run all tests"""
    print("🏃 Comprehensive gRPC Performance Tests with Detailed Metrics")
    print(f"Timestamp: {datetime.now()}")
    
    tests = [
        (1000, "1K"),
        (5000, "5K"),
        (25000, "25K")
    ]
    
    all_results = []
    
    for num_vectors, test_name in tests:
        test = ComprehensiveTest(num_vectors, test_name)
        metrics = test.run_test()
        all_results.append(metrics)
        
        # Brief pause between tests
        time.sleep(3)
        
    # Final report
    print(f"\n{'='*80}")
    print("📊 FINAL COMPREHENSIVE REPORT")
    print(f"{'='*80}")
    
    print(f"\n📈 Performance Summary:")
    print(f"{'Test':<10} {'Vectors':<10} {'Insert Rate':<15} {'Parquet Files':<15} {'Size (MB)':<12} {'Avg Search':<12}")
    print(f"{'-'*10} {'-'*10} {'-'*15} {'-'*15} {'-'*12} {'-'*12}")
    
    for m in all_results:
        if 'error' not in m:
            test = m['test_name']
            vectors = f"{m.get('vectors_inserted', 0):,}"
            rate = f"{m.get('insert_rate', 0):.1f} vec/s"
            files = str(m.get('parquet_files_final', {}).get('count', 0))
            size = f"{m.get('parquet_files_final', {}).get('total_size_mb', 0):.2f}"
            avg_search = f"{sum(m.get('search_times', [])) / len(m.get('search_times', [])) if m.get('search_times') else 0:.3f}s"
            
            print(f"{test:<10} {vectors:<10} {rate:<15} {files:<15} {size:<12} {avg_search:<12}")
    
    print(f"\n✅ All tests completed!")

if __name__ == "__main__":
    main()