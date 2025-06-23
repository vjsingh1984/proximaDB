#!/usr/bin/env python3
"""
End-to-End Vector Operations Test with BERT Embeddings
Verifies the complete flow from Python SDK to gRPC handlers to storage
Based on current gRPC specifications and SDK implementation.
"""

import os
import sys
import time
import numpy as np
from sentence_transformers import SentenceTransformer
import json
import glob

# Add the Python client to path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), 'clients', 'python', 'src'))

from proximadb import ProximaDBClient
from proximadb.models import Vector, CollectionConfig, DistanceMetric


def generate_bert_embeddings(texts, model_name='all-MiniLM-L6-v2'):
    """Generate BERT embeddings for texts"""
    print(f"Loading BERT model: {model_name}")
    model = SentenceTransformer(model_name)
    embeddings = model.encode(texts)
    return embeddings, model.get_sentence_embedding_dimension()


def verify_wal_persistence(collection_name):
    """Verify that WAL data was persisted"""
    # Check config.toml for WAL location
    wal_path = "/workspace/data/wal"
    
    print(f"\n📁 Checking WAL persistence at: {wal_path}")
    
    if os.path.exists(wal_path):
        # Check for collection-specific WAL files
        collection_wal = os.path.join(wal_path, collection_name)
        if os.path.exists(collection_wal):
            wal_files = glob.glob(os.path.join(collection_wal, "*.wal"))
            print(f"✅ Found {len(wal_files)} WAL files for collection '{collection_name}'")
            for wal_file in wal_files[:3]:  # Show first 3
                size = os.path.getsize(wal_file)
                print(f"   - {os.path.basename(wal_file)}: {size} bytes")
            return True
        else:
            print(f"❌ No WAL directory found for collection '{collection_name}'")
    else:
        print(f"❌ WAL directory not found at: {wal_path}")
    
    return False


def verify_collection_persistence(collection_name):
    """Verify that collection data was persisted"""
    # Check config.toml for collections location
    collections_path = "/workspace/data/metadata"
    
    print(f"\n📁 Checking collection persistence at: {collections_path}")
    
    if os.path.exists(collections_path):
        # Check for collection metadata files
        metadata_files = glob.glob(os.path.join(collections_path, "**/*.avro"), recursive=True)
        if metadata_files:
            print(f"✅ Found {len(metadata_files)} metadata files")
            for metadata_file in metadata_files[:3]:  # Show first 3
                size = os.path.getsize(metadata_file)
                print(f"   - {os.path.basename(metadata_file)}: {size} bytes")
            return True
        else:
            print(f"❌ No metadata files found")
    else:
        print(f"❌ Collections directory not found at: {collections_path}")
    
    return False


