#!/usr/bin/env python3
"""


STATUS: ⚠️  Requires External Dependency
SDK Version: v1.0+ (requires aiofiles package)
Server Version: v0.2.0+
Test Result: SKIP - Install with: pip install aiofiles

Streaming Upload Example for ProximaDB Python SDK v1.0

This example demonstrates efficient handling of large datasets:
- Streaming vector uploads
- Chunked file processing
- Progress monitoring
- Memory-efficient operations
- Error recovery and retries

"""

import asyncio
import json
import csv
import time
import numpy as np
from pathlib import Path
from typing import AsyncIterator, List, Dict, Any
import aiofiles

from proximadb import ProximaDBClient, ClientConfig
from proximadb.models import (
    CollectionConfig,
    VectorRecord,
    DistanceMetric,
    StorageEngine,
    InsertOptions
)
from proximadb.streaming import VectorStream, ChunkedUploader, StreamMetrics


async def generate_large_dataset(num_vectors: int, dimension: int) -> AsyncIterator[VectorRecord]:
    """Async generator for creating vectors on-the-fly"""
    print(f"🏭 Generating {num_vectors:,} vectors of dimension {dimension}...")
    
    # Simulate different data sources/categories
    categories = ["sensor_data", "user_embeddings", "product_features", "document_vectors"]
    sources = ["model_v1", "model_v2", "model_v3"]
    
    for i in range(num_vectors):
        # Generate vector with some pattern (simulate real embeddings)
        vector = np.random.randn(dimension)
        
        # Add some structure to simulate clustering
        category_idx = i % len(categories)
        vector[category_idx * 10:(category_idx + 1) * 10] += 0.5
        
        # Create metadata
        metadata = {
            "index": i,
            "category": categories[category_idx],
            "source": sources[i % len(sources)],
            "timestamp": int(time.time() * 1000),
            "quality_score": float(np.random.uniform(0.7, 1.0)),
            "processing_time_ms": int(np.random.randint(10, 100)),
            "tags": ["streaming", "demo"] if i % 10 == 0 else ["streaming"]
        }
        
        yield VectorRecord(
            id=f"vec_{i:08d}",
            vector=vector.tolist(),
            metadata=metadata
        )
        
        # Simulate processing delay
        if i % 1000 == 0:
            await asyncio.sleep(0.001)


async def demo_basic_streaming(client: ProximaDBClient, collection_name: str) -> None:
    """Demonstrate basic streaming upload"""
    print("\n📡 Example 1: Basic Streaming Upload")
    print("=" * 50)
    
    # Create vector stream
    stream = VectorStream(
        client,
        collection_name=collection_name,
        batch_size=500,           # Vectors per batch
        max_concurrent_batches=3,  # Parallel uploads
        retry_on_failure=True,
        progress_callback=lambda m: print(f"   Progress: {m.processed_items:,}/{m.total_items:,} "
                                        f"({m.progress_percentage:.1f}%) - "
                                        f"{m.throughput:.0f} vec/s")
    )
    
    # Stream 10,000 vectors
    num_vectors = 10000
    dimension = 128
    
    print(f"⬆️  Streaming {num_vectors:,} vectors...")
    start_time = time.time()
    
    # Insert vectors from async generator
    metrics = await stream.insert_stream(
        generate_large_dataset(num_vectors, dimension),
        total_items=num_vectors  # Optional: enables progress tracking
    )
    
    elapsed = time.time() - start_time
    
    print(f"\n✅ Streaming completed!")
    print(f"   - Total vectors: {metrics.processed_items:,}")
    print(f"   - Time elapsed: {elapsed:.2f}s")
    print(f"   - Average throughput: {metrics.throughput:.0f} vectors/sec")
    print(f"   - Success rate: {metrics.success_rate:.1%}")
    if metrics.failed_items > 0:
        print(f"   - Failed items: {metrics.failed_items}")


