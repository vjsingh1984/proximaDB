"""
ProximaDB Python Client - Arrow Flight Protocol Implementation

Copyright 2024 Vijaykumar Singh

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
"""

import json
from dataclasses import dataclass
from typing import Any, Dict, Iterator, List, Optional, Tuple

try:
    import pyarrow as pa
    import pyarrow.flight as flight

    ARROW_AVAILABLE = True
except ImportError:
    ARROW_AVAILABLE = False
    pa = None
    flight = None


@dataclass
class WriteMode:
    """Write mode for Arrow Flight operations."""

    WAL = "wal"  # WAL-backed writes (30-50K vectors/sec)
    DIRECT = "direct"  # Direct engine writes (100-200K vectors/sec)


@dataclass
class FlightPutResult:
    """Result from a DoPut operation."""

    success: bool
    vectors_inserted: int
    message: str
    metadata: Dict[str, Any]

    @property
    def records_processed(self) -> int:
        """Record-oriented alias for vectors_inserted."""
        return self.vectors_inserted

    @property
    def records_failed(self) -> int:
        """Number of failed records reported by the server, if present."""
        metrics = self.metadata.get("metrics", {})
        failed = metrics.get("failed_count")
        if failed is not None:
            return int(failed)
        return len(self.metadata.get("errors", []))


@dataclass
class FlightExchangeResult:
    """Result from a DoExchange bulk write operation."""

    success: bool
    records_processed: int
    records_failed: int
    batches_processed: int
    message: str
    progress: List[Dict[str, Any]]
    metadata: Dict[str, Any]


@dataclass
class FlightSearchResult:
    """Result from a DoGet search operation."""

    id: str
    vector: List[float]
    score: float
    metadata: Dict[str, Any]


