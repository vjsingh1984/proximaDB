#!/usr/bin/env python3
"""
Test BERT collection persistence and metadata operations
This tests the parts that are currently implemented: collection management and persistence
"""

import asyncio
import numpy as np
import sys
import subprocess
import time
from datetime import datetime

sys.path.insert(0, 'clients/python/src')
from proximadb.grpc_client import ProximaDBClient

def generate_bert_like_embeddings(texts, dimension=768):
    """Generate BERT-like embeddings for demonstration"""
    embeddings = []
    np.random.seed(42)  # For reproducible results
    
    for i, text in enumerate(texts):
        # Simulate BERT embeddings with some text-dependent variation
        base_embedding = np.random.randn(dimension).astype(np.float32)
        
        # Add text-length based variation
        text_factor = len(text) / 100.0
        base_embedding += np.random.randn(dimension).astype(np.float32) * text_factor * 0.1
        
        # Normalize to unit vector (common with BERT)
        base_embedding = base_embedding / np.linalg.norm(base_embedding)
        embeddings.append(base_embedding.tolist())
    
    return embeddings

async def test_bert_collection_setup():
    """Test BERT collection creation and metadata persistence"""
    print("🤖 BERT Collection Persistence Test")
    print("=" * 60)
    
    # Sample documents for a BERT-based semantic search system
    documents = [
        "Machine learning is a subset of artificial intelligence.",
        "Natural language processing enables computers to understand human language.", 
        "Deep learning uses neural networks with multiple layers.",
        "Vector databases store and search high-dimensional embeddings.",
        "BERT is a transformer-based language model for NLP tasks.",
        "Semantic search finds documents based on meaning, not just keywords.",
        "Embedding vectors capture semantic relationships between words.",
        "ProximaDB provides efficient vector storage and similarity search.",
        "Cosine similarity measures the angle between two vectors.",
        "Fine-tuning adapts pre-trained models to specific tasks."
    ]
    
    client = ProximaDBClient(endpoint='localhost:5679')
    
    # Step 1: Create BERT collections with different configurations
    print(f"\n1️⃣ Creating BERT collections with different configurations...")
    
    timestamp = datetime.now().strftime("%H%M%S")
    collections_to_create = [
        {
            'name': f'bert_base_{timestamp}',
            'dimension': 768,
            'distance_metric': 1,  # COSINE
            'description': 'BERT Base model embeddings (768 dimensions)'
        },
        {
            'name': f'bert_large_{timestamp}', 
            'dimension': 1024,
            'distance_metric': 1,  # COSINE
            'description': 'BERT Large model embeddings (1024 dimensions)'
        },
        {
            'name': f'sentence_bert_{timestamp}',
            'dimension': 384,
            'distance_metric': 2,  # EUCLIDEAN 
            'description': 'Sentence-BERT embeddings (384 dimensions)'
        }
    ]
    
    created_collections = []
    
    for collection_config in collections_to_create:
        try:
            result = await client.create_collection(
                name=collection_config['name'],
                dimension=collection_config['dimension'],
                distance_metric=collection_config['distance_metric']
            )
            
            print(f"   ✅ Created: {collection_config['name']}")
            print(f"      📐 Dimension: {collection_config['dimension']}")
            print(f"      📏 Distance: {client._distance_metric_to_string(collection_config['distance_metric'])}")
            print(f"      🆔 UUID: {result.id}")
            print(f"      📝 Purpose: {collection_config['description']}")
            
            created_collections.append({
                'config': collection_config,
                'result': result,
                'uuid': result.id
            })
            
        except Exception as e:
            print(f"   ❌ Failed to create {collection_config['name']}: {e}")
    
    # Step 2: Generate embeddings for documentation
    print(f"\n2️⃣ Generating BERT embeddings (for future vector insertion)...")
    
    for collection_info in created_collections:
        dimension = collection_info['config']['dimension']
        collection_name = collection_info['config']['name']
        
        embeddings = generate_bert_like_embeddings(documents, dimension=dimension)
        
        print(f"   📊 {collection_name}:")
        print(f"      🔢 Generated {len(embeddings)} embeddings")
        print(f"      📐 Shape: ({len(embeddings)}, {dimension})")
        print(f"      📏 Sample magnitude: {np.linalg.norm(embeddings[0]):.4f}")
        
        # Store embeddings info for later (when vector ops are implemented)
        collection_info['embeddings'] = embeddings
        collection_info['documents'] = documents
    
    # Step 3: Test collection retrieval and listing
    print(f"\n3️⃣ Testing collection management operations...")
    
    # List all collections
    all_collections = await client.list_collections()
    bert_collections = [c for c in all_collections if any(name in c.name for name in ['bert_', 'sentence_'])]
    
    print(f"   📋 Total collections: {len(all_collections)}")
    print(f"   🤖 BERT collections: {len(bert_collections)}")
    
    for collection in bert_collections:
        print(f"      • {collection.name} (dim: {collection.dimension}, UUID: {collection.id})")
    
    # Test individual collection retrieval
    for collection_info in created_collections:
        collection_name = collection_info['config']['name']
        
        try:
            retrieved = await client.get_collection(collection_name)
            if retrieved:
                print(f"   ✅ Retrieved: {collection_name}")
                print(f"      🔍 Matches UUID: {retrieved.id == collection_info['uuid']}")
            else:
                print(f"   ❌ Failed to retrieve: {collection_name}")
        except Exception as e:
            print(f"   ❌ Error retrieving {collection_name}: {e}")
    
    # Step 4: Prepare for vector operations (when implemented)
    print(f"\n4️⃣ Preparing for future vector operations...")
    
    print(f"   💡 When vector operations are implemented, you can:")
    print(f"      • Insert {len(documents)} document embeddings per collection")
    print(f"      • Perform ID-based retrieval: get_vector(collection, 'doc_001')")
    print(f"      • Search by metadata: search_by_metadata(category='ml_nlp')")
    print(f"      • Similarity search: search_vectors(query_embedding, top_k=5)")
    
    # Show sample vector operation calls
    print(f"\n   📋 Sample vector operations (for future implementation):")
    for i, (doc, collection_info) in enumerate(zip(documents[:3], created_collections)):
        collection_name = collection_info['config']['name']
        embedding = collection_info['embeddings'][0]  # First embedding
        
        print(f"      # Insert document {i}")
        print(f"      await client.insert_vector(")
        print(f"          collection='{collection_name}',")
        print(f"          id='doc_{i:03d}',")
        print(f"          vector={str(embedding[:4])}...,  # {len(embedding)} dims")
        print(f"          metadata={{'text': '{doc[:30]}...', 'category': 'nlp'}})")
        print()
    
    # Step 5: Persistence info
    print(f"\n5️⃣ Collection persistence information...")
    
    print(f"   💾 Collections are persisted to:")
    print(f"      📁 Metadata: /data/proximadb/1/metadata/")
    print(f"      📁 WAL logs: /data/proximadb/1/wal/")
    
    print(f"   🔄 To test restart persistence:")
    print(f"      1. Note collection UUIDs and names")
    print(f"      2. Restart server: pkill proximadb-server && cargo run --bin proximadb-server")
    print(f"      3. Run: python {__file__} restart_test")
    
    return {
        'success': True,
        'created_collections': created_collections,
        'documents': documents
    }

