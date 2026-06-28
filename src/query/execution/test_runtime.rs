//! Async test execution strategies for streaming/hang-prone tests.
//!
//! **Problem (CLAUDE.md #11):** the `#[tokio::test]` multi-threaded runtime
//! intermittently hangs under CI load — worker-thread scheduling races surface
//! as indefinite hangs in tests that run late in the ~8290-test sequence
//! (streaming, DataFusion I/O, durable-catalog boot). The hang is in the
//! runtime infrastructure, not in the test logic.
//!
//! **Solution (SOLID):** separate the async execution strategy from the test
//! logic via two helpers. Each test picks the minimal strategy it needs:
//!
//! - [`run_sync`] — for tests whose async code is purely synchronous-in-async
//!   (no real `.await` yield points — e.g. Volcano `ValuesExec`). Uses
//!   `futures::executor::block_on`. **Root-cause fix:** no runtime → no
//!   worker threads → no scheduling race → no hang. Deterministic.
//!
//! - [`run_with_timeout`] — for tests that genuinely need tokio's async
//!   runtime (DataFusion I/O, timers, durable catalog). Uses a
//!   `current_thread` runtime (no worker-thread join-at-shutdown deadlock) +
//!   a bounded timeout (fails fast if a residual hang occurs for another
//!   reason). **Mitigation + isolation:** eliminates the multi-threaded race
//!   while preserving tokio semantics, and bounds any residual hang.
//!
//! Tests use these via `#[test] fn` instead of `#[tokio::test] async fn`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Hard wall-clock cap for hang-prone async tests. Well above their real runtime
/// (<1s) but far below nextest's 120s slow-timeout, so a genuine hang fails the
/// test fast and clearly instead of stalling the whole unit job through nextest's
/// slow-timeout + retry cycle (the measured ~20-min CI tax behind CLAUDE.md #11).
const TEST_WATCHDOG_TIMEOUT: Duration = Duration::from_secs(30);

/// Drive `body` (a closure that runs the test to completion on THIS thread) under
/// an independent watchdog that bounds wall-clock time **even when the work blocks
/// its thread** — a non-async deadlock (e.g. a re-entrant `std::sync::Mutex` in the
/// Volcano executor) that an in-runtime `tokio::time::timeout` cannot interrupt,
/// because the single runtime thread is the one that is wedged.
///
/// The work stays on the calling thread (so test futures need not be `Send`/
/// `'static` — borrows are fine). A separate watchdog thread holds only an
/// `Arc<AtomicBool>`; if `body` has not signalled completion within
/// [`TEST_WATCHDOG_TIMEOUT`], the watchdog aborts the process. Under nextest's
/// process-per-test isolation that fails ONLY this test (fast), instead of letting
/// it hang to the 120s slow-timeout and be retried.
fn with_watchdog<T>(body: impl FnOnce() -> T) -> T {
    let done = Arc::new(AtomicBool::new(false));
    let watchdog_done = done.clone();
    let watchdog = std::thread::Builder::new()
        .name("async-test-watchdog".to_string())
        .spawn(move || {
            let deadline = TEST_WATCHDOG_TIMEOUT;
            let step = Duration::from_millis(50);
            let mut waited = Duration::ZERO;
            while waited < deadline {
                if watchdog_done.load(Ordering::Acquire) {
                    return;
                }
                std::thread::sleep(step);
                waited += step;
            }
            if !watchdog_done.load(Ordering::Acquire) {
                eprintln!(
                    "FATAL: async test exceeded {TEST_WATCHDOG_TIMEOUT:?} — a streaming/runtime \
                     deadlock (CLAUDE.md #11). Aborting to fail fast instead of hanging the unit \
                     job (safe under nextest process-per-test isolation)."
                );
                std::process::abort();
            }
        })
        .expect("spawn async-test watchdog thread");

    let result = body();
    done.store(true, Ordering::Release);
    let _ = watchdog.join();
    result
}

/// Run a synchronous-in-async test body without a tokio runtime.
///
/// Use for tests whose async code has no real yield points (e.g. Volcano
/// `ValuesExec::next_row` — all computation, no I/O, no timers). The
/// `futures::executor::block_on` driver polls the future to completion on
/// the first poll — it can never hang because there are no `Poll::Pending`
/// returns.
pub fn run_sync<F, T>(f: F) -> T
where
    F: std::future::Future<Output = T>,
{
    // The "no yield points → can't hang" assumption did NOT hold for the Volcano
    // streaming path (`native_volcano_stream_*` intermittently hung past 120s in
    // CI): `try_unfold` + the executor's `std::sync::Mutex` object pools can
    // deadlock the single block_on thread. The watchdog bounds that to a fast,
    // clear failure.
    with_watchdog(|| futures::executor::block_on(f))
}

/// Run an async test body on a single-threaded tokio runtime with a bounded
/// timeout.
///
/// Use for tests that need tokio's async infrastructure (DataFusion object
/// store I/O, timers, `tokio::spawn`) but are prone to the multi-threaded
/// runtime's intermittent hang under CI load.
///
/// **Why `current_thread`:** the multi-threaded runtime intermittently hangs
/// because its worker-thread join-at-shutdown deadlocks under CI scheduling
/// pressure. `current_thread` has no worker threads — the runtime shuts down
/// as soon as the test future completes, with no threads to join. DataFusion
/// and other tokio consumers work correctly on `current_thread` (it's a full
/// runtime with I/O + timers enabled — it just doesn't spawn worker threads).
///
/// **Why the timeout:** belt-and-suspenders. If the hang persists for a
/// reason unrelated to the runtime flavor (e.g. a genuine deadlock in
/// DataFusion internals), the timeout converts the indefinite hang into a
/// deterministic, fast failure.
pub fn run_with_timeout<F, T>(timeout_secs: u64, f: F) -> T
where
    F: std::future::Future<Output = T>,
{
    // The in-runtime `tokio::time::timeout` only fires for an *async* stall; if the
    // future BLOCKS the current_thread runtime's only thread (a sync deadlock), the
    // timer can never run and the 30s cap never triggers — which is exactly how the
    // streaming/DataFusion tests rode to nextest's 120s slow-timeout. Keep the inner
    // tokio timeout as the graceful path, and wrap the whole thing in the watchdog
    // so a thread-blocking hang is still bounded (via abort) rather than indefinite.
    with_watchdog(|| {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build current_thread tokio runtime for test");
        rt.block_on(async {
            tokio::time::timeout(Duration::from_secs(timeout_secs), f)
                .await
                .expect("async test timed out — investigate for a genuine deadlock")
        })
    })
}
