# Deprecated Type Definitions

This document tracks deprecated type definitions that are being migrated to foundation types.

## Deprecated Types

### 1. DuckDBDistanceMetric
- **Location**: `src/connectors/duckdb.rs:332`
- **Deprecated Since**: v0.2.0
- **Replacement**: `proximadb_distance_types::DistanceMetric`
- **Reason**: Connector-specific type should use foundation types
- **Migration Path**:
  ```rust,ignore
  // Old (deprecated)
  use crate::connectors::duckdb::DuckDBDistanceMetric;
  
  // New (recommended)
  use proximadb_distance_types::DistanceMetric;
  ```
- **Conversion Traits**: ✅ Implemented (bidirectional `From` traits)

### 2. cluster::rpc::types::DistanceMetric
- **Location**: `src/cluster/rpc/types.rs:386`
- **Deprecated Since**: v0.2.0
- **Replacement**: `proximadb_distance_types::DistanceMetric`
- **Reason**: Wire protocol type should use foundation types
- **Migration Path**:
  ```rust,ignore
  // Old (deprecated)
  use crate::cluster::rpc::types::DistanceMetric;
  
  // New (recommended)
  use proximadb_distance_types::DistanceMetric;
  ```
- **Conversion Traits**: ✅ Implemented (bidirectional `From` traits)

## Non-Deprecated Types

### CompactDistanceMetric
- **Location**: `src/core/compact_enums.rs:12`
- **Status**: ✅ Not deprecated
- **Reason**: Memory-optimized storage (1 byte vs 4 bytes)
- **Purpose**: Efficient storage of distance metrics
- **Note**: This is a storage optimization, not a duplicate

## Migration Timeline

1. **Phase 1** (Current): Deprecation notices added
2. **Phase 2** (1-2 weeks): Update internal usages
3. **Phase 3** (1 month): Update public APIs
4. **Phase 4** (2 months): Remove deprecated types

## Compatibility

All deprecated types have conversion traits implemented for backward compatibility:
- `From<DeprecatedType> for FoundationType`
- `From<FoundationType> for DeprecatedType`

This allows gradual migration without breaking existing code.