async def test_restart_persistence():
    """Test that BERT collections persist across restarts"""
    print("\n🔄 BERT COLLECTION RESTART PERSISTENCE TEST")
    print("=" * 60)
    
    client = ProximaDBClient(endpoint='localhost:5679')
    
    # Step 1: List all collections after restart
    print(f"\n1️⃣ Checking collections after restart...")
    
    try:
        all_collections = await client.list_collections()
        bert_collections = [c for c in all_collections if any(name in c.name for name in ['bert_', 'sentence_'])]
        
        print(f"   📋 Total collections found: {len(all_collections)}")
        print(f"   🤖 BERT collections found: {len(bert_collections)}")
        
        if bert_collections:
            print(f"   ✅ BERT collections survived restart!")
            for collection in bert_collections:
                print(f"      • {collection.name}")
                print(f"        📐 Dimension: {collection.dimension}")
                print(f"        🆔 UUID: {collection.id}")
                print(f"        📏 Distance: {collection.metric or 'N/A'}")
                print()
        else:
            print(f"   ❌ No BERT collections found after restart")
            print(f"   📋 Available collections:")
            for c in all_collections:
                print(f"      • {c.name}")
                
    except Exception as e:
        print(f"   ❌ Failed to list collections: {e}")
        return False
    
    # Step 2: Test collection retrieval by name
    print(f"\n2️⃣ Testing collection retrieval by name...")
    
    for collection in bert_collections:
        try:
            retrieved = await client.get_collection(collection.name)
            if retrieved and retrieved.id == collection.id:
                print(f"   ✅ {collection.name}: UUID matches, fully restored")
            elif retrieved:
                print(f"   ⚠️ {collection.name}: Found but UUID mismatch")
            else:
                print(f"   ❌ {collection.name}: Not found by name")
        except Exception as e:
            print(f"   ❌ {collection.name}: Error during retrieval: {e}")
    
    # Step 3: Verify collection configurations
    print(f"\n3️⃣ Verifying collection configurations...")
    
    expected_configs = [
        {'name_pattern': 'bert_base_', 'dimension': 768, 'distance_metric': 1},
        {'name_pattern': 'bert_large_', 'dimension': 1024, 'distance_metric': 1},
        {'name_pattern': 'sentence_bert_', 'dimension': 384, 'distance_metric': 2}
    ]
    
    for expected in expected_configs:
        matching = [c for c in bert_collections if expected['name_pattern'] in c.name]
        
        if matching:
            collection = matching[0]
            dimension_ok = collection.dimension == expected['dimension']
            metric_ok = getattr(collection, 'distance_metric', None) == expected['distance_metric']
            
            print(f"   📊 {expected['name_pattern']}*:")
            print(f"      ✅ Found: {collection.name}")
            print(f"      {'✅' if dimension_ok else '❌'} Dimension: {collection.dimension} (expected {expected['dimension']})")
            print(f"      {'✅' if metric_ok else '❌'} Distance metric: {getattr(collection, 'distance_metric', 'N/A')} (expected {expected['distance_metric']})")
        else:
            print(f"   ❌ {expected['name_pattern']}*: Not found")
    
    print(f"\n🎉 Restart persistence test completed!")
    
    return len(bert_collections) > 0

