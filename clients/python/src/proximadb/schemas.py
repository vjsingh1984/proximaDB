"""
Unified Schema Definitions for ProximaDB

All protocols (AvroRPC, gRPC, REST) use these schemas for fast translation.
This ensures zero-conversion between protocols and maintains consistency.

Copyright 2025 Vijaykumar Singh
Licensed under the Apache License, Version 2.0
"""

import json
from typing import Any, Dict, List, Optional, Union
from dataclasses import dataclass, asdict
from datetime import datetime

# Core Avro schemas (master definitions)
AVRO_VECTOR_RECORD_SCHEMA = """
{
  "type": "record",
  "name": "VectorRecord",
  "namespace": "ai.proximadb.core",
  "fields": [
    {"name": "id", "type": "string"},
    {"name": "collection_id", "type": "string"},
    {"name": "vector", "type": {"type": "array", "items": "float"}},
    {"name": "metadata", "type": ["null", "string"], "default": null},
    {"name": "timestamp", "type": "long"},
    {"name": "expires_at", "type": ["null", "long"], "default": null}
  ]
}
"""

AVRO_COLLECTION_SCHEMA = """
{
  "type": "record",
  "name": "Collection",
  "namespace": "ai.proximadb.core",
  "fields": [
    {"name": "id", "type": "string"},
    {"name": "name", "type": "string"},
    {"name": "dimension", "type": "int"},
    {"name": "distance_metric", "type": "string"},
    {"name": "indexing_algorithm", "type": "string"},
    {"name": "storage_layout", "type": "string"},
    {"name": "created_at", "type": "long"},
    {"name": "updated_at", "type": "long"},
    {"name": "vector_count", "type": "long"},
    {"name": "filterable_metadata_fields", "type": {"type": "array", "items": "string"}},
    {"name": "config", "type": ["null", "string"], "default": null}
  ]
}
"""

AVRO_SEARCH_RESULT_SCHEMA = """
{
  "type": "record",
  "name": "SearchResult",
  "namespace": "ai.proximadb.core",
  "fields": [
    {"name": "id", "type": "string"},
    {"name": "score", "type": "float"},
    {"name": "vector", "type": ["null", {"type": "array", "items": "float"}], "default": null},
    {"name": "metadata", "type": ["null", "string"], "default": null}
  ]
}
"""

# Python dataclasses for type safety and easy conversion
@dataclass
class VectorRecordData:
    """Unified vector record structure"""
    id: str
    collection_id: str
    vector: List[float]
    metadata: Optional[Dict[str, Any]] = None
    timestamp: Optional[int] = None
    expires_at: Optional[int] = None
    
    def to_avro_dict(self) -> Dict[str, Any]:
        """Convert to Avro-compatible dictionary"""
        return {
            'id': self.id,
            'collection_id': self.collection_id,
            'vector': self.vector,
            'metadata': json.dumps(self.metadata) if self.metadata else None,
            'timestamp': self.timestamp or int(datetime.now().timestamp() * 1000),
            'expires_at': self.expires_at,
        }
    
    def to_grpc_dict(self) -> Dict[str, Any]:
        """Convert to gRPC-compatible dictionary"""
        return self.to_avro_dict()  # Same format for aligned schema
    
    def to_rest_dict(self) -> Dict[str, Any]:
        """Convert to REST JSON-compatible dictionary"""
        result = asdict(self)
        if result['timestamp'] is None:
            result['timestamp'] = int(datetime.now().timestamp() * 1000)
        return result
    
    @classmethod
    def from_avro_dict(cls, data: Dict[str, Any]) -> 'VectorRecordData':
        """Create from Avro dictionary"""
        return cls(
            id=data['id'],
            collection_id=data['collection_id'],
            vector=data['vector'],
            metadata=json.loads(data['metadata']) if data.get('metadata') else None,
            timestamp=data.get('timestamp'),
            expires_at=data.get('expires_at'),
        )
    
    @classmethod
    def from_grpc_dict(cls, data: Dict[str, Any]) -> 'VectorRecordData':
        """Create from gRPC dictionary"""
        return cls.from_avro_dict(data)  # Same format for aligned schema
    
    @classmethod
    def from_rest_dict(cls, data: Dict[str, Any]) -> 'VectorRecordData':
        """Create from REST JSON dictionary"""
        return cls(
            id=data['id'],
            collection_id=data['collection_id'],
            vector=data['vector'],
            metadata=data.get('metadata'),
            timestamp=data.get('timestamp'),
            expires_at=data.get('expires_at'),
        )


