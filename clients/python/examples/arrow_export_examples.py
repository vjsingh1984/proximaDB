#!/usr/bin/env python3
"""
Arrow Export Examples for ProximaDB Python SDK

This example demonstrates how to use the ArrowExportClient to export
ProximaDB data via Arrow Flight for analytics and data science workflows.

Arrow Flight provides zero-copy data transfer, enabling efficient export
to PyArrow, Polars, DuckDB, pandas, and NumPy.

Prerequisites:
    pip install pyarrow polars duckdb numpy

Usage:
    # With a running ProximaDB server (with data):
    python arrow_export_examples.py

    # Server ports:
    # - REST API: 5678
    # - gRPC: 5679
    # - Arrow Flight: 5680 (used by this example)

Copyright 2025 ProximaDB Contributors
Licensed under the Apache License, Version 2.0
"""

import sys
from typing import Optional


# Check for required dependencies
def check_dependencies():
    """Check if required dependencies are installed."""
    missing = []

    try:
        import pyarrow
    except ImportError:
        missing.append("pyarrow")

    try:
        import polars
    except ImportError:
        missing.append("polars")

    try:
        import duckdb
    except ImportError:
        missing.append("duckdb")

    try:
        import numpy
    except ImportError:
        missing.append("numpy")

    return missing


# =============================================================================
# Example 1: Basic Connection to Arrow Flight
# =============================================================================


def example_basic_connection():
    """
    Demonstrate basic connection to ProximaDB Arrow Flight server.

    The ArrowExportClient connects to port 5680 by default, which is
    the Arrow Flight endpoint. This is separate from REST (5678) and
    gRPC (5679) endpoints.
    """
    from proximadb_sdk.arrow_export import ArrowExportClient

    print("=" * 70)
    print("Example 1: Basic Connection to Arrow Flight")
    print("=" * 70)

    # Method 1: Standard connection
    client = ArrowExportClient(
        host="localhost",
        port=5680,  # Arrow Flight port (default)
    )
    print(f"Connected to: {client._uri}")

    # Method 2: Using context manager (recommended)
    # Automatically closes connection when done
    with ArrowExportClient(host="localhost", port=5680) as client:
        print("Context manager connection established")
    print("Connection closed automatically")

    # Method 3: Using convenience function
    from proximadb_sdk.arrow_export import connect_arrow

    with connect_arrow(host="localhost", port=5680) as client:
        print("Convenience function connection established")

    # Method 4: With TLS for production
    # client = ArrowExportClient(
    #     host="production.server.com",
    #     port=5680,
    #     tls=True,
    #     auth_token="your-api-key"
    # )

    print("\nConnection examples completed.")
    return True


# =============================================================================
# Example 2: Listing Files in a Collection
# =============================================================================


def example_list_files(collection_id: str = "embeddings"):
    """
    List available files in a collection for export.

    ProximaDB stores data in different formats depending on the storage engine:
    - ArrowBlock (.arrow): Native Arrow IPC format from SST engine
    - Parquet (.parquet): Columnar format from Nova/VIPER engines
    - ProximaBlocks (.sst): Native SST format (converted on-the-fly)

    Args:
        collection_id: Name of the collection to list files from
    """
    from proximadb_sdk.arrow_export import ArrowExportClient, FileFormat

    print("\n" + "=" * 70)
    print("Example 2: Listing Files in a Collection")
    print("=" * 70)

    with ArrowExportClient() as client:
        # List all files in the collection
        print(f"\nListing files in collection '{collection_id}'...")
        files = client.list_files(collection_id)

        if not files:
            print(f"  No files found in collection '{collection_id}'")
            print("  (Collection may be empty or not exist)")
            return False

        print(f"  Found {len(files)} files:")
        for f in files:
            print(f"\n  File: {f.filename}")
            print(f"    Path: {f.path}")
            print(f"    Format: {f.format.value}")
            print(f"    Size: {f.size_bytes / 1024:.2f} KB")
            print(f"    Records: {f.total_records}")
            print(f"    Dimension: {f.dimension}")
            print(f"    Batches: {f.num_batches}")

        # Filter by format (e.g., only Arrow files)
        print(f"\nFiltering for Arrow files only...")
        arrow_files = client.list_files(collection_id, format_filter=FileFormat.ARROW)
        print(f"  Found {len(arrow_files)} Arrow files")

        # Filter by pattern (glob-style)
        print(f"\nFiltering by pattern 'block_*.arrow'...")
        pattern_files = client.list_files(collection_id, pattern="block_*.arrow")
        print(f"  Found {len(pattern_files)} matching files")

        # Get detailed info for a specific file
        if files:
            first_file = files[0]
            print(f"\nDetailed info for '{first_file.filename}':")
            file_info = client.get_file_info(first_file.path)
            print(f"  Total records: {file_info.total_records}")
            print(f"  Dimension: {file_info.dimension}")

            # Get schema
            schema = client.get_schema(first_file.path)
            print(f"\n  Schema:")
            for field in schema:
                print(f"    - {field.name}: {field.type}")

    return True


