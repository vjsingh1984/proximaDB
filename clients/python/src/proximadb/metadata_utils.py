"""
Metadata conversion utilities for ProximaDB Python client.
Handles conversion between Python dict and typed proto MetadataItem.
"""

from typing import Dict, List, Any, Union
from . import proximadb_pb2 as pb2


def dict_to_proto_metadata(metadata: Dict[str, Any]) -> List[pb2.MetadataItem]:
    """
    Convert a Python dict to a list of typed MetadataItem protos.
    
    Args:
        metadata: Dictionary of metadata key-value pairs
        
    Returns:
        List of MetadataItem proto messages with typed values
    """
    items = []
    for key, value in metadata.items():
        item = pb2.MetadataItem()
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


def proto_metadata_to_dict(metadata_items: List[pb2.MetadataItem]) -> Dict[str, Any]:
    """
    Convert a list of typed MetadataItem protos to a Python dict.
    
    Args:
        metadata_items: List of MetadataItem proto messages
        
    Returns:
        Dictionary of metadata key-value pairs
    """
    result = {}
    for item in metadata_items:
        # Check which field is set using HasField
        if item.HasField('string_value'):
            result[item.key] = item.string_value
        elif item.HasField('number_value'):
            result[item.key] = item.number_value
        elif item.HasField('bool_value'):
            result[item.key] = item.bool_value
        else:
            # No value set, return None (Python convention)
            result[item.key] = None
    return result


def json_compatible_value(value: Any) -> Union[str, float, bool, None]:
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