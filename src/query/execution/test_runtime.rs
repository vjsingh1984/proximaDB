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

use std::time::Duration;

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
    futures::executor::block_on(f)
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
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build current_thread tokio runtime for test");
    rt.block_on(async {
        tokio::time::timeout(Duration::from_secs(timeout_secs), f)
            .await
            .expect("async test timed out — investigate for a genuine deadlock")
    })
}
