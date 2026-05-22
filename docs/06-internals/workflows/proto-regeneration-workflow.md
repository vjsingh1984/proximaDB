# Protocol Buffer Regeneration Workflow

**Created**: 2026-05-01  
**Purpose**: Document how to regenerate Rust code from Protocol Buffer definitions

## Overview

ProximaDB uses Protocol Buffers (protobuf) for defining API contracts between services. When `.proto` files are modified, the corresponding Rust code must be regenerated.

## Architecture

```text
proto/proximadb/v1/*.proto
          ↓
./scripts/regenerate-proto.sh
          ↓
crates/foundation/proximadb-proto/src/proto/proximadb.v1.rs
```

## When to Regenerate Proto Files

Regenerate Rust code from proto files when:
1. ✅ You modify any `.proto` file in `proto/`
2. ✅ You add new message definitions
3. ✅ You add new service definitions
4. ✅ You modify existing service methods
5. ✅ You see compilation errors about missing proto types

## Regeneration Model

Proto files are currently **pre-generated** into the `proximadb-proto` workspace crate. The root
crate no longer owns generated `.rs` artifacts and `src/proto/mod.rs` is only a compatibility
re-export shim.

Normal builds consume the checked-in generated files under:

```text
crates/foundation/proximadb-proto/src/proto/
```

## Manual Regeneration

If you need to force regeneration (e.g., after git conflicts or stale build artifacts):

### Option 1: Use the convenience script

```bash
./scripts/regenerate-proto.sh
```

This script:
1. Cleans existing generated code
2. Touches proto files to trigger rebuild
3. Runs the pinned regeneration path for the proto workspace crate
4. Verifies regeneration succeeded

## Verifying Regeneration

After regeneration, verify that new types are available:

```bash
# Check for specific proto types
grep -c "CreateGraphWithEngineRequest" crates/foundation/proximadb-proto/src/proto/proximadb.v1.rs
grep -c "PulsarGraphStats" crates/foundation/proximadb-proto/src/proto/proximadb.v1.rs
grep -c "CrossShardQueryRequest" crates/foundation/proximadb-proto/src/proto/proximadb.v1.rs
```

Expected output: Count should be > 0 for each type.

## Proto File Locations

### Input Files (Definitions)
- `proto/proximadb/v1/graph.proto` - Graph service definitions
- `proto/proximadb/v1/vector.proto` - Vector service definitions
- `proto/proximadb/v1/document.proto` - Document service definitions
- `proto/proximadb/v1/sql.proto` - SQL query definitions
- `proto/proximadb/v1/catalog.proto` - Catalog service definitions
- `proto/proximadb/v1/cluster.proto` - Cluster service definitions
- `proto/proximadb/v1/context.proto` - Context definitions
- `proto/proximadb/v1/unified.proto` - Unified query definitions
- `proto/proximadb/v1/relations.proto` - Relationship definitions
- `proto/proximadb/explain.proto` - Query explanation definitions

### Output Files (Generated)
- `crates/foundation/proximadb-proto/src/proto/proximadb.v1.rs` - Main proto definitions
- `crates/foundation/proximadb-proto/src/proto/proximadb.v2.rs` - v2 proto definitions
- `crates/foundation/proximadb-proto/src/proto/proximadb.cluster.v1.rs` - Cluster-specific definitions
- `crates/foundation/proximadb-proto/src/proto/proximadb.explain.v1.rs` - Explanation-specific definitions
- `crates/foundation/proximadb-proto/src/proto/proximadb.streaming.v1.rs` - Streaming-specific definitions

## Build Configuration

### Build Dependencies (Cargo.toml)

```toml
[build-dependencies]
tonic-build = "0.14"
tonic-prost-build = "0.14"
```

### Build Script (build.rs)

The root `build.rs` does not regenerate protobuf Rust sources. It documents the current manual
workflow and keeps root builds from rebuilding the protocol surface unnecessarily.

## Troubleshooting

### Problem: "missing proto type" compilation errors

**Solution**: Regenerate proto files
```bash
./scripts/regenerate-proto.sh
```

### Problem: Proto types not found after regeneration

**Cause**: Generated code not in correct location or not compiled

