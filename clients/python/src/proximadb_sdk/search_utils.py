"""Search utilities for ProximaDB Python SDK."""

import logging
from typing import Any

logger = logging.getLogger(__name__)


def build_search_optimization_rest(
    top_k: int | None = None,
    filters: dict[str, Any] | None = None,
    accuracy_threshold: float | None = None,
    include_expired: bool | None = None,
    timeout_ms: int | None = None,
    enable_two_stage: bool | None = None,
    quantization_hint: str | dict[str, Any] | None = None,
    enable_clustering_hint: bool | None = None,
    enable_metadata_filtering_hint: bool | None = None,
    custom_hints: dict[str, Any] | None = None,
    # Additional parameters from DirectVectorService
    distance_metric: str | None = None,
    requires_ordering: bool | None = None,
    candidate_multiplier: float | None = None,
    # Streaming search config
    streaming_buffer_size: int | None = None,
    streaming_concurrent_search: bool | None = None,
    streaming_max_concurrent_tasks: int | None = None,
    streaming_batch_size: int | None = None,
) -> dict[str, Any]:
    """Build search optimization parameters for REST API.

    Returns dict compatible with REST API SearchOptimization structure.
    """
    optimization = {}

    if top_k is not None:
        optimization["top_k"] = top_k
    if filters:
        optimization["filters"] = filters
    if accuracy_threshold is not None:
        optimization["accuracy_threshold"] = accuracy_threshold
    if include_expired is not None:
        optimization["include_expired"] = include_expired
    if timeout_ms is not None:
        optimization["timeout_ms"] = timeout_ms
    if enable_two_stage is not None:
        optimization["enable_two_stage"] = enable_two_stage

    # Handle quantization hint for REST
    if quantization_hint is not None:
        if isinstance(quantization_hint, str):
            hint_lower = quantization_hint.lower()
            if hint_lower in ["none", "no", "fp32", "float32"]:
                optimization["quantization_hint"] = {"hint_type": "none"}
            elif hint_lower in ["binary", "bin"]:
                optimization["quantization_hint"] = {"hint_type": "binary"}
            elif hint_lower in ["scalar", "int8"]:
                optimization["quantization_hint"] = {
                    "hint_type": "scalar",
                    "parameters": {"bits": 8},
                }
            elif hint_lower == "int16":
                optimization["quantization_hint"] = {
                    "hint_type": "scalar",
                    "parameters": {"bits": 16},
                }
            elif hint_lower.startswith("pq"):
                try:
                    bits = int(hint_lower[2:]) if len(hint_lower) > 2 else 8
                    optimization["quantization_hint"] = {
                        "hint_type": "product",
                        "parameters": {"num_subvectors": 8, "bits_per_code": bits},
                    }
                except ValueError:
                    optimization["quantization_hint"] = {
                        "hint_type": "product",
                        "parameters": {"num_subvectors": 8, "bits_per_code": 8},
                    }
        elif isinstance(quantization_hint, dict):
            optimization["quantization_hint"] = quantization_hint

    if enable_clustering_hint is not None:
        optimization["enable_clustering_hint"] = enable_clustering_hint
    if enable_metadata_filtering_hint is not None:
        optimization["enable_metadata_filtering_hint"] = enable_metadata_filtering_hint
    if custom_hints:
        optimization["custom_hints"] = custom_hints

    # Additional parameters
    if distance_metric:
        optimization["distance_metric"] = distance_metric
    if requires_ordering is not None:
        optimization["requires_ordering"] = requires_ordering
    if candidate_multiplier is not None:
        optimization["candidate_multiplier"] = candidate_multiplier

    # Add streaming config to custom hints if provided
    if any(
        [
            streaming_buffer_size,
            streaming_concurrent_search,
            streaming_max_concurrent_tasks,
            streaming_batch_size,
        ]
    ):
        if "custom_hints" not in optimization:
            optimization["custom_hints"] = {}
        if streaming_buffer_size is not None:
            optimization["custom_hints"][
                "streaming_buffer_size"
            ] = streaming_buffer_size
        if streaming_concurrent_search is not None:
            optimization["custom_hints"][
                "streaming_concurrent_search"
            ] = streaming_concurrent_search
        if streaming_max_concurrent_tasks is not None:
            optimization["custom_hints"][
                "streaming_max_concurrent_tasks"
            ] = streaming_max_concurrent_tasks
        if streaming_batch_size is not None:
            optimization["custom_hints"]["streaming_batch_size"] = streaming_batch_size

    return optimization


