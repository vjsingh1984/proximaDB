"""
Metadata conversion utilities for ProximaDB Python client.
Handles conversion between Python dict and typed proto MetadataItem.
"""

from typing import Any

try:
    from proximadb_sdk.v1 import vector_types_pb2 as v1_vector_types_pb2

    GRPC_AVAILABLE = True
except ImportError:
    GRPC_AVAILABLE = False
    v1_vector_types_pb2 = None


def _has_field(item: Any, field_name: str) -> bool:
    """Return whether a proto oneof/optional field is set and exists."""
    try:
        return bool(item.HasField(field_name))
    except (AttributeError, ValueError):
        return False


def dict_to_proto_metadata(metadata: dict[str, Any]) -> list:
    """
    Convert a Python dict to a list of typed MetadataValue protos (v1 API).

    Args:
        metadata: Dictionary of metadata key-value pairs

    Returns:
        List of MetadataValue proto messages with typed values (v1 API)
    """
    if not GRPC_AVAILABLE or v1_vector_types_pb2 is None:
        raise ImportError(
            "gRPC proto modules not available. Install grpcio and regenerate protos."
        )

    items = []
    for key, value in metadata.items():
        item = v1_vector_types_pb2.MetadataItem()
        item.key = key

        # Set the appropriate typed value
        if isinstance(value, bool):
            item.bool_value = value
        elif isinstance(value, (int, float)):
            item.number_value = float(value)
        elif isinstance(value, str):
            item.string_value = value
        elif value is None:
            # For None values, use empty string in proto
            item.string_value = ""
        else:
            # Convert other types to string
            item.string_value = str(value)

        items.append(item)
    return items


def proto_metadata_to_dict(metadata_items: list) -> dict[str, Any]:
    """
    Convert a list of typed MetadataValue protos (v1 API) to a Python dict.

    Args:
        metadata_items: List of MetadataValue proto messages (v1 API)

    Returns:
        Dictionary of metadata key-value pairs
    """
    result = {}
    for item in metadata_items:
        # Check which field is set using HasField
        if _has_field(item, "string_value"):
            result[item.key] = item.string_value
        elif _has_field(item, "number_value"):
            result[item.key] = item.number_value
        elif _has_field(item, "double_value"):
            result[item.key] = item.double_value
        elif _has_field(item, "int64_value"):
            result[item.key] = item.int64_value
        elif _has_field(item, "int_value"):
            result[item.key] = item.int_value
        elif _has_field(item, "bool_value"):
            result[item.key] = item.bool_value
        else:
            # No value set, return None (Python convention)
            result[item.key] = None
    return result


def json_compatible_value(value: Any) -> str | float | bool | None:
    """
    Convert a value to a JSON-compatible type for REST API.

    Args:
        value: Any metadata value

    Returns:
        JSON-compatible value (string, number, boolean, or null)
    """
    if isinstance(value, bool):
        return value
    elif isinstance(value, (int, float)):
        return float(value)
    elif isinstance(value, str):
        return value
    elif value is None:
        return None
    else:
        return str(value)
