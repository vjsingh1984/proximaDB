# Server Lifecycle Tests

**⚠️ WARNING: These tests manage the ProximaDB server process (start/stop/restart)**

## Purpose

This directory contains tests that:
- Start/stop/restart the ProximaDB server
- Test server recovery and persistence
- Verify WAL persistence for vectors, graphs, and entities
- Test server lifecycle behavior
- Validate configuration changes

## Test Files

### 1. `test_comprehensive_recovery.py`
**Tests:** Vector + Graph + Entity recovery after restart

**What it does:**
- Creates vector collection with 10 vectors
- Creates graph with 5 nodes and 4 edges
- Inserts entity data
- Stops server
- Restarts server
- Verifies all data recovered

**Run:** `pytest tests/server_lifecycle/test_comprehensive_recovery.py -v`

### 2. `test_wal_persistence_detailed.py`
**Tests:** Detailed WAL file verification and recovery

**What it does:**
- Creates collection and gets actual storage path
- Inserts 20 vectors
- Verifies WAL files created in correct location (multi-disk layout)
- Stops server
- Restarts server
- Verifies vectors recovered from WAL
- Reports recovery percentage

**Run:** `pytest tests/server_lifecycle/test_wal_persistence_detailed.py -v`

### 3. `test_grpc_vector_get.py`
**Tests:** gRPC operations across server restart

**What it does:**
- Tests gRPC insert/get operations
- Verifies consistency between gRPC and REST
- Tests vector retrieval after insert

**Run:** `pytest tests/server_lifecycle/test_grpc_vector_get.py -v`

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