async def demo_file_uploads(client: ProximaDBClient, collection_name: str) -> None:
    """Demonstrate uploading from various file formats"""
    print("\n📁 Example 2: File-based Uploads")
    print("=" * 50)
    
    uploader = ChunkedUploader(client, collection_name)
    
    # Create sample files
    await create_sample_files()
    
    # 1. Upload from JSONL file
    print("\n📄 Uploading from JSONL file...")
    jsonl_metrics = await uploader.upload_jsonl(
        "sample_vectors.jsonl",
        batch_size=1000,
        progress_callback=lambda m: print(f"   JSONL Progress: {m.progress_percentage:.1f}%")
    )
    print(f"✅ JSONL upload: {jsonl_metrics.processed_items} vectors")
    
    # 2. Upload from CSV file
    print("\n📄 Uploading from CSV file...")
    csv_metrics = await uploader.upload_csv(
        "sample_vectors.csv",
        vector_column="embedding",
        id_column="id",
        metadata_columns=["category", "score"],
        batch_size=1000,
        progress_callback=lambda m: print(f"   CSV Progress: {m.progress_percentage:.1f}%")
    )
    print(f"✅ CSV upload: {csv_metrics.processed_items} vectors")
    
    # 3. Upload from NumPy arrays
    print("\n🔢 Uploading from NumPy arrays...")
    embeddings = np.random.randn(5000, 128)
    ids = [f"numpy_vec_{i:05d}" for i in range(5000)]
    metadata_list = [{"source": "numpy", "index": i} for i in range(5000)]
    
    numpy_metrics = await uploader.upload_numpy(
        embeddings=embeddings,
        ids=ids,
        metadata_list=metadata_list,
        batch_size=1000,
        progress_callback=lambda m: print(f"   NumPy Progress: {m.progress_percentage:.1f}%")
    )
    print(f"✅ NumPy upload: {numpy_metrics.processed_items} vectors")
    
    # Clean up sample files
    Path("sample_vectors.jsonl").unlink(missing_ok=True)
    Path("sample_vectors.csv").unlink(missing_ok=True)


async def demo_memory_efficient_streaming(client: ProximaDBClient, collection_name: str) -> None:
    """Demonstrate memory-efficient streaming for very large datasets"""
    print("\n💾 Example 3: Memory-Efficient Streaming")
    print("=" * 50)
    
    # Monitor memory usage
    import psutil
    process = psutil.Process()
    initial_memory = process.memory_info().rss / 1024 / 1024  # MB
    
    print(f"📊 Initial memory usage: {initial_memory:.1f} MB")
    
    # Stream configuration for minimal memory usage
    stream = VectorStream(
        client,
        collection_name=collection_name,
        batch_size=100,            # Smaller batches
        max_concurrent_batches=2,  # Limit concurrency
        max_queue_size=10          # Limit buffer size
    )
    
    # Process 100,000 vectors without loading all into memory
    num_vectors = 100000
    dimension = 384  # Larger vectors
    
    print(f"⬆️  Streaming {num_vectors:,} large vectors (384D)...")
    
    async def memory_monitor():
        """Monitor memory usage during streaming"""
        max_memory = initial_memory
        while True:
            current_memory = process.memory_info().rss / 1024 / 1024
            max_memory = max(max_memory, current_memory)
            await asyncio.sleep(1)
            if stream._finished:
                break
        return max_memory
    
    # Start memory monitoring
    monitor_task = asyncio.create_task(memory_monitor())
    
    # Stream vectors
    metrics = await stream.insert_stream(
        generate_large_dataset(num_vectors, dimension),
        total_items=num_vectors,
        progress_interval=10000  # Update every 10k vectors
    )
    
    stream._finished = True
    max_memory = await monitor_task
    
    print(f"\n✅ Memory-efficient streaming completed!")
    print(f"   - Vectors processed: {metrics.processed_items:,}")
    print(f"   - Peak memory usage: {max_memory:.1f} MB")
    print(f"   - Memory increase: {max_memory - initial_memory:.1f} MB")
    print(f"   - Throughput: {metrics.throughput:.0f} vectors/sec")