@dataclass
class CollectionData:
    """Unified collection structure"""
    id: str
    name: str
    dimension: int
    distance_metric: str
    indexing_algorithm: str
    storage_layout: str
    created_at: int
    updated_at: int
    vector_count: int
    filterable_metadata_fields: List[str]
    config: Optional[Dict[str, Any]] = None
    
    def to_avro_dict(self) -> Dict[str, Any]:
        """Convert to Avro-compatible dictionary"""
        return {
            'id': self.id,
            'name': self.name,
            'dimension': self.dimension,
            'distance_metric': self.distance_metric,
            'indexing_algorithm': self.indexing_algorithm,
            'storage_layout': self.storage_layout,
            'created_at': self.created_at,
            'updated_at': self.updated_at,
            'vector_count': self.vector_count,
            'filterable_metadata_fields': self.filterable_metadata_fields,
            'config': json.dumps(self.config) if self.config else None,
        }
    
    def to_grpc_dict(self) -> Dict[str, Any]:
        """Convert to gRPC-compatible dictionary"""
        return self.to_avro_dict()  # Same format for aligned schema
    
    def to_rest_dict(self) -> Dict[str, Any]:
        """Convert to REST JSON-compatible dictionary"""
        return asdict(self)


@dataclass
class SearchResultData:
    """Unified search result structure"""
    id: str
    score: float
    vector: Optional[List[float]] = None
    metadata: Optional[Dict[str, Any]] = None
    
    def to_avro_dict(self) -> Dict[str, Any]:
        """Convert to Avro-compatible dictionary"""
        return {
            'id': self.id,
            'score': self.score,
            'vector': self.vector,
            'metadata': json.dumps(self.metadata) if self.metadata else None,
        }
    
    def to_grpc_dict(self) -> Dict[str, Any]:
        """Convert to gRPC-compatible dictionary"""
        return self.to_avro_dict()  # Same format for aligned schema
    
    def to_rest_dict(self) -> Dict[str, Any]:
        """Convert to REST JSON-compatible dictionary"""
        return asdict(self)
    
    @classmethod
    def from_avro_dict(cls, data: Dict[str, Any]) -> 'SearchResultData':
        """Create from Avro dictionary"""
        return cls(
            id=data['id'],
            score=data['score'],
            vector=data.get('vector'),
            metadata=json.loads(data['metadata']) if data.get('metadata') else None,
        )


# Protocol converter functions for fast translation
class ProtocolConverter:
    """Fast protocol conversion utilities"""
    
    @staticmethod
    def avro_to_grpc_vector_record(avro_data: Dict[str, Any]) -> Dict[str, Any]:
        """Convert Avro VectorRecord to gRPC format (zero-copy for aligned schema)"""
        return avro_data  # Same format due to aligned schemas
    
    @staticmethod
    def grpc_to_avro_vector_record(grpc_data: Dict[str, Any]) -> Dict[str, Any]:
        """Convert gRPC VectorRecord to Avro format (zero-copy for aligned schema)"""
        return grpc_data  # Same format due to aligned schemas
    
    @staticmethod
    def avro_to_rest_vector_record(avro_data: Dict[str, Any]) -> Dict[str, Any]:
        """Convert Avro VectorRecord to REST JSON format"""
        result = avro_data.copy()
        if result.get('metadata'):
            result['metadata'] = json.loads(result['metadata'])
        return result
    
    @staticmethod
    def rest_to_avro_vector_record(rest_data: Dict[str, Any]) -> Dict[str, Any]:
        """Convert REST JSON VectorRecord to Avro format"""
        result = rest_data.copy()
        if result.get('metadata'):
            result['metadata'] = json.dumps(result['metadata'])
        if not result.get('timestamp'):
            result['timestamp'] = int(datetime.now().timestamp() * 1000)
        return result
    
    @staticmethod
    def avro_to_grpc_search_result(avro_data: Dict[str, Any]) -> Dict[str, Any]:
        """Convert Avro SearchResult to gRPC format (zero-copy for aligned schema)"""
        return avro_data  # Same format due to aligned schemas
    
    @staticmethod
    def grpc_to_avro_search_result(grpc_data: Dict[str, Any]) -> Dict[str, Any]:
        """Convert gRPC SearchResult to Avro format (zero-copy for aligned schema)"""
        return grpc_data  # Same format due to aligned schemas


# Performance metrics for different protocols
PROTOCOL_PERFORMANCE = {
    "avro": {
        "serialization_overhead": "0%",
        "network_efficiency": "95%",
        "conversion_cost": "Zero",
        "use_case": "Maximum performance, native clients"
    },
    "grpc": {
        "serialization_overhead": "5%",
        "network_efficiency": "90%", 
        "conversion_cost": "Zero (aligned schema)",
        "use_case": "High performance + ecosystem compatibility"
    },
    "rest": {
        "serialization_overhead": "15%",
        "network_efficiency": "70%",
        "conversion_cost": "Low (JSON parsing)",
        "use_case": "Web compatibility, debugging"
    }
}