class ArrowFlightClient:
    """
    Arrow Flight client for ProximaDB high-throughput bulk operations.

    Arrow Flight provides 100K-200K vectors/sec ingestion performance,
    making it ideal for:
    - Bulk data loading from data warehouses
    - Streaming ingestion from Spark/Arrow pipelines
    - High-throughput batch processing

    Features:
    - Zero-copy data transfer when possible
    - Streaming bulk insert (DoPut)
    - Streaming search results (DoGet)
    - Explicit flush/compact actions (DoAction)

    Usage:
        from proximadb_sdk.protocols.arrow_flight import ArrowFlightClient

        client = ArrowFlightClient("grpc://localhost:5678")  # unified port
        # or
        client = ArrowFlightClient("grpc://localhost:5680")  # legacy port

        # Bulk insert
        table = pa.table({
            "id": ["v1", "v2", "v3"],
            "vector": [[0.1] * 768, [0.2] * 768, [0.3] * 768],
            "metadata": [{"k": "v1"}, {"k": "v2"}, {"k": "v3"}]
        })
        result = client.bulk_insert("my_collection", table)

        # Search
        results = client.search("my_collection", [0.1] * 768, top_k=10)
    """

    def __init__(
        self,
        url: str,
        api_key: Optional[str] = None,
        timeout_seconds: float = 300.0,
        max_message_size_mb: int = 512,
    ):
        """
        Initialize Arrow Flight client.

        Args:
            url: Server URL (e.g., "grpc://localhost:5678" or "localhost:5680")
            api_key: Optional API key for authentication
            timeout_seconds: Request timeout in seconds (default: 5 min for bulk ops)
            max_message_size_mb: Maximum message size in MB (default: 512MB)
        """
        if not ARROW_AVAILABLE:
            raise ImportError(
                "PyArrow is required for Arrow Flight. Install with: pip install pyarrow>=14.0.0"
            )

        self._url = url
        self._api_key = api_key
        self._timeout_seconds = timeout_seconds
        self._max_message_size = max_message_size_mb * 1024 * 1024

        # Parse URL for Flight client
        self._location = self._parse_location(url)

        # Initialize client lazily
        self._client: Optional[flight.FlightClient] = None

    def _parse_location(self, url: str) -> "flight.Location":
        """Parse URL into Flight location."""
        # Remove protocol prefix if present
        if url.startswith("grpc://"):
            url = url[7:]
        elif url.startswith("grpc+tls://"):
            return flight.Location.for_grpc_tls(
                url[11:].split(":")[0], int(url.split(":")[-1])
            )
        elif url.startswith("http://"):
            url = url[7:]
        elif url.startswith("https://"):
            return flight.Location.for_grpc_tls(
                url[8:].split(":")[0], int(url.split(":")[-1])
            )

        # Default to grpc://
        if ":" in url:
            host, port = url.rsplit(":", 1)
            return flight.Location.for_grpc_tcp(host, int(port))
        else:
            return flight.Location.for_grpc_tcp(url, 5678)  # default unified port

    def _get_client(self) -> "flight.FlightClient":
        """Get or create Flight client (lazy initialization)."""
        if self._client is None:
            options = flight.FlightClientOptions(
                generic_options=[
                    ("grpc.max_send_message_length", self._max_message_size),
                    ("grpc.max_receive_message_length", self._max_message_size),
                ]
            )
            self._client = flight.FlightClient(self._location, options)

            # Authenticate if API key provided
            if self._api_key:
                self._authenticate()

        return self._client

    def _authenticate(self):
        """Authenticate with API key via handshake."""
        # Arrow Flight uses handshake for auth
        auth_handler = flight.ClientAuthHandler()
        # For now, we pass API key as metadata on each call
        pass

    def _get_call_options(self) -> "flight.FlightCallOptions":
        """Get call options with timeout and auth headers."""
        headers = []
        if self._api_key:
            headers.append((b"authorization", f"Bearer {self._api_key}".encode()))

        return flight.FlightCallOptions(
            timeout=self._timeout_seconds,
            headers=headers,
        )

    @staticmethod
    def _affected_count(result_data: Dict[str, Any], fallback: int) -> int:
        """Extract affected row count from ProximaDB batch metadata."""
        metrics = result_data.get("metrics", {})
        for key in ("successful_count", "total_processed"):
            value = metrics.get(key)
            if value is not None:
                return int(value)
        return fallback

    @staticmethod
    def _decode_metadata(payload: Any) -> Dict[str, Any]:
        """Decode JSON metadata returned by Flight DoPut/DoExchange."""
        if payload is None:
            return {}
        if hasattr(payload, "to_pybytes"):
            payload = payload.to_pybytes()
        if isinstance(payload, memoryview):
            payload = payload.tobytes()
        if isinstance(payload, str):
            payload = payload.encode()
        if not payload:
            return {}
        return json.loads(payload)

    @classmethod
    def _metadata_from_exchange_chunk(cls, chunk: Any) -> Dict[str, Any]:
        """Extract app_metadata from a PyArrow Flight exchange chunk."""
        if hasattr(chunk, "app_metadata"):
            return cls._decode_metadata(chunk.app_metadata)
        if hasattr(chunk, "data") and hasattr(chunk.data, "app_metadata"):
            return cls._decode_metadata(chunk.data.app_metadata)
        return {}

    @staticmethod
    def create_vector_schema(dimension: int) -> "pa.Schema":
        """
        Create Arrow schema for ProximaDB vectors.

        Schema:
        - id: utf8 (required)
        - vector: fixed_size_list<float32>(dimension) (required)
        - metadata: struct<key: utf8, value: utf8> (optional)
        - timestamp: int64 (optional)
        - score: float32 (for search results, optional)
        """
        return pa.schema(
            [
                pa.field("id", pa.utf8(), nullable=False),
                pa.field(
                    "vector",
                    pa.list_(pa.float32(), dimension),
                    nullable=False,
                ),
                pa.field(
                    "metadata",
                    pa.struct(
                        [
                            pa.field("key", pa.utf8()),
                            pa.field("value", pa.utf8()),
                        ]
                    ),
                    nullable=True,
                ),
                pa.field("timestamp", pa.int64(), nullable=True),
                pa.field("score", pa.float32(), nullable=True),
            ]
        )

    def bulk_insert(
        self,
        collection_id: str,
        data: "pa.Table",
        write_mode: str = WriteMode.WAL,
        trigger_compaction: bool = False,
        batch_size: int = 10000,
        operation: str = "insert",
    ) -> FlightPutResult:
        """
        Bulk insert/upsert records using Arrow Flight DoPut.

        Args:
            collection_id: Target collection ID
            data: Arrow Table with columns: id, vector, metadata (optional), timestamp (optional)
            write_mode: "wal" (safe) or "direct" (faster but less durable)
            trigger_compaction: Whether to trigger compaction after insert
            batch_size: Number of rows per batch for streaming
            operation: "insert" or "upsert"

        Returns:
            FlightPutResult with insert statistics

        Example:
            table = pa.table({
                "id": ["v1", "v2", "v3"],
                "vector": [[0.1] * 768, [0.2] * 768, [0.3] * 768],
            })
            result = client.bulk_insert("my_collection", table)
        """
        client = self._get_client()

        # Create FlightDescriptor with collection ID and options
        cmd = json.dumps(
            {
                "collection_id": collection_id,
                "operation": operation,
                "write_mode": write_mode,
                "trigger_compaction": trigger_compaction,
            }
        ).encode()

        # Create a new descriptor with cmd
        descriptor = flight.FlightDescriptor.for_command(cmd)

        # Stream data in batches
        total_rows = 0
        writer, reader = client.do_put(
            descriptor,
            data.schema,
            options=self._get_call_options(),
        )

        try:
            # Write data in batches
            for batch in data.to_batches(max_chunksize=batch_size):
                writer.write_batch(batch)
                total_rows += batch.num_rows

            # Close writer to signal end of stream
            writer.close()

            # Read result
            result_buf = reader.read()
            if result_buf:
                result_data = json.loads(result_buf.to_pybytes())
            else:
                result_data = {}

            affected = self._affected_count(result_data, total_rows)

            return FlightPutResult(
                success=result_data.get("success", True),
                vectors_inserted=affected,
                message=result_data.get("message", f"Bulk {operation} completed"),
                metadata=result_data,
            )

        except Exception as e:
            return FlightPutResult(
                success=False,
                vectors_inserted=0,
                message=str(e),
                metadata={},
            )

    def bulk_upsert(
        self,
        collection_id: str,
        data: "pa.Table",
        write_mode: str = WriteMode.WAL,
        trigger_compaction: bool = False,
        batch_size: int = 10000,
    ) -> FlightPutResult:
        """
        Bulk upsert records using Arrow Flight DoPut.

        This uses the v2 rich-record ingestion path and preserves supported
        Arrow scalar columns as typed record properties.
        """
        return self.bulk_insert(
            collection_id=collection_id,
            data=data,
            write_mode=write_mode,
            trigger_compaction=trigger_compaction,
            batch_size=batch_size,
            operation="upsert",
        )

    def bulk_delete(
        self,
        collection_id: str,
        ids: List[str],
        batch_size: int = 10000,
        trigger_compaction: bool = False,
    ) -> FlightPutResult:
        """
        Bulk delete records using Arrow Flight DoPut.

        The server accepts `id` or `oid`; the SDK sends `id`.
        """
        if not ARROW_AVAILABLE:
            raise ImportError(
                "PyArrow is required. Install with: pip install pyarrow>=14.0.0"
            )

        table = pa.table({"id": ids})
        return self.bulk_insert(
            collection_id=collection_id,
            data=table,
            write_mode=WriteMode.WAL,
            trigger_compaction=trigger_compaction,
            batch_size=batch_size,
            operation="delete",
        )

    def bulk_write_exchange(
        self,
        collection_id: str,
        data: "pa.Table",
        operation: str = "bulk_upsert",
        batch_size: int = 10000,
    ) -> FlightExchangeResult:
        """
        Stream bulk writes over Arrow Flight DoExchange.

        Args:
            collection_id: Target collection ID
            data: Arrow Table. Upsert/insert expects id/oid plus record columns;
                delete expects id or oid.
            operation: "insert"/"bulk_insert", "upsert"/"bulk_upsert", or
                "delete"/"bulk_delete"
            batch_size: Number of rows per streamed batch

        Returns:
            FlightExchangeResult with final metadata and per-batch progress.
        """
        operation = {
            "insert": "bulk_insert",
            "upsert": "bulk_upsert",
            "delete": "bulk_delete",
        }.get(operation, operation)
        if operation not in {"bulk_insert", "bulk_upsert", "bulk_delete"}:
            raise ValueError(
                "operation must be one of insert, upsert, delete, bulk_insert, bulk_upsert, or bulk_delete"
            )

        client = self._get_client()
        descriptor = flight.FlightDescriptor.for_path(operation, collection_id)
        writer, reader = client.do_exchange(
            descriptor,
            options=self._get_call_options(),
        )

        total_rows = 0
        progress: List[Dict[str, Any]] = []
        final_metadata: Dict[str, Any] = {}

        try:
            if hasattr(writer, "begin"):
                writer.begin(data.schema)

            for batch in data.to_batches(max_chunksize=batch_size):
                writer.write_batch(batch)
                total_rows += batch.num_rows

            writer.close()

            for chunk in reader:
                metadata = self._metadata_from_exchange_chunk(chunk)
                if not metadata:
                    continue
                if metadata.get("type") == "complete":
                    final_metadata = metadata
                else:
                    progress.append(metadata)

            records_processed = int(
                final_metadata.get("total_records", total_rows)
            )
            records_failed = int(final_metadata.get("total_failed", 0))
            batches_processed = int(
                final_metadata.get("total_batches", len(progress))
            )
            success = bool(final_metadata.get("success", records_failed == 0))

            return FlightExchangeResult(
                success=success,
                records_processed=records_processed,
                records_failed=records_failed,
                batches_processed=batches_processed,
                message=f"{operation} completed",
                progress=progress,
                metadata=final_metadata,
            )

        except Exception as e:
            return FlightExchangeResult(
                success=False,
                records_processed=0,
                records_failed=0,
                batches_processed=0,
                message=str(e),
                progress=progress,
                metadata=final_metadata,
            )

    def bulk_upsert_exchange(
        self,
        collection_id: str,
        data: "pa.Table",
        batch_size: int = 10000,
    ) -> FlightExchangeResult:
        """Bulk upsert records over DoExchange with progress metadata."""
        return self.bulk_write_exchange(
            collection_id=collection_id,
            data=data,
            operation="bulk_upsert",
            batch_size=batch_size,
        )

    def bulk_delete_exchange(
        self,
        collection_id: str,
        ids: List[str],
        batch_size: int = 10000,
    ) -> FlightExchangeResult:
        """Bulk delete records over DoExchange with progress metadata."""
        if not ARROW_AVAILABLE:
            raise ImportError(
                "PyArrow is required. Install with: pip install pyarrow>=14.0.0"
            )
        return self.bulk_write_exchange(
            collection_id=collection_id,
            data=pa.table({"id": ids}),
            operation="bulk_delete",
            batch_size=batch_size,
        )

    def bulk_insert_from_batches(
        self,
        collection_id: str,
        batches: Iterator["pa.RecordBatch"],
        schema: "pa.Schema",
        write_mode: str = WriteMode.WAL,
        trigger_compaction: bool = False,
        operation: str = "insert",
    ) -> FlightPutResult:
        """
        Stream RecordBatches directly for zero-copy bulk insert/upsert/delete.

        This is the most efficient method for large datasets as it avoids
        materializing the full dataset in memory.

        Args:
            collection_id: Target collection ID
            batches: Iterator of Arrow RecordBatches
            schema: Arrow schema for the data
            write_mode: "wal" (safe) or "direct" (faster)
            trigger_compaction: Whether to trigger compaction after insert
            operation: "insert", "upsert", or "delete"

        Returns:
            FlightPutResult with insert statistics
        """
        client = self._get_client()

        descriptor = flight.FlightDescriptor.for_command(
            json.dumps(
                {
                    "collection_id": collection_id,
                    "operation": operation,
                    "write_mode": write_mode,
                    "trigger_compaction": trigger_compaction,
                }
            ).encode()
        )

        total_rows = 0
        writer, reader = client.do_put(
            descriptor,
            schema,
            options=self._get_call_options(),
        )

        try:
            for batch in batches:
                writer.write_batch(batch)
                total_rows += batch.num_rows

            writer.close()

            result_buf = reader.read()
            result_data = json.loads(result_buf.to_pybytes()) if result_buf else {}
            affected = self._affected_count(result_data, total_rows)

            return FlightPutResult(
                success=result_data.get("success", True),
                vectors_inserted=affected,
                message=f"Bulk {operation} completed",
                metadata=result_data,
            )

        except Exception as e:
            return FlightPutResult(
                success=False,
                vectors_inserted=0,
                message=str(e),
                metadata={},
            )

    def search(
        self,
        collection_id: str,
        query_vector: List[float],
        top_k: int = 10,
        filter_metadata: Optional[Dict[str, Any]] = None,
        include_vectors: bool = False,
    ) -> List[FlightSearchResult]:
        """
        Search vectors using Arrow Flight DoGet.

        Args:
            collection_id: Collection to search
            query_vector: Query vector
            top_k: Number of results to return
            filter_metadata: Optional metadata filter
            include_vectors: Whether to include vectors in results

        Returns:
            List of FlightSearchResult objects
        """
        client = self._get_client()

        # Create search request as Ticket
        ticket_data = json.dumps(
            {
                "collection_id": collection_id,
                "query": query_vector,
                "top_k": top_k,
                "filter": filter_metadata,
                "include_vectors": include_vectors,
            }
        ).encode()

        ticket = flight.Ticket(ticket_data)

        # Execute search
        reader = client.do_get(ticket, options=self._get_call_options())

        # Read results
        results = []
        for chunk in reader:
            batch = chunk.data
            for i in range(batch.num_rows):
                result = FlightSearchResult(
                    id=batch.column("id")[i].as_py(),
                    vector=batch.column("vector")[i].as_py() if include_vectors else [],
                    score=(
                        batch.column("score")[i].as_py()
                        if "score" in batch.schema.names
                        else 0.0
                    ),
                    metadata={},
                )
                results.append(result)

        return results

    def search_batch(
        self,
        collection_id: str,
        query_vectors: List[List[float]],
        top_k: int = 10,
    ) -> List[List[FlightSearchResult]]:
        """
        Batch search for multiple query vectors.

        More efficient than individual searches for multiple queries.

        Args:
            collection_id: Collection to search
            query_vectors: List of query vectors
            top_k: Number of results per query

        Returns:
            List of result lists, one per query vector
        """
        # TODO: Implement batch search protocol
        # For now, fall back to sequential searches
        return [self.search(collection_id, qv, top_k) for qv in query_vectors]

    def flush_collection(self, collection_id: str) -> bool:
        """
        Flush collection WAL to storage engine.

        Args:
            collection_id: Collection to flush

        Returns:
            True if successful
        """
        return self._do_action("flush_collection", {"collection_id": collection_id})

    def compact_collection(self, collection_id: str) -> bool:
        """
        Trigger compaction on a collection.

        Args:
            collection_id: Collection to compact

        Returns:
            True if successful
        """
        return self._do_action("compact_collection", {"collection_id": collection_id})

    def flush_and_compact(self, collection_id: str) -> bool:
        """
        Flush and compact a collection.

        Args:
            collection_id: Collection to flush and compact

        Returns:
            True if successful
        """
        return self._do_action("flush_and_compact", {"collection_id": collection_id})

    def _do_action(self, action_type: str, body: Dict[str, Any]) -> bool:
        """Execute a DoAction request."""
        client = self._get_client()

        action = flight.Action(action_type, json.dumps(body).encode())

        try:
            results = list(client.do_action(action, options=self._get_call_options()))
            return True
        except Exception as e:
            print(f"Action {action_type} failed: {e}")
            return False

    def list_actions(self) -> List[Tuple[str, str]]:
        """
        List available actions.

        Returns:
            List of (action_type, description) tuples
        """
        client = self._get_client()

        try:
            actions = list(client.list_actions(options=self._get_call_options()))
            return [(a.type, a.description) for a in actions]
        except Exception as e:
            print(f"Failed to list actions: {e}")
            return []

    def get_schema(self, collection_id: str) -> Optional["pa.Schema"]:
        """
        Get schema for a collection.

        Args:
            collection_id: Collection ID

        Returns:
            Arrow schema or None if not found
        """
        client = self._get_client()

        descriptor = flight.FlightDescriptor.for_path(collection_id)

        try:
            schema_result = client.get_schema(
                descriptor, options=self._get_call_options()
            )
            return schema_result.schema
        except Exception as e:
            print(f"Failed to get schema: {e}")
            return None

    def close(self):
        """Close the client connection."""
        if self._client is not None:
            self._client.close()
            self._client = None


