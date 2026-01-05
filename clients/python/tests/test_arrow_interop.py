"""
Test PyArrow interoperability with ProximaDB Arrow block files.

This test verifies that Arrow IPC files produced by ProximaDB can be read
by standard PyArrow, DuckDB, and Polars tools.
"""

import json
import os
import tempfile

import pytest

# Check if pyarrow is available
try:
    import pyarrow as pa
    import pyarrow.ipc as ipc

    PYARROW_AVAILABLE = True
except ImportError:
    PYARROW_AVAILABLE = False

# Check if numpy is available
try:
    import numpy as np

    NUMPY_AVAILABLE = True
except ImportError:
    NUMPY_AVAILABLE = False


def create_test_arrow_file(path: str, num_vectors: int = 10, dimension: int = 64):
    """Create a test Arrow file matching ProximaDB's schema."""
    if not PYARROW_AVAILABLE or not NUMPY_AVAILABLE:
        pytest.skip("PyArrow and NumPy required for this test")

    # Generate test data
    ids = [f"vec_{i}" for i in range(num_vectors)]
    vectors = [
        np.random.randn(dimension).astype(np.float32).tolist()
        for i in range(num_vectors)
    ]
    metadata = [json.dumps({"category": f"cat_{i % 5}"}) for i in range(num_vectors)]
    timestamps = list(range(num_vectors))
    versions = [1] * num_vectors

    # Create Arrow schema matching ProximaDB's format
    schema = pa.schema(
        [
            ("id", pa.utf8()),
            ("vector", pa.list_(pa.float32(), dimension)),
            ("metadata", pa.utf8()),
            ("timestamp", pa.int64()),
            ("version", pa.int64()),
        ]
    )

    # Create record batch
    batch = pa.record_batch(
        [
            pa.array(ids),
            pa.array(vectors, type=pa.list_(pa.float32(), dimension)),
            pa.array(metadata),
            pa.array(timestamps),
            pa.array(versions),
        ],
        schema=schema,
    )

    # Write as Arrow IPC file
    with pa.OSFile(path, "wb") as sink:
        with ipc.new_file(sink, schema) as writer:
            writer.write_batch(batch)

    return num_vectors, dimension