def build_search_params_grpc(
    top_k: int | None = None,
    filters: dict[str, Any] | None = None,
    accuracy_threshold: float | None = None,
    include_expired: bool | None = None,
    timeout_ms: int | None = None,
    enable_two_stage: bool | None = None,
    quantization_hint: str | dict[str, Any] | None = None,
    enable_clustering_hint: bool | None = None,
    enable_metadata_filtering_hint: bool | None = None,
    custom_hints: dict[str, Any] | None = None,
    # Additional parameters from DirectVectorService
    distance_metric: str | None = None,
    requires_ordering: bool | None = None,
    candidate_multiplier: float | None = None,
    # Streaming search config
    streaming_buffer_size: int | None = None,
    streaming_concurrent_search: bool | None = None,
    streaming_max_concurrent_tasks: int | None = None,
    streaming_batch_size: int | None = None,
) -> Any:
    """Build search params for gRPC API (v1 proto).

    Returns proximadb.v1.vector_types_pb2.SearchParams instance.

    Args:
        top_k: Number of results to return
        filters: Metadata filters to apply
        accuracy_threshold: Accuracy threshold for search (0.0-1.0)
        include_expired: Include expired vectors in results
        timeout_ms: Search timeout in milliseconds
        enable_two_stage: Enable two-stage search with quantization
        quantization_hint: Quantization hint - either string or dict with params
        enable_clustering_hint: Enable clustering optimization
        enable_metadata_filtering_hint: Enable metadata filtering optimization
        custom_hints: Custom optimization hints
        distance_metric: Distance metric override
        requires_ordering: Require ordered results
        candidate_multiplier: Candidate multiplier for search
        streaming_buffer_size: Streaming buffer size
        streaming_concurrent_search: Enable concurrent streaming search
        streaming_max_concurrent_tasks: Max concurrent streaming tasks
        streaming_batch_size: Streaming batch size

    Returns:
        proximadb.v1.vector_types_pb2.SearchParams instance

    Raises:
        ImportError: If gRPC proto modules are not available
    """
    try:
        from proximadb_sdk.v1 import types_pb2, vector_types_pb2
    except ImportError as e:
        raise ImportError(
            "Proto modules not available. Install with: pip install proximadb[grpc]"
        ) from e

    # Create SearchParams proto
    search_params = vector_types_pb2.SearchParams()

    # Set scalar optional fields
    if top_k is not None:
        search_params.top_k = top_k
    if accuracy_threshold is not None:
        search_params.accuracy_threshold = accuracy_threshold
    if include_expired is not None:
        search_params.include_expired = include_expired
    if timeout_ms is not None:
        search_params.timeout_ms = timeout_ms
    if enable_two_stage is not None:
        search_params.enable_two_stage = enable_two_stage
    if enable_clustering_hint is not None:
        search_params.enable_clustering_hint = enable_clustering_hint
    if enable_metadata_filtering_hint is not None:
        search_params.enable_metadata_filtering_hint = enable_metadata_filtering_hint

    # Build custom_hints map (map<string, SqlValue>)
    hints_dict = {}

    # Add custom hints if provided
    if custom_hints:
        for key, value in custom_hints.items():
            hints_dict[key] = value

    # Add additional parameters as custom hints
    if distance_metric:
        hints_dict["distance_metric"] = distance_metric
    if requires_ordering is not None:
        hints_dict["requires_ordering"] = requires_ordering
    if candidate_multiplier is not None:
        hints_dict["candidate_multiplier"] = candidate_multiplier

    # Add streaming config to custom hints
    if streaming_buffer_size is not None:
        hints_dict["streaming_buffer_size"] = streaming_buffer_size
    if streaming_concurrent_search is not None:
        hints_dict["streaming_concurrent_search"] = streaming_concurrent_search
    if streaming_max_concurrent_tasks is not None:
        hints_dict["streaming_max_concurrent_tasks"] = streaming_max_concurrent_tasks
    if streaming_batch_size is not None:
        hints_dict["streaming_batch_size"] = streaming_batch_size

    # Convert hints_dict to map<string, SqlValue>
    if hints_dict:
        for key, value in hints_dict.items():
            sql_value = _python_value_to_sql_value(value, types_pb2)
            search_params.custom_hints[key].CopyFrom(sql_value)

    return search_params


