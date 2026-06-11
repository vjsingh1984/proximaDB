"""Offline unit tests for proximadb_sdk.batching_unified.

Fully offline: no network, no server. The "backend" flush function is a plain
mock/callable. time.sleep is patched everywhere a loop could spin so the suite
stays well under the timeout.
"""

import asyncio
import time
from concurrent.futures import Future
from unittest.mock import MagicMock, patch

import pytest

from proximadb_sdk.batching_unified import (
    AsyncBatchProcessor,
    BatchConfig,
    BatchMetrics,
    BatchOperationType,
    BatchProcessor,
    BatchRequest,
    BatchStrategy,
    RequestBatcher,
    RestBatchProcessor,
    ThreadedBatchProcessor,
    UnifiedBatchManager,
    VectorBatcher,
    batch_insert_records,
    batch_insert_vectors,
    create_vector_batcher,
)
from proximadb_sdk.models import VectorRecord


# --------------------------------------------------------------------------
# Dataclasses / enums / simple containers
# --------------------------------------------------------------------------

def test_batch_strategy_values():
    assert BatchStrategy.SIZE_BASED == "size_based"
    assert BatchStrategy.TIME_BASED == "time_based"
    assert BatchStrategy.ADAPTIVE == "adaptive"
    assert BatchStrategy.HYBRID == "hybrid"
    assert BatchStrategy.IMMEDIATE == "immediate"


def test_batch_operation_type_values():
    assert BatchOperationType.INSERT_RECORDS == "insert_records"
    assert BatchOperationType.UPSERT_RECORDS == "upsert_records"
    assert BatchOperationType.INSERT_VECTORS == "insert_vectors"
    assert BatchOperationType.DELETE_VECTORS == "delete_vectors"
    assert BatchOperationType.SEARCH_VECTORS == "search_vectors"


def test_batch_config_defaults():
    cfg = BatchConfig()
    assert cfg.max_batch_size == 1000
    assert cfg.min_batch_size == 10
    assert cfg.strategy == BatchStrategy.HYBRID
    assert cfg.enable_compression is True


def test_batch_metrics_defaults():
    m = BatchMetrics()
    assert m.total_requests == 0
    assert m.total_batches == 0
    assert isinstance(m.last_updated, float)


def test_batch_request_defaults_and_ordering():
    r1 = BatchRequest(priority=5)
    r2 = BatchRequest(priority=1)
    # __lt__ : higher priority sorts first
    assert (r1 < r2) is True
    assert (r2 < r1) is False
    assert isinstance(r1.request_id, str)
    assert isinstance(r1.timestamp, float)


# --------------------------------------------------------------------------
# BatchProcessor._estimate_request_size (via concrete subclass)
# --------------------------------------------------------------------------

def _make_threaded(config=None):
    return ThreadedBatchProcessor(config or BatchConfig(), MagicMock())


def test_estimate_size_none_data():
    proc = _make_threaded()
    req = BatchRequest(data=None)
    assert proc._estimate_request_size(req) == 0.001


def test_estimate_size_vector_record_list():
    proc = _make_threaded()
    vecs = [VectorRecord(id="a", vector=[0.1] * 128), VectorRecord(id="b", vector=[0.2] * 128)]
    req = BatchRequest(data=vecs)
    size = proc._estimate_request_size(req)
    assert size > 0


def test_estimate_size_non_vector_list():
    proc = _make_threaded()
    req = BatchRequest(data=[{"x": 1}, {"y": 2}])
    assert proc._estimate_request_size(req) == 0.01


def test_estimate_size_default():
    proc = _make_threaded()
    req = BatchRequest(data={"some": "dict"})
    assert proc._estimate_request_size(req) == 0.01


def test_get_metrics_returns_metrics():
    proc = _make_threaded()
    assert isinstance(proc.get_metrics(), BatchMetrics)


# --------------------------------------------------------------------------
# ThreadedBatchProcessor
# --------------------------------------------------------------------------