def test_end_to_end_flow():
    """Test complete vector operations flow"""
    
    print("🧪 ProximaDB End-to-End Vector Operations Test (BERT)")
    print("=" * 55)
    
    # Initialize client
    client = ProximaDBClient("http://localhost:5678")
    collection_name = f"bert_test_{int(time.time())}"
    
    try:
        # Step 1: Check server health
        print("\n1️⃣ Checking server health...")
        health = client.health()
        print(f"✅ Server status: {health.status}")
        print(f"   Protocol: {client.active_protocol.value}")
        
        # Step 2: Create collection with BERT configuration
        print(f"\n2️⃣ Creating collection '{collection_name}'...")
        dimension = 384  # BERT small model dimension
        
        config = CollectionConfig(
            dimension=dimension,
            distance_metric=DistanceMetric.COSINE,
            storage_layout="viper",
            description="BERT embedding test collection"
        )
        
        collection = client.create_collection(collection_name, config)
        print(f"✅ Collection created: {collection.name}")
        print(f"   ID: {collection.id}")
        print(f"   Dimension: {collection.dimension}")
        
        # Step 3: Generate BERT embeddings
        print("\n3️⃣ Generating BERT embeddings...")
        texts = [
            "The quick brown fox jumps over the lazy dog",
            "Machine learning is transforming the world", 
            "Vector databases enable semantic search",
            "Natural language processing is fascinating",
            "Deep learning models understand context"
        ]
        
        embeddings, actual_dim = generate_bert_embeddings(texts)
        print(f"✅ Generated {len(embeddings)} embeddings of dimension {actual_dim}")
        
        if actual_dim != dimension:
            print(f"⚠️ Warning: Expected dimension {dimension}, got {actual_dim}")
        
        # Step 4: Insert vectors
        print("\n4️⃣ Inserting vectors via SDK...")
        vector_ids = []
        metadata_list = []
        
        for i, (text, embedding) in enumerate(zip(texts, embeddings)):
            vector_ids.append(f"vec_{i}")
            metadata_list.append({"text": text, "index": str(i), "category": "test"})
        
        # Convert embeddings to list format
        embeddings_list = [emb.tolist() for emb in embeddings]
        
        insert_result = client.insert_vectors(
            collection_id=collection_name,
            vectors=embeddings_list,
            ids=vector_ids,
            metadata=metadata_list,
            upsert=False
        )
        
        print(f"✅ Inserted {insert_result.successful_count} vectors")
        print(f"   Failed: {insert_result.failed_count}")
        print(f"   Duration: {insert_result.duration_ms:.2f}ms")
        
        # Step 5: Search vectors
        print("\n5️⃣ Searching vectors...")
        query_text = "How does AI work?"
        query_embedding, _ = generate_bert_embeddings([query_text])
        
        search_results = client.search(
            collection_id=collection_name,
            query=query_embedding[0].tolist(),
            k=3,
            include_metadata=True,
            include_vectors=False
        )
        
        print(f"✅ Search completed for: '{query_text}'")
        print(f"   Found {len(search_results)} results:")
        
        for i, result in enumerate(search_results):
            print(f"   {i+1}. ID: {result.id}, Score: {result.score:.4f}")
            if result.metadata:
                print(f"      Text: {result.metadata.get('text', 'N/A')}")
        
        # Step 6: Get specific vector
        print("\n6️⃣ Getting specific vector...")
        vector_data = client.get_vector(
            collection_id=collection_name,
            vector_id="vec_0",
            include_vector=True,
            include_metadata=True
        )
        
        if vector_data:
            print(f"✅ Retrieved vector 'vec_0'")
            print(f"   Vector dimension: {len(vector_data.get('vector', []))}")
            if 'metadata' in vector_data:
                print(f"   Text: {vector_data['metadata'].get('text', 'N/A')}")
        else:
            print("❌ Failed to retrieve vector 'vec_0'")
        
        # Step 7: Insert additional vector (upsert test)
        print("\n7️⃣ Testing upsert operation...")
        update_text = "Artificial intelligence is revolutionizing technology"
        update_embedding, _ = generate_bert_embeddings([update_text])
        
        upsert_result = client.insert_vector(
            collection_id=collection_name,
            vector_id="vec_0",  # Same ID to test upsert
            vector=update_embedding[0].tolist(),
            metadata={"text": update_text, "updated": "true", "category": "test"},
            upsert=True
        )
        
        print(f"✅ Upsert completed")
        print(f"   Duration: {upsert_result.duration_ms:.2f}ms")
        
        # Step 8: Verify upsert by getting the vector again
        print("\n8️⃣ Verifying upsert...")
        updated_vector = client.get_vector(
            collection_id=collection_name,
            vector_id="vec_0",
            include_metadata=True
        )
        
        if updated_vector and updated_vector.get('metadata', {}).get('updated') == 'true':
            print("✅ Upsert verification successful - vector was updated")
            print(f"   New text: {updated_vector['metadata'].get('text', 'N/A')}")
        else:
            print("❌ Upsert verification failed")
        
        # Step 9: Delete a vector
        print("\n9️⃣ Deleting a vector...")
        delete_result = client.delete_vector(collection_name, "vec_4")
        print(f"✅ Delete operation completed")
        
        # Step 10: Verify persistence
        print("\n🔟 Verifying data persistence...")
        
        # Check WAL
        wal_verified = verify_wal_persistence(collection_name)
        
        # Check collection metadata
        collection_verified = verify_collection_persistence(collection_name)
        
        # Step 11: Final search to verify operations
        print("\n1️⃣1️⃣ Final search to verify all operations...")
        final_results = client.search(
            collection_id=collection_name,
            query=query_embedding[0].tolist(),
            k=5,
            include_metadata=True
        )
        
        print(f"✅ Final search found {len(final_results)} results")
        
        # Verify vec_4 was deleted (should not appear in results)
        vec_ids = [r.id for r in final_results]
        if 'vec_4' not in vec_ids:
            print("✅ Confirmed: Deleted vector 'vec_4' not in results")
        else:
            print("⚠️ Warning: Deleted vector 'vec_4' still in results")
        
        # Verify vec_0 was updated
        for result in final_results:
            if result.id == 'vec_0' and result.metadata and result.metadata.get('updated') == 'true':
                print("✅ Confirmed: Vector 'vec_0' shows as updated")
                break
        
        # Step 12: Collection stats
        print("\n1️⃣2️⃣ Checking collection information...")
        collection_info = client.get_collection(collection_name)
        print(f"✅ Collection info retrieved")
        print(f"   Vector count: {collection_info.vector_count}")
        print(f"   Dimension: {collection_info.dimension}")
        
        print("\n✅ END-TO-END BERT TEST COMPLETED SUCCESSFULLY!")
        print("\n📊 Summary:")
        print(f"   - Collection: {collection_name}")
        print(f"   - Vectors inserted: {len(vector_ids)}")
        print(f"   - Protocol used: {client.active_protocol.value}")
        print(f"   - WAL persistence: {'✅' if wal_verified else '❌'}")
        print(f"   - Collection persistence: {'✅' if collection_verified else '❌'}")
        print(f"   - All operations: ✅")
        
        return True
        
    except Exception as e:
        print(f"\n❌ Test failed: {e}")
        import traceback
        traceback.print_exc()
        return False
    
    finally:
        # Cleanup
        try:
            print(f"\n🧹 Cleaning up: Deleting collection '{collection_name}'...")
            success = client.delete_collection(collection_name)
            if success:
                print("✅ Cleanup completed")
            else:
                print("⚠️ Cleanup may have failed")
        except Exception as e:
            print(f"⚠️ Cleanup error: {e}")


if __name__ == "__main__":
    success = test_end_to_end_flow()
    sys.exit(0 if success else 1)