# Convenience function for creating Arrow tables from Python data
def vectors_to_arrow_table(
    ids: List[str],
    vectors: List[List[float]],
    metadata: Optional[List[Dict[str, Any]]] = None,
    timestamps: Optional[List[int]] = None,
) -> "pa.Table":
    """
    Convert Python data to Arrow Table for bulk insert.

    Args:
        ids: List of vector IDs
        vectors: List of vectors (each a list of floats)
        metadata: Optional list of metadata dicts
        timestamps: Optional list of timestamps (nanoseconds)

    Returns:
        Arrow Table ready for bulk_insert()

    Example:
        table = vectors_to_arrow_table(
            ids=["v1", "v2", "v3"],
            vectors=[[0.1] * 768, [0.2] * 768, [0.3] * 768],
            metadata=[{"category": "A"}, {"category": "B"}, {"category": "C"}]
        )
    """
    if not ARROW_AVAILABLE:
        raise ImportError(
            "PyArrow is required. Install with: pip install pyarrow>=14.0.0"
        )

    if len(ids) != len(vectors):
        raise ValueError("ids and vectors must have same length")

    if len(vectors) == 0:
        raise ValueError("vectors cannot be empty")

    dimension = len(vectors[0])

    # Build arrays
    id_array = pa.array(ids, type=pa.utf8())

    # Vector array as fixed-size list
    flat_vectors = [v for vec in vectors for v in vec]
    vector_array = pa.FixedSizeListArray.from_arrays(
        pa.array(flat_vectors, type=pa.float32()),
        dimension,
    )

    # Metadata array (optional)
    if metadata:
        meta_arrays = []
        for m in metadata:
            if m:
                # Take first key-value pair for simplicity
                key = next(iter(m.keys()), "")
                value = str(next(iter(m.values()), ""))
                meta_arrays.append({"key": key, "value": value})
            else:
                meta_arrays.append(None)
        metadata_array = pa.array(
            meta_arrays,
            type=pa.struct(
                [
                    pa.field("key", pa.utf8()),
                    pa.field("value", pa.utf8()),
                ]
            ),
        )
    else:
        metadata_array = pa.nulls(
            len(ids),
            type=pa.struct(
                [
                    pa.field("key", pa.utf8()),
                    pa.field("value", pa.utf8()),
                ]
            ),
        )

    # Timestamp array (optional)
    if timestamps:
        timestamp_array = pa.array(timestamps, type=pa.int64())
    else:
        timestamp_array = pa.nulls(len(ids), type=pa.int64())

    # Score array (null for inserts)
    score_array = pa.nulls(len(ids), type=pa.float32())

    return pa.table(
        {
            "id": id_array,
            "vector": vector_array,
            "metadata": metadata_array,
            "timestamp": timestamp_array,
            "score": score_array,
        }
    )


def arrow_table_to_vectors(
    table: "pa.Table",
) -> Tuple[List[str], List[List[float]], List[Optional[Dict[str, Any]]]]:
    """
    Convert Arrow Table back to Python data.

    Args:
        table: Arrow Table with id, vector, metadata columns

    Returns:
        Tuple of (ids, vectors, metadata)
    """
    if not ARROW_AVAILABLE:
        raise ImportError(
            "PyArrow is required. Install with: pip install pyarrow>=14.0.0"
        )

    ids = table.column("id").to_pylist()
    vectors = [list(v) for v in table.column("vector").to_pylist()]

    metadata = []
    if "metadata" in table.schema.names:
        for m in table.column("metadata").to_pylist():
            if m and isinstance(m, dict):
                metadata.append(m)
            else:
                metadata.append(None)
    else:
        metadata = [None] * len(ids)

    return ids, vectors, metadata