# =============================================================================
# Example 3: Reading Files into PyArrow Table
# =============================================================================


def example_read_pyarrow(collection_id: str = "embeddings"):
    """
    Read ProximaDB files into PyArrow Tables.

    Arrow Flight provides zero-copy data transfer, making this
    extremely efficient for large datasets.

    Args:
        collection_id: Name of the collection to read from
    """
    from proximadb_sdk.arrow_export import ArrowExportClient

    print("\n" + "=" * 70)
    print("Example 3: Reading Files into PyArrow Table")
    print("=" * 70)

    with ArrowExportClient() as client:
        # List files first
        files = client.list_files(collection_id)
        if not files:
            print(f"  No files found in collection '{collection_id}'")
            return False

        # Read a single file
        file_path = files[0].path
        print(f"\nReading file: {file_path}")

        table = client.read_file(file_path)

        print(f"\nPyArrow Table loaded:")
        print(f"  Rows: {table.num_rows}")
        print(f"  Columns: {table.num_columns}")
        print(f"  Schema: {table.schema}")

        # Access columns
        print(f"\nColumn names: {table.column_names}")

        # Preview data (first 3 rows)
        if table.num_rows > 0:
            print(f"\nFirst 3 records:")
            for i in range(min(3, table.num_rows)):
                row = {col: table[col][i].as_py() for col in table.column_names}
                # Truncate vector for display
                if "vector" in row and row["vector"]:
                    vec = row["vector"]
                    row["vector"] = (
                        f"[{vec[0]:.4f}, {vec[1]:.4f}, ... ({len(vec)} dims)]"
                    )
                print(f"    {i}: {row}")

        # Read entire collection (all files concatenated)
        print(f"\nReading entire collection '{collection_id}'...")
        full_table = client.read_collection(collection_id)
        print(f"  Total rows across all files: {full_table.num_rows}")

        # One-liner convenience function
        from proximadb_sdk.arrow_export import read_proximadb_file

        print("\nUsing convenience function:")
        table2 = read_proximadb_file(file_path)
        print(f"  Loaded {table2.num_rows} rows")

    return True


# =============================================================================
# Example 4: Converting to Polars DataFrame
# =============================================================================


