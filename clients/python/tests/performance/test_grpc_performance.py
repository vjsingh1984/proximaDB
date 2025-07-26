#!/usr/bin/env python3
"""
gRPC Performance Test for LSM vs VIPER
Simple and direct test using connect_grpc
"""

import time
import numpy as np
import uuid
import json
from typing import Dict, List
from datetime import datetime

from proximadb import connect_grpc
from proximadb.models import (
    CollectionConfig,
    VectorRecord,
    SearchQuery,
    StorageEngine,
    DistanceMetric
)


def test_grpc_performance():
    """Test LSM vs VIPER performance using gRPC"""
    
    # Connect to gRPC
    print("🚀 gRPC Performance Test: LSM vs VIPER")
    print("=" * 80)
    
    client = connect_grpc("grpc://localhost:5679")
    print("✅ Connected via gRPC")
    
    dimension = 384
    test_sizes = [1000, 5000, 25000]
    results = {"LSM": {}, "VIPER": {}}
    
    # Test each engine
    for engine in [StorageEngine.LSM, StorageEngine.VIPER]:
        engine_name = engine.value if hasattr(engine, 'value') else str(engine)
        if isinstance(engine_name, list):
            engine_name = engine_name[0] if engine_name else "unknown"
        print(f"\n{'=' * 80}")
        print(f"📦 Testing {engine_name} Storage Engine")
        print(f"{'=' * 80}")
        
        for size in test_sizes:
            print(f"\n📊 Testing with {size//1000}K vectors")
            
            # Create collection
            collection_name = f"{engine.value}_{size}_{uuid.uuid4().hex[:8]}"
            config = CollectionConfig(
                name=collection_name,
                dimension=dimension,
                distance_metric=DistanceMetric.COSINE,
                storage_engine=engine
            )
            
            try:
                collection = client.create_collection(collection_name, config)
                print(f"✅ Created collection: {collection_name}")
            except Exception as e:
                print(f"❌ Failed to create collection: {e}")
                continue
            
            # Generate vectors
            print(f"\n🔥 Inserting {size:,} vectors...")
            vectors = []
            for i in range(size):
                vector = np.random.rand(dimension).astype(np.float32).tolist()
                record = VectorRecord(
                    id=f"vec_{i}",
                    vector=vector,
                    metadata={"index": i}
                )
                vectors.append(record)
            
            # Insert in batches
            batch_size = min(1000, size)  # Conservative for gRPC
            start_time = time.time()
            successful = 0
            
            for i in range(0, size, batch_size):
                batch = vectors[i:i + batch_size]
                try:
                    response = client.insert_vectors(collection_name, batch)
                    successful += len(batch)
                    if i == 0:  # First batch info
                        print(f"  Batch size: {len(batch)} vectors")
                except Exception as e:
                    print(f"  ❌ Batch failed: {e}")
            
            insert_time = time.time() - start_time
            insert_rate = successful / insert_time if insert_time > 0 else 0
            
            print(f"  ✅ Inserted {successful:,} vectors in {insert_time:.2f}s")
            print(f"  📊 Rate: {insert_rate:.0f} vectors/second")
            
            # Wait for flush
            print("  ⏳ Waiting 3s for flush...")
            time.sleep(3)
            
            # Test search
            print("\n🔍 Testing search...")
            query_vector = np.random.rand(dimension).astype(np.float32).tolist()
            
            # Test k=10
            search_times_10 = []
            for _ in range(5):
                start = time.time()
                results = client.search(collection_name, query_vector, top_k=10)
                search_times_10.append((time.time() - start) * 1000)
            
            # Test k=100
            search_times_100 = []
            for _ in range(5):
                start = time.time()
                results = client.search(collection_name, query_vector, top_k=100)
                search_times_100.append((time.time() - start) * 1000)
            
            avg_10 = np.mean(search_times_10)
            avg_100 = np.mean(search_times_100)
            
            print(f"  k=10:  {avg_10:.2f}ms (avg of 5 runs)")
            print(f"  k=100: {avg_100:.2f}ms (avg of 5 runs)")
            
            # Store results
            engine_key = engine_name.upper() if isinstance(engine_name, str) else "UNKNOWN"
            results[engine_key][f"{size//1000}K"] = {
                "insert_rate": insert_rate,
                "search_k10_ms": avg_10,
                "search_k100_ms": avg_100
            }
            
            # Cleanup
            try:
                client.delete_collection(collection_name)
            except:
                pass
    
    # Print summary
    print(f"\n\n{'=' * 80}")
    print("📈 PERFORMANCE SUMMARY")
    print(f"{'=' * 80}\n")
    
    print(f"{'Engine':<10} {'Size':<10} {'Insert Rate':<15} {'Search k=10':<12} {'Search k=100'}")
    print("-" * 70)
    
    for engine in ["LSM", "VIPER"]:
        for size in ["1K", "5K", "25K"]:
            if size in results[engine]:
                data = results[engine][size]
                print(f"{engine:<10} {size:<10} {data['insert_rate']:>8.0f} vec/s  "
                      f"{data['search_k10_ms']:>6.2f}ms     {data['search_k100_ms']:>6.2f}ms")
    
    # Calculate averages
    print("\n🎯 Analysis:")
    
    lsm_rates = [results["LSM"][s]["insert_rate"] for s in ["1K", "5K", "25K"] if s in results["LSM"]]
    viper_rates = [results["VIPER"][s]["insert_rate"] for s in ["1K", "5K", "25K"] if s in results["VIPER"]]
    
    if lsm_rates and viper_rates:
        print(f"\n  Insert Performance:")
        print(f"  - LSM average: {np.mean(lsm_rates):,.0f} vectors/second")
        print(f"  - VIPER average: {np.mean(viper_rates):,.0f} vectors/second")
        print(f"  - Ratio: LSM is {np.mean(lsm_rates)/np.mean(viper_rates):.2f}x VIPER speed")
    
    print(f"\n  Key Insights:")
    print(f"  - Both engines use same WAL & global memtable")
    print(f"  - Insert performance should be similar")
    print(f"  - Differences come from flush/compaction")
    
    # Save results
    timestamp = int(time.time())
    filename = f"grpc_performance_{timestamp}.json"
    
    report = {
        "test": "gRPC LSM vs VIPER",
        "timestamp": timestamp,
        "datetime": datetime.now().isoformat(),
        "results": results
    }
    
    with open(filename, 'w') as f:
        json.dump(report, f, indent=2)
    
    print(f"\n💾 Results saved to: {filename}")


if __name__ == "__main__":
    test_grpc_performance()
    print("\n✅ Test completed!")