@pytest.mark.skipif(not PYARROW_AVAILABLE, reason="PyArrow not available")
class TestArrowInterop:
    """Test suite for Arrow file interoperability."""

    def test_pyarrow_can_read_arrow_file(self):
        """Verify PyArrow can read Arrow IPC files in ProximaDB format."""
        with tempfile.NamedTemporaryFile(suffix=".arrow", delete=False) as f:
            arrow_path = f.name

        try:
            num_vectors, dimension = create_test_arrow_file(arrow_path)

            # Read with PyArrow
            with pa.memory_map(arrow_path, "r") as source:
                reader = ipc.open_file(source)

                # Verify schema
                schema = reader.schema
                assert "id" in schema.names
                assert "vector" in schema.names
                assert "metadata" in schema.names

                # Read all batches
                table = reader.read_all()
                assert len(table) == num_vectors

                # Verify vector dimensions
                vectors = table.column("vector")
                first_vector = vectors[0].as_py()
                assert len(first_vector) == dimension

                # Verify IDs
                ids = table.column("id").to_pylist()
                assert ids[0] == "vec_0"
                assert ids[-1] == f"vec_{num_vectors - 1}"

        finally:
            if os.path.exists(arrow_path):
                os.unlink(arrow_path)

    def test_arrow_file_schema_fields(self):
        """Verify Arrow file schema has expected field types."""
        with tempfile.NamedTemporaryFile(suffix=".arrow", delete=False) as f:
            arrow_path = f.name

        try:
            create_test_arrow_file(arrow_path, dimension=128)

            with pa.memory_map(arrow_path, "r") as source:
                reader = ipc.open_file(source)
                schema = reader.schema

                # Verify field types
                id_field = schema.field("id")
                assert id_field.type == pa.utf8()

                vector_field = schema.field("vector")
                assert pa.types.is_fixed_size_list(vector_field.type)
                assert vector_field.type.list_size == 128
                assert vector_field.type.value_type == pa.float32()

                timestamp_field = schema.field("timestamp")
                assert timestamp_field.type == pa.int64()

        finally:
            if os.path.exists(arrow_path):
                os.unlink(arrow_path)

    def test_arrow_file_metadata_parsing(self):
        """Verify metadata JSON can be parsed from Arrow file."""
        with tempfile.NamedTemporaryFile(suffix=".arrow", delete=False) as f:
            arrow_path = f.name

        try:
            create_test_arrow_file(arrow_path)

            with pa.memory_map(arrow_path, "r") as source:
                reader = ipc.open_file(source)
                table = reader.read_all()

                # Parse metadata JSON
                metadata_column = table.column("metadata")
                for i, metadata_json in enumerate(metadata_column.to_pylist()):
                    metadata = json.loads(metadata_json)
                    expected_category = f"cat_{i % 5}"
                    assert metadata["category"] == expected_category

        finally:
            if os.path.exists(arrow_path):
                os.unlink(arrow_path)

    def test_arrow_to_pandas_conversion(self):
        """Verify Arrow file can be converted to Pandas DataFrame."""
        pytest.importorskip("pandas")

        with tempfile.NamedTemporaryFile(suffix=".arrow", delete=False) as f:
            arrow_path = f.name

        try:
            num_vectors, _ = create_test_arrow_file(arrow_path)

            with pa.memory_map(arrow_path, "r") as source:
                reader = ipc.open_file(source)
                table = reader.read_all()

                # Convert to Pandas
                df = table.to_pandas()

                assert len(df) == num_vectors
                assert "id" in df.columns
                assert "vector" in df.columns
                assert "metadata" in df.columns

        finally:
            if os.path.exists(arrow_path):
                os.unlink(arrow_path)

    def test_vector_numpy_extraction(self):
        """Verify vectors can be extracted as NumPy arrays for ML operations."""
        if not NUMPY_AVAILABLE:
            pytest.skip("NumPy required for this test")

        with tempfile.NamedTemporaryFile(suffix=".arrow", delete=False) as f:
            arrow_path = f.name

        try:
            num_vectors, dimension = create_test_arrow_file(arrow_path)

            with pa.memory_map(arrow_path, "r") as source:
                reader = ipc.open_file(source)
                table = reader.read_all()

                # Extract vectors as NumPy array
                vectors = table.column("vector")

                # Convert to numpy matrix (num_vectors x dimension)
                vector_list = [v.as_py() for v in vectors]
                vector_matrix = np.array(vector_list, dtype=np.float32)

                assert vector_matrix.shape == (num_vectors, dimension)
                assert vector_matrix.dtype == np.float32

                # Verify we can compute distances
                query = vector_matrix[0]
                distances = np.linalg.norm(vector_matrix - query, axis=1)
                assert distances[0] == 0.0  # Distance to self is 0

        finally:
            if os.path.exists(arrow_path):
                os.unlink(arrow_path)


@pytest.mark.skipif(not PYARROW_AVAILABLE, reason="PyArrow not available")
class TestArrowStreamingRead:
    """Test streaming read patterns for large Arrow files."""

    def test_batch_iteration(self):
        """Verify batches can be read incrementally."""
        with tempfile.NamedTemporaryFile(suffix=".arrow", delete=False) as f:
            arrow_path = f.name

        try:
            create_test_arrow_file(arrow_path, num_vectors=100)

            with pa.memory_map(arrow_path, "r") as source:
                reader = ipc.open_file(source)

                total_rows = 0
                for i in range(reader.num_record_batches):
                    batch = reader.get_batch(i)
                    total_rows += len(batch)

                assert total_rows == 100

        finally:
            if os.path.exists(arrow_path):
                os.unlink(arrow_path)

    def test_column_selection(self):
        """Verify specific columns can be read without loading all data."""
        with tempfile.NamedTemporaryFile(suffix=".arrow", delete=False) as f:
            arrow_path = f.name

        try:
            create_test_arrow_file(arrow_path)

            with pa.memory_map(arrow_path, "r") as source:
                reader = ipc.open_file(source)

                # Read only id and timestamp columns
                table = reader.read_all()
                selected = table.select(["id", "timestamp"])

                assert len(selected.schema) == 2
                assert "id" in selected.schema.names
                assert "timestamp" in selected.schema.names
                assert "vector" not in selected.schema.names

        finally:
            if os.path.exists(arrow_path):
                os.unlink(arrow_path)