async def demo_error_recovery(client: ProximaDBClient, collection_name: str) -> None:
    """Demonstrate error handling and recovery during streaming"""
    print("\n🔧 Example 4: Error Recovery and Retries")
    print("=" * 50)
    
    async def flaky_vector_generator(num_vectors: int, error_rate: float = 0.1):
        """Generator that occasionally produces invalid vectors"""
        for i in range(num_vectors):
            if np.random.random() < error_rate:
                # Simulate various errors
                if i % 3 == 0:
                    # Wrong dimension
                    yield VectorRecord(
                        id=f"bad_vec_{i}",
                        vector=[0.1] * 64,  # Wrong dimension!
                        metadata={"error": "dimension_mismatch"}
                    )
                elif i % 3 == 1:
                    # Invalid ID
                    yield VectorRecord(
                        id="",  # Empty ID!
                        vector=[0.1] * 128,
                        metadata={"error": "invalid_id"}
                    )
                else:
                    # Simulate network error by raising exception
                    raise ConnectionError(f"Simulated network error at vector {i}")
            else:
                # Valid vector
                yield VectorRecord(
                    id=f"vec_{i:06d}",
                    vector=np.random.randn(128).tolist(),
                    metadata={"index": i, "valid": True}
                )
    
    # Stream with error handling
    stream = VectorStream(
        client,
        collection_name=collection_name,
        batch_size=100,
        retry_on_failure=True,
        max_retries=3,
        error_callback=lambda e, batch: print(f"   ⚠️  Error in batch: {type(e).__name__} - {str(e)[:50]}")
    )
    
    num_vectors = 1000
    print(f"⬆️  Streaming {num_vectors} vectors with 10% error rate...")
    
    try:
        metrics = await stream.insert_stream(
            flaky_vector_generator(num_vectors, error_rate=0.1),
            continue_on_error=True  # Continue despite errors
        )
        
        print(f"\n✅ Streaming completed with error recovery!")
        print(f"   - Attempted: {num_vectors}")
        print(f"   - Successful: {metrics.processed_items}")
        print(f"   - Failed: {metrics.failed_items}")
        print(f"   - Success rate: {metrics.success_rate:.1%}")
        print(f"   - Retries: {metrics.retry_count}")
        
    except Exception as e:
        print(f"❌ Streaming failed: {e}")


async def demo_parallel_streams(client: ProximaDBClient, collection_name: str) -> None:
    """Demonstrate parallel streaming from multiple sources"""
    print("\n🚀 Example 5: Parallel Multi-Source Streaming")
    print("=" * 50)
    
    # Define multiple data sources
    sources = [
        {"name": "sensor_1", "vectors": 5000, "dimension": 128},
        {"name": "sensor_2", "vectors": 5000, "dimension": 128},
        {"name": "sensor_3", "vectors": 5000, "dimension": 128},
    ]
    
    async def source_generator(source_name: str, num_vectors: int, dimension: int):
        """Generate vectors for a specific source"""
        for i in range(num_vectors):
            yield VectorRecord(
                id=f"{source_name}_vec_{i:05d}",
                vector=np.random.randn(dimension).tolist(),
                metadata={
                    "source": source_name,
                    "timestamp": int(time.time() * 1000),
                    "sequence": i
                }
            )
    
    print(f"⬆️  Streaming from {len(sources)} sources in parallel...")
    start_time = time.time()
    
    # Create parallel streaming tasks
    tasks = []
    for source in sources:
        stream = VectorStream(
            client,
            collection_name=collection_name,
            batch_size=500,
            stream_name=source["name"]  # Named streams for tracking
        )
        
        task = stream.insert_stream(
            source_generator(
                source["name"],
                source["vectors"],
                source["dimension"]
            ),
            total_items=source["vectors"]
        )
        tasks.append(task)
    
    # Run all streams in parallel
    results = await asyncio.gather(*tasks)
    
    elapsed = time.time() - start_time
    total_vectors = sum(source["vectors"] for source in sources)
    total_processed = sum(metrics.processed_items for metrics in results)
    
    print(f"\n✅ Parallel streaming completed!")
    print(f"   - Total vectors: {total_processed:,} / {total_vectors:,}")
    print(f"   - Time elapsed: {elapsed:.2f}s")
    print(f"   - Combined throughput: {total_processed / elapsed:.0f} vectors/sec")
    
    # Show per-source metrics
    print("\n📊 Per-source metrics:")
    for i, (source, metrics) in enumerate(zip(sources, results)):
        print(f"   - {source['name']}: {metrics.processed_items:,} vectors, "
              f"{metrics.throughput:.0f} vec/s")