**Solution**:
1. Check `crates/foundation/proximadb-proto/src/proto/proximadb.v1.rs` exists
2. Verify `crates/foundation/proximadb-proto/src/lib.rs` exports the module
3. Run `cargo clean && cargo build`

### Problem: Regeneration takes too long

**Cause**: Full rebuild every time

**Solution**: Proto files only rebuild when they change. If you want to skip proto regeneration:
```bash
# Build without proto regeneration (uses existing generated code)
cargo build
# Note: This will fail if proto files changed but code wasn't regenerated
```

### Problem: Proto regeneration conflicts

**Cause**: Merge conflicts in generated code

**Solution**:
```bash
# 1. Discard local changes to generated files
git checkout crates/foundation/proximadb-proto/src/proto/*.rs

# 2. Regenerate from clean state
./scripts/regenerate-proto.sh

# 3. Commit regenerated code
git add crates/foundation/proximadb-proto/src/proto/*.rs
git commit -m "chore: regenerate proto files"
```

## Adding New Proto Files

When adding a new `.proto` file:

1. **Create proto file** in `proto/proximadb/v1/`:
   ```bash
   touch proto/proximadb/v1/my_service.proto
   ```

2. **Add to build.rs**:
   ```rust
   tonic_build::configure()
       .build_server(true)
       .build_client(true)
       .compile_protos(
           &[
               // ... existing proto files
               "proto/proximadb/v1/my_service.proto",  // Add new file
           ],
           &["proto"],
       )?;
   ```

3. **Regenerate**:
   ```bash
   ./scripts/regenerate-proto.sh
   ```

4. **Use generated types**:
   ```rust
   use crate::proto::proximadb_v1::my_service::{MyRequest, MyResponse};
   ```

## CI/CD Integration

Generated code is checked in under the `proximadb-proto` crate for reproducible builds. CI should
validate that generated artifacts are current, but root crate builds should not regenerate them
implicitly.

## Best Practices

1. **Commit Generated Code**: Always commit regenerated `.rs` files
2. **Don't Edit Generated Files**: Hand edits will be overwritten
3. **Proto-First Design**: Define APIs in `.proto` files, not Rust
4. **Version Proto Files**: Use semantic versioning for breaking changes
5. **Document Changes**: Update this doc when adding new proto files

## Related Documentation

- [Protocol Buffers Guide](https://protobuf.dev/programming-guides/proto3/)
- [tonic-build Documentation](https://docs.rs/tonic-build/)
- [ProximaDB Architecture](../../05-concepts/architecture.adoc)

## Example: Adding a New gRPC Method

1. **Define in proto** (`proto/proximadb/v1/graph.proto`):
   ```protobuf
   service GraphService {
     rpc CreateGraphWithEngine(CreateGraphWithEngineRequest)
         returns (CreateGraphWithEngineResponse);
   }

   message CreateGraphWithEngineRequest {
     string name = 1;
     GraphConfig config = 2;
     GraphEngineType engine_type = 3;
   }

   message CreateGraphWithEngineResponse {
     string graph_id = 1;
   }
   ```

2. **Regenerate proto**:
   ```bash
   ./scripts/regenerate-proto.sh
   ```

3. **Implement in Rust** (`src/network/grpc/graph_service.rs`):
   ```rust
   use crate::proto::proximadb_v1::graph_service_server::GraphService;
   use crate::proto::proximadb_v1::{CreateGraphWithEngineRequest, CreateGraphWithEngineResponse};

   #[async_trait]
   impl GraphService for GraphServiceImpl {
     async fn create_graph_with_engine(
       &self,
       request: Request<CreateGraphWithEngineRequest>,
     ) -> Result<Response<CreateGraphWithEngineResponse>, Status> {
       // Implementation here
     }
   }
   ```

## Summary

- ✅ Manual regeneration script available for forced updates
- ✅ Generated code is committed under `crates/foundation/proximadb-proto/src/proto/`
- ✅ Root `src/proto/mod.rs` remains a compatibility re-export shim
- ✅ No protoc installation required for normal development builds

**Status**: Proto ownership moved to `proximadb-proto`; automatic regeneration remains a future build-system improvement.
