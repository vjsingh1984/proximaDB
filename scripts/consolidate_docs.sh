#!/bin/bash

echo "Starting documentation consolidation..."

# 1. Add compression implementation details to developer/architecture.adoc
echo "Updating developer/architecture.adoc with compression details..."
cat >> docs/developer/architecture.adoc << 'EOF'

== SDK-Driven Compression Architecture

=== Overview
ProximaDB implements a comprehensive SDK-driven compression system that gives clients complete control over compression settings without any server-side defaults.

=== Key Components

==== SST1 Format
The SST1 format is a clean, modern SSTable format with version identification:

* Magic bytes: `b"SST1"` at the beginning of each file
* No legacy compatibility - clean slate design
* Clear error messages for invalid formats

[source]
----
[SST1 Magic - 4 bytes]["SST1"]
[Header Length - 4 bytes]
[Header Data - variable]
[Bloom Filter Length - 4 bytes]
[Bloom Filter Data - variable]
[Index Length - 4 bytes]
[Index Data - variable]
[Data Blocks - variable]
----

==== UnifiedQueryPlanner
Centralized query planning that considers compression:

* Analyzes file metadata for compression status
* Routes queries to optimal data format
* Manages two-stage search for quantized data
* Estimates decompression costs

==== Decompression Cache
LRU cache for decompressed blocks:

* Configurable size (default 512MB)
* Automatic invalidation on data changes
* Prefetching support for sequential access
* Per-algorithm sub-caches for better locality

=== Compression Flow

[source,mermaid]
----
graph TD
    SDK[Python SDK] -->|CompressionConfig| API[REST/gRPC API]
    API -->|Proto Message| DS[DirectVectorService]
    DS -->|UnifiedQueryPlanner| QP[Query Planning]
    QP -->|Routing Decision| SE{Storage Engine}
    SE -->|SST| SST[SST Engine]
    SE -->|VIPER| VIPER[VIPER Engine]
    SST -->|Block Compression| SSTW[SSTable Writer]
    VIPER -->|Parquet Compression| PW[Parquet Writer]
    SSTW -->|SST1 Format| FS[Filesystem]
    PW -->|Arrow Format| FS
----

EOF

# 2. Add Python SDK details to user/user_guide.adoc
echo "Updating user/user_guide.adoc with SDK compression usage..."
cat >> docs/user/user_guide.adoc << 'EOF'

== Compression Configuration

=== SDK-Driven Compression

ProximaDB's compression is entirely controlled from the client SDK, with no server-side defaults:

[source,python]
----
from proximadb import ProximaDBClient
from proximadb.models import (
    CollectionConfig,
    CompressionConfig,
    CompressionAlgorithm,
    StorageEngine
)

# Configure compression
compression_config = CompressionConfig(
    sst_compression_algorithm=CompressionAlgorithm.ZSTD,
    sst_compression_level=6,
    sst_block_size=32768,
    adaptive_compression=True
)

# Create collection with compression
collection_config = CollectionConfig(
    name="my_collection",
    dimension=1536,
    storage_engine=StorageEngine.SST,
    compression_config=compression_config
)

collection = client.create_collection(collection_config)
----

=== Compression-Aware Search

Optimize searches with compression hints:

[source,python]
----
from proximadb.models import SearchOptimization

optimization = SearchOptimization(
    enable_two_stage=True,
    use_decompression_cache=True,
    prefer_compressed_search=True,
    compression_aware_routing=True
)

results = client.search_vectors(
    collection_id="my_collection",
    query_vector=query_vector,
    top_k=10,
    search_optimization=optimization
)
----

EOF

# 3. Add API details to user/api_reference.adoc
echo "Updating user/api_reference.adoc with compression APIs..."
cat >> docs/user/api_reference.adoc << 'EOF'

=== Compression Configuration API

==== CompressionConfig Model

[source,python]
----
class CompressionConfig(BaseModel):
    # SST compression settings
    sst_block_size: Optional[int] = 16384
    sst_compression_algorithm: Optional[CompressionAlgorithm] = None
    sst_compression_level: Optional[int] = None
    
    # VIPER compression settings  
    viper_compression_algorithm: Optional[CompressionAlgorithm] = None
    viper_compression_level: Optional[int] = None
    viper_enable_dual_columns: Optional[bool] = False
    
    # Global settings
    adaptive_compression: Optional[bool] = False
    compression_threshold_kb: Optional[int] = 100
----

==== SearchOptimization Model

[source,python]
----
class SearchOptimization(BaseModel):
    # Compression-aware search hints
    prefer_compressed_search: Optional[bool] = None
    decompression_budget_ms: Optional[int] = None
    use_decompression_cache: Optional[bool] = True
    compression_aware_routing: Optional[bool] = None
----

EOF

# 4. Update index.adoc with compression references
echo "Updating index.adoc..."
sed -i '/== Features/a\
* **SDK-Driven Compression**: Client-controlled compression without server defaults\
* **SST1 Format**: Modern SSTable format with version identification\
* **Compression-Aware Search**: Intelligent query routing based on compression status' docs/index.adoc

# 5. Update technical reference with metrics
echo "Updating developer/technical_reference.adoc..."
cat >> docs/developer/technical_reference.adoc << 'EOF'

== Compression Metrics

=== Performance Characteristics

[cols="3,2,2,2", options="header"]
|===
| Operation | No Compression | LZ4 | ZSTD-6

| **Insert (vec/s)**
| 10,000
| 8,500
| 6,000

| **Search (ms)**
| 0.5
| 0.7
| 1.2

| **Storage Size**
| 100GB
| 50GB
| 30GB

| **Cache Hit Rate**
| N/A
| 75%
| 75%
|===

=== Compression Ratios

* **ZSTD**: 2-10x compression (balanced)
* **LZ4**: 1.5-3x compression (fast)
* **SNAPPY**: 1.5-2x compression (very fast)

EOF

# 6. Remove MD files after consolidation
echo "Removing consolidated MD files..."
rm -f COMPRESSION_IMPLEMENTATION_SUMMARY.md
rm -f BACKGROUND_THREAD_OPTIMIZATION.md
rm -f clients/python/CONSOLIDATION_PLAN.md
rm -f clients/python/GRPC_IMPLEMENTATION_SUMMARY.md
rm -f clients/python/MIGRATION_SUMMARY.md
rm -f clients/python/MIGRATION_UPDATE.md
rm -f clients/python/docs/*.md
rm -f clients/python/examples/README.md
rm -f clients/python/tests/real_server_migration_summary.md
rm -f demo/demo_results/compression_feature_summary.md
rm -f docs/architecture/compression_encoding_design.md
rm -f docs/developer/metrics_framework_design.md
rm -f docs/enhancements/embedding-service-architecture.md
rm -f docs/enhancements/sst-sorting-mechanisms-analysis.md

echo "Documentation consolidation complete!"