def test_threaded_submit_not_running_raises():
    proc = _make_threaded()
    req = BatchRequest(operation=BatchOperationType.INSERT_RECORDS, collection_id="c")
    with pytest.raises(RuntimeError):
        proc.submit_request(req)


def test_threaded_collect_batch_size_limit():
    cfg = BatchConfig(max_batch_size=2)
    proc = ThreadedBatchProcessor(cfg, MagicMock())
    key = "insert_records_c"
    for i in range(5):
        proc._request_queues[key].append(
            BatchRequest(operation=BatchOperationType.INSERT_RECORDS, collection_id="c", data=i)
        )
    batch = proc._collect_batch(key)
    assert len(batch) == 2


def test_threaded_collect_batch_empty():
    proc = _make_threaded()
    assert proc._collect_batch("nope") == []


def test_threaded_collect_batch_time_limit():
    # max_wait_time_ms=0 forces the time-limit break after first append
    cfg = BatchConfig(max_batch_size=100, max_wait_time_ms=0.0)
    proc = ThreadedBatchProcessor(cfg, MagicMock())
    key = "insert_records_c"
    for i in range(5):
        proc._request_queues[key].append(
            BatchRequest(operation=BatchOperationType.INSERT_RECORDS, collection_id="c", data=i)
        )
    batch = proc._collect_batch(key)
    # First item appended unconditionally, then time-limit break
    assert len(batch) == 1


def test_threaded_execute_batch_sync_distributes_list_results():
    fn = MagicMock(return_value=["r0", "r1"])
    proc = ThreadedBatchProcessor(BatchConfig(), fn)
    reqs = []
    for i in range(2):
        r = BatchRequest(operation=BatchOperationType.INSERT_RECORDS, collection_id="c", data=i)
        r.future = Future()
        reqs.append(r)
    proc._execute_batch_sync("insert_records_c", reqs)
    assert reqs[0].future.result() == "r0"
    assert reqs[1].future.result() == "r1"
    assert proc.metrics.total_batches == 1
    assert proc.metrics.total_requests == 2
    fn.assert_called_once()


def test_threaded_execute_batch_sync_scalar_result():
    fn = MagicMock(return_value={"ok": True})
    proc = ThreadedBatchProcessor(BatchConfig(), fn)
    r = BatchRequest(operation=BatchOperationType.INSERT_RECORDS, collection_id="c", data=1)
    r.future = Future()
    proc._execute_batch_sync("insert_records_c", [r])
    assert r.future.result() == {"ok": True}


def test_threaded_execute_batch_sync_exception_propagates():
    fn = MagicMock(side_effect=ValueError("boom"))
    proc = ThreadedBatchProcessor(BatchConfig(), fn)
    r = BatchRequest(operation=BatchOperationType.INSERT_RECORDS, collection_id="c", data=1)
    r.future = Future()
    proc._execute_batch_sync("insert_records_c", [r])
    with pytest.raises(ValueError):
        r.future.result()


def test_threaded_execute_batch_sync_empty_noop():
    proc = _make_threaded()
    proc._execute_batch_sync("k", [])
    assert proc.metrics.total_batches == 0


def test_threaded_update_metrics_averages():
    proc = _make_threaded()
    proc._update_metrics(10, 50.0)
    proc._update_metrics(20, 70.0)
    assert proc.metrics.total_batches == 2
    assert proc.metrics.avg_batch_size == 15.0
    assert proc.metrics.avg_latency_ms == 60.0


@patch("proximadb_sdk.batching_unified.time.sleep", return_value=None)
def test_threaded_full_submit_flow(_sleep):
    """End-to-end through real processing thread with mocked backend."""
    fn = MagicMock(return_value=[{"status": "ok"}])
    cfg = BatchConfig(max_batch_size=1)
    proc = ThreadedBatchProcessor(cfg, fn)
    proc.start()
    assert proc._running is True
    req = BatchRequest(operation=BatchOperationType.INSERT_RECORDS, collection_id="c", data=1)
    result = proc.submit_request(req)
    assert result == {"status": "ok"}
    proc.stop()
    assert proc._running is False
    assert proc._processing_threads == {}


