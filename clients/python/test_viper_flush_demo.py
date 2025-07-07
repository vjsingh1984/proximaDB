#!/usr/bin/env python3
"""
Quick VIPER Flush Demonstration
Shows 1K BERT vectors triggering WAL flush and Parquet file creation
"""

import asyncio
import time
import glob
import os
from pathlib import Path

# Set up Python path
import sys
sys.path.insert(0, "src")

from proximadb.grpc_client import ProximaDBClient
from tests.utils.bert_embedding_utils import generate_text_corpus, convert_corpus_to_vectors

async def demo_viper_flush():
    """Demonstrate VIPER flush with 1K BERT vectors"""
    print("🚀 VIPER Flush Demo - 1K BERT Vectors")
    print("=" * 50)
    
    # Initialize client
    client = ProximaDBClient("localhost:5679")
    await client.connect()
    
    collection_name = f"viper_flush_demo_{int(time.time())}"
    
    try:
        # Create VIPER collection
        print(f"🏗️ Creating VIPER collection: {collection_name}")
        await client.create_collection(
            name=collection_name,
            dimension=384,
            distance_metric="COSINE",
            storage_engine="VIPER"
        )
        
        # Generate 1K BERT vectors
        print("🧠 Generating 1K BERT embeddings...")
        corpus = generate_text_corpus(1000)
        vectors = convert_corpus_to_vectors(corpus, 384)
        print(f"✅ Generated {len(vectors)} BERT vectors")
        
        # Monitor files before insertion
        print("📁 Monitoring files before insertion...")
        parquet_files_before = len(glob.glob("/workspace/**/*.parquet", recursive=True))
        wal_files_before = len(glob.glob("/workspace/**/*.wal", recursive=True))
        print(f"   Parquet files: {parquet_files_before}")
        print(f"   WAL files: {wal_files_before}")
        
        # Insert vectors in batches
        print("🔥 Inserting 1K vectors...")
        batch_size = 200
        for i in range(0, len(vectors), batch_size):
            batch = vectors[i:i+batch_size]
            result = client.insert_vectors(
                collection_id=collection_name,
                vectors=batch
            )
            print(f"   Batch {i//batch_size + 1}: {len(batch)} vectors inserted")
        
        # Wait for flush to trigger
        print("⏳ Waiting for WAL flush (15 seconds)...")
        await asyncio.sleep(15)
        
        # Monitor files after flush
        print("📁 Monitoring files after flush...")
        parquet_files_after = len(glob.glob("/workspace/**/*.parquet", recursive=True))
        wal_files_after = len(glob.glob("/workspace/**/*.wal", recursive=True))
        print(f"   Parquet files: {parquet_files_after}")
        print(f"   WAL files: {wal_files_after}")
        
        # Check if flush triggered
        if parquet_files_after > parquet_files_before:
            print(f"✅ FLUSH TRIGGERED! {parquet_files_after - parquet_files_before} new Parquet files created")
        else:
            print("⚠️ No new Parquet files detected")
        
        # Test search on both storage tiers
        print("🔍 Testing search on dual storage...")
        query_vector = vectors[0]["vector"]  # Use first vector as query
        
        results = client.search_vectors(
            collection_id=collection_name,
            query_vectors=[query_vector],
            top_k=5,
            include_metadata=True
        )
        
        print(f"   Search results: {len(results)} vectors found")
        
        print("\n🎉 VIPER Flush Demo Completed!")
        print(f"Summary:")
        print(f"   Collection: {collection_name}")
        print(f"   Vectors: {len(vectors)}")
        print(f"   Parquet files created: {parquet_files_after - parquet_files_before}")
        print(f"   WAL files changed: {wal_files_after - wal_files_before}")
        
    except Exception as e:
        print(f"❌ Demo failed: {e}")
        import traceback
        traceback.print_exc()
    
    finally:
        # Cleanup
        try:
            await client.delete_collection(collection_name)
            print(f"🧹 Cleaned up collection: {collection_name}")
        except:
            pass
        await client.close()

if __name__ == "__main__":
    asyncio.run(demo_viper_flush())