// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! TD-LIFECYCLE-1 durable fix — the process shutdown registry.
//!
//! Always-on background loops (discovery executor, recall observer, drift
//! watcher/observer, AXIS EventLog consumer) used to `std::mem::forget` their
//! `watch::Sender<bool>` shutdown handles, so a clean SIGTERM shutdown could
//! never stop them: `Runtime::drop` then blocked forever on any in-flight
//! blocking pass (sampled evidence in the TD). Loops now REGISTER their
//! sender here instead; `ProximaDB::shutdown` fires them all before stopping
//! the servers, so every cooperative loop exits its next `select!` poll.
//!
//! `runtime.shutdown_timeout(5s)` in `main` stays as defense-in-depth for a
//! loop that ignores its signal mid-pass.

use std::sync::Mutex;

use tokio::sync::watch;

static SENDERS: Mutex<Vec<(&'static str, watch::Sender<bool>)>> = Mutex::new(Vec::new());

/// Register a background loop's shutdown sender under a diagnostic name.
/// Keeps the sender alive for the process lifetime (the old `mem::forget`
/// intent) while remaining reachable at shutdown.
pub fn register(name: &'static str, sender: watch::Sender<bool>) {
    if let Ok(mut v) = SENDERS.lock() {
        v.push((name, sender));
    }
}

/// Fire every registered shutdown signal. Idempotent; called from
/// `ProximaDB::shutdown` before server stop. Returns how many loops were
/// signaled.
pub fn fire_all() -> usize {
    let Ok(senders) = SENDERS.lock() else {
        return 0;
    };
    let mut fired = 0;
    for (name, tx) in senders.iter() {
        // send fails only when every receiver is gone — the loop already
        // exited; count only live signals.
        if tx.send(true).is_ok() {
            fired += 1;
            tracing::debug!(loop_name = name, "shutdown signal fired");
        }
    }
    fired
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TD-LIFECYCLE-1: a registered loop observes the fired signal (the exact
    /// select!-based contract the background loops use), and fire_all counts
    /// only loops that were still alive.
    #[tokio::test]
    async fn registered_loops_observe_fire_all() {
        let (tx, mut rx) = watch::channel(false);
        register("test-loop", tx);
        let observer = tokio::spawn(async move {
            loop {
                if rx.changed().await.is_err() || *rx.borrow() {
                    return true;
                }
            }
        });
        assert!(fire_all() >= 1, "at least the registered loop must fire");
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(2), observer)
                .await
                .expect("loop must exit after fire_all")
                .expect("join")
        );
    }
}