@patch("proximadb_sdk.batching_unified.time.sleep", return_value=None)
def test_threaded_processing_loop_stops_on_event(_sleep):
    import threading

    proc = ThreadedBatchProcessor(BatchConfig(), MagicMock())
    ev = threading.Event()
    ev.set()  # already stopped -> loop exits immediately
    proc._batch_processing_loop("k", ev)  # returns without hanging


def test_threaded_stop_idempotent_when_empty():
    proc = _make_threaded()
    proc.start()
    proc.stop()
    # second stop should not blow up
    proc._running = True
    proc.stop()


# --------------------------------------------------------------------------
# AsyncBatchProcessor
# --------------------------------------------------------------------------

def test_async_should_execute_empty_false():
    proc = AsyncBatchProcessor(BatchConfig(), MagicMock())
    assert asyncio.run(proc._should_execute_batch("missing")) is False


def test_async_should_execute_immediate():
    cfg = BatchConfig(strategy=BatchStrategy.IMMEDIATE)
    proc = AsyncBatchProcessor(cfg, MagicMock())
    proc._batches["k"].append(BatchRequest(data=1))
    assert asyncio.run(proc._should_execute_batch("k")) is True


def test_async_should_execute_size_based():
    cfg = BatchConfig(strategy=BatchStrategy.SIZE_BASED, max_batch_size=2)
    proc = AsyncBatchProcessor(cfg, MagicMock())
    proc._batches["k"].extend([BatchRequest(data=1), BatchRequest(data=2)])
    assert asyncio.run(proc._should_execute_batch("k")) is True


def test_async_should_execute_size_based_below_threshold():
    cfg = BatchConfig(strategy=BatchStrategy.SIZE_BASED, max_batch_size=5)
    proc = AsyncBatchProcessor(cfg, MagicMock())
    proc._batches["k"].append(BatchRequest(data=1))
    assert asyncio.run(proc._should_execute_batch("k")) is False


def test_async_should_execute_time_based():
    cfg = BatchConfig(strategy=BatchStrategy.TIME_BASED, max_wait_time_ms=1.0)
    proc = AsyncBatchProcessor(cfg, MagicMock())
    old = BatchRequest(data=1)
    old.timestamp = time.time() - 10  # 10s old -> way past threshold
    proc._batches["k"].append(old)
    assert asyncio.run(proc._should_execute_batch("k")) is True


def test_async_should_execute_hybrid_neither():
    cfg = BatchConfig(strategy=BatchStrategy.HYBRID, max_batch_size=100, max_wait_time_ms=100000.0)
    proc = AsyncBatchProcessor(cfg, MagicMock())
    proc._batches["k"].append(BatchRequest(data=1))
    assert asyncio.run(proc._should_execute_batch("k")) is False


def test_async_start_stop():
    async def run():
        proc = AsyncBatchProcessor(BatchConfig(), MagicMock())
        await proc.start()
        assert proc._running is True
        await proc.stop()
        assert proc._running is False

    asyncio.run(run())


def test_async_submit_not_running_raises():
    async def run():
        proc = AsyncBatchProcessor(BatchConfig(), MagicMock())
        req = BatchRequest(operation=BatchOperationType.INSERT_RECORDS, collection_id="c", data=1)
        with pytest.raises(RuntimeError):
            await proc.submit_request(req)

    asyncio.run(run())


def test_async_submit_immediate_executes():
    async def run():
        async def execute_fn(op, cid, data):
            return [{"status": "ok"} for _ in data]

        cfg = BatchConfig(strategy=BatchStrategy.IMMEDIATE)
        proc = AsyncBatchProcessor(cfg, execute_fn)
        await proc.start()
        req = BatchRequest(operation=BatchOperationType.INSERT_RECORDS, collection_id="c", data=1)
        result = await proc.submit_request(req)
        assert result == {"status": "ok"}
        assert proc.metrics.total_batches == 1
        await proc.stop()

    asyncio.run(run())


