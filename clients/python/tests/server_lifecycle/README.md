# Server Lifecycle Tests

**⚠️ WARNING: These tests manage the ProximaDB server process (start/stop/restart)**

## Purpose

This directory contains tests that:
- Start/stop/restart the ProximaDB server
- Test server recovery and persistence
- Verify server lifecycle behavior
- Test server configuration changes

## Important Notes

### **DO NOT run with regular test suite**

These tests should be run **separately** and **in isolation** because they:
1. Kill running server instances (`pkill`)
2. Start new server processes
3. Restart the server mid-test
4. May interfere with other running tests

### **Running These Tests**

Run server lifecycle tests **separately**:

```bash
# Run ONLY server lifecycle tests (isolated)
pytest tests/server_lifecycle/ -v

# Or run individually
pytest tests/server_lifecycle/test_grpc_vector_get.py -v
```

### **DO NOT run with main test suite**

❌ **Avoid:**
```bash
pytest .  # This will kill server during test run!
```

✅ **Instead:**
```bash
# Run regular tests (with server already running)
pytest tests/unit/ tests/integration/ tests/e2e/ -v

# Then separately, run server lifecycle tests
pytest tests/server_lifecycle/ -v
```

## Test Categories

### Regular Tests (Assume Server Running)
- `tests/unit/` - Unit tests, no server needed
- `tests/integration/` - Integration tests with running server
- `tests/e2e/` - End-to-end tests with running server

### Server Lifecycle Tests (Manage Server)
- `tests/server_lifecycle/` - **THIS DIRECTORY** - Manages server process

## Current Tests

- `test_grpc_vector_get.py` - Tests gRPC operations with server restart

## Adding New Server Lifecycle Tests

If your test needs to:
- Kill the server
- Restart the server
- Test recovery after crash
- Change server configuration

**→ Put it in `tests/server_lifecycle/`**

Otherwise, put it in `tests/integration/` or `tests/e2e/` and assume the server is already running.