def example_polars_conversion(collection_id: str = "embeddings"):
    """
    Convert ProximaDB data to Polars DataFrame for data analysis.

    Polars is a high-performance DataFrame library that works natively
    with Arrow, enabling zero-copy conversion.

    Args:
        collection_id: Name of the collection to convert
    """
    from proximadb_sdk.arrow_export import ArrowExportClient
    import polars as pl

    print("\n" + "=" * 70)
    print("Example 4: Converting to Polars DataFrame")
    print("=" * 70)

    with ArrowExportClient() as client:
        files = client.list_files(collection_id)
        if not files:
            print(f"  No files found in collection '{collection_id}'")
            return False

        file_path = files[0].path
        print(f"\nConverting {file_path} to Polars DataFrame...")

        # Direct conversion to Polars (zero-copy from Arrow)
        df = client.to_polars(file_path, rechunk=True)

        print(f"\nPolars DataFrame loaded:")
        print(f"  Shape: {df.shape}")
        print(f"  Columns: {df.columns}")
        print(f"  Dtypes: {df.dtypes}")

        # Polars operations
        print(f"\nDataFrame preview:")
        print(df.head(3))

        # Example: Filter and analyze with Polars
        if "metadata" in df.columns:
            print("\nMetadata analysis with Polars:")
            # Note: metadata structure depends on your data
            print(df.select(["id", "metadata"]).head(3))

        # Example: Get vector statistics
        if "vector" in df.columns:
            print("\nVector column info:")
            print(f"  Type: {df['vector'].dtype}")
            print(f"  Non-null count: {df['vector'].count()}")

        # Lazy evaluation example (for large datasets)
        print("\nLazy evaluation example:")
        lazy_df = df.lazy()
        result = lazy_df.select(["id"]).limit(5).collect()
        print(f"  Collected {len(result)} IDs")

    return True


# =============================================================================
# Example 5: Loading into DuckDB for SQL Analytics
# =============================================================================


def example_duckdb_analytics(collection_id: str = "embeddings"):
    """
    Load ProximaDB data into DuckDB for SQL-based analytics.

    DuckDB is an embedded OLAP database that integrates seamlessly
    with Arrow, perfect for ad-hoc analytics on vector data.

    Args:
        collection_id: Name of the collection to analyze
    """
    from proximadb_sdk.arrow_export import ArrowExportClient
    import duckdb

    print("\n" + "=" * 70)
    print("Example 5: Loading into DuckDB for SQL Analytics")
    print("=" * 70)

    with ArrowExportClient() as client:
        files = client.list_files(collection_id)
        if not files:
            print(f"  No files found in collection '{collection_id}'")
            return False

        file_path = files[0].path
        print(f"\nLoading {file_path} into DuckDB...")

        # Load into DuckDB (creates in-memory database)
        conn = client.to_duckdb(file_path, table_name="vectors")

        print("\nDuckDB table registered as 'vectors'")

        # Basic SQL queries
        print("\n--- SQL Query Examples ---")

        # Query 1: Count records
        result = conn.execute("SELECT COUNT(*) as count FROM vectors").fetchone()
        print(f"\n1. Total vectors: {result[0]}")

        # Query 2: Sample records
        print("\n2. Sample records:")
        result = conn.execute("""
            SELECT id
            FROM vectors
            LIMIT 5
        """).fetchall()
        for row in result:
            print(f"    ID: {row[0]}")

        # Query 3: Analyze vector dimensions (if stored as array)
        print("\n3. Vector array info:")
        try:
            result = conn.execute("""
                SELECT
                    array_length(vector) as dimension,
                    COUNT(*) as count
                FROM vectors
                GROUP BY dimension
            """).fetchall()
            for row in result:
                print(f"    Dimension {row[0]}: {row[1]} vectors")
        except Exception as e:
            print(f"    (Vector analysis not available: {e})")

        # Query 4: Export to pandas via DuckDB
        print("\n4. Export to pandas via DuckDB:")
        pandas_df = conn.execute("SELECT id FROM vectors LIMIT 3").fetchdf()
        print(pandas_df)

        # Query 5: Using multiple tables
        print("\n5. Loading multiple files into DuckDB:")
        if len(files) > 1:
            for i, f in enumerate(files[:3]):
                conn = client.to_duckdb(
                    f.path, table_name=f"vectors_{i}", conn=conn  # Reuse connection
                )
            print(f"    Registered {min(3, len(files))} tables")

            # List all tables
            tables = conn.execute("SHOW TABLES").fetchall()
            print(f"    Available tables: {[t[0] for t in tables]}")

        # Close connection
        conn.close()

    return True


# =============================================================================
# Example 6: Extracting Vectors as NumPy Arrays
# =============================================================================