def test_async_submit_sets_timer_then_stop_flushes():
    async def run():
        calls = []

        async def execute_fn(op, cid, data):
            calls.append(list(data))
            return ["done" for _ in data]

        # Hybrid with big thresholds -> not executed on submit, timer set instead
        cfg = BatchConfig(strategy=BatchStrategy.HYBRID, max_batch_size=1000, max_wait_time_ms=100000.0)
        proc = AsyncBatchProcessor(cfg, execute_fn)
        await proc.start()
        req = BatchRequest(operation=BatchOperationType.INSERT_RECORDS, collection_id="c", data=1)
        task = asyncio.create_task(proc.submit_request(req))
        await asyncio.sleep(0)  # let submit register the timer
        # timer registered
        assert len(proc._timers) == 1
        # stop() processes remaining batches, fulfilling the future
        await proc.stop()
        result = await task
        assert result == "done"
        assert calls == [[1]]

    asyncio.run(run())


def test_async_batch_timer_flushes():
    async def run():
        async def execute_fn(op, cid, data):
            return ["t" for _ in data]

        cfg = BatchConfig(strategy=BatchStrategy.TIME_BASED, max_wait_time_ms=1.0)
        proc = AsyncBatchProcessor(cfg, execute_fn)
        await proc.start()
        req = BatchRequest(operation=BatchOperationType.INSERT_RECORDS, collection_id="c", data=1)
        req.future = asyncio.Future()
        proc._batches["insert_records_c"].append(req)
        await proc._batch_timer("insert_records_c")
        assert req.future.result() == "t"
        assert proc._batches["insert_records_c"] == []

    asyncio.run(run())


def test_async_batch_timer_no_requests():
    async def run():
        cfg = BatchConfig(max_wait_time_ms=1.0)
        proc = AsyncBatchProcessor(cfg, MagicMock())
        await proc._batch_timer("empty_key")  # nothing to do, no error

    asyncio.run(run())


def test_async_execute_batch_empty_noop():
    async def run():
        proc = AsyncBatchProcessor(BatchConfig(), MagicMock())
        await proc._execute_batch("k", [])
        assert proc.metrics.total_batches == 0

    asyncio.run(run())


def test_async_execute_batch_scalar_result():
    async def run():
        async def execute_fn(op, cid, data):
            return "single"

        proc = AsyncBatchProcessor(BatchConfig(), execute_fn)
        reqs = []
        for i in range(2):
            r = BatchRequest(operation=BatchOperationType.INSERT_RECORDS, collection_id="c", data=i)
            r.future = asyncio.Future()
            reqs.append(r)
        await proc._execute_batch("insert_records_c", reqs)
        assert reqs[0].future.result() == "single"
        assert reqs[1].future.result() == "single"

    asyncio.run(run())


def test_async_execute_batch_exception():
    async def run():
        async def execute_fn(op, cid, data):
            raise RuntimeError("nope")

        proc = AsyncBatchProcessor(BatchConfig(), execute_fn)
        r = BatchRequest(operation=BatchOperationType.INSERT_RECORDS, collection_id="c", data=1)
        r.future = asyncio.Future()
        await proc._execute_batch("insert_records_c", [r])
        with pytest.raises(RuntimeError):
            r.future.result()

    asyncio.run(run())


def test_async_update_metrics():
    proc = AsyncBatchProcessor(BatchConfig(), MagicMock())
    proc._update_metrics(4, 8.0)
    assert proc.metrics.avg_batch_size == 4.0
    assert proc.metrics.avg_latency_ms == 8.0


def test_async_submit_size_executes_and_cancels_existing_timer():
    async def run():
        async def execute_fn(op, cid, data):
            return ["x" for _ in data]

        # size-based threshold of 2; submit two, second triggers execution.
        cfg = BatchConfig(strategy=BatchStrategy.SIZE_BASED, max_batch_size=2)
        proc = AsyncBatchProcessor(cfg, execute_fn)
        await proc.start()
        # First submit will not hit threshold -> timer set
        r1 = BatchRequest(operation=BatchOperationType.INSERT_RECORDS, collection_id="c", data=1)
        t1 = asyncio.create_task(proc.submit_request(r1))
        await asyncio.sleep(0)
        # Second submit hits the size threshold and flushes both
        r2 = BatchRequest(operation=BatchOperationType.INSERT_RECORDS, collection_id="c", data=2)
        res2 = await proc.submit_request(r2)
        res1 = await t1
        assert res1 == "x"
        assert res2 == "x"
        await proc.stop()

    asyncio.run(run())