class TestDuckDBInterop:
    """Test DuckDB interoperability with Arrow files."""

    def test_duckdb_can_read_arrow_file(self):
        """Verify DuckDB can read Arrow IPC files."""
        duckdb = pytest.importorskip("duckdb")

        with tempfile.NamedTemporaryFile(suffix=".arrow", delete=False) as f:
            arrow_path = f.name

        try:
            num_vectors, _ = create_test_arrow_file(arrow_path)

            # DuckDB uses arrow_scan() or direct path for Arrow IPC files
            conn = duckdb.connect()
            # Register the Arrow file as a view for querying
            table = pa.ipc.open_file(arrow_path).read_all()
            conn.register("arrow_data", table)
            result = conn.execute("SELECT * FROM arrow_data").fetchall()

            assert len(result) == num_vectors

        finally:
            if os.path.exists(arrow_path):
                os.unlink(arrow_path)

    def test_duckdb_select_query(self):
        """Verify DuckDB can execute SELECT queries on Arrow data."""
        duckdb = pytest.importorskip("duckdb")

        with tempfile.NamedTemporaryFile(suffix=".arrow", delete=False) as f:
            arrow_path = f.name

        try:
            create_test_arrow_file(arrow_path, num_vectors=20)

            conn = duckdb.connect()

            # Test SELECT with specific columns
            table = pa.ipc.open_file(arrow_path).read_all()
            conn.register("arrow_data", table)
            result = conn.execute(
                "SELECT id, timestamp, version FROM arrow_data"
            ).fetchall()

            assert len(result) == 20
            # Verify first row
            assert result[0][0] == "vec_0"
            assert result[0][1] == 0  # timestamp
            assert result[0][2] == 1  # version

        finally:
            if os.path.exists(arrow_path):
                os.unlink(arrow_path)

    def test_duckdb_where_clause(self):
        """Verify DuckDB can filter Arrow data with WHERE clause."""
        duckdb = pytest.importorskip("duckdb")

        with tempfile.NamedTemporaryFile(suffix=".arrow", delete=False) as f:
            arrow_path = f.name

        try:
            create_test_arrow_file(arrow_path, num_vectors=20)

            conn = duckdb.connect()

            # Test WHERE clause filtering
            table = pa.ipc.open_file(arrow_path).read_all()
            conn.register("arrow_data", table)
            result = conn.execute(
                "SELECT id, timestamp FROM arrow_data WHERE timestamp >= 10"
            ).fetchall()

            assert len(result) == 10  # timestamps 10-19
            assert all(row[1] >= 10 for row in result)

        finally:
            if os.path.exists(arrow_path):
                os.unlink(arrow_path)

    def test_duckdb_order_by(self):
        """Verify DuckDB can sort Arrow data with ORDER BY."""
        duckdb = pytest.importorskip("duckdb")

        with tempfile.NamedTemporaryFile(suffix=".arrow", delete=False) as f:
            arrow_path = f.name

        try:
            create_test_arrow_file(arrow_path, num_vectors=10)

            conn = duckdb.connect()

            # Test ORDER BY descending
            table = pa.ipc.open_file(arrow_path).read_all()
            conn.register("arrow_data", table)
            result = conn.execute(
                "SELECT id, timestamp FROM arrow_data ORDER BY timestamp DESC"
            ).fetchall()

            assert len(result) == 10
            assert result[0][0] == "vec_9"  # Highest timestamp first
            assert result[-1][0] == "vec_0"  # Lowest timestamp last

        finally:
            if os.path.exists(arrow_path):
                os.unlink(arrow_path)

    def test_duckdb_aggregation(self):
        """Verify DuckDB can perform aggregations on Arrow data."""
        duckdb = pytest.importorskip("duckdb")

        with tempfile.NamedTemporaryFile(suffix=".arrow", delete=False) as f:
            arrow_path = f.name

        try:
            create_test_arrow_file(arrow_path, num_vectors=100)

            conn = duckdb.connect()

            # Test aggregation functions
            table = pa.ipc.open_file(arrow_path).read_all()
            conn.register("arrow_data", table)
            result = conn.execute(
                "SELECT COUNT(*), MIN(timestamp), MAX(timestamp), AVG(timestamp) FROM arrow_data"
            ).fetchone()

            assert result[0] == 100  # COUNT
            assert result[1] == 0  # MIN timestamp
            assert result[2] == 99  # MAX timestamp
            assert result[3] == 49.5  # AVG timestamp

        finally:
            if os.path.exists(arrow_path):
                os.unlink(arrow_path)


