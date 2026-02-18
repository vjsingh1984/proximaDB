# Server Lifecycle Tests - How to Run

## ⚠️ CRITICAL WARNING

**DO NOT run these tests while the server is being used for other purposes!**

These tests will:
1. **Kill any running ProximaDB server**
2. Start a new server instance
3. Run tests against it
4. Stop the server when done

## Prerequisites

1. **Build the server** (release mode for best performance):
```bash
cd /Users/vijay.singh/code/proximaDB
cargo build --release
```

2. **Ensure no other tests are running**
3. **Ensure no production server is running** (or you're OK with it being killed)

## Running Server Lifecycle Tests

### Option 1: Run the single test

```bash
cd /Users/vijay.singh/code/proximaDB/clients/python

# Set Python path
export PYTHONPATH=src

# Run ONLY the server lifecycle test (it will manage server)
pytest tests/server_lifecycle/test_grpc_vector_get.py -v
```

### Option 2: Run all server lifecycle tests

```bash
# This will run all tests in the directory with warnings
pytest tests/server_lifecycle/ -v
```

## Expected Output

```
⚠️  WARNING: Running Server Lifecycle Tests
================================================================
These tests will START/STOP/RESTART the ProximaDB server.
Do NOT run these tests concurrently with other test suites!
================================================================

🚀 Starting ProximaDB server...
✅ Server started

🔍 Testing gRPC VectorGet Fix
============================================================

📦 Creating collection: test_grpc_get_1234567890

🧪 Test 1: gRPC insert -> gRPC get
✅ Vectors inserted via gRPC
✅ Found vector vec_001 via gRPC
   - ID: vec_001
   - Has vector: True
   - Has metadata: True

...

🛑 Stopping server...

✅ Server Lifecycle Tests Complete
================================================================
```

## Troubleshooting

### Server binary not found

```
❌ Server binary not found at: /path/to/target/release/proximadb-server
   Please build with: cargo build --release
```

**Fix:** Build the server in release mode

### Config file not found

**Fix:** Ensure `config/config.toml` exists in project root

### Server won't stop

If server hangs, manually kill it:
```bash
pkill -9 proximadb-server
```

### Port already in use

If you get "Address already in use" error:
```bash
# Kill any existing server
pkill -9 proximadb-server

# Wait for port release
sleep 2

# Try again
pytest tests/server_lifecycle/test_grpc_vector_get.py -v
```

## Test Structure Validation

To verify the test is properly structured without running it:

```bash
# Syntax check
python3 tests/server_lifecycle/test_grpc_vector_get.py --help 2>&1 | head -5

# Import check
python3 -c "import sys; sys.path.insert(0, 'src'); from tests.server_lifecycle import test_grpc_vector_get; print('✅ Test imports successfully')"

# Pytest collection check (won't run test)
pytest tests/server_lifecycle/ --collect-only
```

Expected collection output:
```
<Module test_grpc_vector_get.py>
  <Function test_grpc_vector_get>
```

## Integration with CI/CD

For CI/CD pipelines, run server lifecycle tests in a **separate job**:

```yaml
# .github/workflows/test.yml
jobs:
  regular-tests:
    # Regular tests with pre-started server

  server-lifecycle-tests:
    needs: regular-tests  # Run after regular tests
    # Server lifecycle tests that manage their own server
    steps:
      - run: pytest tests/server_lifecycle/ -v
```

## When to Add Tests Here

Add tests to `tests/server_lifecycle/` when testing:
- ✅ Server restart behavior
- ✅ Crash recovery
- ✅ Configuration reloading
- ✅ Graceful shutdown
- ✅ Persistence after restart

**DO NOT add** regular feature tests here - those belong in `tests/integration/`.