# --------------------------------------------------------------------------
# UnifiedBatchManager
# --------------------------------------------------------------------------

def test_manager_get_processor_rest():
    mgr = UnifiedBatchManager()
    proc = mgr.get_processor("rest", MagicMock())
    assert isinstance(proc, ThreadedBatchProcessor)
    # cached
    assert mgr.get_processor("rest", MagicMock()) is proc


def test_manager_get_processor_grpc():
    mgr = UnifiedBatchManager(BatchConfig(max_batch_size=5))
    proc = mgr.get_processor("grpc", MagicMock())
    assert isinstance(proc, AsyncBatchProcessor)


def test_manager_processor_id_distinct():
    mgr = UnifiedBatchManager()
    a = mgr.get_processor("rest", MagicMock(), processor_id="a")
    b = mgr.get_processor("rest", MagicMock(), processor_id="b")
    assert a is not b


def test_manager_get_all_metrics():
    mgr = UnifiedBatchManager()
    mgr.get_processor("rest", MagicMock())
    metrics = mgr.get_all_metrics()
    assert "rest_default" in metrics
    assert isinstance(list(metrics.values())[0], BatchMetrics)


def test_manager_stop_all():
    mgr = UnifiedBatchManager()
    proc = mgr.get_processor("rest", MagicMock())
    proc.start()
    mgr.stop_all()
    assert mgr._processors == {}


def test_manager_default_config():
    mgr = UnifiedBatchManager()
    assert isinstance(mgr.config, BatchConfig)


# --------------------------------------------------------------------------
# Helper functions / VectorBatcher
# --------------------------------------------------------------------------

def test_create_vector_batcher():
    client = MagicMock()
    vb = create_vector_batcher(client, "col", max_batch_size=42)
    assert isinstance(vb, VectorBatcher)
    assert vb.collection_id == "col"
    assert vb.config.max_batch_size == 42
    assert isinstance(vb.get_metrics(), BatchMetrics)


def test_batch_insert_records_with_insert_records():
    client = MagicMock()
    client.insert_records = MagicMock(return_value={"ok": 1})
    records = [{"id": i} for i in range(5)]
    out = batch_insert_records(client, "col", records, batch_size=2)
    # 5 records / batch 2 -> 3 batches
    assert len(out) == 3
    assert client.insert_records.call_count == 3


def test_batch_insert_records_fallback_insert_vectors():
    client = MagicMock(spec=["insert_vectors"])  # no insert_records attr
    client.insert_vectors = MagicMock(return_value={"ok": 1})
    records = [{"id": i} for i in range(3)]
    out = batch_insert_records(client, "col", records, batch_size=10)
    assert len(out) == 1
    client.insert_vectors.assert_called_once()


def test_batch_insert_vectors_alias():
    client = MagicMock()
    client.insert_records = MagicMock(return_value={"ok": 1})
    vecs = [VectorRecord(id=str(i), vector=[0.1, 0.2]) for i in range(2)]
    out = batch_insert_vectors(client, "col", vecs, batch_size=1)
    assert len(out) == 2


def test_batch_insert_records_empty():
    client = MagicMock()
    client.insert_records = MagicMock()
    out = batch_insert_records(client, "col", [], batch_size=10)
    assert out == []
    client.insert_records.assert_not_called()


# --------------------------------------------------------------------------
# Backward-compat aliases
# --------------------------------------------------------------------------

def test_compat_aliases():
    assert RequestBatcher is UnifiedBatchManager
    assert RestBatchProcessor is ThreadedBatchProcessor


def test_batch_processor_is_abstract():
    with pytest.raises(TypeError):
        BatchProcessor(BatchConfig())
