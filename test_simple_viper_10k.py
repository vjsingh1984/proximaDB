#!/usr/bin/env python3
"""
Simple test for VIPER 10k corpus without complex configuration
"""

import asyncio
import sys
import time
import numpy as np
from pathlib import Path

# Add Python client to path
sys.path.insert(0, str(Path(__file__).parent / "clients" / "python" / "src"))

from proximadb.grpc_client import ProximaDBClient

async def test_simple_viper():
    print("🚀 Simple VIPER 10k Test")
    print("=" * 50)
    
    try:
        # Test server connection
        print("🔌 Connecting to ProximaDB...")
        client = ProximaDBClient("localhost:5679")
        print("✅ Connected!")
        
        # Test collection creation
        print("🏗️ Creating test collection...")
        collection_name = "simple_viper_test"
        
        try:
            await client.delete_collection(collection_name)
        except:
            pass
            
        await client.create_collection(
            collection_name,
            dimension=768,
            distance_metric=1,  # COSINE
            storage_engine=1    # VIPER
        )
        print("✅ Collection created!")
        
        # Test basic vector insertion
        print("📤 Testing vector insertion...")
        test_vectors = []
        for i in range(100):  # Start with 100 vectors
            vector = np.random.normal(0, 0.4, 768).astype(np.float32)
            vector = vector / np.linalg.norm(vector)
            
            test_vectors.append({
                "id": f"vec_{i:03d}",
                "vector": vector.tolist(),
                "metadata": {"test": "true", "batch": "1"}
            })
        
        result = client.insert_vectors(collection_name, test_vectors)
        print(f"✅ Inserted {result.count} vectors")
        
        # Wait a moment
        await asyncio.sleep(2)
        
        # Test search
        print("🔍 Testing search...")
        query_vector = test_vectors[0]["vector"]
        search_results = client.search_vectors(
            collection_name,
            [query_vector],
            top_k=5
        )
        
        print(f"✅ Search returned {len(search_results)} results")
        for i, result in enumerate(search_results[:3]):
            if hasattr(result, 'id') and hasattr(result, 'score'):
                print(f"   #{i+1}: ID={result.id}, Score={result.score:.4f}")
            else:
                print(f"   #{i+1}: Result={result}")
        
        if len(search_results) > 0:
            print("\n🎉 Basic VIPER functionality working!")
            
            # Test larger batch if basic works - using smaller batches due to message size limits
            print("\n📈 Testing larger batch (1000 vectors in batches of 50)...")
            batch_size = 50
            total_vectors = 1000
            total_inserted = 0
            
            start_time = time.time()
            for batch_start in range(0, total_vectors, batch_size):
                batch_end = min(batch_start + batch_size, total_vectors)
                current_batch = []
                
                for i in range(batch_start, batch_end):
                    vector = np.random.normal(0, 0.4, 768).astype(np.float32)
                    vector = vector / np.linalg.norm(vector)
                    
                    current_batch.append({
                        "id": f"large_vec_{i:04d}",
                        "vector": vector.tolist(),
                        "metadata": {"test": "true", "batch": "large"}
                    })
                
                result = client.insert_vectors(collection_name, current_batch)
                total_inserted += result.count
                
                if batch_start % 200 == 0:
                    print(f"   Inserted {total_inserted}/{total_vectors} vectors...")
            
            insert_time = time.time() - start_time
            print(f"✅ Inserted {total_inserted} vectors in {insert_time:.2f}s ({total_inserted/insert_time:.1f} vec/s)")
            
            await asyncio.sleep(2)
            
            # Test search on larger dataset
            start_time = time.time()
            search_results = client.search_vectors(
                collection_name,
                [query_vector],
                top_k=10
            )
            search_time = time.time() - start_time
            
            print(f"✅ Search on {total_inserted + 100} vectors: {len(search_results)} results in {search_time*1000:.2f}ms")
            
            return True
        else:
            print("❌ Search returned no results - vector discovery issue")
            return False
            
    except Exception as e:
        print(f"❌ Test failed: {e}")
        import traceback
        traceback.print_exc()
        return False

if __name__ == "__main__":
    success = asyncio.run(test_simple_viper())
    sys.exit(0 if success else 1)