async def create_sample_files():
    """Create sample data files for upload demos"""
    # Create JSONL file
    async with aiofiles.open("sample_vectors.jsonl", "w") as f:
        for i in range(1000):
            record = {
                "id": f"jsonl_vec_{i:04d}",
                "vector": np.random.randn(128).tolist(),
                "metadata": {
                    "category": ["A", "B", "C"][i % 3],
                    "score": float(np.random.uniform(0, 1))
                }
            }
            await f.write(json.dumps(record) + "\n")
    
    # Create CSV file
    async with aiofiles.open("sample_vectors.csv", "w") as f:
        writer = csv.writer(await f.__aenter__())
        
        # Header
        header = ["id", "embedding", "category", "score"]
        await f.write(",".join(header) + "\n")
        
        # Data rows
        for i in range(1000):
            row = [
                f"csv_vec_{i:04d}",
                json.dumps(np.random.randn(128).tolist()),
                ["X", "Y", "Z"][i % 3],
                f"{np.random.uniform(0, 1):.3f}"
            ]
            await f.write(",".join(str(x) for x in row) + "\n")


async def main():
    # Initialize client
    print("🚀 Streaming Upload Example for ProximaDB")
    print("=" * 50)
    
    client = ProximaDBClient(
        ClientConfig(
            url="http://localhost:5678",
            timeout=120.0  # Longer timeout for large uploads
        )
    )
    
    collection_name = "streaming_demo"
    
    try:
        # Create collection
        print(f"📦 Creating collection '{collection_name}'...")
        
        config = CollectionConfig(
            name=collection_name,
            dimension=128,  # Default dimension
            distance_metric=DistanceMetric.COSINE,
            storage_engine=StorageEngine.SST,  # Row-based for streaming
            metadata={
                "description": "Collection for streaming upload demo"
            }
        )
        
        try:
            await client.adelete_collection(collection_name)
        except:
            pass
        
        collection = await client.acreate_collection(config)
        print(f"✅ Collection created: {collection.id}")
        
        # Run all demos
        await demo_basic_streaming(client, collection_name)
        await demo_file_uploads(client, collection_name)
        await demo_memory_efficient_streaming(client, collection_name)
        await demo_error_recovery(client, collection_name)
        await demo_parallel_streams(client, collection_name)
        
        # Show final statistics
        print("\n📊 Final collection statistics:")
        collection = await client.aget_collection(collection_name)
        print(f"   - Total vectors: {collection.vector_count:,}")
        print(f"   - Storage size: {collection.storage_size_bytes / 1024 / 1024:.2f} MB")
        
        print("\n✅ All streaming examples completed!")
        
    finally:
        # Cleanup
        print("\n🧹 Cleaning up...")
        try:
            await client.adelete_collection(collection_name)
            print("✅ Demo collection deleted")
        except Exception as e:
            print(f"⚠️  Cleanup failed: {e}")


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except (ImportError, ModuleNotFoundError) as e:
        print("=" * 70)
        print("🚧 FUTURE FEATURE - Not Yet Implemented")
        print("=" * 70)
        print(f"
❌ Error: {e}
")
        print(f"📋 This example requires: aiofiles package")
        print(f"   Expected in SDK v1.1+
")
        print(f"💡 Workaround:")
        print(f"   Install with: pip install aiofiles
")
        print("=" * 70)
        exit(1)
    except Exception as e:
        print(f"❌ Unexpected error: {e}")
        import traceback
        traceback.print_exc()
        exit(1)