def _python_value_to_sql_value(value: Any, types_pb2: Any) -> Any:
    """Convert Python value to SqlValue proto without flattening rich values.

    Args:
        value: Python value
        types_pb2: proximadb.v1.types_pb2 module

    Returns:
        SqlValue instance
    """
    from google.protobuf.struct_pb2 import NullValue

    sql_value = types_pb2.SqlValue()
    if value is None:
        sql_value.null_value = NullValue.NULL_VALUE
    elif isinstance(value, bool):
        sql_value.bool_value = value
    elif isinstance(value, int) and not isinstance(value, bool):
        sql_value.int64_value = value
    elif isinstance(value, float):
        sql_value.number_value = value
    elif isinstance(value, str):
        sql_value.string_value = value
    elif isinstance(value, (bytes, bytearray, memoryview)):
        sql_value.bytes_value = bytes(value)
    elif isinstance(value, (list, tuple)):
        sql_value.array_value.values.extend(
            _python_value_to_sql_value(item, types_pb2) for item in value
        )
    elif isinstance(value, dict):
        for key, item in value.items():
            sql_value.object_value.fields[str(key)].CopyFrom(
                _python_value_to_sql_value(item, types_pb2)
            )
    else:
        sql_value.string_value = str(value)

    return sql_value


def build_search_hints(
    protocol: str,
    top_k: int | None = None,
    filters: dict[str, Any] | None = None,
    accuracy_threshold: float | None = None,
    include_expired: bool | None = None,
    timeout_ms: int | None = None,
    enable_two_stage: bool | None = None,
    quantization_hint: str | dict[str, Any] | None = None,
    enable_clustering_hint: bool | None = None,
    enable_metadata_filtering_hint: bool | None = None,
    custom_hints: dict[str, Any] | None = None,
    **kwargs,  # Accept additional parameters
) -> dict[str, Any] | Any:
    """Build search hints based on protocol type.

    Args:
        protocol: Either 'rest' or 'grpc'
        Other args: Search optimization parameters

    Returns:
        For REST: Dict with search optimization
        For gRPC: SearchParams proto message
    """
    if protocol.lower() == "rest":
        return build_search_optimization_rest(
            top_k=top_k,
            filters=filters,
            accuracy_threshold=accuracy_threshold,
            include_expired=include_expired,
            timeout_ms=timeout_ms,
            enable_two_stage=enable_two_stage,
            quantization_hint=quantization_hint,
            enable_clustering_hint=enable_clustering_hint,
            enable_metadata_filtering_hint=enable_metadata_filtering_hint,
            custom_hints=custom_hints,
            **kwargs,
        )
    elif protocol.lower() == "grpc":
        return build_search_params_grpc(
            top_k=top_k,
            filters=filters,
            accuracy_threshold=accuracy_threshold,
            include_expired=include_expired,
            timeout_ms=timeout_ms,
            enable_two_stage=enable_two_stage,
            quantization_hint=quantization_hint,
            enable_clustering_hint=enable_clustering_hint,
            enable_metadata_filtering_hint=enable_metadata_filtering_hint,
            custom_hints=custom_hints,
            **kwargs,
        )
    else:
        raise ValueError(f"Unknown protocol: {protocol}")