async def full_restart_test():
    """Complete restart test with server restart"""
    print("🔄 FULL RESTART TEST WITH SERVER RESTART")
    print("=" * 60)
    
    print("1️⃣ Creating BERT collections...")
    collection_info = await test_bert_collection_setup()
    
    if not collection_info.get('success'):
        print("❌ Failed to create collections")
        return False
    
    print(f"\n2️⃣ Stopping server...")
    subprocess.run(["pkill", "-f", "proximadb-server"], capture_output=True)
    time.sleep(3)
    
    print(f"\n3️⃣ Restarting server...")
    # Start server in background
    subprocess.Popen(
        ["cargo", "run", "--bin", "proximadb-server", "--", "--config", "config.toml"],
        stdout=open("bert_test_server.log", "w"),
        stderr=subprocess.STDOUT
    )
    
    # Wait for server to start
    print("   ⏳ Waiting for server startup...")
    for i in range(15):
        try:
            test_client = ProximaDBClient(endpoint='localhost:5679')
            await test_client.list_collections()
            print("   ✅ Server is ready")
            break
        except:
            print(f"   ⏳ Waiting... ({i+1}/15)")
            time.sleep(1)
    else:
        print("   ❌ Server failed to start")
        return False
    
    print(f"\n4️⃣ Testing persistence...")
    return await test_restart_persistence()

if __name__ == "__main__":
    import sys
    
    if len(sys.argv) > 1 and sys.argv[1] == "restart_test":
        # Test persistence after manual restart
        try:
            result = asyncio.run(test_restart_persistence())
            if result:
                print("✅ BERT collections successfully persisted across restart!")
            else:
                print("❌ Persistence test failed")
        except Exception as e:
            print(f"❌ Restart test error: {e}")
    
    elif len(sys.argv) > 1 and sys.argv[1] == "full_restart":
        # Test with automatic server restart
        try:
            result = asyncio.run(full_restart_test())
            if result:
                print("✅ Full restart test passed!")
            else:
                print("❌ Full restart test failed")
        except Exception as e:
            print(f"❌ Full restart test error: {e}")
    
    else:
        # Run collection setup test
        try:
            collection_info = asyncio.run(test_bert_collection_setup())
            
            if collection_info and collection_info.get('success'):
                created_count = len(collection_info['created_collections'])
                print(f"\n🎉 BERT collection test completed!")
                print(f"✅ Created {created_count} BERT collections")
                print(f"📊 Ready for {len(collection_info['documents'])} documents per collection")
                
                print(f"\n🔄 To test persistence:")
                print(f"   Manual: python {sys.argv[0]} restart_test")
                print(f"   Auto:   python {sys.argv[0]} full_restart")
            else:
                print("❌ BERT collection test failed")
                
        except KeyboardInterrupt:
            print("\n⏹️ Test interrupted by user")
        except Exception as e:
            print(f"\n❌ Test failed: {e}")