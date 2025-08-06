"""Search utilities for ProximaDB Python SDK."""

from typing import Optional, Dict, Any, Union
import logging

logger = logging.getLogger(__name__)


def build_search_optimization_rest(
    top_k: Optional[int] = None,
    filters: Optional[Dict[str, Any]] = None,
    accuracy_threshold: Optional[float] = None,
    include_expired: Optional[bool] = None,
    timeout_ms: Optional[int] = None,
    enable_two_stage: Optional[bool] = None,
    quantization_hint: Optional[Union[str, Dict[str, Any]]] = None,
    enable_clustering_hint: Optional[bool] = None,
    enable_metadata_filtering_hint: Optional[bool] = None,
    custom_hints: Optional[Dict[str, Any]] = None,
    # Additional parameters from DirectVectorService
    distance_metric: Optional[str] = None,
    requires_ordering: Optional[bool] = None,
    candidate_multiplier: Optional[float] = None,
    # Streaming search config
    streaming_buffer_size: Optional[int] = None,
    streaming_concurrent_search: Optional[bool] = None,
    streaming_max_concurrent_tasks: Optional[int] = None,
    streaming_batch_size: Optional[int] = None
) -> Dict[str, Any]:
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
                optimization["quantization_hint"] = {
                    "hint_type": "none"
                }
            elif hint_lower in ["binary", "bin"]:
                optimization["quantization_hint"] = {
                    "hint_type": "binary"
                }
            elif hint_lower in ["scalar", "int8"]:
                optimization["quantization_hint"] = {
                    "hint_type": "scalar",
                    "parameters": {"bits": 8}
                }
            elif hint_lower == "int16":
                optimization["quantization_hint"] = {
                    "hint_type": "scalar",
                    "parameters": {"bits": 16}
                }
            elif hint_lower.startswith("pq"):
                try:
                    bits = int(hint_lower[2:]) if len(hint_lower) > 2 else 8
                    optimization["quantization_hint"] = {
                        "hint_type": "product",
                        "parameters": {
                            "num_subvectors": 8,
                            "bits_per_code": bits
                        }
                    }
                except ValueError:
                    optimization["quantization_hint"] = {
                        "hint_type": "product",
                        "parameters": {
                            "num_subvectors": 8,
                            "bits_per_code": 8
                        }
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
    if any([streaming_buffer_size, streaming_concurrent_search, 
            streaming_max_concurrent_tasks, streaming_batch_size]):
        if "custom_hints" not in optimization:
            optimization["custom_hints"] = {}
        if streaming_buffer_size is not None:
            optimization["custom_hints"]["streaming_buffer_size"] = streaming_buffer_size
        if streaming_concurrent_search is not None:
            optimization["custom_hints"]["streaming_concurrent_search"] = streaming_concurrent_search
        if streaming_max_concurrent_tasks is not None:
            optimization["custom_hints"]["streaming_max_concurrent_tasks"] = streaming_max_concurrent_tasks
        if streaming_batch_size is not None:
            optimization["custom_hints"]["streaming_batch_size"] = streaming_batch_size
        
    return optimization


def build_search_params_grpc(
    top_k: Optional[int] = None,
    filters: Optional[Dict[str, Any]] = None,
    accuracy_threshold: Optional[float] = None,
    include_expired: Optional[bool] = None,
    timeout_ms: Optional[int] = None,
    enable_two_stage: Optional[bool] = None,
    quantization_hint: Optional[Union[str, Dict[str, Any]]] = None,
    enable_clustering_hint: Optional[bool] = None,
    enable_metadata_filtering_hint: Optional[bool] = None,
    custom_hints: Optional[Dict[str, Any]] = None,
    # Additional parameters from DirectVectorService
    distance_metric: Optional[str] = None,
    requires_ordering: Optional[bool] = None,
    candidate_multiplier: Optional[float] = None,
    # Streaming search config
    streaming_buffer_size: Optional[int] = None,
    streaming_concurrent_search: Optional[bool] = None,
    streaming_max_concurrent_tasks: Optional[int] = None,
    streaming_batch_size: Optional[int] = None
) -> "pb2.SearchParams":  # type: ignore
    """Build SearchParams proto message for gRPC API.
    
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
        
    Returns:
        SearchParams proto message
    """
    try:
        from . import proximadb_pb2 as pb2
        from google.protobuf import struct_pb2
    except ImportError:
        logger.error("Failed to import proto definitions")
        raise
        
    params = pb2.SearchParams()
    
    if top_k is not None:
        params.top_k = top_k
        
    if filters:
        for key, value in filters.items():
            struct_val = struct_pb2.Value()
            if isinstance(value, str):
                struct_val.string_value = value
            elif isinstance(value, bool):
                struct_val.bool_value = value
            elif isinstance(value, (int, float)):
                struct_val.number_value = float(value)
            elif isinstance(value, list):
                list_val = struct_val.list_value
                for item in value:
                    item_val = list_val.values.add()
                    if isinstance(item, str):
                        item_val.string_value = item
                    elif isinstance(item, bool):
                        item_val.bool_value = item
                    elif isinstance(item, (int, float)):
                        item_val.number_value = float(item)
            params.filters[key].CopyFrom(struct_val)
            
    if accuracy_threshold is not None:
        params.accuracy_threshold = accuracy_threshold
        
    if include_expired is not None:
        params.include_expired = include_expired
        
    if timeout_ms is not None:
        params.timeout_ms = timeout_ms
        
    if enable_two_stage is not None:
        params.enable_two_stage = enable_two_stage
        
    # Handle quantization hint
    if quantization_hint is not None:
        if isinstance(quantization_hint, str):
            # Simple string hints
            hint_lower = quantization_hint.lower()
            if hint_lower in ["none", "no", "fp32", "float32"]:
                params.no_quantization = True
            elif hint_lower in ["binary", "bin"]:
                params.binary.CopyFrom(pb2.BinaryQuantizationParams())
            elif hint_lower in ["scalar", "int8"]:
                params.scalar.bits = 8
            elif hint_lower == "int16":
                params.scalar.bits = 16
            elif hint_lower.startswith("pq"):
                # Product quantization, e.g., "pq8", "pq4"
                try:
                    bits = int(hint_lower[2:]) if len(hint_lower) > 2 else 8
                    params.product.num_subvectors = 8  # Default
                    params.product.bits_per_code = bits
                except ValueError:
                    params.product.num_subvectors = 8
                    params.product.bits_per_code = 8
        elif isinstance(quantization_hint, dict):
            # Detailed quantization config
            hint_type = quantization_hint.get("type", "").lower()
            if hint_type == "binary":
                params.binary.CopyFrom(pb2.BinaryQuantizationParams())
            elif hint_type == "scalar":
                params.scalar.bits = quantization_hint.get("bits", 8)
            elif hint_type == "product" or hint_type == "pq":
                params.product.num_subvectors = quantization_hint.get("num_subvectors", 8)
                params.product.bits_per_code = quantization_hint.get("bits_per_code", 8)
            elif hint_type == "uniform":
                params.uniform.scale = quantization_hint.get("scale", 1.0)
                params.uniform.offset = quantization_hint.get("offset", 0.0)
                
    if enable_clustering_hint is not None:
        params.enable_clustering_hint = enable_clustering_hint
        
    if enable_metadata_filtering_hint is not None:
        params.enable_metadata_filtering_hint = enable_metadata_filtering_hint
        
    if custom_hints:
        for key, value in custom_hints.items():
            struct_val = struct_pb2.Value()
            if isinstance(value, str):
                struct_val.string_value = value
            elif isinstance(value, bool):
                struct_val.bool_value = value
            elif isinstance(value, (int, float)):
                struct_val.number_value = float(value)
            params.custom_hints[key].CopyFrom(struct_val)
            
    # Additional parameters stored in custom hints
    extra_hints = {}
    if distance_metric:
        extra_hints["distance_metric"] = distance_metric
    if requires_ordering is not None:
        extra_hints["requires_ordering"] = str(requires_ordering).lower()
    if candidate_multiplier is not None:
        extra_hints["candidate_multiplier"] = str(candidate_multiplier)
    if streaming_buffer_size is not None:
        extra_hints["streaming_buffer_size"] = str(streaming_buffer_size)
    if streaming_concurrent_search is not None:
        extra_hints["streaming_concurrent_search"] = str(streaming_concurrent_search).lower()
    if streaming_max_concurrent_tasks is not None:
        extra_hints["streaming_max_concurrent_tasks"] = str(streaming_max_concurrent_tasks)
    if streaming_batch_size is not None:
        extra_hints["streaming_batch_size"] = str(streaming_batch_size)
        
    for key, value in extra_hints.items():
        struct_val = struct_pb2.Value()
        struct_val.string_value = value
        params.custom_hints[key].CopyFrom(struct_val)
            
    return params


def build_search_hints(
    protocol: str,
    top_k: Optional[int] = None,
    filters: Optional[Dict[str, Any]] = None,
    accuracy_threshold: Optional[float] = None,
    include_expired: Optional[bool] = None,
    timeout_ms: Optional[int] = None,
    enable_two_stage: Optional[bool] = None,
    quantization_hint: Optional[Union[str, Dict[str, Any]]] = None,
    enable_clustering_hint: Optional[bool] = None,
    enable_metadata_filtering_hint: Optional[bool] = None,
    custom_hints: Optional[Dict[str, Any]] = None,
    **kwargs  # Accept additional parameters
) -> Union[Dict[str, Any], Any]:
    """Build search hints based on protocol type.
    
    Args:
        protocol: Either 'rest' or 'grpc'
        Other args: Search optimization parameters
        
    Returns:
        For REST: Dict with search optimization
        For gRPC: SearchParams proto message
    """
    if protocol.lower() == 'rest':
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
            **kwargs
        )
    elif protocol.lower() == 'grpc':
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
            **kwargs
        )
    else:
        raise ValueError(f"Unknown protocol: {protocol}")