def example_numpy_extraction(collection_id: str = "embeddings"):
    """
    Extract vectors as NumPy arrays for ML/scientific computing.

    This is useful for:
    - Training ML models with existing vectors
    - Computing custom similarity metrics
    - Visualizing embeddings (t-SNE, UMAP)
    - Clustering analysis

    Args:
        collection_id: Name of the collection to extract vectors from
    """
    from proximadb_sdk.arrow_export import ArrowExportClient
    import numpy as np

    print("\n" + "=" * 70)
    print("Example 6: Extracting Vectors as NumPy Arrays")
    print("=" * 70)

    with ArrowExportClient() as client:
        files = client.list_files(collection_id)
        if not files:
            print(f"  No files found in collection '{collection_id}'")
            return False

        file_path = files[0].path
        print(f"\nExtracting vectors from {file_path}...")

        # Extract vectors as NumPy array
        vectors = client.to_numpy(file_path, vector_column="vector")

        print(f"\nNumPy array created:")
        print(f"  Shape: {vectors.shape}")
        print(f"  Dtype: {vectors.dtype}")
        print(f"  Memory: {vectors.nbytes / 1024:.2f} KB")

        # Vector statistics
        if vectors.size > 0:
            print(f"\nVector statistics:")
            print(f"  Min: {np.min(vectors):.6f}")
            print(f"  Max: {np.max(vectors):.6f}")
            print(f"  Mean: {np.mean(vectors):.6f}")
            print(f"  Std: {np.std(vectors):.6f}")

            # L2 norms (useful for checking if vectors are normalized)
            norms = np.linalg.norm(vectors, axis=1)
            print(f"\nL2 norms:")
            print(f"  Min norm: {np.min(norms):.6f}")
            print(f"  Max norm: {np.max(norms):.6f}")
            print(f"  Mean norm: {np.mean(norms):.6f}")

            # Check if vectors are normalized (norm close to 1)
            is_normalized = np.allclose(norms, 1.0, atol=0.01)
            print(f"  Vectors are normalized: {is_normalized}")

            # Example: Compute pairwise cosine similarities
            if len(vectors) >= 2:
                print(f"\nPairwise cosine similarity (first 3 vectors):")
                subset = vectors[: min(3, len(vectors))]
                # Normalize for cosine similarity
                normalized = subset / np.linalg.norm(subset, axis=1, keepdims=True)
                similarity_matrix = normalized @ normalized.T
                print(similarity_matrix)

        # Example: Use with scikit-learn
        print("\nExample: Ready for scikit-learn operations")
        print("  from sklearn.cluster import KMeans")
        print("  kmeans = KMeans(n_clusters=5)")
        print("  labels = kmeans.fit_predict(vectors)")

    return True


# =============================================================================
# Example 7: Streaming Large Files in Batches
# =============================================================================


def example_streaming_batches(collection_id: str = "embeddings"):
    """
    Stream large files in batches for memory-efficient processing.

    This is essential for:
    - Processing files larger than available RAM
    - Incremental processing pipelines
    - Real-time data processing

    Args:
        collection_id: Name of the collection to stream
    """
    from proximadb_sdk.arrow_export import ArrowExportClient

    print("\n" + "=" * 70)
    print("Example 7: Streaming Large Files in Batches")
    print("=" * 70)

    with ArrowExportClient() as client:
        files = client.list_files(collection_id)
        if not files:
            print(f"  No files found in collection '{collection_id}'")
            return False

        file_path = files[0].path
        print(f"\nStreaming batches from {file_path}...")

        # Stream batches for memory-efficient processing
        total_rows = 0
        batch_count = 0

        for batch in client.read_batches(file_path):
            batch_count += 1
            total_rows += batch.num_rows

            print(f"\n  Batch {batch_count}:")
            print(f"    Rows: {batch.num_rows}")
            print(f"    Columns: {batch.num_columns}")
            print(f"    Schema: {batch.schema}")

            # Process each batch
            # Example: Extract vectors from this batch
            if "vector" in batch.schema.names:
                vector_col = batch.column("vector")
                print(f"    Vector column length: {len(vector_col)}")

            # Example: Convert batch to pandas for processing
            # pandas_batch = batch.to_pandas()
            # process_batch(pandas_batch)

            # Example: Convert batch to NumPy
            # import numpy as np
            # for i in range(batch.num_rows):
            #     vector = batch['vector'][i].as_py()
            #     # process vector

            # Limit batches for demo
            if batch_count >= 5:
                print(f"\n  ... (limiting demo to 5 batches)")
                break

        print(f"\nStreaming summary:")
        print(f"  Total batches processed: {batch_count}")
        print(f"  Total rows seen: {total_rows}")

        # Example: Memory-efficient aggregation pattern
        print("\nMemory-efficient aggregation pattern:")
        print("""
    running_sum = 0
    running_count = 0

    for batch in client.read_batches(file_path):
        # Process each batch without loading entire file
        batch_sum = compute_batch_sum(batch)
        running_sum += batch_sum
        running_count += batch.num_rows

    average = running_sum / running_count
        """)

    return True