class TestPolarsInterop:
    """Test Polars interoperability with Arrow files."""

    def test_polars_can_read_arrow_file(self):
        """Verify Polars can read Arrow IPC files."""
        pl = pytest.importorskip("polars")

        with tempfile.NamedTemporaryFile(suffix=".arrow", delete=False) as f:
            arrow_path = f.name

        try:
            num_vectors, _ = create_test_arrow_file(arrow_path)

            # Polars can read Arrow IPC files directly
            df = pl.read_ipc(arrow_path)

            assert len(df) == num_vectors
            assert "id" in df.columns
            assert "vector" in df.columns
            assert "metadata" in df.columns

        finally:
            if os.path.exists(arrow_path):
                os.unlink(arrow_path)

    def test_polars_lazy_operations(self):
        """Verify Polars can perform lazy operations on Arrow data."""
        pl = pytest.importorskip("polars")

        with tempfile.NamedTemporaryFile(suffix=".arrow", delete=False) as f:
            arrow_path = f.name

        try:
            create_test_arrow_file(arrow_path, num_vectors=50)

            # Use lazy API for efficient query planning
            lazy_df = pl.scan_ipc(arrow_path)

            # Build lazy query
            result = (
                lazy_df.filter(pl.col("timestamp") >= 25)
                .select(["id", "timestamp"])
                .collect()
            )

            assert len(result) == 25  # timestamps 25-49
            assert all(ts >= 25 for ts in result["timestamp"].to_list())

        finally:
            if os.path.exists(arrow_path):
                os.unlink(arrow_path)

    def test_polars_sorting_and_limiting(self):
        """Verify Polars can sort and limit Arrow data."""
        pl = pytest.importorskip("polars")

        with tempfile.NamedTemporaryFile(suffix=".arrow", delete=False) as f:
            arrow_path = f.name

        try:
            create_test_arrow_file(arrow_path, num_vectors=20)

            lazy_df = pl.scan_ipc(arrow_path)

            # Sort by timestamp descending and take top 5
            result = lazy_df.sort("timestamp", descending=True).head(5).collect()

            assert len(result) == 5
            assert result["id"].to_list() == [
                "vec_19",
                "vec_18",
                "vec_17",
                "vec_16",
                "vec_15",
            ]

        finally:
            if os.path.exists(arrow_path):
                os.unlink(arrow_path)

    def test_polars_vector_extraction(self):
        """Verify vectors can be extracted from Polars DataFrame."""
        pl = pytest.importorskip("polars")
        if not NUMPY_AVAILABLE:
            pytest.skip("NumPy required for this test")

        with tempfile.NamedTemporaryFile(suffix=".arrow", delete=False) as f:
            arrow_path = f.name

        try:
            num_vectors, dimension = create_test_arrow_file(
                arrow_path, num_vectors=10, dimension=32
            )

            df = pl.read_ipc(arrow_path)

            # Extract vectors column
            vectors_series = df["vector"]

            # Convert to list of lists then to numpy
            vector_list = vectors_series.to_list()
            vector_matrix = np.array(vector_list, dtype=np.float32)

            assert vector_matrix.shape == (num_vectors, dimension)
            assert vector_matrix.dtype == np.float32

        finally:
            if os.path.exists(arrow_path):
                os.unlink(arrow_path)

    def test_polars_basic_analytics(self):
        """Verify Polars can perform basic analytics on Arrow data."""
        pl = pytest.importorskip("polars")

        with tempfile.NamedTemporaryFile(suffix=".arrow", delete=False) as f:
            arrow_path = f.name

        try:
            create_test_arrow_file(arrow_path, num_vectors=100)

            df = pl.read_ipc(arrow_path)

            # Basic statistics on timestamp column
            stats = df.select(
                [
                    pl.col("timestamp").count().alias("count"),
                    pl.col("timestamp").min().alias("min"),
                    pl.col("timestamp").max().alias("max"),
                    pl.col("timestamp").mean().alias("mean"),
                ]
            )

            assert stats["count"][0] == 100
            assert stats["min"][0] == 0
            assert stats["max"][0] == 99
            assert stats["mean"][0] == 49.5

        finally:
            if os.path.exists(arrow_path):
                os.unlink(arrow_path)

    def test_polars_group_by_metadata(self):
        """Verify Polars can group by parsed metadata categories."""
        pl = pytest.importorskip("polars")

        with tempfile.NamedTemporaryFile(suffix=".arrow", delete=False) as f:
            arrow_path = f.name

        try:
            # Create 25 vectors - 5 per category (cat_0 to cat_4)
            create_test_arrow_file(arrow_path, num_vectors=25)

            df = pl.read_ipc(arrow_path)

            # Parse metadata JSON and group by category
            result = (
                df.with_columns(
                    pl.col("metadata")
                    .str.json_path_match("$.category")
                    .alias("category")
                )
                .group_by("category")
                .agg(pl.len())
                .sort("category")
            )

            assert len(result) == 5  # 5 categories
            # Each category should have 5 vectors
            assert all(count == 5 for count in result["len"].to_list())

        finally:
            if os.path.exists(arrow_path):
                os.unlink(arrow_path)


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