# =============================================================================
# Example 8: Collection Statistics
# =============================================================================


def example_collection_stats(collection_id: str = "embeddings"):
    """
    Get comprehensive statistics for a collection.

    Useful for:
    - Monitoring collection growth
    - Capacity planning
    - Understanding data distribution across file formats

    Args:
        collection_id: Name of the collection to analyze
    """
    from proximadb_sdk.arrow_export import ArrowExportClient

    print("\n" + "=" * 70)
    print("Example 8: Collection Statistics")
    print("=" * 70)

    with ArrowExportClient() as client:
        print(f"\nGathering statistics for collection '{collection_id}'...")

        # Get collection statistics
        stats = client.collection_stats(collection_id)

        print(f"\nCollection Statistics:")
        print(f"  Collection ID: {stats['collection_id']}")
        print(f"  Number of files: {stats['num_files']}")
        print(f"  Total records: {stats['total_records']}")
        print(f"  Total size: {stats['total_size_mb']:.2f} MB")
        print(f"  Vector dimension: {stats['dimension']}")

        # Format breakdown
        if stats["formats"]:
            print(f"\n  File format breakdown:")
            for fmt, info in stats["formats"].items():
                print(f"    {fmt.upper()}:")
                print(f"      Files: {info['count']}")
                print(f"      Records: {info['records']}")
                print(f"      Size: {info['bytes'] / 1024:.2f} KB")

        # Calculate derived metrics
        if stats["total_records"] > 0 and stats["dimension"] > 0:
            # Assuming float32 vectors (4 bytes per dimension)
            theoretical_vector_size = stats["total_records"] * stats["dimension"] * 4
            compression_ratio = theoretical_vector_size / max(
                stats["total_size_bytes"], 1
            )

            print(f"\n  Derived metrics:")
            print(
                f"    Bytes per record: {stats['total_size_bytes'] / stats['total_records']:.2f}"
            )
            print(
                f"    Theoretical vector size: {theoretical_vector_size / (1024*1024):.2f} MB"
            )
            print(f"    Compression ratio: {compression_ratio:.2f}x")

        # List all files with details
        files = client.list_files(collection_id)
        if files:
            print(f"\n  Individual file details:")
            for f in files[:5]:  # Limit to first 5 files
                print(
                    f"    {f.filename}: {f.total_records} records, {f.size_bytes/1024:.1f} KB"
                )
            if len(files) > 5:
                print(f"    ... and {len(files) - 5} more files")

    return True


# =============================================================================
# Main Demo Function
# =============================================================================


def main():
    """
    Run all Arrow export examples.

    Requires a running ProximaDB server with data:
    - Arrow Flight endpoint on port 5680
    - At least one collection with vector data
    """
    print("=" * 70)
    print("ProximaDB Arrow Export Examples")
    print("=" * 70)
    print("\nThese examples demonstrate exporting ProximaDB data via Arrow Flight")
    print("for use with PyArrow, Polars, DuckDB, and NumPy.")
    print("\nPrerequisites:")
    print("  1. ProximaDB server running (Arrow Flight on port 5680)")
    print("  2. At least one collection with data")
    print("  3. Dependencies: pip install pyarrow polars duckdb numpy")

    # Check dependencies
    missing = check_dependencies()
    if missing:
        print(f"\nMissing dependencies: {', '.join(missing)}")
        print(f"Install with: pip install {' '.join(missing)}")
        print("\nExiting - please install dependencies first.")
        return 1

    print("\nAll dependencies installed.")

    # Try to connect and check if server is available
    print("\nChecking server connection...")
    try:
        from proximadb_sdk.arrow_export import ArrowExportClient

        with ArrowExportClient(host="localhost", port=5680) as client:
            # Try to list flights to verify connection
            # This will raise an exception if server is not available
            try:
                # Attempt a simple operation to verify connectivity
                list(client.client.list_flights(b"__test__"))
                server_available = True
            except Exception:
                # Server responded but no test collection - that's OK
                server_available = True

    except Exception as e:
        server_available = False
        print(f"\nCould not connect to Arrow Flight server: {e}")
        print("\nTo use these examples:")
        print("  1. Start ProximaDB server:")
        print("     cargo run --release --bin proximadb-server")
        print("")
        print("  2. Create a collection and insert data:")
        print("     python basic_usage.py")
        print("")
        print("  3. Run this example:")
        print("     python arrow_export_examples.py")
        print("")
        print("Server endpoints:")
        print("  - REST API: http://localhost:5678")
        print("  - gRPC: localhost:5679")
        print("  - Arrow Flight: localhost:5680")
        print("")
        print("Example connection code (for reference):")
        print("""
    from proximadb_sdk.arrow_export import ArrowExportClient

    # Connect to Arrow Flight server
    with ArrowExportClient(host="localhost", port=5680) as client:
        # List files in a collection
        files = client.list_files("my_collection")

        # Read into PyArrow Table
        table = client.read_file(files[0].path)

        # Convert to Polars DataFrame
        df = client.to_polars(files[0].path)

        # Load into DuckDB for SQL
        conn = client.to_duckdb(files[0].path)
        result = conn.execute("SELECT * FROM vectors LIMIT 10").fetchall()

        # Extract vectors as NumPy array
        vectors = client.to_numpy(files[0].path)
        print(f"Vectors shape: {vectors.shape}")
        """)
        return 1

    print("Server connection successful.")

    # Default collection name (change as needed)
    collection_id = "embeddings"

    print(f"\nRunning examples with collection: '{collection_id}'")
    print("(Change collection_id in main() if using a different collection)")
    print("-" * 70)

    # Run examples
    # Example 1 doesn't need a collection
    example_basic_connection()

    # These examples need a collection with data
    examples_requiring_data = [
        ("List Files", example_list_files),
        ("Read PyArrow", example_read_pyarrow),
        ("Polars Conversion", example_polars_conversion),
        ("DuckDB Analytics", example_duckdb_analytics),
        ("NumPy Extraction", example_numpy_extraction),
        ("Streaming Batches", example_streaming_batches),
        ("Collection Stats", example_collection_stats),
    ]

    for name, example_func in examples_requiring_data:
        try:
            result = example_func(collection_id)
            if not result:
                print(f"\n[{name}] No data available - skipped")
        except Exception as e:
            print(f"\n[{name}] Error: {e}")
            print(
                "  (This may be expected if the collection is empty or doesn't exist)"
            )

    print("\n" + "=" * 70)
    print("Arrow Export Examples Completed")
    print("=" * 70)
    print("\nKey takeaways:")
    print("  - Arrow Flight enables zero-copy data transfer")
    print("  - Use to_polars() for high-performance DataFrame operations")
    print("  - Use to_duckdb() for SQL analytics on vector data")
    print("  - Use to_numpy() for ML/scientific computing")
    print("  - Use read_batches() for memory-efficient streaming")
    print("  - Use collection_stats() for monitoring and capacity planning")

    return 0


if __name__ == "__main__":
    sys.exit(main())
